# fieldglass-core

Format-agnostic traits and shared types for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files (GRIB1, GRIB2, NetCDF).

This crate is what the format readers are built on. It holds the parsing surface
every format shares:

<!-- parsing-surface: the set of core modules the three format crate libraries
     name, checked by tools/check_parsing_surface.py. The crate documentation
     states it again; both regions have to match the code. -->
`error`, `bits` and `bytes` (bit reading and byte access), `cct_tables` (the WMO
centre tables), `scan` (storage orders), `projection` (map projections and grid
geometry), `lead_time` (the forecast-lead rules both GRIB editions share), and
the three grids that arrive as something other than a rectangle of values —
`sht`, `matrix` and `healpix` — with `global_grid`, the lat/lon grid the first
and last of those are synthesized onto, `array`, the dataset structure
(dimensions, attributes, array descriptions) `fieldglass-netcdf` describes a
file in, and `cf`, the CF conventions read over that structure: which arrays
render, and where a slice of one is placed.
<!-- /parsing-surface -->

On top of that sits an optional viewer layer (warp, overlay, colormap) used by
the rendering front end, and an analysis layer (contours, CSV, field
arithmetic). Three further modules are ungated but are not part of the parsing
surface, because no format crate uses them: format detection, unit conversion,
and the spatial index `cf` builds to place a swath.

The readers themselves are concrete types in their own crates, not
implementations of a trait declared here. What is a trait here is a choice made
at runtime, from a code in the file: where the bytes come from (`ByteSource`),
how a projected grid turns a lat/lon back into a row and column
(`PlanarGridProjector`), and, with `render` on, how a decoded field is warped
onto an output raster (`TargetProjection`, `ForwardMap`).

## Feature flags

- **`render`** *(default)* — the viewer-domain modules (`warp`, `overlay`,
  `colormap`). Depend with `default-features = false` for just the parsing
  surface. `projection` is available either way, since decode-side consumers
  need it.
- **`analysis`** *(default)* — `contour`, `csv`, and `combine`: operations over
  a decoded field that return values rather than pixels. Separate from
  `render`, so a host can draw isolines or export CSV without compiling the
  painter.
- **`serde`** *(default)* — the `Serialize` / `Deserialize` derives on the
  geometry, colour and spatial types. On by default, so nothing changes by
  upgrading; the saving is for a crate that already takes this one with
  `default-features = false` and never serialises what it decodes.
- **`fs`** *(default)* — `detect::detect_format`, which opens a path. Off for a
  target without a filesystem: `wasm32-unknown-unknown` compiles `std::fs` and
  then fails every call at runtime, so the gate is what stops detection from
  silently degrading to a guess from the file extension. Not a `no_std` switch.

## Related crates

- [`fieldglass-grib1`](https://crates.io/crates/fieldglass-grib1) — GRIB edition 1
- [`fieldglass-grib2`](https://crates.io/crates/fieldglass-grib2) — GRIB edition 2
- [`fieldglass-netcdf`](https://crates.io/crates/fieldglass-netcdf) — NetCDF classic and NetCDF-4 / HDF5

## License

Licensed under either of MIT or Apache-2.0 at your option.
