//! BloscLZ decompression — blosc's own codec, and the one it uses when no other
//! is named.
//!
//! A FastLZ derivative. The stream is a sequence of tokens, each either a run of
//! literals or a back-reference:
//!
//! * **literal token** (`ctrl < 32`) — copy `ctrl + 1` bytes straight through;
//! * **match token** (`ctrl >= 32`) — the top three bits are a length and the
//!   low five the high bits of a distance, with a following byte carrying the
//!   distance's low eight. Both fields are *biased*: token length 1 means a
//!   three-byte match, and the distance is stored one less than it is.
//!
//! Two escapes extend that. A length field of 7 means "and more", read as a run
//! of bytes added until one is not 255; and a distance whose high bits are all
//! ones, with a low byte of 255, means two further bytes carry a *far* distance
//! measured from 8192.
//!
//! The very first byte is masked to five bits before it is read as a token. The
//! encoder ORs bit 5 into it as a marker, and can do so losslessly because the
//! stream always opens with a literal run — whose token never has that bit set.
//!
//! Reference: `blosclz_decompress` in c-blosc's `blosc/blosclz.c`. The wire
//! format is unchanged across c-blosc 1.x; `versionlz` is 1 throughout.

use fieldglass_core::FieldglassError;

/// Distances up to this are spelled in the token plus one byte; past it the
/// far-distance escape carries two more (`MAX_DISTANCE` in `blosclz.c`).
const MAX_DISTANCE: usize = 8191;

/// The shortest match the format encodes. The token's length field counts from
/// one meaning three, so three is added back.
const MIN_MATCH: usize = 3;

/// Decompress one BloscLZ stream into a buffer of exactly `expected` bytes.
///
/// As with LZ4, the length is a parameter: the stream states no output size,
/// and the blosc container's `neblock` is the only statement of it. A stream
/// that decodes to a different length is a corrupt block.
pub fn decompress(input: &[u8], expected: usize) -> Result<Vec<u8>, FieldglassError> {
    if input.is_empty() {
        return if expected == 0 {
            Ok(Vec::new())
        } else {
            Err(FieldglassError::Parse(format!(
                "empty blosclz stream cannot decode to {expected} bytes"
            )))
        };
    }

    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut pos = 0usize;
    // Only the first token is masked: the encoder sets bit 5 of the opening
    // byte as a marker, and that byte is always a literal token, whose own
    // value never reaches 32.
    let mut ctrl = (input[0] & 0x1F) as usize;
    pos += 1;

    loop {
        if ctrl < 32 {
            let len = ctrl + 1;
            let end = pos
                .checked_add(len)
                .filter(|&e| e <= input.len())
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "blosclz literal run of {len} bytes at {pos} runs past the \
                         {}-byte stream",
                        input.len()
                    ))
                })?;
            if out.len() + len > expected {
                return Err(too_long(out.len() + len, expected));
            }
            out.extend_from_slice(&input[pos..end]);
            pos = end;
        } else {
            let mut length = ctrl >> 5;
            let distance_high = ctrl & 0x1F;
            if length == 7 {
                length += read_extended_length(input, &mut pos)?;
            }
            length += MIN_MATCH - 1;

            let low = read_byte(input, &mut pos, "match distance")? as usize;
            let distance = if low == 0xFF && distance_high == 0x1F {
                // Far distance: two more bytes, measured from just past the
                // near-distance ceiling.
                let hi = read_byte(input, &mut pos, "far match distance")? as usize;
                let lo = read_byte(input, &mut pos, "far match distance")? as usize;
                ((hi << 8) | lo) + MAX_DISTANCE + 1
            } else {
                ((distance_high << 8) | low) + 1
            };

            if distance > out.len() {
                return Err(FieldglassError::Parse(format!(
                    "blosclz match distance {distance} reaches before the start of the \
                     {} bytes decoded so far",
                    out.len()
                )));
            }
            if out.len() + length > expected {
                return Err(too_long(out.len() + length, expected));
            }
            // Byte at a time: a distance smaller than the length is how the
            // format spells a repeating run, so the source overlaps the
            // destination and a block copy would read unwritten bytes.
            let start = out.len() - distance;
            for i in 0..length {
                let byte = out[start + i];
                out.push(byte);
            }
        }

        if pos >= input.len() {
            break;
        }
        ctrl = input[pos] as usize;
        pos += 1;
    }

    if out.len() != expected {
        return Err(FieldglassError::Parse(format!(
            "blosclz stream decoded to {} bytes, not the {expected} the container declares",
            out.len()
        )));
    }
    Ok(out)
}

/// The "and more" continuation of a length field of 7: bytes added until one is
/// not 255.
fn read_extended_length(input: &[u8], pos: &mut usize) -> Result<usize, FieldglassError> {
    let mut total = 0usize;
    loop {
        let byte = read_byte(input, pos, "length continuation")?;
        // Checked because the run is attacker-controlled and unbounded: a long
        // enough run of 255s is how a crafted stream asks for a length that
        // wraps on a 32-bit target.
        total = total.checked_add(byte as usize).ok_or_else(|| {
            FieldglassError::Parse("blosclz length continuation overflows a usize".to_string())
        })?;
        if byte != 255 {
            return Ok(total);
        }
    }
}

fn read_byte(input: &[u8], pos: &mut usize, what: &str) -> Result<u8, FieldglassError> {
    let byte = *input.get(*pos).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "blosclz stream ends where a {what} byte was expected"
        ))
    })?;
    *pos += 1;
    Ok(byte)
}

fn too_long(would_be: usize, expected: usize) -> FieldglassError {
    FieldglassError::Parse(format!(
        "blosclz stream decodes to at least {would_be} bytes, past the {expected} the \
         container declares"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The opening token is masked to five bits, so the marker bit the encoder
    /// sets does not turn a four-literal run into a match.
    #[test]
    fn the_marker_bit_on_the_first_token_is_masked_off() {
        // 0x23 = literal run of 4 with bit 5 set, exactly as blosclz opens.
        let stream = [0x23, b'a', b'b', b'c', b'd'];
        assert_eq!(decompress(&stream, 4).unwrap(), b"abcd");
        // Without the mask this token would read as a match and index out of
        // an empty output.
        assert!(decompress(&[0x23, b'a'], 1).is_err());
    }

    /// A short match: token length 1 means three bytes, and the distance is
    /// stored one less than it is.
    #[test]
    fn a_short_match_unbiases_both_fields() {
        // Literal "abcd" (token 0x03), then token 0x20 = length field 1
        // (3 bytes), distance high bits 0, low byte 3 => distance 4.
        let stream = [0x03, b'a', b'b', b'c', b'd', 0x20, 0x03];
        assert_eq!(decompress(&stream, 7).unwrap(), b"abcdabc");
    }

    /// A distance shorter than the match is how a run is spelled, and is the
    /// case a block copy gets wrong.
    #[test]
    fn an_overlapping_match_repeats_a_run() {
        // One literal 'x', then a match of 3 at distance 1.
        let stream = [0x00, b'x', 0x20, 0x00];
        assert_eq!(decompress(&stream, 4).unwrap(), b"xxxx");
    }

    /// The length escape accumulates until a byte is not 255, and adds to the
    /// nine a full field already means.
    #[test]
    fn the_length_escape_accumulates() {
        let mut stream = vec![0x00, b'y'];
        // Token 0xE0: length field 7 (escape), distance high bits 0.
        // Continuation byte 2 => length 7 + 2 + 2 = 11. Distance low byte 0
        // => distance 1.
        stream.extend_from_slice(&[0xE0, 0x02, 0x00]);
        assert_eq!(decompress(&stream, 12).unwrap(), b"yyyyyyyyyyyy");
    }

    /// The far-distance escape is only taken when the low byte is 255 *and*
    /// the token's distance bits are all ones; either alone is an ordinary
    /// near match.
    #[test]
    fn the_far_distance_escape_needs_both_halves() {
        let literals: Vec<u8> = (0..=255u8).collect();
        // Opening token 0xFF masks to 0x1F: a run of 32 literals. Then a match
        // whose low distance byte happens to be 255 but whose token distance
        // bits are zero, so this must *not* be read as an escape.
        let mut near = vec![0xFFu8];
        near.extend_from_slice(&literals[..32]);
        near.extend_from_slice(&[0x20, 0xFF]);
        // Read as a near match that is distance 256 into a 32-byte output, so
        // it is refused — which is itself the proof the escape was not taken:
        // an escape would have consumed two more bytes and run out of stream.
        let err = decompress(&near, 64).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("reaches before the start")),
            "got {err:?}"
        );
    }

    /// A far match really does reach past the near ceiling.
    #[test]
    fn a_far_match_is_measured_from_past_the_near_ceiling() {
        // 8200 literals, then a far match of 3 at distance 8192.
        let mut stream = Vec::new();
        let payload: Vec<u8> = (0..8200u32).map(|i| (i % 251) as u8).collect();
        let mut written = 0usize;
        while written < payload.len() {
            let run = (payload.len() - written).min(32);
            stream.push((run - 1) as u8);
            stream.extend_from_slice(&payload[written..written + run]);
            written += run;
        }
        // Token: length field 1 (3 bytes), distance bits all ones; then 0xFF,
        // then the far distance 0 => 8192.
        stream.extend_from_slice(&[0x20 | 0x1F, 0xFF, 0x00, 0x00]);
        let out = decompress(&stream, 8203).unwrap();
        assert_eq!(&out[..8200], &payload[..]);
        assert_eq!(&out[8200..], &payload[8200 - 8192..8200 - 8192 + 3]);
    }

    /// Every truncation and every over-long decode is an error rather than a
    /// panic or a short buffer.
    #[test]
    fn refuses_truncated_and_overlong_streams() {
        // Literal run promising more than the stream holds.
        assert!(decompress(&[0x1F, b'a'], 32).is_err());
        // Match token with no distance byte.
        assert!(decompress(&[0x00, b'a', 0x20], 8).is_err());
        // Decodes longer than declared.
        assert!(decompress(&[0x03, b'a', b'b', b'c', b'd'], 2).is_err());
        // Decodes shorter than declared.
        assert!(decompress(&[0x03, b'a', b'b', b'c', b'd'], 9).is_err());
        // Empty stream, non-zero output.
        assert!(decompress(&[], 4).is_err());
        assert_eq!(decompress(&[], 0).unwrap(), Vec::<u8>::new());
    }

    /// A run of 255s in a length escape must not wrap the accumulator.
    #[test]
    fn a_length_continuation_cannot_overflow() {
        let mut stream = vec![0x00, b'a', 0xE0];
        stream.extend(std::iter::repeat_n(0xFFu8, 128));
        assert!(decompress(&stream, 1 << 20).is_err());
    }
}
