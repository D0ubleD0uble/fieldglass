# fieldglass-core

Format-agnostic traits and shared types for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files (GRIB1, GRIB2, NetCDF).

This crate is the seam the format readers implement. It holds the parsing
surface every format shares — bit reading, format detection, the reader and
metadata traits, error types, and map projections — plus an optional viewer
layer (warp, overlay, colormap) used by the rendering front end.

## Feature flags

- **`render`** *(default)* — the viewer-domain modules (`warp`, `overlay`,
  `colormap`). Depend with `default-features = false` for just the parsing
  surface. `projection` is available either way, since decode-side consumers
  need it.

## Related crates

- [`fieldglass-grib1`](https://crates.io/crates/fieldglass-grib1) — GRIB edition 1
- [`fieldglass-grib2`](https://crates.io/crates/fieldglass-grib2) — GRIB edition 2
- [`fieldglass-netcdf`](https://crates.io/crates/fieldglass-netcdf) — NetCDF classic and NetCDF-4 / HDF5

## License

Licensed under either of MIT or Apache-2.0 at your option.
