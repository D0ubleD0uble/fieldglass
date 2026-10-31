# fieldglass-netcdf

Reads NetCDF files in pure Rust, both the [classic format](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec) and [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) ([HDF5](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html)):
dimensions, variables, attributes and values. This crate is usable on its
own, and is part of [Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

## What it reads

NetCDF is the format research and climate models, satellites and ocean
analyses most often write. A NetCDF file holds named variables, such as sea
surface temperature, laid out over shared, named dimensions such as time,
latitude and longitude. Attributes on each variable say what it is, its
units, and how its numbers are stored. The format comes in two layouts on
disk. The [classic](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec) one is a simple header followed by the data. [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) is
built on [HDF5](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html), a more general container that can compress data in chunks and
nest variables in groups.

- It reads **both layouts**: classic in all three of its versions ([CDF-1](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec),
  [CDF-2](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#offset_format_spec) and [CDF-5](https://parallel-netcdf.github.io/doc/c-reference/pnetcdf-c/CDF_002d5-file-format-specification.html)), and [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) with contiguous, compact and chunked storage.
- It undoes the **common compression steps** for chunked data: deflate,
  shuffle, the fletcher32 checksum, zstd and szip.
- It reads **every dimension, variable and attribute**, including variables in
  nested groups, and it matches each dimension to the coordinate variable
  that gives its values.
- It gives you **real-world values**. Variables often store packed whole
  numbers with a scale and an offset, plus a value that means "no data". You
  can get the numbers exactly as stored, with only the "no data" points
  masked, or converted back to real units following the [CF conventions](https://cfconventions.org/cf-conventions/cf-conventions.html#packed-data).
- It **places a slice on the Earth** from latitude and longitude axes, from
  the [CF grid mapping](https://cfconventions.org/cf-conventions/cf-conventions.html#grid-mappings-and-projections) of a geostationary satellite view, from the projection a
  WRF model file states, or from a latitude and longitude for every cell, as
  in satellite swaths and ocean grids.

What it doesn't do:

- It doesn't read **every kind of value**. A variable of records with named
  fields, or of values of varying length, is listed as unsupported, and the
  rest of the file still opens.
- It doesn't undo **every compression step**. Any other HDF5 filter, such as
  nbit or scale-offset, is recognized and refused with a clear error, instead
  of returning wrong numbers.
- It doesn't **write** NetCDF. It only reads it.

## Example

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

A fuller version, which also prints each plane's size and value range, is
`examples/decode.rs`. Run it with `cargo run -p fieldglass-netcdf --example
decode` from a checkout of the repository.

Decoding happens in two stages, and you can stop after either.
`decode_variable_raw` returns the numbers as stored, with only the "no data"
points masked. `decode_variable_physical` goes on to apply the variable's
`scale_factor`, `add_offset` and `valid_range`, and `decode_plane` does the
same for one 2-D plane of a variable with more dimensions.

`NetcdfReader::from_bytes` reads a buffer you already hold. To read a file in
pieces instead, open it with `NetcdfReader::from_source` over your own
`ByteSource`. Opening reads the header, listing the variables reads their
metadata, and a plane reads only its own bytes or the chunks it overlaps.

## What it is not

It's a reader for one format. To open GRIB, NetCDF and Zarr through one
interface, or to draw a field as a map, use
[`fieldglass`](https://crates.io/crates/fieldglass).

Like the GRIB crates, it hands back the shared grid type from
`fieldglass-core`: `slice_geometry` places a 2-D slice as a `GridGeometry`.
When a file states its placement more than one way, this crate picks for you,
in this order: coordinates for every cell, then a WRF projection, then a [CF
grid mapping](https://cfconventions.org/cf-conventions/cf-conventions.html#grid-mappings-and-projections), then plain latitude and longitude axes. The `projection`
module's `GeostationaryGrid` and WRF grid types, the error type and the
byte-access types (`ByteRange`, `ByteSource`) are all re-exported from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core), so this crate
can be your only Fieldglass dependency.

## How it is checked

Classic files are checked against what Unidata's `netCDF4` library decodes,
and NetCDF-4 files against `netCDF4` and `h5py`. Real files from NOAA,
including ERSST, OISST and GOES-16, sit beside small built files that each
cover one layout, compression step or damaged structure.

## Related crates

| Crate | Use it to |
|---|---|
| [`fieldglass`](https://crates.io/crates/fieldglass) | Open any supported file and draw it |
| [`fieldglass-zarr`](https://crates.io/crates/fieldglass-zarr) | Read Zarr [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) and [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) stores |
| [`fieldglass-aec`](https://crates.io/crates/fieldglass-aec) | Decode the szip compression this crate uses |
| [`fieldglass-core`](https://crates.io/crates/fieldglass-core) | Name the types every Fieldglass reader shares |

## License

MIT OR Apache-2.0

## AI disclosure

Much of this crate's code, tests and documentation was written with AI
assistance (Anthropic's Claude), under the maintainer's direction and review.

### How do I maintain quality?

I treat the AI as a fast contributor whose work I don't take on trust:

- Fieldglass is designed against the specifications and reference libraries are
  used to check against the resulting behaviors. If behaviors differ between
  Fieldglass and a reference, Fieldglass follows the specification and records
  why.
- Every example comes from as close to real-world files as possible while also
  trying not to bloat the size of our libraries with data. In
  `fieldglass-netcdf`, the files are real NOAA products or are written with
  the `netCDF4` and `h5py` libraries, and each is read back with those
  libraries for comparison or refused safely.
- I take a personal hand in testing each feature and delivery before a new tag
  is released. I maintain a testing plan in which I test each change.
- I have many strict automatic checks for formatting, lints, the test suite,
  dependency and security scans, and fuzzing with damaged or hostile files to
  make sure we refuse them safely.

### Where do I currently notice gaps?

- I dislike the current documentation. There's a lot of it, which would be fine
  if it made the docs more readable but instead it's hard to understand. I am
  undertaking efforts to rewrite the user-facing documentation with my own
  words. This file has been rewritten by me.
- While I have been ensuring proper functionality, behaviors, and
  larger-picture dependency graphs, I haven't been reading over every line of
  code. As the shape of things starts to become a bit more stable, I hope to
  review more of the code closely, but to be totally transparent this is more
  of a 'desire' than a reality currently.
