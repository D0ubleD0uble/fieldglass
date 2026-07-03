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
# ECMWF open data, ECCC datamart). Large files are trimmed to a single GRIB2
# message via their .idx/.index sidecar and an HTTP Range request, so nothing
# here downloads more than a few MB.
#
# Usage:
#   tools/fetch_samples.sh                 # all freely-available models
#   tools/fetch_samples.sh gfs hrrr goes   # only the named ones
#   DATE=20260629 CYCLE=00 tools/fetch_samples.sh   # pin a run (see note below)
#
# Run availability: defaults to yesterday's 00Z, which is safe for the NCEP/ECMWF
# models. MRMS, NBM, and ECCC keep only ~1-2 days of data — if one 404s, re-run
# that model with today's DATE.
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="$REPO/samples"
mkdir -p "$OUT"

DATE="${DATE:-$(date -u -d yesterday +%Y%m%d)}"
CYCLE="${CYCLE:-00}"
YEAR="${DATE:0:4}"
DOY="$(date -u -d "${DATE}" +%j)"

info() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
ok()   { printf '\033[1;32m ok\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m  ! \033[0m %s\n' "$*" >&2; }

# Extract the first GRIB2 message matching a regex from a wgrib2-style .idx,
# via a byte-range request. $1 grib url, $2 output path, $3 idx line regex.
extract_wgrib2() {
  local url="$1" out="$2" pat="$3" idx ln start next range
  idx="$(curl -fsSL "${url}.idx" 2>/dev/null)" || { warn "no .idx for $out"; return 1; }
  ln="$(printf '%s\n' "$idx" | grep -n -m1 -E "$pat" | cut -d: -f1)" \
    || { warn "no field matching '$pat' for $out"; return 1; }
  start="$(printf '%s\n' "$idx" | sed -n "${ln}p"      | cut -d: -f2)"
  next="$(printf '%s\n'  "$idx" | sed -n "$((ln+1))p"  | cut -d: -f2)"
  if [ -n "$next" ]; then range="${start}-$((next-1))"; else range="${start}-"; fi
  curl -fsSL -r "$range" "$url" -o "$out" && ok "$out ($(du -h "$out" | cut -f1))"
}

# Extract one message from an ECMWF open-data file via its JSON-lines .index.
# $1 grib url, $2 output path, $3 param short-name (e.g. 2t).
extract_ecmwf() {
  local url="$1" out="$2" param="$3" rec off len
  rec="$(curl -fsSL "${url%.grib2}.index" 2>/dev/null | grep -m1 "\"param\": \"$param\"")" \
    || { warn "no param '$param' in ECMWF index"; return 1; }
  off="$(printf '%s' "$rec" | grep -o '"_offset": *[0-9]*' | grep -o '[0-9]*')"
  len="$(printf '%s' "$rec" | grep -o '"_length": *[0-9]*' | grep -o '[0-9]*')"
  if [ -z "$off" ] || [ -z "$len" ]; then warn "bad ECMWF index record"; return 1; fi
  curl -fsSL -r "${off}-$((off+len-1))" "$url" -o "$out" && ok "$out ($(du -h "$out" | cut -f1))"
}

get_gfs() { # complex+spatial-diff (5.3), regular lat/lon (3.0) — global
  info "GFS 2m temperature (NOMADS filter, global 0.25°)"
  local u="https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl"
  u+="?dir=/gfs.${DATE}/${CYCLE}/atmos&file=gfs.t${CYCLE}z.pgrb2.0p25.f000"
  u+="&var_TMP=on&lev_2_m_above_ground=on&leftlon=0&rightlon=360&toplat=90&bottomlat=-90"
  if curl -fsSL "$u" -o "$OUT/gfs.grib2"; then
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
  key="$(curl -fsSL "https://noaa-mrms-pds.s3.amazonaws.com/?list-type=2&prefix=CONUS/MergedReflectivityQCComposite_00.50/${DATE}/&max-keys=1000" \
    | grep -o '<Key>[^<]*</Key>' | sed 's/<[^>]*>//g' | tail -1)" || { warn "MRMS listing failed"; return 1; }
  [ -n "$key" ] || { warn "no MRMS object for ${DATE}"; return 1; }
  if curl -fsSL "https://noaa-mrms-pds.s3.amazonaws.com/${key}" -o "$OUT/mrms.grib2.gz"; then
    gunzip -f "$OUT/mrms.grib2.gz" && ok "mrms.grib2 ($(du -h "$OUT/mrms.grib2" | cut -f1))"
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
  if curl -fsSL "${base}/${file}" -o "$OUT/eccc.grib2"; then
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
  local hh prefix
  hh="$(printf '%02d' 10#"$CYCLE")"
  prefix="ABI-L2-CMIPM/${YEAR}/${DOY}/${hh}/OR_ABI-L2-CMIPM1-M6C13"
  local list="https://noaa-goes19.s3.amazonaws.com/?list-type=2&prefix=${prefix}&max-keys=5"
  local key
  key="$(curl -fsSL "$list" | grep -o '<Key>[^<]*</Key>' | head -1 | sed 's/<[^>]*>//g')" \
    || { warn "GOES prefix listing failed"; return 1; }
  [ -n "$key" ] || { warn "no C13 file under $prefix"; return 1; }
  curl -fsSL "https://noaa-goes19.s3.amazonaws.com/${key}" -o "$OUT/goes.nc" \
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
  if curl -fsSL "$url" -o "$OUT/oisst.nc"; then
    ok "oisst.nc ($(du -h "$OUT/oisst.nc" | cut -f1))"
  else
    warn "OISST fetch failed"
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

ALL=(gfs hrrr nam rap nbm mrms ecmwf eccc goes oisst wrf)
targets=("$@"); [ ${#targets[@]} -eq 0 ] && targets=("${ALL[@]}")

info "run: ${DATE} ${CYCLE}Z  ->  $OUT"
for t in "${targets[@]}"; do
  if declare -f "get_$t" >/dev/null; then "get_$t"; else warn "unknown model: $t"; fi
done
info "done. Open a file with:  code --extensionDevelopmentPath=\"$REPO/extension\" \"$OUT/<file>\""
info "See samples/README.md for the per-model verification checklist."
