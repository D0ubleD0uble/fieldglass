# fieldglass-netcdf

NetCDF reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Covers both on-disk layouts end to end:

- **Classic** — CDF-1, CDF-2, and CDF-5.
- **NetCDF-4 / HDF5** — the object header, dataspace, datatype, and dimension
  machinery, with contiguous, compact, and chunked storage (deflate, shuffle,
  fletcher32, and zstd filters).

Reads dimensions, variables, and attributes, resolves dimension scales, and
decodes a variable's values into a `Vec<Option<f64>>`. Decoding is two stages
and the reader offers both composed: `decode_variable_values` returns the raw
on-disk codes with only the fill / missing sentinels masked, and
`decode_variable_physical` (or `decode_plane`, for one 2-D plane of an N-D
variable) applies the CF `scale_factor` / `add_offset` / `valid_range`
mask-and-scale on top of them from the variable's own attributes.

Grid geometry comes back as this crate's own types — `SliceGeometry` for a
resolved 2-D slice, and the `projection` module's `GeostationaryGrid` and WRF
grids for the files that state a projection rather than 1-D lat/lon axes. Unlike
the GRIB crates, none of them is `fieldglass_core::GridGeometry`: the precedence
between them (curvilinear coordinates, WRF `MAP_PROJ`, CF `grid_mapping`, 1-D
axes) is still resolved in the render host rather than here, so those modules
stay leaves that name no shared geometry type. What *is* shared is the error type
and the byte-access seam (`ByteRange`, `ByteSource`), re-exported from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core) so this crate can
be the only Fieldglass dependency in a consumer's manifest.

## Usage

```rust
use fieldglass_netcdf::NetcdfReader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any NetCDF file's bytes, classic or NetCDF-4 — nothing below names which
    // layout it got. This one is committed with the crate, so the snippet runs
    // as written from a checkout.
    let bytes = std::fs::read("tests/fixtures/oisst_avhrr_v2.nc")?;
    let reader = NetcdfReader::from_bytes(bytes)?;
    let view = reader.view()?;

    for renderable in view.renderable_variables() {
        // `None` where the CF conventions do not name the horizontal axes (a
        // WRF field is the usual case); then the caller picks them.
        let (Some(y_dim), Some(x_dim)) = (renderable.detected_y_dim, renderable.detected_x_dim)
        else {
            continue;
        };
        let var = view.var(renderable.decode_index).expect("a variable in this view");

        // Decode, pick the plane, then CF mask-and-scale — in that order, which
        // is why it is one call. `fixed` holds one index per declared
        // dimension; the entries for `y_dim` and `x_dim` are ignored, so zero
        // picks the first time step, the surface level, and so on.
        let fixed = vec![0usize; renderable.dims.len()];
        let plane: Vec<Option<f64>> = reader.decode_plane(var, y_dim, x_dim, &fixed)?;
        println!("{}: {} values in physical units", renderable.name, plane.len());
    }

    Ok(())
}
```

`cargo run -p fieldglass-netcdf --example decode` runs that against the fixtures
committed with the crate.

## License

Licensed under either of MIT or Apache-2.0 at your option.
