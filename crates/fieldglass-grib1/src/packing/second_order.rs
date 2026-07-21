//! GRIB1 BDS general-extended second-order (`grid_second_order_*`) decoder.
//!
//! Wire layout (translated from eccodes' `grib1/section.4.def` and
//! `grib1/data.grid_second_order.def`, byte offsets 0-indexed within the
//! BDS):
//!
//! ```text
//! 0..2    section_len
//! 3       flag (sph / complex / int / extra-flags / unused trailing bits)
//! 4..5    binaryScaleFactor (E, sign-magnitude i16)
//! 6..9    referenceValue (R, IBM single-precision)
//! 10      widthOfFirstOrderValues (= bits-per-value field repurposed)
//! 11..12  N1   — byte offset to first-order reference values
//! 13      extendedFlag — see ComplexExtendedHeader bit accessors
//! 14..15  N2   — byte offset to second-order packed values
//! 16..17  codedNumberOfGroups
//! 18..19  numberOfSecondOrderPackedValues (P2)
//! 20      extraValues (numberOfGroups = codedNumberOfGroups + 65536·extraValues)
//! 21      widthOfWidths
//! 22      widthOfLengths
//! 23..24  NL
//! 25      widthOfSPD       — only when orderOfSPD > 0
//! 26..    SPD predictor values, each at widthOfSPD bits, sign-magnitude
//!         (orderOfSPD of them); then group widths (numberOfGroups @
//!         widthOfWidths bits); then group lengths (numberOfGroups @
//!         widthOfLengths bits). May be followed by padding bits up to N1.
//! N1..    First-order reference values: numberOfGroups @
//!         widthOfFirstOrderValues bits each.
//! N2..    Second-order packed values: per group `g`, `groupLength[g]`
//!         values at `groupWidth[g]` bits each (zero-width groups encode
//!         no second-order values — every point in the group equals the
//!         group's first-order ref).
//! ```
//!
//! Reconstruction (matching eccodes'
//! `DataG1SecondOrderGeneralExtendedPacking::unpack`):
//!
//! 1. Read `orderOfSPD + 1` SPD values at `widthOfSPD` bits each. The
//!    first `orderOfSPD` are **unsigned** (the seed values `u[0..k]`); the
//!    last one is **signed** sign-magnitude and is the **bias** added at
//!    every reconstruction step.
//! 2. Decode the second-order grid into `X[orderOfSPD..]`: for each group,
//!    if `groupWidth > 0` read `groupLength` packed values and add the
//!    group's first-order reference to each; if `groupWidth == 0` write
//!    `groupLength` copies of the first-order reference.
//! 3. Plant the seeds: `X[0..orderOfSPD] = SPD[0..orderOfSPD]`.
//! 4. Apply the inverse spatial differencing using two/three running
//!    accumulators with `bias` added at each step (see `apply_spd_inverse`).
//! 5. Multiply by `2^E`, add `R`, divide by `10^D` to get final values.
//! 6. If a BMS bitmap is present, interleave `None` at masked positions.
//! 7. If boustrophedonicOrdering is set, reverse alternate rows (cols is
//!    needed for this — eccodes uses numberOfColumns from the GDS).

use fieldglass_core::{
    FieldglassError,
    bits::{
        BitReader, apply_spd_inverse, bits_to_bytes, expand_second_order_groups,
        sign_magnitude_to_i64,
    },
};

use crate::bds::BdsHeader;

/// Decode a `grid_second_order_*` (general-extended second-order) BDS.
///
/// Limitations: assumes `secondOrderOfDifferentWidth = 1`, `matrixOfValues
/// = 0`, `secondaryBitmapPresent = 0`, `generalExtended2ordr = 1`. Variants
/// outside that envelope are routed back as
/// `FieldglassError::UnsupportedSection` from the caller.
pub fn decode(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
    cols: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let ext = header.complex_extended.ok_or_else(|| {
        FieldglassError::Parse(
            "second-order decoder invoked without complex_extended (internal dispatch error)"
                .into(),
        )
    })?;

    let bds_len = header.section_len as usize;
    if bds.len() < bds_len {
        return Err(FieldglassError::Parse(format!(
            "BDS body shorter than declared section_len {bds_len}"
        )));
    }
    if bds_len < 25 {
        return Err(FieldglassError::Parse(format!(
            "BDS too short ({bds_len}) for second-order extended header"
        )));
    }

    // N1 and N2 are 1-indexed octet pointers per the WMO spec; convert to
    // 0-indexed byte offsets here so all downstream arithmetic stays in
    // Rust-native indexing.
    let n1_octet = ext.n1 as usize;
    let n2_octet = u16::from_be_bytes([bds[14], bds[15]]) as usize;
    if n1_octet == 0 || n2_octet == 0 {
        return Err(FieldglassError::Parse(format!(
            "BDS N1/N2 invalid: N1={n1_octet}, N2={n2_octet}"
        )));
    }
    let n1 = n1_octet - 1;
    let n2 = n2_octet - 1;
    let coded_num_groups = u16::from_be_bytes([bds[16], bds[17]]) as usize;
    let extra_values = bds[20] as usize;
    let width_of_widths = bds[21];
    let width_of_lengths = bds[22];

    // Each group covers ≥1 point, so num_groups can't legitimately exceed
    // expected_count — bounds the per-group alloc below.
    let num_groups = coded_num_groups
        .checked_add(65536usize.saturating_mul(extra_values))
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "BDS num_groups overflows usize (coded={coded_num_groups}, extra={extra_values})"
            ))
        })?;
    if num_groups == 0 {
        return Err(FieldglassError::Parse(
            "BDS reports zero groups for second-order packing".into(),
        ));
    }
    if num_groups > expected_count {
        return Err(FieldglassError::Parse(format!(
            "BDS reports {num_groups} groups but grid only has {expected_count} points"
        )));
    }
    if n2 < n1 {
        return Err(FieldglassError::Parse(format!(
            "BDS N1/N2 ordering invalid: N1={n1_octet}, N2={n2_octet}"
        )));
    }
    // `n2 == bds_len` is legitimate: a message whose groups are all
    // zero-width has no second-order packed values, so N2 (1-indexed) points
    // one past the last byte (`n2 = bds_len`). The resulting `&bds[n2..]`
    // slice is empty and only ever read for a non-zero-width group — and a
    // malformed message that claims `n2 == end` *and* has second-order values
    // hits the BitReader's exhaustion error below. Only `n2 > bds_len` is
    // out of bounds. Matches eccodes 2.34, which decodes such messages.
    if n2 > bds_len {
        return Err(FieldglassError::Parse(format!(
            "BDS N2={n2_octet} exceeds section_len={bds_len}"
        )));
    }

    let order_of_spd = ext.order_of_spd();
    if order_of_spd > 3 {
        return Err(FieldglassError::Parse(format!(
            "BDS reports orderOfSPD={order_of_spd} > 3"
        )));
    }

    // SPD, group-widths, and group-lengths are byte-aligned blocks of
    // ceil(count*width/8) bytes each (matches eccodes Spd/UnsignedBits).
    if width_of_widths > 32 {
        return Err(FieldglassError::Parse(format!(
            "BDS widthOfWidths={width_of_widths} > 32"
        )));
    }
    if width_of_lengths > 32 {
        return Err(FieldglassError::Parse(format!(
            "BDS widthOfLengths={width_of_lengths} > 32"
        )));
    }

    let mut byte_cursor: usize = 25;
    let mut spd_seeds: Vec<i64> = Vec::with_capacity(order_of_spd as usize);
    let mut bias: i64 = 0;
    if order_of_spd > 0 {
        if bds_len <= byte_cursor {
            return Err(FieldglassError::Parse(
                "BDS missing widthOfSPD octet despite orderOfSPD > 0".into(),
            ));
        }
        let width_of_spd = bds[byte_cursor];
        if width_of_spd == 0 || width_of_spd > 32 {
            return Err(FieldglassError::Parse(format!(
                "BDS widthOfSPD={width_of_spd} out of supported range 1..=32"
            )));
        }
        byte_cursor += 1;
        let spd_count = order_of_spd as usize + 1;
        let spd_bytes = bits_to_bytes(spd_count, width_of_spd as usize).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "BDS SPD byte length overflows ({spd_count}×{width_of_spd} bits)"
            ))
        })?;
        if bds_len < byte_cursor + spd_bytes {
            return Err(FieldglassError::Parse(
                "BDS too short for SPD section".into(),
            ));
        }
        let mut r = BitReader::new(&bds[byte_cursor..byte_cursor + spd_bytes]);
        for _ in 0..order_of_spd {
            spd_seeds.push(r.read_bits(width_of_spd)? as i64);
        }
        let raw_bias = r.read_bits(width_of_spd)?;
        bias = sign_magnitude_to_i64(raw_bias, width_of_spd);
        byte_cursor += spd_bytes;
    }

    let widths_bytes = bits_to_bytes(num_groups, width_of_widths as usize).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "BDS groupWidths byte length overflows ({num_groups}×{width_of_widths} bits)"
        ))
    })?;
    if bds_len < byte_cursor + widths_bytes {
        return Err(FieldglassError::Parse(
            "BDS too short for groupWidths section".into(),
        ));
    }
    let mut group_widths: Vec<u8> = Vec::with_capacity(num_groups);
    {
        let mut r = BitReader::new(&bds[byte_cursor..byte_cursor + widths_bytes]);
        for _ in 0..num_groups {
            group_widths.push(r.read_bits(width_of_widths)? as u8);
        }
    }
    byte_cursor += widths_bytes;

    let lengths_bytes = bits_to_bytes(num_groups, width_of_lengths as usize).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "BDS groupLengths byte length overflows ({num_groups}×{width_of_lengths} bits)"
        ))
    })?;
    if bds_len < byte_cursor + lengths_bytes {
        return Err(FieldglassError::Parse(
            "BDS too short for groupLengths section".into(),
        ));
    }
    let mut group_lengths: Vec<u32> = Vec::with_capacity(num_groups);
    {
        let mut r = BitReader::new(&bds[byte_cursor..byte_cursor + lengths_bytes]);
        for _ in 0..num_groups {
            group_lengths.push(r.read_bits(width_of_lengths)?);
        }
    }
    byte_cursor += lengths_bytes;
    // n1 comes from the wire; runtime check, not debug-only.
    if byte_cursor > n1 {
        return Err(FieldglassError::Parse(format!(
            "BDS groupLengths overflows N1 boundary (cursor={byte_cursor}, n1={n1})"
        )));
    }

    // First-order reference values start at byte N1, byte-aligned. Padding
    // between the group-descriptor stream and N1 is discarded silently.
    let width_of_first_order = header.bits_per_value;
    if width_of_first_order > 32 {
        return Err(FieldglassError::Parse(format!(
            "BDS widthOfFirstOrderValues={width_of_first_order} > 32"
        )));
    }
    let mut fo_reader = BitReader::new(&bds[n1..]);
    let mut first_order: Vec<u32> = Vec::with_capacity(num_groups);
    for _ in 0..num_groups {
        first_order.push(fo_reader.read_bits(width_of_first_order)?);
    }

    let mut so_reader = BitReader::new(&bds[n2..]);

    // checked sum: each group_length is u32, up to 2^32-1.
    let mut total_second: usize = 0;
    for &gl in &group_lengths {
        total_second = total_second.checked_add(gl as usize).ok_or_else(|| {
            FieldglassError::Parse("BDS group_lengths sum overflows usize".into())
        })?;
    }
    let total_decoded = total_second
        .checked_add(order_of_spd as usize)
        .ok_or_else(|| FieldglassError::Parse("BDS total decoded count overflows usize".into()))?;
    if total_decoded > expected_count {
        return Err(FieldglassError::Parse(format!(
            "BDS group lengths sum {total_decoded} exceeds grid size {expected_count}"
        )));
    }
    // SPD slots [0..order_of_spd] are planted below; second-order fills the
    // rest via the shared group-reconstruction loop.
    let mut x: Vec<i64> = vec![0; total_decoded];
    expand_second_order_groups(
        &mut so_reader,
        &mut x,
        order_of_spd as usize,
        (0..num_groups).map(|g| {
            (
                group_widths[g] as u32,
                group_lengths[g] as usize,
                first_order[g] as i64,
            )
        }),
    )?;

    for (i, &seed) in spd_seeds.iter().enumerate() {
        x[i] = seed;
    }
    apply_spd_inverse(&mut x, order_of_spd, bias)?;

    // Scale (R + u·2^E) / 10^D, undo boustrophedonic ordering, interleave the
    // bitmap — shared with the classic second-order decoders.
    super::finalize_second_order(
        x,
        header,
        decimal_scale,
        ext.boustrophedonic(),
        cols,
        bitmap,
        expected_count,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bds::ComplexExtendedHeader;

    /// Build a minimal `grid_second_order_SPD1` BDS whose single group is
    /// zero-width, so there are no second-order packed values and N2
    /// (1-indexed) points exactly one past the last byte — i.e. `n2 ==
    /// bds_len`. This is the boundary the decoder used to reject (issue #91);
    /// eccodes 2.34 decodes it. Layout follows the module doc-comment and
    /// `data.grid_second_order_SPD1.def`:
    ///
    /// - orderOfSPD = 1, widthOfSPD = 8, seed = 0, bias = 0
    /// - one group, groupWidth = 0, groupLength = `count`, firstOrderValues = 1
    ///
    /// The order-1 inverse over the seed 0 followed by `count` copies of the
    /// reference 1 is a cumulative sum, yielding the ramp `0, 1, …, count`.
    fn zero_width_spd1_bds(count: u16, n2_octet: u16) -> (Vec<u8>, BdsHeader) {
        const N1_OCTET: u16 = 32; // first-order values start at byte 31
        const SECTION_LEN: usize = 32; // bytes 0..=31
        let mut bds = vec![0u8; SECTION_LEN];
        bds[14..16].copy_from_slice(&n2_octet.to_be_bytes()); // N2
        bds[16..18].copy_from_slice(&1u16.to_be_bytes()); // codedNumberOfGroups
        bds[20] = 0; // extraValues
        bds[21] = 8; // widthOfWidths
        bds[22] = 16; // widthOfLengths
        bds[25] = 8; // widthOfSPD
        bds[26] = 0; // SPD seed (unsigned)
        bds[27] = 0; // SPD bias (sign-magnitude)
        bds[28] = 0; // groupWidths[0] = 0  → zero-width group
        bds[29..31].copy_from_slice(&count.to_be_bytes()); // groupLengths[0]
        bds[31] = 1; // firstOrderValues[0] = 1

        let header = BdsHeader {
            section_len: SECTION_LEN as u32,
            is_spherical_harmonic: false,
            is_complex_packing: true,
            is_integer_data: false,
            has_extra_flags: true,
            unused_trailing_bits: 0,
            binary_scale_factor: 0,
            reference_value: 0.0,
            bits_per_value: 8, // widthOfFirstOrderValues
            spherical_extended: None,
            complex_extended: Some(ComplexExtendedHeader {
                n1: N1_OCTET,
                // secondOrderOfDifferentWidth | generalExtended2ordr | SPD order 1
                extended_flag: 0x10 | 0x08 | 0x01,
            }),
        };
        (bds, header)
    }

    #[test]
    fn decodes_all_zero_width_group_with_n2_at_section_end() {
        // Regression for #91: n2 == bds_len must be accepted, not rejected.
        let count = 7u16;
        let expected_count = count as usize + 1; // + orderOfSPD seed
        let (bds, header) = zero_width_spd1_bds(count, 33); // N2 = section_len + 1
        let out = decode(&bds, &header, 0, None, expected_count, expected_count)
            .expect("all-zero-width second-order BDS should decode");
        let present: Vec<f64> = out.into_iter().map(|v| v.expect("no missing")).collect();
        let want: Vec<f64> = (0..=count).map(f64::from).collect();
        assert_eq!(present, want);
    }

    #[test]
    fn rejects_n2_past_section_end() {
        // The relaxed guard must still catch a genuinely out-of-bounds N2.
        let count = 7u16;
        let (bds, header) = zero_width_spd1_bds(count, 34); // N2 = section_len + 2
        let err = decode(
            &bds,
            &header,
            0,
            None,
            count as usize + 1,
            count as usize + 1,
        )
        .expect_err("N2 beyond section end must be rejected");
        match err {
            FieldglassError::Parse(msg) => assert!(msg.contains("N2=34"), "msg = {msg:?}"),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }
}
