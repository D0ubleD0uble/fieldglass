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

        // Matrix-of-values packing puts a matrix at every grid point; none of
        // our second-order decoders handle it. Reject it explicitly before the
        // packingType dispatch (which ignores the matrix bit).
        if ext.matrix_of_values() {
            return Err(FieldglassError::UnsupportedSection(format!(
                "BDS uses complex / second-order packing — variant `{label}` \
                 with matrixOfValues set, which is not supported."
            )));
        }

        // The second-order layouts size their group / row / secondary-bitmap
        // reads against the *full* grid (`numberOfGroups · cols`, `P2`). When a
        // BMS bit-map masks any point the BDS stores only the present values,
        // so those counts no longer line up: `row_by_row` would read past the
        // packed stream and misdecode, while `constant_width` / `general` fail
        // with a confusing "P2 != grid points". Reject the combination up front
        // rather than emit wrong values. (Full bit-map support for second-order
        // packing is tracked separately.)
        if super::present_count(bitmap, expected_count) != expected_count {
            return Err(FieldglassError::UnsupportedSection(format!(
                "BDS uses second-order packing (`{label}`) together with a \
                 bit-map that masks grid points, which is not yet supported."
            )));
        }

        // Dispatch on the eccodes packingType label (derived from the same
        // extended-flag bits eccodes uses in `grib1/section.4.def`). The
        // general-extended (`generalExtended2ordr = 1`) family — no SPD,
        // SPD-1/2/3 — goes to `second_order`; the three classic WMO layouts go
        // to `second_order_classic`.
        match label {
            "grid_second_order_no_SPD"
            | "grid_second_order_SPD1"
            | "grid_second_order"
            | "grid_second_order_SPD3" => super::second_order::decode(
                bds,
                header,
                decimal_scale,
                bitmap,
                expected_count,
                cols,
            ),
            "grid_second_order_row_by_row" => super::second_order_classic::decode_row_by_row(
                bds,
                header,
                decimal_scale,
                bitmap,
                expected_count,
                cols,
            ),
            "grid_second_order_constant_width" => {
                super::second_order_classic::decode_constant_width(
                    bds,
                    header,
                    decimal_scale,
                    bitmap,
                    expected_count,
                    cols,
                )
            }
            "grid_second_order_general_grib1" => super::second_order_classic::decode_general(
                bds,
                header,
                decimal_scale,
                bitmap,
                expected_count,
                cols,
            ),
            _ => Err(FieldglassError::UnsupportedSection(format!(
                "BDS uses complex / second-order packing — variant `{label}`. \
                 Decoder for this variant is not yet implemented."
            ))),
        }
    }
}
