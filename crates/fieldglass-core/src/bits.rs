//! Bit-level helpers shared by the binary meteorological format crates.
//!
//! These primitives — sign-magnitude integers, IBM single-precision floats,
//! and an MSB-first bit reader for packed integers up to 32 bits — show up
//! in the wire formats of GRIB1, GRIB2, and BUFR alike. Keeping them here
//! lets each format crate reach for the same utilities without re-deriving
//! them.

use crate::FieldglassError;

/// 16-bit sign-magnitude integer used by GRIB for binary scale factors.
/// High bit is sign, low 15 bits are magnitude. Negative zero collapses
/// to `0` (the wire encoding is well-defined; the value isn't).
pub fn sign_magnitude_i16(raw: u16) -> i16 {
    let magnitude = (raw & 0x7FFF) as i16;
    if raw & 0x8000 != 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// Sign-magnitude to signed integer for arbitrary widths up to 32 bits.
/// The high bit of the `width`-bit field is the sign; the lower `width-1`
/// bits are the magnitude. `width == 0` returns 0.
pub fn sign_magnitude_to_i64(raw: u32, width: u8) -> i64 {
    if width == 0 {
        return 0;
    }
    let sign_bit = 1u32 << (width - 1);
    let mag_mask = sign_bit - 1;
    let mag = (raw & mag_mask) as i64;
    if raw & sign_bit != 0 { -mag } else { mag }
}

/// Bytes needed to hold `count * bits_per_value` bits, rounded up. Returns
/// `None` on `usize` overflow so callers can build a parse error with the
/// field name they have on hand.
pub fn bits_to_bytes(count: usize, bits_per_value: usize) -> Option<usize> {
    count
        .checked_mul(bits_per_value)
        .map(|bits| bits.div_ceil(8))
}

/// Inverse spatial-predictor differencing of order `k` in place, mirroring
/// eccodes' `DataG1SecondOrderGeneralExtendedPacking::unpack` (and the
/// GRIB2 second-order templates 5.50001 / 5.50002, whose §7 shares the same
/// codec). The first `order` slots of `x` hold the SPD seeds; the recurrence
/// reconstructs the rest, adding `bias` at each step. The y/z/w initialisation
/// re-uses values about to be overwritten — kept in this exact shape to stay
/// bit-identical to eccodes. Wrapping arithmetic matches eccodes' implicit
/// two's-complement C and avoids overflow panics on extreme (malformed) inputs.
/// Orders 0–3 are defined by WMO Code Table 5.6; a higher order is a parse
/// error.
pub fn apply_spd_inverse(x: &mut [i64], order: u8, bias: i64) -> Result<(), FieldglassError> {
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

/// IBM System/360 single-precision float → `f64`.
/// Layout: sign (1) | characteristic (7, excess-64) | fraction (24), base 16.
pub fn ibm_float_to_f64(raw: u32) -> f64 {
    if raw == 0 {
        return 0.0;
    }
    let sign = if raw & 0x8000_0000 != 0 { -1.0 } else { 1.0 };
    let characteristic = ((raw >> 24) & 0x7F) as i32;
    let fraction = (raw & 0x00FF_FFFF) as f64 / (1u32 << 24) as f64;
    sign * fraction * 16f64.powi(characteristic - 64)
}

/// MSB-first bit reader for packed integer streams up to 32 bits per value.
/// Used by GRIB BDS / DRS decoders and any future format that packs values
/// at non-byte-aligned widths.
pub struct BitReader<'a> {
    bytes: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_offset: 0,
        }
    }

    /// Advance the cursor to the next byte boundary, discarding any unused
    /// bits in the current byte. A no-op when already byte-aligned. GRIB2
    /// complex packing (§7) stores the group reference / width / length / data
    /// sub-blocks each starting on an octet boundary, so decoders pad to the
    /// next byte between them.
    pub fn align_to_byte(&mut self) {
        let rem = self.bit_offset % 8;
        if rem != 0 {
            self.bit_offset += 8 - rem;
        }
    }

    /// Read the next `n` bits (MSB-first) as an unsigned integer.
    ///
    /// `n` must be in `0..=32`: the value is returned as a `u32`, so a wider
    /// request can't be represented and is rejected. Without this guard a
    /// request for `n > 32` would accumulate into the internal `u64` and then
    /// silently truncate its top bits on the `as u32` return. Callers bound the
    /// stored field width to 32; enforcing it here makes the contract explicit
    /// and turns a would-be silent wrong result into a clean error on malformed
    /// input (e.g. a per-group residual width read from an untrusted GRIB
    /// stream).
    pub fn read_bits(&mut self, n: u8) -> Result<u32, FieldglassError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 {
            return Err(FieldglassError::Parse(format!(
                "bit reader asked for {n} bits, but read_bits returns a u32 (max 32)"
            )));
        }
        // checked: bit_offset near usize::MAX would wrap past the bounds check.
        let end_bit = self
            .bit_offset
            .checked_add(n as usize)
            .ok_or_else(|| FieldglassError::Parse("bit reader offset overflow".into()))?;
        let total_bits = self
            .bytes
            .len()
            .checked_mul(8)
            .ok_or_else(|| FieldglassError::Parse("bit reader length overflow".into()))?;
        if end_bit > total_bits {
            return Err(FieldglassError::Parse(format!(
                "bit reader exhausted at offset {} reading {n} bits",
                self.bit_offset
            )));
        }

        let mut value: u64 = 0;
        let mut bits_collected = 0u8;
        let mut bit = self.bit_offset;
        while bits_collected < n {
            let byte_idx = bit / 8;
            let bit_in_byte = bit % 8;
            let take = (8 - bit_in_byte).min((n - bits_collected) as usize) as u8;
            let shift = 8 - bit_in_byte - take as usize;
            let mask = ((1u16 << take) - 1) as u8;
            let chunk = (self.bytes[byte_idx] >> shift) & mask;
            value = (value << take) | chunk as u64;
            bits_collected += take;
            bit += take as usize;
        }
        self.bit_offset = end_bit;
        Ok(value as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ibm_float_zero() {
        assert_eq!(ibm_float_to_f64(0x0000_0000), 0.0);
    }

    #[test]
    fn ibm_float_one_half() {
        // 0.5 = 0x40 80 00 00: char=64 → exp 0, fraction = 0x800000/2^24 = 0.5.
        assert!((ibm_float_to_f64(0x4080_0000) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn ibm_float_negative_one_half() {
        assert!((ibm_float_to_f64(0xC080_0000) + 0.5).abs() < 1e-12);
    }

    #[test]
    fn ibm_float_one() {
        // 1.0 = 0x41 10 00 00: char=65 → exp 1, fraction = 0x100000/2^24 = 1/16, *16 = 1.0.
        assert!((ibm_float_to_f64(0x4110_0000) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn sign_magnitude_positive() {
        assert_eq!(sign_magnitude_i16(0x0005), 5);
    }

    #[test]
    fn sign_magnitude_negative() {
        assert_eq!(sign_magnitude_i16(0x8005), -5);
    }

    #[test]
    fn sign_magnitude_zero_signed() {
        // Negative zero is well-defined in sign-magnitude; we collapse it to 0.
        assert_eq!(sign_magnitude_i16(0x8000), 0);
    }

    #[test]
    fn sign_magnitude_i64_basic() {
        assert_eq!(sign_magnitude_to_i64(0b0_0101, 5), 5);
        assert_eq!(sign_magnitude_to_i64(0b1_0101, 5), -5);
        assert_eq!(sign_magnitude_to_i64((1 << 20) | 7, 21), -7);
        assert_eq!(sign_magnitude_to_i64(0, 0), 0);
    }

    #[test]
    fn bits_to_bytes_rounds_up() {
        assert_eq!(bits_to_bytes(1, 1), Some(1));
        assert_eq!(bits_to_bytes(8, 1), Some(1));
        assert_eq!(bits_to_bytes(9, 1), Some(2));
        assert_eq!(bits_to_bytes(0, 32), Some(0));
        assert_eq!(bits_to_bytes(7, 8), Some(7));
    }

    #[test]
    fn bits_to_bytes_overflow_returns_none() {
        assert_eq!(bits_to_bytes(usize::MAX, 2), None);
    }

    #[test]
    fn bit_reader_byte_aligned() {
        let mut r = BitReader::new(&[0xAB, 0xCD]);
        assert_eq!(r.read_bits(8).unwrap(), 0xAB);
        assert_eq!(r.read_bits(8).unwrap(), 0xCD);
    }

    #[test]
    fn bit_reader_unaligned() {
        // 0b1010_0101_1100_0011 — read as 3,5,8 bits MSB-first.
        let mut r = BitReader::new(&[0b1010_0101, 0b1100_0011]);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.read_bits(5).unwrap(), 0b00101);
        assert_eq!(r.read_bits(8).unwrap(), 0b1100_0011);
    }

    #[test]
    fn bit_reader_crosses_byte_boundary() {
        let mut r = BitReader::new(&[0xFF, 0x00]);
        assert_eq!(r.read_bits(12).unwrap(), 0xFF0);
    }

    #[test]
    fn bit_reader_align_to_byte() {
        let mut r = BitReader::new(&[0b1010_1111, 0b0011_0000]);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        r.align_to_byte(); // skip the remaining 5 bits of byte 0
        assert_eq!(r.read_bits(4).unwrap(), 0b0011);
        // Already aligned here (4 bits into byte 1 is not, but align skips to byte 2).
        r.align_to_byte();
        assert!(r.read_bits(1).is_err(), "only 2 bytes of input");
    }

    #[test]
    fn bit_reader_align_is_noop_when_aligned() {
        let mut r = BitReader::new(&[0xAB, 0xCD]);
        assert_eq!(r.read_bits(8).unwrap(), 0xAB);
        r.align_to_byte(); // already on byte boundary — no-op
        assert_eq!(r.read_bits(8).unwrap(), 0xCD);
    }

    #[test]
    fn bit_reader_exhaustion() {
        let mut r = BitReader::new(&[0x00]);
        assert!(r.read_bits(9).is_err());
    }

    #[test]
    fn bit_reader_reads_full_32_bits() {
        // The boundary of the contract: 32 bits round-trips losslessly.
        let mut r = BitReader::new(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(r.read_bits(32).unwrap(), 0xFFFF_FFFF);
        let mut r = BitReader::new(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(r.read_bits(32).unwrap(), 0x1234_5678);
    }

    #[test]
    fn bit_reader_rejects_more_than_32_bits() {
        // n > 32 can't fit in the returned u32. It must error rather than read
        // the bits and silently truncate the top ones via `as u32`. The buffer
        // is deliberately long enough that the request would otherwise succeed.
        let bytes = [0xAAu8; 8];
        let mut r = BitReader::new(&bytes);
        assert!(r.read_bits(33).is_err());
        assert!(r.read_bits(40).is_err());
        // A rejected read must not advance the cursor.
        assert_eq!(r.bit_offset, 0);
        // A valid read right afterwards still works.
        assert_eq!(r.read_bits(8).unwrap(), 0xAA);
    }

    #[test]
    fn spd_inverse_order1_is_cumulative_sum_with_bias() {
        // Order-1 reconstructs running sum y starting at X[0], adding
        // X[i] + bias at each step. With bias=0 it's a plain cumulative sum.
        let mut seq = vec![10i64, 1, 2, 3];
        apply_spd_inverse(&mut seq, 1, 0).unwrap();
        assert_eq!(seq, vec![10, 11, 13, 16]);

        // Bias of 1 shifts each successive y by +1 cumulatively.
        let mut seq = vec![10i64, 1, 2, 3];
        apply_spd_inverse(&mut seq, 1, 1).unwrap();
        // y starts at 10. y += 1+1=2 → 12; y += 2+1=3 → 15; y += 3+1=4 → 19.
        assert_eq!(seq, vec![10, 12, 15, 19]);
    }

    // Overflow regression: pre-fix, plain `+=` would panic in debug. Values are
    // unspecified at the i64 boundary; just verify the loop ran (tail slot
    // mutated from its sentinel) without panicking.
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
        // Values u[i] = i*i, second-order forward differences with bias 0,
        // then the inverse. After SPD-2 inverse with seeds u[0]=0, u[1]=1,
        // deltas [2,2,2,2] and bias=0:
        //   y_init = X[1] - X[0] = 1; z_init = X[1] = 1
        //   i=2: y += 2 → 3; z += 3 → 4
        //   i=3: y += 2 → 5; z += 5 → 9
        //   i=4: y += 2 → 7; z += 7 → 16
        //   i=5: y += 2 → 9; z += 9 → 25
        // → [0, 1, 4, 9, 16, 25]   (the squares!)
        let mut seq = vec![0i64, 1, 2, 2, 2, 2];
        apply_spd_inverse(&mut seq, 2, 0).unwrap();
        assert_eq!(seq, vec![0, 1, 4, 9, 16, 25]);
    }

    #[test]
    fn spd_inverse_rejects_order_above_3() {
        let mut seq = vec![0i64, 1, 2, 3];
        assert!(apply_spd_inverse(&mut seq, 4, 0).is_err());
    }
}
