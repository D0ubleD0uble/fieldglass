# Manual test plan — everything that changed since v0.2.0

Work top to bottom. Files are opened **once** each, with every check that file
can serve grouped under it, so you never reopen the same file twice.

`samples/README.md` already has a per-file "does it look right" checklist for the
features that shipped **in** 0.2.0 — this plan does not repeat it. Everything
below is new or changed since that release.

Time: roughly 60–75 minutes.

---

## 0. Setup (once)

```sh
# From the repo root, on master.
git checkout master && git pull

# Build the native module into the extension so the dev host runs current Rust.
# (The napi runtime was bumped to 3.10.2 since 0.2.0, so this rebuild matters —
# see §11.)
( cd crates/fieldglass-napi && npx napi build --platform --release \
    --target x86_64-unknown-linux-gnu --output-dir "$(git rev-parse --show-toplevel)/extension/bin" )
( cd extension && npm run compile )

# One sample is generated rather than fetched — the "latest format" NetCDF-4
# file §7 needs. Nothing else in the corpus exercises that decode path.
python3 tools/build_latest_format_sample.py

# Sanity: decode + reproject every sample headlessly before touching the UI.
node tools/preflight_samples.js
```

If `samples/` is empty: `tools/fetch_samples.sh`.

Launch the dev host with `F5` from the repo, then open files from the launched
window. Or open one directly:

```sh
code --extensionDevelopmentPath="$PWD/extension" "$PWD/samples/gfs.grib2"
```

**Opening a file the viewer doesn't claim by extension** (the `.h5` fixtures in
§7): right-click it → **Open With…** → **Fieldglass Viewer**. The viewer
registers `.grib*`/`.nc*` by default and everything else as an option.

---

## 1. `samples/gfs.grib2` — the whole new render panel

One global field, three new control groups. Everything in this section is new
UI. The panel is shared across GRIB1, GRIB2, and NetCDF, and all three funnel
into the same Rust painter, so if it looks right here it is *painted* right
everywhere — but which targets the picker *offers* is decided per grid, so §3
and §6 re-check that on a GRIB1 and a NetCDF grid.

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

Also, on `hrrr.grib2` (Lambert), the edge-pixel fix:

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

While this file is open — the new panel on a **GRIB1** reader (a different napi
entry point from §1's GRIB2 one):

- [ ] The **colormap dropdown** offers all 8 and repaints the image.
- [ ] The **overlay row** offers all five layers; borders and lakes draw.

---

## 4. GRIB2 complex packing — the new decode paths

### 4a. `samples/nbm.grib2` — the new decode path on a real file

New decode path since 0.2.0 (complex packing with missing-value management).
NBM is also the one GRIB2 sample with a producer-specified radius (shape 1), so
it lightly exercises that path too.

The file holds **one** message (2 m temperature). It *declares* missing-value
management (`mvmu=1`) — which is what 0.2.0 rejected, and what makes it decode
here — but the temperature field itself has **no missing points**. So do not go
looking for gaps in it: the proof the mvm-aware decode is right is that the
**values land in the right place**, since a mis-read of the reserved sentinel
shifts the whole field. Actual masking is checked on the fixtures in §4b.

- [ ] Renders (it used to report an unsupported template).
- [ ] Reproject to equirectangular, coastlines on: CONUS temperature, aligned.
- [ ] The value range reads **267.9 … 315.8 K** (auto range). This is the real
      check — it is eccodes' range for this field, to the tenth. A number well
      outside it means the sentinel was decoded as data.
- [ ] The transparent area is only *outside* the CONUS domain (an equirectangular
      canvas around a Lambert grid). There should be **no holes inside** it.

### 4b. The hand-built fixtures — the modes NBM does *not* cover

#217 and #222 each claim more than one mode, and a real file only exercises one
of them. These fixtures are tiny regular lat/lon grids, and the viewer opens
`.grib2` by default, so each one renders. Open each from
`crates/fieldglass-grib2/tests/fixtures/`:

- [ ] `complex_mvm1_regular_latlon.grib2` — **primary** missing values, and the
      only place you can *see* masking work. A 16×31 field with **46 missing
      points**, which must be **transparent**. The legend should read
      **270.5 … 311.1**; the missing points must not appear as a spike at either
      end.
- [ ] `complex_mvm2_regular_latlon.grib2` — **primary + secondary** substitutes:
      **48 missing points**, same expectations. This second mode appears in no
      real sample, so this fixture is its only check.
- [ ] `complex_rowbyrow_regular_latlon.grib2` — groups split **row by row**.
      Renders rather than reporting an unsupported template. No banding and no
      row-shifted image (a mis-read row split shows up as visible stripes).
- [ ] `complex_ng0_regular_latlon.grib2` — **zero groups**, i.e. a constant
      field (#222). Renders as one **flat colour** rather than reporting a
      malformed message, and the legend's min and max are **both 270.47**.
- [ ] `complex_spd2_ng0_regular_latlon.grib2` — the same, with spatial
      differencing. Also flat, also 270.47.

---

## 5. `samples/goes.nc` — CF unpacking + geostationary

- [ ] Renders. Off-disk pixels (the corners of the full disk) are **transparent**,
      not black.
- [ ] The colorbar reads in **real units** (K or radiance), not raw integer
      codes. This is CF `scale_factor` / `add_offset` being applied.
- [ ] Reproject to equirectangular and orthographic: the disk maps correctly.

---

## 6. `samples/oisst.nc` — the NetCDF regression check, and the panel on NetCDF

This file is chunked, but netCDF-4 wrote it in the **default** format, whose
chunk index is the **version-1 B-tree** — the path that shipped in 0.2.0. So this
is a *regression* check, not coverage of the new chunk indexes. Those are §7.

- [ ] Renders. Global SST.
- [ ] Land is **masked / transparent** (fill values), not a block of colour.
- [ ] Colorbar in °C or K.

The new panel on a **NetCDF** reader (`render_slice`, the third napi entry point,
and a synthesised lat/lon grid — the picker decides its eligible targets from the
grid, so the new world targets have to be offered here too):

- [ ] The projection picker offers **Mollweide, Robinson, and Equal Earth** (this
      is a global grid, so it is eligible for them). Pick each: the field fills
      the shape, coastlines land right.
- [ ] The **colormap dropdown** repaints the field and the legend agrees.
- [ ] **Borders / lakes / rivers** draw over it.

---

## 7. NetCDF-4 "latest format" — the new chunk indexes (#216)

**No fetched sample covers this** — `oisst.nc` and `goes.nc` are both version-1
B-tree (see §6). Files written by a recent libhdf5 use the version-4 / version-5
indexes instead, which is what #216 added, and this is the only check of them.

`samples/latest_format.nc` is generated in §0 by
`tools/build_latest_format_sample.py`. It is a global 90×180 field with two
variables: `t2m` (a **fixed-array** chunk index) and `t2m_growable` (a
**filtered extensible-array** index, from its unlimited time dimension).

- [ ] The variables table lists **`t2m` and `t2m_growable`**.
- [ ] **`t2m` renders**: a smooth global field, warm at the equator, cool at the
      poles, with a gentle wave in longitude. Coastlines on: it is a plain lat/lon
      grid, so they land normally.
- [ ] **`t2m_growable` renders** and looks *the same* (it holds the same field).
      Step its **time** index — there is one step.
- [ ] Neither reports **"HDF5 data layout message version 4 / 5 is not
      supported"**. That is the exact error 0.2.0 gives on this file, so if you
      see it, the build is stale — rebuild the native module (§0).

The remaining indexes (implicit, v2-B-tree, single-chunk) only exist as bare HDF5
fixtures with no coordinate variables, so they have no renderable variable and
the UI cannot reach their decode. Open them via **Open With… → Fieldglass
Viewer** and confirm the **metadata tables populate** — that is as far as the UI
goes; their value decode is covered by the h5py-checked tests in CI.

- [ ] `crates/fieldglass-netcdf/tests/fixtures/hdf5_implicit_index.h5`
- [ ] `crates/fieldglass-netcdf/tests/fixtures/hdf5_v2_btree_index.h5`
- [ ] `crates/fieldglass-netcdf/tests/fixtures/hdf5_v4_chunk_index.h5`

---

## 8. NetCDF nested groups (new)

Open **`crates/fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc`**.

- [ ] The **variables table is not empty**. Before this change, a file that put
      its variables inside groups showed an empty or near-empty list.
- [ ] Variable names are **path-qualified**, e.g. `/PRODUCT/...`.
- [ ] A grouped variable **renders** when you click through to it.

> If you have a real Sentinel-5P (`/PRODUCT/...`) or GPM IMERG (`/Grid/...`)
> file, that is a much better test than the synthetic fixture — drop it in.

---

## 9. WRF projections (new: polar stereo, Mercator, unrotated lat-lon)

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

## 10. GRIB1 spectral — decodes, but deliberately does not render

New since 0.2.0: spectral coefficients decode through the Rust API, but a
spectral message has **no grid**, so it cannot be drawn as an image. The point of
this check is that it **fails cleanly**, not that it draws. Both packings the
changelog claims are here — check both.

Open **`crates/fieldglass-grib1/tests/fixtures/spectral_complex_t63.grib1`**:

- [ ] The message table populates, and the packing reads **`spectral_complex`**.
- [ ] Clicking Render gives a **clear message** saying spherical-harmonic
      coefficients are not values on a grid — **not a crash, not a blank panel,
      and not a garbage image**.

Then **`crates/fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1`**:

- [ ] Packing reads **`spectral_simple`**, and Render gives the same clean
      message.

---

## 11. The native-module bump (#262)

napi went from 3.9.4 to 3.10.2 since 0.2.0, and its fixes are all in how a Rust
error crosses into JS. Nothing here is a feature, but the error path is worth one
deliberate look — §10 is the cheapest place to see it, since it deliberately
raises one.

- [ ] The spectral error in §10 arrives as a **readable message**, not `undefined`,
      `[object Object]`, or an empty panel.
- [ ] The extension host does not crash or need a reload after it.

---

## 12. Regression sweep (fast)

Anything from `samples/README.md`'s checklist you want to spot-check. At minimum:

- [ ] `samples/ecmwf.grib2` (CCSDS) renders.
- [ ] `samples/mrms.grib2` (PNG packing) renders — remember the −999 sentinel, so
      set a manual range like 0..70 to see reflectivity.
- [ ] `samples/eccc.grib2` (rotated lat/lon) unrotates so coastlines land right.

---

## Not manually testable (no action needed)

These changed since 0.2.0 but have no UI surface. They are covered by
eccodes- / h5py-validated tests in CI:

- The GRIB1 bit-reader hardening (a corrupt >32-bit width now errors instead of
  decoding wrong values) — reaching it needs a deliberately corrupt file.
- Per-grid-point geolocation (`#267`) — a Rust API primitive with no UI yet. The
  CSV export that will use it (#244) has not shipped. Its sibling fix (the
  edge-pixel rejection) *is* visible, and is checked in §2.
- The value decode of the implicit / v2-B-tree / single-chunk indexes — see §7
  for why the UI cannot reach it, and what you *can* check.

**Release plumbing** (`#269` crates.io publishing, `#275` dropping the
pre-release channel) is CI-only and is exercised by the release itself. Before
tagging, do the `workflow_dispatch` dry run on `release.yml` rather than
discovering a problem from a tag.

---

## If something looks wrong

Note the **file**, the **projection**, the **colormap**, and what you expected.
A screenshot of the render panel is worth a lot. The most likely place for a real
regression is §2 (coastlines that should *not* have moved).
