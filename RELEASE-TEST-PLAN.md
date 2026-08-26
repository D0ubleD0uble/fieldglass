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

- [ ] The variable picker offers the ice fields and **not** `Latitude` /
      `Longitude`. Those are the grid's coordinates, not fields to draw; a file
      that opens on a picture of latitude means the exclusion regressed (#218).
- [ ] It opens. **This is the memory and latency check**: the index is 14.8
      million cells, about 400 MB and two seconds, paid once. If the first
      render is slow that is expected; the *second* should feel instant.
- [ ] **The variable opens on the right plane without touching the axis
      pickers.** `ice_thickness` is `(MT, Y, X)`; the picker must land on
      `Y` × `X`, not the length-1 `MT`. A one-pixel-tall sliver means the axis
      defaults regressed (#218).
- [ ] Pick `ice_thickness` from the variable dropdown (the file opens on
      `ice_coverage`). Render in **Source**: the
      Arctic third of the image is visibly folded — that is the bipolar patch,
      and it is the correct source-projection view.
- [ ] Switch to **Equirectangular**. The fold **unfolds into a real Arctic**.
      This is the whole feature; if the north of the image still looks folded or
      smeared, stop and report.
- [ ] Coastlines on: the ice edge follows the coast.
- [ ] Probe a point in the Arctic: plausible lat/lon (high north) and a value.
- [ ] Units column reads `m` / `degC` — **with no leading space** (#453).

**Open `S: samples/mirs_swath.nc`** (NOAA-21 MiRS, a full half orbit)

- [ ] Pick `TPW` or `RR`. Render **Equirectangular**: it reads as a **ribbon
      across the globe, not a smear**. The pass crosses the antimeridian and
      sweeps over the south pole, so a wrong longitude unwrap shows up here as
      the ribbon spraying across the whole map.
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
- [ ] Render **Equirectangular**. **Look at the east edge**: the field should
      reach the antimeridian cleanly. This grid's widest row is 144 where the
      file declares its last longitude from a 128-column grid, so a wrong
      reading slides every column progressively west — subtle, up to an eighth
      of a cell, most visible as a mismatch against coastlines near 180°.

**Open `F: crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1`**

- [ ] Size reads `N32` and Grid reads `reduced_gaussian` — **the same two
      strings the GRIB2 file showed**. The two editions describing one grid
      differently is what #503 fixed.
- [ ] Contours draw (they were refused for GRIB1 reduced grids before).

## 4. Parameter, unit and centre naming (#415, #424, #425, #426, #432, #440, #441, #469, #453)

Broad rather than deep. Do this against files you have open anyway — one NCEP,
one ECMWF, one DWD.

- [ ] `S: samples/gfs.grib2` — parameters have **names and units**, not
      `Parameter 0/3/192`. Short names read as NCEP writes them (`TMP`,
      `UGRD`, `APCP`, `MSLET`, `REFC`).
- [ ] `S: samples/ecmwf.grib2` — ECMWF local codes (≥ 192) resolve to names
      rather than showing the numeric triple.
- [ ] `S: samples/icon.grib2` — DWD local codes resolve.
- [ ] **Centre** column shows WMO's own wording, e.g. *"European Centre for
      Medium Range Weather Forecasts (ECMWF) (RSMC)"*, with a sub-centre in
      parentheses where there is one.
- [ ] **Units** read as typeset symbols — `m s⁻¹`, `kg m⁻²`, `W m⁻² sr⁻¹` —
      not `m/s` or `kg m-2`. Strings that are *not* units (`Code table 4.253`,
      `Numeric`, `CCITT IA5`) are shown verbatim, unmangled.
- [ ] `S: samples/goes.nc` — a NetCDF variable's units now appear **in the
      render panel title and the probe readout**, where they were absent before
      (#453). Names the file chose (`kelvin`, `meters`, `degree_C`) are shown as
      written, not rewritten.

## 5. Projected grids: contours, CSV, and two placement fixes (#422, #423, #470, #472, #488, #490)

**Open `S: samples/hrrr.grib2`** (Lambert) — or `F: crates/fieldglass-grib2/tests/fixtures/eta_lambert_msg0.grib2`

- [ ] **Contours** draw over the Lambert field, in Source *and* in a reprojected
      view. They were refused entirely before (#470).
- [ ] **Export CSV → long**. The header is `lat,lon,value` and the coordinates
      are real, not blank. (Long CSV was refused on projected grids before.)

**Open `F: crates/fieldglass-grib2/tests/fixtures/transverse_mercator_ukv.grib2`** (#422)

- [ ] Renders and reprojects; the UK lands on the UK with coastlines on.

**Open `F: crates/fieldglass-grib2/tests/fixtures/lambert_azimuthal_efas.grib2`** (#423)

- [ ] Renders and reprojects; the European domain sits where it should.

**Open `S: samples/eccc.grib2`** or `F: crates/fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib` (#488)

- [ ] The **whole** grid paints. This north polar stereographic grid reaches
      4.7° *south*, and everything below the equator used to drop out — look
      for a hard horizontal edge across the image where data simply stops.
- [ ] The **Bounds** column shows real numbers, not `NaN` (#472).

**Open `S: samples/goes.nc`** (#490)

- [ ] The disc edge is **clean**, not speckled. Border pixels used to drop out
      individually, giving a stippled rim rather than a smooth limb.

## 6. NetCDF containers and the metadata view (#412, #413, #452)

- [ ] `S: samples/oisst.nc` or any NetCDF-4 file opens; the metadata view shows
      dimensions, global attributes and variables.
- [ ] **Narrow the editor pane until a row is wider than it.** The table
      **scrolls inside itself**; the heading and the render panel stay put
      (#452). Check both the GRIB message table and the NetCDF variables table.
- [ ] A file using fletcher32 or zstd opens rather than failing (#412, #413) —
      the crate fixtures `hdf5_fletcher32.h5` and `hdf5_zstd.h5` cover the
      decode; here just confirm nothing regressed on the real files.

## 7. Regression floor (pre-0.4.0 coverage)

A quick pass, not a full re-run of `samples/README.md`:

- [ ] `S: samples/gfs.grib2` — renders, reprojects, coastlines align.
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2` — a spectral field still synthesizes and
      renders (0.4.0's headline; the geometry work this cycle touched the seam
      it shares).
- [ ] `F: extension/src/test/fixtures/netcdf_classic_dummy.nc` — classic NetCDF metadata view renders.
- [ ] `F: crates/fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2` — the canonical GRIB2 path renders.
- [ ] Difference map: pick two messages of one variable, **Compare** → the
      difference renders and the probe reads the *combined* value.
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
