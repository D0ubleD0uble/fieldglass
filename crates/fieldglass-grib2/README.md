# fieldglass-grib2

GRIB edition 2 reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Parses every message's sections (§0–§7) for metadata, and decodes grid values
for **every registered §5 Data Representation template** (Code Table 5.0) —
in pure Rust with no external libraries and no build flags. Scalar packings
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

Grid geometry is returned as the shared types from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core).

## Usage

```rust
use fieldglass_grib2::Grib2Reader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any GRIB2 file's bytes. This one is committed with the crate, so the
    // snippet runs as written from a checkout.
    let bytes = std::fs::read("tests/fixtures/regular_latlon_surface.grib2")?;
    let reader = Grib2Reader::from_bytes(bytes)?;

    for index in 0..reader.message_count() {
        let msg = &reader.messages[index];
        // `None` for the families that are not a rectangle of values at all:
        // spherical-harmonic and bi-Fourier coefficients, and HEALPix pixels.
        // They have their own entry points and would refuse the call below.
        let Some((ni, nj)) = msg.gds.dimensions() else {
            continue;
        };

        // Every scalar §5 packing reaches this one call — a caller never
        // branches on `msg.drs.template_number`. `decode_message_raster` hands
        // back `ni · nj` values in row-major order whatever the message stored
        // (reduced rows widened, a `j`-consecutive grid transposed);
        // `decode_message_values` is the field exactly as stored.
        let values: Vec<Option<f64>> = reader.decode_message_raster(index)?;
        println!("§5.{} {ni}x{nj}: {} values", msg.drs.template_number, values.len());
    }

    Ok(())
}
```

`cargo run -p fieldglass-grib2 --example decode` runs that against the fixtures
committed with the crate. The non-scalar packings named above have their own
entry points, because they do not produce one value per grid point.

## License

Licensed under either of MIT or Apache-2.0 at your option.
