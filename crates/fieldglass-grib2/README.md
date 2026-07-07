# fieldglass-grib2

GRIB edition 2 reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Parses every message's sections (§0–§7) for metadata, and decodes grid values
into a `Vec<Option<f64>>` for the data-representation templates it supports:
simple packing (5.0), complex packing (5.2 / 5.3), IEEE floating point (5.4),
JPEG 2000 (5.40), PNG (5.41), and CCSDS / AEC (5.42). Templates outside that set
parse to the section level and report an unsupported-section error on decode.

Value decoders are cross-checked against ECMWF eccodes. The compressed packings
use pure-Rust codecs, so the crate keeps its dependency-light,
cross-compilable build with no C dependencies.

It implements the reader and metadata traits from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core).

## License

Licensed under either of MIT or Apache-2.0 at your option.
