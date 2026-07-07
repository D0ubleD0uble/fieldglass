# 0004 — NetCDF projected-grid geolocation (WRF, GOES) and the geostationary projector

**Status:** Accepted (2026-06-20). Scopes
[#168](https://github.com/D0ubleD0uble/fieldglass/issues/168) and resolves the
source-raster-vs-full-geo question left open after decision
[0002](0002-netcdf-slice-selection-and-rendering.md).

**Amended (2026-07-04):** The non-Lambert WRF `MAP_PROJ` variants listed under
"Out of scope / deferred" shipped in
[#220](https://github.com/D0ubleD0uble/fieldglass/issues/220): polar
stereographic (`MAP_PROJ = 2`) and Mercator (`3`) now resolve through the same
global-attribute reader onto the existing projectors.

**Amended (2026-07-06):** *Unrotated* lat-lon (`MAP_PROJ = 6`, `POLE_LAT = 90`)
shipped in [#226](https://github.com/D0ubleD0uble/fieldglass/issues/226). An
unrotated domain is a plain rectilinear geographic grid — WRF's Cassini
transform reduces to `olat = rlat`, `olon = rlon − const` when the computational
pole coincides with the geographic pole — so it is corner-pinned from the true
`XLAT`/`XLONG` ends onto the existing lat/lon projector, exactly like the
Mercator variant (no 1-D coordinate variables needed). *Rotated* lat-lon
(`POLE_LAT != 90`) stays deferred to source projection: there is no cleanly
documented mapping from WRF's `(POLE_LAT, POLE_LON, STAND_LON)` onto the GRIB2
§3.1 rotated-pole convention (`STAND_LON` and `POLE_LON` are folded into one
longitude offset, the pole-parameter definitions are inconsistent between WRF's
code and docs, and WPS has a known double-`stand_lon` rotation bug), so
synthesising a rotated-pole grid would risk mis-georeferencing. A rotated domain
whose 2-D `XLAT`/`XLONG` arrays are used *directly* is the Model-B curvilinear
path ([#218](https://github.com/D0ubleD0uble/fieldglass/issues/218)).

## Context

Decision 0002 renders NetCDF variables on a **regular 1-D lat/lon** grid by
synthesising a `"latlon"` `MessageMeta` and riding the existing warp. Two 0.2.0
corpus models don't fit that: **WRF `wrfout`** and **GOES** ABI imagery. 0002
deferred them as "curvilinear / 2-D coordinate" grids and #168 framed the choice
as "CF `grid_mapping` → known projection" vs "generic 2-D lat/lon forward
scatter."

Researching the two formats changes that framing. Neither corpus model is
actually an irregular grid:

- **GOES** ABI fixed grid is a **regular grid in geostationary scan-angle
  space** — 1-D `x` / `y` coordinate variables in *radians*, plus a CF
  `grid_mapping` variable `goes_imager_projection` with
  `grid_mapping_name = "geostationary"`
  ([NOAA STAR, GOES Imager Projection](https://www.star.nesdis.noaa.gov/atmospheric-composition-training/satellite_data_goes_imager_projection.php)).
  It carries *no* 2-D lat/lon arrays; lat/lon are derived analytically from the
  scan angles.
- **WRF `wrfout`** is a **regular grid in Lambert projected space** — constant
  `DX` / `DY` in metres, with the projection in WRF *global attributes*
  (`MAP_PROJ`, `TRUELAT1` / `TRUELAT2`, `STAND_LON`, `MOAD_CEN_LAT`), spherical
  Earth
  ([Maussion, "Map projections in WRF"](https://fabienmaussion.info/2018/01/06/wrf-projection/)).
  The 2-D `XLAT` / `XLONG` variables are precomputed conveniences (and a
  verification oracle), not the primary geolocation.

So both are **regular grids in a projected coordinate reference system**, which
is exactly what the analytic-inverse warp seam already handles for GRIB. They do
not need the irregular-scatter path. That is the central decision here.

## Two geolocation models

- **Model A — analytic projection inverse (regular grid in a projected CRS).**
  Reconstruct the projection's `*Params`, synthesise a `MessageMeta` with the
  right `grid_type`, and reuse the existing warp — its inverse map
  `(lat, lon) → GridIndex` is the projection's analytic inverse. This is how
  every GRIB projected grid already renders (`warp_setup_for` dispatch).
- **Model B — irregular 2-D coordinate scatter.** A grid with genuine 2-D
  lat/lon arrays and *no* global analytic projection (ocean tripolar, some
  swath products). There is no closed-form inverse; geolocation must come from
  the coordinate arrays themselves (forward scatter into the target raster, or a
  built spatial index for inverse nearest-neighbour). This does **not** fit the
  analytic seam and needs a different mechanism.

**Decision: 0.2.0 uses Model A for both corpus models.** WRF and GOES are
regular-in-projection, so neither needs Model B. Model B has **no 0.2.0 corpus
model** (the corpus contains no tripolar/swath file) and is deferred.

## Resolving the projection (two metadata paths into Model A)

1. **CF `grid_mapping` variable** — the standard path. A data variable names a
   `grid_mapping` variable whose `grid_mapping_name` + parameters define the
   CRS. This generalises beyond GOES: `lambert_conformal_conic`,
   `polar_stereographic`, `mercator`, `latitude_longitude`, and
   `rotated_latitude_longitude` map onto projectors the warp **already has** —
   only `geostationary` needs a new one. So a small CF-grid-mapping → `*Params`
   translator covers most CF-compliant projected NetCDF, not just GOES.
2. **WRF global attributes** — WRF output is not CF-compliant; the projection is
   in global attributes. A WRF-specific reader maps `MAP_PROJ = 1` (Lambert) to
   `LambertParams` (`TRUELAT1/2 → latin1/2`, `STAND_LON → lov`,
   `MOAD_CEN_LAT → lad`, `DX/DY` metres), with the grid origin taken from
   `XLAT`/`XLONG` corner cells. WRF's spherical Earth matches the existing
   Lambert projector's `EARTH_RADIUS_M`, so no ellipsoid work is needed. (WRF
   also supports `MAP_PROJ` 2 polar-stereo / 3 Mercator / 6 lat-lon — all
   existing projectors — but the corpus `wrfout` is Lambert; others are a
   cheap extension, not required.)

Both paths terminate in the **same** Model-A synthesis: build `*Params`, set
`grid_type`, reuse the warp. Source projection remains the universal fallback
when no projection is resolved or recognised.

## The one new projector: geostationary / space view

GOES is the only corpus model needing a projector the codebase lacks. Add a
**geostationary (space-view perspective) projector** to `fieldglass-core`,
alongside Lambert / polar-stereo / Mercator:

- **Parameters** (from the CF `geostationary` grid mapping): perspective point
  height `H` (`perspective_point_height`), semi-major / semi-minor axes
  (`semi_major_axis` / `semi_minor_axis` — GOES is ellipsoidal, unlike the
  spherical projectors), `longitude_of_projection_origin` (sub-satellite
  longitude), and `sweep_angle_axis` (`x` for GOES-R, `y` for Meteosat — it
  swaps the two scan-angle rotations).
- **Inverse** `(lat, lon) → (scan x, y in radians) → GridIndex`, using the
  GOES-R fixed-grid algorithm
  ([GOES-R PUG Vol. 3; NOAA STAR](https://www.star.nesdis.noaa.gov/atmospheric-composition-training/satellite_data_goes_imager_projection.php)),
  with off-disk points (no Earth intersection) returning `None` so the limb
  renders transparent.
- The 1-D `x` / `y` radian coordinate variables give the grid extent and
  spacing (the regular-grid corners in scan-angle space).

**This projector also makes GRIB2 §3.90 (space view perspective) reprojectable.**
That template is already parsed (`SpaceViewTemplate` in `grib2/src/gds.rs`) but
`grid_is_reprojectable("space_view")` is `false` today and there is no projector
for it. Putting the projector in `fieldglass-core` lets **both** GOES NetCDF and
GRIB2 3.90 reproject from one implementation — the same decode-decoupled reuse
that motivated 0002.

**Recommendation: split the geostationary projector into its own issue** under
both GRIB2 and NetCDF, since it serves both formats and is the substantive new
math. #168 then depends on it for GOES and otherwise reduces to metadata
reconstruction + wiring.

## Architecture

- `fieldglass-core/src/projection.rs` — new `GeostationaryParams` +
  `GeostationaryProjector` (cached constants, `inverse(lat, lon) → Option<GridIndex>`),
  matching the existing projector pattern.
- `MessageMeta` — a `"geostationary"` `grid_type` plus a `geos_*` parameter
  group (sub-satellite lon, `H`, semi-major/minor, sweep axis, and the x/y
  radian extent), mirroring the existing `lambert_*` / `polar_stereo_*` groups.
- `napi/src/lib.rs` — a `geostationary_warp_setup`, registered in
  `warp_setup_for` and `grid_is_reprojectable`; both GRIB2 3.90 and the NetCDF
  path populate the `geos_*` fields, then share it.
- NetCDF side (the `NetcdfHandle` from 0002/#122): a **CF `grid_mapping`
  reader** and a **WRF global-attribute reader** that emit the appropriate
  `*Params` / `MessageMeta`. These consume the coordinate variables (1-D `x`/`y`
  for GOES; `DX`/`DY` + `XLAT`/`XLONG` corners for WRF).

No change to warp, overlay, colormap, or the render panel: the decode-decoupled
seam holds — projected NetCDF reaches the screen through the same
`MessageMeta` + `Vec<Option<f64>>` path as GRIB.

## Scope and guardrails

- **In scope (0.2.0):** GOES via CF `geostationary` grid mapping + the new
  projector; WRF `wrfout` Lambert via the WRF global-attribute reader reusing
  the existing Lambert projector. Both render **full geo-reprojection** (and
  every existing map target), with source projection as the fallback.
- **Out of scope / deferred:** Model B (irregular 2-D coordinate scatter —
  tripolar/swath), no corpus model; the non-Lambert WRF `MAP_PROJ` variants
  (cheap follow-ups); `_Netcdf4Coordinates` 2-D coordinate variables.
- **Guardrails:** an unrecognised `grid_mapping_name`, a missing required
  parameter, or an off-disk-everywhere geometry resolves to **source projection
  with a clear note**, never a blank or mis-georeferenced map. Off-disk pixels
  invert to `None` (transparent limb).

## Validation

- **Geostationary projector:** cross-check the inverse against worked GOES-R PUG
  examples and against a GOES fixture's own implied geolocation (sub-satellite
  point, a known landmark pixel). Reuse the GRIB2 snapshot harness for a §3.90
  fixture if one is available; otherwise the GOES NetCDF file is the oracle.
- **WRF:** the file's own `XLAT` / `XLONG` arrays are ground truth — assert the
  synthesised Lambert grid reproduces them within tolerance at sampled cells.
- **NetCDF validation** follows the netCDF4-python discipline from decision
  0003; provenance in the fixtures `NOTICE.md`.

## Consequences

- #168 is re-scoped from "curvilinear scatter" to **Model-A projected-grid
  reconstruction** for WRF + GOES, with Model B explicitly deferred. Its body
  should be updated to match.
- A new shared **geostationary-projector** issue is warranted (GRIB2 3.90 +
  GOES); #168's GOES path depends on it.
- The source-raster-vs-full-geo question is settled: Model A makes full geo
  cheap (WRF reuses Lambert; GOES is one projector that also pays off for GRIB2),
  so 0.2.0 does full geo rather than settling for un-georeferenced rasters.

## References

- Builds on decisions [0002](0002-netcdf-slice-selection-and-rendering.md)
  (slice render + warp reuse) and
  [0003](0003-netcdf4-dimension-scale-resolution.md) (HDF5 dimensions, which
  GOES needs to expose its `x`/`y`/`grid_mapping`).
- GOES Imager Projection / ABI fixed grid, NOAA STAR:
  <https://www.star.nesdis.noaa.gov/atmospheric-composition-training/satellite_data_goes_imager_projection.php>
- GOES-R fixed-grid lat/lon algorithm (worked example):
  <https://makersportal.com/blog/2018/11/25/goes-r-satellite-latitude-and-longitude-grid-projection-algorithm>
- WRF projection parameters: <https://fabienmaussion.info/2018/01/06/wrf-projection/>
- CF Conventions §5.6 Grid Mappings:
  <https://cfconventions.org/cf-conventions/cf-conventions.html>
