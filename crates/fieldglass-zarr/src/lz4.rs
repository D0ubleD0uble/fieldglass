//! LZ4 *block* decompression — the form blosc stores, and the payload of the
//! numcodecs `lz4` codec.
//!
//! Not the LZ4 *frame* format (magic `04 22 4D 18`, block checksums, a content
//! size field). A blosc block holds the bare compressed block, and its
//! decompressed length is already known from the container, which is what makes
//! the bare form usable at all: the block format itself states no output size.
//! The numcodecs `lz4` codec wraps the same block in a four-byte
//! little-endian length header, which [`decompress_numcodecs`] strips.
//!
//! The block is a sequence of *sequences*, each a token byte followed by
//! literals and then a match:
//!
//! * the token's high nibble is the literal count, the low nibble the match
//!   length minus its four-byte minimum;
//! * a nibble of 15 means "and more", read as a run of bytes added until one is
//!   not 255;
//! * after the literals comes a two-byte little-endian *offset* back into the
//!   output written so far, and the match is copied from there — one byte at a
//!   time, because the ranges are allowed to overlap, which is how LZ4 encodes
//!   a run.
//!
//! The last sequence is literals only: it ends the block with no offset and no
//! match. Reference: the LZ4 block format specification.
//!
//! `lz4hc` produces this same format — it is a slower, denser *encoder*, not a
//! second grammar — so blosc's `lz4hc` blocks decode here too.

use fieldglass_core::FieldglassError;

/// The shortest match LZ4 encodes; the token's low nibble counts from it.
const MIN_MATCH: usize = 4;

/// Decompress one LZ4 block into a buffer of exactly `expected` bytes.
///
/// The output length is a parameter rather than something recovered from the
/// stream because the block format does not carry one. A stream that decodes to
/// a different length than the container promised is a corrupt chunk, and is
/// refused rather than returned short.
pub fn decompress(input: &[u8], expected: usize) -> Result<Vec<u8>, FieldglassError> {
    let mut out: Vec<u8> = Vec::with_capacity(expected);
    let mut pos = 0usize;

    while pos < input.len() {
        let token = input[pos];
        pos += 1;

        let mut literal_len = (token >> 4) as usize;
        if literal_len == 15 {
            literal_len += read_extended_length(input, &mut pos)?;
        }
        let end = pos
            .checked_add(literal_len)
            .filter(|&e| e <= input.len())
            .ok_or_else(|| truncated("literal run", pos, literal_len, input.len()))?;
        if out.len() + literal_len > expected {
            return Err(overflow(out.len() + literal_len, expected));
        }
        out.extend_from_slice(&input[pos..end]);
        pos = end;

        // The block ends after a literal run: the last sequence carries no
        // match, so there is no offset to read. Anything less than the two
        // offset bytes here is that ending, and `pos == input.len()` is the
        // only well-formed spelling of it.
        if pos == input.len() {
            break;
        }
        let offset = u16::from_le_bytes([
            *input.get(pos).ok_or_else(|| truncated_offset(pos))?,
            *input.get(pos + 1).ok_or_else(|| truncated_offset(pos))?,
        ]) as usize;
        pos += 2;
        if offset == 0 || offset > out.len() {
            return Err(FieldglassError::Parse(format!(
                "LZ4 match offset {offset} reaches outside the {} bytes decoded so far",
                out.len()
            )));
        }

        let mut match_len = (token & 0x0F) as usize;
        if match_len == 15 {
            match_len += read_extended_length(input, &mut pos)?;
        }
        match_len += MIN_MATCH;
        if out.len() + match_len > expected {
            return Err(overflow(out.len() + match_len, expected));
        }
        // Byte at a time, deliberately: LZ4 encodes a repeating run as a match
        // whose source overlaps its destination (offset 1 repeats one byte),
        // so a block copy would read bytes that have not been written yet.
        let start = out.len() - offset;
        for i in 0..match_len {
            let byte = out[start + i];
            out.push(byte);
        }
    }

    if out.len() != expected {
        return Err(FieldglassError::Parse(format!(
            "LZ4 block decoded to {} bytes, not the {expected} the container declares",
            out.len()
        )));
    }
    Ok(out)
}

/// Decompress the numcodecs `lz4` codec's payload: a four-byte little-endian
/// decompressed length, then the block.
///
/// The header is why this codec needs no external size, and why its output
/// length is checked against a caller's ceiling rather than against a promise.
pub fn decompress_numcodecs(input: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    let header: [u8; 4] = input
        .get(..4)
        .and_then(|h| h.try_into().ok())
        .ok_or_else(|| {
            FieldglassError::Parse(
                "numcodecs lz4 payload is shorter than its four-byte length header".to_string(),
            )
        })?;
    let expected = u32::from_le_bytes(header) as usize;
    if expected > limit {
        return Err(FieldglassError::Parse(format!(
            "numcodecs lz4 payload declares {expected} bytes, past the {limit}-byte ceiling"
        )));
    }
    decompress(&input[4..], expected)
}

/// Read the "and more" continuation of a 15-nibble length: bytes added until
/// one is not 255.
fn read_extended_length(input: &[u8], pos: &mut usize) -> Result<usize, FieldglassError> {
    let mut total = 0usize;
    loop {
        let byte = *input.get(*pos).ok_or_else(|| {
            FieldglassError::Parse(
                "LZ4 length continuation runs past the end of the block".to_string(),
            )
        })?;
        *pos += 1;
        // `checked_add` because the continuation is attacker-controlled and
        // unbounded in length: a run of 255s is how a crafted block asks for a
        // length that wraps on a 32-bit target.
        total = total.checked_add(byte as usize).ok_or_else(|| {
            FieldglassError::Parse("LZ4 length continuation overflows a usize".to_string())
        })?;
        if byte != 255 {
            return Ok(total);
        }
    }
}

fn truncated(what: &str, pos: usize, len: usize, total: usize) -> FieldglassError {
    FieldglassError::Parse(format!(
        "LZ4 {what} of {len} bytes at offset {pos} runs past the {total}-byte block"
    ))
}

fn truncated_offset(pos: usize) -> FieldglassError {
    FieldglassError::Parse(format!(
        "LZ4 block ends mid-sequence: no match offset at byte {pos}"
    ))
}

fn overflow(would_be: usize, expected: usize) -> FieldglassError {
    FieldglassError::Parse(format!(
        "LZ4 block decodes to at least {would_be} bytes, past the {expected} the container declares"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hand-built block, so the test states the format rather than replaying
    /// an encoder's output. Token `0x50` = five literals, no match; the block
    /// ends after them.
    #[test]
    fn decodes_a_literal_only_block() {
        let block = [0x50, b'h', b'e', b'l', b'l', b'o'];
        assert_eq!(decompress(&block, 5).unwrap(), b"hello");
    }

    /// An overlapping match is how LZ4 spells a run, and is the case a block
    /// copy gets wrong: offset 1, length 4+6, from one literal.
    #[test]
    fn an_overlapping_match_repeats_a_run() {
        // token 0x16: 1 literal, match length 6+4 = 10; offset 1.
        let block = [0x16, b'a', 0x01, 0x00];
        assert_eq!(decompress(&block, 11).unwrap(), b"aaaaaaaaaaa");
    }

    /// The 15-nibble continuation, in both fields at once: 15+4 = 19 literals
    /// and a match of 15+2+4 = 21.
    #[test]
    fn extended_lengths_accumulate_until_a_byte_is_not_255() {
        let mut block = vec![0xFF, 0x04];
        block.extend_from_slice(&(0..19u8).collect::<Vec<_>>());
        block.extend_from_slice(&[19, 0x00, 0x02]);
        let out = decompress(&block, 40).unwrap();
        assert_eq!(&out[..19], &(0..19u8).collect::<Vec<_>>()[..]);
        // The match starts 19 bytes back — the literals — and is two bytes
        // longer than them, so it wraps into what it has just written.
        let mut expected: Vec<u8> = (0..19u8).collect();
        expected.extend_from_slice(&[0, 1]);
        assert_eq!(&out[19..], &expected[..]);
    }

    /// A match reaching before the start of the output is a corrupt block, not
    /// a panic.
    #[test]
    fn refuses_a_match_offset_past_the_output() {
        let block = [0x10, b'a', 0x09, 0x00];
        let err = decompress(&block, 16).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("reaches outside")),
            "got {err:?}"
        );
        // Offset zero has no meaning and would otherwise index out of bounds.
        let zero = [0x10, b'a', 0x00, 0x00];
        assert!(decompress(&zero, 16).is_err());
    }

    /// A literal run longer than the block, and a block that decodes to more
    /// than the container promised, both have to be refused before the
    /// allocation they imply.
    #[test]
    fn refuses_a_block_that_overruns_in_either_direction() {
        let truncated = [0xF0, 0x00, b'a'];
        assert!(decompress(&truncated, 32).is_err());

        // 5 literals but the container says 2.
        let too_long = [0x50, b'h', b'e', b'l', b'l', b'o'];
        let err = decompress(&too_long, 2).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("past the 2")),
            "got {err:?}"
        );
    }

    /// Decoding short is as wrong as decoding long: the container's length is
    /// the claim being checked.
    #[test]
    fn refuses_a_block_that_decodes_short() {
        let block = [0x50, b'h', b'e', b'l', b'l', b'o'];
        let err = decompress(&block, 9).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("not the 9")),
            "got {err:?}"
        );
    }

    /// The numcodecs wrapper's length header is honoured, and a header
    /// promising more than the ceiling is refused before anything is
    /// allocated for it.
    #[test]
    fn the_numcodecs_wrapper_reads_its_length_header() {
        let mut framed = 5u32.to_le_bytes().to_vec();
        framed.extend_from_slice(&[0x50, b'h', b'e', b'l', b'l', b'o']);
        assert_eq!(decompress_numcodecs(&framed, 1024).unwrap(), b"hello");

        let err = decompress_numcodecs(&framed, 4).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("ceiling")),
            "got {err:?}"
        );
        assert!(decompress_numcodecs(&[0, 1], 1024).is_err());
    }

    /// A crafted run of 255s must not wrap the accumulator on a 32-bit target.
    #[test]
    fn a_length_continuation_cannot_overflow() {
        let mut block = vec![0xF0];
        block.extend(std::iter::repeat_n(0xFFu8, 64));
        // Ends without a terminating non-255 byte: truncation, not a wrap.
        assert!(decompress(&block, 1 << 20).is_err());
    }
}
