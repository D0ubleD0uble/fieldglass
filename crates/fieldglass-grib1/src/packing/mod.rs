//! GRIB1 BDS packing decoders.
//!
//! Each variant of GRIB1 packing lives in its own module behind the
//! [`Grib1Packing`] trait. The dispatcher [`decoder_for`] inspects the BDS
//! flag bits and returns the right decoder; today only [`simple::SimplePacking`]
//! actually decodes — [`complex::ComplexPacking`] and
//! [`spherical::SphericalPacking`] are stubs that surface
//! [`FieldglassError::UnsupportedSection`] with a message naming the
//! packing mode, so users get a precise reason rather than a bare
//! "unsupported section".
//!
//! Adding a new packing means: drop a new module under `packing/`, implement
//! `Grib1Packing::decode`, and route it from `decoder_for`. No other crates
//! need to change.

use fieldglass_core::FieldglassError;

use crate::bds::BdsHeader;

pub mod complex;
pub mod second_order;
pub mod second_order_classic;
pub mod simple;
pub mod spherical;

/// A BDS packing decoder. Implementors take the full Binary Data Section
/// (starting at its 3-byte length prefix), the parsed header, the PDS
/// decimal scale factor, an optional per-point bitmap (from BMS), the
/// total grid-point count from the GDS, and the grid's column count
/// `cols` (used by complex/second-order decoders to undo boustrophedonic
/// row scanning — simple-packing impls ignore it). They return one
/// `Option<f64>` per grid point — `None` for bitmap-masked points,
/// `Some(value)` otherwise.
pub trait Grib1Packing {
    fn decode(
        &self,
        bds: &[u8],
        header: &BdsHeader,
        decimal_scale: i16,
        bitmap: Option<&[bool]>,
        expected_count: usize,
        cols: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError>;
}

/// Pick the right [`Grib1Packing`] for a parsed BDS header. Spherical-harmonic
/// wins over complex when both flag bits are set — neither is implemented
/// today, so the precedence only affects the error message.
pub fn decoder_for(header: &BdsHeader) -> Box<dyn Grib1Packing> {
    if header.is_spherical_harmonic {
        return Box::new(spherical::SphericalPacking);
    }
    if header.is_complex_packing {
        return Box::new(complex::ComplexPacking);
    }
    Box::new(simple::SimplePacking)
}

/// Shared reconstruction tail for every GRIB1 second-order packing. Given the
/// reconstructed integer grid `x` in storage order, scale it by
/// `(R + x·2^E) / 10^D`, undo boustrophedonic row ordering (odd rows stored
/// right-to-left) if `boustrophedonic` is set, then interleave `None` at any
/// bitmap-masked points. Boustrophedonic undo must precede the bitmap
/// interleave, because the bitmap maps the storage stream. Used by both
/// [`second_order`] and [`second_order_classic`].
pub(crate) fn finalize_second_order(
    x: Vec<i64>,
    header: &BdsHeader,
    decimal_scale: i16,
    boustrophedonic: bool,
    cols: usize,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let two_pow_e = 2f64.powi(header.binary_scale_factor as i32);
    let d_scale = 10f64.powi(-(decimal_scale as i32));
    let r = header.reference_value;

    let mut scaled: Vec<f64> = x
        .iter()
        .map(|v| (r + (*v as f64) * two_pow_e) * d_scale)
        .collect();

    if boustrophedonic && cols > 0 {
        let rows = scaled.len() / cols;
        for row in (1..rows).step_by(2) {
            let start = row * cols;
            let end = start + cols;
            scaled[start..end].reverse();
        }
    }

    match bitmap {
        None => {
            if scaled.len() != expected_count {
                return Err(FieldglassError::Parse(format!(
                    "second-order decoded {} values but {} expected",
                    scaled.len(),
                    expected_count
                )));
            }
            Ok(scaled.into_iter().map(Some).collect())
        }
        Some(b) => {
            let mut out = Vec::with_capacity(expected_count);
            let mut iter = scaled.into_iter();
            for present in b.iter().take(expected_count) {
                out.push(if *present { iter.next() } else { None });
            }
            Ok(out)
        }
    }
}
