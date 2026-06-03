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
    bits::{BitReader, bits_to_bytes, sign_magnitude_to_i64},
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
    if n2 >= bds_len {
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
    let mut x: Vec<i64> = vec![0; total_decoded];

    // SPD slots [0..order_of_spd] are filled below; second-order fills the rest.
    let mut n = order_of_spd as usize;
    for g in 0..num_groups {
        let w = group_widths[g];
        let count = group_lengths[g] as usize;
        let ref_val = first_order[g] as i64;
        if w == 0 {
            // Zero-width group: every point equals the first-order reference.
            for _ in 0..count {
                x[n] = ref_val;
                n += 1;
            }
        } else {
            for _ in 0..count {
                let raw = so_reader.read_bits(w)? as i64;
                // wrapping_add matches eccodes' implicit 2's-complement C.
                x[n] = ref_val.wrapping_add(raw);
                n += 1;
            }
        }
    }
    debug_assert_eq!(n, total_decoded);

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

/// Inverse spatial differencing of order k in place, mirroring
/// `DataG1SecondOrderGeneralExtendedPacking::unpack()`. Seeds in `x[0..k]`;
/// recurrence reconstructs the rest with `bias` added each step. The y/z/w
/// init dance re-uses values about to be overwritten — kept in this shape
/// to stay bit-identical to eccodes. Wrapping arithmetic matches eccodes'
/// implicit 2's-complement and avoids overflow panics on extreme inputs.
fn apply_spd_inverse(x: &mut [i64], order: u8, bias: i64) -> Result<(), FieldglassError> {
    match order {
        0 => Ok(()),
        1 => {
            // y = X[0]; for i=1..N: y += X[i] + bias; X[i] = y
            let mut y = x[0];
            for v in x.iter_mut().skip(1) {
                y = y.wrapping_add(v.wrapping_add(bias));
                *v = y;
            }
            Ok(())
        }
        2 => {
            // y = X[1] - X[0];  z = X[1];
            // for i=2..N: y += X[i] + bias; z += y; X[i] = z
            if x.len() < 2 {
                return Ok(());
            }
            let mut y = x[1].wrapping_sub(x[0]);
            let mut z = x[1];
            for v in x.iter_mut().skip(2) {
                y = y.wrapping_add(v.wrapping_add(bias));
                z = z.wrapping_add(y);
                *v = z;
            }
            Ok(())
        }
        3 => {
            // y = X[2] - X[1];  z = y - (X[1] - X[0]);  w = X[2];
            // for i=3..N: z += X[i] + bias; y += z; w += y; X[i] = w
            if x.len() < 3 {
                return Ok(());
            }
            let mut y = x[2].wrapping_sub(x[1]);
            let mut z = y.wrapping_sub(x[1].wrapping_sub(x[0]));
            let mut w = x[2];
            for v in x.iter_mut().skip(3) {
                z = z.wrapping_add(v.wrapping_add(bias));
                y = y.wrapping_add(z);
                w = w.wrapping_add(y);
                *v = w;
            }
            Ok(())
        }
        _ => Err(FieldglassError::Parse(format!(
            "unsupported SPD order {order}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spd_inverse_order1_is_cumulative_sum_with_bias() {
        // Order-1 reconstructs running sum y starting at X[0], adding
        // X[i] + bias at each step. With bias=0 it's a plain cumulative
        // sum.
        let mut seq = vec![10i64, 1, 2, 3];
        apply_spd_inverse(&mut seq, 1, 0).unwrap();
        assert_eq!(seq, vec![10, 11, 13, 16]);

        // Bias of 1 shifts each successive y by +1 cumulatively.
        let mut seq = vec![10i64, 1, 2, 3];
        apply_spd_inverse(&mut seq, 1, 1).unwrap();
        // y starts at 10. y += 1+1=2 → 12; y += 2+1=3 → 15; y += 3+1=4 → 19.
        assert_eq!(seq, vec![10, 12, 15, 19]);
    }

    // Overflow regression: pre-fix, plain `+=` would panic in debug.
    // Values are unspecified at the i64 boundary; just verify the loop
    // ran (tail slot mutated from its sentinel) without panicking.
    #[test]
    fn spd_inverse_order1_does_not_panic_on_overflow() {
        let mut seq = vec![i64::MAX, 1, 2, 0];
        apply_spd_inverse(&mut seq, 1, i64::MAX).unwrap();
        assert_ne!(seq[3], 0, "tail slot must be reconstructed");
    }

    #[test]
    fn spd_inverse_order3_does_not_panic_on_overflow() {
        let mut seq = vec![i64::MIN, i64::MAX, i64::MIN, 1, 2, 0];
        apply_spd_inverse(&mut seq, 3, i64::MIN).unwrap();
        assert_ne!(seq[5], 0, "tail slot must be reconstructed");
    }

    #[test]
    fn spd_inverse_order2_reconstructs_quadratic_with_zero_bias() {
        // Take values u[i] = i*i, compute second-order forward differences
        // with bias 0, then run the inverse. eccodes' encoding for
        // SPD-2 gives bias = X[2] - 2*X[1] + X[0] for the first delta;
        // here we just verify the loop matches eccodes' running-y/z
        // recurrence directly for a known input.
        //
        // After SPD-2 inverse with seeds u[0]=0, u[1]=1, deltas [2,2,2,2]
        // and bias=0:
        //   y_init = X[1] - X[0] = 1
        //   z_init = X[1] = 1
        //   i=2: y += X[2] + bias = 2, y becomes 3; z += y, z becomes 4
        //   i=3: y += X[3] + 0 = 2, y becomes 5; z += y, z becomes 9
        //   i=4: y += 2 = 7; z += 7 = 16
        //   i=5: y += 2 = 9; z += 9 = 25
        // → [0, 1, 4, 9, 16, 25]   (the squares!)
        let mut seq = vec![0i64, 1, 2, 2, 2, 2];
        apply_spd_inverse(&mut seq, 2, 0).unwrap();
        assert_eq!(seq, vec![0, 1, 4, 9, 16, 25]);
    }
}
