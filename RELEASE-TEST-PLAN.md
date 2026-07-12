# Manual test plan — everything that changed since v0.2.0

Work top to bottom. Files are opened **once** each, with every check that file
can serve grouped under it, so you never reopen the same file twice.

`samples/README.md` already has a per-file "does it look right" checklist for the
features that shipped **in** 0.2.0 — this plan does not repeat it. Everything
below is new or changed since that release.

Time: roughly 45–60 minutes.

---

## 0. Setup (once)

```sh
# From the repo root, on master.
git checkout master && git pull

# Build the native module into the extension so the dev host runs current Rust.
( cd crates/fieldglass-napi && npx napi build --platform --release \
    --target x86_64-unknown-linux-gnu --output-dir "$(git rev-parse --show-toplevel)/extension/bin" )
( cd extension && npm run compile )

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js
```

If `samples/` is empty: `tools/fetch_samples.sh`.

Launch the dev host with `F5` from the repo, then open files from the launched
window. Or open one directly:

```sh
code --extensionDevelopmentPath="$PWD/extension" "$PWD/samples/gfs.grib2"
```

---

## 1. `samples/gfs.grib2` — the whole new render panel

One global field, three new control groups. Everything in this section is new
UI; if the panel looks right here it is right everywhere.

Open the file, click a message row to open the render panel.

### 1a. Colormap picker (new)

- [ ] A **Colors** row exists: a `Colormap` dropdown and a `Reverse` checkbox.
- [ ] The dropdown is grouped: **Sequential** (Viridis, Plasma, Cividis, Turbo,
      Grayscale) and **Diverging** (Red–Blue, Brown–Teal, Cool–Warm) — 8 total.
- [ ] It opens on **Viridis**, and the image looks exactly as it did in 0.2.0.
- [ ] Pick each of the 8 in turn. The **image repaints** and the **legend strip
      on the right changes to match**. The strip must never disagree with the
      image (that was the bug this design removes).
- [ ] Tick **Reverse**: the legend flips end-for-end and the image inverts its
      colours. Untick: back to normal.
- [ ] Grayscale should be a clean black→white ramp with no colour cast.
- [ ] A diverging map (Red–Blue) should be pale in the middle, saturated at both
      ends.

### 1b. Projection picker — three new world maps

The picker should now list **8 targets**. The three new ones:

- [ ] **Mollweide** — ellipse, 2:1. Corners outside the ellipse are background,
      not black or garbage.
- [ ] **Robinson** — rounded rectangle, flat-ish top and bottom (the poles are
      lines, not points).
- [ ] **Equal Earth** — like Robinson but with more curved sides. Greenland
      should look *small* relative to Africa (it is equal-area); on Robinson,
      Greenland is noticeably larger.
- [ ] Each shows a **Central meridian** box. Set it to `180` on any of them: the
      map recentres on the Pacific, Americas on the right, **no smearing or a
      line dragged across the map** (that would be a broken seam).
- [ ] Coastlines land on coastlines in all three.

### 1c. Overlay layers — four new ones

The Overlay row should now have five layers, each with its own colour swatch and
line-weight box.

- [ ] Toggle **Borders** on: country boundaries appear (Africa, Europe, S. America
      are the obvious ones).
- [ ] Toggle **Lakes**: Great Lakes, Caspian, Victoria.
- [ ] Toggle **Rivers**: Amazon, Nile, Mississippi, Ob.
- [ ] Toggle **Coastlines** and **Graticule** (as before).
- [ ] Change a layer's **colour**: it repaints **immediately** and *only that
      layer* changes. (It should not re-render the field — a colour change is a
      repaint, not a reprojection.)
- [ ] Change a layer's **line weight**: same.
- [ ] All five on at once: draw order is sensible — the graticule sits on top,
      lines are not obscured by the field.

### 1d. Persistence (all of the above)

- [ ] Set a non-default state: Turbo + Reverse, Robinson, borders + rivers on
      with a custom colour.
- [ ] Switch to another editor tab, then switch **back**.
- [ ] Everything comes back as you left it — colormap, reverse, projection,
      central meridian, and each layer's toggle/colour/weight.

---

## 2. `samples/hrrr.grib2`, `nam.grib2`, `rap.grib2` — the *regression* check

These are Lambert grids that declare **shape 6** (the radius we were already
using), so the Earth-radius fix (#271) changes **nothing** for them.

> **These must look identical to 0.2.0.** If a coastline moved here, the fix
> broke something. This is the most important check in the plan.

For each of the three:

- [ ] Reproject to **equirectangular**, coastlines on. Coastlines land on the US
      Gulf / Atlantic / Pacific coasts exactly as before.
- [ ] Not mirrored, not upside-down, no dateline tear.

Also, on `hrrr.grib2` (Lambert), the edge-pixel fix (#267):

- [ ] Zoom in on the **outermost row and column** of the field. There should be
      **no 1-pixel transparent seam** along the edge. Before the fix, a point
      lying exactly on the grid boundary was rejected and painted as background.

---

## 3. GRIB1 polar stereographic — the Earth-radius fix you can *see*

This is where #271 actually shows up. GRIB2 samples all declare a radius within
29 m of what we used, so the fix is invisible there. GRIB1 declares
**6 367 470 m** where we used 6 371 229 m — a 3.5 km error at the far corner.

Open **`crates/fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib`**
(a real CMC 300 hPa wind field on a 135×95 polar-stereographic grid over North
America).

- [ ] It opens, the message table populates, and it renders.
- [ ] Reproject to **equirectangular** with coastlines on.
- [ ] **Coastlines align with the field.** Look at the far corner of the domain
      (the north-east, away from the grid origin) — that is where the old 3.5 km
      error was worst. Compare against 0.2.0 if you want: the field should have
      shifted *slightly*, and shifted *toward* correct.
- [ ] Nothing is mirrored or torn.

---

## 4. `samples/nbm.grib2` — GRIB2 inline missing values

New decode path since 0.2.0 (complex packing with missing-value management).
NBM is also the one GRIB2 sample with a producer-specified radius (shape 1), so
it lightly exercises that path too.

- [ ] Renders (it used to report an unsupported template).
- [ ] Reproject to equirectangular, coastlines on: CONUS temperature, aligned.
- [ ] The value range reads roughly **267.9 … 315.8 K** (auto range).
- [ ] The sparse / no-data regions read as **missing (transparent)**, not as a
      spike of real values.

---

## 5. `samples/goes.nc` — CF unpacking + geostationary

- [ ] Renders. Off-disk pixels (the corners of the full disk) are **transparent**,
      not black.
- [ ] The colorbar reads in **real units** (K or radiance), not raw integer
      codes. This is CF `scale_factor` / `add_offset` being applied.
- [ ] Reproject to equirectangular and orthographic: the disk maps correctly.

---

## 6. `samples/oisst.nc` — NetCDF-4 chunked decode

- [ ] Renders. Global SST.
- [ ] Land is **masked / transparent** (fill values), not a block of colour.
- [ ] Colorbar in °C or K.

---

## 7. NetCDF nested groups (new)

Open **`crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc`**.

- [ ] The **variables table is not empty**. Before this change, a file that put
      its variables inside groups showed an empty or near-empty list.
- [ ] Variable names are **path-qualified**, e.g. `/PRODUCT/...`.
- [ ] A grouped variable **renders** when you click through to it.

> If you have a real Sentinel-5P (`/PRODUCT/...`) or GPM IMERG (`/Grid/...`)
> file, that is a much better test than the synthetic fixture — drop it in.

---

## 8. WRF projections (new: polar stereo, Mercator, unrotated lat-lon)

The bundled `samples/wrf.nc` is the tiny 6×5 synthetic file — enough to prove
the attribute path works, too small to judge a coastline.

Open each of these fixtures and confirm it **renders and reprojects** (not that
it looks beautiful):

- [ ] `crates/fieldglass-netcdf/tests/fixtures/wrf_lambert.nc`
- [ ] `crates/fieldglass-netcdf/tests/fixtures/wrf_polar.nc` (new)
- [ ] `crates/fieldglass-netcdf/tests/fixtures/wrf_mercator.nc` (new)
- [ ] `crates/fieldglass-netcdf/tests/fixtures/wrf_latlon.nc` (new)

Each should reproject to equirectangular rather than falling back to a
source-only image.

> **If you have a real `wrfout` file, use it instead** — this is the weakest part
> of the corpus, and a real regional domain with coastlines is the only way to
> confirm WRF geolocation properly (it also now uses WRF's own 6 370 000 m
> sphere, changed in #271).

---

## 9. GRIB1 spectral — decodes, but deliberately does not render

New since 0.2.0: spectral coefficients decode through the Rust API, but a
spectral message has **no grid**, so it cannot be drawn as an image. The point of
this check is that it **fails cleanly**, not that it draws.

Open **`crates/fieldglass-grib1/tests/fixtures/spectral_complex_t63.grib1`**.

- [ ] The message table populates, and the packing reads **`spectral_complex`**.
- [ ] Clicking Render gives a **clear message** saying spherical-harmonic
      coefficients are not values on a grid — **not a crash, not a blank panel,
      and not a garbage image**.

---

## 10. Regression sweep (fast)

Anything from `samples/README.md`'s checklist you want to spot-check. At minimum:

- [ ] `samples/ecmwf.grib2` (CCSDS) renders.
- [ ] `samples/mrms.grib2` (PNG packing) renders — remember the −999 sentinel, so
      set a manual range like 0..70 to see reflectivity.
- [ ] `samples/eccc.grib2` (rotated lat/lon) unrotates so coastlines land right.

---

## Not manually testable (no action needed)

These changed since 0.2.0 but have no UI surface. They are covered by
eccodes-validated tests in CI:

- GRIB2 constant complex-packed fields (zero groups) — needs a hand-built file.
- The GRIB1 bit-reader hardening (a corrupt >32-bit width now errors instead of
  decoding wrong values).
- Per-grid-point geolocation (`#267`) — a Rust API addition with no UI yet.
- crates.io publishing (`#269`) — CI only; exercised by the release tag itself.

---

## If something looks wrong

Note the **file**, the **projection**, the **colormap**, and what you expected.
A screenshot of the render panel is worth a lot. The most likely place for a real
regression is §2 (coastlines that should *not* have moved).
