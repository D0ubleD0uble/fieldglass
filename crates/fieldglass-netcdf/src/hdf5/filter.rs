//! HDF5 Filter Pipeline message (`0x000B`) decoder and the read-side filters
//! (issue #121, under #33). A chunked dataset may pass each chunk through a
//! pipeline of filters on write; reading reverses them, in the opposite order.
//!
//! Two filters cover the overwhelming majority of NetCDF-4 climate data, and
//! both are decoded here in pure Rust:
//!
//! * **deflate** (filter id 1) — a zlib stream, undone with `miniz_oxide`.
//! * **shuffle** (filter id 2) — reorders an element's bytes so like-significance
//!   bytes sit together (improving deflate); undone by transposing back.
//! * **fletcher32** (filter id 3) — a checksum rather than a compressor: it
//!   appends four bytes to the chunk, which reading verifies and strips (#412).
//! * **zstd** (filter id 32015) — netcdf-c >= 4.9's recommended compressor for
//!   new climate archives, undone with `ruzstd` (#413).
//!
//! Any other filter (szip, nbit, scale-offset, …) is recognised by id and
//! rejected with a clear error rather than silently mis-decoded.
//!
//! Reference: HDF5 file format specification version 3, "Data Storage - Filter
//! Pipeline Message".

use super::object_header::read_uint_le;
use fieldglass_core::FieldglassError;

/// HDF5 reserved filter identifiers we know how to reverse.
const FILTER_DEFLATE: u16 = 1;
const FILTER_SHUFFLE: u16 = 2;
const FILTER_FLETCHER32: u16 = 3;
/// Registered (not reserved) id: HDF5 allocates 32768+ to third parties, and
/// the zstd filter's is 32015 from the earlier registered-filter range.
const FILTER_ZSTD: u16 = 32015;

/// Bytes fletcher32 appends to a chunk (`FLETCHER_LEN` in libhdf5).
const FLETCHER32_LEN: usize = 4;

/// Upper bound on filters in one pipeline — guards a corrupt count.
const MAX_FILTERS: usize = 32;

/// Ceiling on what one chunk may decompress to.
///
/// A compressed chunk is attacker-controlled: the ratio is unbounded, so a few
/// kilobytes on disk can ask for an arbitrarily large allocation. Real HDF5
/// chunks are small — libhdf5's own chunk cache defaults to 1 MiB — so 256 MiB
/// is far past anything a genuine file contains while still refusing a bomb.
/// The caller checks the decompressed length against the chunk's expected size
/// afterwards, but that is after the allocation has already happened.
const MAX_DECOMPRESSED_CHUNK: usize = 256 << 20;

/// One stage of a filter pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub id: u16,
    /// Client data values (filter parameters); shuffle stores the element size
    /// here, deflate the compression level.
    pub client_data: Vec<u32>,
}

/// A dataset's decoded filter pipeline, in write (application) order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilterPipeline {
    pub filters: Vec<Filter>,
}

impl FilterPipeline {
    /// Decode a Filter Pipeline message body (versions 1 and 2).
    pub fn decode(body: &[u8]) -> Result<Self, FieldglassError> {
        let version = *body
            .first()
            .ok_or_else(|| FieldglassError::Parse("empty filter pipeline message".into()))?;
        if version != 1 && version != 2 {
            return Err(FieldglassError::Parse(format!(
                "unsupported filter pipeline message version {version}"
            )));
        }
        let count = *body
            .get(1)
            .ok_or_else(|| FieldglassError::Parse("truncated filter pipeline message".into()))?
            as usize;
        if count > MAX_FILTERS {
            return Err(FieldglassError::Parse(format!(
                "filter pipeline declares {count} filters, exceeds cap of {MAX_FILTERS}"
            )));
        }
        // Version 1 has a 6-byte reserved field after the count; version 2 has none.
        let mut pos = if version == 1 { 8 } else { 2 };

        let mut filters = Vec::with_capacity(count);
        for _ in 0..count {
            let id = read_uint_le(body, pos, 2)? as u16;
            pos += 2;
            // Name length: always present in version 1; in version 2 only when
            // the filter id is >= 256 (the optional-name range).
            let name_len = if version == 1 || id >= 256 {
                let n = read_uint_le(body, pos, 2)? as usize;
                pos += 2;
                n
            } else {
                0
            };
            // flags (2) + number of client-data values (2).
            let _flags = read_uint_le(body, pos, 2)?;
            let nvalues = read_uint_le(body, pos + 2, 2)? as usize;
            pos += 4;
            // Name, padded to an 8-byte multiple in version 1 only.
            let name_padded = if version == 1 {
                name_len.div_ceil(8) * 8
            } else {
                name_len
            };
            pos = pos
                .checked_add(name_padded)
                .filter(|&p| p <= body.len())
                .ok_or_else(|| FieldglassError::Parse("filter name overruns message".into()))?;
            // Client data values: `nvalues` 4-byte integers, padded with one more
            // in version 1 when the count is odd (to an 8-byte boundary).
            let mut client_data = Vec::with_capacity(nvalues);
            for _ in 0..nvalues {
                client_data.push(read_uint_le(body, pos, 4)? as u32);
                pos += 4;
            }
            if version == 1 && nvalues % 2 == 1 {
                pos += 4; // padding value
            }
            filters.push(Filter { id, client_data });
        }
        Ok(Self { filters })
    }

    /// Reverse the pipeline over one chunk's raw bytes. Filters run in the
    /// opposite of write order; a filter whose bit is set in `filter_mask` was
    /// skipped on write for this chunk, so it is skipped on read too.
    /// `element_size` is the dataset's element width, used by shuffle when the
    /// filter itself doesn't carry it.
    pub fn reverse(
        &self,
        mut data: Vec<u8>,
        filter_mask: u32,
        element_size: usize,
    ) -> Result<Vec<u8>, FieldglassError> {
        for (index, filter) in self.filters.iter().enumerate().rev() {
            if filter_mask & (1u32 << index) != 0 {
                continue; // filter was not applied to this chunk
            }
            data = match filter.id {
                FILTER_DEFLATE => inflate(&data)?,
                FILTER_SHUFFLE => {
                    let elem = filter
                        .client_data
                        .first()
                        .map(|&v| v as usize)
                        .filter(|&v| v > 0)
                        .unwrap_or(element_size);
                    unshuffle(&data, elem)
                }
                FILTER_FLETCHER32 => verify_fletcher32(&data)?,
                FILTER_ZSTD => unzstd(&data)?,
                other => {
                    return Err(FieldglassError::UnsupportedSection(format!(
                        "HDF5 filter id {other} is not supported (only deflate, \
                         shuffle, fletcher32, and zstd are decoded)"
                    )));
                }
            };
        }
        Ok(data)
    }
}

/// Verify a chunk's trailing fletcher32 checksum and strip it.
///
/// Unlike the compressors this is not a transform: the filter leaves the chunk
/// bytes alone and appends a four-byte checksum, so reading shortens the chunk
/// by exactly that much. Getting this wrong is not loud: a chunk written with
/// `shuffle + deflate + fletcher32` and left four bytes too long would still
/// divide evenly by the element size, and would unshuffle into one element too
/// many without complaint.
///
/// fletcher32 is written last in the pipeline, so it reverses first and the
/// checksum covers the *filtered* (compressed) bytes, not the values.
fn verify_fletcher32(data: &[u8]) -> Result<Vec<u8>, FieldglassError> {
    let split = data.len().checked_sub(FLETCHER32_LEN).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "chunk of {} bytes is too short to carry a fletcher32 checksum",
            data.len()
        ))
    })?;
    let (body, tail) = data.split_at(split);
    // libhdf5 stores the checksum with `UINT32ENCODE`, i.e. little-endian.
    let stored = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]);
    let computed = fletcher32(body);
    // libhdf5 accepts a second value here, and so must we: before release
    // 1.6.3 its checksum disagreed between big- and little-endian hosts, and
    // the fix left already-written files readable only by also comparing
    // against the old value — the bytes of each 16-bit half swapped
    // (`H5Zfletcher32.c`, "the reversed checksum").
    let legacy = ((computed & 0x00FF_00FF) << 8) | ((computed >> 8) & 0x00FF_00FF);
    if stored != computed && stored != legacy {
        return Err(FieldglassError::Parse(format!(
            "fletcher32 checksum mismatch: stored {stored:#010x}, computed \
             {computed:#010x} over {} bytes — the chunk is corrupt",
            body.len()
        )));
    }
    Ok(body.to_vec())
}

/// HDF5's Fletcher-32 checksum (`H5_checksum_fletcher32`).
///
/// Fletcher's checksum over big-endian 16-bit words, with the accumulators
/// folded every 360 words so neither can overflow, and a final fold of each to
/// 16 bits. A trailing odd byte is taken as the high half of a last word.
fn fletcher32(data: &[u8]) -> u32 {
    let mut sum1: u32 = 0;
    let mut sum2: u32 = 0;
    let (words, remainder) = data.as_chunks::<2>();
    // 360 is the largest word count for which neither running sum can overflow
    // 32 bits before the fold below; libhdf5 uses the same bound.
    for block in words.chunks(360) {
        for w in block {
            sum1 += ((w[0] as u32) << 8) | w[1] as u32;
            sum2 += sum1;
        }
        sum1 = (sum1 & 0xffff) + (sum1 >> 16);
        sum2 = (sum2 & 0xffff) + (sum2 >> 16);
    }
    if let [odd] = *remainder {
        sum1 += (odd as u32) << 8;
        sum2 += sum1;
        sum1 = (sum1 & 0xffff) + (sum1 >> 16);
        sum2 = (sum2 & 0xffff) + (sum2 >> 16);
    }
    // Second reduction, so both sums are back inside 16 bits.
    sum1 = (sum1 & 0xffff) + (sum1 >> 16);
    sum2 = (sum2 & 0xffff) + (sum2 >> 16);
    (sum2 << 16) | sum1
}

/// Inflate a zlib stream (the HDF5 deflate filter's on-disk form).
fn inflate(data: &[u8]) -> Result<Vec<u8>, FieldglassError> {
    inflate_bounded(data, MAX_DECOMPRESSED_CHUNK)
}

/// [`inflate`] with the ceiling supplied, so the bound itself is testable
/// without building a 256 MiB stream.
fn inflate_bounded(data: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    // The unbounded `decompress_to_vec_zlib` would let a crafted chunk name its
    // own allocation size.
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit)
        .map_err(|e| FieldglassError::Parse(format!("deflate (zlib) inflate failed: {e:?}")))
}

/// Decompress a zstd frame (the HDF5 zstd filter's on-disk form).
///
/// Streamed rather than decoded in one shot so the output can be capped:
/// `take` stops one byte past the ceiling, which distinguishes "too large" from
/// a stream that merely ends there.
fn unzstd(data: &[u8]) -> Result<Vec<u8>, FieldglassError> {
    unzstd_bounded(data, MAX_DECOMPRESSED_CHUNK)
}

/// [`unzstd`] with the ceiling supplied, so the bound itself is testable
/// without building a 256 MiB frame.
fn unzstd_bounded(data: &[u8], limit: usize) -> Result<Vec<u8>, FieldglassError> {
    use std::io::Read;

    let mut decoder = ruzstd::decoding::StreamingDecoder::new(data)
        .map_err(|e| FieldglassError::Parse(format!("zstd frame header: {e}")))?;
    let mut out = Vec::new();
    decoder
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| FieldglassError::Parse(format!("zstd decompress failed: {e}")))?;
    if out.len() > limit {
        return Err(FieldglassError::Parse(format!(
            "zstd chunk decompresses past the {limit}-byte ceiling"
        )));
    }
    Ok(out)
}

/// Undo the shuffle filter: bytes were grouped by position-within-element (all
/// byte 0s, then all byte 1s, …); regroup them back into consecutive elements.
fn unshuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    // A trailing partial element (or element_size <= 1) means nothing to undo.
    if element_size <= 1 || !data.len().is_multiple_of(element_size) {
        return data.to_vec();
    }
    let count = data.len() / element_size;
    let mut out = vec![0u8; data.len()];
    for byte_pos in 0..element_size {
        let base = byte_pos * count;
        for elem in 0..count {
            out[elem * element_size + byte_pos] = data[base + elem];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a version-2 filter pipeline body for the given filters.
    fn pipeline_v2(filters: &[(u16, &[u32])]) -> Vec<u8> {
        let mut body = vec![2u8, filters.len() as u8];
        for &(id, cdata) in filters {
            body.extend_from_slice(&id.to_le_bytes());
            // version-2 names omitted for id < 256
            body.extend_from_slice(&0u16.to_le_bytes()); // flags
            body.extend_from_slice(&(cdata.len() as u16).to_le_bytes());
            for &v in cdata {
                body.extend_from_slice(&v.to_le_bytes());
            }
        }
        body
    }

    #[test]
    fn decodes_shuffle_then_deflate() {
        let body = pipeline_v2(&[(FILTER_SHUFFLE, &[4]), (FILTER_DEFLATE, &[6])]);
        let p = FilterPipeline::decode(&body).unwrap();
        assert_eq!(p.filters.len(), 2);
        assert_eq!(p.filters[0].id, FILTER_SHUFFLE);
        assert_eq!(p.filters[0].client_data, vec![4]);
        assert_eq!(p.filters[1].id, FILTER_DEFLATE);
    }

    #[test]
    fn unshuffle_round_trips_a_known_layout() {
        // Two 4-byte elements: 0x01020304 and 0x05060708 (little-endian bytes
        // 04 03 02 01 and 08 07 06 05). Shuffled groups byte positions:
        // [04 08][03 07][02 06][01 05].
        let shuffled = vec![0x04, 0x08, 0x03, 0x07, 0x02, 0x06, 0x01, 0x05];
        let restored = unshuffle(&shuffled, 4);
        assert_eq!(
            restored,
            vec![0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05]
        );
    }

    #[test]
    fn reverse_applies_deflate_then_shuffle() {
        // Round-trip: shuffle(4) then deflate the bytes, then reverse should
        // recover the original.
        let original: Vec<u8> = (0u8..16).collect();
        // shuffle
        let count = original.len() / 4;
        let mut shuffled = vec![0u8; original.len()];
        for elem in 0..count {
            for byte_pos in 0..4 {
                shuffled[byte_pos * count + elem] = original[elem * 4 + byte_pos];
            }
        }
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&shuffled, 6);
        let pipeline = FilterPipeline {
            filters: vec![
                Filter {
                    id: FILTER_SHUFFLE,
                    client_data: vec![4],
                },
                Filter {
                    id: FILTER_DEFLATE,
                    client_data: vec![6],
                },
            ],
        };
        let recovered = pipeline.reverse(compressed, 0, 4).unwrap();
        assert_eq!(recovered, original);
    }

    #[test]
    fn masked_filter_is_skipped() {
        // A pipeline with deflate, but the chunk's mask says filter 0 was not
        // applied — reverse should pass the bytes through untouched.
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_DEFLATE,
                client_data: vec![6],
            }],
        };
        let raw = vec![1u8, 2, 3, 4];
        let out = pipeline.reverse(raw.clone(), 0b1, 4).unwrap();
        assert_eq!(out, raw);
    }

    /// Checksum vectors produced by libhdf5 itself, not by a second copy of
    /// this arithmetic. Each was obtained by storing the payload as a single
    /// chunk with fletcher32 and no compressor, asserting the stored chunk is
    /// the payload verbatim plus four bytes, and reading those four back
    /// (`tools/build_hdf5_fixtures.py::fletcher32_oracle`).
    #[test]
    fn fletcher32_matches_libhdf5() {
        // Even length.
        assert_eq!(fletcher32(b"abcd"), 0x2629_c4c6);
        // Odd length: the trailing byte becomes the high half of a last word.
        assert_eq!(fletcher32(b"abcde"), 0x4ff0_29c7);
        assert_eq!(fletcher32(b"the quick brown fox"), 0x43b8_3dfc);
        // 1024 words, past the 360-word fold libhdf5 applies to the sums.
        let long: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(fletcher32(&long), 0x19eb_a5b9);
    }

    /// The accumulators must not overflow between the folds.
    ///
    /// libhdf5 folds every 360 words because that is the largest block for
    /// which `sum2` provably stays inside 32 bits, and this is a
    /// re-derivation of that bound in a language where getting it wrong is a
    /// panic in debug builds rather than a silent wrap. All-`0xFF` input is the
    /// worst case: every word contributes the maximum. Several blocks' worth,
    /// and each length either side of a fold boundary.
    #[test]
    fn fletcher32_does_not_overflow_on_worst_case_input() {
        for len in [719, 720, 721, 1440, 4096, 65_536] {
            let worst = vec![0xFFu8; len];
            // The assertion is that this returns at all: an overflow here
            // panics under `cargo test`'s debug profile.
            let c = fletcher32(&worst);
            assert!(c > 0, "len {len} produced a suspiciously empty checksum");
        }
    }

    /// A chunk whose mask says fletcher32 was not applied carries no checksum
    /// suffix, so reading must not strip four bytes off it. HDF5 records a
    /// filter that was skipped on write in the per-chunk mask; the pipeline
    /// honours that generally, and this pins it for the filter whose whole job
    /// is a trailing suffix.
    #[test]
    fn a_masked_out_fletcher32_leaves_the_chunk_alone() {
        let body: Vec<u8> = (0u8..32).collect();
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&body, 6);
        let pipeline = FilterPipeline {
            filters: vec![
                Filter {
                    id: FILTER_DEFLATE,
                    client_data: vec![6],
                },
                Filter {
                    id: FILTER_FLETCHER32,
                    client_data: vec![],
                },
            ],
        };
        // Bit 1 = the second filter (fletcher32) was not applied to this chunk,
        // so the bytes are the deflate stream with nothing appended.
        let out = pipeline.reverse(compressed, 0b10, 1).unwrap();
        assert_eq!(out, body);
    }

    #[test]
    fn fletcher32_strips_the_checksum_and_verifies_it() {
        let body = b"the quick brown fox".to_vec();
        let mut chunk = body.clone();
        chunk.extend_from_slice(&fletcher32(&body).to_le_bytes());
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_FLETCHER32,
                client_data: vec![],
            }],
        };
        assert_eq!(pipeline.reverse(chunk, 0, 1).unwrap(), body);
    }

    #[test]
    fn fletcher32_rejects_a_corrupt_chunk() {
        let body = b"the quick brown fox".to_vec();
        let mut chunk = body.clone();
        chunk.extend_from_slice(&fletcher32(&body).to_le_bytes());
        chunk[3] ^= 0xFF; // flip a bit in the data, leave the checksum alone
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_FLETCHER32,
                client_data: vec![],
            }],
        };
        let err = pipeline.reverse(chunk, 0, 1).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("fletcher32 checksum mismatch")),
            "expected a checksum mismatch, got {err:?}"
        );
    }

    /// libhdf5 accepts the pre-1.6.3 checksum as well, so a file written by one
    /// of those releases still reads. Same value with the bytes of each 16-bit
    /// half swapped.
    #[test]
    fn fletcher32_accepts_the_pre_1_6_3_byte_order() {
        let body = b"the quick brown fox".to_vec();
        let c = fletcher32(&body);
        let legacy = ((c & 0x00FF_00FF) << 8) | ((c >> 8) & 0x00FF_00FF);
        assert_ne!(
            legacy, c,
            "the vector must actually distinguish the two orders"
        );
        let mut chunk = body.clone();
        chunk.extend_from_slice(&legacy.to_le_bytes());
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_FLETCHER32,
                client_data: vec![],
            }],
        };
        assert_eq!(pipeline.reverse(chunk, 0, 1).unwrap(), body);
    }

    #[test]
    fn fletcher32_rejects_a_chunk_too_short_to_hold_one() {
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_FLETCHER32,
                client_data: vec![],
            }],
        };
        // Three bytes cannot carry a four-byte checksum; this must not panic.
        let err = pipeline.reverse(vec![1, 2, 3], 0, 1).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }

    /// A zstd frame round-trips through the pipeline, composed with shuffle
    /// the way netcdf-c writes it.
    #[test]
    fn reverse_undoes_zstd_then_shuffle() {
        let original: Vec<u8> = (0u8..64).collect();
        let elem = 4;
        let count = original.len() / elem;
        let mut shuffled = vec![0u8; original.len()];
        for e in 0..count {
            for byte_pos in 0..elem {
                shuffled[byte_pos * count + e] = original[e * elem + byte_pos];
            }
        }
        let compressed = ruzstd::encoding::compress_to_vec(
            shuffled.as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        let pipeline = FilterPipeline {
            filters: vec![
                Filter {
                    id: FILTER_SHUFFLE,
                    client_data: vec![elem as u32],
                },
                Filter {
                    id: FILTER_ZSTD,
                    client_data: vec![],
                },
            ],
        };
        assert_eq!(
            pipeline.reverse(compressed, 0, elem).unwrap(),
            original,
            "zstd must be undone before shuffle"
        );
    }

    /// The decompression ceiling is a real check, not a comment.
    ///
    /// A compressed chunk is attacker-controlled and its ratio is unbounded, so
    /// without this a few kilobytes on disk can ask for an arbitrarily large
    /// allocation. Tested through the `_bounded` helpers with a small limit —
    /// building a 256 MiB stream to exercise the real constant would make the
    /// suite pay for the guarantee on every run.
    #[test]
    fn zstd_refuses_to_decompress_past_the_ceiling() {
        let big = vec![0u8; 4096]; // compresses tiny, expands past a small limit
        let frame = ruzstd::encoding::compress_to_vec(
            big.as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert!(frame.len() < big.len(), "the vector must actually expand");

        assert_eq!(unzstd_bounded(&frame, big.len()).unwrap(), big);
        let err = unzstd_bounded(&frame, 64).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("ceiling")),
            "expected a ceiling error, got {err:?}"
        );
    }

    #[test]
    fn deflate_refuses_to_decompress_past_the_ceiling() {
        let big = vec![0u8; 4096];
        let stream = miniz_oxide::deflate::compress_to_vec_zlib(&big, 6);
        assert!(stream.len() < big.len(), "the vector must actually expand");

        assert_eq!(inflate_bounded(&stream, big.len()).unwrap(), big);
        assert!(inflate_bounded(&stream, 64).is_err());
    }

    #[test]
    fn rejects_a_zstd_frame_that_is_not_one() {
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_ZSTD,
                client_data: vec![],
            }],
        };
        // No zstd magic: must be a clean error, not a panic.
        let err = pipeline.reverse(vec![0u8; 32], 0, 4).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn rejects_unknown_filter() {
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: 4, // szip
                client_data: vec![],
            }],
        };
        assert!(matches!(
            pipeline.reverse(vec![0; 8], 0, 4),
            Err(FieldglassError::UnsupportedSection(_))
        ));
    }
}
