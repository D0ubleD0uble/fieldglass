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

    pub fn read_bits(&mut self, n: u8) -> Result<u32, FieldglassError> {
        if n == 0 {
            return Ok(0);
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
}
