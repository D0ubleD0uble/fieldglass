# fieldglass-grib1

GRIB edition 1 reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Reads every GRIB1 message's sections — indicator, product definition (PDS),
grid description (GDS), bitmap, and binary data (BDS) — and decodes grid values
into a `Vec<Option<f64>>`. It implements the reader and metadata traits from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core), so decoded fields
carry the grid geometry needed for reprojection and overlays without any
format-specific rendering code.

Decoders are cross-checked against ECMWF eccodes.

## License

Licensed under either of MIT or Apache-2.0 at your option.
