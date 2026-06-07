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

use crate::drs::{
    ComplexPackingTemplate, DataRepresentationTemplate, IeeePackingTemplate, SimplePackingTemplate,
};
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
        DataRepresentationTemplate::Complex(t) => {
            decode_complex_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::Ieee(t) => {
            decode_ieee_packing(ds_payload, &t, bitmap, expected_count)
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

/// Decode complex packing (template 5.2). The field is split into NG groups
/// of consecutive points; §7 carries, as one continuous MSB-first bitstream,
/// the NG group reference values (at `bits_per_value` bits each), the NG
/// group widths (at `group_width_bits`, offset by `group_width_reference`),
/// the NG group lengths (at `group_length_bits`, offset by
/// `group_length_reference` and scaled by `group_length_increment`, with the
/// last group's length overridden by `group_length_last`), and finally the
/// per-point offsets (each group's points at that group's width). The value
/// at a point in group `g` is `(R + (group_ref[g] + X) · 2^E) · 10^-D`.
///
/// Supports the common envelope: general group splitting
/// (`group_splitting_method == 1`) with no inline missing-value management
/// (`missing_value_management == 0`). Row-by-row splitting and inline
/// missing values surface as [`FieldglassError::UnsupportedSection`].
fn decode_complex_packing(
    ds_payload: &[u8],
    t: &ComplexPackingTemplate,
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

    if t.group_splitting_method != 1 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "DRS template 5.2 group splitting method {} is not supported \
             (only general group splitting, method 1)",
            t.group_splitting_method
        )));
    }
    if t.missing_value_management != 0 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "DRS template 5.2 missing-value management {} is not supported \
             (only management 0, no inline missing values)",
            t.missing_value_management
        )));
    }
    // The bit-width fields feed `BitReader::read_bits`, whose contract tops
    // out at 32 bits; a wider field is malformed (and would silently truncate).
    for (label, bits) in [
        ("group reference", t.bits_per_value),
        ("group width", t.group_width_bits),
        ("group length", t.group_length_bits),
    ] {
        if bits > 32 {
            return Err(FieldglassError::Parse(format!(
                "complex packing: {label} field width {bits} exceeds 32 bits"
            )));
        }
    }

    let present_count = match bitmap {
        Some(b) => b.iter().filter(|p| **p).count(),
        None => expected_count,
    };

    let num_groups = t.num_groups as usize;
    if num_groups == 0 {
        // A grid with no groups can only describe an empty field; anything
        // else is malformed.
        if present_count != 0 {
            return Err(FieldglassError::Parse(format!(
                "complex packing declares 0 groups but {present_count} values are required"
            )));
        }
        return Ok(interleave_with_bitmap(Vec::new(), bitmap, expected_count));
    }
    // Every group covers at least one point, so NG can't legitimately exceed
    // the number of present points — this bounds the per-group allocations
    // below against a malformed huge NG.
    if num_groups > present_count {
        return Err(FieldglassError::Parse(format!(
            "complex packing declares {num_groups} groups but only {present_count} \
             values are present"
        )));
    }

    let mut reader = BitReader::new(ds_payload);

    // Block 1: group reference values.
    let mut group_refs = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        group_refs.push(reader.read_bits(t.bits_per_value)?);
    }

    // Block 2: group widths (stored value offset by the width reference).
    // Computed in u64 so the reference + a 32-bit stored value can't overflow
    // before the `> 32` range check in the data loop sees it.
    let mut group_widths: Vec<u64> = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        let stored = reader.read_bits(t.group_width_bits)?;
        group_widths.push(t.group_width_reference as u64 + stored as u64);
    }

    // Block 3: group lengths. The stored value for every group is read (so
    // the bit cursor reaches the data block correctly), then the last group's
    // length is overridden by the explicit `group_length_last` field.
    let mut group_lengths = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        let stored = reader.read_bits(t.group_length_bits)? as usize;
        let len = stored
            .checked_mul(t.group_length_increment as usize)
            .and_then(|scaled| scaled.checked_add(t.group_length_reference as usize))
            .ok_or_else(|| {
                FieldglassError::Parse("complex packing: group length overflows usize".into())
            })?;
        group_lengths.push(len);
    }
    // num_groups >= 1 here, so the last element exists.
    *group_lengths.last_mut().expect("num_groups >= 1") = t.group_length_last as usize;

    // The group lengths must account for exactly the present points; validate
    // before allocating so a malformed length can't drive a huge allocation.
    let mut total = 0usize;
    for &len in &group_lengths {
        total = total.checked_add(len).ok_or_else(|| {
            FieldglassError::Parse("complex packing: group lengths sum overflows usize".into())
        })?;
    }
    if total != present_count {
        return Err(FieldglassError::Parse(format!(
            "complex packing: group lengths sum to {total} but {present_count} values are required"
        )));
    }

    // Block 4: the per-point offsets, decoded group by group.
    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));

    let mut decoded = Vec::with_capacity(present_count);
    for g in 0..num_groups {
        let width = group_widths[g];
        let group_ref = group_refs[g] as u64;
        if width == 0 {
            // Zero-width group: every point equals the group reference.
            let value = (r + group_ref as f64 * two_pow_e) * d_inv;
            for _ in 0..group_lengths[g] {
                decoded.push(value);
            }
        } else {
            if width > 32 {
                // The actual width is `group_width_reference + stored`, which
                // can exceed 32 even when each field is individually in range;
                // `read_bits` only honours up to 32 bits per value.
                return Err(FieldglassError::Parse(format!(
                    "complex packing: group {g} width {width} exceeds 32 bits"
                )));
            }
            for _ in 0..group_lengths[g] {
                let x = reader.read_bits(width as u8)? as u64;
                let scaled = group_ref + x;
                decoded.push((r + scaled as f64 * two_pow_e) * d_inv);
            }
        }
    }

    Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
}

/// Decode IEEE floating-point packing (template 5.4). Each present grid point
/// is stored verbatim as a big-endian IEEE float — precision `1` = 32-bit,
/// `2` = 64-bit — with no reference / scale transform. Mirrors GRIB1
/// `grid_ieee`; precision `3` (128-bit) is unimplemented in eccodes too and
/// surfaces as [`FieldglassError::UnsupportedSection`].
fn decode_ieee_packing(
    ds_payload: &[u8],
    t: &IeeePackingTemplate,
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

    let width = match t.precision {
        1 => 4, // IEEE 32-bit
        2 => 8, // IEEE 64-bit
        other => {
            return Err(FieldglassError::UnsupportedSection(format!(
                "DRS template 5.4 uses precision {other} (code-table 5.7); only \
                 32-bit (1) and 64-bit (2) are supported — 128-bit is \
                 unimplemented in eccodes too."
            )));
        }
    };

    let present_count = match bitmap {
        Some(b) => b.iter().filter(|p| **p).count(),
        None => expected_count,
    };

    let stored_count = ds_payload.len() / width;
    if stored_count < present_count {
        return Err(FieldglassError::Parse(format!(
            "DS holds {stored_count} IEEE values but {present_count} are required"
        )));
    }

    let mut decoded = Vec::with_capacity(present_count);
    for chunk in ds_payload.chunks_exact(width).take(present_count) {
        let value = match width {
            4 => f32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f64,
            _ => f64::from_be_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]),
        };
        decoded.push(value);
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

    // -----------------------------------------------------------------
    // Complex packing (template 5.2)
    // -----------------------------------------------------------------

    /// Pack `(value, width)` fields MSB-first into one continuous bitstream —
    /// the §7 layout complex packing uses for its reference / width / length /
    /// data blocks.
    fn pack_fields(fields: &[(u32, u8)]) -> Vec<u8> {
        let total_bits: usize = fields.iter().map(|(_, w)| *w as usize).sum();
        let mut out = vec![0u8; total_bits.div_ceil(8)];
        let mut bit = 0usize;
        for &(v, w) in fields {
            for i in (0..w).rev() {
                let b = ((v >> i) & 1) as u8;
                out[bit / 8] |= b << (7 - (bit % 8));
                bit += 1;
            }
        }
        out
    }

    /// A complex-packing template with the common envelope (general splitting,
    /// no missing values, R/E/D = 0/0/0). Tests override the group descriptors.
    fn complex_template_base() -> ComplexPackingTemplate {
        ComplexPackingTemplate {
            reference_value: 0.0,
            binary_scale_factor: 0,
            decimal_scale_factor: 0,
            bits_per_value: 8,
            original_field_type: 0,
            group_splitting_method: 1,
            missing_value_management: 0,
            primary_missing_value: 0,
            secondary_missing_value: 0,
            num_groups: 0,
            group_width_reference: 0,
            group_width_bits: 4,
            group_length_reference: 0,
            group_length_increment: 1,
            group_length_last: 0,
            group_length_bits: 8,
        }
    }

    #[test]
    fn complex_packing_decodes_two_groups_no_bitmap() {
        // g0: ref 10, width 3, length 2, X = [1, 2] → scaled [11, 12].
        // g1: ref 100, width 4, length 3 (from group_length_last), X =
        //     [0, 5, 15] → scaled [100, 105, 115].
        let t = ComplexPackingTemplate {
            num_groups: 2,
            group_length_last: 3,
            ..complex_template_base()
        };
        let payload = pack_fields(&[
            (10, 8),
            (100, 8), // group references
            (3, 4),
            (4, 4), // stored group widths (reference 0)
            (2, 8),
            (0, 8), // stored group lengths — g1's is overridden by last = 3
            (1, 3),
            (2, 3), // g0 data
            (0, 4),
            (5, 4),
            (15, 4), // g1 data
        ]);
        let decoded = decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 5)
            .expect("decode");
        assert_eq!(
            decoded,
            vec![
                Some(11.0),
                Some(12.0),
                Some(100.0),
                Some(105.0),
                Some(115.0)
            ],
        );
    }

    #[test]
    fn complex_packing_applies_reference_and_scale_factors() {
        // R = 10, E = 1 (×2), D = 1 (×0.1): value = (10 + scaled·2)·0.1.
        // One group, ref 5, width 4, length 4, X = [0, 1, 2, 3] →
        // scaled [5, 6, 7, 8] → values [2.0, 2.2, 2.4, 2.6].
        let t = ComplexPackingTemplate {
            reference_value: 10.0,
            binary_scale_factor: 1,
            decimal_scale_factor: 1,
            num_groups: 1,
            group_length_last: 4,
            ..complex_template_base()
        };
        let payload = pack_fields(&[
            (5, 8), // group reference
            (4, 4), // stored group width
            (0, 8), // stored group length — overridden by last = 4
            (0, 4),
            (1, 4),
            (2, 4),
            (3, 4), // data
        ]);
        let decoded = decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 4)
            .expect("decode");
        let got: Vec<f64> = decoded.into_iter().map(Option::unwrap).collect();
        for (g, w) in got.iter().zip([2.0, 2.2, 2.4, 2.6]) {
            assert!((g - w).abs() < 1e-9, "got {g}, want {w}");
        }
    }

    #[test]
    fn complex_packing_zero_width_group_is_constant() {
        // g0: ref 42, width 0, length 3 → every point = 42 (no data bits).
        // g1: ref 7, width 2, length 2, X = [1, 3] → [8, 10].
        let t = ComplexPackingTemplate {
            num_groups: 2,
            group_length_last: 2,
            ..complex_template_base()
        };
        let payload = pack_fields(&[
            (42, 8),
            (7, 8), // references
            (0, 4),
            (2, 4), // widths — g0 is zero-width
            (3, 8),
            (0, 8), // lengths — g1 overridden by last = 2
            (1, 2),
            (3, 2), // g1 data only (g0 contributes no data bits)
        ]);
        let decoded = decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 5)
            .expect("decode");
        assert_eq!(
            decoded,
            vec![Some(42.0), Some(42.0), Some(42.0), Some(8.0), Some(10.0)],
        );
    }

    #[test]
    fn complex_packing_honours_bitmap() {
        // One group of 3 present values spread across a 5-point grid.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 3,
            ..complex_template_base()
        };
        let payload = pack_fields(&[
            (0, 8), // reference
            (8, 4), // width 8
            (0, 8), // length overridden by last = 3
            (1, 8),
            (2, 8),
            (3, 8), // data
        ]);
        let bitmap = [true, false, true, false, true];
        let decoded = decode_values(
            &payload,
            DataRepresentationTemplate::Complex(t),
            Some(&bitmap),
            5,
        )
        .expect("decode");
        assert_eq!(decoded, vec![Some(1.0), None, Some(2.0), None, Some(3.0)]);
    }

    #[test]
    fn complex_packing_rejects_missing_value_management() {
        let t = ComplexPackingTemplate {
            num_groups: 1,
            missing_value_management: 1,
            ..complex_template_base()
        };
        let err = decode_values(&[0u8; 8], DataRepresentationTemplate::Complex(t), None, 1)
            .expect_err("must reject");
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("missing-value management"), "msg: {msg}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

    #[test]
    fn complex_packing_rejects_row_by_row_splitting() {
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_splitting_method: 0,
            ..complex_template_base()
        };
        let err = decode_values(&[0u8; 8], DataRepresentationTemplate::Complex(t), None, 1)
            .expect_err("must reject");
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("group splitting method"), "msg: {msg}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

    #[test]
    fn complex_packing_rejects_group_length_sum_mismatch() {
        // Group lengths sum to 2 but the caller asks for 5 values.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 2,
            ..complex_template_base()
        };
        let payload = pack_fields(&[(0, 8), (8, 4), (0, 8), (1, 8), (2, 8)]);
        assert!(decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 5).is_err());
    }

    #[test]
    fn complex_packing_rejects_more_groups_than_points() {
        // NG = 4 but only 2 present points — impossible (each group ≥ 1 point).
        let t = ComplexPackingTemplate {
            num_groups: 4,
            ..complex_template_base()
        };
        assert!(
            decode_values(&[0u8; 16], DataRepresentationTemplate::Complex(t), None, 2).is_err()
        );
    }

    #[test]
    fn complex_packing_zero_groups_requires_empty_field() {
        // NG = 0 with a non-empty grid is malformed.
        let t = complex_template_base(); // num_groups = 0
        assert!(decode_values(&[], DataRepresentationTemplate::Complex(t), None, 4).is_err());
    }

    #[test]
    fn complex_packing_rejects_oversized_group_width_without_panicking() {
        // group_width_bits = 32 with a near-max stored width: the actual width
        // (reference + stored) must be computed in wide arithmetic and rejected
        // cleanly, not overflow-panic on the addition (a fuzz-reachable input).
        let t = ComplexPackingTemplate {
            num_groups: 1,
            bits_per_value: 8,
            group_width_reference: 1,
            group_width_bits: 32,
            group_length_last: 1,
            ..complex_template_base()
        };
        let payload = pack_fields(&[
            (0, 8),            // group reference
            (0xFFFF_FFFF, 32), // stored group width → actual width 0x1_0000_0000
            (0, 8),            // stored group length (overridden by last = 1)
        ]);
        let err = decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 1)
            .expect_err("must reject oversized width");
        assert!(err.to_string().contains("exceeds 32 bits"), "got: {err}");
    }

    fn ieee_template(precision: u8) -> DataRepresentationTemplate {
        DataRepresentationTemplate::Ieee(crate::drs::IeeePackingTemplate { precision })
    }

    #[test]
    fn ieee_packing_decodes_32bit_no_bitmap() {
        let values = [-1.5f32, 0.0, 273.15, 1e6];
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        let decoded = decode_values(&payload, ieee_template(1), None, 4).expect("decode");
        let got: Vec<f64> = decoded.into_iter().map(|v| v.unwrap()).collect();
        for (g, w) in got.iter().zip(values.iter()) {
            assert!((g - *w as f64).abs() < 1e-6, "got {g}, want {w}");
        }
    }

    #[test]
    fn ieee_packing_decodes_64bit_no_bitmap() {
        let values = [-1.5f64, 0.0, 273.15, 1e12, f64::MAX];
        let payload: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        let decoded = decode_values(&payload, ieee_template(2), None, 5).expect("decode");
        let got: Vec<f64> = decoded.into_iter().map(|v| v.unwrap()).collect();
        assert_eq!(got, values);
    }

    #[test]
    fn ieee_packing_honours_bitmap() {
        // Two present 32-bit values spread across a 4-point grid.
        let payload: Vec<u8> = [10.0f32, 20.0]
            .iter()
            .flat_map(|v| v.to_be_bytes())
            .collect();
        let bitmap = [false, true, false, true];
        let decoded = decode_values(&payload, ieee_template(1), Some(&bitmap), 4).expect("decode");
        assert_eq!(decoded, vec![None, Some(10.0), None, Some(20.0)]);
    }

    #[test]
    fn ieee_packing_rejects_128bit_precision() {
        let err = decode_values(&[0u8; 16], ieee_template(3), None, 1).expect_err("must reject");
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("5.4"), "names template: {msg}");
                assert!(msg.contains("128"), "names 128-bit: {msg}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

    #[test]
    fn ieee_packing_rejects_short_payload() {
        // Need 2 × 8 bytes for two 64-bit values; supply only 8.
        let payload = [0u8; 8];
        assert!(decode_values(&payload, ieee_template(2), None, 2).is_err());
    }

    #[test]
    fn ieee_packing_rejects_bitmap_length_mismatch() {
        let payload: Vec<u8> = 1.0f32.to_be_bytes().to_vec();
        let bitmap = [true]; // claims 1 point, caller asks for 5
        assert!(decode_values(&payload, ieee_template(1), Some(&bitmap), 5).is_err());
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

    #[test]
    fn parse_data_section_body_rejects_length_below_header() {
        // Section header parser refuses a declared length below the 5-byte
        // preamble, so the §7-specific length check is only reachable when
        // we hand-build a `SectionHeader` with a too-small length. This pins
        // the defensive guard in `parse_data_section_body`.
        let buf = [0u8; 5];
        let header = SectionHeader {
            length: 4,
            number: DS_SECTION_NUMBER,
        };
        let err = parse_data_section_body(&buf, header).expect_err("must reject");
        assert!(
            err.to_string().contains("DS section length"),
            "error names DS length shortfall, got: {err}",
        );
    }

    #[test]
    fn parse_data_section_body_rejects_length_exceeding_buffer() {
        // Declared length is fine on its own but the supplied buffer is
        // shorter — the bounds check must reject before we slice the body.
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&100u32.to_be_bytes());
        buf.push(DS_SECTION_NUMBER);
        let header = parse_section_header(&buf).unwrap();
        let err = parse_data_section_body(&buf, header).expect_err("must reject");
        assert!(
            err.to_string().contains("DS declares length"),
            "error names declared-length overshoot, got: {err}",
        );
    }
}
