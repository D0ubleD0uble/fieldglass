//! HDF5 object-header walker — the lowest layer of the NetCDF-4 / HDF5 deep
//! parser (issue #37, under #33).
//!
//! Given the file offset of an object header, this decodes the chained header
//! *messages* into `(type_code, flags, raw_bytes)` tuples, following
//! continuation messages and verifying version-2 chunk checksums. It decodes
//! the message *envelope* only — interpreting each message body (dataspace,
//! datatype, attribute, link, …) is the job of the higher-layer sub-issues
//! (#38–#40). B-tree / heap / group traversal is likewise out of scope here.
//!
//! Two on-disk layouts are handled:
//!
//! * **Version 1** — a 12-byte prefix, 4 bytes of alignment padding, then
//!   8-byte message headers (`type:u16, size:u16, flags:u8, reserved:3`) whose
//!   data is padded to a multiple of 8. Continuation chunks are bare runs of
//!   messages with no signature or checksum.
//! * **Version 2** — an `OHDR` signature, flag-driven field widths, 4-byte
//!   message headers (plus an optional 2-byte creation-order field), and a
//!   trailing Jenkins lookup3 checksum over each chunk. Continuation chunks
//!   carry an `OCHK` signature and their own checksum.
//!
//! Reference: HDF5 file format specification version 3, "Disk Format: Level 2A
//! — Data Object Headers" <https://docs.hdfgroup.org/hdf5/develop/_f_m_t3.html>.

use fieldglass_core::FieldglassError;
use std::collections::VecDeque;

/// Object-header continuation message (points at the next chunk of messages).
pub const MSG_CONTINUATION: u16 = 0x0010;

/// Version-2 object-header chunk-0 signature.
const OHDR_SIGNATURE: &[u8; 4] = b"OHDR";
/// Version-2 object-header continuation-chunk signature.
const OCHK_SIGNATURE: &[u8; 4] = b"OCHK";

/// Guards against malformed/cyclic continuation chains. Real object headers
/// have a handful of chunks and a few dozen messages; these caps are far above
/// anything legitimate while keeping a hostile file from looping forever.
const MAX_CHUNKS: usize = 4096;
const MAX_MESSAGES: usize = 65_536;

/// A decoded object header: its on-disk version and the flat sequence of header
/// messages, with continuation chunks spliced in at the point they're followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectHeader {
    /// Object-header version (1 or 2).
    pub version: u8,
    /// Header messages in walk order.
    pub messages: Vec<HeaderMessage>,
}

/// A single header message, surfaced as a `(type, flags, body)` tuple. The body
/// is the raw message-data bytes; decoding them is the next layer's job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderMessage {
    /// Message type code (e.g. `0x0011` Symbol Table, `0x0010` Continuation).
    pub msg_type: u16,
    /// Message flags byte.
    pub flags: u8,
    /// Raw message-data bytes.
    pub body: Vec<u8>,
}

/// Walk the object header at `offset`, returning its messages.
///
/// `offset_size` / `length_size` are the superblock's "size of offsets" and
/// "size of lengths" — needed to read continuation-message addresses/lengths.
pub fn walk(
    bytes: &[u8],
    offset: u64,
    offset_size: u8,
    length_size: u8,
) -> Result<ObjectHeader, FieldglassError> {
    let start = usize_at(offset)?;
    if matches!(bytes.get(start..), Some(rest) if rest.starts_with(OHDR_SIGNATURE)) {
        parse_v2(bytes, start, offset_size, length_size)
    } else if bytes.get(start) == Some(&1) {
        parse_v1(bytes, start, offset_size, length_size)
    } else {
        Err(FieldglassError::Parse(format!(
            "no recognizable object header at offset {offset} (expected v1 prefix or OHDR)"
        )))
    }
}

/// Version-1 object header: 12-byte prefix, 4 bytes of padding, then the
/// chunk-0 messages, with continuation messages pointing at bare message runs.
fn parse_v1(
    bytes: &[u8],
    start: usize,
    offset_size: u8,
    length_size: u8,
) -> Result<ObjectHeader, FieldglassError> {
    // Prefix: version(1) reserved(1) num_messages(2) ref_count(4) size(4) = 12,
    // then 4 bytes of padding so messages begin on an 8-byte boundary.
    let header_size = read_uint_le(bytes, start + 8, 4)? as usize;
    let messages_start = start
        .checked_add(16)
        .ok_or_else(|| FieldglassError::Parse("v1 object header offset overflow".into()))?;

    let mut messages = Vec::new();
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();
    queue.push_back((messages_start, header_size));

    let mut chunks = 0usize;
    while let Some((run_start, run_len)) = queue.pop_front() {
        chunks += 1;
        if chunks > MAX_CHUNKS {
            return Err(FieldglassError::Parse(
                "too many object-header continuation chunks".into(),
            ));
        }
        let run = slice(bytes, run_start, run_len)?;
        let mut pos = 0usize;
        // Each message header is 8 bytes; stop when a header no longer fits.
        while pos + 8 <= run_len {
            let msg_type = read_uint_le(run, pos, 2)? as u16;
            let data_size = read_uint_le(run, pos + 2, 2)? as usize;
            let flags = run[pos + 4];
            let body_start = pos + 8;
            let body_end = body_start
                .checked_add(data_size)
                .filter(|&e| e <= run_len)
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "v1 header message data ({data_size} bytes) overruns its chunk"
                    ))
                })?;
            if msg_type == MSG_CONTINUATION {
                enqueue_continuation(
                    &run[body_start..body_end],
                    offset_size,
                    length_size,
                    &mut queue,
                )?;
            }
            push_message(&mut messages, msg_type, flags, &run[body_start..body_end])?;
            pos = body_end;
        }
    }

    Ok(ObjectHeader {
        version: 1,
        messages,
    })
}

/// Version-2 object header: `OHDR` signature, flag-driven field widths, then
/// chunk-0 messages followed by a lookup3 checksum. Continuation messages point
/// at `OCHK` chunks that carry their own signature and checksum.
fn parse_v2(
    bytes: &[u8],
    start: usize,
    offset_size: u8,
    length_size: u8,
) -> Result<ObjectHeader, FieldglassError> {
    // OHDR(4) version(1) flags(1), then optional time/phase-change fields.
    // The signature was matched by the caller; only version 2 is defined.
    let version = *bytes
        .get(start + 4)
        .ok_or_else(|| FieldglassError::Parse("truncated v2 object header".into()))?;
    if version != 2 {
        return Err(FieldglassError::Parse(format!(
            "unsupported v2 object-header version {version}"
        )));
    }
    let flags = *bytes
        .get(start + 5)
        .ok_or_else(|| FieldglassError::Parse("truncated v2 object header".into()))?;
    let mut pos = start + 6;
    if flags & 0x20 != 0 {
        pos += 16; // access/modification/change/birth times
    }
    if flags & 0x10 != 0 {
        pos += 4; // max-compact / min-dense attribute phase-change values
    }
    // Bits 0-1 select the width of the "size of chunk 0" field: 1, 2, 4, or 8.
    let size_width = 1usize << (flags & 0x03);
    let chunk0_size = read_uint_le(bytes, pos, size_width)? as usize;
    pos += size_width;

    let track_creation_order = flags & 0x04 != 0;

    let mut messages = Vec::new();
    let mut queue: VecDeque<(usize, usize)> = VecDeque::new();

    // Chunk 0 ends with a 4-byte checksum over [start .. checksum_pos).
    let checksum_pos = pos
        .checked_add(chunk0_size)
        .ok_or_else(|| FieldglassError::Parse("v2 chunk-0 size overflow".into()))?;
    verify_checksum(bytes, start, checksum_pos)?;
    parse_v2_run(
        bytes,
        pos,
        chunk0_size,
        track_creation_order,
        offset_size,
        length_size,
        &mut messages,
        &mut queue,
    )?;

    let mut chunks = 0usize;
    while let Some((chunk_start, chunk_len)) = queue.pop_front() {
        chunks += 1;
        if chunks > MAX_CHUNKS {
            return Err(FieldglassError::Parse(
                "too many object-header continuation chunks".into(),
            ));
        }
        // OCHK(4) + messages + checksum(4): at least 8 bytes of framing.
        if chunk_len < 8 {
            return Err(FieldglassError::Parse(
                "v2 continuation chunk too small for OCHK framing".into(),
            ));
        }
        // Bounds-check the whole chunk up front so the address arithmetic below
        // can't overflow on a malformed continuation pointer.
        let chunk = slice(bytes, chunk_start, chunk_len)?;
        if !chunk.starts_with(OCHK_SIGNATURE) {
            return Err(FieldglassError::Parse(
                "v2 continuation chunk missing OCHK signature".into(),
            ));
        }
        let checksum_pos = chunk_start + chunk_len - 4;
        verify_checksum(bytes, chunk_start, checksum_pos)?;
        parse_v2_run(
            bytes,
            chunk_start + 4,
            chunk_len - 8,
            track_creation_order,
            offset_size,
            length_size,
            &mut messages,
            &mut queue,
        )?;
    }

    Ok(ObjectHeader {
        version: 2,
        messages,
    })
}

/// Parse a run of version-2 messages from `[run_start, run_start + run_len)`.
#[allow(clippy::too_many_arguments)]
fn parse_v2_run(
    bytes: &[u8],
    run_start: usize,
    run_len: usize,
    track_creation_order: bool,
    offset_size: u8,
    length_size: u8,
    messages: &mut Vec<HeaderMessage>,
    queue: &mut VecDeque<(usize, usize)>,
) -> Result<(), FieldglassError> {
    let run = slice(bytes, run_start, run_len)?;
    // Header is type(1) size(2) flags(1), plus a 2-byte creation order when the
    // header tracks it.
    let header_len = if track_creation_order { 6 } else { 4 };
    let mut pos = 0usize;
    // Trailing bytes too small to hold a message header are a permitted gap.
    while pos + header_len <= run_len {
        let msg_type = run[pos] as u16;
        let data_size = read_uint_le(run, pos + 1, 2)? as usize;
        let flags = run[pos + 3];
        let body_start = pos + header_len;
        let body_end = body_start
            .checked_add(data_size)
            .filter(|&e| e <= run_len)
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "v2 header message data ({data_size} bytes) overruns its chunk"
                ))
            })?;
        if msg_type == MSG_CONTINUATION {
            enqueue_continuation(&run[body_start..body_end], offset_size, length_size, queue)?;
        }
        push_message(messages, msg_type, flags, &run[body_start..body_end])?;
        pos = body_end;
    }
    Ok(())
}

/// Read a continuation message body — `address` then `length`, sized by the
/// superblock — and enqueue the chunk it points at.
fn enqueue_continuation(
    body: &[u8],
    offset_size: u8,
    length_size: u8,
    queue: &mut VecDeque<(usize, usize)>,
) -> Result<(), FieldglassError> {
    let osize = offset_size as usize;
    let lsize = length_size as usize;
    if body.len() < osize + lsize {
        return Err(FieldglassError::Parse(
            "continuation message too small for address + length".into(),
        ));
    }
    let address = read_uint_le(body, 0, osize)?;
    let length = read_uint_le(body, osize, lsize)?;
    queue.push_back((usize_at(address)?, usize_at(length)?));
    Ok(())
}

/// Append a message, enforcing the total-message cap.
fn push_message(
    messages: &mut Vec<HeaderMessage>,
    msg_type: u16,
    flags: u8,
    body: &[u8],
) -> Result<(), FieldglassError> {
    if messages.len() >= MAX_MESSAGES {
        return Err(FieldglassError::Parse(
            "object header exceeds message limit".into(),
        ));
    }
    messages.push(HeaderMessage {
        msg_type,
        flags,
        body: body.to_vec(),
    });
    Ok(())
}

/// Verify the 4-byte little-endian lookup3 checksum stored at `checksum_pos`
/// against the bytes `[start, checksum_pos)`.
fn verify_checksum(bytes: &[u8], start: usize, checksum_pos: usize) -> Result<(), FieldglassError> {
    if checksum_pos < start {
        return Err(FieldglassError::Parse(
            "checksum precedes chunk start".into(),
        ));
    }
    let region = slice(bytes, start, checksum_pos - start)?;
    let stored = read_uint_le(bytes, checksum_pos, 4)? as u32;
    let computed = checksum_lookup3(region);
    if stored != computed {
        return Err(FieldglassError::Parse(format!(
            "object-header chunk checksum mismatch (stored {stored:#010x}, computed {computed:#010x})"
        )));
    }
    Ok(())
}

/// Bob Jenkins' `lookup3` `hashlittle` with `initval = 0` — the function HDF5
/// uses for metadata checksums (`H5_checksum_lookup3`).
fn checksum_lookup3(data: &[u8]) -> u32 {
    fn rot(x: u32, k: u32) -> u32 {
        x.rotate_left(k)
    }
    fn mix(a: &mut u32, b: &mut u32, c: &mut u32) {
        *a = a.wrapping_sub(*c);
        *a ^= rot(*c, 4);
        *c = c.wrapping_add(*b);
        *b = b.wrapping_sub(*a);
        *b ^= rot(*a, 6);
        *a = a.wrapping_add(*c);
        *c = c.wrapping_sub(*b);
        *c ^= rot(*b, 8);
        *b = b.wrapping_add(*a);
        *a = a.wrapping_sub(*c);
        *a ^= rot(*c, 16);
        *c = c.wrapping_add(*b);
        *b = b.wrapping_sub(*a);
        *b ^= rot(*a, 19);
        *a = a.wrapping_add(*c);
        *c = c.wrapping_sub(*b);
        *c ^= rot(*b, 4);
        *b = b.wrapping_add(*a);
    }
    fn final_mix(a: &mut u32, b: &mut u32, c: &mut u32) {
        *c ^= *b;
        *c = c.wrapping_sub(rot(*b, 14));
        *a ^= *c;
        *a = a.wrapping_sub(rot(*c, 11));
        *b ^= *a;
        *b = b.wrapping_sub(rot(*a, 25));
        *c ^= *b;
        *c = c.wrapping_sub(rot(*b, 16));
        *a ^= *c;
        *a = a.wrapping_sub(rot(*c, 4));
        *b ^= *a;
        *b = b.wrapping_sub(rot(*a, 14));
        *c ^= *b;
        *c = c.wrapping_sub(rot(*b, 24));
    }

    let mut a = 0xdead_beefu32.wrapping_add(data.len() as u32);
    let (mut b, mut c) = (a, a);

    // Mirror the reference `while (length > 12)`: every full block *except* the
    // final 1..=12 bytes is mixed here; the rest go through the tail. This is
    // why an exact multiple of 12 still routes its last block to the tail.
    let mut i = 0;
    while data.len() - i > 12 {
        let blk = &data[i..i + 12];
        a = a.wrapping_add(u32::from_le_bytes([blk[0], blk[1], blk[2], blk[3]]));
        b = b.wrapping_add(u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]));
        c = c.wrapping_add(u32::from_le_bytes([blk[8], blk[9], blk[10], blk[11]]));
        mix(&mut a, &mut b, &mut c);
        i += 12;
    }

    let tail = &data[i..];
    if tail.is_empty() {
        return c;
    }
    let g = |i: usize| tail[i] as u32;
    if tail.len() >= 12 {
        c = c.wrapping_add(g(11) << 24);
    }
    if tail.len() >= 11 {
        c = c.wrapping_add(g(10) << 16);
    }
    if tail.len() >= 10 {
        c = c.wrapping_add(g(9) << 8);
    }
    if tail.len() >= 9 {
        c = c.wrapping_add(g(8));
    }
    if tail.len() >= 8 {
        b = b.wrapping_add(g(7) << 24);
    }
    if tail.len() >= 7 {
        b = b.wrapping_add(g(6) << 16);
    }
    if tail.len() >= 6 {
        b = b.wrapping_add(g(5) << 8);
    }
    if tail.len() >= 5 {
        b = b.wrapping_add(g(4));
    }
    if tail.len() >= 4 {
        a = a.wrapping_add(g(3) << 24);
    }
    if tail.len() >= 3 {
        a = a.wrapping_add(g(2) << 16);
    }
    if tail.len() >= 2 {
        a = a.wrapping_add(g(1) << 8);
    }
    a = a.wrapping_add(g(0));
    final_mix(&mut a, &mut b, &mut c);
    c
}

/// Read a little-endian unsigned integer of `width` bytes (1..=8) at `at`.
pub(crate) fn read_uint_le(bytes: &[u8], at: usize, width: usize) -> Result<u64, FieldglassError> {
    if width == 0 || width > 8 {
        return Err(FieldglassError::Parse(format!(
            "unsupported integer width {width}"
        )));
    }
    let region = slice(bytes, at, width)?;
    let mut value = 0u64;
    for (i, &byte) in region.iter().enumerate() {
        value |= (byte as u64) << (8 * i);
    }
    Ok(value)
}

/// Bounds-checked slice of `len` bytes at `at`.
fn slice(bytes: &[u8], at: usize, len: usize) -> Result<&[u8], FieldglassError> {
    let end = at
        .checked_add(len)
        .ok_or_else(|| FieldglassError::Parse("offset + length overflow".into()))?;
    bytes.get(at..end).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "read of {len} bytes at {at} runs past end of file ({} bytes)",
            bytes.len()
        ))
    })
}

/// Convert a file address to a `usize` index, rejecting the HDF5 "undefined
/// address" sentinel (all `0xFF`) and anything too large for the platform.
fn usize_at(address: u64) -> Result<usize, FieldglassError> {
    if address == u64::MAX {
        return Err(FieldglassError::Parse("undefined HDF5 address".into()));
    }
    usize::try_from(address).map_err(|_| {
        FieldglassError::Parse(format!("address {address} too large for this platform"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append a v1 message (8-byte header + padded data) to `buf`.
    fn push_v1(buf: &mut Vec<u8>, msg_type: u16, data: &[u8]) {
        buf.extend_from_slice(&msg_type.to_le_bytes());
        buf.extend_from_slice(&(data.len() as u16).to_le_bytes());
        buf.push(0); // flags
        buf.extend_from_slice(&[0, 0, 0]); // reserved
        buf.extend_from_slice(data);
    }

    /// Build a v1 object header whose chunk-0 holds `messages`.
    fn v1_header(messages: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (t, d) in messages {
            push_v1(&mut body, *t, d);
        }
        let mut buf = Vec::new();
        buf.push(1); // version
        buf.push(0); // reserved
        buf.extend_from_slice(&(messages.len() as u16).to_le_bytes());
        buf.extend_from_slice(&1u32.to_le_bytes()); // ref count
        buf.extend_from_slice(&(body.len() as u32).to_le_bytes()); // header size
        buf.extend_from_slice(&[0, 0, 0, 0]); // alignment padding
        buf.extend_from_slice(&body);
        buf
    }

    /// Build a v2 object header (flags 0) whose chunk-0 holds `messages`, with a
    /// valid trailing checksum.
    fn v2_header(messages: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut body = Vec::new();
        for (t, d) in messages {
            body.push(*t as u8);
            body.extend_from_slice(&(d.len() as u16).to_le_bytes());
            body.push(0); // flags
            body.extend_from_slice(d);
        }
        let mut buf = Vec::new();
        buf.extend_from_slice(OHDR_SIGNATURE);
        buf.push(2); // version
        buf.push(0); // flags: 1-byte chunk size, no times/creation-order
        buf.push(body.len() as u8); // size of chunk 0
        buf.extend_from_slice(&body);
        let checksum = checksum_lookup3(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());
        buf
    }

    /// Build a v2 `OCHK` continuation chunk: signature + messages + checksum.
    fn ochk_chunk(messages: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(OCHK_SIGNATURE);
        for (t, d) in messages {
            buf.push(*t as u8);
            buf.extend_from_slice(&(d.len() as u16).to_le_bytes());
            buf.push(0); // flags
            buf.extend_from_slice(d);
        }
        let checksum = checksum_lookup3(&buf);
        buf.extend_from_slice(&checksum.to_le_bytes());
        buf
    }

    #[test]
    fn walks_v1_chunk0_messages() {
        let bytes = v1_header(&[(0x0011, vec![1, 2, 3, 4, 5, 6, 7, 8]), (0x000c, vec![9; 8])]);
        let oh = walk(&bytes, 0, 8, 8).unwrap();
        assert_eq!(oh.version, 1);
        let types: Vec<u16> = oh.messages.iter().map(|m| m.msg_type).collect();
        assert_eq!(types, vec![0x0011, 0x000c]);
        assert_eq!(oh.messages[0].body, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn follows_v1_continuation() {
        // Chunk 0 is a single continuation pointing past the header bytes.
        let tail = v1_header(&[(0x0011, vec![0xAA; 8])]);
        // Continuation body: address (8) + length (8) of the message run.
        let cont_addr = 64u64; // where we'll place the run below
        let cont_len = 16u64; // one 8-byte header + 8-byte body
        let mut cont_body = Vec::new();
        cont_body.extend_from_slice(&cont_addr.to_le_bytes());
        cont_body.extend_from_slice(&cont_len.to_le_bytes());
        let header = v1_header(&[(MSG_CONTINUATION, cont_body)]);

        let mut bytes = header.clone();
        bytes.resize(cont_addr as usize, 0);
        // The run: one Symbol Table message with an 8-byte body.
        push_v1(&mut bytes, 0x0011, &[0xAA; 8]);
        let _ = tail;

        let oh = walk(&bytes, 0, 8, 8).unwrap();
        let types: Vec<u16> = oh.messages.iter().map(|m| m.msg_type).collect();
        assert_eq!(types, vec![MSG_CONTINUATION, 0x0011]);
    }

    #[test]
    fn walks_v2_with_valid_checksum() {
        let bytes = v2_header(&[(0x0001, vec![1, 2, 3, 4]), (0x0003, vec![5, 6])]);
        let oh = walk(&bytes, 0, 8, 8).unwrap();
        assert_eq!(oh.version, 2);
        let types: Vec<u16> = oh.messages.iter().map(|m| m.msg_type).collect();
        assert_eq!(types, vec![0x0001, 0x0003]);
    }

    #[test]
    fn follows_v2_ochk_continuation() {
        // The continuation body is a fixed 16 bytes (address + length), so a
        // dummy build reveals the header length without hardcoding it.
        let header_len = v2_header(&[(MSG_CONTINUATION, vec![0u8; 16])]).len();
        let ochk = ochk_chunk(&[(0x000c, vec![1, 2, 3, 4])]);

        let mut cont_body = Vec::new();
        cont_body.extend_from_slice(&(header_len as u64).to_le_bytes());
        cont_body.extend_from_slice(&(ochk.len() as u64).to_le_bytes());
        let mut bytes = v2_header(&[(MSG_CONTINUATION, cont_body)]);
        assert_eq!(bytes.len(), header_len);
        bytes.extend_from_slice(&ochk);

        let oh = walk(&bytes, 0, 8, 8).unwrap();
        let types: Vec<u16> = oh.messages.iter().map(|m| m.msg_type).collect();
        assert_eq!(types, vec![MSG_CONTINUATION, 0x000c]);
        assert_eq!(oh.messages[1].body, vec![1, 2, 3, 4]);
    }

    #[test]
    fn rejects_v2_corrupt_checksum() {
        let mut bytes = v2_header(&[(0x0001, vec![1, 2, 3, 4])]);
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF; // corrupt the checksum
        let err = walk(&bytes, 0, 8, 8).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn continuation_past_eof_errors_without_panic() {
        let mut cont_body = Vec::new();
        cont_body.extend_from_slice(&4096u64.to_le_bytes()); // address past EOF
        cont_body.extend_from_slice(&16u64.to_le_bytes());
        let bytes = v1_header(&[(MSG_CONTINUATION, cont_body)]);
        let err = walk(&bytes, 0, 8, 8).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn unrecognized_header_errors() {
        let err = walk(&[0xFFu8; 32], 0, 8, 8).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn lookup3_matches_known_vectors() {
        // Reference values from Bob Jenkins' lookup3.c `hashlittle(key, len, 0)`.
        assert_eq!(checksum_lookup3(b""), 0xdead_beef);
        assert_eq!(
            checksum_lookup3(b"Four score and seven years ago"),
            0x17770551
        );
        // Exact multiples of 12 must route their final block through the tail,
        // not the block loop. Bytes are 0, 1, 2, … .
        let seq: Vec<u8> = (0..24).collect();
        assert_eq!(checksum_lookup3(&seq[..12]), 0x5e4a_a593);
        assert_eq!(checksum_lookup3(&seq[..24]), 0x9c0a_dd53);
    }
}
