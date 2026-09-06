//! The two DEFLATE wrappers Zarr reaches: zlib and gzip.
//!
//! Both hold the same compressed stream (RFC 1951) behind different framing,
//! and Zarr's two editions ask for different ones — the numcodecs `zlib` codec
//! is RFC 1950 and both the numcodecs `gzip` codec and Zarr v3's `gzip` are
//! RFC 1952. Handing a gzip stream to a zlib reader fails on the first byte, so
//! the distinction is not academic.
//!
//! `miniz_oxide` reads zlib and raw streams; the gzip member header is parsed
//! here, because it is a handful of optional fields and one of them
//! (`FEXTRA`) is length-prefixed in a way a fixed-size skip gets wrong.

use fieldglass_core::FieldglassError;

/// Inflate an RFC 1950 zlib stream.
pub fn inflate_zlib(data: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit)
        .map_err(|e| FieldglassError::Parse(format!("zlib stream failed to inflate: {e:?}")))
}

/// Inflate an RFC 1952 gzip member: parse the header, inflate the raw stream,
/// and check the trailer's length against what came out.
///
/// The trailing CRC-32 is *not* checked. It would be a second checksum over
/// data the storage layer has already checksummed, and `miniz_oxide` has no
/// bounded entry point that verifies it; the length check below is the part
/// that catches a truncated member, which is the failure that actually
/// happens.
pub fn inflate_gzip(data: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    let body = gzip_body(data)?;
    let out = miniz_oxide::inflate::decompress_to_vec_with_limit(body, limit)
        .map_err(|e| FieldglassError::Parse(format!("gzip stream failed to inflate: {e:?}")))?;
    // ISIZE is the uncompressed length modulo 2^32, which is exactly how the
    // trailer states it, so the comparison is on the low 32 bits.
    let isize_field = u32::from_le_bytes([
        data[data.len() - 4],
        data[data.len() - 3],
        data[data.len() - 2],
        data[data.len() - 1],
    ]);
    if out.len() as u32 != isize_field {
        return Err(FieldglassError::Parse(format!(
            "gzip member decoded to {} bytes but its trailer declares {isize_field}",
            out.len()
        )));
    }
    Ok(out)
}

/// The raw DEFLATE bytes of a gzip member: everything between the header's
/// optional fields and the eight-byte trailer.
fn gzip_body(data: &[u8]) -> Result<&[u8], FieldglassError> {
    /// Header up to and including the OS byte, plus the eight-byte trailer.
    const FIXED: usize = 10 + 8;
    if data.len() < FIXED {
        return Err(FieldglassError::Parse(format!(
            "gzip member is {} bytes, too short to hold a header and a trailer",
            data.len()
        )));
    }
    if data[0] != 0x1F || data[1] != 0x8B {
        return Err(FieldglassError::Parse(
            "gzip member does not start with the 1f 8b magic".to_string(),
        ));
    }
    if data[2] != 8 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "gzip compression method {} is not DEFLATE",
            data[2]
        )));
    }
    let flags = data[3];
    let mut pos = 10usize;
    let end = data.len() - 8;

    // FEXTRA: a two-byte length then that many bytes.
    if flags & 0x04 != 0 {
        let len_bytes = data
            .get(pos..pos + 2)
            .ok_or_else(|| truncated("FEXTRA length"))?;
        let extra = u16::from_le_bytes([len_bytes[0], len_bytes[1]]) as usize;
        pos = pos
            .checked_add(2 + extra)
            .filter(|&p| p <= end)
            .ok_or_else(|| truncated("FEXTRA field"))?;
    }
    // FNAME and FCOMMENT: NUL-terminated strings.
    for (bit, what) in [(0x08u8, "FNAME"), (0x10, "FCOMMENT")] {
        if flags & bit != 0 {
            let rest = data.get(pos..end).ok_or_else(|| truncated(what))?;
            let nul = rest
                .iter()
                .position(|&b| b == 0)
                .ok_or_else(|| truncated(what))?;
            pos += nul + 1;
        }
    }
    // FHCRC: a two-byte header checksum.
    if flags & 0x02 != 0 {
        pos = pos
            .checked_add(2)
            .filter(|&p| p <= end)
            .ok_or_else(|| truncated("FHCRC field"))?;
    }
    data.get(pos..end)
        .ok_or_else(|| truncated("deflate stream"))
}

fn truncated(what: &str) -> FieldglassError {
    FieldglassError::Parse(format!("gzip member ends inside its {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a gzip member around a raw deflate stream, with whichever optional
    /// header fields are asked for. Hand-built rather than produced by an
    /// encoder, so the test states the framing rather than replaying it.
    fn gzip(payload: &[u8], flags: u8, extras: &[u8]) -> Vec<u8> {
        let raw = miniz_oxide::deflate::compress_to_vec(payload, 6);
        let mut out = vec![0x1F, 0x8B, 8, flags, 0, 0, 0, 0, 0, 0xFF];
        out.extend_from_slice(extras);
        out.extend_from_slice(&raw);
        out.extend_from_slice(&0u32.to_le_bytes()); // CRC-32, not checked
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out
    }

    #[test]
    fn inflates_a_plain_gzip_member() {
        let payload: Vec<u8> = (0..200u8).collect();
        assert_eq!(
            inflate_gzip(&gzip(&payload, 0, &[]), 1 << 20).unwrap(),
            payload
        );
    }

    /// Each optional header field moves the start of the deflate stream, and
    /// `FEXTRA` moves it by an amount only its own length prefix states. A
    /// fixed-size skip decodes garbage here.
    #[test]
    fn skips_every_optional_header_field() {
        let payload = b"the quick brown fox".to_vec();
        // FEXTRA (0x04): two length bytes then five bytes of subfield.
        let extra = [0x05, 0x00, 1, 2, 3, 4, 5];
        assert_eq!(
            inflate_gzip(&gzip(&payload, 0x04, &extra), 1 << 20).unwrap(),
            payload
        );
        // FNAME (0x08) and FCOMMENT (0x10), both NUL-terminated.
        assert_eq!(
            inflate_gzip(&gzip(&payload, 0x08, b"chunk.bin\0"), 1 << 20).unwrap(),
            payload
        );
        assert_eq!(
            inflate_gzip(&gzip(&payload, 0x18, b"chunk.bin\0a comment\0"), 1 << 20).unwrap(),
            payload
        );
        // FHCRC (0x02): two more bytes before the stream.
        assert_eq!(
            inflate_gzip(&gzip(&payload, 0x02, &[0xAB, 0xCD]), 1 << 20).unwrap(),
            payload
        );
    }

    /// A zlib stream is not a gzip member and vice versa; each reader must
    /// refuse the other's framing rather than inflate it into noise.
    #[test]
    fn the_two_wrappers_are_not_interchangeable() {
        let payload = b"wrapped".to_vec();
        let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&payload, 6);
        assert_eq!(inflate_zlib(&zlib, 1 << 20).unwrap(), payload);
        assert!(inflate_gzip(&zlib, 1 << 20).is_err());
        assert!(inflate_zlib(&gzip(&payload, 0, &[]), 1 << 20).is_err());
    }

    /// A member whose trailer disagrees with what came out is truncated or
    /// corrupt, and saying so beats handing back a short chunk.
    #[test]
    fn refuses_a_member_whose_trailer_disagrees() {
        let payload = b"length matters".to_vec();
        let mut member = gzip(&payload, 0, &[]);
        let len = member.len();
        member[len - 4] ^= 0xFF;
        let err = inflate_gzip(&member, 1 << 20).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("trailer declares")),
            "got {err:?}"
        );
    }

    #[test]
    fn refuses_truncated_and_foreign_members() {
        assert!(inflate_gzip(&[0x1F, 0x8B, 8], 1 << 20).is_err());
        assert!(inflate_gzip(&[0u8; 32], 1 << 20).is_err());
        // Method 9 is not DEFLATE.
        let mut wrong = gzip(b"x", 0, &[]);
        wrong[2] = 9;
        assert!(inflate_gzip(&wrong, 1 << 20).is_err());
        // FNAME with no terminator inside the member.
        let mut unterminated = gzip(b"x", 0x08, &[]);
        unterminated[3] = 0x08;
        assert!(inflate_gzip(&unterminated, 1 << 20).is_err());
    }

    #[test]
    fn the_ceiling_is_enforced() {
        let payload = vec![0u8; 4096];
        assert!(inflate_gzip(&gzip(&payload, 0, &[]), 64).is_err());
        let zlib = miniz_oxide::deflate::compress_to_vec_zlib(&payload, 6);
        assert!(inflate_zlib(&zlib, 64).is_err());
    }
}
