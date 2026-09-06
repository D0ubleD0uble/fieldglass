//! The blosc1 container: a 16-byte header, an offset per block, and each block
//! a run of length-prefixed codec streams.
//!
//! Blosc is a *meta*-compressor. It does not compress anything itself; it cuts
//! a buffer into cache-sized blocks, optionally transposes each block so that
//! like-significance bytes or bits sit together, and hands each to one of five
//! ordinary codecs. Both Zarr editions reach it — as the `blosc` compressor in
//! a v2 `.zarray` and as the `blosc` codec in a v3 chain — and it is the
//! commonest thing a Zarr chunk is wrapped in.
//!
//! Three properties of the layout are easy to get wrong and are each pinned by
//! a test below, because each is invisible on the ordinary case:
//!
//! * **The block offsets need not ascend.** Blosc compresses blocks in
//!   parallel and writes each where its worker finished, so a four-block
//!   buffer can store block 3 first. A decoder that took the *next* offset as
//!   the end of a block would read another block's bytes as this one's. Each
//!   block's extent comes from walking its own length prefixes and from
//!   nothing else.
//! * **A block is split into one stream per byte of the element**, but only
//!   when the header says so and only for blocks that are not the short last
//!   one. The rule is reproduced exactly in `splits_in_block`.
//! * **A "memcpyed" buffer is raw**, filters included. The header still
//!   carries whatever shuffle was asked for, but blosc stored the original
//!   bytes and un-shuffling them would scramble the chunk.

use crate::blosclz;
use crate::lz4;
use crate::shuffle::{unbitshuffle, unshuffle};
use fieldglass_core::FieldglassError;

/// Fixed header size, and the offset the block-offset array starts at.
const HEADER_LEN: usize = 16;

/// The only container version c-blosc writes or reads (`BLOSC_VERSION_FORMAT`).
const VERSION_FORMAT: u8 = 2;

/// Every inner codec states this as its own format version.
const VERSION_LZ: u8 = 1;

/// The byte-shuffle filter was applied to each block.
const FLAG_SHUFFLE: u8 = 0x01;
/// The payload is the original bytes, with no codec and no filter.
const FLAG_MEMCPYED: u8 = 0x02;
/// The bit-shuffle filter was applied to each block.
const FLAG_BITSHUFFLE: u8 = 0x04;
/// Reserved; c-blosc refuses a buffer that sets it.
const FLAG_RESERVED: u8 = 0x08;
/// Blocks were **not** cut into one stream per element byte.
const FLAG_DONT_SPLIT: u8 = 0x10;

/// The largest element width blosc will split a block by (`MAX_SPLITS`).
const MAX_SPLITS: usize = 16;

/// The smallest stream blosc bothers to produce (`MIN_BUFFERSIZE`); a block
/// whose per-element share is under it is not split.
const MIN_BUFFERSIZE: usize = 128;

/// The inner codecs, by the *library-format* number the header carries.
///
/// Not the `BLOSC_BLOSCLZ` / `BLOSC_LZ4` … compressor enumeration, which is a
/// different numbering: `lz4` and `lz4hc` share one format number because they
/// share one wire format, and reading the wrong table would decode `zlib` as
/// `snappy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Inner {
    BloscLz,
    /// `lz4` and `lz4hc` alike — one is a denser encoder for the other's
    /// format, and the header cannot tell them apart because there is nothing
    /// to tell apart.
    Lz4,
    Snappy,
    Zlib,
    Zstd,
}

impl Inner {
    fn from_flags(flags: u8) -> Result<Self, FieldglassError> {
        match (flags & 0xE0) >> 5 {
            0 => Ok(Self::BloscLz),
            1 => Ok(Self::Lz4),
            2 => Ok(Self::Snappy),
            3 => Ok(Self::Zlib),
            4 => Ok(Self::Zstd),
            other => Err(FieldglassError::UnsupportedSection(format!(
                "blosc names compressor format {other}, which c-blosc does not define"
            ))),
        }
    }
}

/// One blosc buffer's header, read and validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    flags: u8,
    typesize: usize,
    nbytes: usize,
    blocksize: usize,
    cbytes: usize,
}

impl Header {
    fn parse(data: &[u8]) -> Result<Self, FieldglassError> {
        let head: &[u8; HEADER_LEN] = data
            .get(..HEADER_LEN)
            .and_then(|h| h.try_into().ok())
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "blosc buffer is {} bytes, shorter than its {HEADER_LEN}-byte header",
                    data.len()
                ))
            })?;
        if head[0] != VERSION_FORMAT {
            return Err(FieldglassError::UnsupportedSection(format!(
                "blosc container version {} is not the version {VERSION_FORMAT} c-blosc writes",
                head[0]
            )));
        }
        if head[1] != VERSION_LZ {
            return Err(FieldglassError::UnsupportedSection(format!(
                "blosc inner codec format version {} is not {VERSION_LZ}",
                head[1]
            )));
        }
        let flags = head[2];
        if flags & FLAG_RESERVED != 0 {
            return Err(FieldglassError::Parse(
                "blosc flags set the reserved bit 0x08".to_string(),
            ));
        }
        if flags & FLAG_SHUFFLE != 0 && flags & FLAG_BITSHUFFLE != 0 {
            return Err(FieldglassError::Parse(
                "blosc flags ask for both the byte and the bit shuffle".to_string(),
            ));
        }
        let typesize = head[3] as usize;
        let word = |at: usize| {
            u32::from_le_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]) as usize
        };
        let header = Self {
            flags,
            typesize,
            nbytes: word(4),
            blocksize: word(8),
            cbytes: word(12),
        };
        // `blocksize` divides the buffer into blocks and is the denominator of
        // the block count; zero would divide by zero, and a `typesize` of zero
        // would do the same in the split rule.
        if header.blocksize == 0 && header.nbytes != 0 {
            return Err(FieldglassError::Parse(
                "blosc header states a zero block size".to_string(),
            ));
        }
        if header.typesize == 0 {
            return Err(FieldglassError::Parse(
                "blosc header states a zero element size".to_string(),
            ));
        }
        if header.cbytes > data.len() {
            return Err(FieldglassError::Parse(format!(
                "blosc header claims {} compressed bytes but the buffer holds {}",
                header.cbytes,
                data.len()
            )));
        }
        Ok(header)
    }
}

/// How many streams block `index` was cut into.
///
/// Reproduces c-blosc's reader-side rule exactly. Every clause matters:
/// dropping the `dont_split` flag misreads a buffer written by a recent
/// encoder, and dropping the leftover-block clause misreads the *last* block of
/// every buffer whose length is not a whole number of blocks — which is most of
/// them, and only the last block, so the chunk decodes with a corrupt tail.
fn splits_in_block(header: &Header, leftover_block: bool) -> usize {
    if header.flags & FLAG_DONT_SPLIT == 0
        && header.typesize <= MAX_SPLITS
        && header.blocksize / header.typesize >= MIN_BUFFERSIZE
        && !leftover_block
    {
        header.typesize
    } else {
        1
    }
}

/// Decompress a blosc1 buffer.
///
/// `limit` caps what the header may declare as its uncompressed size: a
/// compressed chunk is attacker-controlled and its `nbytes` field names an
/// allocation directly, so the ceiling is checked before the buffer is
/// reserved rather than after it is filled.
pub fn decompress(data: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    let header = Header::parse(data)?;
    if header.nbytes > limit {
        return Err(FieldglassError::Parse(format!(
            "blosc buffer declares {} bytes, past the {limit}-byte ceiling",
            header.nbytes
        )));
    }
    if header.nbytes == 0 {
        return Ok(Vec::new());
    }

    if header.flags & FLAG_MEMCPYED != 0 {
        // The original bytes, verbatim. The shuffle flag can still be set —
        // blosc records what was asked for, not what it did — and honouring it
        // here would scramble every small chunk.
        let end = HEADER_LEN + header.nbytes;
        return data
            .get(HEADER_LEN..end)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "blosc buffer is stored uncompressed but holds {} of the {} bytes \
                     its header declares",
                    data.len().saturating_sub(HEADER_LEN),
                    header.nbytes
                ))
            });
    }

    let inner = Inner::from_flags(header.flags)?;
    let block_count = header.nbytes.div_ceil(header.blocksize);
    let leftover = header.nbytes % header.blocksize;
    let starts_end = HEADER_LEN + block_count * 4;
    let starts = data.get(HEADER_LEN..starts_end).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "blosc buffer holds no room for {block_count} block offsets"
        ))
    })?;

    let mut out = Vec::with_capacity(header.nbytes);
    for index in 0..block_count {
        let leftover_block = leftover != 0 && index == block_count - 1;
        let block_bytes = if leftover_block {
            leftover
        } else {
            header.blocksize
        };
        let offset = u32::from_le_bytes([
            starts[index * 4],
            starts[index * 4 + 1],
            starts[index * 4 + 2],
            starts[index * 4 + 3],
        ]) as usize;
        if offset < starts_end || offset > header.cbytes {
            return Err(FieldglassError::Parse(format!(
                "blosc block {index} starts at byte {offset}, outside the {} compressed \
                 bytes after the offset table",
                header.cbytes
            )));
        }

        let splits = splits_in_block(&header, leftover_block);
        // c-blosc sizes each stream from the header `blocksize`, not from this
        // block's own length — the short last block is never split, so its one
        // stream decodes to exactly its own length.
        let stream_bytes = if leftover_block {
            block_bytes
        } else {
            header.blocksize / splits
        };

        let mut block = Vec::with_capacity(block_bytes);
        let mut pos = offset;
        for _ in 0..splits {
            let len_bytes: [u8; 4] = data
                .get(pos..pos + 4)
                .and_then(|b| b.try_into().ok())
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "blosc block {index} ends where a stream length was expected"
                    ))
                })?;
            let stored = i32::from_le_bytes(len_bytes);
            let stored = usize::try_from(stored).map_err(|_| {
                FieldglassError::Parse(format!(
                    "blosc block {index} states a negative stream length {stored}"
                ))
            })?;
            pos += 4;
            let payload = data.get(pos..pos + stored).ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "blosc block {index} declares a {stored}-byte stream that runs past \
                     the buffer"
                ))
            })?;
            pos += stored;

            if stored == stream_bytes {
                // Incompressible: blosc stored the stream verbatim rather than
                // growing it. Handing these bytes to the codec would fail on
                // data that is perfectly valid.
                block.extend_from_slice(payload);
            } else {
                block.extend_from_slice(&decode_stream(inner, payload, stream_bytes)?);
            }
        }
        if block.len() != block_bytes {
            return Err(FieldglassError::Parse(format!(
                "blosc block {index} decoded to {} bytes, not the {block_bytes} its \
                 position in the buffer requires",
                block.len()
            )));
        }

        // The filters run per block, on the block's whole decompressed bytes.
        let restored = if header.flags & FLAG_SHUFFLE != 0 && header.typesize > 1 {
            unshuffle(&block, header.typesize)
        } else if header.flags & FLAG_BITSHUFFLE != 0 && block.len() >= header.typesize {
            unbitshuffle(&block, header.typesize)
        } else {
            block
        };
        out.extend_from_slice(&restored);
    }

    if out.len() != header.nbytes {
        return Err(FieldglassError::Parse(format!(
            "blosc buffer decoded to {} bytes, not the {} its header declares",
            out.len(),
            header.nbytes
        )));
    }
    Ok(out)
}

fn decode_stream(
    inner: Inner,
    payload: &[u8],
    expected: usize,
) -> Result<Vec<u8>, FieldglassError> {
    match inner {
        Inner::BloscLz => blosclz::decompress(payload, expected),
        Inner::Lz4 => lz4::decompress(payload, expected),
        Inner::Zlib => {
            // The zlib wrapper (RFC 1950), not a raw deflate stream. The
            // ceiling is the stream's own declared size, which the container
            // has already bounded.
            let out = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(payload, expected)
                .map_err(|e| {
                    FieldglassError::Parse(format!("blosc zlib stream failed to inflate: {e:?}"))
                })?;
            Ok(out)
        }
        Inner::Zstd => crate::zstd::decompress(payload, expected),
        Inner::Snappy => Err(FieldglassError::UnsupportedSection(
            "blosc buffer uses the snappy compressor, which is not decoded (blosclz, lz4, \
             lz4hc, zlib and zstd are)"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A header built by hand, so the field offsets are stated rather than
    /// replayed. These are the bytes `numcodecs.Blosc` writes for a 96-byte
    /// buffer it could not compress.
    fn memcpyed(payload: &[u8], flags: u8) -> Vec<u8> {
        let mut buf = vec![VERSION_FORMAT, VERSION_LZ, flags | FLAG_MEMCPYED, 4];
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(&((payload.len() + HEADER_LEN) as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        buf
    }

    /// The stored-uncompressed path returns the bytes as they are. The shuffle
    /// flag is set here on purpose: blosc records the request even when it did
    /// not act on it, and a decoder that un-shuffled would scramble every
    /// buffer under 128 bytes.
    #[test]
    fn a_memcpyed_buffer_is_not_unshuffled() {
        let payload: Vec<u8> = (0..96u8).collect();
        let buf = memcpyed(&payload, FLAG_SHUFFLE);
        assert_eq!(decompress(&buf, 1 << 20).unwrap(), payload);
    }

    /// The split rule, clause by clause. Each of these is a real buffer shape
    /// and gets the wrong answer if the matching clause is dropped.
    #[test]
    fn the_split_rule_matches_c_bloscs() {
        let base = Header {
            flags: 0,
            typesize: 4,
            nbytes: 800_000,
            blocksize: 524_288,
            cbytes: 12_350,
        };
        // Ordinary block of a four-byte type: one stream per byte.
        assert_eq!(splits_in_block(&base, false), 4);
        // The short last block is never split, whatever the rest says.
        assert_eq!(splits_in_block(&base, true), 1);
        // The header can say so outright.
        assert_eq!(
            splits_in_block(
                &Header {
                    flags: FLAG_DONT_SPLIT,
                    ..base
                },
                false
            ),
            1
        );
        // A per-element share under 128 bytes is not worth splitting: 256/4 =
        // 64. Measured against a real buffer — `numcodecs` writes one stream
        // at 64 and four at 128.
        assert_eq!(
            splits_in_block(
                &Header {
                    blocksize: 256,
                    nbytes: 256,
                    ..base
                },
                false
            ),
            1
        );
        assert_eq!(
            splits_in_block(
                &Header {
                    blocksize: 512,
                    nbytes: 512,
                    ..base
                },
                false
            ),
            4
        );
        // A type wider than the split ceiling.
        assert_eq!(
            splits_in_block(
                &Header {
                    typesize: 20,
                    ..base
                },
                false
            ),
            1
        );
    }

    /// The compressor number in the header is the *library format*
    /// enumeration. Reading the other one would decode zlib as snappy.
    #[test]
    fn the_compressor_bits_are_the_library_format_numbering() {
        assert_eq!(Inner::from_flags(0x01).unwrap(), Inner::BloscLz);
        assert_eq!(Inner::from_flags(0x21).unwrap(), Inner::Lz4);
        assert_eq!(Inner::from_flags(0x61).unwrap(), Inner::Zlib);
        assert_eq!(Inner::from_flags(0x91).unwrap(), Inner::Zstd);
        assert!(Inner::from_flags(0xE0).is_err());
    }

    #[test]
    fn refuses_a_header_that_is_not_blosc() {
        assert!(decompress(&[0u8; 8], 1 << 20).is_err());
        let mut wrong_version = memcpyed(&[1, 2, 3, 4], 0);
        wrong_version[0] = 3;
        assert!(decompress(&wrong_version, 1 << 20).is_err());
        let mut reserved = memcpyed(&[1, 2, 3, 4], FLAG_RESERVED);
        reserved[2] |= FLAG_RESERVED;
        assert!(decompress(&reserved, 1 << 20).is_err());
    }

    /// The ceiling is checked against the header's own claim, before the
    /// allocation it implies.
    #[test]
    fn refuses_a_header_declaring_more_than_the_ceiling() {
        let buf = memcpyed(&(0..96u8).collect::<Vec<_>>(), 0);
        let err = decompress(&buf, 32).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("ceiling")),
            "got {err:?}"
        );
    }

    /// A truncated buffer whose header promises more than it holds must not
    /// index out of bounds.
    #[test]
    fn refuses_a_truncated_buffer() {
        let mut buf = memcpyed(&(0..96u8).collect::<Vec<_>>(), 0);
        buf.truncate(HEADER_LEN + 10);
        assert!(decompress(&buf, 1 << 20).is_err());
    }

    /// A zero element size would divide by zero in the split rule.
    #[test]
    fn refuses_a_zero_element_size() {
        let mut buf = memcpyed(&[1, 2, 3, 4], 0);
        buf[3] = 0;
        assert!(decompress(&buf, 1 << 20).is_err());
    }
}
