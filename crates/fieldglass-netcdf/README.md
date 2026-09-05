# fieldglass-netcdf

NetCDF reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Covers both on-disk layouts end to end:

- **Classic** — CDF-1, CDF-2, and CDF-5.
- **NetCDF-4 / HDF5** — the object header, dataspace, datatype, and dimension
  machinery, with contiguous, compact, and chunked storage (deflate, shuffle,
  fletcher32, and zstd filters).

Reads dimensions, variables, and attributes, resolves dimension scales, and
decodes a variable's values into a `Vec<Option<f64>>` — including CF
`scale_factor` / `add_offset` unpacking and `missing_value` masking. Grid
geometry is returned as the shared types from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core).

## License

Licensed under either of MIT or Apache-2.0 at your option.
