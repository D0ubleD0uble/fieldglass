//! Open a GRIB2 file, list its messages, and decode each field.
//!
//!     cargo run -p fieldglass-grib2 --example decode
//!
//! Runs against three committed fixtures, so it works from a clean checkout
//! with no network and no `samples/`. The set is deliberate. Every scalar §5
//! packing reaches the same [`Grib2Reader::decode_message_values`], so a caller
//! never branches on the template — but that method returns the field *as the
//! message stores it*, and a reduced Gaussian grid stores `sum(PL)` values
//! rather than the `Ni × Nj` §3 reports.
//! [`Grib2Reader::decode_message_raster`] is the entry point that hands back
//! the rectangle either way, and printing both counts is the shortest way to
//! show the difference. The third fixture is not a rectangle of values at all,
//! which is the other thing a caller has to handle.

use fieldglass_grib2::{FieldglassError, Grib2Reader, lookup_discipline};

/// ECMWF 16 × 31 regular lat/lon 2-metre temperature, simple packing (5.0).
const REGULAR_LATLON: &[u8] = include_bytes!("../tests/fixtures/regular_latlon_surface.grib2");

/// ECMWF reduced Gaussian pressure-level field: rows of differing width, so
/// the stored count and the raster count are not the same number.
const REDUCED_GAUSSIAN: &[u8] =
    include_bytes!("../tests/fixtures/reduced_gaussian_pressure_level.grib2");

/// A HEALPix field (§3.101): a list of pixels, with no rows and columns to
/// report, so it takes the skip branch below rather than the scalar decode.
const HEALPIX: &[u8] = include_bytes!("../tests/fixtures/healpix_n2_nested.grib2");

fn main() -> Result<(), FieldglassError> {
    for (label, bytes) in [
        ("regular lat/lon", REGULAR_LATLON),
        ("reduced Gaussian", REDUCED_GAUSSIAN),
        ("HEALPix", HEALPIX),
    ] {
        let reader = Grib2Reader::from_bytes(bytes.to_vec())?;
        println!("{label}: {} message(s)", reader.message_count());

        for index in 0..reader.message_count() {
            let msg = &reader.messages[index];
            // `None` for the three families that are not a rectangle of values
            // at all — spherical-harmonic and bi-Fourier coefficients, HEALPix
            // pixels — which have their own entry points and would refuse the
            // scalar decode below, and also for a §3 template this build does
            // not model, whose shape is simply unknown. `size_label` is how a
            // message of the first kind states its own size (`T63`, `Nside 4`).
            let Some((ni, nj)) = msg.gds.dimensions() else {
                println!(
                    "  [{index}] {} ({}): no raster shape; skipping",
                    msg.gds.template_name(),
                    msg.gds
                        .size_label()
                        .unwrap_or_else(|| "no size".to_string()),
                );
                continue;
            };

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
