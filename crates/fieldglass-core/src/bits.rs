//! A bit reader that reads values of any bit width from packed bytes, and
//! functions that help decode specific byte formats.
//!
//! GRIB packs most of its values at a bit width the file chooses, back to back
//! with no padding, most significant bit first. [`BitReader`] reads them. Each
//! [`read_bits`](BitReader::read_bits) call takes the next 0 to 32 bits as an
//! unsigned number, and [`align_to_byte`](BitReader::align_to_byte) skips to
//! the start of the next byte.
//!
//! The bit reader carries a machine-checked proof, made with the Verus
//! verifier. For any input, a read either returns exactly the bits asked for
//! and moves past them, or fails and leaves the position where it was. It
//! never reads past the end of the buffer, overflows or panics.
//!
//! The functions read number formats the binary weather formats use:
//!
//! - [`sign_magnitude_i16`] and [`sign_magnitude_to_i64`] read signed
//!   integers stored as a sign bit plus a size, rather than in two's
//!   complement. GRIB stores its scale factors this way.
//! - [`ibm_float_to_f64`] reads IBM's 32-bit floating-point format, which
//!   GRIB1 uses for reference values.
//! - [`bits_to_bytes`] works out how many bytes a run of packed values takes,
//!   or `None` if that overflows.

// The bit reader is a verified kernel (#771), so it sits in a file of its own
// that the verification crate includes; see `docs/verification.md`.
mod reader;

pub use reader::BitReader;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// IBM single precision has no NaN or infinity: every bit pattern is a
    /// number, at most 16^63 in magnitude, so a GRIB1 float field is always
    /// finite and needs no check a GRIB2 IEEE one does (#823). Every
    /// characteristic with the largest and smallest fractions, both signs.
    #[test]
    fn every_ibm_float_is_finite() {
        for sign in [0u32, 0x8000_0000] {
            for characteristic in 0u32..128 {
                for fraction in [0x00_0001, 0x10_0000, 0x80_0000, 0xFF_FFFF] {
                    let raw = sign | characteristic << 24 | fraction;
                    let v = ibm_float_to_f64(raw);
                    assert!(v.is_finite(), "{raw:#010x} -> {v}");
                }
            }
        }
        assert!(ibm_float_to_f64(0x7FFF_FFFF) > 7.2e75);
    }

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
}
