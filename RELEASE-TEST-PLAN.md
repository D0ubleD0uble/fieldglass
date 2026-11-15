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

# Full samples.
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

Every committed GRIB file holds **one** message, and so does every file
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
# octets. eccodes reads every one of them as the comments say.
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
# range, 22 and 23 N, the number of products a statistic combines. Off ECMWF, its local PDS extension (octets 29-52) is dropped and
# both lengths fixed, and the parameter is table 2 code 11 (temperature)
# unless given. The first message is byte for byte message 14 of 0.6.1's
# grib1_time_ranges.grib1.
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
# type 1 time range, in §4 from octet 47: length unit at +2, length at +3,
# increment unit at +7, increment at +8), with the length and the increment
# rewritten (#1627): every 24 hours over 720; over 24 every 6; over 24 every 24;
# over 24 every 48; over 24 every 1 day (unit 2).
monthly = Path("crates/fieldglass/tests/fixtures/monthly_mean_of_analyses.grib2").read_bytes()


def runs(length, increment, increment_unit=1):
    b = bytearray(monthly)
    at = section(monthly, 4) + 46
    b[at + 3:at + 7] = length.to_bytes(4, "big")
    b[at + 7] = increment_unit
    b[at + 8:at + 12] = increment.to_bytes(4, "big")
    return bytes(b)


write("grib2_runs.grib2", monthly, runs(720, 24), runs(24, 6), runs(24, 24),
      runs(24, 48), runs(24, 1, increment_unit=2))
# The first two of those, and the second alone: one file the panel reads, and
# the file it becomes (#1636).
write("stats_two.grib2", monthly, runs(720, 24))
write("stats_one.grib2", runs(720, 24))

# GRIB2 temperature 6 hours before the reference time and 6 hours after (#1616):
# the forecast time is §4 octets 19-22, sign and magnitude.
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
node tools/preflight_samples.js samples/0.6.0
( ulimit -v 12000000; python3 -I tools/probe_parity_sweep.py samples/0.6.0 )
```

`.h5` files do not open in Fieldglass by double-click: right-click the file,
*Open With…*, *Fieldglass Viewer*.

### Files by section

Each file is opened once; this is where its checks are.

| File | Sections |
|---|---|
| `S: samples/gfs.grib2` | 1, 12 |
| `S: samples/mirs_swath.nc` | 1 |
| `S: samples/oisst.nc` | 1 |
| `S: samples/rap.grib2`, `eccc.grib2`, `rrfs.grib2` | 13 |
| `S: samples/reduced_gg_n32_smooth.grib1`, `octahedral_gaussian_o32.grib2`, `reduced_ll_wave.grib1`, `octahedral_o1280.grib1` | 5 |
| `B: samples/0.6.0/gfs_wind.grib2` | 1, 6, 12 |
| `B: samples/0.6.0/ncep_air_daily_2024.nc` | 1, 8, 12 |
| `B: samples/0.6.0/ncep_air_levels_ltm.nc` | 11 |
| `B: samples/0.6.0/air_v3.zarr` | 1 |
| `B: samples/0.6.0/cmc_pair.grib`, `grib1_names.grib1` | 1 |
| `B: samples/0.6.0/grib1_unused_p1_p2.grib1` | 2 |
| `B: samples/0.6.0/six_fields.grib2` | 12 |
| `B: samples/0.6.0/grib1_one_product.grib1` | 4 |
| `B: samples/0.6.0/grib2_runs.grib2`, `grib2_leads.grib2`, `stats_two.grib2`, `stats_one.grib2` | 4 |
| `B: samples/0.6.0/halves.nc` | 8 |
| `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2`, `bifourier_ellipse_ieee32.grib2` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/reduced_latlon_10deg.grib2` | 5 |
| `F: crates/fieldglass-grib2/tests/fixtures/jpeg2000_regular_latlon.grib2`, `jpeg2000_local_40000.grib2` | 13 |
| `F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1` | 11 |
| `F: crates/fieldglass/tests/fixtures/uv_mismatch.grib2`, `uv_members.grib2`, `uv_derived.grib2` | 6 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc` | 1 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf_numeric_calendar.nc` | 3 |
| `F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc` | 7 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf_values_no_degrees.nc` | 9 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_huge_attributes.nc`, `netcdf4_damaged_text.nc` | 11 |
| `F: crates/fieldglass-zarr/tests/fixtures/windows/window_s2n_v3` | 9 |
| `F: crates/fieldglass-zarr/tests/fixtures/fixedscaleoffset`, `wrapped_shards` | 10 |
| `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` | 1 |
| The release dry run's `@fieldglass/wasm` tarball | 14 |

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
      on screen and in the PNG, a key reads `Contours every 10 K · 210.4 …
      309.7 K`, with your field's interval, range and units. Type an
      interval and the key follows; blank is the automatic spacing (#1463).
      The key's range is the color range the lines are spaced over, not
      always the data's. Set Color Range to Manual with min and max inside
      your field's range (say 250 and 300 on a 210 to 310 K field): the key
      reads `Contours every 5 K · 250.0 … 300.0 K`: the lines respace over
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
- [ ] On a file with two grids, set Compare to A − B with Field B on the other
      grid. The status line says "Error: …", and **Export PNG** says "Export
      failed: the last render failed; see the status line." and saves nothing.
      Pick a Field B on the same grid: the export works again (#1187).
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
      #934 regression). The heap and budget changes, and the one reader of a
      global-heap ID and the one all-ones helper for undefined addresses and
      unlimited sizes, sit on the path every NetCDF-4 attribute is read
      through. Open `S: samples/oisst.nc` in 0.6.1
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
      store that does not open.

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

Five more ways the Fcst column used to say less than the file did, or more.
Each is a lookup against a code table, not a formula.

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

      0.6.1 words 3 to 5 as `average of analyses every 24h over 24h`, `…
      every 48h over 24h` and `… every 24h over 24h`. Failure looks like a
      series on a row that cannot hold two runs, or the raw form on 2, which
      holds four. `statistical_windows.rs` (`a_series_that_cannot_hold_two_runs_is_shown_raw`)
      holds the rest, including two time ranges.

**`B: samples/0.6.0/grib2_leads.grib2`**

- [ ] **priority** Message 0 reads `−6h` and message 1 `+6h`. The minus is a true minus
      sign, the one a statistic's window uses, not a hyphen. 0.6.1 read
      `+-6h` on message 0.

**`B: samples/0.6.0/stats_two.grib2`**

0.6.1 gave two statistics that differ only in how far apart their fields are
the same identity, so a panel could draw the wrong one after its file changed.

- [ ] Copy `stats_two.grib2` to `samples/0.6.0/reload.grib2`, open the copy
      and render message 0 (every 6h over 720h). Overwrite the copy with
      `stats_one.grib2`, which holds only the every-24h statistic
      (`cp samples/0.6.0/stats_one.grib2 samples/0.6.0/reload.grib2`). The panel
      says `This message is no longer in the file (or is ambiguous); pick one.`
      and draws nothing. 0.6.1 draws the every-24h field under the every-6h
      heading. Delete `reload.grib2`.

---

## 5. Reduced grids (#1760, #1759, #1796, #811, #1312)

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
      Hover a cell in the second band: it reads a value from 100 to 105. Fail:
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
      points, as does the line to the row above or below. Hover across one of
      those lines: the value changes at the line, and the hover names the block
      under the pointer, never one a row away. 0.6.1 sometimes showed the
      point to one side of the nearest, such as 48°E where 54°E was nearer.
      Set Resampling to Bilinear: the blocks smooth out, and hover on a pixel
      still names the nearest point.
- [ ] The probe-parity sweep (section 0) checks 200 points of each against
      eccodes' `codes_grib_find_nearest`, which is what judges the nearest
      point to the meter. By eye, this check can only find a block that is off
      by a row.

**`S: samples/reduced_ll_wave.grib1`** and **`S: samples/octahedral_o1280.grib1`**

- [ ] `reduced_ll_wave.grib1` (significant wave height): land is masked, and
      the rows over the poles hold no points, so they stay transparent.
      Hover over an ice cap: no value. This file is the same in 0.6.1.
- [ ] `octahedral_o1280.grib1` is ECMWF's operational grid, 2560 rows and 6.6
      million points, and its field is constant, so it renders flat. Open it
      in 0.6.1 and in this build. The message table reads the same in both: the
      same Grid and Size cells, and the Render button. The page opens at least
      as fast, and so does the render panel and a hover (0.6.2 no longer
      sends the 2,560 row counts each time a file or panel opens, and keeps the
      grid's nearest-point search between hovers). Failure looks like a blank
      Grid or Size cell, which means the listing lost something it needs.

---

## 6. The Vectors row says where u and v differ (#1140, #1794, #1309)

The V component picker takes any message in the file. 0.6.1 drew arrows for a
10 m u paired with an 850 hPa v, a +204h u with a +210h v, or member 1's u with
member 7's v, and said nothing. The arrows are still drawn. The row now says
where each component is. The note comes from the file's own level, time and
member, a lookup and not a formula.

**`F: crates/fieldglass/tests/fixtures/uv_mismatch.grib2`**

Message 0 is a u at 10 m, +204h. Messages 1 to 6 are v: 1 the same level and
time, 2 at 850 hPa, 3 at +210h, 4 at 850 hPa and +210h, 5 at 10 m stated with
another scale factor, 6 at 10 m valid at the same instant from a run 6 hours
later. 9 is a v on the ground and 10 a v whose level is coded missing.

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

**`B: samples/0.6.0/gfs_wind.grib2`**

- [ ] Message 0 (UGRD) and V 1 (VGRD) are one level and one time, so the
      Vectors row has no note beyond what 0.6.1 shows. A real pair must not
      be flagged.

**`B: samples/0.6.0/gfs_wind.grib2`**, Arrows on, in a narrow panel

- [ ] **The Vectors row does not move the map during a zoom** (#1309).
      Narrow the editor until the Vectors row wraps onto two lines, then wheel
      the map in with the pointer held still, several ticks. The row's
      "Full-length arrow: …" text changes as arrows leave the window, and goes
      blank when none is in view. The map must not shift under the pointer:
      every tick zooms on the same place. 0.6.1 re-wrapped the row on some
      ticks, so the next tick landed somewhere else. The map does not move
      when you zoom back out either. `vector-row-zoom.test.ts` pins the row's
      height; the wheel by hand is the part no test drives.

---

## 7. A satellite view says where a cell is (#1676)

`F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc` is a 24 by 24
window of GOES-16 band 13, drawn in its own geostationary projection.

- [ ] Render `CMI` on Source projection and click the cell at grid 12,12. The
      readout reads `38.48°N, 74.99°W`, then the value and `grid 12,12`.
      0.6.1 gave the value and the grid cell only. Switch to Equirectangular
      and click the same cell: the position is the same, to as many decimals
      as that map gives. PROJ 9.4.0 places the cell at 38.479028°N,
      74.988007°W; the page may show fewer digits.
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
      looked alike. Press Animate: its label and the line under the heading
      carry the same fractions, and no two steps read alike.
- [ ] Export PNG at step 1 and at step 2: the suggested names end `-000000.5`
      and `-000001.0`, so two steps no longer share one. Cancel both dialogs.
- [ ] Open `B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`: the stepper still
      reads `2024-01-01` with no fraction. An axis a second or more apart
      reads as it did.

---

## 9. The note beside the variable picker says why a field is not placed (#1305, #1727)

A field whose coordinates no latitude or longitude can hold is drawn in its
own grid, with no map. 0.6.1 said why only when the coordinate had lost its
attributes. It now always does, and a latitude that states it is one and holds
a value past a pole is refused.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf_values_no_degrees.nc`**

- [ ] Render `bare`. The field is drawn in its own grid. The note beside the
      variable picker says `"x" holds longitudes from -2000 to 2000, more than
      twice around the globe`, and no coastline, border or graticule is
      offered over it.
- [ ] Render `attributed`. Its `lat` states `degrees_north` and holds 0 to 2000,
      so the panel is refused: the error and the note both say `"lat" holds
      latitudes from 0 to 2000, past the poles`. 0.6.1 drew the field with no
      word. Failure looks like a drawn field with no note, or `-0` in a number.

**`F: crates/fieldglass-zarr/tests/fixtures/windows/window_s2n_v3`**

- [ ] *Fieldglass: Open Zarr Store…* on that folder. The panel opens on `t2m`
      and the note beside the variable picker reads `The store holds no
      "latitude/c/2", so those values of "latitude" are its fill value, 0`.
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
one with `unsupported numcodecs id`.

- [ ] *Fieldglass: Open Zarr Store…* on the folder. All ten arrays are listed
      and none is under a refused or "not decoded" heading.
- [ ] Render `uint8` (4 by 6). The values rise by 0.1 from 1000 at one corner
      to 1002.3 at the other, in order, and the color bar spans those. Render
      `same_type` (20 by 30): a noisy field between about 250 and 310, with
      one empty cell. Compare `int16` with it: the same picture. They are the
      same values stored two ways.
- [ ] Render `cf_int`: four cells are empty (they decode to its fill value,
      which CF masks), and the rest of the values are the filter's codes
      scaled and offset by CF, such as 350, 516.5 and 450 across the first
      row, not the filter's own codes (900, 1233, 1100). A render that shows
      the codes means the CF scale and offset were skipped.
      `crates/fieldglass-zarr/tests/fixedscaleoffset.rs` compares every value
      with zarr-python's, so this check is that each array draws.

**`F: crates/fieldglass-zarr/tests/fixtures/wrapped_shards`**

Six Zarr v3 arrays whose shards are compressed whole (`gzip`, or `crc32c` and
`zstd`, after `sharding_indexed`). 0.6.1 read them as unsharded and reported
every shard as a corrupt stream.

- [ ] Open the folder. All six arrays are listed. Render `gzip` (6 by 8): the
      values rise from 1.0 by 0.25 along the rows to 12.75, except a 2 by 4
      block at the top of one side that reads -1, the fill value, because that
      inner chunk was never written. There is no `corrupt` or `inflates past`
      error. Render `crc32c_zstd`, `transposed`, `inner_gzip`, `index_start`
      and `plain_index`: each draws a field with no error.

---

## 11. The panel stays usable when narrow (#1729, #1529, #1677)

Narrow the editor by dragging its edge, or by opening the panel beside another
editor.

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, `air` (time, level, lat, lon)

- [ ] **Slice steppers stay on the page** (#1729). With two held dimensions,
      turn Compare on and narrow the panel to about 300 px, then to 280 px.
      Field A's and Field B's steppers, with their slider, number box and
      coordinate, stay inside the toolbar, and the page does not scroll
      sideways. A long coordinate such as `0 minutes since 1870-01-01 00:00`
      breaks at its spaces. 0.6.1 ran Field B's steppers a few pixels past the
      toolbar at 300 px, and pushed the number box and coordinate off the page
      at 280 px. `narrow-panel.test.ts` measures this at 300, 450 and 900
      px; 280 and the wrapped coordinate are by eye.
- [ ] **A cross-section keeps its image** (#1529). Set Y axis to `level` and X
      axis to `lat`, so the slice is a section and not a map, and narrow the
      panel to about 200 px. The image stays visible, the Y axis labels get
      narrow first, and the axis caption breaks between an axis and its
      units, never inside `(degrees_north)`. A date axis too wide for the
      labels gives its first and last date in the caption. Export PNG and save
      it: the line `Exported PNG to …` wraps and does not run past the panel's
      edge. 0.6.1 let the labels take the row until only a sliver of
      the color bar was left.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_huge_attributes.nc`**

- [ ] On its metadata page, a long global attribute is collapsed to a preview
      that stays inside its cell. The Global attributes table does not
      scroll sideways at any editor width down to about 320 px (#1677).

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_damaged_text.nc`**

- [ ] On its metadata page the `temperature` row names `Attribute "long_name"
      not read: the file's record of the text is damaged.` and Global
      attributes names the `title` the same way. Each ends in exactly one
      full stop (0.6.1 had none).

**`F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1`**, a constant field

- [ ] Render it and Export PNG (save it). The color bar's label shows the one
      value once, not three times over each other. Open the exported PNG to
      see it. `small-layout.test.ts` covers this, and the panel's contour
      key: on `gfs.grib2` tick Contours with an interval of 5, and narrow the
      panel until the key wraps. It breaks between words, never inside
      `Contours`.

---

## 12. Stepping through GRIB messages, and the panel's requests (#1803, #1808)

0.6.1 kept every GRIB message's values it had drawn, probed or exported until
the file closed. It now keeps the last four, and fewer when they are large, so
stepping back to an old message decodes it again. That must change nothing the
eye sees. How much memory it holds cannot be judged by eye;
`crates/fieldglass-napi/src/warp_memo_tests.rs` pins the bound.

**`B: samples/0.6.0/six_fields.grib2`** (six different fields)

- [ ] Render message 0 on Equirectangular and note its hover value at one
      point. Render 1, 2, 3, 4 and 5 in turn, and render 0 again. The picture
      and the value at that point are what they were. Failure looks like a
      message drawn as another's, or a value that changed. By the
      time you return the memo has let go of message 0, so it is decoded
      again, which is the path to watch.

**`S: samples/gfs.grib2`** and **`B: samples/0.6.0/ncep_air_daily_2024.nc`**

- [ ] **Every request the panel sends is answered** (#1808). Both render
      panels now take one typed request dispatch. On `gfs.grib2` tick
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
      `DATE=$(date -u +%Y%m%d) tools/fetch_samples.sh rrfs`. There is no
      `samples/nam.grib2` any more.

---

## 14. The npm package carries Rust's license notice (#1749)

From the release workflow's dry run (RELEASING.md section 2), download the
`@fieldglass/wasm` tarball artifact.

- [ ] `tar -tzf fieldglass-wasm-*.tgz` lists `package/COPYRIGHT-library.html`
      beside `LICENSE-APACHE`, `LICENSE-MIT` and `NOTICE`.
      `tar -xzOf fieldglass-wasm-*.tgz package/package.json` has a `rustc`
      field naming release 1.99.1 and its commit. `package.mjs` checks both on
      every pull request, so this is the look at the real artifact.

---

## No manual check

`[Unreleased]` entries, or parts of them, with no check above, and why.

- **The npm package's new calls** (#1750, #815, #811, #1785). Nothing in
  the editor calls `Handle.contoursLonLat`, `Handle.probeCombined` (the Compare
  hover already shows the same value, and section 1 checks it),
  `decodeDisplayPrefix` or `Handle.messageListing` by itself. The conformance
  suite runs each against the Node addon and the browser module
  (`crates/fieldglass/conformance/probe_combined.json`,
  `message_listing.json`, and `suite.json`), `contours_lonlat.mjs` covers the
  contour points, and `crates/fieldglass-grib2/tests/decode_prefix.rs` with
  `crates/fieldglass-napi/src/display_tests.rs` cover a message cut off
  partway. Section 5 checks the extension's use of the message listing.
- **`verifyMessage` accepting the ECMWF step of a statistic over runs** (#1787),
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
- **The off-Earth readout** (#1676), past the part section 7 can check:
  `space-view-probe.test.ts`.
- **HDF5 szip written by the HDF Group's SZIP 2.x encoder** (#820). Fixtures and
  a test only: `crates/fieldglass-netcdf/tests/hdf5_szip.rs`. The decoder did
  not change.
- **CI, tooling and refactors**: the semver job's crate list and job-name check
  (#1712, #1825), the nightly sample sentinel (#1031), the wasm toolchain pin
  and its size gate (#1749, #1811), the probe-parity self-test (#1029), the
  perf harness and its szip rows (#1818, #764), the AEC oracle's path (#803),
  a committed libhdf5 oracle (#904), the reduced-grid rows of the probe-parity sweep
  (#1312, which section 0 runs), one HDF5 heap-ID reader (#1805), one
  all-ones helper (#934) and the DTO mirror guard (#1806). Dependabot bumps and
  the `h5netcdf` read in the sweep (#1311) are not user-visible.

## Recording the outcome

Note pass/fail per section in the release PR or issue, and keep the record in
`.release-check/PROGRESS.md` so the next cycle can see what was actually
exercised. **Date that file and name the version it covers** — a stale progress
file reads as current and is the one trap this plan has actually sprung.

**What this plan cannot see, by construction:** the value-level cross-checks
against eccodes all run in CI. This plan is only for the things that need eyes —
whether a picture is in the right place and reads correctly.
