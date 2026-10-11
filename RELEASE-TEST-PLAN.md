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
( cd extension && npm run compile )

# Full samples.
tools/fetch_samples.sh

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js
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
```

### Files by section

Each file is opened once; this is where its checks are.

| File | Sections |
|---|---|
| `S: samples/gfs.grib2` | 1, 7, 8, 38 |
| `S: samples/hrrr.grib2` | 8, 21 |
| `S: samples/mrms.grib2` | 28 |
| `S: samples/ecmwf.grib2` | 17 |
| `S: samples/goes.nc` | 21 |
| `S: samples/rtofs_ice.nc` | 41 |
| `B: samples/0.6.0/ncep_air_daily_2024.nc` | 2, 3, 4, 5 |
| `B: samples/0.6.0/ncep_air_levels_ltm.nc` | 3, 4, 5 |
| `B: samples/0.6.0/air_v2.zarr`, `air_v3.zarr` | 2 |
| `B: samples/0.6.0/gfs_wind.grib2`, `hrrr_wind.grib2` | 1, 6 |
| `B: samples/0.6.0/cmc_pair.grib` | 6, 24 |
| `B: samples/0.6.0/hrrr_frames.grib2`, `two_grids.grib2` | 6 |
| `B: samples/0.6.0/spectral_pair.grib2` | 16 |
| `B: samples/0.6.0/grib1_names.grib1` | 11, 12, 13, 15 |
| `B: samples/0.6.0/grib2_codes.grib2` | 13, 14 |
| `B: samples/0.6.0/members_*.grib2` | 23 |
| `B: samples/0.6.0/hsv.cpt`, `F: crates/fieldglass-core/tests/fixtures/cpt/*.cpt` | 9 |
| `F: crates/fieldglass-zarr/tests/fixtures/stores/*` | 2 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc` | 18, 19, 20, 22 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc` | 3, 4, 27 |
| `F: crates/fieldglass-netcdf/tests/fixtures/goes_geostationary*.nc`, `goes16_abi_cmip.nc` | 21, 40 |
| `F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2` | 19, 20, 39 |
| `F: crates/fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1` (and `.grib2`) | 35 |
| `F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_*.grib1`, `F: crates/fieldglass-grib2/tests/fixtures/*reduced_gaussian.grib2` | 8, 36, 37 |
| `F: crates/fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1` | 13 |
| `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` | 1, 15 |
| `F: crates/fieldglass-grib2/tests/fixtures/ccsds_flags46_12bit.grib2` | 17 |
| `F: extension/src/test/fixtures/healpix_n4_ring.grib2` | 1, 14 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc` | 22, 25 |
| `F: crates/fieldglass-netcdf/tests/fixtures/fill_only_*.nc` | 27 |
| HDF5 and NetCDF-4 fixtures under `crates/fieldglass-netcdf/tests/fixtures/` | 10, 26, 29, 30, 31, 32, 34 |

---

## 1. Regression floor

Carried forward each cycle. This is the minimum for a release whose diff is
small, and it is *not* a substitute for the per-feature sections below it.

- [ ] `S: samples/gfs.grib2` — renders, reprojects, coastlines align.
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

---

## 2. Open a Zarr store (#246, #657, #658, #659, #704)

Each store is opened with *Fieldglass: Open Zarr Store…* from the command
palette, picking the store's folder.

**`B: samples/0.6.0/air_v2.zarr`** (consolidated v2, written by xarray)

- [ ] A render panel opens on `air`, a global temperature map 144 × 73, with
      the poles cold and the tropics warm. Coastlines align on Equirectangular
      and Orthographic.
- [ ] The slice picker lists `time` with 366 steps. Stepping to another day
      changes the map.
- [ ] **priority** Probe one point, say 40° N 100° W, on 2024-01-15. Note the
      value.

**`B: samples/0.6.0/air_v3.zarr`** (unconsolidated v3) and
**`B: samples/0.6.0/ncep_air_daily_2024.nc`**

- [ ] **priority** The same day and point read the same value in both, and the
      same as the v2 store. The three images match pixel for pixel by eye,
      including where the date line falls. A store written by xarray is placed
      exactly as the NetCDF file it came from; a mismatch here is a placement
      bug, not a decode one (`crates/fieldglass/tests/zarr_netcdf_twins.rs`
      checks the committed pair).
- [ ] Copy `air_v2.zarr` to a folder named `air_copy` (no `.zarr` suffix) and
      open that: it opens the same way. The suffix is not what the check uses.

**`F: crates/fieldglass-zarr/tests/fixtures/stores/`**

- [ ] `cf_v3` (consolidated v3) and `v2_slash` (unconsolidated v2) each open
      and draw. `cf_v3`'s `t` reads in physical units (it is packed with
      `scale_factor` 0.25 and `add_offset` 200).
- [ ] `v3_v2keys` (unconsolidated v3, no dimension names): the slice picker
      names the axes `phony_dim_0` and `phony_dim_1`, as NetCDF names them.
- [ ] `v3_sharded`: `s` draws a 7 × 5 source view with no error. Whether the
      sharded values are right cannot be judged by eye; `fieldglass-zarr`'s
      `tests/real_stores.rs` and `tests/stores.rs` check them against
      zarr-python.
- [ ] `v2_problems` holds only 1-D arrays: opening it says the store holds
      nothing this build can draw. It does not open an empty panel or show a
      raw error.
- [ ] `crates/fieldglass-zarr/tests/fixtures` is a folder of stores, not a
      store: opening it says so, and nothing opens.

The five blosc compressors and the other codecs are checked value for value in
CI (`fieldglass-zarr` `tests/`); there is nothing more to see by eye.

## 3. Animate through a dimension (#170)

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, variable `air`

- [ ] The panel shows an Animate row with first, back, play/pause, forward and
      last buttons, a speed control and a Loop toggle. It starts on `time`.
- [ ] Press play: the date in the subtitle advances a day per frame and the
      Northern Hemisphere warms towards July.
- [ ] With the colour range on automatic, the colour bar's numbers do **not**
      change during playback: the range of the step playback started from is
      held, so frames stay comparable.
- [ ] Faster and slower speeds change the frame rate. With Loop on, the last
      day (2024-12-31) is followed by the first; with Loop off, playback stops
      on the last day.

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, variable `air`

- [ ] Choose `level` as the dimension to step through. Frames step through
      17 pressure levels, 1000 to 10 hPa. Over the tropics it is coldest near
      100 hPa and warmer again above.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc`**, `t2m`

- [ ] Play from step 0. Every step but 7 and 119 has no data and shows "no
      values" with no colour bar (#871). Step 7 shows bands of latitude; step
      119 the same bands 100 higher. Playback does not stall or queue frames.

## 4. Click a cell for a line plot (#172)

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, `air`

- [ ] Click a cell in Siberia (60° N 100° E). A line plot appears under the
      readout: a time series over 2024, cold in January (about 240 K) and
      warm in July (about 290 K). The x axis is labelled with dates, the y axis
      in the variable's units (`degK`).
- [ ] A dashed rule marks the day on screen. Step to another day and the rule
      moves.

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, `air`

- [ ] Choose `level` as the axis to plot along: a vertical profile, labelled
      in millibar, cooling with height to the tropopause.

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_large_sparse.nc`**, `t2m`

- [ ] Click any cell and plot along `time`: two points, at steps 7 and 119, and
      no line drawn between them. Gaps stay gaps.

## 5. Any two dimensions as axes (#171)

**`B: samples/0.6.0/ncep_air_daily_2024.nc`**, `air`

- [ ] Set the axes to `time` (X) and `lat` (Y). The panel says it is a
      cross-section, hides the projection picker and the map overlays, and
      draws a time–latitude plot: the seasonal swing is strongest at high
      latitudes. The time axis reads as dates, the latitude axis as degrees.
      Before this change the plane was placed on the Earth, reading time as
      longitude.
- [ ] Set the axes back to `lon` and `lat`: the map, the projection picker
      and the overlays return.

**`B: samples/0.6.0/ncep_air_levels_ltm.nc`**, `air`

- [ ] Axes `lat` (X) and `level` (Y): a latitude–height section labelled in
      degrees and millibar.

A depth axis with fill values used to fail to draw; no file here has one.
`crates/fieldglass/tests/cross_section_placement.rs` covers it.

## 6. Wind and current arrows (#241, #793, #805)

**`B: samples/0.6.0/gfs_wind.grib2`** (earth-relative)

- [ ] Open message 0 (UGRD). The Vectors row offers message 1 (VGRD) as v.
      Turn it on: arrows over the field, with a legend giving the speed of a
      full-length arrow. Larger spacing draws fewer arrows.
- [ ] Mid-latitude westerlies point east; the trade winds point west. Around a
      low in the Northern Hemisphere the arrows turn anticlockwise.
- [ ] **priority** Orthographic centred on each pole (Center lat ±90), and Polar
      stereographic for each hemisphere: arrows keep
      following the flow around the pole rather than all pointing one screen
      direction. Arrows are projected like a coastline, so a wrong bearing
      shows up here first.

**`B: samples/0.6.0/hrrr_wind.grib2`** (grid-relative, Lambert)

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

**`B: samples/0.6.0/hrrr_frames.grib2`**

- [ ] Message 0 as u and message 1 as v is refused with a note that their
      component frame differs (u grid-relative, v earth-relative). Nothing is
      drawn.
- [ ] Message 0 as u and message 2 as v draws.

**`B: samples/0.6.0/two_grids.grib2`**

- [ ] Message 0 with message 1 is refused as two different grids, not as a
      frame difference.

## 7. Zoom and pan (#245)

**`S: samples/gfs.grib2`**, Equirectangular, then Web Mercator

- [ ] Scrolling zooms around the pointer, and dragging pans. The image follows
      the gesture and sharpens when it ends.
- [ ] Pan across the date line: the map stays continuous, with no blank strip
      or jump.
- [ ] Zoom in, Zoom out and Reset view sit in the Bounds row. After each
      gesture the bounds fields show the window; typing a window by hand still
      works.
- [ ] A click without a drag probes the value under the pointer.
- [ ] On Mollweide and Orthographic, scrolling and dragging leave the map
      where it is.

## 8. A zonal average (#240)

**`S: samples/gfs.grib2`**

- [ ] Tick Zonal average: a line beneath the map, near 300 K in the tropics
      and coldest over Antarctica. It has a point for every row of latitude.
      Change message or slice and the line follows.

**`F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1`**

- [ ] The line spans all 64 rows with no gap at the short polar rows. Whether
      only stored values were averaged cannot be seen;
      `crates/fieldglass/tests/zonal_mean.rs` checks the means against eccodes.

**`S: samples/hrrr.grib2`**

- [ ] The checkbox is disabled, with the note that a zonal average needs rows
      that are circles of latitude (#945).

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

- [ ] Message 0's level reads `Ground or water surface`, not `Cloud base
      level`, and shows no level number.

The four level types now read in their table's unit (a height in centimetres,
eta levels and layers, the kPa top of a mixed-precision layer) have no file
here; `crates/fieldglass-grib1/tests/code_tables.rs` covers them.

## 13. A code a table does not name shows as its number (#774)

**`B: samples/0.6.0/grib1_names.grib1`** (same file as §11)

- [ ] Message 4's level reads `Level type 250`. Message 5's reads `Missing`.

**`B: samples/0.6.0/grib2_codes.grib2`** (also used in §14)

- [ ] Message 1's discipline reads `Discipline 99`, not "Unknown discipline".

**`F: crates/fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1`**

- [ ] The Center column shows the sub-centre as `Sub-centre 105`. In 0.5.0 it
      showed nothing.

The other wordings (`Production status 200`, `Fixed surface 242`,
`unsupported(13)` and the rest) have no file here;
`crates/fieldglass/tests/unknown_codes.rs` covers them.

## 14. Code-table labels and dimensionless units (#653, #654, #873)

**`B: samples/0.6.0/grib2_codes.grib2`** (same file as §13)

- [ ] Message 0 reads discipline `Computational parameters` and parameter
      `Stochastically Perturbed Parametrization Tendency (SPPT)`.
- [ ] Its units are empty, not `Numeric`, in the message table, the panel
      title and the colour bar.

**`F: extension/src/test/fixtures/healpix_n4_ring.grib2`** (also used in §1)

- [ ] The Grid column names the HEALPix grid, not `Unknown grid template`.

`crates/fieldglass-grib2/tests/wmo_code_tables.rs` holds every other label to the
WMO tables.

## 15. The metadata table (#773, #775)

**`B: samples/0.6.0/grib1_names.grib1`** (same file as §11), and
**`F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2`**

- [ ] The GRIB2 Center cell ends with its production status (for example
      `· Operational products`). The GRIB1 cells have none: GRIB1 carries no
      production status.
- [ ] No cell in either table shows a dash or a placeholder such as "Unknown".

A GRIB2 message with no forecast or level type, which now leaves them out of the
Compare picker and the subtitle, has no file here;
`crates/fieldglass/tests/placeholders_are_null.rs` covers it.

## 16. Spectral fields drawn at map resolution (#637, #814)

**`B: samples/0.6.0/spectral_pair.grib2`** (T383, above the T359 the map shows)

- [ ] **priority** The subtitle reads `shown at T359 of T383`. The field is
      smooth, with no speckle, stripes, rings or blank patches.
- [ ] **priority** A probe shows the map's value and, beside it, the field's
      full-detail value. The two are close: T359 drops only the finest detail
      of a T383 field.
- [ ] Compare message 0 with message 1 (A − B): the difference is zero
      everywhere, and the Compare map keeps the `shown at T359 of T383` label.
- [ ] Export PNG: the image carries the label. Export CSV: the file starts
      with a `#` line naming T359.

`F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2` and every
sample file are at or below T359 and draw as before (§1). The transform's
accuracy up to T8192 is checked in CI (`tests/spectral_truncation.rs`,
`fieldglass-core` `tests/sht_extended_range.rs`).

## 17. GRIB2 CCSDS with Fieldglass's own decoder (#762)

**`S: samples/ecmwf.grib2`**

- [ ] Renders as in 0.5.0: global 2 m temperature, coastlines aligned.

**`F: crates/fieldglass-grib2/tests/fixtures/ccsds_flags46_12bit.grib2`**

- [ ] Opens and draws a global field with a maximum near 314.7. The pinned
      eccodes 2.34.1 fails on this file (`AEC_DATA_ERROR`); Fieldglass decodes
      it (ADR-0012). In CI (`tests/decode_ccsds.rs`) its values are checked
      against eccodes' decode of the source field it was repacked from, not
      against eccodes' decode of this file.

## 18. A NetCDF or Zarr panel offers only what the slice can do (#574, #822, #831)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`** (also used in §19, §20 and §22)

- [ ] Open `temperature`: the projection picker offers the map projections.
- [ ] Switch the variable to `lat_bnds`, which has no coordinates: the picker
      offers only the source view, and the caption says `source`.
- [ ] Switch back to `temperature`: the map projections return.

The same rule on a Zarr store is covered by
`extension/src/test/suite/slice-picker.test.ts`.

## 19. The projection note says "not placed", not "not supported" (#960)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`** (same file as §18)

- [ ] On `lat_bnds`, the note beside the projection picker reads
      "Reprojection needs a grid placed on the Earth, and this one isn't." It
      does not say "isn't available for source grids yet".

**`F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2`** (Dx = Dy = 0; also used in §20 and §39)

- [ ] The same note, not "Reprojection isn't available for polar_stereo grids
      yet".

## 20. Overlays offered only where they can be drawn (#840)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`** (same file as §18)

- [ ] On `temperature`, turn on coastlines and contours. Switch to
      `lat_bnds`: the overlays and contours are disabled, with
      "Overlays need a grid placed on the Earth, and this one isn't." No
      error is shown.
- [ ] Switch back to `temperature`: the coastlines and contours you had on
      come back.

**`F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2`** (same file as §19)

- [ ] The overlays are disabled with "Overlays need a grid placed on the
      Earth, and this one isn't." The file holds one message, so there is no
      Vectors row; refusing arrows on an unplaced grid is covered by
      `extension/src/test/suite/overlay-placement.test.ts`.

## 21. Contours, the long CSV and the zonal average gated by grid (#945, #979)

**`F: crates/fieldglass-netcdf/tests/fixtures/goes_geostationary.nc`** (`Rad`), **`F: crates/fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc`** (`CMI`) and **`S: samples/goes.nc`**

The first two are small sectors with every pixel on the Earth; `goes.nc` is a
mesoscale sector. The same notes apply to all three, and to a full disc.

- [ ] The field is placed: coastlines are offered and draw.
- [ ] Contours are disabled with "Contours aren't available for
      satellite-view grids yet."
- [ ] The zonal average is disabled with "A zonal average needs rows that
      are circles of latitude, and this grid's rows aren't."
- [ ] Export CSV offers only the matrix layout, and its prompt says "The long
      layout isn't available for satellite-view grids yet."
- [ ] No note claims that some of the grid's points have no position, and
      nothing shows an error.

A NetCDF panel has no Vectors row, so arrows are not checked here.

**`S: samples/hrrr.grib2`** (Lambert)

- [ ] Contours stay offered and draw. Only the zonal average is disabled
      (§8). The file holds one message, so it has no Vectors row; arrows on a
      Lambert grid are checked on `hrrr_wind.grib2` in §6.

## 22. A NetCDF or Zarr panel copes with its file changing or closing (#839)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc`**, copied to a scratch folder first

- [ ] Open the copy, open a panel on `temperature`, then close the file's
      editor. Change the variable in the panel to `lat_bnds`: the projection
      picker and overlays follow the slice you picked (source only), not the
      last one drawn.
- [ ] Overwrite the copy with `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc`
      and open it again. The panel's variable list is the new file's, and the
      panel says the variable it was showing is gone rather than drawing
      something else under the old name.

## 23. A GRIB panel finds its message after its file changes (#959)

Copy `B: samples/0.6.0/members_7_8.grib2` to a scratch file `members.grib2`.

- [ ] Open it. Both messages list a parameter, level and forecast time; in
      0.5.0 the ensemble template (4.1) left them blank.
- [ ] Open a render panel on message 1 (member 8). Close the file's editor,
      copy `members_8_7.grib2` over `members.grib2` and open it again. Make the
      panel draw again (*Developer: Reload Webviews*, or import a colour
      table, which rebuilds every open panel). It draws member 8, now message 0,
      under its own heading. Before the change, set Compare with message 0
      (member 7) as Field B; after it, Field B is still member 7, now
      message 1.
- [ ] Close the editor again, copy `members_8_8.grib2` over it (member 8 twice)
      and reopen. The panel draws nothing, its heading reads `No message
      drawn`, and it lists the file's messages to pick from.

Derived forecasts (a mean against a spread) and GRIB2 local parameters are told
apart the same way; `crates/fieldglass/tests/message_identity.rs` and
`extension/src/test/suite/grib-reload.test.ts` cover them. GRIB1 ensemble
members are not told apart yet, by design.

## 24. A render with no values says "no values" (#871)

**`B: samples/0.6.0/cmc_pair.grib`** (also used in §6)

- [ ] Equirectangular with the bounds set to 50° S to 30° S, 60° E to 90° E,
      which this northern grid never reaches. The panel shows "no values" and
      no colour bar, not a 0 to 1 range. With arrows on, no arrow key is
      shown.

## 25. A variable in a nested NetCDF-4 group (#704)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc`**

- [ ] The variables read `PRODUCT/qa_value` and
      `PRODUCT/SUPPORT_DATA/surface_altitude`, with no leading `/`.

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

- [ ] The 8192 × 8192 file draws one flat value; at 64 Mi cells this can
      take a few seconds. The 10000 × 10000 file is refused with a message
      giving the size and saying the file itself is fine, before anything is
      read.

How many bytes a slice reads, and the 2 ms and 5 ms step times, are measured in
CI by `crates/fieldglass-perf` against `docs/performance.md`; the browser
package's half is in §44.

## 28. Very large grids drawn at up to about 67 million pixels (#918)

**`S: samples/mrms.grib2`** (7000 × 3500)

- [ ] Mollweide renders without a long stall. Export PNG and check its size
      (`file export.png`): width × height is at most 67,108,864, and the shape
      is the Mollweide 2:1. In 0.5.0 this asked for about 98 million pixels.

## 29. HDF5 files that begin with a userblock (#936)

**`F: crates/fieldglass-netcdf/tests/fixtures/hdf5_userblock_latest.h5`** and **`hdf5_no_userblock_latest.h5`**

- [ ] Both open, with the same variables (`c`, `v`, `x`, `y`), and `c` draws
      the same picture in each. The userblock file used to fail with "invalid
      magic bytes".

## 30. One unreadable variable no longer hides a NetCDF-4 file (#550)

**`F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc`**

- [ ] The file opens. `station_info` is listed under "Variables not decoded"
      with the HDF5 class that stopped it (compound); `visits`, `time` and
      `temperature` are listed as usual.

## 31. More HDF5 and NetCDF-4 files open (#899, #900, #907, #914, #920, #922)

**`F: crates/fieldglass-netcdf/tests/fixtures/`** `hdf5_sizes_o4_l8.h5`, `hdf5_sizes_o8_l4.h5`, `hdf5_soft_links_earliest.h5`, `netcdf4_huge_attributes.nc`, `hdf5_huge_link_name.h5`, `hdf5_btree_k_sb1.h5`, `hdf5_btree_k_sb2.h5`

- [ ] Each opens and lists its variables. `netcdf4_huge_attributes.nc` shows
      its long attributes in full. The two `btree_k` files are written with
      larger B-tree nodes than the default and must still open (#920). Values
      are checked against h5py in CI.

## 32. Damaged or hostile files fail fast (#837, #847, #888, #895, #901, #908, #920, #921, #970, #975, #981, #986, #987, #989)

Open each file in **`F: crates/fieldglass-netcdf/tests/fixtures/`** below and
click through its variables. For every one: a message within a second or two,
no crash, no frozen window, and the extension host's memory (*Developer: Open
Process Explorer*) does not jump by hundreds of MB.

- [ ] `hdf5_chunk_past_ceiling.h5`: `zstd_f4_past_field_cap` and
      `zstd_f8_past_ceiling` are refused as unsupported, with a message saying
      the file itself is fine (#970). `deflate_f8_bomb` is refused.
      `deflate_f8_unwritten` stores no chunk and reads as its fill value.
- [ ] `hdf5_szip_hostile.h5`: both datasets refused (#975).
- [ ] `hdf5_codec_bombs.h5`: both bombs refused as a corrupt chunk (#981).
      `deflate_short_stream` and `zstd_short_stream` are each refused as a
      chunk that decoded to 64 bytes, expected 268,435,456, with no jump in
      memory (#986). The matching bounds on Zarr's blosc, LZ4 and transpose
      paths are covered only by tests (`fieldglass-zarr`'s
      `tests/codec_bombs.rs`).
- [ ] `hdf5_shared_attribute_storage.h5`, `hdf5_shared_link_storage.h5`,
      `hdf5_shared_local_heap.h5`: refused rather than read (#921).
      *Confirm after #991 merges:* the message says the file is fine and
      uses a layout Fieldglass does not support, rather than calling it
      corrupt (#987). `hdf5_shared_continuation.h5` stays a corrupt-file
      error.
- [ ] `hdf5_deep_group_chain.h5` (600 nested groups, a valid file): opens
      at once and lists `v` at the bottom of the chain (#921).
- [ ] `hdf5_duplicate_chunk_records.h5` (one chunk named 16 times) and
      `hdf5_oversized_chunk.h5` (a legal 16 Mi-element chunk) open at once
      (#837). `hdf5_conflicting_chunk_records.h5` is refused (#888).
- [ ] `hdf5_fixed_array_huge_count.h5`: refused (#939 review).
- [ ] *Confirm after the #989 PR merges:* its windowed zstd fixture is refused
      before anything large is reserved, in NetCDF-4 and Zarr.

Memory and time limits are measured exactly in CI (`tests/hdf5_codec_bombs.rs`,
`tests/hdf5_szip_hostile.rs`, `tests/hdf5_shared_blocks.rs`,
`tests/hdf5_chunk_ceiling.rs`, `crates/fieldglass/tests/hdf5_listing_cap.rs`,
and `crates/fieldglass/tests/whole_variable_budget.rs` for #847).
The GRIB cases in the CHANGELOG list (#631, #802, #848, #849, #913) have no openable
fixture beyond those tests and the fuzz corpus.

## 33. HDF5 integers narrower than their storage (#795)

No manual check: the datasets in `hdf5_fixed_point_precision.h5` are 1-D, which
the panel does not draw. `crates/fieldglass-netcdf/tests/hdf5_fixed_point_precision.rs`
checks every value against h5py (-20 reads as -20, not 65516).

## 34. A classic file whose only record variable is bytes or shorts (#204)

**`F: crates/fieldglass-netcdf/tests/fixtures/record_single_short_cdf1.nc`**

- [ ] `s` (3 × 3) draws a source view with no error. The values are checked
      in CI (`tests/classic_record_layout.rs`); 0.5.0 read the wrong bytes.

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

- [ ] **priority** Equirectangular at the default window: the left and right
      edges of the image are field, not a one-pixel stripe of background.

## 39. Grids that cannot be placed (#576, #603, #610, #843, #844)

**`F: crates/fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2`** (same file as §19)

- [ ] The source view draws the values. The grid reports its real 16 × 31,
      not 0 by 0.
- [ ] Contours and the long CSV are not offered, and the note says the grid
      is not placed (§20). No message calls the grid type unsupported.

The other degenerate grids (a radius of zero, a scale factor of zero, a
latitude of true scale of 270°, non-finite parameters, an Earth smaller than a
cell, a geostationary camera inside the Earth, a rotation angle of 1e30°) are
hand-built in tests: `crates/fieldglass/tests/non_finite_geometry.rs`,
`fieldglass-core`'s `tests/grid_geometry.rs`, and the #842 and #961/#962 tests
in `crates/fieldglass/tests/declared_grid_family.rs`.

## 40. A geostationary grid in metres is placed (#966)

**`F: crates/fieldglass-netcdf/tests/fixtures/goes_geostationary.nc`** (also used in §21) and **`goes_geostationary_metres.nc`**

- [ ] **priority** On Equirectangular both draw the same 6 × 6 patch in the
      same place, between about 6.5° S and 6.5° N. Probe one pixel in each:
      the same latitude and longitude. In 0.5.0 the metres file was drawn far
      off the Earth.

## 41. Repainting a slice reuses its placement (#662)

**`S: samples/rtofs_ice.nc`** (3298 × 4500 tripolar)

- [ ] The first draw takes a couple of seconds. After that, changing the
      colour map, the range or the variable to another one on the same grid
      repaints at once. Timings are held in CI by
      `crates/fieldglass/tests/placement_is_memoised.rs`.

## 42. Error messages read the same in both hosts (#574)

No separate manual check: the wording is shared code, and the cross-host
conformance suite replays it on both bindings. §19, §20 and §39 see it in the
extension, and §44 sees the browser package's.

## 43. GRIB1 P1 re-save that changes nothing

No manual check: the in-table P1 editor is not enabled in released builds.
`crates/fieldglass/tests/grib1_p1_edit.rs` covers it.

## 44. The browser package (#460, #466, #580, #808, #950)

Install the tarball the release dry run built (RELEASING.md §2), not one built
locally, and decode in a real browser page.

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
const rgba = new Uint8ClampedArray(grib.render(t, {}, false));
canvas.getContext('2d').putImageData(new ImageData(rgba, t.ni(), t.nj()), 0, 0);
t.free();
grib.free();

// NetCDF: a slice placed by its 2-D coordinates.
const ice = open(await bytes('rtofs_tripolar_arctic.nc'));
const iv = ice.variables().find((v) => v.name === 'ice_coverage');
const f = ice.decodeSlice(iv.index, iv.detectedYDim ?? 1, iv.detectedXDim ?? 2, new Uint32Array(iv.dims.length), {});
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
  const p = big.decodeSlice(bv.index, bv.detectedYDim ?? 1, bv.detectedXDim ?? 2, at, {});
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

---

## No manual check

`[Unreleased]` entries with no section above, and why:

- **GRIB2 fields capped at 64 Mi points; a whole-variable read capped at
  2 GiB.** Hostile-input limits; §27 and §32 see the refusals that reach a user.
- **Painting or probing a field of the wrong length (#845, #902), bi-Fourier
  past 65,535 coefficients in the browser (#889), lengths that wrap on 32-bit
  (#561), GRIB bitmaps of the wrong length (#804), NetCDF-4 shuffle on a
  partial element (#203).** No file a user would open shows them; the unit and
  wasm32 tests in CI do.
- **Two NetCDF-4 files of the same size read as one (#681).** The committed
  equal-size pairs have the same structure, so the bug would not show by eye.
  `crates/fieldglass-netcdf/tests/hdf5_traversal_cache.rs` builds a pair that
  does.
- **Two grids that cannot be placed compared by what their files declare
  (#961, #962).** Needs two hand-built unplaceable grids;
  `crates/fieldglass/tests/declared_grid_family.rs`.
- **Rust API** (breaking changes and additions) and **Internal**: no user
  interface. The semver list is checked with `cargo semver-checks`, and each
  crate is tested from its packaged `.crate` in CI (RELEASING.md §3).

---

## Recording the outcome

Note pass/fail per section in the release PR or issue, and keep the record in
`.release-check/PROGRESS.md` so the next cycle can see what was actually
exercised. **Date that file and name the version it covers** — a stale progress
file reads as current and is the one trap this plan has actually sprung.

**What this plan cannot see, by construction:** the value-level cross-checks
against eccodes all run in CI. This plan is only for the things that need eyes —
whether a picture is in the right place and reads correctly.
