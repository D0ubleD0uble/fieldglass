//! Open a GRIB1 file, list its messages, and decode one field.
//!
//!     cargo run -p fieldglass-grib1 --example decode
//!
//! Runs against two committed fixtures, so it works from a clean checkout with
//! no network and no `samples/`. The pair is deliberate: the crate's central
//! decode rule is that [`Grib1Reader::decode_message_values`] hands back the
//! field *as the message stores it*, which is `Ni × Nj` values only when the
//! grid is regular. On the reduced Gaussian fixture the two counts differ, and
//! printing both is the shortest way to show why
//! [`Grib1Reader::decode_message_raster`] exists.

use fieldglass_grib1::{FieldglassError, Grib1MessageKind, Grib1Reader};

/// Canadian Meteorological Centre regional model, wind speed at 300 hPa on a
/// 135 × 95 polar-stereographic grid — a regular grid, so storage and raster
/// agree.
const POLAR_STEREO: &[u8] = include_bytes!("../tests/fixtures/cmc_wind_300_2010052400_p012.grib");

/// An N32 reduced Gaussian grid: 64 rows whose widths differ, stored as
/// `sum(PL)` values rather than the `Ni × Nj` the GDS reports.
const REDUCED_GAUSSIAN: &[u8] = include_bytes!("../tests/fixtures/reduced_gg_n32_smooth.grib1");

fn main() -> Result<(), FieldglassError> {
    for (label, bytes) in [
        ("polar stereographic", POLAR_STEREO),
        ("reduced Gaussian", REDUCED_GAUSSIAN),
    ] {
        let reader = Grib1Reader::from_bytes(bytes.to_vec())?;
        println!("{label}: {} message(s)", reader.message_count());

        for index in 0..reader.message_count() {
            // Which decode entry point applies is not a property of the packing
            // label alone, so ask rather than guess.
            if reader.message_kind(index) != Grib1MessageKind::Grid {
                println!("  [{index}] not a one-value-per-point grid; skipping");
                continue;
            }

            let gds = reader.messages[index]
                .gds
                .as_ref()
                .expect("a Grid message resolves a grid description");
            let (ni, nj) = gds.dimensions().expect("a raster shape");

            let stored = reader.decode_message_values(index)?;
            let raster = reader.decode_message_raster(index)?;
            let present = raster.iter().filter(|v| v.is_some()).count();

            println!(
                "  [{index}] {} {ni}×{nj}: {} stored, {} in the raster ({present} present)",
                gds.grid_type_name(),
                stored.len(),
                raster.len(),
            );
        }
    }

    Ok(())
}
