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
        header: &BdsHeader,
        _decimal_scale: i16,
        _bitmap: Option<&[bool]>,
        _expected_count: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        // Use the parsed extended-flag bits to surface the eccodes-style
        // packingType (e.g. `grid_second_order`, `grid_second_order_SPD3`)
        // rather than a generic "complex packing" message. Lets users grep
        // their failing files against eccodes' documentation directly.
        let label = header
            .complex_extended
            .map(|c| c.packing_type_label())
            .unwrap_or("complex (no extended header — likely WMO-strict second-order)");
        Err(FieldglassError::UnsupportedSection(format!(
            "BDS uses complex / second-order packing — variant `{label}`. \
             Decoder not yet implemented; only simple grid-point packing \
             decodes today."
        )))
    }
}
