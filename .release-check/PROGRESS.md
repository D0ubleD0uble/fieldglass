# 0.6.0 manual pass — outcome

**Version: 0.6.0. Pass 1 of 2: stopped 2026-10-16, on master `41b8ed39`,
before prep.** If the version above is not the one you are cutting, this file is
history, not status.

Pass 1 was an early run, so its findings could be fixed in this cycle. It ended
after §7 check 6 so the 0.6.0 issues it filed could be fixed. **Pass 2** is the
release pass that #855 requires. It runs the whole plan from §0 after those
fixes merge, and its result table goes above Pass 1's. Pass 1's findings and the
"Retest after fixes" table stay below as its input.

Env (pass 1): native module (release, x86_64-unknown-linux-gnu) and TypeScript
built from `41b8ed39`; `samples/0.6.0/` built per §0 on 2026-10-12, plus
`tiny_earth_pair.grib2`. Headless preflight: every `samples/` file decodes and
renders. In `samples/0.6.0/`, only `tiny_earth_pair.grib2` fails to render,
which is the refusal §39 expects.

## Between the passes

- **Each fix PR updates `RELEASE-TEST-PLAN.md`** so the section it touches
  expects the fixed behaviour (as #1034 did for #1027/#1028), and ticks nothing
  here. Pass 2 then tests the fix in its normal section. The retest table below
  lists what pass 2 must see. A row whose fix changed the plan is marked "in
  plan".
- **New 0.6.0 work** (issues pulled in or drafted from the gap review) adds or
  extends a plan section in the same PR, per the plan's "How to add" rules.
- **Pass 1 stopped at:** §7 check 7 (`gfs_wind.grib2` overlays after zoom,
  priority). §7.7 to §44 have not been run yet. Their first run is pass 2.
- **Issues from pass 1.** 0.6.0: #1035–#1043, #1045, #1046, #1049, #1051.
  0.7.0: #1044, #1047, #1048 (needs-decision), #1050, #1079, #1080, and #249
  moved there.
- **Elsewhere:** #994 was being fixed by a separate /next-issue worker. The
  ux-agent's #1036 comment waits for the owner's approval.
- **Pass 2 setup:** run §0 again at the SHA under test: rebuild the addon,
  copy it and `index.d.ts` to `extension/bin/`, compile the TypeScript and
  re-run the headless preflight. `samples/0.6.0/` can be reused unless a fix
  changes a file it is built from. `samples/0.6.0/air_copy` (for §2.4) is
  git-ignored. Launch with
  `code --new-window --extensionDevelopmentPath=$PWD/extension $PWD <files>`.
  Opening a file from the CLI into a running dev host reloads it, so close the
  window first.

## Pass 1 result

| Section | Result | Notes / issues |
|---|---|---|
| §0 Setup (once) | done | |
| §1 Regression floor | pass (10/10) | F1 (#1035), F2 (#1036), F3 (#1037), F4 |
| §2 Open a Zarr store (#246, #657, #658, #659, #704) | pass (2.1–2.5f) | F5 (#1039), F6 (#1038), F7 (#1040) |
| §3 Animate through a dimension (#170) | pass | F8 (#1041), F9 (#1042) |
| §4 Click a cell for a line plot (#172) | pass | F10 (#1043; parts added to #1037, #1041) |
| §5 Any two dimensions as axes (#171) | pass with findings (5.1 labels, 5.5 orientation) | F11 (#1043 widened), F12 (#1044, 0.7.0) |
| §6 Wind and current arrows (#241, #793, #805) | pass with findings | F13 (#1045), F14 (#1046), F15 (#1049; #1047 and #1048 are 0.7.0), F16 (#1050, 0.7.0), F17 (#1051) |
| §7 Zoom and pan (#245) | 1–6 pass (1–3 on both projections); 7–8 not run | Bounds question answered (see Note §7), F18 (#1079, 0.7.0), F19 (#1080, 0.7.0) |
| §8 A zonal average (#240) | | |
| §9 Import a GMT color table (#236) | | |
| §10 NetCDF-4 and HDF5 files compressed with szip (#421, #816) | | |
| §11 Every ECMWF GRIB1 parameter has a name (#547, #601, #633) | | |
| §12 GRIB1 surface fields are labelled as the surface (#869) | | |
| §13 A code a table does not name shows as its number (#774) | | |
| §14 Code-table labels and dimensionless units (#653, #654, #873) | | |
| §15 The metadata table (#773, #775) | | |
| §16 Spectral fields drawn at map resolution (#637, #814) | | |
| §17 GRIB2 CCSDS with Fieldglass's own decoder (#762) | | |
| §18 A NetCDF or Zarr panel offers only what the slice can do (#574, #822, #831) | | |
| §19 The projection note says "not placed", not "not supported" (#960) | | |
| §20 Overlays offered only where they can be drawn (#840) | | |
| §21 Contours, the long CSV and the zonal average gated by grid (#945, #979) | | |
| §22 A NetCDF or Zarr panel copes with its file changing or closing (#839) | | |
| §23 A GRIB panel finds its message after its file changes (#959) | | |
| §24 A render with no values says "no values" (#871) | | |
| §25 A variable in a nested NetCDF-4 group (#704) | | |
| §26 Unnamed axes are named as NetCDF names them (#704) | | |
| §27 A NetCDF slice reads only that slice (#939, #950, #942) | | |
| §28 Very large grids drawn at up to about 67 million pixels (#918) | | |
| §29 HDF5 files that begin with a userblock (#936) | | |
| §30 One unreadable variable no longer hides a NetCDF-4 file (#550) | | |
| §31 More HDF5 and NetCDF-4 files open (#899, #900, #907, #914, #920, #922) | | |
| §32 Damaged or hostile files fail fast (#837, #847, #888, #895, #901, #908, #920, #921, #970, #975, #981, #986, #987, #989, #1000) | | |
| §33 HDF5 integers narrower than their storage (#795) | | |
| §34 A classic file whose only record variable is bytes or shorts (#204) | | |
| §34a A streaming classic file shows its records (#1008) | | |
| §35 A field stored column by column is the right way round (#542, #602, #792) | | |
| §36 Reduced grids in second-order packing (#605, #611) | | |
| §37 CSV on reduced grids (#244) | | |
| §38 A global grid's default window covers the seam | | |
| §39 Grids that cannot be placed (#576, #603, #610, #843, #844, #1002, #1011) | | |
| §40 A geostationary grid in metres is placed (#966) | | |
| §41 Repainting a slice reuses its placement (#662) | | |
| §42 Error messages read the same in both hosts (#574) | | |
| §43 GRIB1 P1 re-save that changes nothing | | |
| §44 The browser package (#460, #466, #808, #939, #950) | | |

## Findings

- **F1** (§1, GRIB render panel): an empty "Message" dropdown shows on every GRIB render panel.
  `.toolbar-row { display: flex }` beats the `hidden` attribute, so `#message-pick-row` (#959) is
  never hidden. `#animate-row` has the same shape. Filed as #1035 (0.6.0).

- **F2** (§1, critique): features a grid can't support (zonal average, contours, arrows, overlays, long CSV, reprojection) are greyed out with a note. The owner prefers hiding them. Filed as #1036; ux-agent asked to research where the explanation goes.
- **F3** (§1, critique): clicking a NetCDF map always opens the line plot ("Plot along Channel"). It's unclear what it plots, and it can't be turned off. Filed as #1037: a toggle, off by default, and a clearer label.
- **F4** (§1, observation): the Reopen With picker lists "Fieldglass Viewer" twice (`fieldglass.viewer` and `fieldglass.viewer.any`). Not filed.
- **F5** (§2, critique + defect): Export CSV sits in a different row from Export PNG. Its save dialog also suggests no name or folder (PNG suggests both), and doesn't add `.csv`, so the owner's export was saved as `~/Downloads/test-fieldglass` with no extension. The content was right: 10,512 rows, and 40°N 260°E read 246.6. Filed as #1039.
- **Note** (§1): the 141.4 to 501.4 longitude bounds on mirs_swath came from the owner panning. Whether a value over 360 should show in the Bounds fields is checked in §7.
- **F6** (§2): no panel names its file or store. NetCDF/Zarr headings say "Message 0 — s", and the slice tab title keeps the first variable's name. Filed as #1038.
- **F7** (§2.5e/f, wording, filed as #1040): selecting `fixtures/stores` (a folder of stores) says "is not a Zarr store — it holds no zarr.json, .zmetadata, .zgroup or .zarray. A store is a directory of metadata documents and chunks…". That is accurate but technical, and it doesn't say the folder *contains* stores. The v2_problems "Left out" reasons are raw codec and dtype tokens.
- **F8** (§3): steppers and the Animate label show only indexes (`level 5`, `step 5 of 0–16`), not 500 hPa or the date. Filed as #1041.
- **F9** (§3): the transport buttons are mismatched glyphs. Back has no bar while forward does, and ⏮/⏭ are small and above centre. Filed as #1042.
- **F10** (§4.3): the line plot has no ticks, the `1000 … 10` label reads backwards, a pressure profile is drawn sideways, the range caption `-72.769–31.484` hides the plus sign, and Field B's steppers are mistaken for the main ones. Axes filed as #1043. The caption went to #1037 and the stepper labels to #1041, as comments.
- **F11** (§5): cross-section end labels collide (`-90` over `2024-01-01`, `2024-12-31` under the colour bar, `90` under the note), and level × lat draws 1000 mb at the top. Added to #1043, which now covers line-plot and cross-section axes.
- **F12** (§5, feature): size a cross-section or source view independently of the panel. #1044, 0.7.0.
- **F13** (§6): GFS arrows smear along the top and bottom edges. Pole-row cells (one point, u/v in rotating meridian frames) each start an arrow. Filed as #1045.
- **F14** (§6): the speed key `⟶ 18.1 m s⁻¹` is a fixed glyph, so a reader can't see what a full-length arrow looks like. Filed as #1046 (draw a real reference arrow). #1045 is also confirmed as a half-fan at the South Pole in Orthographic −90.
- **F15** (§6, discussion): arrows over u-only colours, the from/to wind-direction conventions, derived fields and layering. Filed #1049 (0.6.0: warn on a non-u/v pair), #1047 (0.7.0: derived speed and direction with from/to), #1048 (0.7.0, needs-decision: arrows over any coloured field, plus the ROADMAP "manage layers" wording).
- **F16** (§6.7): Borders draws country lines only (NE 1:110m admin-0), with no state lines, and is coarse when zoomed in. Filed as 0.7.0 feature #1050.
- **F17** (§6.9): the arrow refusal ("component frame differs") stays on screen after a valid pair draws, because it shares the contour status line and is never cleared. Filed as #1051.

- **Note** (§7): after panning, lon reached 171 to 394 on `gfs.grib2`. The window runs past 360 and folds back once it is fully past. The owner finds this reasonable. Not filed; the §1 open question is closed.
- **F18** (§7, feature): with the colour scale on auto, zooming or panning rescales it to the values in the window. So the colours change even though the data didn't. Filed as #1079 (0.7.0): lock the auto range by default from the full view, with a Lock toggle.
- **F19** (§7.6, feature): Mollweide, Orthographic and polar stereographic ignore scroll and drag (out of scope in #245). Owner wants them, for 0.7.0. Drag spins the globe or shifts the central meridian, scroll zooms. Filed as #1080.

## Retest after fixes

Checks to run again once the issue's fix has merged and the addon and extension are rebuilt.

| Issue | Retest | In plan / done |
|---|---|---|
| #1035 | Fresh GRIB render panel (any, e.g. `samples/gfs.grib2`): no Message row under Export PNG. 2-D-only NetCDF variable: no Animate row. §23: the Message row appears, with options, after an ambiguous file change. | |
| #1036 | `samples/mirs_swath.nc` and `tiny_earth_pair.grib2`: unavailable controls are hidden, not greyed out; the agreed explanation shows. `gfs.grib2`: every control still shown. | |
| #1037 | NetCDF panel (`mirs_swath.nc` BT): Line plot box unticked by default, a click draws no plot; ticked, the plot appears with a label naming the variable and the axis. | |
| #1038 | Open `air_v2.zarr`, `air_v3.zarr` and `ncep_air_daily_2024.nc` on `air`: each tab and heading names its source. Switch the variable: the tab follows. No NetCDF/Zarr panel says "Message". |  |
| #1039 | Both export buttons in one row (GRIB and slice panels). The CSV dialog opens in the file's folder with a suggested `.csv` name. Typing `out` saves `out.csv`. |  |
| #1040 | Open Zarr Store on `fixtures/stores`: plain message listing the stores inside. On `v2_problems`: a plain lead sentence, no raw codec or dtype tokens. |  |
| #1041 | `ncep_air_levels_ltm.nc` level 5: 500 millibar beside the stepper, in the Animate label and in the subtitle. `ncep_air_daily_2024.nc` time 14: 2024-01-15. |  |
| #1042 | Animate buttons: matching SVG icons, equal size, centred; back and forward mirror. |  |
| #1043 | `ncep_air_levels_ltm.nc` 30°S 25°E along `level`: an upright profile with the surface at the bottom and millibar ticks. Level × lat section: ground at the bottom by default. Time × lat on the daily file: no colliding end labels. The Siberia time plot keeps date ticks. The sparse plot still shows two unjoined points. |  |
| #1045 | `gfs_wind.grib2` arrows: no band along the top or bottom edge in Equirectangular, and no starburst at either pole in Orthographic ±90. |  |
| #1046 | `gfs_wind.grib2` arrows: the key is a drawn arrow matching the longest on the map, and follows spacing and projection changes. |  |
| #1049 | `gfs_wind.grib2` UGRD+VGRD: no note. `cmc_pair.grib`: the not-a-u/v-pair note, and arrows still draw. VGRD as u: the swapped-order note. |  |
| #1051 | `hrrr_frames.grib2`: V #1 refused, then V #2 draws and the refusal is gone. Unticking Arrows clears the message. |  |
