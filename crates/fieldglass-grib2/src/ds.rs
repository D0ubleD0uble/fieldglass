//! GRIB2 Data Section (§7) — decoders for every supported §5 packing.
//!
//! §7 is a thin wrapper around the packed bytes: 5 bytes of section header
//! (length + section number = 7) followed by the packed payload. All
//! decoding rules live in the [`DataRepresentationSection`](crate::drs::DataRepresentationSection)
//! (§5); this module turns the payload into one `Option<f64>` per grid point.
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 7 + Template 5.0 decoding formula.

use crate::drs::{
    CcsdsPackingTemplate, ComplexPackingTemplate, ComplexSpatialDiffTemplate,
    DataRepresentationTemplate, IeeePackingTemplate, Jpeg2000PackingTemplate,
    LogPreprocessingPackingTemplate, PngPackingTemplate, RunLengthPackingTemplate,
    SecondOrderPackingTemplate, SimplePackingTemplate,
};
use crate::section::{SECTION_HEADER_LEN, SectionHeader};
use fieldglass_core::{
    FieldglassError,
    bits::{BitReader, apply_spd_inverse, sign_magnitude_to_i64},
};

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
        DataRepresentationTemplate::MatrixSimple(t) => {
            if t.matrix_bitmaps_present != 0 {
                // A genuine per-point NR×NC matrix delimited by secondary
                // bitmaps is not one value per grid point, so it cannot satisfy
                // this scalar contract. eccodes cannot decode this variant
                // either (its accessor asserts out), so there is no oracle; it
                // is left for a dedicated matrix entry point.
                return Err(FieldglassError::UnsupportedSection(format!(
                    "§7 is a grid_simple_matrix field with matrixBitmapsPresent=1 (NR={}, NC={}): \
                     a per-grid-point {}×{} matrix delimited by secondary bitmaps is not a single \
                     2-D field, and is not decoded yet.",
                    t.nr, t.nc, t.nr, t.nc
                )));
            }
            // matrixBitmapsPresent = 0: §7 is plain simple packing, one value
            // per grid point (NR/NC are descriptive metadata) — decode it
            // exactly like template 5.0.
            let simple = SimplePackingTemplate {
                reference_value: t.reference_value,
                binary_scale_factor: t.binary_scale_factor,
                decimal_scale_factor: t.decimal_scale_factor,
                bits_per_value: t.bits_per_value,
                original_field_type: 0,
            };
            decode_simple_packing(ds_payload, &simple, bitmap, expected_count)
        }
        DataRepresentationTemplate::Complex(t) => {
            decode_complex_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::ComplexSpatialDiff(t) => {
            decode_complex_spatial_diff(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::Ieee(t) => {
            decode_ieee_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::Png(t) => {
            decode_png_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::Ccsds(t) => {
            decode_ccsds_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::Jpeg2000(t) => {
            decode_jpeg2000_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::RunLength(t) => {
            decode_run_length_packing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::LogPreprocessing(t) => {
            decode_log_preprocessing(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::SecondOrder(t) => {
            decode_second_order(ds_payload, &t, bitmap, expected_count)
        }
        DataRepresentationTemplate::SpectralSimple(_)
        | DataRepresentationTemplate::SpectralComplex(_) => {
            Err(FieldglassError::UnsupportedSection(
                "§7 holds spherical-harmonic coefficients (template 5.50 / 5.51), which are not \
                 values on a grid — decode them with `Grib2Reader::decode_spectral_message`. \
                 Rendering one as a 2-D field needs an inverse spherical-harmonic transform, \
                 which is not implemented yet."
                    .to_string(),
            ))
        }
        DataRepresentationTemplate::BiFourier(_) => Err(FieldglassError::UnsupportedSection(
            "§7 holds bi-Fourier spectral coefficients (template 5.53), which are not values \
             on a grid — decode them with `Grib2Reader::decode_bifourier_message`. Rendering \
             one as a 2-D field needs an inverse bi-Fourier transform, which is not \
             implemented yet."
                .to_string(),
        )),
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

/// Decode simple packing with logarithmic pre-processing (template 5.61). §7 is
/// an ordinary simple-packing integer stream, so the decode is
/// [`decode_simple_packing`] to recover the log-domain value
/// `X = (R + packed · 2^E) · 10^-D`, followed by the inverse transform
/// `Y = exp(X) - B`, where `B` is the pre-processing parameter. `B == 0`
/// reduces to `Y = exp(X)`; the subtraction is applied unconditionally since
/// subtracting zero is a no-op, matching eccodes'
/// `DataG2SimplePackingWithPreprocessing`. Missing (bitmap) points pass through
/// untouched, the same seam every other decoder uses.
fn decode_log_preprocessing(
    ds_payload: &[u8],
    t: &LogPreprocessingPackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    // The §7 payload is packed exactly like template 5.0, so borrow that
    // decoder for the log-domain values. Template 5.61 has no
    // type-of-original-values octet; simple packing ignores that field anyway,
    // so a placeholder is harmless.
    let simple = SimplePackingTemplate {
        reference_value: t.reference_value,
        binary_scale_factor: t.binary_scale_factor,
        decimal_scale_factor: t.decimal_scale_factor,
        bits_per_value: t.bits_per_value,
        original_field_type: 0,
    };
    let log_domain = decode_simple_packing(ds_payload, &simple, bitmap, expected_count)?;

    let bias = t.pre_processing_parameter as f64;
    Ok(log_domain
        .into_iter()
        .map(|v| v.map(|x| x.exp() - bias))
        .collect())
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
/// Both splitting methods and inline missing-value management (1: primary
/// substitutes; 2: primary + secondary) are supported — substituted points
/// come back as `None`, the same seam the §6 bitmap uses, so rendering
/// needs no packing-specific handling. See [`decode_complex_groups`].
///
/// `NG == 0` is a constant field — see [`complex_ng0_constant_field`].
fn decode_complex_packing(
    ds_payload: &[u8],
    t: &ComplexPackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let present_count = check_bitmap_present_count(bitmap, expected_count)?;

    if let Some(constant) = complex_ng0_constant_field(t, bitmap, expected_count) {
        return Ok(constant);
    }

    let mut reader = BitReader::new(ds_payload);
    let scaled = decode_complex_groups(&mut reader, t, present_count)?;

    Ok(complex_scaled_to_values(t, scaled, bitmap, expected_count))
}

/// The `NG == 0` constant-field rule shared by 5.2 and 5.3 (eccodes
/// ECC-2095, `DataG22OrderPacking::unpack`): `Some` when the template
/// declares zero groups, with every present point equal to the reference
/// value `R` verbatim — no `2^E · 10^-D` transform, nothing read from §7.
fn complex_ng0_constant_field(
    t: &ComplexPackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Option<Vec<Option<f64>>> {
    (t.num_groups == 0)
        .then(|| materialise_constant(t.reference_value as f64, bitmap, expected_count))
}

/// Shared 5.2 / 5.3 tail: apply the `R`/`E`/`D` transform to the expanded
/// scaled integers (missing points pass through as `None`) and spread the
/// result across the grid per the §6 bitmap.
fn complex_scaled_to_values(
    t: &ComplexPackingTemplate,
    scaled: Vec<Option<i64>>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));
    let decoded = scaled
        .into_iter()
        .map(|s| s.map(|s| (r + s as f64 * two_pow_e) * d_inv));
    interleave_present_points(decoded, bitmap, expected_count)
}

/// Decode complex packing with spatial differencing (template 5.3). The packed
/// integers are 1st- or 2nd-order spatial *differences* of the scaled field,
/// reduced by their overall minimum so they group as non-negative values
/// (mirroring the GRIB1 SPD orders). §7 opens with the spatial-differencing
/// *extra descriptors* — the first `order` original scaled values, then the
/// (sign-magnitude) overall minimum difference, each in
/// `extra_descriptor_octets` octets — ahead of the normal complex-packing
/// blocks decoded by [`decode_complex_groups`].
///
/// After group expansion yields the reduced differences `d`, the original
/// scaled integers are recovered by reversing the differencing
/// (`bias` = overall minimum):
/// - order 1: `g[0] = ival1`; `g[i] = d[i] + g[i-1] + bias`.
/// - order 2: `g[0] = ival1`, `g[1] = ival2`;
///   `g[i] = d[i] + 2·g[i-1] − g[i-2] + bias`.
///
/// Points marked missing by inline missing-value management take no part in
/// the recurrence: the seed values fill the first `order` *non-missing*
/// slots, and each later non-missing point recurses on the nearest previous
/// non-missing values (eccodes `DataG22OrderPacking` post-process). A field
/// with fewer non-missing points than the differencing order simply seeds
/// what exists, as eccodes does.
///
/// The `R`/`E`/`D` transform then applies as in simple/complex packing.
/// Missing-value handling is shared with 5.2 via [`decode_complex_groups`].
///
/// `NG == 0` is a constant field — see [`complex_ng0_constant_field`].
/// eccodes detects it before validating the differencing order and reads
/// nothing from §7, not even the extra descriptors, so the check sits ahead
/// of both here too.
fn decode_complex_spatial_diff(
    ds_payload: &[u8],
    t: &ComplexSpatialDiffTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let present_count = check_bitmap_present_count(bitmap, expected_count)?;

    if let Some(constant) = complex_ng0_constant_field(&t.complex, bitmap, expected_count) {
        return Ok(constant);
    }

    let order = t.spatial_diff_order;
    if order != 1 && order != 2 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "DRS template 5.3 spatial-differencing order {order} is not supported \
             (only first-order (1) and second-order (2), WMO Code Table 5.6)"
        )));
    }
    // Each extra descriptor is read whole via `BitReader::read_bits`, capped at
    // 32 bits; the signed minimum also needs ≥1 magnitude bit. Real files use
    // 1–4 octets, so anything outside that is malformed.
    let octets = t.extra_descriptor_octets;
    if octets == 0 || octets > 4 {
        return Err(FieldglassError::Parse(format!(
            "complex packing: spatial-differencing extra-descriptor width {octets} octets \
             is out of the supported 1..=4 range"
        )));
    }
    let descriptor_bits = octets * 8;

    let mut reader = BitReader::new(ds_payload);

    // Spatial-differencing extra descriptors, ahead of the group blocks: the
    // first `order` original scaled values (unsigned), then the overall
    // minimum difference (sign-magnitude). Each occupies `descriptor_bits`,
    // a whole number of octets, so the group-reference block stays aligned.
    let ival1 = reader.read_bits(descriptor_bits)? as i64;
    let ival2 = if order == 2 {
        reader.read_bits(descriptor_bits)? as i64
    } else {
        0
    };
    let bias = sign_magnitude_to_i64(reader.read_bits(descriptor_bits)?, descriptor_bits);

    let mut vals = decode_complex_groups(&mut reader, &t.complex, present_count)?;

    // Reverse the differencing in wide wrapping arithmetic — a malformed
    // descriptor could otherwise overflow the accumulation and panic in debug.
    // Missing slots are skipped: the seeds land on the first `order`
    // non-missing slots and the recurrence tracks the nearest previous
    // non-missing values, mirroring eccodes' post-process. A field with fewer
    // non-missing points than `order` just seeds what exists.
    match order {
        1 => {
            let mut last: Option<i64> = None;
            for slot in vals.iter_mut() {
                let Some(d) = slot.as_mut() else { continue };
                *d = match last {
                    None => ival1,
                    Some(prev) => d.wrapping_add(prev).wrapping_add(bias),
                };
                last = Some(*d);
            }
        }
        // order == 2 (the only other value past the guard above).
        _ => {
            let (mut penultimate, mut last): (Option<i64>, Option<i64>) = (None, None);
            for slot in vals.iter_mut() {
                let Some(d) = slot.as_mut() else { continue };
                *d = match (last, penultimate) {
                    (None, _) => ival1,
                    (Some(_), None) => ival2,
                    (Some(l), Some(p)) => d
                        .wrapping_add(l.wrapping_mul(2))
                        .wrapping_sub(p)
                        .wrapping_add(bias),
                };
                (penultimate, last) = (last, Some(*d));
            }
        }
    }

    Ok(complex_scaled_to_values(
        &t.complex,
        vals,
        bitmap,
        expected_count,
    ))
}

/// Validate the §6 bitmap against the grid-point count and return the number
/// of present points the §7 payload must carry (the whole grid when there is
/// no bitmap). Shared by every packing decoder.
fn check_bitmap_present_count(
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<usize, FieldglassError> {
    match bitmap {
        Some(b) if b.len() != expected_count => Err(FieldglassError::Parse(format!(
            "bitmap length {} != grid-point count {expected_count}",
            b.len()
        ))),
        Some(b) => Ok(b.iter().filter(|p| **p).count()),
        None => Ok(expected_count),
    }
}

/// Expand the complex-packing §7 group structure into one scaled integer
/// (`group_ref[g] + X`) per present point — `None` for points marked missing
/// by inline missing-value management. `reader`'s bit cursor must sit at
/// the start of the group-reference block — at the very front of §7 for plain
/// complex packing (5.2), or just past the spatial-differencing extra
/// descriptors for 5.3. Shared by both decoders; the caller applies the
/// `R`/`E`/`D` transform (and, for 5.3, the inverse differencing).
///
/// Missing points are flagged by all-ones sentinels, not by the template's
/// substitute values (eccodes `DataG22OrderPacking::unpack`): in a zero-width
/// group, the group *reference* equal to `2^bits_per_value − 1` marks the
/// whole group missing; in a wider group, a per-point offset equal to
/// `2^width − 1` marks that point missing. Management 2 additionally treats
/// sentinel − 1 (the secondary substitute) as missing; primary and secondary
/// both decode to `None`. Splitting method (Code Table 5.4) does not affect
/// decoding — the group structure is self-describing whether the encoder
/// split row by row (0) or generally (1) — so both decode on this one path,
/// as in eccodes. Reserved values of either field surface as
/// [`FieldglassError::UnsupportedSection`]. Callers must intercept
/// `NG == 0` (the ECC-2095 constant-field case) before calling; it is an
/// error here.
fn decode_complex_groups(
    reader: &mut BitReader,
    t: &ComplexPackingTemplate,
    present_count: usize,
) -> Result<Vec<Option<i64>>, FieldglassError> {
    if t.group_splitting_method > 1 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "DRS complex packing group splitting method {} is not supported \
             (Code Table 5.4 defines only 0, row by row, and 1, general)",
            t.group_splitting_method
        )));
    }
    if t.missing_value_management > 2 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "DRS complex packing missing-value management {} is not supported \
             (Code Table 5.5 defines only 0, none; 1, primary; 2, primary + secondary)",
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

    let num_groups = t.num_groups as usize;
    // NG == 0 is the constant-field case (ECC-2095) and both callers
    // intercept it before any §7 read; reaching here with 0 groups is a
    // caller bug, kept as an error so the `last_mut` below can't panic.
    if num_groups == 0 {
        return Err(FieldglassError::Parse(
            "complex packing: group expansion invoked with 0 groups".into(),
        ));
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

    // The four §7 sub-blocks (group references, widths, lengths, then the data
    // values) each begin on an octet boundary, so we realign after every one.
    // eccodes does the same: each `buf_*` pointer is advanced by the previous
    // block's *byte* size, `ceil(bits / 8)` (DataG22OrderPacking::unpack).
    // Without this, a block whose bit length isn't a multiple of 8 leaves the
    // cursor mid-byte and every following block is misread.

    // Block 1: group reference values.
    let mut group_refs = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        group_refs.push(reader.read_bits(t.bits_per_value)?);
    }
    reader.align_to_byte();

    // Block 2: group widths (stored value offset by the width reference).
    // Computed in u64 so the reference + a 32-bit stored value can't overflow
    // before the `> 32` range check in the data loop sees it.
    let mut group_widths: Vec<u64> = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        let stored = reader.read_bits(t.group_width_bits)?;
        group_widths.push(t.group_width_reference as u64 + stored as u64);
    }
    reader.align_to_byte();

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

    // Block 4: the per-point offsets, decoded group by group. Starts on the
    // octet boundary after the group-length block.
    reader.align_to_byte();

    // Missing-value classification per Code Table 5.5 (eccodes
    // `DataG22OrderPacking::unpack`): the all-ones value at the given field
    // width is the primary substitute; management 2 adds all-ones − 1 as the
    // secondary. Both decode to missing. `field_bits <= 32` at every call
    // site, so the u64 shift can't overflow.
    let mvm = t.missing_value_management;
    let is_missing = |raw: i64, field_bits: u8| {
        let sentinel = ((1u64 << field_bits) - 1) as i64;
        match mvm {
            1 => raw == sentinel,
            2 => raw == sentinel || raw == sentinel - 1,
            _ => false,
        }
    };

    let mut scaled = Vec::with_capacity(present_count);
    for g in 0..num_groups {
        let width = group_widths[g];
        let group_ref = group_refs[g] as i64;
        if width == 0 {
            // Zero-width group: no per-point offsets are stored. Every point
            // equals the group reference — unless the reference is the
            // missing sentinel at `bits_per_value`, which marks the whole
            // group missing.
            let value = (!is_missing(group_ref, t.bits_per_value)).then_some(group_ref);
            for _ in 0..group_lengths[g] {
                scaled.push(value);
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
                let x = reader.read_bits(width as u8)? as i64;
                let value = (!is_missing(x, width as u8)).then_some(group_ref + x);
                scaled.push(value);
            }
        }
    }

    Ok(scaled)
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

/// Decode PNG packing (template 5.41). §7 carries a complete PNG image whose
/// pixels are the packed integers `X`; after the image is decoded, the value
/// transform is the simple-packing formula `value = (R + X · 2^E) · 10^-D`.
///
/// eccodes (`grid_png`) selects the PNG sample layout from `bits_per_value` —
/// 8-bit grayscale (≤8), 16-bit grayscale (≤16), 8-bit RGB (≤24), or 8-bit
/// RGBA (≤32) — and writes one value per pixel in raster order, with the
/// value's bytes laid out most-significant-first across the pixel's
/// samples/channels. So reading the decoded buffer as `ceil(bits/8)`-byte
/// big-endian groups recovers `X` regardless of colour type, mirroring
/// eccodes' own unpack. The image carries exactly `present_count` pixels.
fn decode_png_packing(
    ds_payload: &[u8],
    t: &PngPackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let present_count = check_bitmap_present_count(bitmap, expected_count)?;

    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));

    // Constant field: no PNG stream is written; every present point is R·10^-D.
    if t.bits_per_value == 0 {
        let constant = r * d_inv;
        return Ok(materialise_constant(constant, bitmap, expected_count));
    }
    if t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "PNG packing: bits_per_value {} exceeds 32",
            t.bits_per_value
        )));
    }
    let bytes_per_value = t.bits_per_value.div_ceil(8) as usize;

    // Parse only the PNG header first; this lets us reject an image whose pixel
    // count disagrees with the field before allocating the output buffer, so a
    // crafted IHDR on an untrusted file can't drive a huge allocation.
    // png 0.18's `Decoder` needs `BufRead + Seek`; a `Cursor` over the §7
    // bytes provides both without copying.
    let mut reader = png::Decoder::new(std::io::Cursor::new(ds_payload))
        .read_info()
        .map_err(|e| FieldglassError::Parse(format!("PNG packing: invalid PNG stream: {e}")))?;
    let info = reader.info();
    let pixels = (info.width as u64)
        .checked_mul(info.height as u64)
        .ok_or_else(|| FieldglassError::Parse("PNG packing: image dimensions overflow".into()))?;
    if pixels != present_count as u64 {
        return Err(FieldglassError::Parse(format!(
            "PNG packing: image holds {pixels} pixels but {present_count} values are required"
        )));
    }

    let buf_size = reader.output_buffer_size().ok_or_else(|| {
        FieldglassError::Parse("PNG packing: decoded image size overflows usize".into())
    })?;
    let mut buf = vec![0u8; buf_size];
    let out = reader
        .next_frame(&mut buf)
        .map_err(|e| FieldglassError::Parse(format!("PNG packing: decode failed: {e}")))?;
    let data = &buf[..out.buffer_size()];

    // Each value occupies `bytes_per_value` bytes, so the decoded buffer must
    // hold exactly that many bytes per pixel. A mismatch means the PNG used an
    // unexpected colour type / sample depth (e.g. a palette image), which we
    // reject rather than misread. (`present_count * bytes_per_value` can't
    // overflow: present_count is capped upstream and bytes_per_value ≤ 4.)
    if data.len() != present_count * bytes_per_value {
        return Err(FieldglassError::Parse(format!(
            "PNG packing: decoded {} bytes for {present_count} values at {bytes_per_value} \
             bytes each",
            data.len()
        )));
    }

    let mut decoded = Vec::with_capacity(present_count);
    for chunk in data.chunks_exact(bytes_per_value) {
        let mut x: u64 = 0;
        for &b in chunk {
            x = (x << 8) | b as u64;
        }
        decoded.push((r + x as f64 * two_pow_e) * d_inv);
    }

    Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
}

/// Decode CCSDS / AEC packing (template 5.42). §7 carries a CCSDS-121.0-B
/// adaptive-entropy-coding (libaec-compatible) bitstream whose decoded samples
/// are the packed integers `X`; after decompression the value transform is the
/// simple-packing formula `value = (R + X · 2^E) · 10^-D`.
///
/// The AEC codec is the pure-Rust [`rust_aec`] crate (no C dependency, so the
/// six-target `.vsix` cross-compile is preserved); see ADR-0001. Any decoder
/// error — a malformed stream, or a flag/parameter combination `rust_aec`
/// doesn't cover — is surfaced as [`FieldglassError::UnsupportedSection`] so an
/// untrusted file degrades gracefully (the message is reported undecodable)
/// rather than crashing the addon.
///
/// Flag handling mirrors eccodes' `grid_ccsds` (`modify_aec_flags`): the `MSB`
/// and `DATA_3BYTE` flags only govern how `rust_aec` serialises decoded
/// samples to bytes, not the entropy-decoded integer `X`. We pin them — force
/// `MSB` on (samples big-endian) and `DATA_3BYTE` off (17–24-bit samples in 4
/// bytes) — so a fixed `ceil(bits/8)`-byte big-endian read recovers `X`
/// regardless of the file's stored byte-order bit. The `PREPROCESS` /
/// `SIGNED` / `RESTRICTED` / `PAD_RSI` flags, which do drive the decode, are
/// honoured as stored. Like eccodes, the decoded samples are read as unsigned
/// offsets from `R`.
fn decode_ccsds_packing(
    ds_payload: &[u8],
    t: &CcsdsPackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let present_count = check_bitmap_present_count(bitmap, expected_count)?;

    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));

    // Constant field: bitsPerValue == 0 means §7 is empty and every present
    // point equals the reference value verbatim. This matches eccodes'
    // `grid_ccsds` unpack, which returns `R` directly (without the 10^-D
    // factor that simple/PNG packing apply) because its encoder stores the
    // already-scaled physical value in `R`. The two agree whenever D == 0,
    // which is the case for essentially every constant field.
    if t.bits_per_value == 0 {
        return Ok(materialise_constant(r, bitmap, expected_count));
    }
    if t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "CCSDS packing: bits_per_value {} exceeds 32",
            t.bits_per_value
        )));
    }
    // Output sample width with `DATA_3BYTE` forced off: 17–24-bit samples
    // occupy 4 bytes, matching `rust_aec`'s serialisation under the pinned
    // flags below. (`present_count * bytes_per_value` can't overflow:
    // present_count is capped upstream and bytes_per_value ≤ 4.)
    let bytes_per_value: usize = match t.bits_per_value {
        1..=8 => 1,
        9..=16 => 2,
        _ => 4,
    };

    let mut flags = rust_aec::flags_from_grib2_ccsds_flags(t.ccsds_flags);
    flags.insert(rust_aec::AecFlags::MSB);
    flags.remove(rust_aec::AecFlags::DATA_3BYTE);
    let params = rust_aec::AecParams::new(
        t.bits_per_value,
        t.block_size as u32,
        t.reference_sample_interval as u32,
        flags,
    );

    let data = rust_aec::decode(ds_payload, params, present_count).map_err(|e| {
        FieldglassError::UnsupportedSection(format!("CCSDS packing: AEC decode failed: {e}"))
    })?;

    // `rust_aec` returns exactly `present_count * bytes_per_value` bytes under
    // the pinned flags; verify before slicing so a contract change surfaces as
    // a clean error rather than a panic.
    if data.len() != present_count * bytes_per_value {
        return Err(FieldglassError::Parse(format!(
            "CCSDS packing: decoded {} bytes for {present_count} values at {bytes_per_value} \
             bytes each",
            data.len()
        )));
    }

    let mut decoded = Vec::with_capacity(present_count);
    for chunk in data.chunks_exact(bytes_per_value) {
        let mut x: u64 = 0;
        for &b in chunk {
            x = (x << 8) | b as u64;
        }
        decoded.push((r + x as f64 * two_pow_e) * d_inv);
    }

    Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
}

/// Decode JPEG 2000 packing (template 5.40). §7 carries a JPEG 2000 codestream
/// (ISO/IEC 15444-1 Annex A, no JP2 boxes) whose decoded single-component
/// samples are the packed integers `X`; after decompression the value transform
/// is the simple-packing formula `value = (R + X · 2^E) · 10^-D`, identical to
/// PNG (5.41) / CCSDS (5.42).
///
/// The codec is the pure-Rust [`rust_j2k`] crate (no C dependency, so the
/// six-target `.vsix` cross-compile is preserved); see ADR-0001. Any decoder
/// error — a malformed codestream, or a JPEG 2000 feature `rust_j2k` doesn't
/// cover yet — is surfaced as [`FieldglassError::UnsupportedSection`] so an
/// untrusted file degrades gracefully (the message is reported undecodable)
/// rather than crashing the addon.
///
/// `rust_j2k` returns samples already DC-level-shifted into the unsigned range
/// `[0, 2^bits-1]`, matching eccodes' `grid_jpeg`, which reads them as unsigned
/// offsets from `R`. The reversible 5/3 (lossless) and irreversible 9/7 (lossy)
/// wavelet paths are selected by the codestream itself, so the decode does not
/// branch on the template's `type_of_compression_used`.
fn decode_jpeg2000_packing(
    ds_payload: &[u8],
    t: &Jpeg2000PackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let present_count = check_bitmap_present_count(bitmap, expected_count)?;

    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));

    // Constant field: bitsPerValue == 0 means §7 is empty and every present
    // point equals the reference value verbatim, matching eccodes' `grid_jpeg`
    // unpack (which returns `R` directly, without the 10^-D factor).
    if t.bits_per_value == 0 {
        return Ok(materialise_constant(r, bitmap, expected_count));
    }
    if t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "JPEG 2000 packing: bits_per_value {} exceeds 32",
            t.bits_per_value
        )));
    }

    let image = rust_j2k::decode(ds_payload).map_err(|e| {
        FieldglassError::UnsupportedSection(format!("JPEG 2000 packing: decode failed: {e}"))
    })?;

    // Operational `grid_jpeg` always stores an unsigned component, and eccodes
    // reads the samples as unsigned offsets from `R`. A signed component would
    // make `rust_j2k` return negative samples, which the unsigned-offset
    // transform below would silently misread — so reject it rather than guess.
    if image.signed {
        return Err(FieldglassError::UnsupportedSection(
            "JPEG 2000 packing: signed component is unsupported".into(),
        ));
    }

    // The codestream must hold exactly one sample per present point; a mismatch
    // means the §7 geometry disagrees with the field, which we reject rather
    // than misread.
    if image.samples.len() != present_count {
        return Err(FieldglassError::Parse(format!(
            "JPEG 2000 packing: codestream holds {} samples but {present_count} values are required",
            image.samples.len()
        )));
    }

    // Samples are non-negative unsigned offsets `X` (the encoder stores an
    // unsigned component, and `rust_j2k` level-shifts unsigned components back
    // into `[0, 2^bits-1]`). Read them as such, mirroring eccodes.
    let mut decoded = Vec::with_capacity(present_count);
    for &x in &image.samples {
        decoded.push((r + x as f64 * two_pow_e) * d_inv);
    }

    Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
}

/// Decode run-length packing (template 5.200). §7 is a stream of
/// `bits_per_value`-wide MSB-first codes. A code `v <= max_level_value` opens
/// a run of level `v` (level `0` = missing); any immediately following codes
/// greater than `max_level_value` are base-`range` run-length digits
/// (least-significant first) that extend that run, where
/// `range = 2^bits_per_value - 1 - max_level_value`. A level `v >= 1` resolves
/// through the level-value table to `level_values[v - 1] · 10^-D`; level `0`
/// and bitmap-masked points come back as `None` — the same missing seam every
/// other decoder uses. There is no `R`/`E` transform.
///
/// This mirrors eccodes' `DataRunLengthPacking::unpack_double`, including its
/// treatment of a zero max level or an empty code stream as a wholly-missing
/// field.
fn decode_run_length_packing(
    ds_payload: &[u8],
    t: &RunLengthPackingTemplate,
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

    let bits = t.bits_per_value;
    // A zero max level, or a §7 with no whole code in it, is a wholly-missing
    // field. eccodes short-circuits both of these *before* validating the
    // template, so a degenerate template (bad range / level count / bit width)
    // with an empty code stream still decodes to all-missing rather than
    // erroring — we match that ordering. The `bits == 0` guard comes first so
    // the code-count division below can't divide by zero.
    if bits == 0 || t.max_level_value == 0 {
        return Ok(vec![None; expected_count]);
    }
    let num_codes = (ds_payload.len() * 8) / bits as usize;
    if num_codes == 0 {
        return Ok(vec![None; expected_count]);
    }

    // From here the stream carries at least one code, so the template must be
    // valid. These checks only run in that case, mirroring eccodes.
    if bits > 32 {
        return Err(FieldglassError::Parse(format!(
            "run-length packing: bits_per_value {bits} exceeds 32"
        )));
    }
    if t.max_level_value > t.number_of_level_values {
        return Err(FieldglassError::Parse(format!(
            "run-length packing: max_level_value {} exceeds number_of_level_values {}",
            t.max_level_value, t.number_of_level_values
        )));
    }

    let max = t.max_level_value as u64;
    // `range` (the base for run-length digits) must be positive, i.e. the
    // largest code `2^bits - 1` must exceed `max_level_value`. eccodes computes
    // this as a signed value and rejects `range <= 0`; guard the `max >= span`
    // case first so the unsigned subtraction below can't underflow.
    let span = (1u64 << bits) - 1;
    if max >= span {
        return Err(FieldglassError::Parse(format!(
            "run-length packing: max_level_value {max} leaves no room for run digits below 2^{bits} - 1 = {span}"
        )));
    }
    let range = span - max;

    // levels[0] = missing; levels[v] = table[v - 1] · 10^-D for v in 1..=MVL.
    let scale = 10f64.powi(-(t.decimal_scale_factor as i32));
    let mut levels: Vec<Option<f64>> = Vec::with_capacity(t.level_values.len() + 1);
    levels.push(None);
    levels.extend(t.level_values.iter().map(|&lv| Some(lv as f64 * scale)));

    let mut reader = BitReader::new(ds_payload);
    let mut codes = Vec::with_capacity(num_codes);
    for _ in 0..num_codes {
        codes.push(reader.read_bits(bits)? as u64);
    }

    let mut decoded: Vec<Option<f64>> = Vec::with_capacity(present_count);
    let mut i = 0;
    while i < codes.len() {
        let v = codes[i];
        if v > max {
            // A run-length digit with no open run: malformed stream.
            return Err(FieldglassError::Parse(format!(
                "run-length packing: code {v} at position {i} exceeds max_level_value {max} with no open run"
            )));
        }
        i += 1;
        // Run length: 1 for the level itself, plus base-`range` digits (LSB
        // first). Saturating arithmetic keeps a hostile stream from panicking;
        // an over-long run is caught by the present-count overflow check below.
        let mut run = 1u64;
        let mut factor = 1u64;
        while i < codes.len() && codes[i] > max {
            run = run.saturating_add(factor.saturating_mul(codes[i] - max - 1));
            factor = factor.saturating_mul(range);
            i += 1;
        }
        // `v <= max <= number_of_level_values == levels.len() - 1`, so this
        // index is always in range; `get` keeps it total regardless.
        let level = *levels.get(v as usize).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "run-length packing: level index {v} has no table entry"
            ))
        })?;
        for _ in 0..run {
            if decoded.len() == present_count {
                return Err(FieldglassError::Parse(format!(
                    "run-length packing: decoded run overflows the {present_count} expected values"
                )));
            }
            decoded.push(level);
        }
    }

    if decoded.len() != present_count {
        return Err(FieldglassError::Parse(format!(
            "run-length packing: decoded {} values but {present_count} were expected",
            decoded.len()
        )));
    }

    Ok(interleave_present_points(
        decoded.into_iter(),
        bitmap,
        expected_count,
    ))
}

/// Decode second-order (general-extended) packing (templates 5.50001 and
/// 5.50002) — the GRIB1 `grid_second_order` codec carried into GRIB2.
///
/// §5 (the [`SecondOrderPackingTemplate`]) carries the `R` / `E` / `D`
/// transform, the group-descriptor bit widths, the group count, and the
/// spatial-predictor-differencing (SPD) seeds. §7 carries, as three
/// byte-aligned blocks, the per-group widths (`num_groups` @ `width_of_widths`
/// bits), per-group lengths (`num_groups` @ `width_of_lengths` bits), and
/// first-order group reference values (`num_groups` @
/// `width_of_first_order_values` bits), followed by the second-order packed
/// per-point offsets (group `g` contributes `group_lengths[g]` values at
/// `group_widths[g]` bits each, starting on a byte boundary). This matches
/// eccodes' `unsigned_bits` accessors (each rounded up to a whole byte) and
/// `DataG1SecondOrderGeneralExtendedPacking::unpack`, which reads the
/// second-order stream from `byte_offset()`.
///
/// Expansion mirrors the GRIB1 decoder: each point starts at its group's
/// first-order reference plus the packed offset, the `order_of_spd` SPD seeds
/// prime the first slots, and [`apply_spd_inverse`] reconstructs the scaled
/// integers before the `(R + X · 2^E) · 10^-D` transform. Boustrophedonic
/// (alternating-row) ordering — 5.50002 only — is undone after the grid width
/// is known; see [`undo_second_order_boustrophedonic`].
fn decode_second_order(
    ds_payload: &[u8],
    t: &SecondOrderPackingTemplate,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let present_count = check_bitmap_present_count(bitmap, expected_count)?;

    // Constant field: no groups means §7 carries no data, so every present
    // point is the reference value scaled by 10^-D — the same degenerate
    // convention simple packing uses for bits_per_value == 0.
    if t.num_groups == 0 {
        let constant = t.reference_value as f64 * 10f64.powi(-(t.decimal_scale_factor as i32));
        return Ok(materialise_constant(constant, bitmap, expected_count));
    }

    // Every bit-width field feeds `BitReader::read_bits`, whose contract tops
    // out at 32 bits; a wider field is malformed. (widthOfSPD is validated at
    // parse time.)
    for (label, bits) in [
        ("widthOfFirstOrderValues", t.width_of_first_order_values),
        ("widthOfWidths", t.width_of_widths),
        ("widthOfLengths", t.width_of_lengths),
    ] {
        if bits > 32 {
            return Err(FieldglassError::Parse(format!(
                "second-order packing: {label} field width {bits} exceeds 32 bits"
            )));
        }
    }

    let order_of_spd = t.order_of_spd as usize;
    let num_groups = t.num_groups as usize;
    // Every group covers at least one point, so NG can't legitimately exceed
    // the present-point count — this bounds the per-group allocations below.
    if num_groups > present_count {
        return Err(FieldglassError::Parse(format!(
            "second-order packing declares {num_groups} groups but only {present_count} values are present"
        )));
    }
    if t.spd_seeds.len() != order_of_spd {
        return Err(FieldglassError::Parse(format!(
            "second-order packing: {} SPD seeds for orderOfSPD={order_of_spd}",
            t.spd_seeds.len()
        )));
    }

    // §7 blocks are byte-aligned, so realign after each — eccodes' `unsigned_bits`
    // accessors round every block up to a whole byte, and the second-order data
    // starts on a byte boundary.
    let mut reader = BitReader::new(ds_payload);

    // Block 1: group widths. Read into u32 (not u8) so a width_of_widths > 8
    // can't silently truncate a stored value past the 32-bit ceiling checked in
    // the decode loop below — matching how group lengths and first-order values
    // are read.
    let mut group_widths: Vec<u32> = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        group_widths.push(reader.read_bits(t.width_of_widths)?);
    }
    reader.align_to_byte();

    // Block 2: group lengths.
    let mut group_lengths: Vec<u32> = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        group_lengths.push(reader.read_bits(t.width_of_lengths)?);
    }
    reader.align_to_byte();

    // Block 3: first-order (group reference) values.
    let mut first_order: Vec<u32> = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        first_order.push(reader.read_bits(t.width_of_first_order_values)?);
    }
    reader.align_to_byte();

    // The group lengths plus the SPD seeds must reconstruct exactly the present
    // points; validate before allocating so a malformed length can't drive a
    // huge allocation.
    let mut total_second = 0usize;
    for &gl in &group_lengths {
        total_second = total_second.checked_add(gl as usize).ok_or_else(|| {
            FieldglassError::Parse("second-order packing: group lengths sum overflows usize".into())
        })?;
    }
    let total_decoded = total_second.checked_add(order_of_spd).ok_or_else(|| {
        FieldglassError::Parse("second-order packing: decoded count overflows usize".into())
    })?;
    if total_decoded != present_count {
        return Err(FieldglassError::Parse(format!(
            "second-order packing: group lengths + SPD reconstruct {total_decoded} values but {present_count} are required"
        )));
    }

    // Block 4: second-order packed offsets, group by group. Slots [0..order]
    // hold the SPD seeds; each group's points start at its first-order
    // reference plus the packed offset (wrapping matches eccodes' implicit
    // two's-complement C).
    let mut x: Vec<i64> = vec![0; total_decoded];
    let mut n = order_of_spd;
    for g in 0..num_groups {
        let w = group_widths[g];
        if w > 32 {
            return Err(FieldglassError::Parse(format!(
                "second-order packing: group {g} width {w} exceeds 32 bits"
            )));
        }
        let ref_val = first_order[g] as i64;
        if w == 0 {
            // Zero-width group: every point equals the first-order reference.
            for _ in 0..group_lengths[g] {
                x[n] = ref_val;
                n += 1;
            }
        } else {
            for _ in 0..group_lengths[g] {
                let raw = reader.read_bits(w as u8)? as i64;
                x[n] = ref_val.wrapping_add(raw);
                n += 1;
            }
        }
    }
    debug_assert_eq!(n, total_decoded);

    // Plant the SPD seeds and reverse the spatial differencing.
    for (i, &seed) in t.spd_seeds.iter().enumerate() {
        x[i] = seed;
    }
    apply_spd_inverse(&mut x, t.order_of_spd, t.spd_bias)?;

    // Apply the R / E / D transform and spread across the grid per the bitmap.
    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));
    let decoded = x.into_iter().map(|v| (r + v as f64 * two_pow_e) * d_inv);
    Ok(interleave_present_points(
        decoded.map(Some),
        bitmap,
        expected_count,
    ))
}

/// Undo the template-5.50002 boustrophedonic row ordering in place, once the
/// grid width is known. Odd rows (`1, 3, 5, …`) are stored right-to-left, so
/// reversing each restores scan order. A no-op for any other template, when the
/// boustrophedonic flag is clear, or when `columns == 0`.
///
/// eccodes applies this as a post-decode wrapper over the second-order accessor
/// (`data_apply_boustrophedonic` in `template.7.50002.def`), so doing it here —
/// after [`decode_values`] returns and the reader supplies `columns` (the grid
/// width Ni) — mirrors that layering. Crucially, `template.7.50002.def` applies
/// `data_apply_bitmap` *before* `data_apply_boustrophedonic`, so the reversal is
/// meant to run on the full grid *after* the §6 bitmap has spread the present
/// points into place. [`decode_second_order`] does exactly that ordering
/// (`interleave_present_points`, then this reversal on the length-`expected_count`
/// output), so the row reversal is correct whether or not a bitmap is present.
pub fn undo_second_order_boustrophedonic(
    values: &mut [Option<f64>],
    template: &DataRepresentationTemplate,
    columns: usize,
) {
    let DataRepresentationTemplate::SecondOrder(t) = template else {
        return;
    };
    if !t.boustrophedonic || columns == 0 {
        return;
    }
    let rows = values.len() / columns;
    for row in (1..rows).step_by(2) {
        values[row * columns..(row + 1) * columns].reverse();
    }
}

/// Spread `present` into the full grid using `bitmap` — `Some(value)` for
/// flagged points, `None` for unflagged. Asserts shape internally; callers
/// pre-check lengths.
fn interleave_with_bitmap(
    present: Vec<f64>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    interleave_present_points(present.into_iter().map(Some), bitmap, expected_count)
}

/// Iterator-based body shared by [`interleave_with_bitmap`] and the complex
/// packing decoders (whose present-point values may already be `None` from
/// inline missing-value management; those pass through as missing alongside
/// the bitmap's).
fn interleave_present_points(
    mut present: impl Iterator<Item = Option<f64>>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    match bitmap {
        None => present.collect(),
        Some(b) => {
            let mut out = Vec::with_capacity(expected_count);
            for &flag in b {
                out.push(if flag { present.next().flatten() } else { None });
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

    fn matrix_template(matrix_bitmaps: u8, bits: u8) -> DataRepresentationTemplate {
        DataRepresentationTemplate::MatrixSimple(crate::drs::MatrixSimplePackingTemplate {
            reference_value: 300.0,
            binary_scale_factor: 0,
            decimal_scale_factor: 0,
            bits_per_value: bits,
            matrix_bitmaps_present: matrix_bitmaps,
            number_of_coded_values: 4,
            nr: 2,
            nc: 3,
            first_dim_coordinate_definition: 0,
            second_dim_coordinate_definition: 0,
            first_dim_physical_significance: 0,
            second_dim_physical_significance: 0,
            coefficients_first: vec![],
            coefficients_second: vec![],
        })
    }

    #[test]
    fn matrix_simple_flat_decodes_like_simple_packing() {
        // matrixBitmapsPresent = 0 → one value per grid point, decoded as 5.0.
        let packed = pack_bits(&[0, 5, 10, 20], 8);
        let decoded = decode_values(&packed, matrix_template(0, 8), None, 4).expect("decode");
        assert_eq!(
            decoded,
            vec![Some(300.0), Some(305.0), Some(310.0), Some(320.0)]
        );
    }

    #[test]
    fn matrix_simple_with_secondary_bitmaps_is_rejected() {
        // matrixBitmapsPresent = 1 is the eccodes-unsupported true-matrix variant.
        let err = decode_values(&[0u8; 8], matrix_template(1, 8), None, 4).expect_err("reject");
        assert!(
            matches!(err, FieldglassError::UnsupportedSection(_))
                && format!("{err:?}").contains("matrixBitmapsPresent=1"),
            "rejects the secondary-bitmap matrix variant, got: {err:?}"
        );
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

    fn run_length_template(
        bits: u8,
        max_level: u16,
        level_values: Vec<u16>,
        d: i16,
    ) -> DataRepresentationTemplate {
        DataRepresentationTemplate::RunLength(RunLengthPackingTemplate {
            bits_per_value: bits,
            max_level_value: max_level,
            number_of_level_values: level_values.len() as u16,
            decimal_scale_factor: d,
            level_values,
        })
    }

    /// Encode `(level, count)` runs into a §7 run-length payload. Asserts the
    /// stream is a whole number of bytes — real encoders never emit a partial
    /// trailing code, and `pack_bits` would zero-pad one into existence.
    fn rle_codes(runs: &[(u16, u32)], bits: u8, max_level: u16) -> Vec<u8> {
        let range = (1u32 << bits) - 1 - max_level as u32;
        let mut codes: Vec<u32> = Vec::new();
        for &(level, count) in runs {
            codes.push(level as u32);
            let mut rem = count - 1;
            while rem > 0 {
                codes.push(rem % range + max_level as u32 + 1);
                rem /= range;
            }
        }
        assert_eq!(
            (codes.len() * bits as usize) % 8,
            0,
            "test runs are not byte-aligned",
        );
        pack_bits(&codes, bits)
    }

    #[test]
    fn run_length_decodes_runs_and_missing() {
        // levels [10,20,30], level 0 = missing. runs: 1×2, missing×3, 3×1.
        let template = run_length_template(8, 3, vec![10, 20, 30], 0);
        let packed = rle_codes(&[(1, 2), (0, 3), (3, 1)], 8, 3);
        let decoded = decode_values(&packed, template, None, 6).expect("decode");
        assert_eq!(
            decoded,
            vec![Some(10.0), Some(10.0), None, None, None, Some(30.0)],
        );
    }

    #[test]
    fn run_length_decodes_multidigit_run() {
        // A run longer than `range` (251 > 250) needs a second base-`range`
        // digit: 251 = 1 + 0·1 + 1·250.
        let template = run_length_template(8, 5, vec![10, 20, 30, 40, 50], 1);
        let packed = rle_codes(&[(2, 251)], 8, 5);
        let decoded = decode_values(&packed, template, None, 251).expect("decode");
        assert_eq!(decoded.len(), 251);
        assert!(decoded.iter().all(|v| (v.unwrap() - 2.0).abs() < 1e-9));
    }

    #[test]
    fn run_length_applies_negative_decimal_scale() {
        // decimalScaleFactor = -1 → value = level_value · 10^1.
        let template = run_length_template(8, 2, vec![3, 4], -1);
        let packed = rle_codes(&[(1, 1), (2, 1)], 8, 2);
        let decoded = decode_values(&packed, template, None, 2).expect("decode");
        assert!((decoded[0].unwrap() - 30.0).abs() < 1e-9);
        assert!((decoded[1].unwrap() - 40.0).abs() < 1e-9);
    }

    #[test]
    fn run_length_interleaves_with_bitmap() {
        // Present points decode to 10, 20; the bitmap spreads them with a gap.
        let template = run_length_template(8, 3, vec![10, 20, 30], 0);
        let packed = rle_codes(&[(1, 1), (2, 1)], 8, 3);
        let bitmap = [true, false, true];
        let decoded = decode_values(&packed, template, Some(&bitmap), 3).expect("decode");
        assert_eq!(decoded, vec![Some(10.0), None, Some(20.0)]);
    }

    #[test]
    fn run_length_all_missing_when_max_level_zero() {
        let template = run_length_template(8, 0, vec![], 0);
        let decoded = decode_values(&[0xFF], template, None, 4).expect("decode");
        assert_eq!(decoded, vec![None; 4]);
    }

    #[test]
    fn run_length_all_missing_when_bits_zero() {
        let template = run_length_template(0, 3, vec![10, 20, 30], 0);
        let decoded = decode_values(&[0xFF], template, None, 4).expect("decode");
        assert_eq!(decoded, vec![None; 4]);
    }

    #[test]
    fn run_length_all_missing_when_no_codes() {
        // Empty §7 → no codes → wholly-missing field, matching eccodes.
        let template = run_length_template(8, 3, vec![10, 20, 30], 0);
        let decoded = decode_values(&[], template, None, 4).expect("decode");
        assert_eq!(decoded, vec![None; 4]);
    }

    #[test]
    fn run_length_rejects_zero_range() {
        // 2^3 - 1 = 7 = max_level → no code values left for run digits.
        let template = run_length_template(3, 7, vec![0; 7], 0);
        assert!(decode_values(&[0xFF], template, None, 4).is_err());
    }

    #[test]
    fn run_length_rejects_max_level_above_code_span() {
        // max_level_value 10 exceeds the largest 3-bit code (7): the range
        // subtraction must reject this rather than underflow.
        let template = run_length_template(3, 10, vec![0; 10], 0);
        assert!(decode_values(&[0xFF], template, None, 4).is_err());
    }

    #[test]
    fn run_length_rejects_max_level_over_levels() {
        // max_level_value 5 > number_of_level_values 3.
        let template = run_length_template(8, 5, vec![10, 20, 30], 0);
        assert!(decode_values(&[0x01], template, None, 4).is_err());
    }

    #[test]
    fn run_length_rejects_bits_over_32() {
        // Needs a payload holding at least one 33-bit code, else the empty-code
        // short-circuit returns all-missing before the width is validated.
        let template = run_length_template(33, 3, vec![10, 20, 30], 0);
        assert!(decode_values(&[0xFF; 8], template, None, 4).is_err());
    }

    #[test]
    fn run_length_empty_stream_is_all_missing_even_for_invalid_template() {
        // eccodes short-circuits the empty-code-stream case to an all-missing
        // field before validating the template, so a degenerate template with
        // no §7 codes decodes to all-missing rather than erroring.
        let template = run_length_template(8, 5, vec![10, 20, 30], 0); // max > levels
        let decoded = decode_values(&[], template, None, 4).expect("decode");
        assert_eq!(decoded, vec![None; 4]);
    }

    #[test]
    fn run_length_rejects_orphan_run_digit() {
        // First code (6) exceeds max_level (3) with no open run.
        let template = run_length_template(8, 3, vec![10, 20, 30], 0);
        let packed = pack_bits(&[6], 8);
        assert!(decode_values(&packed, template, None, 2).is_err());
    }

    #[test]
    fn run_length_rejects_too_few_values() {
        // One code decodes to a single value but the grid expects three.
        let template = run_length_template(8, 3, vec![10, 20, 30], 0);
        let packed = pack_bits(&[1], 8);
        assert!(decode_values(&packed, template, None, 3).is_err());
    }

    #[test]
    fn run_length_rejects_too_many_values() {
        // Three level codes decode to three values but the grid expects two.
        let template = run_length_template(8, 3, vec![10, 20, 30], 0);
        let packed = pack_bits(&[1, 1, 1], 8);
        assert!(decode_values(&packed, template, None, 2).is_err());
    }

    fn log_template(r: f32, e: i16, d: i16, bits: u8, ppp: f32) -> DataRepresentationTemplate {
        DataRepresentationTemplate::LogPreprocessing(LogPreprocessingPackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            pre_processing_parameter: ppp,
        })
    }

    #[test]
    fn log_preprocessing_zero_bias() {
        // R=0, E=0, D=0 → simple value X = packed; ppp=0 → Y = exp(X).
        let template = log_template(0.0, 0, 0, 8, 0.0);
        let packed = pack_bits(&[0, 1, 2], 8);
        let decoded = decode_values(&packed, template, None, 3).expect("decode");
        assert!((decoded[0].unwrap() - 1.0).abs() < 1e-9); // exp(0)
        assert!((decoded[1].unwrap() - 1f64.exp()).abs() < 1e-9);
        assert!((decoded[2].unwrap() - 2f64.exp()).abs() < 1e-9);
    }

    #[test]
    fn log_preprocessing_nonzero_bias() {
        // ppp=1 → Y = exp(X) - 1.
        let template = log_template(0.0, 0, 0, 8, 1.0);
        let packed = pack_bits(&[0, 1, 2], 8);
        let decoded = decode_values(&packed, template, None, 3).expect("decode");
        assert!((decoded[0].unwrap() - 0.0).abs() < 1e-9); // exp(0) - 1
        assert!((decoded[1].unwrap() - (1f64.exp() - 1.0)).abs() < 1e-9);
        assert!((decoded[2].unwrap() - (2f64.exp() - 1.0)).abs() < 1e-9);
    }

    #[test]
    fn log_preprocessing_preserves_bitmap_missing() {
        // A masked point stays missing through the exp transform.
        let template = log_template(0.0, 0, 0, 8, 0.0);
        let packed = pack_bits(&[0, 2], 8);
        let bitmap = [true, false, true];
        let decoded = decode_values(&packed, template, Some(&bitmap), 3).expect("decode");
        assert!((decoded[0].unwrap() - 1.0).abs() < 1e-9);
        assert_eq!(decoded[1], None);
        assert!((decoded[2].unwrap() - 2f64.exp()).abs() < 1e-9);
    }

    #[test]
    fn log_preprocessing_constant_field() {
        // bits_per_value == 0 → simple decode yields the constant R·10^-D, then
        // the exp transform applies. R=1, D=0, ppp=0 → Y = exp(1) everywhere.
        let template = log_template(1.0, 0, 0, 0, 0.0);
        let decoded = decode_values(&[], template, None, 4).expect("decode");
        assert_eq!(decoded.len(), 4);
        for v in decoded {
            assert!((v.unwrap() - 1f64.exp()).abs() < 1e-9);
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

    /// Pack `(value, width)` fields MSB-first into one continuous bitstream,
    /// padded to a whole number of bytes at the end.
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

    /// Pack the §7 complex-packing sub-blocks (group references, widths,
    /// lengths, data) with each block starting on a byte boundary, as the
    /// GRIB2 layout requires. `pack_fields` already pads each block to whole
    /// bytes, so concatenating them yields the octet-aligned stream the
    /// decoder expects.
    fn pack_blocks(blocks: &[&[(u32, u8)]]) -> Vec<u8> {
        blocks.iter().flat_map(|b| pack_fields(b)).collect()
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
        let payload = pack_blocks(&[
            &[(10, 8), (100, 8)],                       // group references
            &[(3, 4), (4, 4)],                          // stored group widths (reference 0)
            &[(2, 8), (0, 8)], // stored group lengths — g1's is overridden by last = 3
            &[(1, 3), (2, 3), (0, 4), (5, 4), (15, 4)], // g0 data (2×3b) then g1 data (3×4b)
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
        let payload = pack_blocks(&[
            &[(5, 8)],                         // group reference
            &[(4, 4)],                         // stored group width
            &[(0, 8)],                         // stored group length — overridden by last = 4
            &[(0, 4), (1, 4), (2, 4), (3, 4)], // data
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
        let payload = pack_blocks(&[
            &[(42, 8), (7, 8)], // references
            &[(0, 4), (2, 4)],  // widths — g0 is zero-width
            &[(3, 8), (0, 8)],  // lengths — g1 overridden by last = 2
            &[(1, 2), (3, 2)],  // g1 data only (g0 contributes no data bits)
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
        let payload = pack_blocks(&[
            &[(0, 8)],                 // reference
            &[(8, 4)],                 // width 8
            &[(0, 8)],                 // length overridden by last = 3
            &[(1, 8), (2, 8), (3, 8)], // data
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
    fn complex_packing_rejects_reserved_missing_value_management() {
        // Code Table 5.5 defines 0/1/2; 3 is reserved.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            missing_value_management: 3,
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
    fn complex_packing_rejects_reserved_splitting_method() {
        // Code Table 5.4 defines 0 (row by row) and 1 (general); 2 is reserved.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_splitting_method: 2,
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
    fn complex_packing_row_by_row_decodes_like_general() {
        // The §7 group structure is self-describing, so splitting method 0
        // (row by row) decodes on the same path as method 1 — byte-identical
        // results from the same payload, as in eccodes.
        let payload = pack_blocks(&[
            &[(10, 8), (100, 8)],
            &[(3, 4), (4, 4)],
            &[(2, 8), (0, 8)],
            &[(1, 3), (2, 3), (0, 4), (5, 4), (15, 4)],
        ]);
        let decode = |method: u8| {
            let t = ComplexPackingTemplate {
                num_groups: 2,
                group_length_last: 3,
                group_splitting_method: method,
                ..complex_template_base()
            };
            decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 5)
                .expect("decode")
        };
        assert_eq!(decode(0), decode(1));
    }

    /// Decode one width-4 group (ref 10, offsets `[1, 15, 3, 14]`) under the
    /// given missing-value management. On this payload 15 is the primary
    /// sentinel (`2^4 − 1`) and 14 the secondary, so the management modes
    /// disagree only on those two points.
    fn decode_width4_sentinel_payload(mvm: u8) -> Vec<Option<f64>> {
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 4,
            missing_value_management: mvm,
            ..complex_template_base()
        };
        let payload = pack_blocks(&[
            &[(10, 8)],
            &[(4, 4)],
            &[(0, 8)],
            &[(1, 4), (15, 4), (3, 4), (14, 4)],
        ]);
        decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 4).expect("decode")
    }

    #[test]
    fn complex_packing_mvm1_marks_sentinel_offsets_missing() {
        // Management 1: the all-ones offset is missing; the would-be
        // secondary (14) is an ordinary value.
        assert_eq!(
            decode_width4_sentinel_payload(1),
            vec![Some(11.0), None, Some(13.0), Some(24.0)],
        );
    }

    #[test]
    fn complex_packing_mvm2_marks_secondary_offsets_missing_too() {
        // Management 2 on the same payload: the secondary sentinel goes
        // missing as well.
        assert_eq!(
            decode_width4_sentinel_payload(2),
            vec![Some(11.0), None, Some(13.0), None],
        );
    }

    #[test]
    fn complex_packing_mvm1_zero_width_group_reference_sentinel_is_missing() {
        // A zero-width group stores no offsets; under management 1 a group
        // reference equal to 2^bits_per_value − 1 (255 here) marks the whole
        // group missing. A second, ordinary group still decodes (its offsets
        // stay below the width-2 sentinel 3).
        let t = ComplexPackingTemplate {
            num_groups: 2,
            group_length_last: 2,
            missing_value_management: 1,
            ..complex_template_base()
        };
        let payload = pack_blocks(&[
            &[(255, 8), (7, 8)],
            &[(0, 4), (2, 4)],
            &[(3, 8), (0, 8)],
            &[(1, 2), (2, 2)],
        ]);
        let decoded = decode_values(&payload, DataRepresentationTemplate::Complex(t), None, 5)
            .expect("decode");
        assert_eq!(decoded, vec![None, None, None, Some(8.0), Some(9.0)]);
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
    fn complex_packing_zero_groups_is_constant_reference_value() {
        // NG = 0 is a constant field: every point equals R *verbatim* — the
        // non-zero E and D here must not be applied (eccodes ECC-2095 returns
        // reference_value directly). The empty payload proves §7 is not read.
        let t = ComplexPackingTemplate {
            reference_value: 2.5,
            binary_scale_factor: 1,
            decimal_scale_factor: 1,
            ..complex_template_base() // num_groups = 0
        };
        let decoded =
            decode_values(&[], DataRepresentationTemplate::Complex(t), None, 4).expect("decode");
        assert_eq!(decoded, vec![Some(2.5); 4]);
    }

    #[test]
    fn complex_packing_zero_groups_empty_grid_decodes_empty() {
        // NG = 0 with no grid points: still the constant path, yielding an
        // empty field (the behaviour the pre-ECC-2095 code also had).
        let t = complex_template_base(); // num_groups = 0
        let decoded =
            decode_values(&[], DataRepresentationTemplate::Complex(t), None, 0).expect("decode");
        assert_eq!(decoded, Vec::<Option<f64>>::new());
    }

    #[test]
    fn complex_packing_zero_groups_respects_bitmap() {
        // NG = 0 with a §6 bitmap: present points are R, absent points None.
        let t = ComplexPackingTemplate {
            reference_value: -7.0,
            ..complex_template_base() // num_groups = 0
        };
        let bitmap = [true, false, true, false];
        let decoded = decode_values(
            &[],
            DataRepresentationTemplate::Complex(t),
            Some(&bitmap),
            4,
        )
        .expect("decode");
        assert_eq!(decoded, vec![Some(-7.0), None, Some(-7.0), None]);
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

    // -----------------------------------------------------------------
    // Complex packing + spatial differencing (template 5.3)
    // -----------------------------------------------------------------

    /// Wrap a 5.2 template plus spatial-differencing descriptors as a 5.3
    /// template payload.
    fn spd_template(
        complex: ComplexPackingTemplate,
        order: u8,
        octets: u8,
    ) -> DataRepresentationTemplate {
        DataRepresentationTemplate::ComplexSpatialDiff(ComplexSpatialDiffTemplate {
            complex,
            spatial_diff_order: order,
            extra_descriptor_octets: octets,
        })
    }

    /// Build a §7 payload for spatial differencing: the raw extra-descriptor
    /// octets followed by the octet-aligned complex-packing group blocks.
    fn pack_spd(extras: &[u8], blocks: &[&[(u32, u8)]]) -> Vec<u8> {
        let mut out = extras.to_vec();
        out.extend(pack_blocks(blocks));
        out
    }

    #[test]
    fn spatial_diff_order_1_reverses_first_differences() {
        // Original scaled integers g = [10, 13, 12, 20] (R/E/D = 0 → values
        // equal the integers). First differences e = [3, -1, 8]; minimum −1,
        // so reduced differences stored in the groups are d = [_, 4, 0, 9]
        // with d[0] a placeholder overwritten by ival1 = 10.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 4,
            ..complex_template_base()
        };
        // Extras (1 octet each): ival1 = 10, then minimum −1 as sign-magnitude
        // 0x81 (sign bit + magnitude 1).
        let payload = pack_spd(
            &[10, 0x81],
            &[
                &[(0, 8)],                         // group reference 0
                &[(4, 4)],                         // width 4
                &[(0, 8)],                         // length overridden by last = 4
                &[(0, 4), (4, 4), (0, 4), (9, 4)], // reduced differences d
            ],
        );
        let decoded = decode_values(&payload, spd_template(t, 1, 1), None, 4).expect("decode");
        assert_eq!(
            decoded,
            vec![Some(10.0), Some(13.0), Some(12.0), Some(20.0)],
        );
    }

    #[test]
    fn spatial_diff_order_2_reverses_second_differences() {
        // g = [5, 8, 14, 23]; second differences are both 3, minimum 3, so the
        // reduced differences stored in the groups are all zero (a zero-width
        // group). ival1 = 5, ival2 = 8 seed the recurrence.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 4,
            ..complex_template_base()
        };
        // Extras: ival1 = 5, ival2 = 8, minimum +3 (sign-magnitude 0x03).
        let payload = pack_spd(
            &[5, 8, 3],
            &[
                &[(0, 8)], // group reference 0
                &[(0, 4)], // width 0 → zero-width group, no data bits
                &[(0, 8)], // length overridden by last = 4
            ],
        );
        let decoded = decode_values(&payload, spd_template(t, 2, 1), None, 4).expect("decode");
        assert_eq!(decoded, vec![Some(5.0), Some(8.0), Some(14.0), Some(23.0)],);
    }

    #[test]
    fn spatial_diff_zero_groups_is_constant_reference_value() {
        // NG = 0 under 5.3: constant field, R verbatim — the empty payload
        // proves not even the spatial-differencing extra descriptors are read
        // (eccodes ECC-2095 returns before touching §7).
        let t = ComplexPackingTemplate {
            reference_value: 4.25,
            binary_scale_factor: 2,
            decimal_scale_factor: 1,
            ..complex_template_base() // num_groups = 0
        };
        let decoded = decode_values(&[], spd_template(t, 2, 1), None, 3).expect("decode");
        assert_eq!(decoded, vec![Some(4.25); 3]);
    }

    #[test]
    fn spatial_diff_zero_groups_wins_over_invalid_order() {
        // eccodes checks NG == 0 before validating the differencing order, so
        // a constant field with a reserved order still decodes; match that.
        let t = ComplexPackingTemplate {
            reference_value: 1.5,
            ..complex_template_base() // num_groups = 0
        };
        let decoded = decode_values(&[], spd_template(t, 3, 1), None, 2).expect("decode");
        assert_eq!(decoded, vec![Some(1.5); 2]);
    }

    #[test]
    fn spatial_diff_applies_reference_and_scale_factors() {
        // Reuse the order-1 integers but scale: R = 100, E = 0, D = 1 → value
        // = (100 + g) · 0.1 → [11.0, 11.3, 11.2, 12.0].
        let t = ComplexPackingTemplate {
            reference_value: 100.0,
            decimal_scale_factor: 1,
            num_groups: 1,
            group_length_last: 4,
            ..complex_template_base()
        };
        let payload = pack_spd(
            &[10, 0x81],
            &[
                &[(0, 8)],
                &[(4, 4)],
                &[(0, 8)],
                &[(0, 4), (4, 4), (0, 4), (9, 4)],
            ],
        );
        let decoded = decode_values(&payload, spd_template(t, 1, 1), None, 4).expect("decode");
        let got: Vec<f64> = decoded.into_iter().map(Option::unwrap).collect();
        for (g, w) in got.iter().zip([11.0, 11.3, 11.2, 12.0]) {
            assert!((g - w).abs() < 1e-9, "got {g}, want {w}");
        }
    }

    #[test]
    fn spatial_diff_honours_bitmap() {
        // Three present order-1 values spread across a 5-point grid.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 3,
            ..complex_template_base()
        };
        // g = [4, 7, 6]; e = [3, -1], minimum −1, reduced d = [_, 4, 0].
        let payload = pack_spd(
            &[4, 0x81],
            &[&[(0, 8)], &[(4, 4)], &[(0, 8)], &[(0, 4), (4, 4), (0, 4)]],
        );
        let bitmap = [true, false, true, false, true];
        let decoded =
            decode_values(&payload, spd_template(t, 1, 1), Some(&bitmap), 5).expect("decode");
        assert_eq!(decoded, vec![Some(4.0), None, Some(7.0), None, Some(6.0)]);
    }

    #[test]
    fn spatial_diff_rejects_unsupported_order() {
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 1,
            ..complex_template_base()
        };
        let err = decode_values(&[0u8; 16], spd_template(t, 3, 1), None, 1)
            .expect_err("must reject order 3");
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("spatial-differencing order 3"), "msg: {msg}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

    #[test]
    fn spatial_diff_rejects_out_of_range_octet_width() {
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 1,
            ..complex_template_base()
        };
        // 0 octets and >4 octets both exceed what `read_bits` can serve.
        for octets in [0u8, 5] {
            let err = decode_values(&[0u8; 16], spd_template(t, 1, octets), None, 1)
                .expect_err("must reject octet width");
            assert!(
                err.to_string().contains("extra-descriptor width"),
                "octets {octets}: {err}",
            );
        }
    }

    #[test]
    fn spatial_diff_inherits_complex_envelope_restrictions() {
        // A reserved missing-value-management value is rejected by the shared
        // group decoder even via the 5.3 path.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            missing_value_management: 3,
            group_length_last: 1,
            ..complex_template_base()
        };
        // Extras consume the first 2 octets; the group decode then trips on
        // the reserved management value.
        let err = decode_values(&[0u8; 16], spd_template(t, 1, 1), None, 1)
            .expect_err("must reject reserved management");
        match err {
            FieldglassError::UnsupportedSection(msg) => {
                assert!(msg.contains("missing-value management"), "msg: {msg}");
            }
            other => panic!("expected UnsupportedSection, got {other:?}"),
        }
    }

    #[test]
    fn spatial_diff_order1_skips_missing_in_recurrence() {
        // Order 1, management 1, width 4 (offset 15 = missing). Packed
        // offsets: [15, 0, 4, 15, 0]. The seed (ival1 = 10) lands on the
        // first *non-missing* slot; each later non-missing point recurses on
        // the nearest previous non-missing value (bias = +1):
        //   [None, 10, 4+10+1 = 15, None, 0+15+1 = 16].
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 5,
            missing_value_management: 1,
            ..complex_template_base()
        };
        let payload = pack_spd(
            &[10, 0x01],
            &[
                &[(0, 8)],
                &[(4, 4)],
                &[(0, 8)],
                &[(15, 4), (0, 4), (4, 4), (15, 4), (0, 4)],
            ],
        );
        let decoded = decode_values(&payload, spd_template(t, 1, 1), None, 5).expect("decode");
        assert_eq!(
            decoded,
            vec![None, Some(10.0), Some(15.0), None, Some(16.0)],
        );
    }

    #[test]
    fn spatial_diff_seeds_short_fields_without_error() {
        // A field with fewer points than the differencing order just seeds
        // what exists, as eccodes' post-process does: one point, order 2 →
        // the point takes ival1.
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 1,
            ..complex_template_base()
        };
        let payload = pack_spd(&[5, 8, 3], &[&[(0, 8)], &[(0, 4)], &[(0, 8)]]);
        let decoded = decode_values(&payload, spd_template(t, 2, 1), None, 1).expect("decode");
        assert_eq!(decoded, vec![Some(5.0)]);
    }

    #[test]
    fn spatial_diff_order2_skips_missing_in_recurrence() {
        // Order 2, management 1, width 4. Packed offsets:
        // [15, 0, 0, 2, 15]. Seeds ival1 = 5 and ival2 = 8 land on the first
        // two non-missing slots; the recurrence (bias = +3) then gives
        // 2 + 3 + 2·8 − 5 = 16 for the next non-missing point:
        //   [None, 5, 8, 16, None].
        let t = ComplexPackingTemplate {
            num_groups: 1,
            group_length_last: 5,
            missing_value_management: 1,
            ..complex_template_base()
        };
        let payload = pack_spd(
            &[5, 8, 3],
            &[
                &[(0, 8)],
                &[(4, 4)],
                &[(0, 8)],
                &[(15, 4), (0, 4), (0, 4), (2, 4), (15, 4)],
            ],
        );
        let decoded = decode_values(&payload, spd_template(t, 2, 1), None, 5).expect("decode");
        assert_eq!(decoded, vec![None, Some(5.0), Some(8.0), Some(16.0), None],);
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

    // -----------------------------------------------------------------
    // PNG packing (template 5.41)
    // -----------------------------------------------------------------

    fn png_template(r: f32, e: i16, d: i16, bits: u8) -> DataRepresentationTemplate {
        DataRepresentationTemplate::Png(PngPackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            original_field_type: 0,
        })
    }

    /// Encode `samples` (the raw PNG image bytes — big-endian for 16-bit) as a
    /// grayscale PNG, mirroring how eccodes lays the packed integers out in §7.
    fn encode_png_gray(samples: &[u8], width: u32, height: u32, depth: png::BitDepth) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Grayscale);
            enc.set_depth(depth);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(samples).expect("png data");
            writer.finish().expect("png finish");
        }
        out
    }

    #[test]
    fn png_packing_decodes_8bit_grayscale_no_bitmap() {
        // 4 values in a 4×1 8-bit grayscale image; R = 300, E = 0, D = 0 →
        // value = 300 + X.
        let png = encode_png_gray(&[0, 5, 10, 250], 4, 1, png::BitDepth::Eight);
        let decoded = decode_values(&png, png_template(300.0, 0, 0, 8), None, 4).expect("decode");
        assert_eq!(
            decoded,
            vec![Some(300.0), Some(305.0), Some(310.0), Some(550.0)],
        );
    }

    #[test]
    fn png_packing_decodes_16bit_grayscale() {
        // 13-bit values need a 16-bit grayscale image (2 bytes/value), the same
        // layout as the `png_eta_lambert` fixture. R = 100 → value = 100 + X.
        let values: [u16; 4] = [0, 1, 4000, 8191];
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_be_bytes()).collect();
        let png = encode_png_gray(&bytes, 2, 2, png::BitDepth::Sixteen);
        let decoded = decode_values(&png, png_template(100.0, 0, 0, 13), None, 4).expect("decode");
        assert_eq!(
            decoded,
            vec![Some(100.0), Some(101.0), Some(4100.0), Some(8291.0)],
        );
    }

    #[test]
    fn png_packing_applies_reference_and_scale_factors() {
        // R = 0, E = -1 (×0.5), D = 1 (×0.1): value = (X·0.5)·0.1 = X·0.05.
        let png = encode_png_gray(&[0, 2, 20], 3, 1, png::BitDepth::Eight);
        let decoded = decode_values(&png, png_template(0.0, -1, 1, 8), None, 3).expect("decode");
        let got: Vec<f64> = decoded.into_iter().map(Option::unwrap).collect();
        for (g, w) in got.iter().zip([0.0, 0.1, 1.0]) {
            assert!((g - w).abs() < 1e-9, "got {g}, want {w}");
        }
    }

    #[test]
    fn png_packing_constant_field_no_stream() {
        // bits_per_value == 0 → no PNG is written; every point equals R · 10^-D.
        let decoded = decode_values(&[], png_template(7.0, 0, 1, 0), None, 5).expect("decode");
        assert_eq!(decoded.len(), 5);
        for v in decoded {
            assert!((v.unwrap() - 0.7).abs() < 1e-9);
        }
    }

    #[test]
    fn png_packing_honours_bitmap() {
        // Three present values across a 5-point grid; the image holds exactly
        // the three present pixels.
        let png = encode_png_gray(&[1, 2, 3], 3, 1, png::BitDepth::Eight);
        let bitmap = [true, false, true, false, true];
        let decoded =
            decode_values(&png, png_template(0.0, 0, 0, 8), Some(&bitmap), 5).expect("decode");
        assert_eq!(decoded, vec![Some(1.0), None, Some(2.0), None, Some(3.0)]);
    }

    #[test]
    fn png_packing_rejects_pixel_count_mismatch() {
        // The image holds 4 pixels but the grid declares 6 points.
        let png = encode_png_gray(&[0; 4], 4, 1, png::BitDepth::Eight);
        let err =
            decode_values(&png, png_template(0.0, 0, 0, 8), None, 6).expect_err("must reject");
        assert!(err.to_string().contains("pixels"), "got: {err}");
    }

    #[test]
    fn png_packing_rejects_unexpected_colour_type() {
        // An RGB image (3 bytes/pixel) where the template expects 1 byte/value:
        // the bytes-per-pixel disagree, so decode rejects rather than misreads.
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, 2, 2);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut writer = enc.write_header().expect("png header");
            writer.write_image_data(&[0u8; 12]).expect("png data");
            writer.finish().expect("png finish");
        }
        let err =
            decode_values(&out, png_template(0.0, 0, 0, 8), None, 4).expect_err("must reject");
        assert!(err.to_string().contains("bytes"), "got: {err}");
    }

    #[test]
    fn png_packing_rejects_malformed_stream() {
        let err = decode_values(b"not a png", png_template(0.0, 0, 0, 8), None, 4)
            .expect_err("must reject");
        assert!(err.to_string().contains("PNG packing"), "got: {err}");
    }

    #[test]
    fn png_packing_rejects_bits_above_32() {
        let err =
            decode_values(&[], png_template(0.0, 0, 0, 33), None, 4).expect_err("must reject");
        assert!(err.to_string().contains("exceeds 32"), "got: {err}");
    }

    // -----------------------------------------------------------------
    // CCSDS / AEC packing (template 5.42)
    //
    // `rust_aec` has no encoder, so a valid AEC stream can't be synthesised
    // in-crate. The happy-path decode is cross-checked against the committed
    // eccodes oracle in `tests/decode_ccsds.rs`; these unit tests cover the
    // branches that don't need a real stream — the constant-field shortcut,
    // input validation, and the graceful-degradation guardrail.
    // -----------------------------------------------------------------

    fn ccsds_template(r: f32, e: i16, d: i16, bits: u8) -> DataRepresentationTemplate {
        DataRepresentationTemplate::Ccsds(CcsdsPackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            original_field_type: 0,
            ccsds_flags: 0x0e,
            block_size: 32,
            reference_sample_interval: 128,
        })
    }

    #[test]
    fn ccsds_packing_constant_field_no_stream() {
        // bits_per_value == 0 → §7 is empty; every present point equals R
        // verbatim (eccodes `grid_ccsds` semantics — no 10^-D factor).
        let decoded = decode_values(&[], ccsds_template(7.0, 0, 1, 0), None, 5).expect("decode");
        assert_eq!(decoded, vec![Some(7.0); 5]);
    }

    #[test]
    fn ccsds_packing_constant_field_honours_bitmap() {
        let bitmap = [true, false, true, false, true];
        let decoded =
            decode_values(&[], ccsds_template(3.5, 0, 0, 0), Some(&bitmap), 5).expect("decode");
        assert_eq!(decoded, vec![Some(3.5), None, Some(3.5), None, Some(3.5)],);
    }

    #[test]
    fn ccsds_packing_rejects_bits_above_32() {
        let err =
            decode_values(&[], ccsds_template(0.0, 0, 0, 33), None, 4).expect_err("must reject");
        assert!(err.to_string().contains("exceeds 32"), "got: {err}");
    }

    #[test]
    fn ccsds_packing_degrades_on_malformed_stream() {
        // A non-AEC payload must surface as a recoverable UnsupportedSection
        // error (graceful degradation), never a panic.
        let err = decode_values(b"not an aec stream", ccsds_template(0.0, 0, 0, 16), None, 8)
            .expect_err("must reject");
        assert!(
            matches!(err, FieldglassError::UnsupportedSection(_)),
            "expected UnsupportedSection, got: {err:?}"
        );
        assert!(err.to_string().contains("CCSDS packing"), "got: {err}");
    }

    // -----------------------------------------------------------------
    // JPEG 2000 packing (template 5.40)
    //
    // The §7 codestream is decoded by `rust_j2k`; the happy path is
    // cross-checked against the committed eccodes oracle in
    // `tests/decode_jpeg2000.rs`. These unit tests cover the branches that
    // don't need a real codestream — the constant-field shortcut, the
    // bits-per-value guard, and the graceful-degradation guardrail.
    // -----------------------------------------------------------------

    fn jpeg2000_template(r: f32, e: i16, d: i16, bits: u8) -> DataRepresentationTemplate {
        DataRepresentationTemplate::Jpeg2000(Jpeg2000PackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            original_field_type: 0,
            type_of_compression_used: 0,
            target_compression_ratio: 255,
        })
    }

    #[test]
    fn jpeg2000_packing_constant_field_no_stream() {
        // bits_per_value == 0 → §7 is empty; every present point equals R
        // verbatim (eccodes `grid_jpeg` semantics — no 10^-D factor).
        let decoded = decode_values(&[], jpeg2000_template(7.0, 0, 1, 0), None, 5).expect("decode");
        assert_eq!(decoded, vec![Some(7.0); 5]);
    }

    #[test]
    fn jpeg2000_packing_constant_field_honours_bitmap() {
        let bitmap = [true, false, true, false, true];
        let decoded =
            decode_values(&[], jpeg2000_template(3.5, 0, 0, 0), Some(&bitmap), 5).expect("decode");
        assert_eq!(decoded, vec![Some(3.5), None, Some(3.5), None, Some(3.5)]);
    }

    #[test]
    fn jpeg2000_packing_rejects_bits_above_32() {
        let err =
            decode_values(&[], jpeg2000_template(0.0, 0, 0, 33), None, 4).expect_err("must reject");
        assert!(err.to_string().contains("exceeds 32"), "got: {err}");
    }

    #[test]
    fn jpeg2000_packing_degrades_on_malformed_stream() {
        // A non-J2K payload must surface as a recoverable UnsupportedSection
        // error (graceful degradation), never a panic.
        let err = decode_values(
            b"not a j2k stream",
            jpeg2000_template(0.0, 0, 0, 16),
            None,
            8,
        )
        .expect_err("must reject");
        assert!(
            matches!(err, FieldglassError::UnsupportedSection(_)),
            "expected UnsupportedSection, got: {err:?}"
        );
        assert!(err.to_string().contains("JPEG 2000 packing"), "got: {err}");
    }

    #[test]
    fn unsupported_template_yields_unsupported_error() {
        // 50 is unassigned in WMO Code Table 5.0 (40/41/42 all decode now).
        let template = DataRepresentationTemplate::Unsupported(50);
        let err = decode_values(&[], template, None, 0).expect_err("must reject");
        assert!(err.to_string().contains("template 5.50"));
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

    fn second_order_template(num_groups: u32, boustrophedonic: bool) -> SecondOrderPackingTemplate {
        SecondOrderPackingTemplate {
            reference_value: 5.0,
            binary_scale_factor: 0,
            decimal_scale_factor: 0,
            bits_per_value: 8,
            width_of_first_order_values: 8,
            num_groups,
            num_second_order_packed_values: 0,
            width_of_widths: 4,
            width_of_lengths: 8,
            boustrophedonic,
            order_of_spd: 0,
            width_of_spd: 0,
            spd_seeds: vec![],
            spd_bias: 0,
        }
    }

    #[test]
    fn second_order_ng0_is_constant_field() {
        // NG == 0: §7 carries no data, every point is R · 10^-D.
        let t = DataRepresentationTemplate::SecondOrder(second_order_template(0, false));
        let decoded = decode_values(&[], t, None, 4).expect("decode");
        assert_eq!(decoded, vec![Some(5.0); 4]);
    }

    #[test]
    fn second_order_decodes_single_group_zero_width() {
        // One zero-width group of 4 points, orderOfSPD=0, widthOfFirstOrder=8,
        // firstOrderValues[0]=7 → every point decodes to R + 7 = 12.
        let mut t = second_order_template(1, false);
        t.width_of_widths = 8;
        t.num_second_order_packed_values = 4;
        // §7: groupWidths[0]=0 (1 byte), groupLengths[0]=4 (1 byte),
        // firstOrderValues[0]=7 (1 byte). No second-order stream (width 0).
        let payload = vec![0u8, 4u8, 7u8];
        let template = DataRepresentationTemplate::SecondOrder(t);
        let decoded = decode_values(&payload, template, None, 4).expect("decode");
        assert_eq!(decoded, vec![Some(12.0); 4]);
    }

    #[test]
    fn undo_boustrophedonic_reverses_odd_rows() {
        // 2 columns × 3 rows: rows 1 (odd) reversed, rows 0 and 2 untouched.
        let template = DataRepresentationTemplate::SecondOrder(second_order_template(1, true));
        let mut v: Vec<Option<f64>> = (0..6).map(|i| Some(i as f64)).collect();
        undo_second_order_boustrophedonic(&mut v, &template, 2);
        let got: Vec<f64> = v.into_iter().map(|x| x.unwrap()).collect();
        // row0 [0,1] kept; row1 [2,3]→[3,2]; row2 [4,5] kept.
        assert_eq!(got, vec![0.0, 1.0, 3.0, 2.0, 4.0, 5.0]);
    }

    #[test]
    fn undo_boustrophedonic_is_noop_when_flag_clear() {
        let template = DataRepresentationTemplate::SecondOrder(second_order_template(1, false));
        let mut v: Vec<Option<f64>> = (0..6).map(|i| Some(i as f64)).collect();
        let before = v.clone();
        undo_second_order_boustrophedonic(&mut v, &template, 2);
        assert_eq!(v, before);
    }

    #[test]
    fn undo_boustrophedonic_is_noop_for_other_templates() {
        let template = simple_template(0.0, 0, 0, 8);
        let mut v: Vec<Option<f64>> = (0..6).map(|i| Some(i as f64)).collect();
        let before = v.clone();
        undo_second_order_boustrophedonic(&mut v, &template, 2);
        assert_eq!(v, before);
    }
}
