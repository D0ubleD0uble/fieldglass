#!/usr/bin/env bash
#
# Fetch real, full operational files for manual render testing (issue #123).
#
# These are the visual counterpart to the tiny oracle-checked fixtures under
# crates/*/tests/fixtures/: full real files you open in the extension to confirm
# a whole render looks right (coastlines land, colorbar in real units, correct
# framing). Everything lands in samples/, which is git-ignored except its README.
#
# All endpoints are public and need NO credentials (NOAA NOMADS / AWS Open Data,
# ECMWF open data and eccodes test data, ECCC datamart, DWD opendata, the HDF5 repository on GitHub). Large
# files are trimmed to a single GRIB2 message via their .idx/.index sidecar and
# an HTTP Range request, so nothing here downloads more than a few MB.
#
# Usage:
#   tools/fetch_samples.sh                 # all freely-available models
#   tools/fetch_samples.sh gfs hrrr goes   # only the named ones
#   DATE=20260629 CYCLE=00 tools/fetch_samples.sh   # pin a run (see note below)
#   SAMPLES_OUT=/some/dir tools/fetch_samples.sh   # write somewhere else than samples/
#
# DATE is YYYYMMDD and CYCLE an hour (6 and 06 both work); a bad one stops the
# run instead of putting a hole in fourteen URLs.
#
# Run availability: defaults to yesterday's 00Z, which is safe for the NCEP/ECMWF
# models. MRMS, NBM, ECCC and ICON keep only ~1-2 days of data — if one 404s,
# re-run that model with today's DATE.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${SAMPLES_OUT:-$REPO/samples}"
mkdir -p "$OUT"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m ok\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m  ! \033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31merror\033[0m %s\n' "$*" >&2; exit 2; }

# GNU coreutils and BSD/macOS `date` take incompatible flags for both things
# this script needs — "one day ago" and "parse this YYYYMMDD" — so pick the
# pair up front. Deliberately not a `gnu_form || bsd_form` fallback chain:
# BSD `date` reads the argument of -d as a daylight-saving flag and then still
# prints *today*, so the chain would succeed and hand every URL a plausible
# wrong date. Only GNU date answers `--version`, and asking costs nothing.
if date --version >/dev/null 2>&1; then
  utc_yesterday() { date -u -d yesterday +%Y%m%d; }
  day_of_year()   { date -u -d "$1" +%j; }
else
  utc_yesterday() { date -u -v-1d +%Y%m%d; }
  day_of_year()   { date -u -j -f %Y%m%d "$1" +%j; }
fi

DATE="${DATE:-$(utc_yesterday)}"
CYCLE="${CYCLE:-00}"

# Validate before fetching rather than after. An unusable DATE used to leave a
# hole in every URL and fail all fourteen models one confusing 404 at a time;
# the only tell was the blank in the run banner.
case "$DATE" in
  [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]) ;;
  *) die "DATE must be YYYYMMDD (got '${DATE}'). Pass one explicitly: DATE=$(date -u +%Y%m%d) tools/fetch_samples.sh" ;;
esac
case "$CYCLE" in
  ''|*[!0-9]*) die "CYCLE must be an hour 0-23 (got '${CYCLE}')" ;;
esac
[ "$((10#$CYCLE))" -le 23 ] || die "CYCLE must be an hour 0-23 (got '${CYCLE}')"

# Normalize 6 -> 06 once, here, so every model's URL gets the padded form.
# `10#` is why this is arithmetic expansion and not `printf '%02d' 10#$CYCLE`:
# bash's printf rejects a based literal outright ("10#00: invalid number").
CYCLE="$(printf '%02d' "$((10#$CYCLE))")"

YEAR="${DATE:0:4}"
DOY="$(day_of_year "$DATE")" || die "could not read a day-of-year out of DATE=${DATE}"
[ -n "$DOY" ] || die "could not read a day-of-year out of DATE=${DATE}"

# Every request gives up on a stalled connection rather than hanging the run:
# 30 s to connect, and under 1 KB/s for a minute. There is no cap on the total
# time, so a slow download that keeps moving still finishes.
curl_get() { curl -fsSL --connect-timeout 30 --speed-limit 1024 --speed-time 60 "$@"; }

# Download to a file, or leave no file: curl writes as it receives, so a
# transfer that fails midway would otherwise leave a truncated file that looks
# fetched. $1 output path, then curl's arguments.
download() {
  local out="$1"; shift
  curl_get "$@" -o "$out" || { rm -f "$out"; return 1; }
}

# Under pipefail, never pipe a download into a reader that stops early
# (`grep -m1`, `head`): the writer is killed by SIGPIPE, the pipeline fails, and
# a file that is there is reported missing. The ECMWF index outgrew the pipe
# buffer and did exactly that (#1031). So every listing or index is read into a
# variable first and searched from a here-string.

# Extract the first GRIB2 message matching a regex from a wgrib2-style .idx,
# via a byte-range request. $1 grib url, $2 output path, $3 idx line regex.
extract_wgrib2() {
  local url="$1" out="$2" pat="$3" idx ln start next range
  idx="$(curl_get "${url}.idx" 2>/dev/null)" || { warn "no .idx for $out"; return 1; }
  ln="$(grep -n -m1 -E "$pat" <<<"$idx" | cut -d: -f1)" \
    || { warn "no field matching '$pat' for $out"; return 1; }
  start="$(printf '%s\n' "$idx" | sed -n "${ln}p"      | cut -d: -f2)"
  next="$(printf '%s\n'  "$idx" | sed -n "$((ln+1))p"  | cut -d: -f2)"
  if [ -n "$next" ]; then range="${start}-$((next-1))"; else range="${start}-"; fi
  download "$out" -r "$range" "$url" && ok "$out ($(du -h "$out" | cut -f1))"
}

# Extract one message from an ECMWF open-data file via its JSON-lines .index.
# $1 grib url, $2 output path, $3 param short-name (e.g. 2t).
extract_ecmwf() {
  local url="$1" out="$2" param="$3" idx rec off len
  idx="$(curl_get "${url%.grib2}.index" 2>/dev/null)" || { warn "no ECMWF index for $out"; return 1; }
  rec="$(grep -m1 "\"param\": \"$param\"" <<<"$idx")" \
    || { warn "no param '$param' in ECMWF index"; return 1; }
  off="$(printf '%s' "$rec" | grep -o '"_offset": *[0-9]*' | grep -o '[0-9]*')"
  len="$(printf '%s' "$rec" | grep -o '"_length": *[0-9]*' | grep -o '[0-9]*')"
  if [ -z "$off" ] || [ -z "$len" ]; then warn "bad ECMWF index record"; return 1; fi
  download "$out" -r "${off}-$((off+len-1))" "$url" && ok "$out ($(du -h "$out" | cut -f1))"
}

get_gfs() { # complex+spatial-diff (5.3), regular lat/lon (3.0) — global
  info "GFS 2m temperature (NOMADS filter, global 0.25°)"
  local u="https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl"
  u+="?dir=/gfs.${DATE}/${CYCLE}/atmos&file=gfs.t${CYCLE}z.pgrb2.0p25.f000"
  u+="&var_TMP=on&lev_2_m_above_ground=on&leftlon=0&rightlon=360&toplat=90&bottomlat=-90"
  if download "$OUT/gfs.grib2" "$u"; then
    ok "gfs.grib2 ($(du -h "$OUT/gfs.grib2" | cut -f1))"
  else
    warn "GFS fetch failed (run may not be posted yet)"
  fi
}

get_hrrr() { # complex + spatial-diff (5.3), Lambert (3.30) — CONUS
  info "HRRR surface temperature (byte-range from wrfsfc)"
  extract_wgrib2 \
    "https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.${DATE}/conus/hrrr.t${CYCLE}z.wrfsfcf00.grib2" \
    "$OUT/hrrr.grib2" ":TMP:surface:"
}

get_nam() { # complex / JPEG 2000, Lambert (3.30) — CONUS
  info "NAM surface temperature (byte-range from awphys)"
  extract_wgrib2 \
    "https://noaa-nam-pds.s3.amazonaws.com/nam.${DATE}/nam.t${CYCLE}z.awphys00.tm00.grib2" \
    "$OUT/nam.grib2" ":TMP:surface:"
}

get_rap() { # JPEG 2000 (5.40), Lambert (3.30) — CONUS
  info "RAP surface temperature (byte-range from awp130)"
  extract_wgrib2 \
    "https://noaa-rap-pds.s3.amazonaws.com/rap.${DATE}/rap.t${CYCLE}z.awp130pgrbf00.grib2" \
    "$OUT/rap.grib2" ":TMP:surface:"
}

get_nbm() { # complex + spatial-diff (5.3, inline missing values), Lambert (3.30) — CONUS (NOMADS; ~2-day retention)
  info "NBM 2m temperature (byte-range from NOMADS core.co)"
  extract_wgrib2 \
    "https://nomads.ncep.noaa.gov/pub/data/nccf/com/blend/prod/blend.${DATE}/${CYCLE}/core/blend.t${CYCLE}z.core.f001.co.grib2" \
    "$OUT/nbm.grib2" ":TMP:2 m above ground:"
}

get_mrms() { # PNG (5.41), regular lat/lon (3.0) — CONUS reflectivity
  info "MRMS composite reflectivity (AWS bucket, newest object of the day)"
  # The NOMADS `latest` symlink streams and often truncates; the AWS Open Data
  # bucket is stable. List the day's prefix and take the newest key.
  local key
  key="$(curl_get "https://noaa-mrms-pds.s3.amazonaws.com/?list-type=2&prefix=CONUS/MergedReflectivityQCComposite_00.50/${DATE}/&max-keys=1000" \
    | grep -o '<Key>[^<]*</Key>' | sed 's/<[^>]*>//g' | tail -1)" || { warn "MRMS listing failed"; return 1; }
  [ -n "$key" ] || { warn "no MRMS object for ${DATE}"; return 1; }
  if download "$OUT/mrms.grib2.gz" "https://noaa-mrms-pds.s3.amazonaws.com/${key}"; then
    if gunzip -f "$OUT/mrms.grib2.gz"; then
      ok "mrms.grib2 ($(du -h "$OUT/mrms.grib2" | cut -f1))"
    else
      warn "MRMS file did not decompress"; rm -f "$OUT/mrms.grib2" "$OUT/mrms.grib2.gz"
    fi
  else
    warn "MRMS fetch failed"
  fi
}

get_ecmwf() { # CCSDS / AEC (5.42), regular lat/lon (3.0) — global
  info "ECMWF IFS 2m temperature (byte-range from open-data .index)"
  local stream="oper"; [ "$CYCLE" = "06" ] || [ "$CYCLE" = "18" ] && stream="scda"
  extract_ecmwf \
    "https://data.ecmwf.int/forecasts/${DATE}/${CYCLE}z/ifs/0p25/${stream}/${DATE}${CYCLE}0000-0h-${stream}-fc.grib2" \
    "$OUT/ecmwf.grib2" "2t"
}

get_eccc() { # rotated lat/lon (3.1) — HRDPS continental (datamart; short retention)
  info "ECCC HRDPS 2m temperature (rotated grid, whole file)"
  local hhh=001
  local base="https://dd.weather.gc.ca/${DATE}/WXO-DD/model_hrdps/continental/2.5km/${CYCLE}/${hhh}"
  local file="${DATE}T${CYCLE}Z_MSC_HRDPS_TMP_AGL-2m_RLatLon0.0225_PT${hhh}H.grib2"
  if download "$OUT/eccc.grib2" "${base}/${file}"; then
    ok "eccc.grib2 ($(du -h "$OUT/eccc.grib2" | cut -f1))"
  else
    warn "ECCC fetch failed (datamart retention is ~1-2 days; try today's DATE)"
  fi
}

get_goes() { # NetCDF-4, geostationary — mesoscale band 13
  info "GOES-19 ABI L2 CMIP mesoscale band 13 (list prefix, then fetch)"
  # S3 lists lexicographically, and the band token (M6C13) sorts before the
  # timestamp — so the band must be in the prefix or a page of keys never
  # reaches band 13. Target sector M1, band 13 directly.
  local prefix="ABI-L2-CMIPM/${YEAR}/${DOY}/${CYCLE}/OR_ABI-L2-CMIPM1-M6C13"
  local list="https://noaa-goes19.s3.amazonaws.com/?list-type=2&prefix=${prefix}&max-keys=5"
  local listing key
  listing="$(curl_get "$list")" || { warn "GOES prefix listing failed"; return 1; }
  key="$(grep -o '<Key>[^<]*</Key>' <<<"$listing" | sed 's/<[^>]*>//g' | sed -n 1p)"
  [ -n "$key" ] || { warn "no C13 file under $prefix"; return 1; }
  download "$OUT/goes.nc" "https://noaa-goes19.s3.amazonaws.com/${key}" \
    && ok "goes.nc ($(du -h "$OUT/goes.nc" | cut -f1))"
}

get_oisst() { # NetCDF-4, regular 1/4° global lat/lon — real full OISST analysis
  info "NOAA OISST v2.1 daily analysis (NOAA CDR archive, full real file)"
  # A pinned date rather than \$DATE: the CDR archive's recent days are
  # preliminary/late-arriving, while this object is final and immutable — the
  # same one the committed oisst_avhrr_v2.nc test fixture was subset from
  # (provenance in crates/fieldglass-netcdf/tests/fixtures/NOTICE.md). NOAA
  # CDR output is a U.S. Government work in the public domain. ~1.6 MB.
  local url="https://noaa-cdr-sea-surface-temp-optimum-interpolation-pds.s3.amazonaws.com/data/v2.1/avhrr/202501/oisst-avhrr-v02r01.20250101.nc"
  if download "$OUT/oisst.nc" "$url"; then
    ok "oisst.nc ($(du -h "$OUT/oisst.nc" | cut -f1))"
  else
    warn "OISST fetch failed"
  fi
}

get_icon() { # simple packing (5.0), regular lat/lon (3.0) — DWD ICON-D2, local params
  info "DWD ICON-D2 modified cloud cover (opendata.dwd.de, regular lat/lon)"
  # The reason this one is here is §1: center 78 with localTablesVersion 1, on
  # a local triple (0/6/199) that only the DWD table names (#425). Open a
  # different parameter and the message table shows what the WMO master set
  # gives you instead. DWD open data is free to use with attribution
  # (GeoNutzV); nothing from it is committed.
  local base="https://opendata.dwd.de/weather/nwp/icon-d2/grib/${CYCLE}"
  local name="icon-d2_germany_regular-lat-lon_single-level_${DATE}${CYCLE}_000_2d_clct_mod.grib2.bz2"
  if download "$OUT/icon.grib2.bz2" "${base}/clct_mod/${name}" \
     && bunzip2 -f "$OUT/icon.grib2.bz2"; then
    ok "icon.grib2 ($(du -h "$OUT/icon.grib2" | cut -f1))"
  else
    warn "ICON-D2 fetch failed (opendata.dwd.de keeps only ~24h; try today's DATE)"
    rm -f "$OUT/icon.grib2.bz2" "$OUT/icon.grib2"
  fi
}

get_wrf() { # NetCDF classic, WRF Lambert (MAP_PROJ attrs) — synthetic stand-in
  info "WRF wrfout stand-in (copied from the committed test fixture)"
  # Real wrfout files are self-generated model output with no public
  # no-credential endpoint, so this stages the repo's tiny (6×5) synthetic
  # wrfout-style fixture. It exercises the WRF-attribute + Lambert render
  # path end to end, but it's too small for a meaningful coastline check —
  # drop a real wrfout into samples/ by hand for that (see samples/README.md).
  if cp "$REPO/crates/fieldglass-netcdf/tests/fixtures/wrf_lambert.nc" "$OUT/wrf.nc"; then
    ok "wrf.nc (synthetic 6×5 stand-in — drop in a real wrfout for visual checks)"
  else
    warn "WRF fixture copy failed"
  fi
}

get_rtofs() { # NetCDF-4, tripolar curvilinear (2-D lat/lon, no grid mapping)
  info "RTOFS global ocean ice (tripolar curvilinear)"
  # The committed fixture is a 200x260 window of the Arctic fold; this is the
  # whole 3298x4500 grid, which is what a coastline check needs. NOMADS keeps
  # only a few days, so this uses the AWS mirror's dated archive instead.
  local url="https://noaa-nws-rtofs-pds.s3.amazonaws.com/rtofs.${RTOFS_DATE:-20240201}/rtofs_glo_2ds_n000_ice.nc"
  if download "$OUT/rtofs_ice.nc" "$url"; then
    ok "rtofs_ice.nc ($(du -h "$OUT/rtofs_ice.nc" | cut -f1))"
  else
    warn "RTOFS fetch failed (set RTOFS_DATE to a day the AWS mirror holds)"
    rm -f "$OUT/rtofs_ice.nc"
  fi
}

get_mirs() { # NetCDF-4, satellite swath (per-pixel 2-D lat/lon)
  info "NOAA-21 MiRS imagery granule (swath curvilinear)"
  # A full 33-minute granule: 768 scanlines by 96 fields of view, a complete
  # half orbit. The committed fixture is 100 scanlines of it. The bucket keeps
  # only recent days, so list the run day's prefix and take its first granule.
  local base="https://noaa-nesdis-n21-pds.s3.amazonaws.com"
  local listing name
  listing="$(curl_get "$base/?list-type=2&prefix=NPR_MIRS_IMG_33min/${DATE:0:4}/${DATE:4:2}/${DATE:6:2}/&max-keys=1")" \
    || { warn "MiRS listing failed"; return 1; }
  name="$(grep -o '<Key>[^<]*</Key>' <<<"$listing" | sed 's/<[^>]*>//g' | sed -n 1p)"
  [ -n "$name" ] || { warn "no MiRS granule for ${DATE}"; return 1; }
  if download "$OUT/mirs_swath.nc" "$base/$name"; then
    ok "mirs_swath.nc ($(du -h "$OUT/mirs_swath.nc" | cut -f1))"
  else
    warn "MiRS fetch failed"
    rm -f "$OUT/mirs_swath.nc"
  fi
}

get_reduced() { # reduced grids: O1280, reduced lat/lon, and the coarse N32/O32 — eccodes test data + fixtures
  info "ECMWF reduced grids (eccodes test data: octahedral O1280, reduced lat/lon wave field, N32; fixtures: N32, O32)"
  # ECMWF open data is interpolated to regular grids, so no forecast feed has a
  # reduced one. These come from the corpus eccodes' own test suite downloads,
  # which keeps each file under a fixed name. The md5 of each is pinned here,
  # taken from the .md5 file published beside it on 2026-11-09, so a file that
  # changes on the server is noticed rather than swept. It is downloaded to a
  # temporary name and moved into place only when it matches, so a failed
  # re-fetch leaves a copy an earlier run verified. eccodes and its test data
  # are Apache 2.0. Nothing from here is committed.
  #   octahedral_o1280.grib1  msl.octa.glob.grib1: mean sea level pressure on
  #                           ECMWF's operational O1280 grid (2560 rows,
  #                           6,599,680 points). The field is constant (0 bits
  #                           per value), so it checks the cell, not the value.
  #   reduced_ll_wave.grib1   reduced_latlon_surface.grib1: significant wave
  #                           height on a 0.36° reduced lat/lon grid, with a
  #                           bitmap over land and empty rows over the poles.
  #   reduced_gaussian_surface.grib2
  #                           the same name: a classic reduced Gaussian N32
  #                           surface field.
  # Coarse grids are where a pixel's nearest point is most often not the one
  # its widened column shows (#1759), so the two committed coarse fixtures
  # are copied in beside it: reduced_gg_n32_smooth.grib1 (N32, GRIB1) and
  # octahedral_gaussian_o32.grib2 (O32). Their provenance is in the format
  # crates' tests/fixtures/NOTICE.md.
  local fixture
  for fixture in \
    fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1 \
    fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2; do
    if cp "$REPO/crates/$fixture" "$OUT/"; then
      ok "${fixture##*/} (committed fixture)"
    else
      warn "${fixture##*/} copy failed"
    fi
  done
  local base="https://get.ecmwf.int/test-data/eccodes/data"
  local entry src dst want have tmp
  for entry in \
    msl.octa.glob.grib1:octahedral_o1280.grib1:9e23fe6ab7d88782f0cea528ab53bdda \
    reduced_latlon_surface.grib1:reduced_ll_wave.grib1:fdc00de45d0cae31810d2cdc4e46e1e8 \
    reduced_gaussian_surface.grib2:reduced_gaussian_surface.grib2:13b657181f58cc509b99717f254ce4aa; do
    IFS=: read -r src dst want <<<"$entry"
    tmp="$OUT/.$dst.part"
    if ! download "$tmp" "$base/$src"; then
      warn "$src fetch failed"; rm -f "$tmp"; continue
    fi
    if command -v md5sum >/dev/null 2>&1; then have="$(md5sum "$tmp" | cut -d' ' -f1)"; else have="$(md5 -q "$tmp")"; fi
    if [ "$want" != "$have" ]; then
      warn "$src does not match its pinned checksum ($want, got $have); not saved"; rm -f "$tmp"; continue
    fi
    mv -f "$tmp" "$OUT/$dst"
    ok "$dst ($(du -h "$OUT/$dst" | cut -f1))"
  done
}

get_szip2() { # HDF5 szip written by the HDF Group's SZIP 2.x, not libaec — HDF5 test files
  info "HDF5 szip from the SZIP 2.x encoder (HDF5 repository test files, 2006 and 2011)"
  # Every szip file the fixtures build is written by libaec. These two were
  # written before libaec existed, by libhdf5 linked against the HDF Group's
  # SZIP, and check the 32-bit byte-plane end-of-stream rule against it (#820).
  # Both are committed as fixtures too (hdf5_szip2_*.h5, provenance in
  # crates/fieldglass-netcdf/tests/fixtures/NOTICE.md); this puts the originals
  # in samples/ to open by hand. Read from a pinned commit, and saved only when
  # the md5 matches. HDF5 and its test files are BSD-3-Clause.
  #   szip2_h5repack.h5  h5repack_szip.h5: int32 40x20, szip NN (2006)
  #   szip2_le_data.h5   le_data.h5: float32 7x6 szip, little- and big-endian
  #                      (2011), plus filters Fieldglass does not decode
  local base="https://raw.githubusercontent.com/HDFGroup/hdf5/348d507bdb77645f9f59f8d2b2e9bc3b276151a8"
  local entry src dst want have tmp
  for entry in \
    tools/test/testfiles/h5repack_szip.h5:szip2_h5repack.h5:971004382cbf83e96230f0279aabdc4e \
    test/testfiles/le_data.h5:szip2_le_data.h5:f5d55d75bf7903e28b2b03a646e0c200; do
    IFS=: read -r src dst want <<<"$entry"
    tmp="$OUT/.$dst.part"
    if ! download "$tmp" "$base/$src"; then
      warn "$src fetch failed"; rm -f "$tmp"; continue
    fi
    if command -v md5sum >/dev/null 2>&1; then have="$(md5sum "$tmp" | cut -d' ' -f1)"; else have="$(md5 -q "$tmp")"; fi
    if [ "$want" != "$have" ]; then
      warn "$src does not match its pinned checksum ($want, got $have); not saved"; rm -f "$tmp"; continue
    fi
    mv -f "$tmp" "$OUT/$dst"
    ok "$dst ($(du -h "$OUT/$dst" | cut -f1))"
  done
}

ALL=(gfs hrrr nam rap nbm mrms ecmwf eccc icon goes oisst wrf rtofs mirs reduced szip2)
targets=("$@"); [ ${#targets[@]} -eq 0 ] && targets=("${ALL[@]}")

info "run: ${DATE} ${CYCLE}Z (day ${DOY})  ->  $OUT"
for t in "${targets[@]}"; do
  if declare -f "get_$t" >/dev/null; then "get_$t"; else warn "unknown model: $t"; fi
done
info "done. Open a file with:  code --extensionDevelopmentPath=\"$REPO/extension\" \"$OUT/<file>\""
info "See samples/README.md for the per-model verification checklist."
