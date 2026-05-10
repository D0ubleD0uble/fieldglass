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

    pub fn read_bits(&mut self, n: u8) -> Result<u32, FieldglassError> {
        if n == 0 {
            return Ok(0);
        }
        let end_bit = self.bit_offset + n as usize;
        if end_bit > self.bytes.len() * 8 {
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
    fn bit_reader_exhaustion() {
        let mut r = BitReader::new(&[0x00]);
        assert!(r.read_bits(9).is_err());
    }
}
