//! Open a GRIB2 file, list its messages, and decode each field.
//!
//!     cargo run -p fieldglass-grib2 --example decode
//!
//! Runs against two committed fixtures, so it works from a clean checkout with
//! no network and no `samples/`. The pair is deliberate: every scalar §5
//! packing reaches the same [`Grib2Reader::decode_message_values`], so a
//! caller never branches on the template — but that method returns the field
//! *as the message stores it*, and a reduced Gaussian grid stores `sum(PL)`
//! values rather than the `Ni × Nj` §3 reports.
//! [`Grib2Reader::decode_message_raster`] is the entry point that hands back
//! the rectangle either way, and printing both counts is the shortest way to
//! show the difference.

use fieldglass_grib2::{FieldglassError, Grib2Reader, lookup_discipline};

/// ECMWF 16 × 31 regular lat/lon 2-metre temperature, simple packing (5.0).
const REGULAR_LATLON: &[u8] = include_bytes!("../tests/fixtures/regular_latlon_surface.grib2");

/// ECMWF reduced Gaussian pressure-level field: rows of differing width, so
/// the stored count and the raster count are not the same number.
const REDUCED_GAUSSIAN: &[u8] =
    include_bytes!("../tests/fixtures/reduced_gaussian_pressure_level.grib2");

fn main() -> Result<(), FieldglassError> {
    for (label, bytes) in [
        ("regular lat/lon", REGULAR_LATLON),
        ("reduced Gaussian", REDUCED_GAUSSIAN),
    ] {
        let reader = Grib2Reader::from_bytes(bytes.to_vec())?;
        println!("{label}: {} message(s)", reader.message_count());

        for index in 0..reader.message_count() {
            let msg = &reader.messages[index];
            let (ni, nj) = msg.gds.dimensions().expect("a raster shape");

            let stored = reader.decode_message_values(index)?;
            let raster = reader.decode_message_raster(index)?;
            let present = raster.iter().filter(|v| v.is_some()).count();

            println!(
                "  [{index}] {} / §3.{} {ni}×{nj} / §5.{}: {} stored, {} in the raster \
                 ({present} present)",
                lookup_discipline(msg.is.discipline),
                msg.gds.template_number,
                msg.drs.template_number,
                stored.len(),
                raster.len(),
            );
        }
    }

    Ok(())
}
