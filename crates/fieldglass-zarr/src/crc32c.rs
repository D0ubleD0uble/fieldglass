//! CRC-32C (Castagnoli), the checksum Zarr v3's `crc32c` codec appends.
//!
//! Not the CRC-32 of zlib and PNG: a different generator polynomial, so a
//! decoder that reached for the familiar one would reject every conforming
//! shard index. The reflected form is used, which is what RFC 3720 (iSCSI)
//! specifies and what every implementation of this codec computes.
//!
//! The codec is `bytes -> bytes`: encoding appends the checksum as a 32-bit
//! little-endian integer, so decoding splits those four bytes off, checks them,
//! and keeps the rest. Nothing else about the payload changes.

use fieldglass_core::FieldglassError;

/// Reflected CRC-32C generator (`0x1EDC6F41` reversed).
const POLY: u32 = 0x82F6_3B78;

/// The checksum's width in bytes, as stored.
pub const CHECKSUM_LEN: usize = 4;

/// The CRC-32C of a buffer.
#[must_use]
pub fn checksum(data: &[u8]) -> u32 {
    // Computed bitwise rather than from a table: these payloads are a shard
    // index — sixteen bytes per inner chunk — so the table would cost more
    // cache than the loop costs cycles, and a table is a second place for the
    // polynomial to be written down wrongly.
    let mut crc = !0u32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (POLY & mask);
        }
    }
    !crc
}

/// Verify and strip a trailing CRC-32C, which is what decoding the codec means.
pub fn verify_and_strip(data: &[u8]) -> Result<Vec<u8>, FieldglassError> {
    let split = data.len().checked_sub(CHECKSUM_LEN).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "a {}-byte buffer is too short to carry a crc32c checksum",
            data.len()
        ))
    })?;
    let (body, tail) = data.split_at(split);
    let stored = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
    let computed = checksum(body);
    if stored != computed {
        return Err(FieldglassError::Parse(format!(
            "crc32c mismatch: stored {stored:#010x}, computed {computed:#010x} over \
             {} bytes — the shard index is corrupt",
            body.len()
        )));
    }
    Ok(body.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published CRC-32C check values, which no second copy of this
    /// arithmetic could agree with by accident. `"123456789"` is the standard
    /// check vector for every CRC in the catalogue.
    #[test]
    fn matches_the_published_check_values() {
        assert_eq!(checksum(b""), 0x0000_0000);
        assert_eq!(checksum(b"123456789"), 0xE306_9283);
        assert_eq!(checksum(b"a"), 0xC1D0_4330);
        assert_eq!(checksum(&[0u8; 32]), 0x8A91_36AA);
    }

    /// The familiar zlib CRC-32 of the same string is a different number.
    /// Pinning that keeps a future "simplification" onto the wrong polynomial
    /// from passing.
    #[test]
    fn is_not_the_zlib_crc32() {
        assert_ne!(checksum(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn strips_a_good_checksum_and_refuses_a_bad_one() {
        let body = b"the shard index".to_vec();
        let mut framed = body.clone();
        framed.extend_from_slice(&checksum(&body).to_le_bytes());
        assert_eq!(verify_and_strip(&framed).unwrap(), body);

        framed[2] ^= 0xFF;
        assert!(verify_and_strip(&framed).is_err());
        assert!(verify_and_strip(&[1, 2]).is_err());
    }
}
