# Manual test plan — everything that changed since v0.5.0

**Baseline: v0.5.0 (2026-09-04).** This plan is rewritten each cycle to cover
what landed since the last tag, so it is deliberately near-empty right after a
release. Add a section as each user-facing feature merges, rather than trying to
reconstruct the cycle from the log at prep time — the previous cycle's plan
found three defects that way, and all three were things a reader of the diff
would not have thought to look for.

`samples/README.md` has a per-file "does it look right" checklist for coverage
that shipped **at or before** 0.5.0. This plan does not repeat it beyond the
regression floor below.

---

## How to add to this plan

One section per feature, named for the issue. Under each, group checks by the
**file they need**, so a file is opened once and every check it can serve runs
together. For each check, say what to look at and what failure looks like —
"renders correctly" is not a check anyone can fail honestly.

Before writing a check, **open the fixture and confirm it can show the thing**.
A constant field, a single-message file, or a domain that never reaches the
latitude in question will all pass a visual check while proving nothing.

Mark a check **priority** when it covers a path no automated test can judge:
something resampled, interpolated, or placed by a lookup rather than a formula.
Those are where the last cycle's defects were.

State explicitly where a check **cannot** be made by eye, and name the test that
covers it instead, rather than leaving a reader to attempt a judgement the data
cannot support.

---

## 0. Setup (once)

```sh
# From the repo root, at the commit being released (RELEASING.md's
# $RELEASE_SHA, the prep merge), not whatever master holds now.
git fetch origin && git checkout "$RELEASE_SHA"

# Build the native module into the extension so the dev host runs current Rust,
# then compile the TypeScript.
( cd crates/fieldglass-napi && npx napi build --platform --release \
    --target x86_64-unknown-linux-gnu --output-dir "$(git rev-parse --show-toplevel)/extension/bin" )
( cd extension && npm ci && npm run compile )

# Full samples.
tools/fetch_samples.sh

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js
# Run it again over samples/0.6.0 once the next step has built those files.
```

Then `F5` to launch the dev host.

Paths below are relative to the repo root. `F:` marks a committed crate fixture
(always present); `S:` marks a `samples/` file (needs the fetch above); `B:`
marks a file the next step builds into `samples/0.6.0/`.

### Build the files this cycle's checks need

Every committed GRIB file holds **one** message, and so does every file
`fetch_samples.sh` writes. That includes `cmc_wind_300_2010052400_p012.grib`,
which is wind *speed* at 300 hPa, not a u/v pair. Arrows, Compare on GRIB and
the reload checks all need two or more messages, and no committed NetCDF or Zarr
file has a real time or level axis to animate. This step builds those files.
Everything lands in `samples/0.6.0/`, which git ignores. It needs `curl`, and
`xarray`, `zarr` 3 and `netCDF4` in the `python3` you run it with. If that
Python lacks them, make a throwaway environment for this step:
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
# id 229 (swh); DWD's own table 129; Meteo-France with ECMWF as sub-centre;
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

# GRIB2 temperature with discipline 191, then 99.
latlon = (g2 / "regular_latlon_surface.grib2").read_bytes()
write("grib2_codes.grib2", patched(latlon, [(6, 191)]), patched(latlon, [(6, 99)]))

# Ensemble members 7 and 8 (template 4.1 octet 36), in both orders and doubled.
member = (g2 / "ensemble_member_4_1.grib2").read_bytes()
m8 = patched(member, [(section(member, 4) + 35, 8)])
write("members_7_8.grib2", member, m8)
write("members_8_7.grib2", m8, member)
write("members_8_8.grib2", m8, m8)

# A colour table in a model Fieldglass refuses.
(out / "hsv.cpt").write_text("# COLOR_MODEL = HSV\n0 0-1-1 1 360-1-1\n")
EOF

# Every file above decodes and renders headlessly.
node tools/preflight_samples.js samples/0.6.0
```

`.h5` files do not open in Fieldglass by double-click: right-click the file,
*Open With…*, *Fieldglass Viewer*.

### Files by section

Each file is opened once; this is where its checks are.

| File | Sections |
|---|---|
| `S: samples/gfs.grib2` | 1, 3, 7, 8, 38 |
| `S: samples/hrrr.grib2` | 8, 21 |
| `S: samples/mrms.grib2` | 7, 28 |
| `S: samples/ecmwf.grib2` | 17 |
| `S: samples/goes.nc` | 21 |
| `S: samples/rtofs_ice.nc` | 41 |
| `S: samples/mirs_swath.nc` | 1 |
| `B: samples/0.6.0/ncep_air_daily_2024.nc` | 1, 2, 3, 4, 5, 7, 8, 46 |
| `B: samples/0.6.0/ncep_air_levels_ltm.nc` | 3, 4, 5 |
| `B: samples/0.6.0/air_v2.zarr`, `air_v3.zarr` | 2, 5 |
| `B: samples/0.6.0/gfs_wind.grib2`, `hrrr_wind.grib2` | 1, 6, 7, 21, 45, 46 (`gfs_wind` only) |
| `B: samples/0.6.0/cmc_pair.grib` | 6, 24 |
| `B: samples/0.6.0/gfs_unnamed_pair.grib2` | 6 |
| `B: samples/0.6.0/hrrr_frames.grib2`, `two_grids.grib2` | 6 |
| `B: samples/0.6.0/spectral_pair.grib2` | 16 |
| `B: samples/0.6.0/grib1_names.grib1` | 11, 12, 13, 15 |
| `B: samples/0.6.0/grib2_codes.grib2` | 13, 14 |
| `B: samples/0.6.0/members_*.grib2` | 23 |
| `B: samples/0.6.0/hsv.cpt`, `F: crates/fieldglass-core/tests/fixtures/cpt/*.cpt` | 9 |
| `F: crates/fieldglass-zarr/tests/fixtures/stores/*` | 2 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc` | 18, 19, 20, 22 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc` | 3, 4, 27, 44 |
| `F: crates/fieldglass-netcdf/tests/fixtures/goes_geostationary*.nc`, `goes16_abi_cmip.nc` | 21, 40 |
| `F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2` (and the `tiny_earth_pair.grib2` built from it) | 19, 20, 39 |
| `F: crates/fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1`, `F: crates/fieldglass-grib2/tests/fixtures/j_consecutive_latlon.grib2` | 35 |
| `F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_*.grib1`, `F: crates/fieldglass-grib2/tests/fixtures/*reduced_gaussian.grib2` | 8, 36, 37 |
| `F: crates/fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1` | 13 |
| `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` | 1, 15, 44, 45 |
| `F: crates/fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc` | 44 |
| `F: crates/fieldglass-grib2/tests/fixtures/ccsds_flags46_12bit.grib2` | 17 |
| `F: extension/src/test/fixtures/healpix_n4_ring.grib2`, `netcdf_classic_dummy.nc` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2`, `bifourier_ellipse_ieee32.grib2` | 1, 16 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc` | 22, 25 |
| `F: crates/fieldglass-netcdf/tests/fixtures/fill_only_*.nc` | 27 |
| HDF5, NetCDF-4 and classic NetCDF fixtures under `crates/fieldglass-netcdf/tests/fixtures/` (including `numrecs_*.nc`) | 10, 26, 29, 30, 31, 32, 34, 34a |

---

## 1. Regression floor

Carried forward each cycle. This is the minimum for a release whose diff is
small, and it is *not* a substitute for the per-feature sections below it.

- [ ] `S: samples/gfs.grib2` — renders, reprojects, coastlines align.
- [ ] **Controls a grid can't use are hidden, and the panel says why**
      (#1036). On `gfs.grib2` every control shows except Resampling, which is hidden
      while Projection is on Source projection (next step), and there is no "Not
      available for this grid" line under the caption. On
      `S: samples/mirs_swath.nc`, Zonal average and Resampling are hidden;
      the line under the caption reads `Not available for this grid:
      Resampling, Zonal average`, starts closed, and opens (click, or Tab to
      it and press Enter) to `Resampling: this grid places each cell by
      its own coordinates, so the map always shows the nearest cell.` and
      `Zonal average: this grid's rows aren't circles of latitude.` Nothing
      is greyed out.
- [ ] **Resampling shows only when the projection warps** (#1091). On
      `gfs.grib2` with Projection on Source projection there is no
      Resampling picker. Choose Equirectangular: it appears. Set it to
      Bilinear, switch to Source projection and back: it is still Bilinear and
      the map is the same as before. Do the same on a NetCDF slice
      (`B: samples/0.6.0/ncep_air_daily_2024.nc`, `air`).
      Nothing else in the toolbar leaves a blank gap or stays visible when it
      should be gone.
- [ ] **No Resampling on a grid of cell centres** (#1225). On
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
      render, so the Size column is all there is to check on it.
- [ ] **A coarse grid reprojects at display scale** (#514). The HEALPix file
      above in **Orthographic**: the globe's edge is a smooth circle, not a
      staircase, and the coastlines follow it rather than floating over blank
      pixels. The data stays blocky — 192 pixels is all there is — and the
      source view shows that honestly.
- [ ] `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` — classic NetCDF
      metadata view renders.
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` —
      the canonical GRIB2 path renders.
- [ ] **Difference map, on NetCDF**: `S: samples/mirs_swath.nc` → `BT` at two
      different `Channel` indices, **Compare**. The difference renders and the
      probe reads the *combined* value, not field A (#329).
- [ ] **Difference map, on GRIB**: `B: samples/0.6.0/gfs_wind.grib2`, message 0
      (UGRD) **Compare** A − B against message 1 (VGRD). The difference renders,
      and the probe at one point reads u minus v there, not u.
- [ ] **Export PNG** from one render; the file opens and matches what was on
      screen.
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

---

## 2. Open a Zarr store (#246, #657, #658, #659, #704)

Each store is opened with *Fieldglass: Open Zarr Store…* from the command
palette, picking the store's folder.

- [ ] First, from a cold window: *Developer: Reload Window* with no Fieldglass
      file open, then run *Fieldglass: Open Zarr Store…*. The folder dialog
      opens, with no "command not found". Reload again and do the same with
      *Fieldglass: Import Color Table…* (cancel the dialog).

**`B: samples/0.6.0/air_v2.zarr`** (consolidated v2, written by xarray)

- [ ] A render panel opens on `air`, a global temperature map 144 × 73, with
      the poles cold and the tropics warm. Coastlines align on Equirectangular
      and Orthographic.
- [ ] The slice picker lists `time` with 366 steps. Stepping to another day
      changes the map.
- [ ] **priority** Set `time` to 14 and probe 40° N 100° W. It reads about
      246.6 degK. Note the value.
- [ ] With `time` at 14, the stepper reads `2024-01-15` beside the number,
      and the line under the heading ends `· time 2024-01-15` (#1041).
      Failure looks like the index alone, or a count of hours.
- [ ] Export CSV… sits beside Export PNG…, in the top toolbar row (#1039).
      In 0.5.0 it sat at the end of the slice row.
- [ ] Export CSV…, long layout: 10,512 data rows (`grep -c '^[-0-9]'`), one
      per cell of 144 × 73. The matrix layout exports too.
- [ ] With `time` at 14, Export CSV…: the save dialog opens inside the store's
      folder on `air_v2_air_time-2024-01-15.csv`. Type `out` and save: the
      file is `out.csv`. In 0.5.0 the dialog suggested nothing, and on Linux
      it saved `out` with no extension.
- [ ] Export PNG: the save dialog opens inside the store's folder on
      `air_v2_air_time-2024-01-15.png`. Step `time` to 15: it offers
      `air_v2_air_time-2024-01-16.png`, not the same name again (#1112).
      Typing `out` saves `out.png`.

**`B: samples/0.6.0/air_v3.zarr`** (unconsolidated v3) and
**`B: samples/0.6.0/ncep_air_daily_2024.nc`**

- [ ] **priority** The same day and point read the same value in both, and the
      same as the v2 store. The three images match pixel for pixel by eye,
      including where the date line falls. A store written by xarray is placed
      exactly as the NetCDF file it came from; a mismatch here is a placement
      bug, not a decode one (`crates/fieldglass/tests/zarr_netcdf_twins.rs`
      checks the committed pair).
- [ ] With all three open on `air`, each says which it is (#1038). The tabs
      read `air · air_v2.zarr`, `air · air_v3.zarr` and
      `air · ncep_air_daily_2024.nc`; each heading reads `air (degK)`, and the
      line under it is the store or file name. No panel says "Message"
      anywhere. Pick `time_bnds` in one: its tab becomes
      `time_bnds · <name>`, and Export PNG offers `<stem>_time_bnds.png`
      (`air_v2_time_bnds.png`). In 0.5.0 every heading read `Message 0 — air`
      and the tab kept `Render: air`.
- [ ] In `ncep_air_daily_2024.nc`, pick `time_bnds` (2 × 366): the image is
      a column about eight times as high as it is wide that fits the panel's
      height, two columns of colour, with the colour bar beside it. Failure
      looks like a blank white box with a broken-image glyph, about 140,000 px
      tall, or a column several screens tall. Click near the top of the left
      column and near the bottom of the right one: each readout gives a
      different value, and the grid indices are the cell clicked. Set Y axis
      to `nbnds` and X axis to `time`: a wide band about an eighth as tall as
      it is wide, two rows, not a strip a few pixels high. Export PNG still
      writes the raster at its own size, 2 × 366 (#1226;
      `thin-slice.test.ts` measures the image and clicks its cells).
- That a lat/lon map more than eight times as long one way as the other
  keeps its shape on Equirectangular and Web Mercator cannot be checked with
  these files: a regular grid's map takes the grid's own shape, 2:1 for
  `air`, whatever the Bounds. `thin-slice.test.ts` draws a belt of 144 × 17
  cells round the globe and a strip of 9 × 145 and checks both keep their
  shape, the tall one within the panel's height (#1226).
- [ ] Copy `air_v2.zarr` to a folder named `air_copy` (no `.zarr` suffix) and
      open that: it opens the same way. The suffix is not what the check uses.

**`F: crates/fieldglass-zarr/tests/fixtures/stores/`**

- [ ] `cf_v3` (consolidated v3) and `v2_slash` (unconsolidated v2) each open
      and draw. `cf_v3`'s `t` reads in physical units (it is packed with
      `scale_factor` 0.25 and `add_offset` 200).
- [ ] `v3_v2keys` (unconsolidated v3, no dimension names): the slice picker
      names the axes `phony_dim_0` and `phony_dim_1`, as NetCDF names them.
      Both are image axes, so there is no stepper. A stepper on an axis with
      no coordinate variable shows its index alone, with no value beside it;
      `slice-coordinates.test.ts` checks one on a 3-D store (#1041).
- [ ] `v3_sharded`: `s` draws a 7 × 5 source view with no error. Whether the
      sharded values are right cannot be judged by eye; `fieldglass-zarr`'s
      `tests/real_stores.rs` and `tests/stores.rs` check them against
      zarr-python.
- [ ] `v2_problems` holds nothing that can be drawn: opening it shows a
      warning that starts "Nothing in this store can be drawn as a map." and
      then says why for each of its five arrays: `names holds text, not
      numbers; x2 gives dimension "x" a different length from another
      array; and bz, ok and x1 have only one dimension.` The warning has
      no codec or data type names such as `<U4` or `bz2`. Click Details: a
      dialog gives the store's path and each array's reason in the reader's
      words (`names` holds `<U4` text, `x2` gives `x` a second length, and
      the three it read have 1 dimension). No empty panel opens (#1040,
      #1129).
- [ ] Open `crates/fieldglass-zarr/tests/fixtures/stores`, a folder of
      stores: the message reads `"stores" isn't a Zarr store. It contains 8
      Zarr stores: cf_v2, cf_v3, v2_nested, v2_problems, v2_slash, v3_nested,
      v3_sharded, v3_v2keys. Pick one of those.` It names no metadata files.
      Details gives the path and the files a store starts with. Nothing
      opens. Before #1040 the message named `zarr.json`, `.zgroup` and the
      rest and explained the format.
- [ ] Open `crates/fieldglass-zarr/tests/fixtures`: the message counts its
      stores, names the first 8 and says how many more. Open a folder with
      no stores in it, such as `crates/fieldglass-zarr/src`: the message
      says it isn't a Zarr store and to pick the folder that holds the
      store's data.
- [ ] Make a folder with 210 empty subfolders (`d000` to `d209`) and a Zarr
      store named `zzzz` beside them, and open it: the message says it isn't a
      Zarr store and that Fieldglass looked at only the first 200 folders in
      it and found no Zarr store. Details adds that only the first 200, in
      name order, were checked (#1135).

The five blosc compressors and the other codecs are checked value for value in
CI (`fieldglass-zarr` `tests/`); there is nothing more to see by eye.

## 3. Animate through a dimension (#170, #1118)

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, variable `air`

- [ ] The panel shows an Animate row with first, back, play/pause, forward and
      last buttons, a speed control and a Loop toggle. It starts on `time`.
- [ ] The five buttons draw matching icons, `|◀◀`, `|◀`, `▶`, `▶|` and
      `▶▶|`: the same height, in the middle of each button, and in the
      button's text colour. Back and forward mirror each other, and so do
      first and last. Press play: the icon becomes two bars and the button
      keeps its size. Press it again to stop. Switch to a light theme (and
      back to a dark one): the buttons and their icons take the theme's colours in
      both, with no light-grey buttons in the dark theme, and Export PNG, Export
      CSV and the zoom buttons match. Failure looks like `⏮`/`⏭` drawn
      small and above the middle, or a back arrow with no bar (#1042), or
      light-grey buttons in a dark theme (#1118).
- [ ] Keyboard and high contrast (#1118). In the Dark Modern theme, click
      the heading (it opens nothing), then press Tab along the toolbar: focus
      goes to Export PNG…, Export CSV…, then on through the selects to the
      Animate buttons and the zoom buttons, and each button shows a ring
      while it has focus. Do not click Export PNG… to place focus: it opens a
      Save dialog. Switch to the High
      Contrast theme (Ctrl+K Ctrl+T): every button has a clear border.
      Failure looks like a light-grey button, no ring on Tab, or a button with no border in High
      Contrast. `panel-button-theme.test.ts` (`every button takes the
      theme's colours in light, dark and high-contrast themes, and the
      themes differ`) checks the same in all four themes.
- [ ] Press play: the label beside Loop counts `2024-01-02 (step 1 of
      0–365)`, `2024-01-03 (step 2 of 0–365)`, …, the `time` stepper and its
      date follow, and so does the date at the end of the line under the
      heading. The Northern Hemisphere warms towards July (#1041).
- [ ] With the colour range on automatic, the colour bar's numbers do **not**
      change during playback: the range of the step playback started from is
      held, so frames stay comparable. A start frame with one value in every
      cell has no range to hold and each frame sets its own (#1155). No sample
      has such a frame and no test drives it; `extension/src/test/suite/animation.test.ts`
      (`a range locked for playback holds across frames with different
      extremes`) covers the hold itself.
- [ ] Press play from step 0 and pause a few steps on: the paused frame keeps
      the held range. Click another editor tab in the same group and come
      back: the colour bar reads the same numbers as before the switch (#1227).
      Step forward by hand, or change the colour map or the range: that frame
      then takes its own range, and keeps it through another tab switch.
      `animation.test.ts` (`a paused frame keeps the range playback held
      through a hide/show and a rebuild`) drives the same steps.
- [ ] Faster and slower speeds change the frame rate. With Loop on (the
      default), step 365 (2024-12-31) is followed by step 0; with Loop off,
      playback stops on step 365.
- [ ] Choose 5 / s, untick Loop and tick Zonal average. Click another editor
      tab in the same group and come back: all three are as you left them
      (#1027).

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, variable `air`

- [ ] Choose `level` as the dimension to step through: 17 levels, 1000 to 10
      hPa. Set `level` to 5: the stepper reads `500 millibar` beside the
      number, the Animate label `500 millibar (step 5 of 0–16)`, and the line
      under the heading ends `· level 500 millibar`. Export PNG: the picture's
      subtitle says the same (#1041). The `time` stepper reads `0000-12-30`
      for the first month, where the file means `0001-01-01`: a date before
      1582 is counted on the proleptic Gregorian calendar, not the CF
      standard one. That is a known gap, beside #1033; note it, not a fail.
      Step with the forward button rather than Play, so each level takes its
      own range. Probe a point on the equator: about −80 °C at 100 hPa,
      warmer again above (about −45 °C at 10 hPa). This file is in `degC`.
      During Play the first level's range is held, so the upper levels paint
      in the bottom colour; that is intended.
- [ ] Choose Compare `A − B` and set Field B's `level` to 2. Field B's
      steppers read `Field B · time …` and `Field B · level` with `850
      millibar`, sit in a tinted box with a coloured rule down its left, and
      have a `·` and a gap between them. Failure looks like steppers that
      match the main ones, or `0–11level` run together. The line under the
      heading ends `· Field B at level 850 millibar` (#1041). Set Compare
      back to off.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc`**, `t2m`

- [ ] Play from step 0. Every step but 7 and 119 has no data and shows "no
      values" with no colour bar (#871). Step 7 shows bands of latitude; step
      119 the same bands 100 higher. Playback does not stall or queue frames.

**`S: samples/gfs.grib2`**, the file's metadata view (the page that opens
first)

- [ ] Click a message row to open it: the file page's buttons (Render, Export CSV…) in the Dark Modern theme
      take the theme's button colours, with no light-grey button (#1118).
      Press Tab: each button shows a ring while it has focus. In the High
      Contrast theme each has a clear border. The NetCDF page's Render
      buttons use the same style and are not opened separately;
      `panel-button-theme.test.ts` (`the dump page's buttons take the
      theme's colours in the same themes`) reads the GRIB and NetCDF pages in
      all four themes.

## 4. Click a cell for a line plot (#172, #1037, #1043)

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, `air`

- [ ] A Line plot box sits beside Zonal average, unticked. Click a cell: the
      readout gives the value and no plot appears, as in a GRIB panel.
- [ ] Tick Line plot. The plot appears at once for the cell you clicked. Click
      a cell in Siberia (60° N 100° E): the plot under the readout, above the
      map, is a time series over 2024, cold in January (about 250 K) and warm
      in July (about 293 K). Above it, `air at this point, across time`, with
      `time` as the axis picker. The caption reads `air across time (hours
      since 1800-01-01 00:00:0.0) · 238.35 to 300.83 degK · 366 of 366
      points`. Time runs along the bottom, ticked with 2024 dates from
      `2024-01-01` to `2024-12-31`, not raw hours (#1028), and the values are
      ticked up the side. No two labels touch (#1043).
- [ ] Click another editor tab in the same group and come back: Line plot is
      still ticked. Untick it: the plot goes, and the next click only reads
      the value. Tick it again for the checks below.
- [ ] A dashed rule marks the day on screen. Step to another day: the plot
      clears with the new render. Click the same cell again and the rule is at
      the new day.

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, `air`

- [ ] With Line plot ticked, click 30° S 25° E and choose `level` as the
      axis: an upright profile (#1043). Level runs up the side, ticked in
      millibar with `1000` at the bottom and `10` at the top, spaced by log
      pressure, so `100` sits halfway up. The surface is at the bottom near
      +31 °C, the line cools to its coldest near 100 mb, and the values are
      ticked along the bottom, a negative one with a true minus (`−50`). The
      dashed rule is level, across the plot at the level on screen. Failure
      looks like a line drawn sideways with `1000 … 10` under its left end.
- [ ] The caption's range reads like `−72.8 to 31.5 degC`, with "to" between
      the numbers and a minus sign on the first only. Failure looks like
      `-72.769–31.484`, which reads as two negative numbers.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc`**, `t2m`

- [ ] With Line plot ticked, click any cell and plot along `time`: two
      points, at steps 7 and 119, and no line drawn between them. Gaps stay
      gaps.

## 5. Any two dimensions as axes (#171, #1099, #1043, #1229, #1258)

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, `air`

- [ ] On the map, choose Mollweide and set Resampling to Bilinear (used by
      the last two checks below).
- [ ] Set the axes to `time` (X) and `lat` (Y). The panel says
      `Cross-section: these axes are not a map, so it is drawn in index
      space.`, hides the projection picker, Resampling, the map overlays and
      Zonal average (the line under the caption names them; nothing is
      greyed out), and draws a
      time–latitude plot: the
      seasonal swing is strongest at high latitudes. Latitude ticks read
      round values from 90 to −90 (`90, 60, 30, 0, −30, −60, −90` at full
      width), with `0` level with the middle of the image (#1229), and the
      time ticks read as dates, though this file's time reference has
      fractional seconds (#1028). Failure looks like `90, 55, 17.5, −17.5`,
      the latitudes of evenly spaced rows. Before this change the plane
      was placed on the Earth, reading time as longitude.
- [ ] The end labels stay beside and under the image (#1043): `−90` and
      `2024-01-01` do not overprint in the bottom-left corner, `2024-12-31`
      ends at the image's right edge rather than under the colour bar, and
      `90` does not reach up into the Line plot box above. Drag the panel
      narrower: labels that no longer fit are dropped, and none overlap.
- [ ] Click a cell: the readout gives the value and its grid cell, with no
      longitude. With Line plot ticked, the plot is offered along `lon`.
- [ ] Do the same on `B: samples/0.6.0/air_v2.zarr`, whose time units have no
      fractional seconds: its time ticks read as dates.
- [ ] Back on `ncep_air_daily_2024.nc`, set the axes to `lon` and `lat`: the
      map, the projection picker and the overlays return, and the map is drawn
      in Mollweide again, with the picker showing it and Resampling back on
      Bilinear (#1099). Failure looks like a map in Source projection, or a
      picker reading Source projection.
- [ ] Set the axes to `time` and `lat` again, hide the panel's tab behind
      another and show it, then set the axes back: still Mollweide (#1027,
      #1099).

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, `air`

- [ ] Axes `lat` (X) and `level` (Y): a latitude–height section labelled in
      degrees and millibar, with the ground at the bottom (#1043): `1000` at
      the foot of the level ticks and `10` at the top, warm along the bottom,
      without touching Flip Y axis. Tick Flip Y axis: the section turns over,
      `1000` at the top. Untick it again for the next check. Failure looks like
      1000 mb at the top by default.

A depth axis with fill values used to fail to draw; no file here has one.
`crates/fieldglass/tests/cross_section_placement.rs` covers it. No file here
has an unevenly spaced depth axis either, which a rail labels at its own points
rather than at round depths that would crowd its deep end (#1258);
`extension/src/test/suite/cross-section.test.ts` covers it.

## 6. Wind and current arrows (#241, #793, #805, #1049, #1051, #1142)

**`B: samples/0.6.0/gfs_wind.grib2`** (earth-relative)

- [ ] Open message 0 (UGRD). In the Vectors row pick `#1 · VGRD …` as V
      component (it starts on message 0, the u field itself), then tick
      Arrows: arrows over the field, and a key in the map's lower-left corner:
      an arrow in the arrows' colour and weight, labelled `… m s⁻¹`, as long
      as the longest arrow on the map (#1046, #1159). Arrows of about the
      same speed are about the same length everywhere, near the poles as at
      the equator. The Vectors row reads
      "Full-length arrow: … m s⁻¹". A larger "every N cells" draws fewer,
      longer arrows, and the key lengthens with them. No note about grid axes
      shows: GFS states earth-relative components. No note about the pair
      shows either: UGRD and VGRD are a u/v pair (#1049).
- [ ] Open message 1 (VGRD) and pick `#0 · UGRD …` as V component, then tick
      Arrows: the arrows draw, and the Vectors row reads "These messages are a
      u/v pair the wrong way round: VGRD is the v component and UGRD the u.
      For arrows that point the right way, open UGRD and pick VGRD as V."
      Untick Arrows: the note goes (#1049).
- [ ] Change the arrow colour and weight: the arrows and the key redraw in
      them. Export PNG: the arrows and the key are in the image, the key at
      the right length for its label beside the arrows, and its label readable
      however large the panel is (#1046). Untick Arrows: the arrows, the key
      and the row's "Full-length arrow" text all go.
- [ ] Zoom in twice with +, then switch to Orthographic: each time the key's
      length changes with the arrows' (#1046). Back on Equirectangular, keep
      zooming in with + until it stops zooming (a hundredth of a degree):
      after the first few clicks the arrows and the key stop growing, at a
      quarter of the map's shorter side, and the arrows spread apart instead. An
      arrow in view keeps its head, the key stays whole inside the map's
      corner with its label beside it, and its speed is the one the Vectors
      row gives (#1168). Export PNG there: the key is inside the map in the
      image too. Zoomed into a window without the fastest arrow, or on a
      globe with it on the far side, the key is still as long as an arrow at
      its speed is drawn on that view, and longer than every arrow shown. On
      Equirectangular and on Orthographic, no arrow is longer than the key,
      the polar ones included (#1159). Narrow the panel until the map is too
      small for the key and its label: the key goes and the Vectors row reads
      "No arrow key: the map is too small to fit one." Failure looks like a
      key that keeps one length while the arrows grow or shrink, polar arrows
      on Equirectangular stretched east–west past the key's length, or, zoomed
      in, arrows drawn as long lines with no head and a key or label running
      off the map.
- [ ] Turn arrows back on, spacing 4, colour red. Click another editor tab in
      the same group and come back, then import `batlow.cpt` (§9): both times
      the arrows, V component, spacing and colour are as you left them
      (#1027).
- [ ] Mid-latitude westerlies point east; the trade winds point west. Around a
      low in the Northern Hemisphere the arrows turn anticlockwise.
- [ ] On Equirectangular, with and without Reset view, no band of arrows runs
      along the top or bottom edge. This grid's first and last rows are the
      poles, where east and north are undefined, so no arrow starts there
      (#1045).
- [ ] **priority** Orthographic centred on each pole (Center lat ±90), and Polar
      stereographic for each hemisphere: arrows keep
      following the flow around the pole rather than all pointing one screen
      direction, and no fan of arrows radiates from the pole itself (#1045).
      Each arrowhead sits square on the end of its own shaft. Each arrow
      points the way the flow runs on the map, so a wrong bearing shows up
      here first. Arrows nearest the globe's edge are drawn whole, running
      past the edge rather than cut off (#1159).

**`B: samples/0.6.0/hrrr_wind.grib2`** (grid-relative, Lambert)

- [ ] Message 0's Vectors row reads "This file states its components along
      the grid's axes; they are rotated to true north."
- [ ] **priority** Draw arrows on Equirectangular and compare with the GFS
      arrows over the same states (same date and cycle). Directions agree to
      within the difference two models give, on both the west and east edges
      of the domain. HRRR components run along the Lambert grid, whose axes
      are turned from north by sin 38.5° × (lon + 97.5°): about 15° to 23°
      along the east and west edges, most at the northern corners. A missing
      rotation shows as arrows that agree in the middle and disagree at the
      edges.

**`B: samples/0.6.0/cmc_pair.grib`**

- [ ] u and v are the same field, so every arrow is at 45° to the grid. In the
      source view they all point the same way; on Equirectangular they fan
      out with longitude, since this polar grid's axes turn.
- [ ] The arrows draw, and the Vectors row reads "These messages aren't a u/v
      pair (WIND, WIND). The arrows treat them as the u and v components.",
      under the note about the grid's axes (#1049). Failure looks like
      arrows with no note, or a refusal instead of arrows.

**`B: samples/0.6.0/gfs_unnamed_pair.grib2`**

- [ ] The message table lists `Parameter 0/2/250` and `Parameter 0/2/251`:
      no table names either (eccodes calls both `unknown`). Open message 0,
      pick message #1 as V component and tick Arrows: the arrows draw, and
      the Vectors row shows no note about the pair, since nothing says what
      the two are (#1142). Failure looks like "These messages aren't a u/v pair".

**`B: samples/0.6.0/hrrr_frames.grib2`**

- [ ] Message 0 as u and message 1 as v is refused with a note that their
      component frame differs (u grid-relative, v earth-relative). Nothing is
      drawn.
- [ ] Message 0 as u and message 2 as v draws, with the not-a-u/v-pair note:
      both messages are temperature (#1049).
- [ ] The refusal from message 1 is gone as soon as message 2 is picked (#1051).
      Picking message 1 again brings it back. Unticking Arrows clears it. With
      Contours ticked, the contours drawing leaves the arrow refusal in place.
- [ ] The refusal sits on a line of its own below the render summary, not in the
      render summary line. With the refusal showing, change the colour map or
      the message: the summary line updates and the refusal stays until the
      pair changes. Failure looks like the summary text and the refusal
      replacing each other (#1051).
- [ ] **`B: samples/0.6.0/gfs_wind.grib2`**: Export PNG and cancel the save
      dialog. "PNG export cancelled." shows on the export line, below the
      render summary, and the summary is still there. Export again and save:
      "Exported PNG to <path>". Change a control so the map redraws: the
      export line empties, since it spoke of the previous image. Failure looks
      like the export result replacing the render summary, or still showing
      after the map changed (#1051).

**`B: samples/0.6.0/two_grids.grib2`**

- [ ] Message 0 with message 1 is refused as two different grids, not as a
      frame difference.

## 7. Zoom and pan (#245, #1114, #1147, #1228)

**`S: samples/gfs.grib2`**, Equirectangular, then Web Mercator

- [ ] Scrolling zooms around the pointer, and dragging pans. The image follows
      the gesture and sharpens when it ends.
- [ ] Pan across the date line: the map stays continuous, with no blank strip
      or jump.
- [ ] The +, − and Reset view buttons sit in the Bounds row (Equirectangular
      and Web Mercator only). After each gesture the bounds fields show the
      window; typing a window by hand still works.
- [ ] Zoom into Europe, then Export PNG: the image is the zoomed window, not
      the whole globe.
- [ ] A click without a drag probes the value under the pointer. Drag the map
      and let go off the image, then click the map: that click probes too.
- [ ] On Mollweide and Orthographic, scrolling and dragging leave the map
      where it is.
- [ ] Tick Flip Y axis (the map turns upside down, south at the top). Scroll
      near the top edge: the zoom stays on the place under the pointer. Drag
      down: the map moves down with the pointer. Failure looks like a zoom
      that slides toward the other edge, or a map moving against the drag
      (#1147).
- [ ] Untick Flip Y axis and Reset view. Scroll five notches quickly over
      eastern Asia, then Reset view and do the same over the Americas. While
      each zoom redraws, the stretched old image stays under the pointer.
      Then drag the whole map most of the way across: the stretched or
      moved image follows the pointer until the new one lands. Failure looks
      like a blank map, or the wrong side of the world, until the redraw
      (#1228).

**`S: samples/mrms.grib2`** (7000 × 3500, the slowest render in the samples)

- [ ] What a click reads while a new projection is still rendering cannot be
      checked by eye. The render runs on the extension host, so the click is
      answered only after the Mollweide image has landed, and that reply is
      dropped as stale (#1146). All you see is that a click made during the
      render changes nothing: the readout stays empty, or keeps an earlier
      click's reading. Click again once the map is up and it reads normally.
      Failure looks like the readout changing in answer to the click made
      during the render. The
      rule that a click reads the image on screen, not the one on its way, is
      held by `extension/src/test/suite/axes.test.ts` (`a GRIB map, after
      picking a projection, before its render arrives`), which holds the
      render back, and the drop of the stale reply by
      `extension/src/test/suite/probe-seq.test.ts` (#1114).
- [ ] On Equirectangular with Bounds on Auto, pick Mollweide and scroll over
      the map straight away, while the old image is still up. Bounds stays on
      Auto. Pick Equirectangular again: the whole map is drawn, not a zoomed
      window. Failure looks like Bounds turning to Manual (#1147).
- [ ] On Equirectangular, hold the pointer still over one place and scroll
      five notches as fast as you can, quicker than the map redraws, then
      Reset view and scroll five notches slowly, letting each redraw. Both
      end on about the same window, with that place under the pointer.
      Failure looks like the quick notches zooming in less than the slow
      ones, or ending on another place (#1228).

**`B: samples/0.6.0/gfs_wind.grib2`**, Equirectangular, coastlines, contours and arrows on

- [ ] **priority** Zoom into Europe and pan. When the gesture ends, the
      coastlines, contours and arrows all line up with the field.
- [ ] Reset view, so Bounds is on Auto. Hold the pointer still over one
      place, such as 10° E 50° N in central Europe, and scroll in five
      notches one at a time, letting each redraw. That place stays under the
      pointer the whole way, and the map does not drop down when Bounds
      switches to Manual and its fields appear. Failure looks like Europe
      sliding toward an edge of the map (#1228).

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, `air`

- [ ] Choose Equirectangular first (a slice panel opens on Source projection,
      and one carried over from §5 is on Mollweide; neither zooms). Zoom and
      pan then work the same on a slice panel, and Reset view returns to the
      whole globe.

## 8. A zonal average (#240, #1229)

**`S: samples/gfs.grib2`**

- [ ] Tick Zonal average: a line appears under the readout, above the map,
      near 300 K in the tropics and coldest over Antarctica. The caption ends
      `721 of 721 points`, one per row.
- [ ] Latitude runs along the bottom, south on the left, ticked at round
      values evenly spaced: every 15°, 30° or 45° depending on the panel's
      width, such as `−90, −60, −30, 0, 30, 60, 90` (#1229). Drag the panel narrower: fewer
      round values, never two labels touching. Failure looks like
      `−80.5, −71, −61.5, −52, −42.75`, the latitudes of evenly spaced rows.

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, `air`

- [ ] Tick Zonal average, then step `time`: the line follows the slice.

**`F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1`**

- [ ] The line spans all 64 rows (`64 of 64 points`) with no gap at the short
      polar rows, from about 248 K at the poles to 288 K at the equator.
      Whether only stored values were averaged cannot be seen;
      `crates/fieldglass/tests/zonal_mean.rs` checks the means against eccodes.
- [ ] Its latitudes are Gaussian, none of them round, but the ticks still
      read round values such as `−60, −30, 0, 30, 60`, with `0` under the
      middle of the line (#1229). Failure looks like `−87.8638, −71.1578,
      −57.2066`, the rows' own latitudes.

**`S: samples/hrrr.grib2`**

- [ ] The checkbox is hidden, and the "Not available for this grid" line
      under the caption opens to `Zonal average: this grid's rows aren't
      circles of latitude.` (#945, #1036).

## 9. Import a GMT color table (#236)

*Fieldglass: Import Color Table…*, with any render panel open.

**`F: crates/fieldglass-core/tests/fixtures/cpt/batlow.cpt`**, `vik.cpt`, `balance.cpt`

- [ ] Each appears in the Colormap menu under Imported, in the panel already
      open and in a new one. batlow runs dark blue to pink; vik and balance are
      diverging.
- [ ] Reload the window (*Developer: Reload Window*): the imports are still in
      the menu.

**`F: crates/fieldglass-core/tests/fixtures/cpt/batlowS.cpt`**

- [ ] Refused with "line 15: this is a categorical table … which is not
      supported". Nothing is added to the menu.

**`B: samples/0.6.0/hsv.cpt`**

- [ ] Refused with "line 1: colour model "HSV" is not supported; only RGB
      tables are".

## 10. NetCDF-4 and HDF5 files compressed with szip (#421, #816)

**`F: crates/fieldglass-netcdf/tests/fixtures/hdf5_szip.h5`**, `hdf5_szip_hand.h5`

A `.h5` file does not open in Fieldglass by double-click: right-click it, *Open
With…*, *Fieldglass Viewer*. The same goes for every `.h5` file below.

- [ ] Both open. Every 2-D dataset draws a source view (`i2_ppb16`,
      `i2_ppb10`, `f4_ppb18`, `f8_ppb8` and the rest) with no error. Values
      match h5py in CI (`tests/hdf5_szip.rs`); by eye there is only "draws".

#816 settled on naming no real product: AIRS and MODIS granules are HDF4, which
does not open yet (#248). There is no real-world szip file in the samples.

## 11. Every ECMWF GRIB1 parameter has a name (#547, #601, #633)

**`B: samples/0.6.0/grib1_names.grib1`** (also used in §12, §13 and §15)

- [ ] Message 0: `2 metre temperature` (table 128, as before).
- [ ] Message 1: `Significant height of combined wind waves and swell`, in m.
      This is ECMWF table 140, which 0.5.0 showed as a code.
- [ ] Message 2 (DWD's own table 129): `Parameter 78/129/167`. It must not
      read as a WMO or ECMWF temperature name.
- [ ] Message 3 (Météo-France with ECMWF as sub-centre): the wave height name
      again, read from ECMWF's table.

## 12. GRIB1 surface fields are labelled as the surface (#869)

**`B: samples/0.6.0/grib1_names.grib1`** (same file as §11)

- [ ] Message 0's Level Type reads `Ground or water surface`, not `Cloud base
      level`, and its Level cell is a dash, not a number.

The four level types now read in their table's unit (a height in centimetres,
eta levels and layers, the kPa top of a mixed-precision layer) have no file
here; the `values_read_in_the_unit_their_table_row_states` test in
`crates/fieldglass-grib1/src/reader.rs` covers them.

## 13. A code a table does not name shows as its number (#774)

**`B: samples/0.6.0/grib1_names.grib1`** (same file as §11)

- [ ] Message 4's level reads `Level type 250`. Message 5's reads `Missing`.

**`B: samples/0.6.0/grib2_codes.grib2`** (also used in §14)

- [ ] Message 1's parameter reads `Parameter 99/0/0`: discipline 99 is in no
      table. The panel does not show the discipline itself;
      `crates/fieldglass/tests/unknown_codes.rs` checks it reads `Discipline 99`.

**`F: crates/fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1`**

- [ ] The Center column shows the sub-centre as `Sub-centre 105`. In 0.5.0 it
      showed nothing.

The other wordings have no file here.
`crates/fieldglass/tests/unknown_codes.rs` covers `Production status 200`,
`Data type 99` and the sub-centre; `Fixed surface 242` is pinned in
`crates/fieldglass-napi/src/lib.rs`, and `unsupported(13)`, `Level type 250` and
`+5 time unit 200` in `fieldglass-grib1`'s unit tests (`src/gds.rs`,
`src/reader.rs`).

## 14. Code-table labels and dimensionless units (#653, #654, #873)

**`B: samples/0.6.0/grib2_codes.grib2`** (same file as §13)

- [ ] Message 0's parameter reads `Stochastically Perturbed Parametrization
      Tendency (SPPT)`. The panel does not show the discipline; the
      `fieldglass-grib2` table tests check that 191 reads `Computational
      parameters`.
- [ ] Its units are empty, not `Numeric`, in the message table, the panel
      title and the colour bar.

The Table 3.1 grid names (HEALPix, and the four templates that read `Unknown
grid template`) are not shown in the panel: the Grid column shows the decoder's
short name (`healpix`), as it did in 0.5.0.
`crates/fieldglass-grib2/tests/wmo_code_tables.rs` checks them, and holds every
other label to the WMO tables.

## 15. The metadata table (#773, #775)

**`B: samples/0.6.0/grib1_names.grib1`** (same file as §11), and
**`F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2`**

- [ ] The GRIB2 Center cell ends with its production status (for example
      `· Operational products`). The GRIB1 cells have none: GRIB1 carries no
      production status.
- [ ] No cell in either table shows a placeholder such as "Unknown",
      "Reserved" or "N/A". The only dash is in the GRIB1 Level column, for a
      level with no value (the surface, and message 5's `Missing`).

A GRIB2 message with no forecast or level type, which now leaves them out of the
Compare picker and the subtitle, has no file here;
`crates/fieldglass/tests/placeholders_are_null.rs` covers it.

## 16. Spectral fields drawn at map resolution (#637, #814)

**`B: samples/0.6.0/spectral_pair.grib2`** (T383, above the T359 the map shows)

- [ ] **priority** The subtitle reads `shown at T359 of T383`. The map is
      fine speckle at every scale, because the fixture is seeded white noise
      (flat spectrum, `NOTICE.md`), so smoothness cannot be judged. Failure
      looks like stripes, rings, blank patches or an empty map.
- [ ] **priority** Probe the cell at 0° N 0° E (the readout names the latitude
      and longitude). It reads `320.26 K shown · 308.37 K at full detail
      (T383)`. The two differ by about 12 K here, and by 5 to 18 K across
      the oracle's 12 points: the dropped T360 to T383 band is a flat-spectrum
      slice of seeded noise, not fine detail. The numbers
      are the oracle's, from `spectral_simple_t383.truncation.oracle.json`;
      any other pair, or no full-detail value, is the failure.
- [ ] Compare message 0 with message 1 (A − B): the difference is zero
      everywhere, and the Compare map keeps the `shown at T359 of T383` label.
- [ ] Export PNG: the image carries the label. Export CSV: the file starts
      with a `#` line naming T359.

`F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2` and every
sample file are at or below T359 and draw as before (§1). The transform is
checked in CI against pyshtools at T2500 and 80-bit arithmetic at T3000
(`fieldglass-core` `tests/sht_extended_range.rs`), and the T383 band limit in
`fieldglass-grib2`'s `tests/spectral_truncation.rs`.

## 17. GRIB2 CCSDS with Fieldglass's own decoder (#762)

**`S: samples/ecmwf.grib2`**

- [ ] Renders as in 0.5.0: global 2 m temperature, coastlines aligned.

**`F: crates/fieldglass-grib2/tests/fixtures/ccsds_flags46_12bit.grib2`**

- [ ] Opens and draws a global field with a maximum near 314.7. The pinned
      eccodes 2.34.1 fails on this file (`AEC_DATA_ERROR`); Fieldglass decodes
      it (ADR-0012). In CI (`tests/decode_ccsds.rs`) its values are checked
      against eccodes' decode of the source field it was repacked from, not
      against eccodes' decode of this file.

## 18. A NetCDF or Zarr panel offers only what the slice can do (#574, #822, #831, #1099)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`** (also used in §19, §20 and §22)

- [ ] Open `temperature`: the projection picker offers the map projections.
- [ ] Switch the variable to `lat_bnds`, which has no coordinates: the
      projection picker and Resampling are hidden, the caption says
      `source`, and the line under it reads `Not available for this grid:
      Projection, Resampling, Overlay, Zonal average, Long layout (Export
      CSV)`.
- [ ] Switch back to `temperature`: the picker returns with the map
      projections, and the line goes.
- [ ] On `temperature`, choose Equirectangular, then switch to `lat_bnds`
      and back to `temperature`: it is drawn in Equirectangular again, and the
      picker shows it (#1099). Failure looks like the map coming back in Source
      projection.

The same rule on a Zarr store is covered by
`extension/src/test/suite/slice-picker.test.ts`.

## 19. The projection reason says "not placed", not "not supported" (#960)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`** (same file as §18)

- [ ] On `lat_bnds`, open the "Not available for this grid" line under the
      caption. It lists Projection and Resampling with "this grid has no
      known position on the Earth." It does not say "not yet supported for source grids".

**`F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2`** (Dx = Dy = 0; also used in §20 and §39)

- [ ] The same reason, not "not yet supported for polar_stereo grids".

## 20. Overlays offered only where they can be drawn (#840)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`** (same file as §18)

- [ ] On `temperature`, turn on coastlines and contours. Switch to
      `lat_bnds`: the Overlay row, contours included, is hidden, and the line
      under the caption lists Overlay with "this grid has no known position on
      the Earth." No error is shown.
- [ ] Switch back to `temperature`: the Overlay row returns, and the
      coastlines and contours you had on come back.

**`F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2`** (same file as §19)

- [ ] The Overlay row is hidden, and the line lists Overlay with "this grid
      has no known position on the Earth." The file holds one message, so there is no
      Vectors row; arrows on an unplaced grid are checked on
      `tiny_earth_pair.grib2` in §39.

## 21. Contours, the long CSV and the zonal average gated by grid (#945, #979)

**`F: crates/fieldglass-netcdf/tests/fixtures/goes_geostationary.nc`** (`Rad`), **`F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc`** (`CMI`) and **`S: samples/goes.nc`**

The first two are small sectors with every pixel on the Earth; `goes.nc` is a
mesoscale sector. The same reasons apply to all three, and to a full disc.

- [ ] The field is placed: the projection picker and coastlines are offered,
      and coastlines draw.
- [ ] Contours and Zonal average are hidden. The line under the caption reads
      `Not available for this grid: Zonal average, Contours, Long layout
      (Export CSV)`. Opened, it gives two reasons, the grid's own first:
      `Zonal average: this grid's rows aren't circles of latitude.`, then
      `Contours, Long layout (Export CSV): not yet supported for
      satellite-view grids.` Tab reaches the summary, which shows a focus
      outline, and Enter opens it.
- [ ] Export CSV offers only the matrix layout, and its prompt says "The long
      layout isn't available for satellite-view grids yet."
- [ ] No reason claims that some of the grid's points have no position, and
      nothing shows an error.

A NetCDF panel has no Vectors row, so arrows are not checked here.

**`S: samples/hrrr.grib2`** (Lambert)

- [ ] Contours stay offered and draw. Only the zonal average is hidden
      (§8). The file holds one message, so it has no Vectors row; arrows on a
      Lambert grid are checked on `hrrr_wind.grib2` in §6.

## 22. A NetCDF or Zarr panel copes with its file changing or closing (#839)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`**, copied to a scratch folder first

- [ ] Open the copy, open a panel on `temperature`, then close the file's
      editor. Change the variable in the panel to `lat_bnds`. The panel says
      `NetCDF handle was disposed`, and the projection picker and overlays
      follow the slice you picked (hidden), not the last one drawn. No "Not
      available for this grid" line shows while the file is closed: it says
      nothing about the grid.
- [ ] Overwrite the copy with `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc`
      and open it again. Reopening alone does not touch the panel: make it
      draw again (*Developer: Reload Webviews*, or flip any control). The
      panel's variable list is the new file's (`PRODUCT/qa_value`,
      `PRODUCT/SUPPORT_DATA/surface_altitude`), and it says `lat_bnds is no
      longer in the file, so nothing was drawn. Pick a variable to draw.`
      rather than drawing something else under the old name. The Variable
      picker is blank, the heading reads `No variable drawn` with only the
      file's name under it, and the tab reads `pick a variable ·
      netcdf4_dimscale.nc` (or whatever the copy is called). *Export CSV…*
      says to pick a variable first. The Compare row is gone while nothing is
      drawn.
- [ ] Before picking, import a colour table (which rebuilds every open
      panel), then switch to another tab and back to the panel. Each time the
      panel stays on `No variable drawn` with the Variable picker blank, and
      draws nothing. Pick `PRODUCT/qa_value`: it draws under its own heading,
      the tab is renamed after it, and the Compare row is back.
- [ ] Close the file's editor, overwrite the copy with
      `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_hdf5_dummy.nc`,
      which has no variable to draw, and open it again. Make the panel draw
      again. It says `The NetCDF file no longer has a variable to draw.`, the
      picture and its colour bar are gone, the heading reads `No variable
      drawn` with only the file's name under it, and the tab reads `pick a
      variable · …`.

## 23. A GRIB panel finds its message after its file changes (#959)

Copy `B: samples/0.6.0/members_7_8.grib2` to a scratch file `members.grib2`.

- [ ] Open it. Both messages list a parameter, level and forecast time; in
      0.5.0 the ensemble template (4.1) left them blank.
- [ ] Open a render panel on message 1 (member 8) and turn on Compare with
      message 0 (member 7) as Field B. Close the file's editor, copy
      `samples/0.6.0/members_8_7.grib2` over `members.grib2` and open it
      again. Make the panel draw again (*Developer: Reload Webviews*, or
      import a colour table, which rebuilds every open panel). It draws
      member 8, now message 0, under its own heading, and Field B is still
      member 7, now message 1.
- [ ] Close the editor again, copy `samples/0.6.0/members_8_8.grib2` over it
      (member 8 twice), reopen and make the panel draw again. It draws
      nothing, its heading reads `No message drawn`, its tab reads
      `pick a message · members.grib2`, and it lists the file's messages to
      pick from. The Compare and Vectors rows are gone until a message is
      picked. Pick message 1: it draws member 8 under its own heading, and
      the tab is renamed `msg 1 … · members.grib2`. Field B (member 7) is not
      in this file, so Compare is off: Operation reads `(off)`, Field B is
      blank to pick, and the summary under the map names no operation (it
      reads `16×31 · range …`, not `A − B · …`).

Derived forecasts (a mean against a spread) and GRIB2 local parameters are told
apart the same way; `crates/fieldglass/tests/message_identity.rs` and
`extension/src/test/suite/grib-reload.test.ts` cover them. GRIB1 ensemble
members are not told apart yet, by design.

## 24. A render with no values says "no values" (#871)

**`B: samples/0.6.0/cmc_pair.grib`** (also used in §6)

- [ ] Equirectangular with the bounds set to 50° S to 30° S, 60° E to 90° E,
      which this northern grid never reaches. Nothing is drawn, and the colour
      bar keeps the field's own range: a window is a view of the field, not a
      field with no values (#1155). It never reads 0 to 1. With arrows on, no
      arrow key is shown. A field with no values at all shows "no values" and
      no colour bar; no sample has one, so `extension/src/test/suite/vectors.test.ts`
      and `crates/fieldglass/tests/colour_range_rules.rs` check it.

## 25. A variable in a nested NetCDF-4 group (#704)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc`**

- [ ] The variables read `PRODUCT/qa_value` and
      `PRODUCT/SUPPORT_DATA/surface_altitude`, with no leading `/`.
- [ ] The file page spells them the same way as the Render buttons: in the
      Variables table, in each variable's Dimensions column
      (`PRODUCT/scanline, PRODUCT/ground_pixel`) and in the Dimensions table.
      No name on the page starts with `/` (#1231).

## 26. Unnamed axes are named as NetCDF names them (#704)

**`F: crates/fieldglass-netcdf/tests/fixtures/hdf5_phony_dims.h5`**

- [ ] `a_8x8` lists its axes as `phony_dim_0` and `phony_dim_1`, the same
      names the Zarr store `v3_v2keys` got in §2.

## 27. A NetCDF slice reads only that slice (#939, #950, #942)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc`** (also used in §3 and §4)

- [ ] The file is 40 KB and `t2m` is 2.5 GB decoded whole. It opens at once,
      and step 7 draws in well under a second. 0.5.0 decoded the whole
      variable before drawing any step.

**`F: crates/fieldglass-netcdf/tests/fixtures/fill_only_at_field_cap.nc`** and **`fill_only_past_field_cap.nc`**

- [ ] The 8192 × 8192 file draws: every cell is the fill value, so the
      panel shows "no values" and no colour bar. At 64 Mi cells this can take
      a few seconds. The 10000 × 10000 file is refused before anything is
      read, with `Error: render failed: this read asks for 100000000 values
      at once, more than the 67108864 one field may hold. The file itself is
      fine`, `Error:` once and no colour bar (#1053).

How many bytes a slice reads, and the 2 ms and 5 ms step times, are measured in
CI by `crates/fieldglass-perf` against `docs/performance.md`; the browser
package's half is in §44.

## 28. Very large grids drawn at up to about 67 million pixels (#918)

**`S: samples/mrms.grib2`** (7000 × 3500)

- [ ] Mollweide renders without a long stall. Export PNG and check its size
      (`file export.png`): about 11,711 × 5,866: an 11,585 × 5,792 map
      (67.1 million pixels, 2:1) plus 126 px of margins and colour bar
      across and 74 px of title, file name and margins down. In 0.5.0 the
      map alone was 14,000 × 7,000, about 98 million pixels.

## 29. HDF5 files that begin with a userblock (#936)

**`F: crates/fieldglass-netcdf/tests/fixtures/hdf5_userblock_latest.h5`** and **`hdf5_no_userblock_latest.h5`**

- [ ] Both open, with the same variables (`c`, `v`, `x`, `y`), and `c` draws
      the same picture in each. The userblock file used to fail with "invalid
      magic bytes".

## 30. One unreadable variable no longer hides a NetCDF-4 file (#550)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc`**

- [ ] The file opens. `station_info` is listed under "Variables not decoded"
      with the HDF5 class that stopped it (`class 6 (compound)`). `visits` is
      listed there too, as `class 9 (variable-length)`. `time` and
      `temperature` are listed under Variables as usual.

## 31. More HDF5 and NetCDF-4 files open (#899, #900, #907, #914, #920, #922)

**`F: crates/fieldglass-netcdf/tests/fixtures/`** `hdf5_sizes_o4_l8.h5`, `hdf5_sizes_o8_l4.h5`, `hdf5_soft_links_earliest.h5`, `netcdf4_huge_attributes.nc`, `hdf5_huge_link_name.h5`, `hdf5_btree_k_sb1.h5`, `hdf5_btree_k_sb2.h5`

- [ ] Each opens without the "metadata could not be fully resolved" note and
      lists its variables. `hdf5_huge_link_name.h5` holds only groups, so it
      lists none. Values are checked against h5py in CI.
- [ ] In `netcdf4_huge_attributes.nc`, the global `history` cell shows its
      first 120 characters, a `…` and a ▶ arrow. Click it: the cell opens to
      all 5,600 characters, wrapped, ending `ncks -O`, and reads "Show less"
      above them. Click "Show less" and it closes to the one line again. Do it
      once more from the keyboard: Tab until the history cell has a focus
      ring, press Enter to open and Enter to close (#1232). The other global
      values (`value 0` …) are plain one-line text, as before.
- [ ] In the same file, `t`'s Attributes cell reads
      `attr_0=value 0; attr_1=value 1; attr_2=value 2; ▶ +8 more`. Open
      "+8 more": `attr_3` to `attr_9` each on a line, then `comment=` with its
      own 120-character preview, which opens to its whole value, about 5,000
      characters (#1232). Its bytes are checked in CI
      (`tests/hdf5_huge_objects.rs`).

The two `btree_k` files open, but their larger-than-default chunk B-tree is read
only when `v` is decoded, and `v` is 1-D, which the panel does not draw.
`tests/hdf5_btree_k.rs` covers #920.

## 32. Damaged or hostile files fail fast (#837, #847, #888, #895, #901, #908, #920, #921, #970, #975, #981, #986, #987, #989, #1000)

Open each file in **`F: crates/fieldglass-netcdf/tests/fixtures/`** below and
click through its variables. For every one: a message within a second or two,
no crash, no frozen window, and the extension host's memory (*Developer: Open
Process Explorer*) does not jump by hundreds of MB.

- [ ] `hdf5_chunk_past_ceiling.h5`: `zstd_f8_past_ceiling` and
      `deflate_f8_bomb` are refused with "…stored in chunks that decode to
      320.0 MiB each… The file itself is fine", and `zstd_f4_past_field_cap`
      with "this read asks for 268435456 values at once… The file itself is
      fine" (#970). `deflate_f8_unwritten` stores no chunk and, with no fill
      value set, draws as a flat 0.
- [ ] `hdf5_szip_hostile.h5`: both datasets refused (#975).
- [ ] `hdf5_codec_bombs.h5`: `deflate_bomb` and `zstd_bomb` are refused with
      "parse error: deflate (zstd) chunk decompresses past the chunk's
      3145728-byte length" (#981). `deflate_short_stream` and
      `zstd_short_stream` are each refused with "chunk decoded to 64 bytes,
      expected 268435456", with no jump in memory (#986). The matching bounds on Zarr's blosc, LZ4 and transpose
      paths are covered only by tests (`fieldglass-zarr`'s
      `tests/codec_bombs.rs`).
- [ ] `hdf5_shared_attribute_storage.h5`, `hdf5_shared_link_storage.h5`,
      `hdf5_shared_local_heap.h5` (#921, #987): each opens to the superblock
      view only, with no variables, and a note that the metadata could not be
      fully resolved because "two objects in this file share one dense
      attribute or link store" ("one local heap" for the third) and "The file
      itself is fine". `hdf5_shared_continuation.h5` gives the same view with
      "parse error: object-header chunk at … overlaps one already read", and
      no "file is fine".
- [ ] `hdf5_deep_group_chain.h5` (600 nested groups, a valid file): opens
      at once and lists `v` at the bottom of the chain (#921).
- [ ] `hdf5_fixed_array_huge_count.h5`: `v` is refused at once with "this
      read asks for 17179869184 values at once… The file itself is fine". The
      hostile index itself is reached only by a line read, covered by
      `crates/fieldglass/tests/whole_variable_budget.rs`.
- [ ] `hdf5_zstd_wide_window.h5`: `zstd_wide_window` is refused at once with
      "this variable's zstd chunks ask to keep 64.0 MiB of history … more
      than the 8.0 MiB this reader keeps for a chunk this size. The file
      itself is fine", with no jump in memory (#989). The Zarr half is
      covered by `fieldglass-zarr`'s `tests/codec_bombs.rs`.
- [ ] `numrecs_huge_cdf1.nc` and `numrecs_huge_cdf5.nc` (248 and 364 bytes,
      stating 60,000,000 records and holding 2): the file page shows the record
      dimension as `unlimited`, with no count, so the stated 60,000,000 is not
      shown anywhere. **Render v** is refused at once with `Error: render
      failed: parse error: variable "v": the region ends at byte 720000221,
      which exceeds source size 248` (CDF-5: byte 720000337 and size 364),
      with no jump in memory (#1000). The line says `Error:` once, and no
      colour bar is shown beside the empty map (#1053).

The chunk-index files (`hdf5_duplicate_chunk_records.h5`,
`hdf5_oversized_chunk.h5`, `hdf5_conflicting_chunk_records.h5`) each hold a 1-D
`v`, which the panel does not draw, and the index is read only on decode.
`tests/hdf5_chunk_records.rs` covers #837 and #888.

Memory and time limits are measured exactly in CI (`tests/hdf5_codec_bombs.rs`,
`tests/hdf5_szip_hostile.rs`, `tests/hdf5_shared_blocks.rs`,
`tests/hdf5_chunk_ceiling.rs`, `crates/fieldglass/tests/hdf5_listing_cap.rs`,
and `crates/fieldglass/tests/whole_variable_budget.rs` for #847). The HDF5
index and heap cases (#895, #901, #908) are covered by
`tests/hdf5_shared_snod.rs`, `tests/hdf5_local_heap.rs` and the unit tests in
`src/hdf5/heap.rs`. The GRIB cases in the CHANGELOG list have no openable
fixture; tests cover them: #631 (`malformed_inputs.rs`, GRIB1 and GRIB2), #802
(`decode_matrix.rs`, `decode_matrix_reshape.rs`), #848
(`decode_jpeg2000.rs`), #849 (`bifourier_hostile.rs`,
`decode_ecmwf_complex.rs`), #913 (`crates/fieldglass/tests/field_index_u64.rs`)
and #1001 (`allocations_ahead_of_data.rs`, GRIB1 and GRIB2), plus the fuzz
corpus.

## 33. HDF5 integers narrower than their storage (#795)

No manual check: the datasets in `hdf5_fixed_point_precision.h5` are 1-D, which
the panel does not draw. `crates/fieldglass-netcdf/tests/hdf5_fixed_point_precision.rs`
checks every value against h5py (-20 reads as -20, not 65516).

## 34. A classic file whose only record variable is bytes or shorts (#204)

**`F: crates/fieldglass-netcdf/tests/fixtures/record_single_short_cdf1.nc`**

- [ ] `s` (3 × 3) draws a source view with the colour bar running from 1 to
      9 and nine distinct cells stepping evenly from one corner to the other.
      Read with the padded stride, as 0.5.0 did, the second record reads 5,
      6, 7, and 4 and 8 never appear. CI checks the values
      (`tests/classic_record_layout.rs`).

## 34a. A streaming classic file shows its records (#1008)

**`F: crates/fieldglass-netcdf/tests/fixtures/numrecs_streaming_cdf1.nc`**, `numrecs_streaming_cdf5.nc`, `numrecs_streaming_partial_cdf1.nc`, `numrecs_streaming_partial_cdf5.nc`

These state their record count as unknown, as a file still being written does.

- [ ] In each streaming file, **Render v** draws two cells, and probing reads
      7 and -8. In 0.5.0, `v` opened with no records.
- [ ] In each partial file, `v` draws one cell, 7. The second record is cut
      short, so it is not counted.

## 35. A field stored column by column is the right way round (#542, #602, #792)

**`F: crates/fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1`** and **`F: crates/fieldglass-grib2/tests/fixtures/j_consecutive_latlon.grib2`**

The field is `10·j + i` on an 8 × 5 grid from 60° N 0° E, so every cell names
its own row and column.

- [ ] **priority** On Equirectangular, probe the four corner cells: 60° N 0° E
      reads 0, 60° N 35° E reads 7, 40° N 0° E reads 40, 40° N 35° E reads
      47. Both editions read the same. A transposed field gives other numbers
      and a 5 × 8 picture. A range check cannot see this; only positions can.

## 36. Reduced grids in second-order packing (#605, #611)

**`F: crates/fieldglass-grib1/tests/fixtures/`** `reduced_gg_second_order.grib1`, `reduced_gg_second_order_boust.grib1`, `reduced_gg_row_by_row.grib1`, `reduced_gg_row_by_row_boust.grib1`; **`F: crates/fieldglass-grib2/tests/fixtures/`** `second_order_reduced_gaussian.grib2`, `second_order_boust_reduced_gaussian.grib2`

Each pair is one field on an N32 reduced Gaussian grid, stored two ways: the
GRIB1 pairs a smooth synthetic field, the GRIB2 pair a pressure-level
temperature. Compare within a pair, not across them.

- [ ] **priority** The two files of each pair draw the same picture.
      A reversed row shows as a comb: every second row offset or mirrored
      against its neighbours. The row-by-row files used to be refused.

## 37. CSV on reduced grids (#244)

**`F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1`** (also used in §8)

- [ ] Export CSV, long layout: 6,114 rows of data
      (`grep -c '^[-0-9]' file.csv`), one per stored value. 0.5.0 wrote 8,192.
- [ ] Export CSV, matrix layout: still offered and exports.

## 38. A global grid's default window covers the seam

**`S: samples/gfs.grib2`** (also used in §1, §7 and §8)

- [ ] **priority** Equirectangular at the default window: Export PNG and look
      at the map's first and last columns at full size. Both are field, not a
      one-pixel stripe of background. (At panel size the image is scaled, and
      a one-pixel stripe can vanish.)

## 39. Grids that cannot be placed (#576, #603, #610, #843, #844, #1002, #1011)

**`F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2`** (same file as §19)

- [ ] The source view draws the values: the field is a constant 273.15 K, so
      it is one flat colour. Once drawn, the caption reads `source: polar_stereo
      16×31 → polar_stereo (no reprojection)`. Its `→` joins the source and
      the target, not a corner pair, and no latitude or longitude appears in
      it. The message table's Bounds column shows `—`, which is the check that
      the grid has no corner. 0.5.0 showed the first point twice (#1011).
- [ ] The Overlay row, contours included, is hidden and the long CSV is not
      offered; the line under the caption and the CSV prompt say the grid is
      not placed (§20). No message calls the grid type unsupported.

**A polar stereographic grid on an Earth smaller than one cell (#1002).** Build
it from the same file, written twice so a Vectors row appears:

```sh
python3 - crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2 samples/0.6.0/tiny_earth_pair.grib2 <<'PY'
import struct, sys
b = bytearray(open(sys.argv[1], 'rb').read())
at = 16
while b[at + 4] != 3:
    at += struct.unpack('>I', b[at:at + 4])[0]
b[at + 14] = 1                                   # Earth radius given in the file
b[at + 15] = 0
b[at + 16:at + 20] = struct.pack('>I', 80_000)   # R = 80 km
b[at + 55:at + 63] = struct.pack('>II', 100_000_000, 100_000_000)  # Dx = Dy = 100 km
open(sys.argv[2], 'wb').write(bytes(b) * 2)
PY
```

- [ ] Open `samples/0.6.0/tiny_earth_pair.grib2`. The projection picker,
      Resampling, the Overlay row, the Vectors row and Zonal average are
      hidden. The line under the caption reads `Not available for this grid:
      Projection, Resampling, Overlay, Vectors, Zonal average, Long layout
      (Export CSV)`, and opened, `Projection, Resampling, Overlay, Vectors,
      Zonal average, Long layout (Export CSV): this grid has no known position
      on the Earth.` Nothing mentions satellite-view grids, and no error is
      shown. 0.5.0 placed this grid and then refused contours "for
      satellite-view grids".

The other degenerate grids (a radius of zero, a scale factor of zero, a
latitude of true scale of 270°, non-finite parameters, a geostationary camera
inside the Earth, a rotation angle of 1e30°) are hand-built in tests:
`crates/fieldglass/tests/non_finite_geometry.rs` (with the #961 and #962
cases), `fieldglass-core`'s `tests/grid_geometry.rs`,
`crates/fieldglass/tests/placement.rs` (#1002, #1011), and
`crates/fieldglass-grib2/tests/grid_definition.rs` for the 1e30° rotation
(#842).

## 40. A geostationary grid in metres is placed (#966)

**`F: crates/fieldglass-netcdf/tests/fixtures/goes_geostationary.nc`** (also used in §21) and **`goes_geostationary_metres.nc`**

- [ ] **priority** On Equirectangular with coastlines on, both draw the same
      6 × 6 patch over the equator at 75° W, reaching about 7.8° either side,
      with the Pacific coast of Ecuador and Colombia near its west edge. Probe
      the same spot in each: the same value and the same `grid i,j`. In 0.5.0
      the metres file was drawn far off the Earth.

## 41. Repainting a slice reuses its placement (#662)

**`S: samples/rtofs_ice.nc`** (3298 × 4500 tripolar)

- [ ] The first draw takes a couple of seconds. After that, changing the
      colour map, the range or the variable to another one on the same grid
      repaints at once. `crates/fieldglass/tests/placement_is_memoised.rs`
      checks that a second placement reads nothing and derives no second
      geometry; nothing times it.

## 42. Error messages read the same in both hosts (#574)

No separate manual check: the wording is shared code, and the cross-host
conformance suite replays it on both bindings. §19, §20 and §39 see it in the
extension, and §44 sees the browser package's.

## 43. GRIB1 P1 re-save that changes nothing

No manual check: the in-table P1 editor is not enabled in released builds.
`crates/fieldglass/tests/grib1_p1_edit.rs` covers it.

## 44. The browser package (#460, #466, #808, #939, #950)

Install the tarball the release dry run built (RELEASING.md §2), not one built
locally, and decode in a real browser page. Run that dry run at the prep SHA
first: the npm API changed in #1201, so a tarball from an earlier run is the
wrong one.

```sh
mkdir -p /tmp/fieldglass-browser && cd /tmp/fieldglass-browser
gh run download <dry-run id> -n fieldglass-wasm-npm -D .
npm init -y > /dev/null
npm install --no-audit --no-fund ./fieldglass-wasm-X.Y.Z.tgz
REPO=<path to the fieldglass checkout>
cp "$REPO/crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2" \
   "$REPO/crates/fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc" \
   "$REPO/crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc" .
cat > index.html <<'EOF'
<!doctype html>
<meta charset="utf-8">
<title>fieldglass wasm check</title>
<pre id="out"></pre>
<canvas id="map" style="width: 320px; image-rendering: pixelated"></canvas>
<script type="module">
import init, { open } from './node_modules/@fieldglass/wasm/fieldglass_wasm.js';

const out = document.getElementById('out');
const say = (s) => { out.textContent += s + '\n'; };
const bytes = async (name) => new Uint8Array(await (await fetch(name)).arrayBuffer());
await init();

// GRIB2: one message, painted north up.
const grib = open(await bytes('regular_latlon_surface.grib2'));
const t = grib.decode(0, {});
const s = t.stats();
say(`grib2: ${grib.format()} ${t.ni()}x${t.nj()} ${t.parameter()} ${t.units()} valid=${s.validCount} ${s.min.toFixed(2)}..${s.max.toFixed(2)}`);
const canvas = document.getElementById('map');
canvas.width = t.ni();
canvas.height = t.nj();
const rgba = new Uint8ClampedArray(grib.render(t, {}, false).rgba);
canvas.getContext('2d').putImageData(new ImageData(rgba, t.ni(), t.nj()), 0, 0);
t.free();
grib.free();

// NetCDF: a slice placed by its 2-D coordinates.
const ice = open(await bytes('rtofs_tripolar_arctic.nc'));
const iv = ice.variables().find((v) => v.name === 'ice_coverage');
const f = ice.decodeSlice({ variable: iv.index, yDim: iv.detectedYDim ?? 1, xDim: iv.detectedXDim ?? 2, sliceIndices: new Uint32Array(iv.dims.length) }, {});
const g = f.grid();
say(`netcdf: ${ice.format()} ${f.ni()}x${f.nj()} kind=${g.kind} label=${g.label}`);
f.free();
ice.free();

// One time step of a 2.5 GB variable.
const big = open(await bytes('netcdf4_large_sparse.nc'));
const bv = big.variables().find((v) => v.name === 't2m');
for (const step of [7, 8, 119]) {
  const at = new Uint32Array(bv.dims.length);
  at[bv.detectedTimeDim ?? 0] = step;
  const t0 = performance.now();
  const p = big.decodeSlice({ variable: bv.index, yDim: bv.detectedYDim ?? 1, xDim: bv.detectedXDim ?? 2, sliceIndices: at }, {});
  const ps = p.stats();
  say(`t2m step ${step}: ${(performance.now() - t0).toFixed(0)} ms valid=${ps.validCount} ${ps.min}..${ps.max}`);
  p.free();
}
big.free();
say('done');
</script>
EOF
python3 -m http.server 8000
```

Open <http://localhost:8000/> in a current Chrome or Firefox.

- [ ] `grib2: grib2 16x31 Temperature K valid=496`, with the range between
      about 270 and 312. The canvas shows the same picture as the extension's
      source view of this file, north up.
- [ ] `netcdf: netcdf 260x200 kind=lookup label=curvilinear`. The extension's
      caption for this file says `curvilinear` too (#808).
- [ ] Step 7: valid 1,038,240, from -90 to 90. Step 8: valid 0, `null..null`.
      Step 119: from 10 to 190. Each step takes tens of milliseconds at most;
      before #939 the first one decoded the whole variable, which is past the
      2 GiB read limit and would be refused.
- [ ] `done` is printed and the browser console shows no error.

**The GPU shader the package ships.** Nothing in CI runs it on a GPU; the
crate's smoke page does. Point it at the same tarball, from the repo root:

```sh
rm -rf crates/fieldglass-wasm/pkg/web && mkdir -p crates/fieldglass-wasm/pkg/web
tar -xzf /tmp/fieldglass-browser/fieldglass-wasm-X.Y.Z.tgz --strip-components=1 -C crates/fieldglass-wasm/pkg/web
python3 -m http.server 8001
```

- [ ] Open <http://localhost:8001/crates/fieldglass-wasm/examples/smoke/>. It
      decodes `rap_jpeg2000_lambert.grib2` in a Web Worker and paints it with
      the shader and the CPU painter. The verdict reads `PASS — N pixels
      compared, M off by one lookup entry, 0 mismatched.`

## 45. One colour-range rule in both hosts (#1155, #1156, #1149)

**`F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2`**
(temperature, about 270 to 312 K)

- [ ] **Priority.** Equirectangular, colour range on Auto. Note the colour
      bar's two numbers. Set the bounds to a small box inside the grid, then
      switch Resampling to Bilinear: the numbers do not change. In 0.5.0 they
      followed the window and the resampling.
- [ ] Colour range Manual, min `300`, max cleared. The map draws, the colour
      bar runs from 300 to the field's maximum, and every cell below 300 is
      the lowest colour, not transparent.
- [ ] Min `300`, max `290`. The last picture stays, and under the fields, in
      the error colour, reads `minimum 300 is not below maximum 290; set a
      minimum below the maximum`. Both fields are outlined. In 0.5.0 the
      panel quietly drew the automatic range.
- [ ] Max cleared, min `400`. The note reads `minimum 400 is above the
      field's maximum 311.1 (taken from the data); set a maximum too`, the
      maximum written as the colour bar wrote it, not `311.0986328125`
      (#1053). Only the min field is outlined.
- [ ] Min `300`, max `290` again, then click Export PNG: nothing is saved,
      and the line under the toolbar says the export failed and points at the
      range fields. Open Compare and pick a Field B on another grid (needs a
      file with two grids): the grid error shows, and the note under the
      fields stays up. Change either field and the note and that line go.
- [ ] Min `280`. The map draws from 280 to the maximum, and the note and the
      outlines go.

**`B: samples/0.6.0/gfs_wind.grib2`**, message 0 (UGRD), Equirectangular
(#1149)

- [ ] **Priority.** Set Colour range to Manual, min `-30`, max `30`, and tick
      Contours: isolines of the wind's east-west speed lie over the map. Under
      Compare, pick `A − B` and Field B `#1 · VGRD …` (north-south speed). The
      map becomes the difference, and the isolines redraw to follow it: they
      bend where the colours change, and none are left over from the first
      picture. Pick another Field B, or set Compare back to off, and the
      isolines follow again each time. Failure looks like isolines that stay
      where they were while the colours underneath change.
      `extension/src/test/suite/contour-compare.test.ts` (`under a manual
      range, Compare on and a new Field B each ask for contours with the
      rider`) checks the requests, not the picture.

---

## 46. The line plot and the zonal average on a Compare map (#1208)

With Compare on, the probe and the contours read the combined map. In 0.5.0
the line plot and the zonal average still read Field A alone. Tick Line plot
for the checks that plot through a cell, and Zonal average for the zonal
ones; each replaces the other. The caption of each starts with the operation
(`A − B ·`) when it is combined.

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, `air`, axes `lon` (X) and `lat` (Y)

- [ ] Set `time` to 100 (2024-04-10), choose Compare `A − B` and set Field B's
      `time` to 10 (2024-01-11). Click a cell in northern mid-latitudes, say
      45° N 10° E, and plot along `lon`. The line is a row of the difference
      map: mostly above zero, from about −5 to +27 K (spring warmer than
      winter), and the caption starts with `A − B`. Failure looks like a line
      of temperatures near 270 to 290 K, which is Field A alone. At the
      clicked longitude the line meets the value the readout gives for that
      cell, about +7.1 K at 45° N 10° E.
- [ ] Plot along `lat` instead: the same, a column of the difference map.
- [ ] **Priority.** Plot along `time`. This runs along a held axis. Field A
      steps along the axis and Field B stays on the day it is drawn at, so the
      line is Field A on each day minus Field B on 2024-01-11. It is zero on
      2024-01-11, the day Field B is on (read the axis for that date), and meets
      the dashed rule (day 100) at the value the readout gives for the cell.
      Failure looks like absolute temperatures, a line that is zero all the
      way, or one that is not zero on 2024-01-11.
- [ ] Set Field B's `time` to 200 and click the cell again: the whole line has
      moved by one constant, and is now zero on 2024-07-19. Day 100 reads about
      −15.6 K. Step
      Field A to another day and click again: the line is the same, only the
      dashed rule has moved, because Field B stays where it is drawn.
- [ ] Zonal average, with Field A and Field B both on day 100: the line is flat
      at zero, one point per row. Set Field B's `time` to 10: it is no longer
      flat, and is above zero in the northern mid-latitudes. Set Compare back to
      off: the line returns to absolute temperatures, about 220 to 300 K.
- [ ] **Each replaces the other (#1234).** With the zonal average on screen,
      tick Line plot (Zonal average stays ticked until you click) and click a
      cell: the cell's line replaces the zonal average and Zonal average
      unticks. Tick Zonal average: the zonal average replaces the line
      and Line plot unticks. Click another editor tab in the same group and
      come back: Zonal average is still ticked and Line plot unticked, and a
      click only reads the value. Failure looks like both boxes ticked under
      one plot.

**`B: samples/0.6.0/gfs_wind.grib2`**, message 0 (UGRD)

- [ ] Tick Zonal average with Compare off and note the numbers in the caption.
      Choose Compare `A + B` with Field B `#0 · UGRD …`, the same message: the line has the
      same shape and its numbers are twice what you noted (the last digit may
      differ by one). Failure
      looks like unchanged numbers, which is Field A alone.
- [ ] Choose `A − B` with Field B `#0 · UGRD …`: a flat line at zero. Pick
      Field B `#1 · VGRD …`: the numbers differ from both the Compare-off ones
      and zero, the zonal mean of u minus v.

`crates/fieldglass/tests/decode_line.rs` (`a_combined_line_is_the_combined_map_along_the_axis`,
`a_combined_line_is_the_combine_of_the_slices_it_crosses`) and
`extension/src/test/suite/compare-readouts.test.ts` (the NetCDF line along each
axis and the zonal average, and the GRIB zonal average) compare every point
against A op B. The browser package has no Compare line.
`extension/src/test/suite/line-plot.test.ts` checks that each box unticks the
other when its plot replaces the other's, and that a rebuilt panel keeps it so.

## No manual check

`[0.6.0]` entries with no section above, and why:

- **GRIB2 fields capped at 64 Mi points; a whole-variable read capped at
  2 GiB.** Hostile-input limits. §27 sees only the slice cap; the rest are
  held by `malformed_inputs.rs::the_grid_cap_is_the_core_field_cap_to_the_point`
  (GRIB1 and GRIB2) and the two `whole_variable_budget.rs` files (in
  `crates/fieldglass` and `fieldglass-netcdf`).
- **Painting or probing a field of the wrong length (#845, #902), bi-Fourier
  past 65,535 coefficients in the browser (#889), lengths that wrap on 32-bit
  (#561), GRIB bitmaps of the wrong length (#804, with #201 and #202, which it
  is part of), NetCDF-4 shuffle on a
  partial element (#203).** No file a user would open shows them. The tests:
  #845 `fieldglass-core`'s `colormap.rs::paint_tolerates_slices_of_the_wrong_length`
  and `crates/fieldglass/src/render.rs::probe_pixel_on_a_short_slice_reports_no_value`;
  #902 `crates/fieldglass/tests/field_index_u64.rs`; #889
  `fieldglass-grib2`'s `spectral.rs::bifourier_laplacian_does_not_overflow_a_32_bit_usize`;
  #561 `fieldglass-core`'s `bytes.rs::checked_usize_refuses_to_wrap_a_value_a_32_bit_target_cannot_hold`;
  #804 `fieldglass-grib1`'s `tests/decode_bitmap.rs::a_bitmap_shorter_than_the_grid_is_an_error`
  and `fieldglass-grib2`'s `ds.rs::rejects_bitmap_length_mismatch`; #203
  `fieldglass-core`'s `shuffle.rs::a_ragged_tail_is_carried_across_and_the_rest_transposed`.
- **Spectral fields in the browser package (#580).** `Session` synthesises
  them as the extension does; `crates/fieldglass/tests/spectral_truncation.rs`
  and the wasm conformance suite (`crates/fieldglass-wasm/tests/node/conformance.mjs`)
  cover it, and §16 sees the same code in the extension.
- **Two NetCDF-4 files of the same size read as one (#681).** The committed
  equal-size pairs have the same structure, so the bug would not show by eye.
  `crates/fieldglass-netcdf/tests/hdf5_traversal_cache.rs` builds a pair that
  does.
- **Two grids that cannot be placed compared by what their files declare
  (#961, #962).** Needs two hand-built unplaceable grids;
  `crates/fieldglass/tests/non_finite_geometry.rs`
  (`two_declined_grids_combine_only_when_they_declare_the_same_grid`,
  `two_declined_geostationary_slices_combine_only_when_they_declare_the_same_grid`).
- **A level in the dimensionless unit `1` shows no stray `1` (#1115).** No
  sample has a level coordinate with units of `1` (a sigma level); the stepper,
  Animate label and subtitle can only be seen on a store built for it.
  `extension/src/test/suite/slice-coordinates.test.ts` builds one: `a sigma
  level with units "1" shows no stray 1 in the stepper, the Animate label or
  the subtitle`, and `CF's dimensionless unit "1" is left out, and only exactly
  that` holds the rule.
- **A click while a Flip Y axis redraw is pending (#1114), and a cross-section's
  axis labels turning over before the image (#1114).** A slice redraws too
  fast to click inside by eye; the slowest render in the samples (`mrms.grib2`,
  §7) still answers the click only after the new image has landed.
  `extension/src/test/suite/axes.test.ts` holds the render back and clicks
  (`after Flip Y axis, with an axis answer landing before the render`, `a
  restored section, before its turned-over render arrives`, `a GRIB map, after
  picking a projection, before its render arrives`).
- **A click or Export CSV while a variable, image axis or Compare redraw is
  pending (#1145).** Same window as #1114, too short to hit by hand.
  `extension/src/test/suite/drawn-slice.test.ts` holds the render back, then
  reads the axis labels, clicks and exports (`after a variable switch, before
  its render arrives`, `after a Y-axis switch, before its render arrives`,
  `with Compare on, after Field B and the operation change, before the render
  arrives`, `a GRIB Compare map, after the operation changes, before the
  render arrives`, `Export CSV during a variable switch saves the field on
  screen, named for it`).
- **A late probe, line or zonal reply for an older click or render (#1146).**
  The reply has to land after a newer render or in the wrong order, which a
  person cannot arrange by clicking. `extension/src/test/suite/probe-seq.test.ts`
  holds the reply back and delivers it late (`a reply held past a newer render
  fills neither the readout nor the line`, `two quick probes answered in
  reverse order show the second`).
- **Zarr windows planned and decoded in the browser package (#1083, #1163,
  #1124).** `planWindow`, `decodeWindow` and `coordinateChunks` are not in the
  npm package yet, so no page can call them. The tests are
  `crates/fieldglass/tests/zarr_window.rs` (for example
  `a_left_out_coordinate_is_named_with_why`,
  `a_coordinate_chunk_the_store_never_wrote_is_its_fill_value`) and
  `crates/fieldglass-wasm/tests/node/zarr.mjs`.
- **The overlay error line (#1051).** It shows only when the provider fails to
  project a coastline or graticule layer, which no sample does. No automated
  test checks that line's text either. The arrow and export lines are in §6.
- **The browser module refuses an array as an options object (#1210).** It
  is an npm-package behaviour with no editor to see it in.
  `crates/fieldglass-wasm/tests/node/reduced.mjs` passes `["f64"]`, a string and
  a number to `decode`, `decodeDisplay`, `warp` and `palette` and expects
  `invalid_option` (null and undefined stay the default), and
  `crates/fieldglass-wasm/tests/node/netcdf.mjs` does the same for
  `sliceIndices`, which takes an array, a typed array or an index-keyed object
  and refuses any other object.
- **Neither host takes binary data as an options object (#1239).** The
  extension never passes one, so there is nothing to see in the editor.
  `extension/src/test/suite/one-handle.test.ts` (`binary data is not an options
  object, as on the browser package (#1239)` and `binary data is not a target or
  a field, and a message index or slice still is (#1239)`) passes a typed array,
  a `Buffer`, a `DataView` and an `ArrayBuffer` to every object argument the
  addon reads and expects `invalid_option`, with `null` still the default.
  On the browser module, `crates/fieldglass-wasm/tests/node/reduced.mjs`,
  `netcdf.mjs`, `fetchplan.mjs` and `zarr.mjs` check the shared guard with a
  typed array, a `DataView` and an `ArrayBuffer` on `decode`, `decodeDisplay`,
  `warp`, `palette`, the full-field stats, the slice and line requests, `select`
  and `planWindow`. A `SharedArrayBuffer`, and buffers made in another realm
  with `vm.runInNewContext`, are refused the same way on both hosts (#1242):
  `a SharedArrayBuffer and another realm's buffers are binary data too, as on
  the browser package (#1242)` in `one-handle.test.ts`, and `reduced.mjs`.
- **A panel whose file changed draws the variable it was showing, even on a
  four-dimensional variable (#1202).** The fault is a message from the old page
  arriving just after the redraw, a window no click can hit.
  `extension/src/test/suite/render-panel-script.test.ts` (`a slice panel finds
  its variable by name when its page asks for axes first (#1202)` and `a ready
  from a page the panel has written over is not served (#1202)`) holds it; §22 and §23 see
  the same redraw by eye.
- **A Zarr store's units are written the way NetCDF's are (#1073).** The units
  are typeset in the panel's title line and probe readout. The sample stores
  carry plain `degK`, which typesets to itself, so by eye there is nothing to
  tell; the rule is held by
  `extension/src/test/suite/one-handle.test.ts` (`the panel's listing typesets
  each variable's units, for a NetCDF file and a Zarr store`).
- **File: Revert File reads the file again into the editor and its panels
  (#1217).** VS Code runs a custom editor's revert only when the document has
  unsaved edits, and a Fieldglass document has none (the P1 edit is switched
  off), so the command does nothing a user can see.
  `extension/src/test/suite/revert.test.ts` (`a GRIB editor's table is written
  from the reverted file, and the old table's rows open nothing`, `a NetCDF
  editor and its slice panel are written from the reverted file`) calls the
  provider's revert directly and holds both editors and the slice panel.
- **Rust API** (breaking changes and additions) and **Internal**: no user
  interface, except the error kinds they add, which reach a user as the
  messages §32 checks. Those that change what a user sees have tests: a dense
  attribute or link past 16 MiB skipped, and a dropped `DIMENSION_LIST` giving
  `phony_dim_N` (#921), in `fieldglass-netcdf`'s `src/hdf5.rs`;
  `crates/fieldglass/tests/contiguous_past_the_file.rs` (#997);
  `tests/hdf5_szip_hostile.rs` (#1016). The semver list is checked with
  `cargo semver-checks`, and each crate is tested from its packaged `.crate`
  in CI (RELEASING.md §3).

---

## Recording the outcome

Note pass/fail per section in the release PR or issue, and keep the record in
`.release-check/PROGRESS.md` so the next cycle can see what was actually
exercised. **Date that file and name the version it covers** — a stale progress
file reads as current and is the one trap this plan has actually sprung.

**What this plan cannot see, by construction:** the value-level cross-checks
against eccodes all run in CI. This plan is only for the things that need eyes —
whether a picture is in the right place and reads correctly.
