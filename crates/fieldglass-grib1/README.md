# fieldglass-grib1

GRIB edition 1 reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Reads every GRIB1 message's sections — indicator, product definition (PDS),
grid description (GDS), bitmap, and binary data (BDS) — and decodes grid values
into a `Vec<Option<f64>>`. It implements the reader and metadata traits from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core), so decoded fields
carry the grid geometry needed for reprojection and overlays without any
format-specific rendering code.

The non-gridded forms have their own entry points: spherical-harmonic **spectral**
messages decode to coefficients (`decode_spectral_message`) and synthesize back
onto a lat/lon grid via the shared inverse spherical-harmonic transform
(`synthesize_spectral_message`) so they render like any other field, and true
**matrix-of-values** messages (`matrixOfValues = 1`) decode to an `NR×NC` matrix
per grid point (`decode_matrix_message`).

Decoders are cross-checked against ECMWF eccodes; the spectral transform and the
matrix reshape — which eccodes cannot perform — against the definitive spec.

## License

Licensed under either of MIT or Apache-2.0 at your option.
