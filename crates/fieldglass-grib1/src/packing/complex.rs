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
        bds: &[u8],
        header: &BdsHeader,
        decimal_scale: i16,
        bitmap: Option<&[bool]>,
        expected_count: usize,
        cols: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let ext = header.complex_extended.ok_or_else(|| {
            // Strict-WMO complex packing without the extra-flags octet
            // is undocumented in eccodes' GRIB1 templates and not seen in
            // any operational source we know of; surface it explicitly.
            FieldglassError::UnsupportedSection(
                "BDS uses complex packing without extra-flags octet — \
                 layout undefined in eccodes' GRIB1 templates."
                    .into(),
            )
        })?;

        let label = ext.packing_type_label();

        // Route the variants the second-order decoder handles. Today that's
        // the general-extended (`generalExtended2ordr = 1`,
        // `secondOrderOfDifferentWidth = 1`, `secondaryBitmapPresent = 0`)
        // family — no SPD, SPD-1, SPD-2 (eccodes' canonical
        // `grid_second_order`), and SPD-3. Other packings (matrix,
        // secondary-bitmap, row-by-row, constant-width) return an
        // unsupported error naming the eccodes packingType.
        let supported_general_extended = !ext.matrix_of_values()
            && !ext.secondary_bitmap_present()
            && ext.second_order_of_different_width()
            && ext.general_extended_2ordr();

        if supported_general_extended {
            return super::second_order::decode(
                bds,
                header,
                decimal_scale,
                bitmap,
                expected_count,
                cols,
            );
        }

        Err(FieldglassError::UnsupportedSection(format!(
            "BDS uses complex / second-order packing — variant `{label}`. \
             Decoder for this variant is not yet implemented; only the \
             general-extended `grid_second_order_*` family decodes today."
        )))
    }
}
