//! Spherical-harmonic coefficient encoding — GRIB1 BDS flag bit 0 = 1.
//!
//! Used by IFS analyses and a handful of model outputs. Recovering a grid
//! requires an inverse Legendre transform; out of scope for this crate
//! today. Surfaces a precise [`FieldglassError::UnsupportedSection`] so
//! callers see *why* the message can't be decoded.

use fieldglass_core::FieldglassError;

use crate::bds::BdsHeader;

use super::Grib1Packing;

pub struct SphericalPacking;

impl Grib1Packing for SphericalPacking {
    fn decode(
        &self,
        _bds: &[u8],
        _header: &BdsHeader,
        _decimal_scale: i16,
        _bitmap: Option<&[bool]>,
        _expected_count: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        Err(FieldglassError::UnsupportedSection(
            "BDS uses spherical-harmonic coefficients (not yet supported — \
             only simple grid-point packing decodes today)"
                .into(),
        ))
    }
}
