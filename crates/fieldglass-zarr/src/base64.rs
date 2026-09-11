//! Standard base64, decoded strictly.
//!
//! Two readers need it. A kerchunk reference document stores a small chunk
//! inline as `base64:…` (`fieldglass-fetchplan`), and xarray writes a floating
//! `_FillValue` into a Zarr v3 array's attributes as base64 of its bytes, since
//! JSON has no way to spell a NaN (the store walker, #658). It lives here, in
//! the crate both can reach, rather than once in each.
//!
//! Hand-rolled rather than taken as a dependency: the alternative to forty
//! lines of well-specified arithmetic is another entry in every downstream
//! consumer's licence scan. Strict on purpose — no whitespace, no alternative
//! alphabet, no missing padding — because a lenient decoder turns a corrupt
//! document into plausible bytes, and the fuzz targets drive this.

/// Decode standard base64, or `None` for anything that is not exactly that.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }

    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (block, quad) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let last = block == bytes.len() / 4 - 1;
        // Padding is only ever the last one or two characters of the last
        // quad: `=` anywhere else is a corrupt document, not a short one.
        let padding = if last {
            quad.iter().filter(|b| **b == b'=').count()
        } else {
            0
        };
        if padding > 2 || quad[..4 - padding].contains(&b'=') {
            return None;
        }
        let mut packed = 0u32;
        for byte in &quad[..4 - padding] {
            packed = (packed << 6) | sextet(*byte)?;
        }
        // The bits a padded quad does not carry must be zero, or two distinct
        // encodings would decode to the same bytes.
        packed <<= 6 * padding;
        let decoded = packed.to_be_bytes();
        if padding > 0 && decoded[4 - padding..].iter().any(|b| *b != 0) {
            return None;
        }
        out.extend_from_slice(&decoded[1..4 - padding]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base64 value decodes to bytes, and a corrupt one is refused rather
    /// than truncated into plausible data.
    #[test]
    fn base64_values_decode_strictly() {
        assert_eq!(decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode("aGVsbG8h").unwrap(), b"hello!");
        assert_eq!(decode("").unwrap(), b"");
        assert_eq!(decode("TQ==").unwrap(), b"M");

        // Unpadded, mispadded, out of alphabet, and non-zero trailing bits.
        assert!(decode("aGVsbG8").is_none());
        assert!(decode("a=VsbG8=").is_none());
        assert!(decode("aGVs bG8=").is_none());
        assert!(decode("aGVsbG8*").is_none());
        assert!(decode("TR==").is_none());
    }

    /// The `_FillValue` xarray writes for a float array under Zarr v3 — the
    /// bytes of a little-endian `f64` — decodes to the number it stands for.
    #[test]
    fn an_xarray_fill_value_decodes_to_its_float() {
        let bytes = decode("AAAAAICHw8A=").unwrap();
        assert_eq!(f64::from_le_bytes(bytes.try_into().unwrap()), -9999.0);
    }
}
