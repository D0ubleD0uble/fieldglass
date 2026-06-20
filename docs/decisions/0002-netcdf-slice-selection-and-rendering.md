# 0002 — NetCDF slice selection and 2-D rendering

**Status:** Accepted (2026-06-20). Resolves the [#112](https://github.com/D0ubleD0uble/fieldglass/issues/112)
research spike and scopes the [#122](https://github.com/D0ubleD0uble/fieldglass/issues/122)
implementation.

## Context

The render pipeline assumes a single 2-D field. That holds for GRIB, where a
message is inherently one 2-D grid. NetCDF variables are routinely 3-D or 4-D —
`time × level × lat × lon` — so before NetCDF can render we need two things the
GRIB path never required:

1. a way to pick which 2-D slice to draw out of an N-D variable, and
2. grid geometry, because NetCDF carries no GDS / projection metadata the way
   GRIB does.

The value-decode half is already in place. `decode_netcdf_variable(bytes, index)`
returns a `DecodedVariable { values, mask, shape }` — the full row-major (C-order)
array plus its dimension lengths — for both classic (CDF-1/2/5) and NetCDF-4 /
HDF5 backings. `open_netcdf` returns `DatasetMeta` with per-variable dimension
names and attributes for the **classic** backing. This note decides how to turn
that into a rendered image, and it deliberately reuses the existing warp rather
than building a NetCDF-specific one.

The spike asked four questions: axis identification, slice-picker UX,
render-pipeline reuse, and geolocation. They are answered in turn, then the
decision, the sequencing, and the implementation contract follow.

## Q1 — Axis identification (which dims are lat / lon)

Detect the horizontal axes from **CF conventions** on the coordinate variables,
not from dimension order or position.

A *coordinate variable* is a 1-D variable whose name equals one of its
dimension's names (e.g. a `lat(lat)` variable). CF identifies its axis type, in
priority order:

1. `units` — the primary signal. Latitude is any accepted spelling of
   `degrees_north`; longitude is any accepted spelling of `degrees_east`
   (`degree_north`, `degreeN`, `degrees_N`, …). This is the canonical CF test
   and is sufficient for the overwhelming majority of operational files.
2. `standard_name` of `latitude` / `longitude`, and/or `axis` of `Y` / `X` —
   accepted as a direct alternative and as a tie-breaker. These are present in
   CF-compliant files and let us avoid parsing `units` when they are richer.

The remaining (non-horizontal) dimensions — typically `time` and a vertical
level — are the ones the slice picker indexes. We do not need to fully classify
them to render; anything that is not the chosen lat and lon axis is an
"index me" dimension. We surface their names (and, where present, `time` /
vertical `units` and `standard_name`) only to label the controls.

**First pass is 1-D coordinate variables only.** CF also allows *2-D*
(curvilinear) latitude/longitude coordinate variables — a variable carrying a
`coordinates` attribute that points at 2-D `lat(y,x)` / `lon(y,x)` auxiliary
arrays (WRF `XLAT`/`XLONG`, GOES fixed-grid, ocean-model tripolar grids). Those
are not a regular lat/lon grid and cannot feed the corner-coordinate warp; they
are explicitly a **second pass** (see *Out of scope*).

**Fallbacks, in order**, when CF metadata is absent or ambiguous:

- Name heuristic on the dimensions/variables (`lat`/`latitude`, `lon`/
  `longitude`, `x`/`y`) — a best-effort guess, clearly marked as assumed.
- Explicit user override in the picker (two "which dim is X / Y" selectors,
  pre-filled with the detected axes). This is always available, so a file that
  defeats detection is still renderable by hand.

## Q2 — Slice-picker UX

A NetCDF document does not map to "one message"; it maps to "a dataset of
variables, most of which are N-D". The picker therefore has two tiers:

1. **Variable selector.** List the *renderable* variables — numeric (not
   `char`/string), at least 2-D, and with at least two axes that resolve (or can
   be overridden) to horizontal. Coordinate variables and 1-D series are
   excluded from the render list but still available in the metadata viewer.
2. **Per-dimension index controls.** For the selected variable, render one
   control per non-horizontal dimension: a labelled slider + numeric input over
   `0..len-1`, defaulting to index 0 (for an unlimited/record `time`, default to
   the last index — the most recent step is the common intent). The two
   horizontal dimensions are shown as "image X / Y" pre-filled with the detected
   axes (Q1) and stay user-overridable from the same selectors, so a file that
   defeats detection is still renderable by hand.

Selecting a variable or moving any index posts the existing render request with
the resolved 2-D slice; the panel repaints. This reuses the established
render-panel message loop (`rerenderRequest` → `gridReady`) rather than adding a
new transport.

Slicing itself is cheap and stays on the JS side at first: `DecodedVariable`
already hands back the whole array, so a slice is a strided copy of one
`lat × lon` plane out of the C-order buffer at the chosen higher-dimension
indices. (A later optimisation can push slice-on-decode into Rust for very large
variables — noted under *Consequences*, not required for #122.)

## Q3 — Render-pipeline reuse

**Reuse the existing warp. Do not build a NetCDF-specific render path.**

The warp pipeline is keyed entirely off `MessageMeta` geometry — `grid_type`,
`grid_ni`/`grid_nj`, and the corner coordinates `lat_first`/`lon_first`/
`lat_last`/`lon_last` — plus a flat `&[Option<f64>]` field. A regular NetCDF
lat/lon grid is exactly a GRIB `"latlon"` grid with no projection. So the
NetCDF path **synthesises a `MessageMeta` with `grid_type = "latlon"`** from the
coordinate variables (Q4) and feeds it through the same `render_with_options`
that GRIB uses. Source projection and equirectangular both work immediately;
Web Mercator / orthographic / polar-stereographic come along for free because
`grid_is_reprojectable("latlon", …)` is already `true`.

This keeps the decode-decoupled invariant (`.claude/rules/conventions.md`): the
new path produces a `Vec<Option<f64>>` + geometry and changes nothing in
projection, overlay, warp, or colormap.

The one new surface is the napi entry. Two viable shapes:

- **(A) A `render_netcdf_slice` free function** taking `(bytes, variable_index,
  slice_indices, axis_assignment, RenderOptions)` and returning the existing
  `RenderedGrid`. Stateless, mirrors `decode_netcdf_variable`.
- **(B) A `NetcdfHandle`** mirroring `Grib1Handle`/`Grib2Handle`, caching the
  parsed reader and decoded variable across re-renders (slider drags re-render
  often; re-decoding a multi-hundred-MB variable per frame is wasteful).

**Recommendation: (B)**, for the same caching reason the GRIB handles exist
(see `cached_decode`). A slider drag should re-slice a cached decode, not
re-parse the file. The handle owns: the parsed `NetcdfReader`, an `Arc` cache of
the last decoded variable, and the synthesised `MessageMeta`. `project_overlay`
then works through the same handle, so coastlines/graticule render on NetCDF
exactly as on GRIB.

## Q4 — Geolocation

Derive grid bounds from the **1-D lat/lon coordinate variables**:

- `grid_ni = len(lon)`, `grid_nj = len(lat)`.
- `lon_first/lon_last` = first/last value of the lon coordinate array;
  `lat_first/lat_last` = first/last of the lat array. The existing warp already
  handles descending latitude (north-to-south, the common storage order) and an
  antimeridian-crossing or 0–360 longitude window via the corner coordinates and
  `flip_y`, so no special-casing is needed here.

**Regular-spacing assumption.** The synthesised `"latlon"` geometry implies
uniform spacing between corners. Most operational reanalysis/model grids (ERA5,
MERRA-2, CMIP6, ERSST) are regular, so corner-to-corner linear mapping is exact
for them. When a coordinate array is detectably *irregular* (non-uniform deltas
beyond a small tolerance — e.g. a Gaussian latitude axis or a stretched vertical
collapsed onto lat), the first pass still renders via the corner mapping but the
panel flags "irregular spacing — geolocation approximate". Exact handling of
irregular 1-D axes (true Gaussian rows already have a Gaussian inverse map;
arbitrary spacing would need a per-row lookup) is a follow-up, not a blocker.

**No coordinate variables at all** → assume a regular grid over the dimension
extents with a clearly-marked "assumed grid, no geolocation" note, rendering in
source projection only (no map reprojection offered, since bounds are unknown).

Coordinate arrays are fetched with the **existing** `decode_*` surface — a lat or
lon coordinate variable is just another variable index — so Q4 needs no new
decode API, only the geometry synthesis.

## Decision

Build NetCDF 2-D rendering as a thin adapter in front of the existing warp:

1. **Axis detection** by CF `units` → `standard_name`/`axis` → name heuristic →
   user override, over 1-D coordinate variables.
2. **A two-tier picker** (variable selector + per-non-horizontal-dim index
   controls) driving the existing render-panel message loop.
3. **A `NetcdfHandle`** napi class that parses once, caches the decoded
   variable, synthesises a `"latlon"` `MessageMeta` from the coordinate
   variables, and calls the shared `render_with_options` / `project_overlay`.
4. **Regular 1-D lat/lon grids only** in the first pass; curvilinear (2-D
   lat/lon) and projected fixed grids are a tracked second pass.

This is the smallest change that gets real files on screen while preserving the
no-native-deps, decode-decoupled architecture.

## Sequencing and dependencies

**Classic NetCDF renders first; NetCDF-4 / HDF5 follows
[#33](https://github.com/D0ubleD0uble/fieldglass/issues/33).** The picker needs
dimension names, lengths, and the coordinate variables' CF attributes.
`open_netcdf` returns all of that for the **classic** backing today, but the
HDF5 backing still reports `fully_parsed = false` with empty
dimensions/variables/attributes — deep HDF5 metadata (variables, dimensions,
attributes) is #33, which is open. HDF5 *value* decode works, but without the
metadata the picker has nothing to drive.

This refines #122's stated validation set:

| #122 fixture | Backing | Grid | First pass? |
| --- | --- | --- | --- |
| ERA5 | NetCDF-4 / HDF5 | regular 1-D lat/lon | needs #33 metadata |
| MERRA-2 | NetCDF-4 / HDF5 | regular 1-D lat/lon | needs #33 metadata |
| CMIP6 | NetCDF-4 / HDF5 | regular 1-D lat/lon | needs #33 metadata |
| ERSST (already a fixture) | classic CDF | regular 1-D lat/lon | **yes** |
| WRF `wrfout` | classic CDF | **2-D curvilinear (Lambert)** | no — second pass |

So #122's "4-D field with a slice picker" is best demonstrated on a **classic**
4-D file (and the committed ERSST classic fixture is the zero-cost first target),
not on ERA5/MERRA-2, which are blocked on #33. And WRF `wrfout`, listed in #122,
is **curvilinear** (2-D `XLAT`/`XLONG`, a Lambert grid) — it is not a regular
1-D lat/lon grid and belongs to the curvilinear second pass, not #122.

## Implementation contract for #122

- Add `NetcdfHandle` to `crates/fieldglass-napi/src/lib.rs` (factory
  `from_bytes`; methods `variables()` → renderable `VariableMeta`,
  `render_slice(variable_index, slice_indices, axis_assignment, options)` →
  `RenderedGrid`, `project_overlay(...)`). Caches the parsed reader + last
  decode like the GRIB handles.
- Axis detection + `MessageMeta` synthesis live in a small helper in the napi
  crate (or `fieldglass-core` if it grows test surface). Cross-check axis
  detection against a table of CF `units` spellings.
- Extension: extend the render panel with the two-tier picker; reuse the
  `rerenderRequest`/`gridReady` loop and the overlay path unchanged.
- Validation: render the committed **classic** ERSST fixture and one
  multi-dimensional classic fixture end-to-end; assert the synthesised geometry
  against the coordinate arrays. Unit-test axis detection across CF `units` /
  `standard_name` / `axis` / name-heuristic / override cases.
- Feature matrix: flip NetCDF **"2-D grid rendering with colormap"** from
  `❌ Not yet` to a classic-scoped ✅ **only when #122 lands** — not in this
  note.

## Out of scope (tracked separately)

- **Curvilinear / 2-D coordinate grids** (WRF `wrfout`, GOES fixed-grid, ocean
  tripolar). These need a scattered-point or per-cell geolocation path, not the
  corner-coordinate warp. Recommend a dedicated follow-up issue; do not fold
  into #122.
- **NetCDF-4 / HDF5 rendering** — unblocked by #33; the `NetcdfHandle` from this
  decision serves both backings, so it is "render HDF5 once its metadata is
  exposed", not a separate render design.
- **Slice-on-decode in Rust** for very large variables — an optimisation over
  the JS-side strided copy; only worth it if a fixture proves the full-array
  decode is a problem.

## Consequences

- The warp, projection, overlay, and colormap code is untouched: NetCDF reaches
  the screen through the same `MessageMeta` + `Vec<Option<f64>>` seam as GRIB,
  honouring the decode-decoupled rule.
- #122 is scoped to **classic, regular 1-D lat/lon** rendering with a working
  slice picker, with its validation set corrected (ERSST classic as the first
  target; ERA5/MERRA-2 deferred behind #33; WRF moved to the curvilinear
  follow-up).
- Two follow-ups are implied and should be filed (drafted for review, not filed
  automatically per project convention): curvilinear/2-D-coordinate rendering,
  and HDF5 rendering once #33 lands.

## References

- Spike: [#112](https://github.com/D0ubleD0uble/fieldglass/issues/112).
  Implementation: [#122](https://github.com/D0ubleD0uble/fieldglass/issues/122).
  HDF5 metadata dependency: [#33](https://github.com/D0ubleD0uble/fieldglass/issues/33).
- CF Conventions, Ch. 4 (Coordinate Types) and Ch. 5 (Coordinate Systems):
  <https://cfconventions.org/cf-conventions/cf-conventions.html>
- Prior decision in the same shape (reuse / no native dep over a bespoke path):
  [`0001-grib2-compressed-packing-codecs.md`](0001-grib2-compressed-packing-codecs.md).
