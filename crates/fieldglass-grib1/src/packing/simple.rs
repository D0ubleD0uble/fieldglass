//! Simple grid-point packing — GRIB1 BDS flag bits 1+2 = 00.
//!
//! Each grid point is stored as `bits_per_value` bits, MSB-first, in scan
//! order; the unpacked value is `(R + X * 2^E) / 10^D` where `R` is the BDS
//! reference value, `E` the binary scale factor, `D` the PDS decimal scale,
//! and `X` the packed integer. `bits_per_value == 0` is the constant-field
//! special case: every present point equals `R / 10^D`.
//!
//! This is the only packing supported today.

use fieldglass_core::FieldglassError;

use crate::bds::{BDS_DATA_OFFSET, BdsHeader};

use super::{Grib1Packing, interleave_with_bitmap, present_count, unpack_simple_values};

pub struct SimplePacking;

impl Grib1Packing for SimplePacking {
    fn decode(
        &self,
        bds: &[u8],
        header: &BdsHeader,
        decimal_scale: i16,
        bitmap: Option<&[bool]>,
        expected_count: usize,
        _cols: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        if (bds.len() as u32) < header.section_len {
            return Err(FieldglassError::Parse(format!(
                "BDS body shorter than declared section_len {}",
                header.section_len
            )));
        }

        let d_scale = 10f64.powi(-(decimal_scale as i32));
        let r = header.reference_value;
        let two_pow_e = 2f64.powi(header.binary_scale_factor as i32);

        // Constant field: every present grid point equals R / 10^D.
        if header.bits_per_value == 0 {
            let constant = r * d_scale;
            return Ok(materialise_constant(constant, bitmap, expected_count));
        }

        if header.bits_per_value > 32 {
            return Err(FieldglassError::Parse(format!(
                "BDS bits_per_value {} exceeds 32",
                header.bits_per_value
            )));
        }

        let n = header.bits_per_value;
        let packed_byte_len = header.section_len as usize - BDS_DATA_OFFSET;
        let total_packed_bits = packed_byte_len
            .saturating_mul(8)
            .saturating_sub(header.unused_trailing_bits as usize);
        let stored_count = total_packed_bits / n as usize;

        let present = present_count(bitmap, expected_count);
        if stored_count < present {
            return Err(FieldglassError::Parse(format!(
                "BDS holds {stored_count} values but {present} are required"
            )));
        }

        let packed = &bds[BDS_DATA_OFFSET..header.section_len as usize];
        let decoded = unpack_simple_values(packed, n, r, two_pow_e, d_scale, present)?;
        Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
    }
}

fn materialise_constant(
    value: f64,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    match bitmap {
        Some(b) => b
            .iter()
            .take(expected_count)
            .map(|present| if *present { Some(value) } else { None })
            .collect(),
        None => vec![Some(value); expected_count],
    }
}
