# fieldglass-core

Format-agnostic traits and shared types for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files (GRIB1, GRIB2, NetCDF).

This crate is what the format readers are built on. It holds the parsing surface
every format shares — bit reading, format detection, message metadata, the WMO
centre tables, error types, and map projections — plus an optional viewer layer
(warp, overlay, colormap) used by the rendering front end.

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
