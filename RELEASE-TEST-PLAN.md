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
# oracle's. The MRMS -999 sentinel and eccodes' j-consecutive lookup are listed
# in the output as documented exceptions, not failures.
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

# GRIB2 temperature with discipline 191, then 99.
latlon = (g2 / "regular_latlon_surface.grib2").read_bytes()
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
| `S: samples/gfs.grib2` | 1 |
| `S: samples/mirs_swath.nc` | 1 |
| `S: samples/oisst.nc` | 1 |
| `B: samples/0.6.0/gfs_wind.grib2` | 1 |
| `B: samples/0.6.0/ncep_air_daily_2024.nc` | 1 |
| `B: samples/0.6.0/air_v3.zarr` | 1 |
| `B: samples/0.6.0/cmc_pair.grib`, `grib1_names.grib1` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` | 1 |
| `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2`, `bifourier_ellipse_ieee32.grib2` | 1 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf4_unsupported_type.nc` | 1 |
| `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` | 1 |
| `F: crates/fieldglass-netcdf/tests/fixtures/netcdf_numeric_calendar.nc` | 2 |

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
- [ ] **A real NetCDF-4 file reads as it did** (#1556, #1557, #1562
      regression). The heap and budget changes sit on the path every
      NetCDF-4 attribute is read through. Open `S: samples/oisst.nc` in 0.6.1
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

## 2. A numeric calendar is not dated (#1645)

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

## No manual check

`[Unreleased]` entries, or parts of them, with no check above, and why. None
yet.

## Recording the outcome

Note pass/fail per section in the release PR or issue, and keep the record in
`.release-check/PROGRESS.md` so the next cycle can see what was actually
exercised. **Date that file and name the version it covers** — a stale progress
file reads as current and is the one trap this plan has actually sprung.

**What this plan cannot see, by construction:** the value-level cross-checks
against eccodes all run in CI. This plan is only for the things that need eyes —
whether a picture is in the right place and reads correctly.
