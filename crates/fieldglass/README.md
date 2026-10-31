# fieldglass

Weather and climate data comes in a few common formats: GRIB from forecast
centers, NetCDF from research and climate models, and Zarr for large datasets
kept in the cloud. `fieldglass` opens any of them in pure Rust and answers the
same questions the same way whatever the format: what is in the file, what are
the values, where on the Earth do they sit, and what do they look like as a
map. It is the library behind the Fieldglass VS Code extension and the
`@fieldglass/wasm` browser package, and is part of
[Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

## What it reads

A weather file holds one or more fields: a grid of values such as temperature
or wind speed over a region, for one time and one height. GRIB files hold a
list of fields, called messages. NetCDF files and Zarr stores hold named
variables, which can have extra axes such as time or height that you step
through.

| Format | Read by | Notes |
|---|---|---|
| [GRIB edition 1](https://www.nco.ncep.noaa.gov/pmb/docs/on388/) | [`fieldglass-grib1`](https://crates.io/crates/fieldglass-grib1) | Metadata for every message, and values for every packing it reads |
| [GRIB edition 2](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/) | [`fieldglass-grib2`](https://crates.io/crates/fieldglass-grib2) | See the [packing table](https://github.com/D0ubleD0uble/fieldglass#grib2-packing-modes) for what decodes |
| NetCDF [classic](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec) and [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) ([HDF5](https://support.hdfgroup.org/documentation/hdf5/latest/_f_m_t3.html)) | [`fieldglass-netcdf`](https://crates.io/crates/fieldglass-netcdf) | Including compressed variables and groups |
| Zarr [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) and [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) | [`fieldglass-zarr`](https://crates.io/crates/fieldglass-zarr) | From the bytes of a store you hand it |

- It **places each field on the Earth**, for every common grid its format
  crate models: regular latitude and longitude, Gaussian, Mercator, rotated
  latitude and longitude, Lambert conformal, polar stereographic, transverse
  Mercator, Lambert azimuthal equal-area, geostationary satellite views, and
  grids given a latitude and longitude for every cell, such as satellite swaths
  and ocean models. GRIB1's Mercator isn't one of them.
- It gives you **real-world values**, with packed numbers converted to their
  units and missing cells marked as missing.
- It **draws maps**: it reprojects a field onto a map, colors it, and draws
  contour lines, wind arrows and lines you give it, such as coastlines, on top.
- It **compares fields**, such as the difference between two forecasts.
- It **reads the value at a point**, and exports values as CSV.

What it doesn't do:

- It doesn't **fetch** anything. You read the file or the store's objects and
  hand it the bytes, so the same code works with a local file, a download, or
  a browser.
- It doesn't **write** or edit files. It only reads them.
- It doesn't **decode every format variant**. Anything it can't read is
  refused with a reason, instead of returning wrong numbers.

### Choosing a crate

Use `fieldglass` when you want to open a file of any supported format and work
with its fields, or draw them. Use one of the format crates above when you only
need that one format's full detail, without the rest.

## Examples

Run each example with `cargo run -p fieldglass --example <name>` from a checkout
of the repository. Each one reads a small real forecast file that's committed
in the repository.

| Example | What it answers |
|---|---|
| [`what_is_in_this_file`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass/examples/what_is_in_this_file.rs) | What fields or variables are in this GRIB or NetCDF file? Start here. |
| [`what_was_the_value_here`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass/examples/what_was_the_value_here.rs) | What is the value at one latitude and longitude, with its units? |
| [`draw_it_as_a_picture`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass/examples/draw_it_as_a_picture.rs) | What does this field look like? Saves a colored picture you can open. |
| [`how_do_two_fields_compare`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass/examples/how_do_two_fields_compare.rs) | How does one field differ from another on the same grid? |

## Feature flags

Every feature but `schema` and `conformance` is on by default. Turn off the
ones you don't need with `default-features = false` and list the ones you want,
and your program won't include the code for the rest.

| Feature | Default | What it adds |
|---|---|---|
| `grib1` | on | Reading [GRIB edition 1](https://www.nco.ncep.noaa.gov/pmb/docs/on388/) |
| `grib2` | on | Reading [GRIB edition 2](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/) |
| `netcdf` | on | Reading NetCDF [classic](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec) and [NetCDF-4](https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#netcdf_4_spec) |
| `zarr` | on | Reading Zarr stores |
| `fetchplan` | on | Working out which byte ranges of a remote file to fetch (needs `grib2`) |
| `render` | on | Reprojecting, coloring and drawing maps |
| `analysis` | on | Comparing fields, contour lines and CSV export |
| `schema` | off | A JSON Schema of every data type, for generating other languages' type definitions |
| `conformance` | off | The shared test suite every Fieldglass host is checked against; useful only from a checkout of the repository |

How the API is designed, how color is decided once and shared with the GPU,
and how the test suite keeps every host in step are described on
[docs.rs](https://docs.rs/fieldglass).

## License

MIT OR Apache-2.0

## AI disclosure

Much of this crate's code, tests and documentation was written with AI
assistance (Anthropic's Claude), under the maintainer's direction and review.

### How do I maintain quality?

I treat the AI as a fast contributor whose work I don't take on trust:

- Fieldglass is designed against the specifications and reference libraries
  are used to check against the resulting behaviors. If behaviors differ
  between Fieldglass and a reference, Fieldglass follows the specification and
  records why.
- Every example comes from as close to real-world files as possible while also
  trying not to bloat the size of our libraries with data. In `fieldglass`,
  the files come from forecast centers and models, and are checked against
  ecCodes, the netCDF and HDF5 libraries, and xarray.
- I take a personal hand in testing each feature and delivery before a new tag
  is released. I maintain a testing plan in which I test each change.
- I have many strict automatic checks for formatting, lints, the test suite,
  dependency and security scans, and fuzzing with damaged or hostile files to
  make sure we refuse them safely.

### Where do I currently notice gaps?

- I dislike the current documentation. There's a lot of it, and it's hard to
  understand. I am undertaking efforts to rewrite the user-facing documentation
  with my own words. This file has been rewritten by me.
- While I have been ensuring proper functionality, behaviors, and larger-picture
  dependency graphs, I haven't been reading over every line of code. As the
  shape of things starts to become a bit more stable, I hope to review more of
  the code closely, but to be totally transparent this is more of a 'desire'
  than a reality currently.
