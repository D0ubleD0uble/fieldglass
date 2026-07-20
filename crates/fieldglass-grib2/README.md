# fieldglass-grib2

GRIB edition 2 reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Parses every message's sections (§0–§7) for metadata, and decodes grid values
for **every registered §5 Data Representation template** (Code Table 5.0) — a
claim no C-stack tool makes, in pure Rust with zero build flags. Scalar packings
(simple 5.0, complex 5.2 / 5.3, IEEE 5.4, JPEG 2000 5.40, PNG 5.41, CCSDS / AEC
5.42, log pre-processing 5.61, run-length 5.200, second-order 5.50001 / 5.50002,
flat matrix 5.1) decode to a `Vec<Option<f64>>`. The non-scalar packings have
their own entry points: spherical-harmonic spectral (5.50 / 5.51, which also
synthesize back to a lat/lon grid via the inverse transform), bi-Fourier
spectral (5.53), and the true per-point matrix (5.1). The pre-standard local
image templates (5.40000 / 5.40010) decode too.

Value decoders are cross-checked against ECMWF eccodes; for the handful eccodes
cannot handle (it crashes on the true matrix, cannot synthesise spectral grids,
and ships no 5.40010 definition), against the definitive spec and independent
implementations. The compressed packings use pure-Rust codecs, so the crate
keeps its dependency-light, cross-compilable build with no C dependencies.

It implements the reader and metadata traits from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core).

## License

Licensed under either of MIT or Apache-2.0 at your option.
