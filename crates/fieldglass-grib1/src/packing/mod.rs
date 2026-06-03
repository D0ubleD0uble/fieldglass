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
