# Manual test plan — everything that changed since v0.6.1

**Baseline: v0.6.1 (2026-11-05).** This plan is rewritten each cycle to cover
what landed since the last tag, so it is deliberately near-empty right after a
release. Add a section as each user-facing feature merges, rather than trying
to reconstruct the cycle from the log at prep time. The 0.6.0 plan found three
defects that way, and all three were things a reader of the diff would not
have thought to look for.

`samples/README.md` has a per-file "does it look right" checklist for coverage
that shipped **at or before** 0.6.0. This plan does not repeat it beyond the
regression floor below.

---

## How to add to this plan

One section per feature, named for the issue. Under each, group checks by the
**file they need**, so a file is opened once and every check it can serve runs
together. For each check, say what to look at and what failure looks like.
"Renders correctly" is not a check anyone can fail honestly.

Before writing a check, **open the fixture and confirm it can show the thing**.
A constant field, a single-message file, or a domain that never reaches the
latitude in question will all pass a visual check while proving nothing.

Mark a check **priority** when it covers a path no automated test can judge:
something resampled, interpolated, or placed by a lookup rather than a formula.
Those are where the last cycle's defects were.

State explicitly where a check **cannot** be made by eye, and name the test that
covers it instead, rather than leaving a reader to attempt a judgment the data
cannot support.

A check marked **known** shows a gap that is already filed or reviewed and does
not block the release. Record what you see; a known check fails only if it
shows something other than what it describes.

---

## 0. Setup (once)

```sh
# From the repo root, at the commit being released (RELEASING.md's
# $RELEASE_SHA, the prep merge), not whatever master holds now. A pass that
# runs before the prep merge uses the tip of master instead.
git fetch origin && git checkout "$RELEASE_SHA"

# The napi CLI comes from the root package.json, so install it first.
npm ci --ignore-scripts

# Build the native module into the extension so the dev host runs current Rust,
# then compile the TypeScript.
( cd crates/fieldglass-napi && npx napi build --platform --release \
    --target x86_64-unknown-linux-gnu --output-dir "$(git rev-parse --show-toplevel)/extension/bin" )
( cd extension && npm ci && npm run compile )

# Full samples. If samples/ holds symlinks into another checkout
# (`find samples -maxdepth 1 -type l`), remove them first: the fetch writes
# through them. Remove a leftover samples/nam.grib2 too (NAM was retired on
# 2026-11-03; rrfs.grib2 replaced it).
tools/fetch_samples.sh

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js

# Placement: read 200 seeded points per message or slice through the native
# module and through eccodes / xarray, and compare the cell, the value and the
# mask. The preflight only proves a file draws; this proves it draws in the
# right place. Needs the pre-commit venv's python3 (eccodes, xarray, netCDF4,
# h5netcdf, zarr, scipy). About a quarter of an hour, most of it the RTOFS grid. Exit 0
# is clean; a FAILURES block is a decode or placement bug until shown to be the
# oracle's. The MRMS -999 sentinel, eccodes' j-consecutive lookup, its alternate-row
# values and its answer past a reduced grid's last row are listed in the output
# as documented exceptions, not failures.
( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py samples )
```

The script fetches yesterday's 00Z run. NBM and ICON keep only a day or two of
data (MRMS and ECCC are short too), so a model can fail with a 403 or a 404.
The ECMWF step prints `no param '2t'` when it cannot fetch the index at all,
not only when the parameter is missing. Re-run just that model with today's
date or a later cycle, for example `DATE=$(date -u +%Y%m%d) tools/fetch_samples.sh nbm icon`
and `CYCLE=12 tools/fetch_samples.sh ecmwf`.

Then `F5` to launch the dev host, and in it File > Open Folder > `samples`.

To drive the dev host headless or on a virtual display, set
`files.simpleDialog.enable: true` and `window.dialogStyle: custom` in its
settings first. Without them the native Export dialogs never appear.

Paths below are relative to the repo root. `F:` marks a committed crate fixture
(always present); `S:` marks a `samples/` file (needs the fetch above); `B:`
marks a file the next step builds into `samples/0.6.0/`.

### 0.6.1 beside it

Several checks below compare with what 0.6.1 showed for the same file. Install
the released 0.6.1 from the Marketplace into a user-data and extensions folder
of its own, so it does not replace anything in your usual setup, and open the
same folder there. `--profile` will not do on its own: `--install-extension`
fails with "Profile not found" for a profile that has never been opened.

```sh
code --user-data-dir ~/.fg061-userdata --extensions-dir ~/.fg061-ext \
    --install-extension fieldglass.fieldglass@0.6.1
code --user-data-dir ~/.fg061-userdata --extensions-dir ~/.fg061-ext \
    --disable-workspace-trust samples
```

Keep `--disable-workspace-trust`. Without it the window opens in Restricted
Mode, and 0.6.1 declares no support for untrusted workspaces, so every file
opens in the text editor.

If the Marketplace is out of reach, build the `v0.6.1` tag in a worktree
outside this checkout the way the setup above builds this one, and launch its
dev host instead.

### Build the files this cycle's checks need

Most committed GRIB files hold one message, and so does every file
`fetch_samples.sh` writes. That includes `cmc_wind_300_2010052400_p012.grib`,
which is wind *speed* at 300 hPa, not a u/v pair. Arrows, Compare on GRIB and
the reload checks all need two or more messages, and no committed NetCDF or Zarr
file has a real time or level axis to animate. This step builds those files
into `samples/0.6.0/`, which git ignores; the regression floor uses some of
them. It needs `curl`, and `xarray`, `zarr` 3 and `netCDF4` in the `python3`
you run it with. The pre-commit venv has them all. If yours lacks any, make a
throwaway environment for this step:
`python3 -m venv /tmp/fg-venv && /tmp/fg-venv/bin/pip install xarray 'zarr>=3' netCDF4`,
then run the Python steps below with `/tmp/fg-venv/bin/python` in place of
`python3`.

```sh
mkdir -p samples/0.6.0 && cd samples/0.6.0

# Real u/v pairs at 10 m: GFS states earth-relative components, HRRR grid-
# relative ones. NOMADS keeps a few days, so use yesterday's 00Z run.
D=$(date -u -d yesterday +%Y%m%d)
curl -fsSL -o gfs_wind.grib2 "https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25.pl?dir=/gfs.${D}/00/atmos&file=gfs.t00z.pgrb2.0p25.f000&var_UGRD=on&var_VGRD=on&lev_10_m_above_ground=on"
curl -fsSL -o hrrr_wind.grib2 "https://nomads.ncep.noaa.gov/cgi-bin/filter_hrrr_2d.pl?dir=/hrrr.${D}/conus&file=hrrr.t00z.wrfsfcf00.grib2&var_UGRD=on&var_VGRD=on&lev_10_m_above_ground=on"

# NCEP/NCAR reanalysis air temperature: 366 daily global fields for 2024, and
# 12 monthly means on 17 pressure levels. NetCDF-4, deflated, public domain.
curl -fsSL -o ncep_air_daily_2024.nc https://psl.noaa.gov/thredds/fileServer/Datasets/ncep.reanalysis/Dailies/surface/air.sig995.2024.nc
curl -fsSL -o ncep_air_levels_ltm.nc https://psl.noaa.gov/thredds/fileServer/Datasets/ncep.reanalysis/Monthlies/pressure/air.mon.ltm.1991-2020.nc

# The daily file again as Zarr, written by xarray: a consolidated v2 store
# and an unconsolidated v3 store.
python3 -c "
import xarray as xr
ds = xr.open_dataset('ncep_air_daily_2024.nc')
ds.to_zarr('air_v2.zarr', zarr_format=2, consolidated=True, mode='w')
ds.to_zarr('air_v3.zarr', zarr_format=3, consolidated=False, mode='w')
"

# A failed download can leave an HTML error page under the right name. This
# must print nothing.
for f in gfs_wind.grib2 hrrr_wind.grib2; do [ "$(head -c4 "$f")" = GRIB ] || echo "BAD $f"; done
python3 -c "
import netCDF4
for f in ['ncep_air_daily_2024.nc', 'ncep_air_levels_ltm.nc']:
    d = netCDF4.Dataset(f)
    assert d['air'].shape[0] in (366, 12), f
"
cd ../..

# Multi-message GRIB files made from committed fixtures, by rewriting a few
# octets. eccodes reads every one of them as the comments say, except the
# step of the two messages in time unit 20, which it cannot give.
python3 - <<'EOF'
from pathlib import Path

out = Path("samples/0.6.0")
g1 = Path("crates/fieldglass-grib1/tests/fixtures")
g2 = Path("crates/fieldglass-grib2/tests/fixtures")


def section(msg, number):
    at = 16
    while msg[at:at + 4] != b"7777":
        if msg[at + 4] == number:
            return at
        at += int.from_bytes(msg[at:at + 4], "big")
    raise SystemExit(f"no section {number}")


def patched(src, edits):
    b = bytearray(src)
    for at, value in edits:
        b[at] = value
    return bytes(b)


def write(name, *messages):
    (out / name).write_bytes(b"".join(messages))


# CMC's 300 hPa wind speed twice: a u/v "pair" at 45 degrees to the grid.
cmc = (g1 / "cmc_wind_300_2010052400_p012.grib").read_bytes()
write("cmc_pair.grib", cmc, cmc)

# The committed GFS message as NCEP local parameters 0-2-250 and 0-2-251,
# which no table names (eccodes reads both as `unknown`): a pair whose names
# say nothing (#1142).
gfs = (g2 / "gfs_c255_latlon.grib2").read_bytes()
s4 = section(gfs, 4)
write("gfs_unnamed_pair.grib2",
      patched(gfs, [(s4 + 9, 2), (s4 + 10, 250)]),
      patched(gfs, [(s4 + 9, 2), (s4 + 10, 251)]))

# HRRR temperature stating grid-relative, earth-relative, grid-relative
# components (template 3.30 flags, bit 5), and against a lat/lon grid.
hrrr = (g2 / "hrrr_complex_spd_lambert.grib2").read_bytes()
flags = section(hrrr, 3) + 14 + 32
grid_rel = patched(hrrr, [(flags, hrrr[flags] | 0x08)])
earth_rel = patched(hrrr, [(flags, hrrr[flags] & 0xF7)])
write("hrrr_frames.grib2", grid_rel, earth_rel, grid_rel)
write("two_grids.grib2", grid_rel, (g2 / "regular_latlon_surface.grib2").read_bytes())

# A T383 spectral field twice, to Compare.
t383 = (g2 / "spectral_simple_t383.grib2").read_bytes()
write("spectral_pair.grib2", t383, t383)

# ECMWF 2 m temperature with the PDS rewritten: as committed; table 140
# id 229 (swh); DWD's own table 129; Meteo-France with ECMWF as sub-center;
# level type 250; level type 255.
t2m = (g1 / "j_consecutive_latlon.grib1").read_bytes()
p = 8
write(
    "grib1_names.grib1",
    t2m,
    patched(t2m, [(p + 3, 140), (p + 8, 229)]),
    patched(t2m, [(p + 4, 78), (p + 3, 129)]),
    patched(t2m, [(p + 4, 85), (p + 25, 98), (p + 3, 140), (p + 8, 229)]),
    patched(t2m, [(p + 9, 250)]),
    patched(t2m, [(p + 9, 255)]),
)

# The same ECMWF 2 m temperature with a P1 or P2 its time range does not
# read (#1674): DWD's code 13 at P1 6h, at 0, and as precipitation (table 2
# parameter 61) at P1 0, P2 1h; ECMWF code 1 at P1 6h, and at P1 6h, P2 18h;
# ECMWF 123 at P1 6h and 0 (P2 12h); ECMWF code 0 at P1 6h, P2 18h. PDS
# octet 9 is the parameter, 18 the unit, 19 and 20 P1 and P2, 21 the time
# range, 22 and 23 N, the number of products a statistic combines. Off
# ECMWF, its local PDS extension (octets 29-52) is dropped and both lengths
# fixed, and the parameter is table 2 code 11 (temperature) unless given.
# The first message is byte for byte the 15th message (the `# 14` row) of
# 0.6.1's grib1_time_ranges.grib1.
def stamp(centre, p1, p2, code, param=11, unit=1, n=0):
    b = bytearray(t2m)
    b[p + 4] = centre
    b[p + 17], b[p + 18], b[p + 19], b[p + 20] = unit, p1, p2, code
    b[p + 21:p + 23] = n.to_bytes(2, "big")
    if centre != 98:
        b[p + 3], b[p + 8] = 2, param
        b[p + 2] = 28
        del b[p + 28:p + 52]
        b[4:7] = len(b).to_bytes(3, "big")
    return bytes(b)


write("grib1_unused_p1_p2.grib1",
      stamp(78, 6, 0, 13), stamp(78, 0, 0, 13), stamp(78, 0, 1, 13, param=61),
      stamp(98, 6, 0, 1), stamp(98, 6, 18, 1),
      stamp(98, 6, 12, 123), stamp(98, 0, 12, 123), stamp(98, 6, 18, 0))

# A statistic over one product (#1623): code 113 with N 1 at P1 6h and at 0,
# ECMWF 123 with N 1 at P1 6h, code 113 with N 2 and with N 65535, JMA's 132
# with N 1 at 0. Then a time unit Table 4 reserves (#1632): code 4 from P1 0 to
# P2 6 and a forecast at P1 6, both in unit 20.
write("grib1_one_product.grib1",
      stamp(98, 6, 0, 113, n=1), stamp(98, 0, 0, 113, n=1), stamp(98, 6, 0, 123, n=1),
      stamp(98, 6, 0, 113, n=2), stamp(98, 6, 0, 113, n=65535),
      stamp(34, 0, 0, 132, n=1),
      stamp(98, 0, 6, 4, unit=20), stamp(98, 6, 0, 0, unit=20))

# A monthly mean of analyses over 720 hours, every 6 hours (a Code Table 4.11
# type 1 time range, in GRIB2 section 4 from octet 47: length unit at +2,
# length at +3, increment unit at +7, increment at +8), with the length and
# the increment rewritten (#1627): every 24 hours over 720; over 24 every 6;
# over 24 every 24; over 24 every 48; over 24 every 1 day (unit 2). Last, every
# 6 over 720 hours with the increment in time unit 99, which Code Table 4.4
# reserves (#1632).
monthly = Path("crates/fieldglass/tests/fixtures/monthly_mean_of_analyses.grib2").read_bytes()


def runs(length, increment, increment_unit=1):
    b = bytearray(monthly)
    at = section(monthly, 4) + 46
    b[at + 3:at + 7] = length.to_bytes(4, "big")
    b[at + 7] = increment_unit
    b[at + 8:at + 12] = increment.to_bytes(4, "big")
    return bytes(b)


write("grib2_runs.grib2", monthly, runs(720, 24), runs(24, 6), runs(24, 24),
      runs(24, 48), runs(24, 1, increment_unit=2), runs(720, 6, increment_unit=99))
# The first two of those, and the second alone: one file the panel reads, and
# the file it becomes (#1636).
write("stats_two.grib2", monthly, runs(720, 24))
write("stats_one.grib2", runs(720, 24))

# GRIB2 temperature 6 hours before the reference time and 6 hours after (#1616):
# the forecast time is GRIB2 section 4 octets 19-22, sign and magnitude.
latlon = (g2 / "regular_latlon_surface.grib2").read_bytes()
s4 = section(latlon, 4)
write("grib2_leads.grib2", patched(latlon, [(s4 + 18, 0x80), (s4 + 21, 6)]),
      patched(latlon, [(s4 + 21, 6)]))

# Six different fields in one file, so stepping through them can show a
# message that comes back as another's (#1803).
write("six_fields.grib2", *[(g2 / name).read_bytes() for name in (
    "gfs_c255_latlon.grib2", "regular_latlon_surface.grib2", "complex_regular_latlon.grib2",
    "jpeg2000_regular_latlon.grib2", "spectral_simple_t63.grib2", "reduced_gaussian_pressure_level.grib2")])

# A time axis that steps by half a second (#1481).
import netCDF4
import numpy as np
with netCDF4.Dataset(out / "halves.nc", "w", format="NETCDF4_CLASSIC") as d:
    for name, size in (("time", 4), ("lat", 3), ("lon", 3)):
        d.createDimension(name, size)
    for name, units, values in (("time", "seconds since 1970-01-01", [0, 0.5, 1, 1.5]),
                                ("lat", "degrees_north", [-10, 0, 10]),
                                ("lon", "degrees_east", [0, 10, 20])):
        d.createVariable(name, "f8", (name,)).units = units
        d[name][:] = values
    d.createVariable("v", "f4", ("time", "lat", "lon"))[:] = np.arange(36).reshape(4, 3, 3)

# GRIB2 temperature with discipline 191, then 99.
write("grib2_codes.grib2", patched(latlon, [(6, 191)]), patched(latlon, [(6, 99)]))

# Ensemble members 7 and 8 (template 4.1 octet 36), in both orders and doubled.
member = (g2 / "ensemble_member_4_1.grib2").read_bytes()
m8 = patched(member, [(section(member, 4) + 35, 8)])
write("members_7_8.grib2", member, m8)
write("members_8_7.grib2", m8, member)
write("members_8_8.grib2", m8, m8)

# A color table in a model Fieldglass refuses.
(out / "hsv.cpt").write_text("# COLOR_MODEL = HSV\n0 0-1-1 1 360-1-1\n")
EOF

# Every file above decodes and renders headlessly, and lands in the right place.
# The preflight skips Zarr stores; the sweep covers them.
node tools/preflight_samples.js samples/0.6.0
( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py samples/0.6.0 )
```

`.h5` files do not open in Fieldglass by double-click: right-click the file,
*Open With…*, *Fieldglass Viewer*.

### Files by section

Each file is opened once; this is where its checks are.

| File | Sections |
|---|---|
| `S: samples/gfs.grib2` | 1, 11, 12 |
| `S: samples/mirs_swath.nc` | 1 |
| `S: samples/oisst.nc` | 1 |
| `S: samples/szip2_h5repack.h5`, `szip2_le_data.h5` | 1 |
| `S: samples/rap.grib2`, `eccc.grib2`, `rrfs.grib2` | 13 |
| `S: samples/reduced_gg_n32_smooth.grib1`, `octahedral_gaussian_o32.grib2`, `reduced_ll_wave.grib1`, `octahedral_o1280.grib1` | 5 |
| `B: samples/0.6.0/gfs_wind.grib2` | 1, 6, 12 |
| `B: samples/0.6.0/hrrr_wind.grib2` | 6 |
| `B: samples/0.6.0/ncep_air_daily_2024.nc` | 1, 8, 12 |
| `B: samples/0.6.0/ncep_air_levels_ltm.nc` | 11 |
| `B: samples/0.6.0/air_v3.zarr`, `air_v2.zarr` | 1 |
| `B: samples/0.6.0/two_grids.grib2` | 1, 6 |
| `B: samples/0.6.0/spectral_pair.grib2` | 1 |
| `B: samples/0.6.0/cmc_pair.grib` | 1, 6 |
| `B: samples/0.6.0/grib1_names.grib1` | 1 |
| `B: samples/0.6.0/grib1_unused_p1_p2.grib1` | 2 |
| `B: samples/0.6.0/six_fields.grib2` | 12 |
| `B: samples/0.6.0/grib1_one_product.grib1` | 4 |
| `B: samples/0.6.0/grib2_runs.grib2`, `grib2_leads.grib2`, `stats_two.grib2`, `stats_one.grib2` | 4 |
| `B: samples/0.6.0/halves.nc` | 8 |
| `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2`, `bifourier_ellipse_ieee32.grib2` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/reduced_latlon_10deg.grib2` | 5 |
| `F: crates/fieldglass-grib2/tests/fixtures/jpeg2000_regular_latlon.grib2`, `jpeg2000_local_40000.grib2` | 13 |
| `F: crates/fieldglass-grib2/tests/fixtures/transverse_mercator_ukv.grib2` | 15 |
| `F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1` | 11 |
| `F: crates/fieldglass/tests/fixtures/uv_mismatch.grib2`, `uv_members.grib2`, `uv_derived.grib2` | 6 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc` | 1 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf_numeric_calendar.nc` | 3 |
| `F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc` | 7 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf_values_no_degrees.nc` | 9 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_huge_attributes.nc`, `netcdf4_damaged_text.nc`, `ersst_v5_187001_cdf1.nc` | 11 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_bzip2_chunks.nc` | 16 |
| `F: crates/fieldglass-zarr/tests/fixtures/windows/window_s2n_v3` | 9 |
| `F: crates/fieldglass-zarr/tests/fixtures/fixedscaleoffset`, `wrapped_shards` | 10 |
| `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` | 1 |
| `F: extension/src/test/fixtures/hdf5_scale_offset_listing.h5` | 16 |
| The release dry run's `@fieldglass/wasm` tarball and a `.vsix` | 14 |

---

## 1. Regression floor

Carried forward each cycle. This is the minimum for a release whose diff is
small, and it is *not* a substitute for the per-feature sections below it.

- [ ] `S: samples/gfs.grib2` — renders, reprojects, coastlines align.
- [ ] **Controls a grid can't use are hidden, and the panel says why**
      (#1036). On `gfs.grib2` with Projection on Source projection (next
      step), Resampling and the projection settings (Bounds, Center lon and
      lat, Hemisphere, Central meridian) are hidden: each appears only with a
      projection it acts on. Every other control shows, and there is no "Not
      available for this grid" line under the caption. On
      `S: samples/mirs_swath.nc`, Zonal average and Resampling are hidden;
      the line under the caption reads `Not available for this grid:
      Resampling, Zonal average`, starts closed, and opens (click, or Tab to
      it and press Enter) to `Resampling: this grid places each cell by
      its own coordinates, so the map always shows the nearest cell.` and
      `Zonal average: this grid's rows aren't circles of latitude.` Nothing
      is grayed out.
- [ ] **Resampling shows only when the projection warps** (#1091). On
      `gfs.grib2` with Projection on Source projection there is no
      Resampling picker. Choose Equirectangular: it appears. Set it to
      Bilinear, switch to Source projection and back: it is still Bilinear and
      the map is the same as before. Do the same on a NetCDF slice
      (`B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`).
      Nothing else in the toolbar leaves a blank gap or stays visible when it
      should be gone. The one exception is the empty row under Bounds while
      it is on Auto: that space is reserved on purpose, so the map does not
      move under the pointer when a wheel or drag switches Bounds to Manual
      (#1228). Field B's place in the Compare row while Compare is off is
      kept empty too, one control row at most, as the Bounds row's is
      (#1429, #1715): a slice's Field B steppers that take several lines
      when shown keep one line while hidden.
- [ ] **No Resampling on a grid of cell centers** (#1225). On
      `S: samples/mirs_swath.nc` (`TPW`), choose Equirectangular, then
      Mollweide: there is still no Resampling picker, the line still lists
      Resampling, and the caption ends `(nearest)`. Fail: a Resampling picker
      on either map. The warp draws such a grid nearest whatever is asked, so
      Bilinear would change nothing.
- [ ] **Contours on `gfs.grib2` in Mollweide.** No isoline streaks straight
      across the map. Use Mollweide rather than Equirectangular: an
      equirectangular raster is as wide as the grid, which lands the last pixel
      on the seam and hides the gap the bug lives in (#332).
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2` — a
      spectral field still synthesizes and renders.
- [ ] **A field with no rows and columns states its own size.** The spectral
      file above reads `T63`, `extension/src/test/fixtures/healpix_n4_ring.grib2`
      reads `Nside 4`, and
      `F: crates/fieldglass-grib2/tests/fixtures/bifourier_ellipse_ieee32.grib2`
      reads `N4 M4`. Bi-Fourier decodes only to coefficients and does not
      render, so check its Size column and the note in its row, which reads
      `Render not available: Bi-Fourier synthesis isn't supported, so these
      coefficients can't be drawn.` (#1465), not "grid dimensions unknown".
- [ ] **A coarse grid reprojects at display scale** (#514). The HEALPix file
      above in **Orthographic**: the globe's edge is a smooth circle, not a
      staircase, and the coastlines follow it rather than floating over blank
      pixels. The data stays blocky — 192 pixels is all there is — and the
      source view shows that honestly.
- [ ] `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` — classic NetCDF
      metadata view renders.
- [ ] **A variable with no cells says why** (#1425). In the same file `time`
      is unlimited with no records. Click Render on `AGC_1202`: the status
      reads `nothing to draw: "time" (dimension 0 of "AGC_1202") has
      length 0`, with no `Error:` in front, no picture and no color bar.
      Export PNG says `Nothing to export: …`, not that the last render
      failed. The axis pickers and Compare stay; the empty frame, the Line
      plot box, Flip Y axis and the Colors, Color Range and Overlay rows are
      hidden (#1426). Fail: the panel stays on `Rendering…`, or calls it an
      error.
      Set Y axis to `z` (time across), then turn Compare on, then pick
      `CD_310`: each says the same of its own variable, and the panel keeps
      answering.
- [ ] Make an empty `.nc` file anywhere (`: > empty.nc`) and open it: the page says
      `Nothing to list in this file.`, not `No messages found.`, which is
      kept for GRIB (#1053).
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` —
      the canonical GRIB2 path renders.
- [ ] **Difference map, on NetCDF**: `S: samples/mirs_swath.nc` → `BT` at two
      different `Channel` indices, **Compare**. The difference renders and the
      probe reads the *combined* value, not field A (#329).
- [ ] **Difference map, on GRIB**: `B: samples/0.6.0/gfs_wind.grib2`, message 0
      (UGRD) **Compare** A − B against message 1 (VGRD). The difference renders,
      and the probe at one point reads u minus v there, not u.
- [ ] **A Compare click on a spectral field reads both values** (#815
      regression). The combined readout now comes from the library's own
      rule. `B: samples/0.6.0/spectral_pair.grib2` holds one T383 field
      twice, which the map draws at T359. Render message 0 and click a point:
      the readout reads `… shown · … at full detail (T383)`. Set Compare to
      `A + B` with Field B on message 1 and click the same point: both numbers
      are twice what they were, and the `(T383)` stays. Then Export CSV… with
      Compare off: the file's first line is `# spectral field shown at T359
      of T383: wavenumbers above T359 are not included`. Failure looks like
      a single value on the Compare map, or a CSV with no `#` line.
      `crates/fieldglass/conformance/probe_combined.json` holds the values.
- [ ] **Field B shows only while Compare is on** (#1715). On the GRIB
      difference map above, set Operation to `(off)`: the Field B picker
      is hidden, leaving its room empty, and nothing else in the toolbar
      or the map below it moves. Set A − B again: Field B is back, still on
      message 1, and the map is the difference again. Do the same on
      `B: samples/0.6.0/ncep_air_daily_2024.nc` (`air`) with Field B's
      `time` stepper moved to step 5: with Compare off its steppers are
      hidden, leaving their room empty, and with it on they read step 5
      again. Narrow the editor until Field B's steppers take two lines or
      more with Compare on, then set Operation to `(off)`: the room left
      is one line, not several (#1429); turning Compare on again grows it
      back to every stepper. On the GRIB map, untick Arrows: the V
      component and `every … cells` are hidden, leaving their room empty,
      while the arrows' color and weight stay; tick it and they are back
      with the V component chosen before. Fail: a Field B picker or
      stepper with Compare off, a V component with Arrows off, several
      blank lines where Field B was, the map or the toolbar's rows
      jumping as Compare is toggled in a wide panel or Arrows in any, or
      Field B back on another message or step.
      `compare-field-b.test.ts` drives all of this through the panel.
- [ ] **Export PNG** from one render; the file opens and matches what was on
      screen.
- [ ] Tick **Contours** and **only**, then **Export PNG**: the file shows the
      isolines without the color field, and no color bar. In the bar's place,
      a key reads `Contours every 10 K`
      above `221.5 … 309.7 K`. In the PNG it is two lines; on screen it wraps
      only between words, so it can take three or four lines. It carries your field's interval, range and units. Type an
      interval and the key follows; blank is the automatic spacing (#1463).
      The key's range is the color range the lines are spaced over, not
      always the data's. Set Color Range to Manual with min and max inside
      your field's range (say 250 and 300 on gfs's 221 to 310 K field): the key
      reads `Contours every 5 K` above `250.0 … 300.0 K`: the lines respace over
      that range, every 5 K where they were every 10. Back to
      Auto, it reads the data's range again. Failure looks like a key that
      keeps the data's range under a manual one.
      `a_manual_range_spaces_the_automatic_contours` and
      `one_manual_end_takes_the_other_from_the_data` (both in
      `crates/fieldglass/tests/color_range_rules.rs`; the second also in
      `crates/fieldglass/src/color_range.rs`) pin the one range the
      lines and the colors share, and `composeContourKey` in
      `extension/src/test/suite/contour-key.test.ts` pins the key's text. No
      test drives the panel's respacing under a manual range, so that part
      is by eye.
      An interval too small for the range is refused, not drawn in part
      (#1484). Back on Auto, type `0.01` as the interval: no isolines are
      drawn, the key reads `No contours drawn`, on screen and in the PNG,
      and the status line reads `Contours: contour interval 0.01 would draw
      … levels, more than the 2000 allowed; choose a wider interval`, with
      about the range divided by 0.01 in place of the `…`. Type `5`: the
      lines, the key and the status line are as before. Then set Color Range
      to Manual, min `300`, max `290`: the key reads `No contours drawn`
      again. Set min back below max, back to Auto, and clear the interval.
      Failure looks like lines over the bottom of the range only, or a key
      describing lines that are not on the map. `contour-key.test.ts` (`The
      contour key claims no lines that are not drawn (#1484)`) drives both
      cases through the panel, and
      `crates/fieldglass-napi/src/range_rule_tests.rs`
      (`contours_refuse_an_interval_past_the_level_cap`) holds the refusal
      for every contour call.
      The map area is the editor's background, as on screen: dark in Dark
      Modern, white in Light Modern, with the isolines and coastlines clearly
      readable in both (the title and key stay on white). Untick **only**: the
      color bar is back and the key is gone, on screen and in the next PNG.
      Untick Contours with **only** still ticked: the field is back
      on screen and in the next PNG (#1270, #1265). The **only** box and the
      contour interval field go while Contours is off, and leave their room
      empty, so the toolbar does not move when Contours is ticked again
      (#1260). Failure looks like a blank map with Contours off, or an
      interval field still showing.
- [ ] On a file with two grids (`B: samples/0.6.0/two_grids.grib2`: a
      Lambert message, then a lat/lon one), render message 0 and set Compare
      to A − B with Field B on message 1. The status line says "Error: …", and **Export PNG** says "Export
      failed: the last render failed; see the status line." and saves nothing.
      Set Field B to message 0, the same grid: the export works again (#1187).
- [ ] On the `gfs.grib2` render panel, Export CSV… sits beside Export PNG…
      (#1039). In 0.5.0 a GRIB field's CSV could be exported only from the
      metadata table. The save dialog opens in the file's folder on the PNG's
      name with `.csv`, `gfs_<parameter>_message-<n>.csv`, and the file holds
      the field.
- [ ] On the GRIB difference map above, hover Export CSV…: the tooltip says
      that with Compare on it exports Field A. Click it: the layout prompt
      starts "Compare is on: the CSV holds Field A, not the difference.", and
      the CSV holds message 0's values (u), not u minus v. A CSV of the
      difference is #1111.
- [ ] **Standard-calendar times are still dates** (#1548 regression). On
      `B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`, the `time` stepper
      reads `2024-01-01` at step 0 and `2024-12-31` at the last step, and the
      line under the heading carries the same date. Failure looks like
      `1963536 hours since 1800-01-01 …`: this file states no `calendar`,
      which must still count as standard.
- [ ] **GRIB1 forecasts with no time range are unchanged** (#876
      regression). `B: samples/0.6.0/cmc_pair.grib` reads `+12h` in the Fcst
      column for both messages, and `B: samples/0.6.0/grib1_names.grib1`
      reads `+0h` for all six. Failure looks like `time range 0, …` or an
      empty Fcst cell.
- [ ] **The "Variables not decoded" table still lists a file's unreadable
      variables** (#1556 regression). On
      `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc`
      the table lists `station_info` with `unsupported section: HDF5 datatype
      class 6 (compound)` and `visits` with a reason ending
      `(variable-length)`. Under it the legend reads `Fieldglass could not
      read these variables, for the reason given beside each.` and nothing
      more (0.6.0 said these variables "use an HDF5 datatype Fieldglass does
      not decode"; the new legend no longer goes on to say the rest of the
      metadata is complete, since a listed variable can lack an attribute).
      Failure looks like
      the table missing, a variable in both tables, or a legend that claims
      `The rest of this file's metadata is complete` (#1571).
- [ ] **A real NetCDF-4 file reads as it did** (#1556, #1557, #1562, #1805,
      #934 regression). The heap and budget changes, and two refactors of how an
      HDF5 address and a heap ID are read, sit on the path every NetCDF-4
      attribute is read through. Open `S: samples/oisst.nc` in 0.6.1
      and in this build. Both list the same 37 global attributes and the same
      attributes on `time`, `zlev`, `lat`, `lon`, `sst`, `anom`, `err` and
      `ice`, with the same values. There is no "Variables not decoded"
      table in either. The metadata page appears in about the same time in
      both, no more than a second apart. Failure looks like an attribute
      missing or changed in this build, a new "Variables not decoded" table, or a
      page that is clearly slower to open.
      A variable's axes come from the same heap, through its
      `DIMENSION_LIST` (#1695, #1698). Render `sst` in both: the axis
      pickers name `time`, `zlev`, `lat` and `lon`, the field is the same
      ocean picture in the same place, and the readout at one point gives
      the same value. Failure looks like a `phony_dim_` axis name, a note
      that a `DIMENSION_LIST` is damaged, or a field 0.6.1 placed and this
      build does not.
- [ ] **A Zarr v3 store renders and its times are dates** (#1630, #1648
      regression). Open `B: samples/0.6.0/air_v3.zarr` with *Fieldglass:
      Open Zarr Store…* in 0.6.1 and in this build. `air` draws the same global
      field in both, and the `time` stepper reads `2024-01-01` at step 0 and
      `2024-12-31` at the last step, with no note beside it. Failure looks
      like a number with units, a `(not shown as a date: …)` note, or a
      store that does not open. Do the same with
      `B: samples/0.6.0/air_v2.zarr`, the same data as a consolidated v2
      store: the Zarr codec and shard readers changed this cycle (#1823,
      #1303), and a v2 store reads its codecs through the same code.
- [ ] **An HDF5 file still opens and reads** (#934, #1805 regression). The
      two HDF5 refactors above also sit under every plain `.h5` file.
      Right-click `S: samples/szip2_h5repack.h5`, *Open With…*, *Fieldglass
      Viewer*, and render `dset_szip` (40 by 20, szip). Click the corners:
      they read 0 and 799, and the field rises steadily between them, in
      0.6.1 and in this build. Then open `S: samples/szip2_le_data.h5`. The
      Variables table lists the 10 datasets the reader decodes (`Array_*`,
      `Deflate_*`, `Fletcher_*`, `Shuffle_*` and `Szip_*`, each `_be` and
      `_le`), with a Render button each. Variables not decoded lists the
      other 14 with a reason. The two `Nbit_float_data_*` read `unsupported
      section: HDF5 floating-point datatype with a non-IEEE layout: a float
      of 20 bits in a 32-bit element, with a…`, and a click (the cell then
      offers "Show less") shows the full reason, ending `with an exponent of
      6 bits, a mantissa of 13 bits and exponent bias 31`. The 12 `Scale_offset_*` read, whole, `unsupported section: HDF5
      filter id 6 is not supported (only deflate, shuffle, fletcher32, zstd,
      and szip are decoded)` (#1855; 0.6.1 listed those 12 as variables, and
      their Render failed with that error). `Szip_float_data_le` and
      `Szip_float_data_be` both render, starting 0.333, 0.667, 1.0. Failure
      looks like a dataset that no longer renders, a `Scale_offset_*` with
      a Render button, or a reason in the Rust form `FloatLayout { … }`.

---

## 2. A GRIB1 time range names a P1 or P2 it does not read (#1674)

DWD's local time-range code 13 is "Fields from analyses valid at reference
time for P1 = 0", and WMO's code 1 an analysis for P1 = 0. Neither reads P1
or P2. Codes 118, 123 and 124 are statistics over analyses that start at the
reference time, and read P2 as their step but not P1. A forecast (code 0,
and DWD's 11 and 14) reads P1 but not P2. 0.6.1 showed `analysis` for a
code-13 message with P1 = 6h, and `+6h` for a forecast with P2 = 18h, so the
field was lost without a word.

**`B: samples/0.6.0/grib1_unused_p1_p2.grib1`**

- [ ] The Fcst column reads, message by message:

      | # | Center | Code, P1, P2 | Fcst |
      |---|---|---|---|
      | 0 | DWD | 13, 6h, 0 | `analysis (P1 6h not used)` |
      | 1 | DWD | 13, 0, 0 | `analysis` |
      | 2 | DWD | 13, 0, 1h (precipitation) | `analysis (P2 1h not used)` |
      | 3 | ECMWF | 1, 6h, 0 | `analysis (P1 6h not used)` |
      | 4 | ECMWF | 1, 6h, 18h | `analysis (P1 6h, P2 18h not used)` |
      | 5 | ECMWF | 123, 6h, 12h | `average of analyses every 12h (P1 6h not used)` |
      | 6 | ECMWF | 123, 0, 12h | `average of analyses every 12h` |
      | 7 | ECMWF | 0, 6h, 18h | `+6h (P2 18h not used)` |

      0.6.1 reads `analysis` on 0 to 4, `average of analyses every 12h` on
      5 and 6, and `+6h` on 7. eccodes 2.34.1 reads 0, 3, 4 and 5 with step
      6 (valid at 18:00), so it takes the P1 as a step, and reads 2 as
      `accum` over `0-1`, from its DWD step-type list for precipitation.
      Fieldglass does not read either field at these codes, and says so.
      Failure looks like a row with a nonzero P1 or P2 the code does not
      read that does not name it, or a `(… not used)` on 1 or 6.

Neither `forecastHours` nor `offsetHours` is shown in the panel. They do not
change: `forecastHours` is P1 in hours on every row, and `offsetHours` is 0
on rows 0 to 6 and 6 on row 7.
`crates/fieldglass/tests/time_ranges_name_an_unused_p1_or_p2.rs` holds
both, with codes 118 and 124 and DWD's 11 and 14 too.

---

## 3. A numeric calendar is not dated (#1645)

`F: crates/fieldglass-netcdf/tests/fixtures/netcdf_numeric_calendar.nc`. Its
`t` is over `time`, whose `calendar` is the number `360`, and `u` over
`time2`, whose `calendar` is the list `1, 2`. Both are `days since
2000-01-01` at 0 and 360.

- [ ] Render `t`, then `u`. The time stepper reads `0 days since 2000-01-01`
      with `(not shown as a date: its calendar is not text)` beside it, and
      at the last step `360 days since 2000-01-01`. Click a point: the line
      plot's time axis is numbered 0 to 360, not dated. Failure looks like
      `2000-01-01` and `2000-12-26`, which is 360 days on the standard
      calendar and what 0.6.1 shows, or a number with no note.

---

## 4. More forecast-column cases (#1623, #1632, #1627, #1616, #1636)

Four more ways the Fcst column said less than the file did, or more, and one
way a panel drew the wrong statistic.

**`B: samples/0.6.0/grib1_one_product.grib1`**

A GRIB1 statistic combines N products whose reference times or leads are P2
apart. With P2 = 0, 0.6.1 showed the raw form because it never read N. N is
now read from octets 22 and 23, and when it is 1 the message is that one
product. Time units 20 and 99 are in no table, so a period or a step stated in
one has no known length. 0.6.1 read one as if it had.

- [ ] **priority** The Fcst column reads, message by message:

      | # | Center | Code, P1, P2, N | Fcst |
      |---|---|---|---|
      | 0 | ECMWF | 113, 6h, 0, 1 | `average of one +6h forecast` |
      | 1 | ECMWF | 113, 0, 0, 1 | `average of one initialized analysis` |
      | 2 | ECMWF | 123, 6h, 0, 1 | `average of one analysis (P1 6h not used)` |
      | 3 | ECMWF | 113, 6h, 0, 2 | `time range 113, P1 6h, P2 0h` |
      | 4 | ECMWF | 113, 6h, 0, 65535 | `time range 113, P1 6h, P2 0h` |
      | 5 | JMA | 132, 0, 0, 1 | `variance of one analysis` |
      | 6 | ECMWF | 4, 0 to 6, unit 20 | `time range 4, P1 0 time unit 20, P2 6 time unit 20` |
      | 7 | ECMWF | 0, 6, unit 20 | `+6 time unit 20` |

      0.6.1 reads 0 to 5 as `time range 113, P1 …, P2 0h` (or the 123 and 132
      equivalents) and 6 as `0–6 time unit 20 accum`. Failure looks like
      words on 3 or 4 (N is not 1, so there is no one product), or an
      `accum` on 6.

`forecastHours` and `offsetHours` do not change, so `offsetHours` stays empty
on 0 to 5. `crates/fieldglass-grib1/tests/number_included.rs` and
`crates/fieldglass-grib1/tests/code_tables.rs` hold both.

**`B: samples/0.6.0/grib2_runs.grib2`**

A GRIB2 statistic over successive runs (Code Table 4.11 type 1) is worded as a
series, `average of analyses every 6h over 720h`. If its increment is at
least its length it can hold one run at most, and 0.6.1 worded it as a series
all the same.

- [ ] **priority** The Fcst column reads:

      | # | Length, increment | Fcst |
      |---|---|---|
      | 0 | 720h, 6h | `average of analyses every 6h over 720h` |
      | 1 | 720h, 24h | `average of analyses every 24h over 720h` |
      | 2 | 24h, 6h | `average of analyses every 6h over 24h` |
      | 3 | 24h, 24h | `+0h, statistics: average over 24h, increment 24h of type 1 (successive runs)` |
      | 4 | 24h, 48h | `+0h, statistics: average over 24h, increment 48h of type 1 (successive runs)` |
      | 5 | 24h, 1 day | `+0h, statistics: average over 24h, increment 24h of type 1 (successive runs)` |
      | 6 | 720h, 6 in unit 99 | `+0h, statistics: average over 720h, increment 6 time-range unit 99 of type 1 (successive runs)` |

      0.6.1 words 3 to 5 as `average of analyses every 24h over 24h`, `…
      every 48h over 24h` and `… every 24h over 24h`, and 6 as `average of
      analyses every 6 time-range unit 99 over 720h`, a series in a unit with
      no known length. Failure looks like a series on a row that cannot hold
      two runs or on 6, or the raw form on 2, which holds four. `statistical_windows.rs` (`a_series_that_cannot_hold_two_runs_is_shown_raw`)
      holds the rest, including two time ranges.

**`B: samples/0.6.0/grib2_leads.grib2`**

- [ ] **priority** Message 0 reads `−6h` and message 1 `+6h`. The minus is a true minus
      sign, the one a statistic's window uses, not a hyphen. By eye the minus
      is as wide as the plus; a hyphen is shorter. 0.6.1 read `+-6h` on
      message 0.

**`B: samples/0.6.0/stats_two.grib2`**

0.6.1 gave two statistics that differ only in how far apart their fields are
the same identity, so a panel could draw the wrong one after its file changed.

- [ ] Copy `stats_two.grib2` to `samples/0.6.0/reload.grib2`, open the copy
      and render message 0 (every 6h over 720h). Close the `reload.grib2`
      editor tab (keep the render panel open), overwrite the copy with
      `stats_one.grib2` (`cp samples/0.6.0/stats_one.grib2
      samples/0.6.0/reload.grib2`), and open `reload.grib2` again. The editor
      now lists one message. Bring the render panel's tab to the front (reopening the file puts the
      editor in front of it); it asks again. The panel heading reads `No message drawn` and the status
      `Error: This message is no longer in the file (or is ambiguous); pick
      one.`, and it draws nothing. Fieldglass does not watch the file, so a
      panel only learns of new bytes when the editor reopens. 0.6.1, on coming to the front, draws the
      every-24h field in place of the every-6h one it was asked for, and
      retitles the panel `average of analyses every 24h over 720h` without
      saying the message changed. The fixture is constant 0, so only the
      heading tells the two statistics apart. Delete `reload.grib2`.

---

## 5. Reduced grids (#1760, #1759, #1781, #811, #1312)

A reduced grid stores each row with its own number of points. Fieldglass
widens every row to the longest one before it draws, so a pixel can name a
column that is not the grid's nearest point. 0.6.2 reads a GRIB2 reduced
lat/lon grid, picks the nearest point by distance, and lists messages without
each row's point count.

**`F: crates/fieldglass-grib2/tests/fixtures/reduced_latlon_10deg.grib2`**

A hand-built global grid at 10 degrees: 19 rows from 90N to 90S with no points
on the two polar rows, and a bitmap that masks 38 points. The value at row
`r`, point `q` is `100*r + q`, so every point names its place.

- [ ] The message table lists it. The Grid column reads `reduced_latlon`, and
      the row has a Render button. 0.6.1 listed it as drawable and then
      refused it with `grid dimensions 4294967295×19`.
- [ ] Render it on Equirectangular, Nearest. The map is global, and the
      colors step by latitude: row 0 sits at 90N with no points, so the top
      band is empty and so is the bottom one, and the values grow to the
      south in bands. Scattered cells inside the bands are empty (the bitmap).
      Click a cell in the second band: it reads a value from 100 to 105. Fail:
      a refusal, bands that run across the wrong way, or points on the polar
      rows.

A reduced grid that covers only part of a circle, such as one from 0 to 100
degrees, is now shown as not placed, where 0.6.1 spread each row around the
whole globe. No committed file has one that decodes, so
`a_reduced_latlon_sub_area_is_declined_not_drawn_global`, in
`crates/fieldglass-grib2/src/geometry.rs` and `crates/fieldglass-grib1/src/geometry.rs`,
is the check, and `every_reduced_fixture_still_places` in each holds that no
global fixture is declined.

**`S: samples/reduced_gg_n32_smooth.grib1`**, then **`S: samples/octahedral_gaussian_o32.grib2`**

N32 and O32 are coarse: a pixel is near several points. The field in the first
is smooth, so a block in the wrong place shows as a step.

- [ ] **priority** Render each on Equirectangular, Nearest. Each point is a visible block,
      and the line between two blocks on a row falls halfway between their
      points, as does the line to the row above or below. Click either side of one
      of those lines: the value changes at the line, and the readout names the
      block under the pointer, never one a row away. 0.6.1 sometimes showed the
      point to one side of the nearest, such as 48°E where 54°E was nearer.
      Set Resampling to Bilinear: the blocks smooth out, and a click on a pixel
      still names the nearest point.
- [ ] Section 0's probe-parity sweep reported no FAILURES for these four
      files. It checks 200 points of each against
      eccodes' `codes_grib_find_nearest`, which is what judges the nearest
      point to the meter. By eye, this check can only find a block that is off
      by a row.

**`S: samples/reduced_ll_wave.grib1`** and **`S: samples/octahedral_o1280.grib1`**

- [ ] `reduced_ll_wave.grib1` (significant wave height): land is masked, and
      the rows over the poles hold no points, so they stay transparent.
      Click an ice cap: no value. This file is the same in 0.6.1.
- [ ] `octahedral_o1280.grib1` is ECMWF's operational grid, 2560 rows and 6.6
      million points, and its field is constant, so it renders flat. Open it
      in 0.6.1 and in this build. The message table reads the same in both:
      Grid reads `reduced_gaussian` and Size `O1280`, and the row has a Render
      button. The page opens at least as fast, and so does the render panel and
      a click's readout (0.6.2 no longer sends the 2,560 row counts each time a file or
      panel opens, #811, and looks at most 32 rows either side of a pixel,
      #1781). For the speed
      check, open each file twice and compare the second open. Failure looks
      like a blank Grid or Size cell, which means the listing lost something it
      needs.

---

## 6. The Vectors row says where u and v differ (#1140, #1794, #1309, #1843, #1850, #1844, #1856)

The V component picker takes any message in the file. 0.6.1 drew arrows for a
10 m u paired with an 850 hPa v, a +204h u with a +210h v, or member 1's u with
member 7's v, and said nothing. The arrows are still drawn. The row now says
where each component is. The note comes from the file's own level, time and
member.

**`F: crates/fieldglass/tests/fixtures/uv_mismatch.grib2`**

Message 0 is a u at 10 m, +204h. Messages 1 to 6 are v: 1 the same level and
time, 2 at 850 hPa, 3 at +210h, 4 at 850 hPa and +210h, 5 at 10 m stated with
another scale factor, 6 at 10 m valid at the same instant from a run 6 hours
later. 7 is a u and 8 a v at 0.3 m, stated as 3 at scale factor 1 and as 30
at scale factor 2. 9 is a v on the ground and 10 a v whose level is coded
missing.

- [ ] **priority** Open the file, click Render on message 0, tick Arrows and pick each V
      component in turn. The Vectors row reads:

      | V | Note |
      |---|---|
      | 1 | none |
      | 2 | `U is at 10 m above ground and V at 850 hPa.` |
      | 3 | `U is for +204h and V for +210h.` |
      | 4 | `U is at 10 m above ground and V at 850 hPa. U is for +204h and V for +210h.` |
      | 5 | none |
      | 6 | `U and V are from two runs: U is for 2012-03-02T06:00:00Z +204h and V for 2012-03-02T12:00:00Z +198h.` |
      | 9 | `U is at 10 m above ground and V at Ground or water surface.` |
      | 10 | none |

      Arrows are drawn for every row, with their key, and there is no
      "not a u/v pair" note on any of them. Untick Arrows: the note goes.
      0.6.1 shows none of these. Failure looks like a note on 1 or 5 (the
      same height is one level however its scale factor is written), none on
      2, or arrows that vanish when a note appears.
- [ ] **A scaled level reads in its own decimals** (#1844). The Level column
      reads `0.3` for messages 7 and 8, and the V component picker lists
      `#7 · UGRD · 0.3 · +204h` and `#8 · VGRD · 0.3 · +204h`. 0.6.1 reads
      `0.30000000000000004` for message 7 in both places; message 8 already
      reads `0.3`. Render message 7 and tick
      Arrows: V starts on 8, and there is no note, since the two spellings
      are one height. Message 5's level still reads `10`.

**`F: crates/fieldglass/tests/fixtures/uv_members.grib2`** and
**`F: crates/fieldglass/tests/fixtures/uv_derived.grib2`**

- [ ] `uv_members.grib2`: u is message 0 (member 1). V 1 (member 1) gets no
      note, and V 2 (member 7) reads `U is member 1 and V member 7.`
- [ ] `uv_derived.grib2`: u is message 0, the unweighted mean of all
      members. V 1 (the same mean) and V 4 (a derived forecast coded
      missing) get no note. V 2 (member 7) reads `U is the unweighted mean of
      all members and V member 7.` V 3 (the spread) reads `U is the unweighted
      mean of all members and V the spread of all members.` Failure looks
      like no note on 2 or 3, or a note on 1 or 4.
- [ ] `uv_derived.grib2`, the V component and Field B pickers (#1850): V 1
      reads `#1 · VGRD · 2 · +0h · Unweighted mean of all members`, V 3
      `… · Spread of all members` and V 4 `… · Derived forecast, kind
      missing`, and V 2 still reads `#2 · VGRD · 2 · +0h · member 7`. Field
      B's options read the same. The message list has a Derived column that
      reads `Unweighted mean of all members` on 0, 1 and 5, `Spread of all
      members` on 3, `Derived forecast, kind missing` on 4, and `—` on the
      members 2 and 6. The render panel's heading for message 0 ends `·
      Unweighted mean of all members`, and so does the subtitle of a PNG
      exported from it. 0.6.1 shows 1, 3 and 4 alike. Failure looks like two
      of them reading the same, or a member gaining a Derived part.
      `B: samples/0.6.0/gfs_wind.grib2` has no Derived column, and its
      pickers read as in 0.6.1.

**`B: samples/0.6.0/gfs_wind.grib2`**

- [ ] Message 0 (UGRD) and V 1 (VGRD) are one level and one time, so the
      Vectors row has no note beyond what 0.6.1 shows. A real pair must not
      be flagged.
- [ ] **Ticking Arrows starts on v and moves nothing** (#1843). Render
      message 0 in a wide panel and note where the Color Range row and the
      top of the map sit. Tick Arrows: the V component starts on `#1 · VGRD
      · …`, arrows draw with their key and no note, and once they have drawn,
      Color Range, Overlay and the map are where they were. Untick and tick
      again: nothing moves either time. Failure looks like a blank line under
      the Vectors row once the arrows draw, pushing the rows below it down.
      0.6.1 starts V on `#0`, the u itself, with a "not a u/v pair" note. Do
      the same on
      `B: samples/0.6.0/hrrr_wind.grib2` (Lambert): V starts on `#1`.
      On `B: samples/0.6.0/cmc_pair.grib`, two wind speeds that are not a
      pair, V still starts on `#0`, as in 0.6.1.
      `vector-arrows-toggle.test.ts` measures the rows.

**`B: samples/0.6.0/gfs_wind.grib2`**, Arrows on, in a narrow panel

- [ ] **The Vectors row does not move the map during a zoom** (#1309).
      Set Projection to Equirectangular first; Source projection does not zoom
      with the wheel. Narrow the editor until the Vectors row wraps onto two lines, then wheel
      the map in with the pointer held still, several ticks. The row's
      "Full-length arrow: …" text keeps its value while any arrow is in view
      and goes blank when none is, and the line keeps its height. The map must
      not shift under the pointer: every tick zooms on the same place,
      including the tick where the text goes blank. 0.6.1 dropped the line
      when it went blank, so the map jumped up one line under the pointer. The map does not move
      when you zoom back out either. `vector-row-zoom.test.ts` drives synthetic
      wheel ticks; a real mouse wheel is the part no test drives.

**`B: samples/0.6.0/two_grids.grib2`**

- [ ] **No Vectors row where no message could be V** (#1856). Render message
      0, then message 1. Neither shows a Vectors row, and the toolbar leaves
      no gap where it was. 0.6.1 showed the row on both messages, with its
      "rotated to true north" note on message 0 only. `cmc_pair.grib` still shows the row, as above.
      `vector-row-partner.test.ts` drives it.

---

## 7. A satellite view says where a cell is (#1676, #1797)

`F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc` is a 24 by 24
window of GOES-16 band 13, drawn in its own geostationary projection.

- [ ] Render `CMI` on Source projection and click near the middle until the
      readout ends `grid 12,12`. It reads `38.48°N, 74.99°W · 279.… K · grid
      12,12`.
      0.6.1 gave the value and the grid cell only. Switch to Equirectangular
      and click inside the same cell: the readout still ends `grid 12,12` and
      gives the clicked point, which lies within about 0.02° of 38.48°N,
      74.99°W (PROJ 9.4.0 places the cell center at 38.479028°N,
      74.988007°W).
- [ ] The readout has no `off the Earth` anywhere in this window. A pixel
      whose line of sight misses the Earth, such as the corner of a full-disc
      image, reads `off the Earth` in place of a position, and so does a Lambert
      azimuthal cell past the far side of the Earth. No committed file has
      either, so `space-view-probe.test.ts` ("a full-disc corner says it is
      off the Earth, and the middle has a position") and
      `crates/fieldglass/tests/space_view_probe.rs` cover it, and nothing by
      eye can.

---

## 8. A time under a second shows its fraction (#1481)

`B: samples/0.6.0/halves.nc`: `v` over `time` (0, 0.5, 1 and 1.5 `seconds
since 1970-01-01`), `lat` and `lon`.

- [ ] Render `v`. The time stepper reads `1970-01-01 00:00:00.0` at step 0,
      then `00:00:00.5`, `00:00:01.0` and `00:00:01.5`. 0.6.1 read
      `00:00:00`, `00:00:01`, `00:00:01`, `00:00:02`, so neighboring steps
      looked alike. Set Animate to `time` and step through it: the label beside
      the buttons and the line under the heading carry the same fractions, and
      no two steps read alike.
- [ ] Export PNG at step 1 and at step 2: the suggested names end in `-000000.5.png`
      and `-000001.0.png`, so two steps no longer share one. Cancel both dialogs.
- [ ] Open `B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`: the stepper still
      reads `2024-01-01` with no fraction. An axis a second or more apart
      reads as it did.

---

## 9. The note beside the variable picker says why a field is not placed (#1305, #1727)

A field whose coordinates no latitude or longitude can hold is drawn in its
own grid, with no map. 0.6.1 drew both fields below with no word. It now always
says why, and a latitude that states it is one and holds
a value past a pole is refused.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf_values_no_degrees.nc`**

- [ ] Render `bare`. The field is drawn in its own grid. The note beside the
      variable picker says `"y" holds latitudes from -1000 to 1000, past the
      poles` and `"x" holds longitudes from -2000 to 2000, more than twice
      around the globe`, and no coastline, border or graticule is offered over
      it.
- [ ] Render `attributed`. Its `lat` states `degrees_north` and holds 0 to 2000,
      so the panel is refused: the error and the note both say `"lat" holds
      latitudes from 0 to 2000, past the poles`. 0.6.1 drew the field with no
      word. Failure looks like a drawn field with no note, or `-0` in a number.

**`F: crates/fieldglass-zarr/tests/fixtures/windows/window_s2n_v3`**

- [ ] *Fieldglass: Open Zarr Store…* on that folder. The panel opens on `t2m`
      and the note beside the variable picker reads `The store holds no
      "latitude/c/2", so those values of "latitude" are its fill value, 0.`
      The store never wrote that chunk of `latitude`, so those rows sit at the
      equator, the fill value. 0.6.1 drew them there and said nothing. A
      NetCDF file has no such note.

A latitude stored a hair past a pole (90.000003) is read as the pole, a
coordinate at its own fill value is refused by name, and a 2-D latitude and
longitude cell outside the pair's `valid_range` has no position. None of these
has a committed file whose picture differs by eye. They are covered by
`crates/fieldglass-netcdf/tests/coordinate_missing_values.rs` and
`crates/fieldglass-netcdf/tests/coordinates_past_any_degree.rs`.

---

## 10. Zarr codecs (#1823, #1303)

**`F: crates/fieldglass-zarr/tests/fixtures/fixedscaleoffset`**

Ten arrays, each with numcodecs' `fixedscaleoffset` filter. 0.6.1 refused every
one with `unsupported section: Zarr numcodecs id "fixedscaleoffset" is not
decoded (blosc, zlib, gzip, zstd, lz4, shuffle, bytes, transpose, crc32c and
sharding_indexed are)`.

- [ ] *Fieldglass: Open Zarr Store…* on the folder. Pick each of the ten
      arrays in turn: each draws, with no `unsupported section` error.
- [ ] Render `uint8` (4 by 6). The values rise by 0.1 from 1000 at one corner
      to 1002.3 at the other, in order. The color bar spans 1000 to 1002, and a
      click on the far corner reads 1002.3. Render
      `same_type` (20 by 30): a noisy field between about 250 and 310, with
      one empty cell. Compare `int16` with it: the same picture, except that
      the cell empty in `same_type` reads 280 in `int16`, where the NaN was
      stored as code 0.
- [ ] Render `cf_int`: four cells are empty (they decode to its fill value,
      which CF masks), and the rest of the values are the filter's codes
      scaled and offset by CF, such as 350, 516.5 and 450 across the first
      row, not the filter's output itself (900, 1233, 1100). A render that shows
      the codes means the CF scale and offset were skipped.
      `crates/fieldglass-zarr/tests/fixedscaleoffset.rs` compares every value
      with zarr-python's, so this check is that each array draws.

**`F: crates/fieldglass-zarr/tests/fixtures/wrapped_shards`**

Six Zarr v3 arrays whose shards are compressed whole (`gzip`, or `crc32c` and
`zstd`, after `sharding_indexed`). 0.6.1 read them as unsharded and reported
every shard as a stream error, such as `parse error: gzip stream inflates
past the 128-byte ceiling` (or `zstd stream decompresses past …`).

- [ ] *Fieldglass: Open Zarr Store…* on the folder. Render `gzip` (8×6 in the panel: 6 rows of 8): the
      values rise from 1.0 by 0.25 along the rows to 12.75, except a 2 by 4
      block at the top of one side that reads -1, the fill value, because that
      inner chunk was never written. There is no `corrupt` or `inflates past`
      error. Render `crc32c_zstd`, `transposed`, `inner_gzip`, `index_start`
      and `plain_index`: each draws a field with no error.

---

## 11. The panel stays usable when narrow (#1729, #1529, #1677, #1841), and a short export (#1842)

Narrow the editor by dragging its edge, or by opening the panel beside another
editor.

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, `air` (time, level, lat, lon)

- [ ] **Slice steppers stay on the page** (#1729). With two held dimensions,
      turn Compare on and narrow the panel to about 300 px, then to 280 px.
      Field A's and Field B's steppers, with their slider, number box and
      coordinate, stay inside the toolbar, and the page does not scroll
      sideways. 0.6.1 ran Field B's stepper box past the toolbar
      only at the narrowest width (about 220 px), and the page scrolled
      sideways; there was no overflow at 300 or 280 px. `narrow-panel.test.ts` measures the steppers at 200, 280,
      300, 450 and 900 px; the look of the wrap is by eye. For a long
      coordinate, open
      `F: crates/fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc`,
      `sst`, with Compare on: its time reads `0 minutes since 1870-01-01 00:00`.
      Narrow the panel from 300 px down as far as VS Code allows (about 220 px):
      in Field A and in Field B the time breaks only at its spaces, never
      after the hyphen in `1870-01-` (#1841). Field B's box is narrower than
      Field A's, so it is the one to watch. 0.6.1 did not wrap the time at
      all and ran `0 minutes since 1870-01-…` off the panel.
- [ ] **A cross-section keeps its image** (#1529). Set Y axis to `level` and X
      axis to `lat`, so the slice is a section and not a map, and narrow the
      panel as far as VS Code allows (about 220 px). The image stays visible, the Y axis labels get
      narrow first, and the axis caption breaks between an axis and its
      units, never inside `(degrees_north)`. Then set Y axis to `time`: a date
      axis too wide for its labels gives its first and last date in the
      caption. Export PNG and save it: the line `Exported PNG to …` wraps and does not run past the panel's
      edge. 0.6.1 let the labels take the row until only a sliver of
      the color bar was left.
- [ ] **A short field's PNG keeps its color-bar labels apart** (#1842). Still
      on `air`, with Compare off, set Y axis to `time` and X axis to `lat`: a
      field 73 cells wide and 12 tall. Export PNG at any panel width and open
      the file. The color bar's three labels are each readable, one above the
      other, and the image is taller than the field so the bar and its labels
      fit (the bar hangs below the field). 0.6.1 exported it 12 px tall,
      with the labels printed over each other. Then set Y axis to `lat` and X axis to
      `lon` and export again: a full map exports as it did, the bar as tall
      as the field. `export-short-field.test.ts` checks the label boxes.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_huge_attributes.nc`**

- [ ] On its metadata page, a long global attribute is collapsed to a preview
      that stays inside its cell. The Global attributes table does not
      scroll sideways at any editor width down to about 320 px (#1677).

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_damaged_text.nc`**

- [ ] On its metadata page the `temperature` row names `Attribute "long_name"
      not read: the file's record of the text is damaged.` and Global
      attributes names the `title` the same way. Each ends in exactly one
      period (0.6.1 had none).

**`F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1`**, a constant field

- [ ] Render it and Export PNG (save it). The color bar's label shows the one
      value once, not three times over each other. Open the exported PNG to
      see it. `small-layout.test.ts` covers this.

**`S: samples/gfs.grib2`**

- [ ] Tick Contours, then its **only** box, with an interval of 5, and narrow
      the panel until the contour key beside the map wraps. It breaks between words, never inside `Contours`.
      `small-layout.test.ts` covers this too.
- [ ] **Nothing in the toolbar runs past its edge at the narrowest width**
      (#1841). Keep Contours **only** with interval 5, set Projection to
      Equirectangular, set Color Range to Manual (any min and max), and
      narrow the panel as far as VS Code allows (about 220 px). The contour
      interval box, the Manual min and max boxes, and the zoom buttons (`+`,
      `−`, Reset view) in the Bounds row all stay inside the toolbar, shrinking
      or wrapping onto another line, and the page does not scroll sideways.
      In 0.6.1 at 220 px the interval box ran about 32 px past the edge, and
      the range min and max and Reset view ran past too. Set Color Range
      back to Auto. `narrow-panel.test.ts` ("the contour interval box, and
      every other control, stays inside the toolbar at the narrowest width
      (#1841)") holds every control to the edge.

---

## 12. Stepping through GRIB messages, and the panel's requests (#1803, #1808)

0.6.1 kept every GRIB message's values it had drawn, probed or exported until
the file closed. It now keeps the last four, and fewer when they are large, so
stepping back to an old message decodes it again. That must change nothing the
eye sees. How much memory it holds cannot be judged by eye;
`crates/fieldglass-napi/src/warp_memo_tests.rs` pins the bound.

**`B: samples/0.6.0/six_fields.grib2`** (six different fields)

- [ ] Render message 0 on Equirectangular and note its clicked value at one
      point. Render 1, 2, 3, 4 and 5 in turn, and render 0 again. The picture
      and the value at that point are what they were. Failure looks like a
      message drawn as another's, or a value that changed. By then
      Fieldglass no longer holds message 0's values, so it decodes it again,
      which is the path to watch.

**`S: samples/gfs.grib2`**, **`B: samples/0.6.0/gfs_wind.grib2`** and **`B: samples/0.6.0/ncep_air_daily_2024.nc`**

- [ ] **Every request the panel sends is answered** (#1808). On `gfs.grib2` tick
      Coastlines, Contours and Graticule, click a point for its value, and
      tick Zonal average: each draws or reads out. On
      `B: samples/0.6.0/gfs_wind.grib2` tick Arrows: they draw, with their key.
      On `B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`, the time stepper
      reads a date, and with Line plot ticked a click on the map draws the
      plot. A request with no answer leaves the panel waiting, so a plot or
      a readout that never arrives is the failure. The change that makes an
      unanswerable request reply with no result has no step that reaches it,
      and `panel-replies.test.ts` drives it.

---

## 13. JPEG 2000 and the RRFS sample (#1801, #1800)

GRIB2 fields packed with JPEG 2000 decode with rust-j2k 0.4.0, which is about
3.6 to 4.3 times fewer instructions than 0.3.0. A field packed with the lossy
9/7 wavelet can move by one step at a few points. `decode_jpeg2000.rs` pins
the values against eccodes, so this section checks the pictures.

- [ ] `S: samples/rap.grib2` (Lambert), `S: samples/eccc.grib2` (rotated lat/lon),
      `F: crates/fieldglass-grib2/tests/fixtures/jpeg2000_regular_latlon.grib2` and
      `F: crates/fieldglass-grib2/tests/fixtures/jpeg2000_local_40000.grib2`
      render, with the same picture and value range as 0.6.1, and coastlines
      aligned. `rap.grib2` is the real JPEG 2000 on Lambert. If you time the
      open, 0.6.2 is quicker; no number is pinned.
- [ ] `S: samples/rrfs.grib2` (RRFS 3 km, 1799 by 1059, Lambert; replaced the
      NAM, which NOAA retired on 2026-11-03) renders a CONUS temperature field
      with coastlines aligned and not upside down or mirrored. NOMADS keeps a
      day or two: if the fetch printed a 404, re-run
      `DATE=$(date -u +%Y%m%d) tools/fetch_samples.sh rrfs`. A `samples/nam.grib2`
      from an earlier fetch is not replaced; delete it.
      This file is complex packing with spatial differencing, not JPEG 2000,
      so its check covers the RRFS grid and not rust-j2k.

---

## 14. The npm package and the `.vsix` carry their license notices (#1749, #1801)

From run 43445966403 (the #1749 dry run), or the release-SHA dry run once
RELEASING.md section 2 has made one, download the `@fieldglass/wasm` tarball
artifact: `gh run download <run-id> -n fieldglass-wasm-npm`.

- [ ] `tar -tzf fieldglass-wasm-*.tgz` lists `package/COPYRIGHT-library.html`
      beside `LICENSE-APACHE`, `LICENSE-MIT` and `NOTICE`.
      `tar -xzOf fieldglass-wasm-*.tgz package/package.json` has a `rustc`
      field naming release 1.99.1 and its commit. `package.mjs` checks both on
      every pull request, so this is the look at the real artifact.
- [ ] **The notices follow rust-j2k 0.4.0** (#1801). This needs the
      release-SHA dry run: run 43445966403 predates the bump. rust-j2k 0.4.0
      carries code ported from Little CMS 2.14 and Arm's optimized-routines,
      whose MIT notices must ship with it. `tar -xzOf fieldglass-wasm-*.tgz
      package/NOTICE` lists `rust-j2k` at `0.4.0` and has a `Code ported into
      rust-j2k` section with both notices under `Little CMS 2.14, ported in
      rust-j2k's src/jp2/lcms.rs (MIT)` and `Arm optimized-routines, ported
      in rust-j2k's src/jp2/math.rs (MIT)`. Download one `.vsix` from the
      same run (`gh run download <run-id> -n vsix-linux-x64`) and read its
      notice (`unzip -p fieldglass-*.vsix extension/NOTICE`): the same
      section, `rust-j2k` at `0.4.0` and `napi` at `3.16.0`.
      `third_party_notices.rs` checks the crate rows on every pull request,
      but reads the ported-code section as prose.

---

## 15. A GRIB2 value with a scale factor is the value the file states (#1849)

A GRIB2 Earth radius or axis, a level, and a size or wavelength limit are each
stated as an integer and a power of ten. They are now read in one rounding, not
two. The level text this changes is checked in section 6 (`uv_mismatch.grib2`,
#1844). The rest moves a number in its last bit only, which no picture shows,
so this section is a regression check on the grids that read their Earth's
shape this way.

**`F: crates/fieldglass-grib2/tests/fixtures/transverse_mercator_ukv.grib2`**

- [ ] Render it on Source projection and on Equirectangular, with Coastlines
      on. It is the Met Office UKV domain on the British National Grid, its
      Earth the Airy 1830 ellipsoid stated in kilometers at scale factor 3: a
      synthetic temperature ramp over the British Isles, with the coastlines on
      the land and the map from about 48°N to 61°N, as in 0.6.1. The PROJ
      string the npm package gives now ends `+b=6356257 +units=m +no_defs`,
      where 0.6.1 gave `+b=6356257.000000001`; the extension does not show
      it, and `crates/fieldglass/conformance/message_listing.json` pins it.
      Failure looks like a field off its coastlines, or flipped.

---

## 16. A variable Fieldglass cannot decode is not offered (#1855)

0.6.1 offered a variable stored with an HDF5 filter Fieldglass does not undo,
such as scale-offset (id 6), with a Render button, and every render of it
failed. A variable on an axis whose coordinate was stored that way failed too.
Now such a variable is listed under Variables not decoded and no picker offers
it, a variable on such an axis draws in its own grid with a note, and a
variable the filter never ran on renders as before. Section 1 checks the
listing of `szip2_le_data.h5`.

**`F: extension/src/test/fixtures/hdf5_scale_offset_listing.h5`**

Six datasets, built by `tools/build_hdf5_scale_offset_listing_fixture.py`.
`x` (6 values, 0 to 50 by 10) and `packed` (4 by 6, 100 to 105.75) are stored
with scale-offset. `y` (0 to 3) is plain. `field` (4 by 6 on `y` and `x`, 0 to
5.75 in steps of 0.25) is deflated. `unwritten` (4 by 6) is set to
scale-offset but has no chunk written, so it reads as its fill value, -1.
`skipped` (4 by 6, 200 to 205.75) is set to scale-offset, but its only chunk
was written without the filter. Right-click it, *Open With…*, *Fieldglass
Viewer*.

- [ ] **The page offers no undecodable variable.** The Render section has three
      buttons, `Render field`, `Render skipped` and `Render unwritten`, and no
      `Render packed`. The Variables table lists `field`, `skipped`,
      `unwritten` and `y`. Variables not decoded lists `packed` and `x`, each
      with `unsupported section: HDF5 filter id 6 is not supported (only
      deflate, shuffle, fletcher32, zstd, and szip are decoded)`. 0.6.1 had a
      Render button on `packed` too, listed all six as variables, and had no
      Variables not decoded table. Failure looks like a `Render packed`
      button, or `packed` in both tables.
- [ ] **No picker in the render panel offers it.** Click `Render field`. The
      variable picker lists `field`, `skipped` and `unwritten`, and not
      `packed`. Tick Compare: a NetCDF or HDF5 panel's Field B is the same
      variable at another slice, with no variable picker of its own, and
      `field` has no other dimension, so the box holds only its label. There
      is nothing more to check there (a NetCDF or HDF5 panel has no Vectors
      row either). Failure looks like `packed` in the variable picker.
- [ ] **priority** **A variable on an undecodable axis renders** (0.6.1:
      failed). The `field` panel opened above draws a 6 by 4 field in its
      own grid, rising from 0 at one corner to 5.75 at the opposite one, with
      no coastline, border or graticule offered over it. The note beside the
      variable picker reads `"x" cannot be decoded, so the field is not
      placed on the map by it.` Click a cell: the readout gives its value, a
      multiple of 0.25. 0.6.1 opened the panel and every render failed with
      `Error: unsupported section: HDF5 filter id 6 is not supported (only
      deflate, shuffle, fletcher32, zstd, and szip are decoded)` in the status
      line. Failure looks like that error, a field without the note, or the
      note on `skipped` or `unwritten`.
- [ ] **A variable the filter never ran on still renders.** Pick `skipped`
      in the variable picker: a 6 by 4 field from 200 to 205.75, with no note.
      Pick `unwritten`: every cell reads -1, the fill value, and there is no
      error and no note. Both drew the same in 0.6.1. Failure looks like
      either one refused with the filter's error, or `skipped` reading
      anything but 200 to 205.75.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_bzip2_chunks.nc`**

A NetCDF-4 file whose one variable, `bz` (4 by 6 on `y` and `x`, in four
chunks), is stored with netCDF-C's bzip2 filter (id 307), which Fieldglass
does not decode, and every chunk ran it.

- [ ] Open it. There is no Render section and no Variables table. Variables not
      decoded lists `bz` with `unsupported section: HDF5 filter id 307 is not
      supported (only deflate, shuffle, fletcher32, zstd, and szip are
      decoded)`. 0.6.1 listed `bz` as a variable with a Render button, and its
      render failed with that error. Failure looks like a `Render bz` button.

For a dataset of several chunks, the first block of chunks decides whether the
filter ran. The other cases (an LZ4 dataset whose first chunk skipped the
filter, an optional third-party filter libhdf5 skipped on every chunk) look on
the page like one of the files above, so they have no check of their own.
`crates/fieldglass-netcdf/tests/hdf5_unsupported_filter.rs` and
`crates/fieldglass/tests/undecodable_filter.rs` cover them, with
`render.test.ts` for the page and the pickers.

---

## No manual check

`[Unreleased]` entries, or parts of them, with no check above, and why.

- **The npm package's new calls** (#1750, #815, #811, #1785). Nothing in
  the editor calls `Handle.contoursLonLat`, `Handle.probeCombined` (the Compare
  click readout already shows the same value, and section 1 checks it),
  `decodeDisplayPrefix` or `Handle.messageListing` by itself. The conformance
  suite runs each against the Node addon and the browser module
  (`crates/fieldglass/conformance/probe_combined.json`,
  `message_listing.json`, and `suite.json`), `contours_lonlat.mjs` covers the
  contour points, and `crates/fieldglass-grib2/tests/decode_prefix.rs` with
  `crates/fieldglass-napi/src/display_tests.rs` cover a message cut off
  partway. Section 5 checks the extension's use of the message listing.
- **The npm package's faster reduced-grid hover** (#1796). It keeps the
  nearest-point search in `Session::probe`, which the browser host's hover
  uses; the extension's click readout goes through the render path and did not
  change with it. The values are the same:
  `crates/fieldglass/tests/reduced_grid_nearest_point.rs` and the
  conformance suite.
- **A scaled value's last bit in the library** (#1849), past what section 15
  checks: `FixedSurface::value`, `ScaledLimit::value`, the Earth's axes, and
  `verifyMessage`'s level and size comparisons in `fetchplan`. The
  `fieldglass-grib2` `pds` tests and the conformance files hold them.
- **`verifyMessage` accepting the ECMWF step of a statistic over runs** (#1624),
  and the structured statistical window behind the forecast column. It is a
  library call, and the Fcst column it feeds is unchanged for every committed
  file: `crates/fieldglass/tests/statistical_windows.rs`.
- **A slice request refused in the same words everywhere** (#1804). No step in
  the editor builds an invalid request, so no panel shows a refusal worded this
  way. `crates/fieldglass/tests/slice_request.rs` and
  `crates/fieldglass-netcdf/tests/slice_geometry_precedence.rs` hold the
  wordings and the order of the checks.
- **Export PNG after a revert during the save dialog** (#1315). VS Code reverts
  a custom editor only when it has unsaved edits, which a Fieldglass document
  cannot have, so nothing reaches this today. `revert.test.ts` drives it.
- **A request the panel cannot serve now gets a reply with no result** (#1808).
  No step in the editor leads there. `panel-replies.test.ts` drives it. Section
  12 checks that the requests that can be served still are.
- **A latitude a hair past a pole, a coordinate at its own fill value, and a 2-D
  latitude and longitude cell outside its `valid_range`** (#1305, #1727). No
  committed file draws differently by eye. Section 9 checks the notes and
  refusals that show. `crates/fieldglass-netcdf/tests/coordinate_missing_values.rs`
  and `coordinates_past_any_degree.rs` hold the rest.
- **The library side of #1855.** `Session::undecodable`, the hosts'
  `undecodable()`, `AttributesNotRead::undecodable`, `ArraySource::undecodable`
  and the `*_with_absent` placement calls have no use in the editor beyond what
  section 16 checks. `crates/fieldglass/tests/undecodable_filter.rs` and
  `crates/fieldglass-wasm/tests/node/netcdf.mjs` hold them.
- **The off-Earth readout** (#1676), past the part section 7 can check:
  `space-view-probe.test.ts`.
- **HDF5 szip written by the HDF Group's SZIP 2.x encoder** (#820). Fixtures and
  a test only: `crates/fieldglass-netcdf/tests/hdf5_szip.rs`. The decoder did
  not change.
- **A Zarr folder store's keys stay inside the folder on Windows too** (#1867).
  A key with a drive, a separator, a NUL byte, a part Windows reads as `..` or
  as a device, or one naming a pipe or a device link, reads as absent. The
  escape is Windows-only and needs a hand-built hostile store, and every
  committed store reads as before, so nothing shows by eye. The tests in
  `crates/fieldglass-napi/src/directory_store.rs` hold it:
  `a_key_cannot_escape_the_root`,
  `a_key_segment_windows_reads_as_a_prefix_parent_or_device_is_refused` (its
  escape and honest key lists) and
  `a_key_that_is_not_a_plain_file_is_absent_and_does_not_block`.
- **CI, tooling and refactors**: the semver job's crate list and job-name check
  (#1712, #1825), the runner pins to ubuntu-24.04 (#1836, #1839), the
  nightly sample sentinel (#1031), the wasm toolchain pin
  and its size gate (#1749, #1811), the probe-parity self-test (#1029), the
  perf harness and its szip rows (#1818, #764), the AEC oracle's path (#803),
  a committed libhdf5 oracle (#904), the fuzz job's artifacts and its
  debug-build timeout on a byte shuffle wider than its buffer (#1862), the
  reduced-grid rows of the probe-parity sweep
  (#1312, which section 0 runs) and the DTO mirror guard (#1806). Dependabot
  bumps of CI actions and the extension's dev tools (#1745, #1746, #1747,
  #1826, #1827, #1828) and the `h5netcdf` read in the sweep (#1311) are not
  user-visible.
- **The `napi` bump** (#1744: napi 3.16.0, napi-derive 3.7.0). The extension's
  addon is built with it, so every check above that opens a file runs through
  it, and there is no check of its own. The same PR held `wasm-bindgen` at
  0.2.129, with `js-sys`, so the npm package's bindings are as in 0.6.1.
- **Docs and Dependabot settings** (#1735, #1736, #1737, #1790, #1795, #1762,
  #1861, and this plan's own #1837, #1847, #1854 and #1859):
  nothing a user runs changes.

## Recording the outcome

Note pass/fail per section in the release PR or issue, and keep the record in
`.release-check/PROGRESS.md` so the next cycle can see what was actually
exercised. **Date that file and name the version it covers** — a stale progress
file reads as current and is the one trap this plan has actually sprung.

**What this plan cannot see, by construction:** the value-level cross-checks
against eccodes all run in CI. This plan is only for the things that need eyes —
whether a picture is in the right place and reads correctly.
