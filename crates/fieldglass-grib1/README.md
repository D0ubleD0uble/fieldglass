# fieldglass-grib1

GRIB edition 1 reader for [Fieldglass](https://github.com/D0ubleD0uble/fieldglass),
a viewer for meteorological data files.

Reads every GRIB1 message's sections — indicator, product definition (PDS),
grid description (GDS), bitmap, and binary data (BDS) — and decodes grid values
into a `Vec<Option<f64>>`. Grid geometry is returned as the shared types from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core), so decoded fields
carry what reprojection and overlays need without any format-specific rendering
code.

The non-gridded forms have their own entry points: spherical-harmonic **spectral**
messages decode to coefficients (`decode_spectral_message`) and synthesize back
onto a lat/lon grid via the shared inverse spherical-harmonic transform
(`synthesize_spectral_message`) so they render like any other field, and true
**matrix-of-values** messages (`matrixOfValues = 1`) decode to an `NR×NC` matrix
per grid point (`decode_matrix_message`).

Decoders are cross-checked against ECMWF eccodes; the spectral transform and the
matrix reshape — which eccodes cannot perform — against the definitive spec.

## Usage

```rust
use fieldglass_grib1::{Grib1MessageKind, Grib1Reader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any GRIB1 file's bytes. This one is committed with the crate, so the
    // snippet runs as written from a checkout.
    let bytes = std::fs::read("tests/fixtures/cmc_wind_300_2010052400_p012.grib")?;
    let reader = Grib1Reader::from_bytes(bytes)?;

    for index in 0..reader.message_count() {
        // Which entry point applies is not a property of the packing label
        // alone, so ask rather than guess.
        if reader.message_kind(index) != Grib1MessageKind::Grid {
            continue;
        }
        let gds = reader.messages[index].gds.as_ref().expect("a grid description");
        let (ni, nj) = gds.dimensions().expect("a raster shape");

        // `decode_message_raster` hands back `ni · nj` values in row-major
        // order whatever the message stored — reduced rows widened, a
        // `j`-consecutive grid transposed. `decode_message_values` is the field
        // exactly as stored, when that is what you want.
        let values: Vec<Option<f64>> = reader.decode_message_raster(index)?;
        println!("{} {ni}x{nj}: {} values", gds.grid_type_name(), values.len());
    }

    Ok(())
}
```

`cargo run -p fieldglass-grib1 --example decode` runs that against the fixtures
committed with the crate.

## License

Licensed under either of MIT or Apache-2.0 at your option.
