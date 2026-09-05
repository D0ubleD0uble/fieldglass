# How a grid reaches the renderer, and what a new one costs

*Verified against the code 2026-08-23.*

## The core seam

The `fieldglass-core` render pipeline is **inverse-mapped**. `core::warp::warp`
walks *output* pixels, asks `SourceGrid::inverse_at(lat, lon)` for a source
index, and samples there. `SourceGrid` abstracts both halves, so at the core
layer a new grid is a sampler plus an inverse.

Three patterns get a grid into that shape. Which one a template needs is what
determines its cost:

- **Analytic inverse.** A closed-form `(lat, lon) → index`: regular lat/lon,
  Mercator, Lambert, polar stereographic, Gaussian, space view. A formula and a
  match arm.
- **Synthesize at decode.** The message is resampled onto a regular lat/lon
  grid once, at the decode seam, and everything downstream treats it as an
  ordinary lat/lon field. Spectral does this
  (`napi::spectral_render_meta_from`); reduced grids do a cheaper version
  (`expand_reduced_to_regular`). Costs a synthesis-resolution decision, and the
  result is resampled rather than exact.
- **Lookup inverse.** The grid carries an explicit coordinate list, so "which
  cell contains this point?" means searching millions of cells. ICON (3.101),
  NCEP curvilinear (3.204), and NetCDF 2-D coordinate grids (#218) are all
  this. Costs a spatial index behind `inverse_at`.

## Where the cost actually lands: the napi layer

Core needs no structural change for any of the three. The napi layer is less
abstract, and every new grid pays there:

- `build_warp_target` (`crates/fieldglass-napi/src/lib.rs`) derives *every*
  target raster dimension from `ni`/`nj`: equirectangular is `ni × nj`,
  orthographic and polar stereographic are `ni.max(nj)` square, world
  projections go through `world_raster_dims(ni, nj)`. A grid whose `(ni, nj)`
  is not a raster shape needs its own target-dims rule.
- Reprojection eligibility *was* a grid-type string allow-list here
  (`grid_is_reprojectable`, `gate_planar_reprojection`,
  `source_grid_is_periodic`), so each new grid touched several sites. #571 made
  it one property of the geometry — `GridGeometry::reprojectable`,
  `is_periodic_x`, `contour_seam_wraps`, `render_window` — and napi keeps only
  the mapping from its DTO to that type (`meta_geometry`). A new grid family now
  adds arms in `core` and none here.
- `GridGeometry` is ~30 scalars *and* is the render-cache key. A grid that
  needs coordinate arrays or an index handle changes the cache key and all
  three `warp_setup_for` callers (`warp_message`, `project_overlay_impl`,
  `probe_impl`).
- Resampling is user-selected and passed through verbatim; there is no
  per-grid override. A grid that must force nearest-neighbour needs a hook and
  a greyed picker.
- The extension's `messageIsRenderable` is `(gridNi && gridNj) || spectral`
  and the `Ni × Nj` caption assumes a raster; each non-raster grid needs a
  special case, as spectral already has.

Contours and long-format CSV are gated on the grid-type family, so an
unsupported family gets the existing "not supported" path rather than wrong
output. That is a property of the gate, not of the grid shape; do not rely on
`nj = 1` behaving.

## HEALPix (3.150)

`ang2pix` gives a closed-form inverse, so HEALPix looks analytic. But
`(ni, nj) = (Npix, 1)` is not a raster shape: equirectangular would be a
one-row strip and orthographic an `Npix × Npix` allocation (about 1.6e14
pixels at Nside 1024), and the default first render is the "source" view,
which paints `ni × nj` directly.

Follow the spectral precedent instead: synthesize onto a lat/lon grid at
decode, with a resolution rule derived from Nside (spectral pins 0.5°,
720×361). Downstream then gets a real lat/lon grid, so contours, probe, and
CSV work rather than degrade. Also needs NESTED↔RING handling. eccodes decodes
HEALPix since 2.32.0, so the 2.34.1 pin is a valid oracle. Size: weeks, not
days.

## The spatial-index seam

One capability, three consumers. A k-d tree, BVH, or HEALPix-binned bucket
lookup behind the existing `SourceGrid` interface, plus the napi plumbing
above, paid once by the first consumer.

Sequence so the cheapest consumer validates the seam:

1. **NetCDF curvilinear grids (#218).** Coordinates arrive in-band, so there
   is no acquisition policy to design, and the index can be validated against
   real ocean tripolar and satellite-swath files.
2. **GRIB2 3.204 curvilinear (NCEP local).** Same shape, GRIB packaging.
   RTOFS ocean data. eccodes lacks this template entirely.
3. **GRIB2 3.101 general unstructured (ICON).** Needs an ADR first.

### Why ICON needs an ADR before code

§3.101 carries only a grid UUID and a point count, never the coordinates,
because the mesh is identical across every message. Geometry comes
out-of-band: an ICON grid NetCDF matched by UUID from
icon-downloads.mpimet.mpg.de, or `CLAT`/`CLON` companion GRIBs from
opendata.dwd.de. The ADR must settle UUID resolution, cache location, download
versus user-pointed companion file, and graceful failure when the mesh is
unavailable. The decode seam itself is additive: core surfaces the geometry
*reference*, rendering accepts coordinate arrays as input, and resolution
policy lives in the host adapter.

The first two consumers validate the index and the napi plumbing, which is
the smaller half of ICON. UUID resolution, mesh caching, the companion-file
policy, the host-adapter surface, and the extension's renderability check for
a message whose mesh is not yet resolved are exercised by neither. ICON stays
a substantial effort after the seam lands.

Deferred within this seam: 3.32768/3.32769 rotated Arakawa E/B (legacy NAM
native; archive only).
