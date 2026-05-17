//! GRIB2 Data Section (§7) — simple-packing (template 5.0) decoder.
//!
//! §7 is a thin wrapper around the packed bytes: 5 bytes of section header
//! (length + section number = 7) followed by the bit-packed values. All
//! decoding rules live in the [`DataRepresentationSection`] (§5); this
//! module only converts the packed bytes into one `Option<f64>` per grid
//! point.
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 7 + Template 5.0 decoding formula.

use crate::drs::{DataRepresentationTemplate, SimplePackingTemplate};
use crate::section::{SECTION_HEADER_LEN, SectionHeader};
use fieldglass_core::{FieldglassError, bits::BitReader};

/// Section number for the Data Section.
pub const DS_SECTION_NUMBER: u8 = 7;

/// Validate the §7 header and return the byte slice that holds the packed
/// values. Errors if the slice doesn't start with a §7 header, or if the
/// declared section length exceeds `bytes.len()`.
pub fn parse_data_section_body(
    bytes: &[u8],
    header: SectionHeader,
) -> Result<&[u8], FieldglassError> {
    if header.number != DS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "expected DS (section {DS_SECTION_NUMBER}), got section {}",
            header.number
        )));
    }
    let len = header.length as usize;
    if len < SECTION_HEADER_LEN {
        return Err(FieldglassError::Parse(format!(
            "DS section length {len} is below the {SECTION_HEADER_LEN}-byte header minimum"
        )));
    }
    if bytes.len() < len {
        return Err(FieldglassError::Parse(format!(
            "DS declares length {len} but only {} bytes available",
            bytes.len()
        )));
    }
    Ok(&bytes[SECTION_HEADER_LEN..len])
}

/// Decode the §7 packed bytes for a message using the dispatch already
/// captured in the §5 DRS. Returns one entry per grid point in scan order:
/// `Some(value)` for present points, `None` for points masked out by the
/// §6 bitmap.
///
/// `expected_count` is the grid-point count from §3 GDS — the output is
/// always that length. `bitmap` is the decoded §6 bitmap when present;
/// pass `None` when §6 indicator was 255 (no bitmap).
pub fn decode_values(
    ds_payload: &[u8],
    template: DataRepresentationTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    match template {
        DataRepresentationTemplate::Simple(t) => {
            decode_simple_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::Unsupported(n) => Err(FieldglassError::UnsupportedSection(
            format!("DRS template 5.{n} decoding is not implemented"),
        )),
    }
}

/// Decode simple packing (template 5.0). Formula per WMO spec:
/// `value = R + X · 2^E · 10^-D`.
fn decode_simple_packing(
    ds_payload: &[u8],
    t: &SimplePackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    if let Some(b) = bitmap
        && b.len() != expected_count
    {
        return Err(FieldglassError::Parse(format!(
            "bitmap length {} != grid-point count {expected_count}",
            b.len()
        )));
    }

    let present_count = match bitmap {
        Some(b) => b.iter().filter(|p| **p).count(),
        None => expected_count,
    };

    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));

    // Constant field: every present point equals R · 10^-D.
    if t.bits_per_value == 0 {
        let constant = r * d_inv;
        return Ok(materialise_constant(constant, bitmap, expected_count));
    }

    if t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "simple packing: bits_per_value {} exceeds 32",
            t.bits_per_value
        )));
    }

    let total_bits = ds_payload.len().saturating_mul(8);
    let stored_count = total_bits / t.bits_per_value as usize;
    if stored_count < present_count {
        return Err(FieldglassError::Parse(format!(
            "DS holds {stored_count} values but {present_count} are required"
        )));
    }

    let mut reader = BitReader::new(ds_payload);
    let mut decoded = Vec::with_capacity(present_count);
    for _ in 0..present_count {
        let x = reader.read_bits(t.bits_per_value)?;
        decoded.push((r + x as f64 * two_pow_e) * d_inv);
    }

    Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
}

/// Spread `present` into the full grid using `bitmap` — `Some(value)` for
/// flagged points, `None` for unflagged. Asserts shape internally; callers
/// pre-check lengths.
fn interleave_with_bitmap(
    present: Vec<f64>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    match bitmap {
        None => present.into_iter().map(Some).collect(),
        Some(b) => {
            let mut out = Vec::with_capacity(expected_count);
            let mut iter = present.into_iter();
            for &flag in b {
                out.push(if flag { iter.next() } else { None });
            }
            out
        }
    }
}

/// Constant-field special case (`bits_per_value == 0`): every present point
/// equals the same value; absent points are `None`.
fn materialise_constant(
    value: f64,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    match bitmap {
        None => vec![Some(value); expected_count],
        Some(b) => b
            .iter()
            .map(|&flag| if flag { Some(value) } else { None })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drs::SimplePackingTemplate;
    use crate::section::parse_section_header;

    fn simple_template(r: f32, e: i16, d: i16, bits: u8) -> DataRepresentationTemplate {
        DataRepresentationTemplate::Simple(SimplePackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            original_field_type: 0,
        })
    }

    /// Pack `values` as MSB-first n-bit unsigned ints into a byte vector.
    fn pack_bits(values: &[u32], bits: u8) -> Vec<u8> {
        let total_bits = values.len() * bits as usize;
        let bytes_needed = total_bits.div_ceil(8);
        let mut out = vec![0u8; bytes_needed];
        let mut bit = 0usize;
        for &v in values {
            for i in (0..bits).rev() {
                let b = ((v >> i) & 1) as u8;
                out[bit / 8] |= b << (7 - (bit % 8));
                bit += 1;
            }
        }
        out
    }

    #[test]
    fn simple_packing_decodes_no_bitmap() {
        // R = 300.0, E = 0, D = 0, 8 bits/value → value = R + X.
        let template = simple_template(300.0, 0, 0, 8);
        let packed = pack_bits(&[0, 5, 10, 20], 8);
        let decoded = decode_values(&packed, template, None, 4).expect("decode");
        assert_eq!(
            decoded,
            vec![Some(300.0), Some(305.0), Some(310.0), Some(320.0)],
        );
    }

    #[test]
    fn simple_packing_applies_decimal_scale() {
        // R = 0.0, E = 0, D = 1 → value = X * 10^-1.
        let template = simple_template(0.0, 0, 1, 8);
        let packed = pack_bits(&[0, 5, 15], 8);
        let decoded = decode_values(&packed, template, None, 3).expect("decode");
        assert!((decoded[0].unwrap() - 0.0).abs() < 1e-9);
        assert!((decoded[1].unwrap() - 0.5).abs() < 1e-9);
        assert!((decoded[2].unwrap() - 1.5).abs() < 1e-9);
    }

    #[test]
    fn simple_packing_applies_binary_scale() {
        // R = 0.0, E = -1, D = 0 → value = X * 2^-1.
        let template = simple_template(0.0, -1, 0, 8);
        let packed = pack_bits(&[0, 2, 6], 8);
        let decoded = decode_values(&packed, template, None, 3).expect("decode");
        assert!((decoded[0].unwrap() - 0.0).abs() < 1e-9);
        assert!((decoded[1].unwrap() - 1.0).abs() < 1e-9);
        assert!((decoded[2].unwrap() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn simple_packing_constant_field() {
        // bits_per_value == 0 → every present point equals R · 10^-D.
        let template = simple_template(7.0, 0, 1, 0);
        let decoded = decode_values(&[], template, None, 5).expect("decode");
        assert_eq!(decoded.len(), 5);
        for v in decoded {
            assert!((v.unwrap() - 0.7).abs() < 1e-9);
        }
    }

    #[test]
    fn simple_packing_honours_bitmap() {
        let template = simple_template(0.0, 0, 0, 8);
        // 3 present values packed (X = 1, 2, 3); bitmap flags points 0, 2, 4
        // as present out of 5 total points.
        let packed = pack_bits(&[1, 2, 3], 8);
        let bitmap = [true, false, true, false, true];
        let decoded = decode_values(&packed, template, Some(&bitmap), 5).expect("decode");
        assert_eq!(decoded, vec![Some(1.0), None, Some(2.0), None, Some(3.0)],);
    }

    #[test]
    fn simple_packing_constant_field_honours_bitmap() {
        let template = simple_template(42.0, 0, 0, 0);
        let bitmap = [true, false, true];
        let decoded = decode_values(&[], template, Some(&bitmap), 3).expect("decode");
        assert_eq!(decoded, vec![Some(42.0), None, Some(42.0)]);
    }

    #[test]
    fn rejects_bitmap_length_mismatch() {
        let template = simple_template(0.0, 0, 0, 8);
        let packed = pack_bits(&[1], 8);
        let bitmap = [true]; // says 1 point, but caller asks for 5
        assert!(decode_values(&packed, template, Some(&bitmap), 5).is_err());
    }

    #[test]
    fn rejects_short_payload() {
        let template = simple_template(0.0, 0, 0, 16);
        // Only 1 byte = 8 bits, but we need 16 bits per value × 2 points = 32.
        let packed = [0xFF];
        assert!(decode_values(&packed, template, None, 2).is_err());
    }

    #[test]
    fn rejects_bits_per_value_above_32() {
        let template = simple_template(0.0, 0, 0, 33);
        assert!(decode_values(&[0u8; 64], template, None, 4).is_err());
    }

    #[test]
    fn unsupported_template_yields_unsupported_error() {
        let template = DataRepresentationTemplate::Unsupported(40);
        let err = decode_values(&[], template, None, 0).expect_err("must reject");
        assert!(err.to_string().contains("template 5.40"));
    }

    #[test]
    fn parse_data_section_body_strips_header() {
        // Construct a §7 with 5-byte header + 4-byte body.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&9u32.to_be_bytes());
        buf.push(DS_SECTION_NUMBER);
        buf.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);
        let header = parse_section_header(&buf).unwrap();
        let body = parse_data_section_body(&buf, header).expect("body");
        assert_eq!(body, &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn parse_data_section_body_rejects_wrong_section_number() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&5u32.to_be_bytes());
        buf.push(5); // claim §5
        let header = parse_section_header(&buf).unwrap();
        assert!(parse_data_section_body(&buf, header).is_err());
    }
}
