# Manual test plan — everything that changed since v0.4.0

Work top to bottom. Files are opened **once** each, with every check that file
can serve grouped under it, so you never reopen the same file twice.

`samples/README.md` has a per-file "does it look right" checklist for coverage
that shipped **at or before** 0.4.0 — this plan does not repeat it beyond a
quick regression pass (§7). Everything else below is new since 0.4.0.

**The headline this cycle is geometry.** Three grid families that Fieldglass
could previously open but not place on a map now render: HEALPix (§3.150),
reduced Gaussian in GRIB2 (§3.40 with no `Ni`), and NetCDF curvilinear grids
that give a latitude and longitude for every cell. All three reach the map by a
route no automated test can fully judge — one is resampled, one is row-expanded,
one is a nearest-cell lookup — so §1–§3 are the priority.

The second theme is **naming**: the WMO master parameter tables plus three
centres' local tables landed, which changes what the parameter, units and centre
columns say on almost every file. That is broad rather than deep, so §4 checks
it across whatever you already have open.

Time: roughly 60–80 minutes.

---

## 0. Setup (once)

```sh
# From the repo root, on master.
git checkout master && git pull

# Build the native module into the extension so the dev host runs current Rust,
# then compile the TypeScript.
( cd crates/fieldglass-napi && npx napi build --platform --release \
    --target x86_64-unknown-linux-gnu --output-dir "$(git rev-parse --show-toplevel)/extension/bin" )
( cd extension && npm run compile )

# Full samples, including the two curvilinear files added this cycle.
tools/fetch_samples.sh

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js
```

Then `F5` to launch the dev host.

Paths below are relative to the repo root. `F:` marks a committed crate fixture
(always present); `S:` marks a `samples/` file (needs the fetch above).

---

## 1. HEALPix grids (#416, #442, #443) — **priority**

HEALPix tiles the sphere into equal-area pixels and has no rows or columns at
all, so it is resampled onto a lat/lon grid at decode. Everything downstream
then treats it as an ordinary grid, which is exactly the assumption to check.

**Open `F: extension/src/test/fixtures/healpix_n4_ring.grib2`**

- [ ] The message table lists the message, and the **Size** column reads
      `Nside 4` — not a dash, and not an `Ni×Nj` it invented.
- [ ] Render in **Source**. It paints. (At `Nside 4` it is coarse by nature —
      12·16 = 192 pixels of data — so expect blocky, not detailed.)
- [ ] Switch to **Equirectangular**. The field lands on the map, and coastlines
      (Overlay → Coastlines) sit where they should against it.
- [ ] Switch to **Mollweide** and **Orthographic**. Both paint; nothing streaks
      or wraps across the seam.
- [ ] Point-probe a pixel: it reports a lat/lon **and** a value. A grid that
      resampled but did not geolocate reports the value with no coordinates.
- [ ] Contours on. Lines appear and follow the map through a projection change.

> Both orderings (RING and NESTED) are covered by the oracle tests against
> eccodes, pixel for pixel. What is being judged here is that the *resampled*
> grid is geolocated, which those tests do not see.

## 2. Curvilinear NetCDF — ocean tripolar and satellite swath (#218, #444, #445) — **priority**

These describe position as a list — a latitude and longitude per cell — with no
projection to compute from. Placement is a nearest-cell lookup, and the two
files break in opposite directions if it is wrong.

**Open `S: samples/rtofs_ice.nc`** (global HYCOM sea ice, 3298 × 4500)

- [ ] It opens on the **metadata view**, not on a picture: a NetCDF file lists
      its dimensions, attributes and variables, with a **Render** section
      holding one button per renderable variable. Nothing draws until you press
      one.
- [ ] That Render section offers the five ice fields and **no** button for
      `Latitude` / `Longitude`. Those are the grid's coordinates, not fields to
      draw; a button offering to draw a picture of latitude means the exclusion
      regressed (#218).
- [ ] It opens. **This is the memory and latency check**: the index is 14.8
      million cells, about 400 MB and two seconds, paid once. If the first
      render is slow that is expected; the *second* should feel instant.
- [ ] **The variable opens on the right plane without touching the axis
      pickers.** `ice_thickness` is `(MT, Y, X)`; the picker must land on
      `Y` × `X`, not the length-1 `MT`. A one-pixel-tall sliver means the axis
      defaults regressed (#218).
- [ ] **North is at the top in Source projection.** RTOFS stores its rows
      south-first, so an unflipped raster draws the Antarctic at the top. Every
      source view is flipped to face north-up (#286); a NetCDF file that looks
      inverted means that flip regressed.
- [ ] Press **Render ice_thickness**. (The panel opens on whichever button you
      press; there is also a **Variable** dropdown inside it for switching
      afterwards.) In **Source**: the Arctic third of the image is visibly
      folded — that is the bipolar patch, and it is the correct
      source-projection view.
- [ ] Switch to **Equirectangular**. The fold **unfolds into a real Arctic**.
      This is the whole feature; if the north of the image still looks folded or
      smeared, stop and report.
- [ ] Coastlines on: the ice edge follows the coast.
- [ ] Probe a point in the Arctic: plausible lat/lon (high north) and a value.
- [ ] Units column reads `m` / `degC` — **with no leading space** (#453).

**Open `S: samples/mirs_swath.nc`** (NOAA-21 MiRS, a full half orbit)

- [ ] Pick `TPW` or `RR`. Render **Equirectangular**: the track is a **single
      coherent band**, widening towards the bottom as the descending pass nears
      the south pole and the meridians fan out. A wrong longitude unwrap sprays
      it across the whole frame instead.

- [ ] Render **TPW** as well as `RR`. `TPW` covers the whole pass, so the band
      should **fan out across every longitude along the bottom edge** — at the
      pole the meridians converge, so a polar-crossing orbit genuinely does
      touch all of them. `RR` stops around 55°S because rain rate is not
      retrieved that far south; a blank bottom third on `RR` is the data, not a
      fault.

> The raster is shaped from the window rather than from the array (#515), so a
> swath is no longer drawn many times too tall. Whether every cell lands where
> its own file says it does is checked cell by cell in `curvilinear_corpus.rs`;
> this is a sanity look at the shape.
- [ ] Nothing paints outside the swath. Off-swath pixels stay transparent
      rather than being filled with the nearest edge cell.
- [ ] Ask for **Bilinear** resampling. The render caption must say
      **`(nearest)`** — a lookup grid downgrades, and the caption reports what
      actually happened rather than what was asked (#453 sibling fix).
- [ ] `RR` units read `mm hr⁻¹`, typeset (#453).
- [ ] Contours on: lines follow the swath, not the whole map.

## 3. Reduced Gaussian grids in GRIB2 (#500, #503)

ECMWF's ordinary output. Each row of latitude holds a different number of
points, so there is no `Ni`; the rows are widened to the widest one at decode.

**Open `F: crates/fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2`**

- [ ] **Size** column reads `N32` — the grid's own name, not `—` and not
      `128×64`.
- [ ] **Grid** column reads `reduced_gaussian` (it read `gaussian` before, which
      disagreed with the GRIB1 side).
- [ ] Render **Source**: paints, 128 × 64, temperatures in a plausible range.
- [ ] **Equirectangular**: a recognisable global temperature field. Polar rows
      are stretched copies of few points — that is correct for a reduced grid.
- [ ] Contours and probe both work.

**Open `F: crates/fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2`**

- [ ] **Size** reads `O32`, not `N32`. (This is the octahedral/classic split.)
- [ ] Render **Equirectangular**. It paints, and the field reaches the east
      edge with no blank column.

> The placement this file exists to catch is **not** an eye check. Its values
> are a sawtooth (`index mod 50`), so nothing in the field is meant to line up
> with a coast, and the defect is at most an eighth of a cell — sub-pixel at any
> normal window size. Judge it by probing instead: the grid's widest row is 144,
> but the file declares its last longitude from a 128-column grid, so **the last
> column must report 357.5°, not 357.1875°**. Column spacing is then 2.5°
> (360/144). A reader that trusts the declared `lonLast` slides every column
> progressively west.

**Open `F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1`**

- [ ] Size reads `N32` and Grid reads `reduced_gaussian` — **the same two
      strings the classic GRIB2 file showed** (the `N32` one, not the
      octahedral). The two editions describing one grid differently is what
      #503 fixed.
- [ ] Its field is a constant 285.5 K, so it renders as one flat colour and
      **draws no contours**. That is correct, not a failure: a constant has no
      isolines. The constant is deliberate — it pins the `grid_simple`
      all-values-equal-the-reference path.

**Open `F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1`**

- [ ] **Contours draw**, and follow the map through a projection change. They
      were refused for GRIB1 reduced grids before (#503). This is the sibling
      of the file above — same grid, same `PL` list, a smooth field in place of
      the constant, so there is something to contour.
- [ ] The isolines are **smooth zonal bands with two crests**. A break or kink
      in an otherwise continuous line means a row was widened to the wrong
      width.
- [ ] **Export CSV → long** from either reduced grid. The header is
      `lat,lon,value` and the coordinates are real. A reduced grid used to
      render and reproject but refuse to give per-point coordinates, so this is
      the other half of the same fix.

## 4. Parameter, unit and centre naming (#415, #424, #425, #426, #432, #440, #441, #469, #453)

Broad rather than deep. Do this against files you have open anyway — one NCEP,
one ECMWF, one DWD.

**Each of these samples holds exactly one message**, so the naming checks are a
spot check per centre, not a sweep. There is no sample carrying an ECMWF local
code, and none carrying a compound unit; both are pinned in CI instead (see the
note below).

- [ ] `S: samples/gfs.grib2` — its one message reads `TMP` / *Temperature* /
      `K`, not `Parameter 0/0/0`. The short name is NCEP's own spelling.
- [ ] `S: samples/icon.grib2` — its one message is a **DWD local code** and
      resolves: `CLCT_MOD` / *"Modified cloud cover for media"*. This is the
      substantive local-table check.
- [ ] `S: samples/ecmwf.grib2` — resolves to `TMP` / `K`. Note this is a plain
      WMO parameter, so it does **not** exercise the ECMWF local tables.
- [ ] **Centre** column shows WMO's own wording, e.g. *"European Centre for
      Medium Range Weather Forecasts (ECMWF) (RSMC)"*, with a sub-centre in
      parentheses where there is one. Check all three: NCEP, ECMWF and
      *"Offenbach (RSMC) - DWD"*.
- [ ] Strings that are *not* units are shown **verbatim, unmangled**. `icon.grib2`
      reads `Numeric`; it must not gain a superscript. This is the trap an
      over-eager typesetter falls into.
- [ ] **Typeset units.** `F: crates/fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib`
      reads `m s⁻¹` and `ecmwf_lfpw_msg0.grib1` reads `m² s⁻²`. Both are GRIB1,
      which is the point: the units column was typeset for GRIB2 and shown raw
      for GRIB1 until this cycle, and these two are the only GRIB messages in
      the tree whose units exercise it.
- [ ] The same in NetCDF: `S: samples/mirs_swath.nc` `RR` and `SFR` read
      `mm hr⁻¹`, `rtofs_ice.nc` `ice_uvelocity` reads `m s⁻¹`, and `goes.nc`
      `band_wavelength` reads `µm`.
- [ ] `S: samples/oisst.nc` — names the file chose are shown **as written**:
      `sst` reads `Celsius`, not rewritten to `°C`; `zlev` reads `meters`.
      `mirs_swath.nc` likewise reads `Kelvin`, spelled out.
- [ ] NetCDF units appear in **three** places, all of which must agree: the
      **Units column** of the variables table, the **render panel title**, and
      the **probe readout** (#453). They were absent from all three before.
- [ ] **Switch variables in the picker.** The title and the probe readout
      re-label to the new variable. In `oisst.nc`, moving from `sst` (`Celsius`)
      to `ice` (`%`) must change the unit; a stale `Celsius` against a
      percentage is the bug this check exists for.

> **The tables reach much further than any file here.** Only two GRIB messages
> in the tree carry a unit that exercises the typesetting, and no GRIB2 one
> does. `unit_notation.snapshot.txt` pins all 73 distinct unit strings the
> tables can produce, with exact outputs, so forms like `W m⁻² sr⁻¹` are
> covered exhaustively in CI even though nothing here displays them.

## 5. Projected grids: contours, CSV, and two placement fixes (#422, #423, #470, #472, #488, #490)

**Open `S: samples/hrrr.grib2`** (Lambert) — or `F: crates/fieldglass-grib2/tests/fixtures/eta_lambert_msg0.grib2`

- [ ] **Contours** draw over the Lambert field, in Source *and* in a reprojected
      view. They were refused entirely before (#470).
- [ ] With contours **and** coastlines on, turn **coastlines off**. The contours
      stay. They share one canvas with the geographic layers, and turning off
      the last of those used to wipe them until something forced a redraw.
- [ ] **Export CSV → long**. The header is `lat,lon,value` and the coordinates
      are real, not blank. (Long CSV was refused on projected grids before.)

**Open `F: crates/fieldglass-grib2/tests/fixtures/transverse_mercator_ukv.grib2`** (#422)

- [ ] Renders and reprojects. The grid is 24 × 30, so expect coarse blocks.
- [ ] **Judge placement by the Bounds column, not by the coastline.** It must
      read about 60.37°N / −13.61° to 48.20°N / 4.27°, which is the UKV domain.
      The shipped coastline is Natural Earth 1:110m and puts only 66 vertices
      inside this window, covering Britain, Ireland and the near Continent
      between them — too coarse to recognise, let alone to register a field
      against. Turn it on if you like, but it cannot settle this check.

**Open `F: crates/fieldglass-grib2/tests/fixtures/lambert_azimuthal_efas.grib2`** (#423)

- [ ] Renders and reprojects; the European domain sits where it should.

**Open `S: samples/eccc.grib2`** (#472)

- [ ] The **Bounds** column shows real numbers, not `NaN`.

> **#488 cannot be checked by eye, on any file in the tree.** The fix let a
> polar stereographic grid cross the equator. Neither candidate has data to
> lose: `eccc.grib2` is `rotated_latlon` (not polar at all) spanning 27.4°N to
> 70.6°N, and `cmc_wind_300_2010052400_p012.grib` is polar stereographic but
> eccodes puts its minimum latitude at 19.93°N with all 12,825 points present.
> `grid_round_trip.rs` covers it with a synthetic grid at `lat_first: 11.43`
> that does cross, and its own header records that the real fixture "starts at
> 27.2 degN, entirely in the northern" hemisphere. A real sub-equatorial file
> is worth fetching once remote fetching lands.

**Open `S: samples/goes.nc`** (#490)

- [ ] It renders and reprojects.

> **The limb check in #490 cannot be performed here.** `samples/goes.nc` is a
> *mesoscale* sector (`OR_ABI-L2-CMIPM1-…`, 500 × 500 at 2 km) sitting well
> inside the disc: all 250,000 pixels are on-earth, so there is no rim to be
> speckled. Neither crate fixture has one either — both are mesoscale or
> synthetic. The fix touched `projection.rs` and is covered by
> `grid_round_trip.rs`. A full-disk file would make this checkable and is worth
> fetching once remote fetching lands.

## 6. NetCDF containers and the metadata view (#412, #413, #452)

- [ ] `S: samples/oisst.nc` or any NetCDF-4 file opens; the metadata view shows
      dimensions, global attributes and variables, and the variables table has
      a **Units** column. Units used to be reachable only by rendering: the
      attribute preview stops at three, and `units` is the fifth attribute on
      every OISST field. Its Source view is north-up —
      OISST stores latitude ascending, so this is the regular-grid half of the
      same flip.
- [ ] **Narrow the editor pane until a row is wider than it.** The table
      **scrolls inside itself**; the heading and the render panel stay put
      (#452). Check both the GRIB message table and the NetCDF variables table.
- [ ] Open `F: crates/fieldglass-netcdf/tests/fixtures/hdf5_fletcher32.h5` and
      `hdf5_zstd.h5` (#412, #413). Both need right-click → **Reopen Editor
      With… → Fieldglass Viewer**: `.h5` matches only the opt-in editor, by
      design, so that a default-priority `*` does not hijack every file in the
      workspace. **No sample uses either filter** — all six are gzip/shuffle —
      so these fixtures are the only way to exercise the check.

## 7. Regression floor (pre-0.4.0 coverage)

A quick pass, not a full re-run of `samples/README.md`:

- [ ] `S: samples/gfs.grib2` — renders, reprojects, coastlines align.
- [ ] **Contours on `gfs.grib2` in Mollweide.** No isoline streaks straight
      across the map. Where the antimeridian runs through a grid, a contour
      crossing it used to be drawn as a line back across the whole width. Use
      Mollweide rather than Equirectangular: an equirectangular raster is as
      wide as the grid, which lands the last pixel on the seam and hides the
      gap the bug lives in.
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2` — a spectral field still synthesizes and
      renders (0.4.0's headline; the geometry work this cycle touched the seam
      it shares).
- [ ] **A field with no rows and columns states its own size**, rather than a
      dash. The three families each name themselves their own way: the spectral
      file above reads `T63`, `healpix_n4_ring.grib2` reads `Nside 4` (§1), and
      `F: crates/fieldglass-grib2/tests/fixtures/bifourier_ellipse_ieee32.grib2`
      reads `N4 M4`. Bi-Fourier decodes only to coefficients and does not
      render, so the Size column is all there is to check on it.
- [ ] `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` — classic NetCDF metadata view renders.
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` — the canonical GRIB2 path renders.
- [ ] Difference map, on **NetCDF**: `S: samples/mirs_swath.nc` → `BT` at two
      different `Channel` indices, **Compare**. The difference renders and the
      probe reads the *combined* value, not field A (#329). `oisst.nc`'s `sst`
      against `anom` works too — same 720 × 1440 grid.

> **Not on GRIB.** No GRIB file in the tree holds more than one message, so
> there is no pair to compare. `oisst.nc` cannot do it across time either —
> `time` and `zlev` are both length 1.
- [ ] **Export PNG** from one render; the file opens and matches what was on
      screen.

---

## Recording the outcome

Note pass/fail per section in the release PR or issue. A failure in §1–§3 blocks
the release; a failure in §4 is judged on how many files it touches.

**What this plan cannot see, by construction:** the value-level cross-checks
against eccodes (every committed fixture, every distinct unit string, every cell
of both curvilinear grids) all run in CI. This plan is only for the things that
need eyes — whether a picture is in the right place and reads correctly.

**Checks deliberately not made by eye.** Several fixes this cycle were verified
in CI against *synthetic* grids, because no committed file exhibits the defect:
#488 (a polar grid crossing the equator), #490 (the geostationary limb), and the
octahedral column placement, which is real but sub-pixel. Where that is the
case the section says so and names the covering test, rather than asking for a
judgement the file cannot support. **When writing a new check, open the fixture
first and confirm it can show the thing** — a constant field, a single-message
file, or a domain that never reaches the latitude in question will all pass a
visual check while proving nothing.

Two of these want a real file rather than a synthetic one, and are worth
revisiting once remote fetching lands: a sub-equatorial polar stereographic
grid, and a GOES full-disk scene.
