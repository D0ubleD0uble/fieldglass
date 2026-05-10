//! Complex / second-order grid-point packing — GRIB1 BDS flag bit 1 = 1.
//!
//! Used by ECMWF and most modern operational archives. Splits the grid into
//! groups, with per-group reference values, widths, and primary/secondary
//! bitmaps (see WMO Manual on Codes Vol I.2 Code Table 11). Not yet
//! implemented; surfaces a precise [`FieldglassError::UnsupportedSection`]
//! so callers can tell *which* packing is the blocker.

use fieldglass_core::FieldglassError;

use crate::bds::BdsHeader;

use super::Grib1Packing;

pub struct ComplexPacking;

impl Grib1Packing for ComplexPacking {
    fn decode(
        &self,
        _bds: &[u8],
        _header: &BdsHeader,
        _decimal_scale: i16,
        _bitmap: Option<&[bool]>,
        _expected_count: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        Err(FieldglassError::UnsupportedSection(
            "BDS uses complex / second-order packing (not yet supported — \
             only simple grid-point packing decodes today)"
                .into(),
        ))
    }
}
