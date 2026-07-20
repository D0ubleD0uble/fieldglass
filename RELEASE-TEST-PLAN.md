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

Time: roughly 45–60 minutes.

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

## 1. Spectral field rendering — the flagship (test fixtures) · #302 #303 #322–#325

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

Then, quicker, confirm the sibling paths render the same way:

- [ ] `spectral_complex_t63.grib2` — the ECMWF IFS complex form. Renders a
      smooth global field. (#324)
- [ ] `crates/fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1` — GRIB1
      spectral, same shared engine. Renders identically. (#325)

---

## 2. `samples/gfs.grib2` — the new render-panel features · #172 #238 #243 #244 #292

Open `samples/gfs.grib2`, click a message row → render panel. Everything below is
new this cycle; do them all on this one open file.

- [ ] **2a. Point probe (#172/#299).** Click a point on the map → a readout
      shows the **value and its lat/lon** at that pixel. Click ocean vs. land,
      or high vs. low areas → the value tracks the colour. Click a transparent /
      off-grid pixel → no value (or a clean "no data"), not a crash or `NaN`.
- [ ] **2b. Contour lines (#238/#298).** Toggle contours on → isolines overlay
      the field and **follow the colour bands** (a line sits on each colour
      transition, not offset). Change the contour interval → line density
      changes accordingly. They redraw correctly after a reprojection.
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
- [ ] **2e. Export CSV (#244).** Click **Export CSV…** → **matrix** → open the
      `.csv`: a 2-D grid of values, empty cells where masked. Then **long** → a
      `lat,lon,value` table with one row per grid point; spot-check that a
      row's lat/lon matches where that value sits on the map. Confirm a
      large-export confirmation appears before writing.

---

## 3. Difference maps · #239 #293 #295 #296

- [ ] **3a. GRIB difference (#295).** Still in `gfs.grib2` (or any file with two
      same-grid messages), use the **difference** workflow → pick a second
      message → an `a − b` field renders through the normal pipeline with a
      **diverging colormap centred on zero**. Sum / ratio / average variants
      (#293) produce sensible fields. Mismatched-grid messages are refused with a
      clear message, not a torn render.
- [ ] **3b. NetCDF difference (#296).** Open `samples/oisst.nc`, render a slice,
      then difference it against another time step / slice → the difference
      renders (near-zero for adjacent steps).

---

## 4. `samples/oisst.nc` (or `goes.nc`) — render-panel additions on NetCDF · #316 #317

The panel features are format-agnostic; confirm they work on a NetCDF slice too.

- [ ] Render a slice, then **Export CSV… (#317)** → matrix and long. The matrix
      is a rectangular grid of the slice; the long form has one row per grid
      point with sensible lat/lon.
- [ ] **Export PNG** the slice — image matches the on-screen view.
- [ ] **Point-probe** and **contour** the slice — same behaviour as GRIB.

---

## 5. Regression pass — the pre-0.3.0 corpus still renders

Two refactors this cycle moved shared code into `fieldglass-core` (the
second-order SPD inverse and the matrix reshape) and the spectral-render wiring
touched `provider.ts` / `render_grid`. Nothing here should have changed, so this
is a fast "still renders + reprojects" pass — see `samples/README.md` for the
per-file "looks right" detail:

- [ ] `hrrr.grib2` — Lambert complex-spatial-diff; render at **source** then
      reproject. Also **Export PNG + CSV** here to confirm export works off a
      **projected / warped** view, not just regular lat/lon.
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
     "$PWD/crates/fieldglass-grib2/tests/fixtures/runlength_regular_latlon.grib2"
```

They are tiny synthetic grids, so the check is only "renders a coherent
low-resolution field", not a coastline pass.
