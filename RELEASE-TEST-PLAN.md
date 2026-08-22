# Manual test plan — everything that changed since v0.3.0

Work top to bottom. Files are opened **once** each, with every check that file
can serve grouped under it, so you never reopen the same file twice.

`samples/README.md` has a per-file "does it look right" checklist for the grid /
packing coverage that shipped **at or before** 0.3.0 — this plan does not repeat
it beyond a quick regression pass (§5). Everything else below is new since 0.3.0.

**The headline this cycle:** the GRIB2 §5 packing census is complete (every
registered template decodes), and Fieldglass now **renders spherical-harmonic
spectral fields** — something no other viewer does. That render is the one thing
automated tests can't fully judge, so §1 is the priority.

A second wave landed after the census (#329–#344, PRs #345–#361): the panel
features that only ever worked on the plain single-field path — probe, contours,
CSV, PNG — were fixed to follow the field actually on screen (spectral,
combined, reprojected). Those fixes are marked **(wave 2)** below and are the
next priority after §1, because each one is a "the picture and the number
disagreed" class of bug.

Time: roughly 60–75 minutes.

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

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js
```

If `samples/` is empty: `tools/fetch_samples.sh`.

Every file in `samples/` holds exactly **one** GRIB message, and no NetCDF
sample has a second time step or level. The difference-map workflow needs two
fields, so §3 builds its own inputs — do that here so they're ready:

```sh
# Two identical messages on one grid → the combine path is exercisable, and
# every operation has an exactly known answer (see §3).
cat samples/gfs.grib2 samples/gfs.grib2 > /tmp/fg-pair.grib2

# Same grid type and size (latlon 1440x721) but a different longitude origin
# (0 vs 180) → must be refused, not silently combined.
cat samples/gfs.grib2 samples/ecmwf.grib2 > /tmp/fg-mixed.grib2
```

Launch the dev host with `F5` from the repo, then open files from the launched
window — or open one directly:

```sh
code --extensionDevelopmentPath="$PWD/extension" "$PWD/samples/gfs.grib2"
```

Some checks below open **test fixtures** (committed under `crates/*/tests/`)
rather than `samples/` files, because the feature they exercise has no
operational sample in the corpus. They open the same way:

```sh
code --extensionDevelopmentPath="$PWD/extension" \
     "$PWD/crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2"
```

---

## 1. Spectral field rendering — the flagship (test fixtures) · #302 #303 #322–#325 #330 #334 #335

No operational file in `samples/` is spectral (they're ECMWF archive), so use
the committed T63 fixtures. **This is the check to do carefully** — the numerics
are validated against the spec, but whether it *looks* like a real field is on
you.

Open `crates/fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2`:

- [ ] The message table shows parameter **Temperature**, packing **Spectral
      (spherical harmonic)**, and grid type `spherical_harmonic`.
- [ ] The row offers a **Render** button — *not* "Render not available — grid
      dimensions unknown". (This was the #288/#302 regression class.)
- [ ] Click **Render**. A smooth **global** field appears — a plausible
      temperature pattern (warm tropics, cold poles), roughly 235–317 K on the
      colorbar. No NaN blocks, no garbage speckle, no hard seams.
- [ ] The **poles read as single values** — the top and bottom rows are each a
      uniform colour (a pole is one point, longitude-independent).
- [ ] Reproject through **equirectangular, orthographic, Web Mercator**. The
      field warps sensibly and coastlines overlay on the right places (it is a
      real global grid after synthesis).
- [ ] Point-probe a few spots (§2a) — values read in Kelvin, tropics warmer than
      poles.

**(wave 2) Every other panel feature now works on a spectral message (#330).**
Before this fix only the map itself rendered; probe, contours, CSV and PNG all
failed with an internal error, because they went round the synthesis. Still on
`spectral_simple_t63.grib2`:

- [ ] **Contours** toggle on → isolines trace the synthesized field and follow
      the colour bands. No "internal error", no empty overlay.
- [ ] **Export CSV… → matrix** and **→ long** both write a file. The long form
      carries lat/lon per row on the synthesized global grid.
- [ ] **Export PNG…** writes the view, colorbar and overlays included.
- [ ] **Overlays** (coastlines, graticule) draw over the field.
- [ ] The first render may take a beat; **re-render** (change colormap, toggle an
      overlay, reproject) is fast — the synthesized field is cached and the
      transform precomputes its latitude-invariant tables (#334/#357, #335/#349).
      A second reprojection that visibly re-grinds for seconds is a regression.

Then, quicker, confirm the sibling paths render the same way:

- [ ] `spectral_complex_t63.grib2` — the ECMWF IFS complex form. Renders a
      smooth global field. (#324)
- [ ] `crates/fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1` — GRIB1
      spectral, same shared engine. Renders identically. (#325)

---

## 2. `samples/gfs.grib2` — the render-panel features · #172 #238 #243 #244 #292 #331 #332 #336 #338 #341

Open `samples/gfs.grib2`, click the message row → render panel. Everything below
is new since 0.3.0; do it all on this one open file.

- [ ] **2a. Point probe (#172/#299).** Click a point on the map → a readout
      shows the **value and its lat/lon** at that pixel. Click ocean vs. land,
      or high vs. low areas → the value tracks the colour. Click a transparent /
      off-grid pixel → no value (or a clean "no data"), not a crash or `NaN`.
- [ ] **2a′. (wave 2) Probe the periodic seam (#332/#346).** Reproject to
      **Mollweide** (leave the central meridian at 0, which puts the 359.75°→360°
      seam gap down the vertical centre of the map). Click a column of pixels
      straight down that centreline. **Every painted pixel must return a value** —
      a vertical stripe of "no data" through an otherwise painted field is the
      bug. Mollweide matters here: it oversamples columns ~2×, so pixels actually
      land inside the seam gap. An equirectangular raster never does, so this
      check is invisible there.
- [ ] **2b. Contour lines (#238/#298).** Toggle contours on → isolines overlay
      the field and **follow the colour bands** (a line sits on each colour
      transition, not offset). Change the contour interval → line density
      changes accordingly. They redraw correctly after a reprojection.
- [ ] **2b′. (wave 2) Contours survive a repaint cheaply (#336/#355).** With
      contours on, toggle an overlay and change the colormap. The lines redraw
      immediately and identically — extraction is memoized, so a repaint that
      stalls for seconds each time is a regression.
- [ ] **2c. Log10 colour scaling (#292).** Toggle log scale. On a positive field
      it recolours (compresses the high end). If the field dips to ≤ 0 the toggle
      is **disabled or refuses with a clear message** — it must not paint garbage
      (log of a non-positive value).
- [ ] **2d. Export PNG (#243).** Click **Export PNG…**, pick a location, then
      **open the saved `.png`**. It must match the on-screen view: the map
      raster **plus every overlay currently shown** (contours if on, coastlines,
      borders, graticule) **plus the colorbar and the title**, at the field's
      native resolution. The filename is derived from the parameter / slice. Try
      it once with contours + overlays on and once with them off — the export
      should reflect whichever is showing.
- [ ] **2d′. (wave 2) Exported colorbar midpoint under log10 (#331/#351).** Set a
      **manual range of `1..100`**, turn **log10 on**, export the PNG, and read
      the colorbar's **middle tick label** in the saved file. It must read **≈ 10**
      (the geometric mean, which is the value the colour at mid-height actually
      has), not `50.5`. Turn log10 off, export again: the mid tick reads `50.5`.
      The in-panel colorbar has no mid label, so this number only ever existed in
      the shared artifact — the file is the only place to check it.
- [ ] **2e. Export CSV (#244).** Click **Export CSV…** → **matrix** → open the
      `.csv`: a 2-D grid of values, empty cells where masked. Then **long** → a
      `lat,lon,value` table with one row per grid point; spot-check that a
      row's lat/lon matches where that value sits on the map. Confirm a
      large-export confirmation appears before writing.
- [ ] **2e′. (wave 2) Large CSV stays responsive (#341/#356).** GFS 0.25° is
      1440×721 ≈ 1M points. The long export should complete without the window
      going unresponsive or memory ballooning — the row building no longer
      allocates per cell.

---

## 3. Difference maps · #239 #293 #295 #296 #329 #333

**Read this first:** every GRIB2 file in `samples/` holds exactly one message,
and the Compare row is hidden below two fields — so the difference workflow
cannot be reached from any sample file as shipped. Use the two files built in
§0. NetCDF has the mirror problem (see 3c).

- [ ] **3a. GRIB difference, known answers (#295/#293).** Open
      `/tmp/fg-pair.grib2` — two identical GFS temperature messages. The panel
      shows a **Compare** row with an operation menu (`A − B`, `B − A`, `A + B`,
      `Mean`, `A / B`) and a Field B picker. Because B is a copy of A, every
      operation has an exact expected answer:
      - `A − B` → a **flat field of exactly 0**, on a **diverging colormap
        centred on zero**, colorbar range `0..0`.
      - `A / B` → flat **1**.
      - `A + B` → the field doubled: the colorbar range is exactly 2× field A's
        (GFS temperature ≈ 2×230–320 K).
      - `Mean` → identical to plain field A.
      Anything else — a torn raster, an off-centre diverging bar, a range that
      isn't the arithmetic above — is a real failure.
- [ ] **3b. (wave 2) The probe and contours read the *difference*, not field A
      (#329/#358).** This is what the self-difference file is really for: field A
      reads ≈ 230–320 K, the difference reads 0, so the two are impossible to
      confuse.
      - With `A − B` displayed, **probe** several points → each must read
        **≈ 0**, in the field's units. A probe that returns 250-something is
        reading field A under a raster painted with the difference — the bug.
      - Switch to `A + B` and **turn contours on** → the isolines must sit on the
        *doubled* field's colour bands (levels roughly 2× the ones plain field A
        draws). Contours tracing field A's levels over a doubled raster is the
        same bug.
- [ ] **3c. Mismatched grids are refused, not silently combined (#333/#345).**
      Open `/tmp/fg-mixed.grib2` — GFS and ECMWF, both `latlon 1440×721`, but
      with longitude origins 0 and 180. Pick any operation → it must fail with a
      **clear message** ("the two fields are on different grids; combining needs
      identical grid dimensions and definition"), not render a plausible-looking
      but meaningless field. Same-type, same-size, different-origin is exactly
      the case the old check let through, so this is the check that matters —
      a latlon-vs-Lambert pair (`cat samples/gfs.grib2 samples/hrrr.grib2`) is
      the easy case and optional.
- [ ] **3d. NetCDF difference (#296).** Open `samples/oisst.nc` and render an
      `sst` slice. The NetCDF Compare row differences the **same variable at its
      own slice indices** — and every NetCDF sample in the corpus is a single
      time step and single level (`time:1, zlev:1`), so Field B can only be the
      same slice. That still exercises the path: `A − B` → flat 0 on a diverging
      bar, `A / B` → 1, and a **probe on the difference reads 0**, not the SST
      value (the 3b check on the NetCDF path). For a substantive eyeball you need
      a multi-step file dropped in by hand — an `era5.nc` / `merra2.nc` subset
      with several time steps, or a real `wrfout` (see the auth-gated table in
      `samples/README.md`). With one of those: difference two adjacent time steps
      → a near-zero but structured field, largest where weather moved.

---

## 4. `samples/oisst.nc` (or `goes.nc`) — render-panel additions on NetCDF · #316 #317

The panel features are format-agnostic; confirm they work on a NetCDF slice too.

- [ ] Render a slice, then **Export CSV… (#317)** → matrix and long. The matrix
      is a rectangular grid of the slice; the long form has one row per grid
      point with sensible lat/lon.
- [ ] **Export PNG** the slice — image matches the on-screen view.
- [ ] **Point-probe** and **contour** the slice — same behaviour as GRIB.
- [ ] `oisst.nc` is a global 1/4° grid, so it also serves the **seam probe** of
      §2a′: reproject to Mollweide and probe down the centreline — every painted
      pixel returns a value.

---

## 5. Regression pass — the pre-0.3.0 corpus still renders

Three refactors this cycle moved shared code into `fieldglass-core` (the
second-order SPD inverse, the second-order group expansion, and the matrix
reshape), and the spectral-render wiring reworked the napi decode seam that
*every* feature now calls (#330/#359). Nothing here should have changed, so this
is a fast "still renders + reprojects" pass — see `samples/README.md` for the
per-file "looks right" detail:

- [ ] `hrrr.grib2` — Lambert complex-spatial-diff; render at **source** then
      reproject. Also **Export PNG + CSV** here to confirm export works off a
      **projected / warped** view, not just regular lat/lon.
- [ ] **(wave 2) The long-CSV refusal on a projected grid names CSV, not
      contours (#337/#347).** Still in `hrrr.grib2` (Lambert), **Export CSV… →
      long**. Long format needs per-point coordinates, which Lambert doesn't
      offer yet, so it must refuse — with a message that **names the CSV long
      format and points at the Matrix layout**, and that **never mentions
      contours**. Then export **matrix** from the same view: it succeeds.
- [ ] **(wave 2) A contour error doesn't clobber the render summary
      (#338/#350).** Still in `hrrr.grib2`: note the status line's render summary
      (dimensions + value range), then toggle **contours on**. Contours aren't
      wired for projected grids, so an error appears — on **its own line**, with
      the render summary still readable above it. Toggle contours **off** → the
      error line clears rather than lingering until the next full render.
- [ ] `rap.grib2` — JPEG 2000 on Lambert.
- [ ] `mrms.grib2` — PNG packing; set a manual range (e.g. `0..70`) to see the
      reflectivity past the −999 sentinel.
- [ ] `ecmwf.grib2` — CCSDS / AEC, global (decodes with no libaec dependency).
- [ ] `eccc.grib2` — JPEG 2000 on a rotated grid (unrotates correctly).
- [ ] `nbm.grib2` — inline missing-value management; value range ~267.9–315.8 K.
- [ ] `goes.nc` / `wrf.nc` — geostationary / WRF Lambert still frame correctly.

---

## 6. Not UI-testable this cycle (for awareness — no action)

Several §5 packings shipped this cycle as **decode-only** with **no operational
sample file**, so they can't be exercised from the UI. They are validated by
automated oracle / cross-edition tests (see each crate's
`tests/fixtures/NOTICE.md`), not by this plan:

- **Run-length (5.200)**, **log pre-processing (5.61)**, **second-order
  (5.50001 / 5.50002)** — decode to one value per grid point.
- **Bi-Fourier (5.53)** and **spectral (5.50 / 5.51)** — decode to coefficients;
  spectral additionally renders via §1, bi-Fourier does not render yet.
- **Matrix-of-values (5.1)** — the flat form renders like 5.0; the true per-point
  matrix decodes to a matrix field (not a single 2-D image), so there is nothing
  to eyeball.
- **Pre-standard local image (5.40000 / 5.40010)** — decode paths of 5.40 / 5.41.

If you want to *smoke-test* that the one-value-per-point ones paint at all, these
committed fixtures open in the dev host and render like any small grid:

```sh
code --extensionDevelopmentPath="$PWD/extension" \
     "$PWD/crates/fieldglass-grib2/tests/fixtures/second_order_regular_latlon.grib2"
code --extensionDevelopmentPath="$PWD/extension" \
     "$PWD/crates/fieldglass-grib1/tests/fixtures/hand_second_order_SPD1.grib1"
code --extensionDevelopmentPath="$PWD/extension" \
     "$PWD/crates/fieldglass-grib2/tests/fixtures/runlength_regular_latlon.grib2"
```

The two second-order fixtures are worth opening as a pair: #340/#361 moved the
group expansion into `fieldglass-core` so GRIB1 and GRIB2 now share one
implementation, and these are the two editions of it.

They are tiny synthetic grids, so the check is only "renders a coherent
low-resolution field", not a coastline pass.
