//! HDF5 Filter Pipeline message (`0x000B`) decoder and the read-side filters
//! (issue #121, under #33). A chunked dataset may pass each chunk through a
//! pipeline of filters on write; reading reverses them, in the opposite order.
//!
//! Five filters cover the overwhelming majority of NetCDF-4 and HDF5 science
//! data, and all are decoded here in pure Rust:
//!
//! * **deflate** (filter id 1) — a zlib stream, undone with `miniz_oxide`.
//! * **shuffle** (filter id 2) — reorders an element's bytes so like-significance
//!   bytes sit together (improving deflate); undone by transposing back.
//! * **fletcher32** (filter id 3) — a checksum rather than a compressor: it
//!   appends four bytes to the chunk, which reading verifies and strips (#412).
//! * **zstd** (filter id 32015) — netcdf-c >= 4.9's recommended compressor for
//!   new climate archives, undone with `ruzstd` (#413).
//! * **szip** (filter id 4) — CCSDS 121.0 adaptive entropy coding, common
//!   across the NASA EOS archive (AIRS, MODIS). The coder and libsz's framing
//!   are `fieldglass_aec::sz`; the HDF5 part (the `cd_values` order and the
//!   size prefix) is here (#421).
//!
//! Any other filter (nbit, scale-offset, …) is recognised by id and rejected
//! with a clear error rather than silently mis-decoded.
//!
//! Reference: HDF5 file format specification version 3, "Data Storage - Filter
//! Pipeline Message".

use super::object_header::{read_uint_le, read_usize_le};
use fieldglass_aec::AecError;
use fieldglass_aec::sz::{self, SzParams};
use fieldglass_core::FieldglassError;
// The shuffle filter's inverse is the byte transpose blosc and Zarr use too, so
// it lives once in core, which proves it (#203). A chunk that is not a whole
// number of elements keeps its trailing bytes in place, as libhdf5 does.
use fieldglass_core::shuffle::unshuffle;

/// HDF5 reserved filter identifiers we know how to reverse.
const FILTER_DEFLATE: u16 = 1;
const FILTER_SHUFFLE: u16 = 2;
const FILTER_FLETCHER32: u16 = 3;
const FILTER_SZIP: u16 = 4;
/// Registered (not reserved) id: HDF5 allocates 32768+ to third parties, and
/// the zstd filter's is 32015 from the earlier registered-filter range.
const FILTER_ZSTD: u16 = 32015;

/// Bytes fletcher32 appends to a chunk (`FLETCHER_LEN` in libhdf5).
const FLETCHER32_LEN: usize = 4;

/// Client data values the szip filter carries (`H5Z_SZIP_TOTAL_NPARMS`).
const SZIP_CD_VALUES: usize = 4;

/// Bytes of the uncompressed-size prefix libhdf5 writes in front of every
/// szip stream (`UINT32ENCODE` in `H5Zszip.c`, so little-endian).
const SZIP_PREFIX_LEN: usize = 4;

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

/// Ceiling on the window a zstd frame may ask the decoder to buffer.
///
/// A separate bound from [`MAX_DECOMPRESSED_CHUNK`], because the two bound
/// different things: the window is a back-reference buffer sized from the frame
/// *header*, before any output exists, so the output ceiling cannot see it. The
/// two are also genuinely independent — a small window producing enormous
/// output is precisely what a decompression bomb is.
///
/// An encoder sets the window to about the size of the data it is compressing,
/// so a window this large already implies a single HDF5 chunk of 64 MiB, which
/// no sane writer produces (libhdf5's own chunk cache defaults to 1 MiB). This
/// is deliberately tighter than `ruzstd`'s 100 MiB default: the guarantee
/// should be ours, and stated, rather than inherited from an upstream default
/// that a version bump could quietly widen.
const MAX_ZSTD_WINDOW: u64 = 64 << 20;

/// One stage of a filter pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    /// The registered HDF5 filter id — 1 deflate, 2 shuffle, 32015 zstd, …
    pub id: u16,
    /// Client data values (filter parameters); shuffle stores the element size
    /// here, deflate the compression level.
    pub client_data: Vec<u32>,
}

/// A dataset's decoded filter pipeline, in write (application) order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilterPipeline {
    /// The stages in write order; reading applies their inverses in reverse.
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
                let n = read_usize_le(body, pos, 2)?;
                pos += 2;
                n
            } else {
                0
            };
            // flags (2) + number of client-data values (2).
            let _flags = read_uint_le(body, pos, 2)?;
            let nvalues = read_usize_le(body, pos + 2, 2)?;
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
    /// filter itself doesn't carry it. `expected_len` is the chunk's length
    /// before any filter ran, which szip checks its size prefix against when
    /// every filter before it keeps the length, and bounds it otherwise. The
    /// caller must still check that the result is exactly `expected_len`:
    /// deflate and zstd are bounded by a ceiling only.
    pub fn reverse(
        &self,
        mut data: Vec<u8>,
        filter_mask: u32,
        element_size: usize,
        expected_len: usize,
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
                FILTER_SZIP => {
                    let exact = self.length_before(index, filter_mask, expected_len);
                    unszip(
                        &data,
                        &filter.client_data,
                        exact,
                        szip_limit(exact, expected_len),
                    )?
                }
                other => {
                    return Err(FieldglassError::UnsupportedSection(format!(
                        "HDF5 filter id {other} is not supported (only deflate, \
                         shuffle, fletcher32, zstd, and szip are decoded)"
                    )));
                }
            };
        }
        Ok(data)
    }

    /// The exact length filter `index` was handed on write, when it is known:
    /// `expected_len` if every filter before it that ran on this chunk keeps
    /// the length, and `None` otherwise.
    ///
    /// Only shuffle keeps the length. deflate and zstd produce a stream of
    /// any length, and fletcher32 adds four bytes. When one of them ran before
    /// szip, szip's size prefix is that filter's output length, which nothing
    /// outside the stream records, so the prefix is only bounded, by
    /// [`szip_limit`], and the caller's check that the chunk comes back
    /// exactly its own length does the rest. libhdf5 writes and reads such
    /// pipelines, so they are decoded rather than refused.
    fn length_before(&self, index: usize, filter_mask: u32, expected_len: usize) -> Option<usize> {
        self.filters[..index]
            .iter()
            .enumerate()
            .all(|(i, f)| filter_mask & (1u32 << i) != 0 || f.id == FILTER_SHUFFLE)
            .then_some(expected_len)
    }
}

/// The most an szip size prefix may declare before it is allocated.
///
/// When the exact length is known the prefix must equal it, so the ceiling
/// alone is the backstop. When it is not, a length-changing filter ran
/// before szip, and szip's input was that filter's output: deflate's or
/// zstd's worst case grows the data by well under an eighth (zlib's bound is
/// a few bytes per 16 KiB, zstd's about 1/256), and fletcher32 adds four
/// bytes. So the chunk's length plus an eighth plus 4 KiB covers any real
/// file, and keeps a 12-byte chunk from committing a 256 MiB zeroed buffer
/// (which on wasm is a real allocation, not a lazily mapped one).
fn szip_limit(exact: Option<usize>, expected_len: usize) -> usize {
    match exact {
        Some(_) => MAX_DECOMPRESSED_CHUNK,
        None => expected_len
            .saturating_add(expected_len / 8)
            .saturating_add(SZIP_SLACK)
            .min(MAX_DECOMPRESSED_CHUNK),
    }
}

/// Fixed headroom in [`szip_limit`] for small chunks, whose compressed form
/// can carry a header larger than an eighth of the data.
const SZIP_SLACK: usize = 4096;

/// Undo the HDF5 szip filter (id 4) on one chunk.
///
/// The chunk is a 4-byte little-endian uncompressed size, then an szip stream
/// (`H5Zszip.c`). `cd_values` are in libhdf5's order, `(mask, pixels per
/// block, bits per pixel, pixels per scanline)` (`H5Zpublic.h`), which is
/// **not** [`SzParams`]' libsz order `(mask, bits per pixel, pixels per block,
/// pixels per scanline)`. Passing them through positionally swaps the middle
/// two, and most swaps are still valid parameters that decode to garbage.
///
/// The prefix is checked before anything is allocated for it. It must be:
///
/// - at most `limit` bytes ([`szip_limit`] outside tests);
/// - equal to `exact`, when that is known ([`FilterPipeline::length_before`]);
/// - a whole number of szip pixels. The pixel is the width bits per pixel
///   rounds up to (1, 2, 4 or 8 bytes), not the dataset's element: libhdf5
///   codes a 16-bit-precision `int32` at 16 bits per pixel, and the chunk is
///   still a whole number of those (ADR-0012 decision D2).
///
/// Parameters `fieldglass_aec` does not accept are
/// [`FieldglassError::UnsupportedSection`]; a stream that does not decode to
/// exactly the prefix's length is [`FieldglassError::Parse`].
fn unszip(
    data: &[u8],
    cd_values: &[u32],
    exact: Option<usize>,
    limit: usize,
) -> Result<Vec<u8>, FieldglassError> {
    let &[mask, ppb, bpp, pps] = cd_values else {
        return Err(FieldglassError::Parse(format!(
            "szip filter carries {} client data values, expected {SZIP_CD_VALUES} \
             (mask, pixels per block, bits per pixel, pixels per scanline)",
            cd_values.len()
        )));
    };
    // The swap: HDF5 stores (mask, ppb, bpp, pps), libsz takes (mask, bpp, ppb, pps).
    let params = SzParams::new(mask, bpp, ppb, pps).map_err(szip_error)?;

    let (prefix, stream) = data.split_first_chunk::<SZIP_PREFIX_LEN>().ok_or_else(|| {
        FieldglassError::Parse(format!(
            "szip chunk of {} bytes is too short for its {SZIP_PREFIX_LEN}-byte size prefix",
            data.len()
        ))
    })?;
    // A `u32` fits a `usize` on every target this builds for (64-bit hosts and
    // wasm32); saturating would only matter on a 16-bit one.
    let len = usize::try_from(u32::from_le_bytes(*prefix)).unwrap_or(usize::MAX);
    if len > limit {
        return Err(FieldglassError::Parse(format!(
            "szip chunk declares {len} bytes, past the {limit}-byte ceiling"
        )));
    }
    if let Some(expected) = exact
        && len != expected
    {
        return Err(FieldglassError::Parse(format!(
            "szip chunk declares {len} bytes, but the chunk is {expected} bytes"
        )));
    }
    let pixel = params.bytes_per_pixel();
    if !len.is_multiple_of(pixel) {
        return Err(FieldglassError::Parse(format!(
            "szip chunk declares {len} bytes, not a whole number of {pixel}-byte pixels \
             at {bpp} bits per pixel"
        )));
    }

    let mut out = vec![0u8; len];
    sz::decompress(stream, &params, &mut out).map_err(szip_error)?;
    Ok(out)
}

/// Map a codec error onto the reader's: a parameter set the codec does not
/// take is an unsupported file, anything else is a corrupt one.
fn szip_error(e: AecError) -> FieldglassError {
    match e {
        AecError::BitsPerPixel(_)
        | AecError::PixelsPerBlock(_)
        | AecError::PixelsPerScanline(_)
        | AecError::BitsPerSample(_)
        | AecError::BlockSize(_)
        | AecError::Rsi(_)
        | AecError::Restricted(_)
        | AecError::OutputTooLarge { .. } => {
            FieldglassError::UnsupportedSection(format!("HDF5 szip filter: {e}"))
        }
        _ => FieldglassError::Parse(format!("szip decompress failed: {e}")),
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

    // Two ceilings, because a crafted frame has two ways to ask for memory.
    // [`MAX_ZSTD_WINDOW`] bounds the buffer sized from the frame header, which
    // is allocated before any output exists and so is invisible to the `take`
    // below; [`MAX_DECOMPRESSED_CHUNK`] bounds the output itself.
    let mut decoder =
        ruzstd::decoding::StreamingDecoder::new_with_max_window_size(data, MAX_ZSTD_WINDOW)
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

    /// A chunk that is not a whole number of elements is unshuffled the way
    /// libhdf5 does it (`H5Z__filter_shuffle`): the whole elements are
    /// transposed and the trailing bytes stay where they are. Before the
    /// transform moved to core (#203) such a chunk came back still shuffled.
    #[test]
    fn reverse_unshuffles_the_whole_elements_of_a_ragged_chunk() {
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_SHUFFLE,
                client_data: vec![4],
            }],
        };
        // Two shuffled 4-byte elements, then three bytes past the last one.
        let chunk = vec![
            0x04, 0x08, 0x03, 0x07, 0x02, 0x06, 0x01, 0x05, 0xE0, 0xE1, 0xE2,
        ];
        assert_eq!(
            pipeline.reverse(chunk, 0, 4, 11).unwrap(),
            vec![
                0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05, 0xE0, 0xE1, 0xE2
            ]
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
        let recovered = pipeline.reverse(compressed, 0, 4, original.len()).unwrap();
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
        let out = pipeline.reverse(raw.clone(), 0b1, 4, raw.len()).unwrap();
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
        let out = pipeline.reverse(compressed, 0b10, 1, body.len()).unwrap();
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
        assert_eq!(pipeline.reverse(chunk, 0, 1, body.len()).unwrap(), body);
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
        let err = pipeline.reverse(chunk, 0, 1, body.len()).unwrap_err();
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
        assert_eq!(pipeline.reverse(chunk, 0, 1, body.len()).unwrap(), body);
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
        let err = pipeline.reverse(vec![1, 2, 3], 0, 1, 0).unwrap_err();
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
            pipeline
                .reverse(compressed, 0, elem, original.len())
                .unwrap(),
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

    /// A frame header declaring an enormous window is refused before anything
    /// is allocated for it.
    ///
    /// This is the decompression bomb the output ceiling cannot catch: the
    /// window buffer is sized from the header, so the failure has to happen
    /// during frame init, not while reading output.
    #[test]
    fn rejects_a_zstd_frame_demanding_an_enormous_window() {
        // Hand-built zstd frame header (RFC 8878 §3.1.1): magic, then a frame
        // header descriptor of 0 (no content size, not single-segment, no
        // dictionary, no checksum) so a window descriptor follows. That byte is
        // `exponent << 3 | mantissa`, and window = 2^(10 + exponent) scaled by
        // the mantissa — exponent 31 asks for 2 TiB.
        let frame = [0x28u8, 0xB5, 0x2F, 0xFD, 0x00, 0xF8];
        let err = unzstd(&frame).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("frame header")),
            "expected the header to be rejected, got {err:?}"
        );

        // The ceiling is *ours*, not whatever the decoder defaults to. This
        // window is 72 MiB — inside `ruzstd`'s 100 MiB default, outside our
        // 64 MiB one — so it fails only because we set the bound ourselves. If
        // a future bump changed the upstream default, this would still hold.
        // The ceiling is *ours*, not whatever the decoder defaults to. This
        // frame is otherwise complete and valid — same header, then one raw
        // block of four bytes — and declares a 72 MiB window, inside `ruzstd`'s
        // 100 MiB default but outside our 64 MiB one. Under the default it
        // decodes to `AA BB CC DD`; it fails here only because we set the
        // bound, so a future bump that widened the upstream default could not
        // pass this test unnoticed.
        let valid_but_wide = [
            0x28u8, 0xB5, 0x2F, 0xFD, // magic
            0x00, // frame header descriptor: no content size, no dictionary
            0x81, // window descriptor: exponent 16, mantissa 1 => 72 MiB
            0x21, 0x00, 0x00, // block header: last block, raw, 4 bytes
            0xAA, 0xBB, 0xCC, 0xDD, // the block's literal bytes
        ];
        let err = unzstd(&valid_but_wide).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("frame header")),
            "expected our window bound to refuse this frame, got {err:?}"
        );

        let ok = ruzstd::encoding::compress_to_vec(
            [0u8; 128].as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert!(unzstd(&ok).is_ok(), "an ordinary frame must still decode");
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
        let err = pipeline.reverse(vec![0u8; 32], 0, 4, 32).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn rejects_unknown_filter() {
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: 5, // nbit
                client_data: vec![],
            }],
        };
        assert!(matches!(
            pipeline.reverse(vec![0; 8], 0, 4, 8),
            Err(FieldglassError::UnsupportedSection(_))
        ));
    }

    /// An szip stream from `fieldglass_aec::sz`'s own documentation: 8-bit
    /// pixels, blocks of 2, one pixel per scanline, so each scanline is a
    /// block of the pixel and one pad. It decodes to `[7, 8, 9]`.
    const SZ_STREAM: [u8; 8] = [0xE0, 0xE0, 0x1C, 0x20, 0x03, 0x84, 0x80, 0x00];
    const SZ_PIXELS: [u8; 3] = [7, 8, 9];
    /// The same parameters in HDF5's `cd_values` order: mask, pixels per
    /// block, bits per pixel, pixels per scanline.
    const SZ_CD: [u32; 4] = [0, 2, 8, 1];

    /// A chunk as libhdf5 stores it: the size prefix, then the stream.
    fn szip_chunk(prefix: u32) -> Vec<u8> {
        let mut chunk = prefix.to_le_bytes().to_vec();
        chunk.extend_from_slice(&SZ_STREAM);
        chunk
    }

    fn szip_pipeline(before: &[u16], cd: &[u32]) -> FilterPipeline {
        let mut filters: Vec<Filter> = before
            .iter()
            .map(|&id| Filter {
                id,
                client_data: vec![1],
            })
            .collect();
        filters.push(Filter {
            id: FILTER_SZIP,
            client_data: cd.to_vec(),
        });
        FilterPipeline { filters }
    }

    fn parse_message(r: Result<Vec<u8>, FieldglassError>) -> String {
        match r {
            Err(FieldglassError::Parse(m)) => m,
            other => panic!("expected a Parse error, got {other:?}"),
        }
    }

    #[test]
    fn szip_decodes_a_chunk_through_the_pipeline() {
        let pipeline = szip_pipeline(&[], &SZ_CD);
        assert_eq!(pipeline.reverse(szip_chunk(3), 0, 1, 3).unwrap(), SZ_PIXELS);
    }

    /// HDF5 stores `(mask, ppb, bpp, pps)`; libsz takes `(mask, bpp, ppb,
    /// pps)`. Here ppb is 2 and bpp is 8, so passing `cd_values` through
    /// positionally would ask for 2-bit pixels in blocks of 8: still valid
    /// parameters, and not these pixels.
    #[test]
    fn szip_reads_cd_values_in_hdf5_order() {
        let [mask, ppb, bpp, pps] = SZ_CD;
        let swapped = [mask, bpp, ppb, pps];
        assert!(
            SzParams::new(mask, ppb, bpp, pps).is_ok(),
            "the swapped set must be valid, or this test proves nothing"
        );
        assert_ne!(
            unszip(&szip_chunk(3), &swapped, Some(3), MAX_DECOMPRESSED_CHUNK).ok(),
            Some(SZ_PIXELS.to_vec())
        );
        assert_eq!(
            unszip(&szip_chunk(3), &SZ_CD, Some(3), MAX_DECOMPRESSED_CHUNK).unwrap(),
            SZ_PIXELS
        );
    }

    #[test]
    fn szip_needs_exactly_four_cd_values() {
        for cd in [&SZ_CD[..3], &[0, 2, 8, 1, 0][..], &[][..]] {
            let m = parse_message(unszip(&szip_chunk(3), cd, Some(3), MAX_DECOMPRESSED_CHUNK));
            assert!(m.contains("client data values"), "{cd:?}: {m}");
        }
    }

    /// A parameter set the codec does not take is an unsupported file, not a
    /// corrupt one: 7 pixels per block is odd, 48 bits per pixel is neither
    /// 1–32 nor 64.
    #[test]
    fn szip_parameters_outside_the_codec_are_unsupported() {
        for cd in [[0, 7, 8, 1], [0, 2, 48, 1], [0, 2, 8, 0], [0, 2, 8, 5000]] {
            let r = unszip(&szip_chunk(3), &cd, Some(3), MAX_DECOMPRESSED_CHUNK);
            assert!(
                matches!(r, Err(FieldglassError::UnsupportedSection(_))),
                "{cd:?}: {r:?}"
            );
        }
    }

    /// The prefix must equal the chunk's length when every filter before szip
    /// keeps the length. The chunk is valid apart from the prefix: one byte
    /// either side of the right length is refused, the right one decodes.
    #[test]
    fn szip_prefix_must_equal_the_expected_length() {
        let pipeline = szip_pipeline(&[FILTER_SHUFFLE], &SZ_CD);
        for prefix in [2, 4] {
            let m = parse_message(pipeline.reverse(szip_chunk(prefix), 0, 1, 3));
            assert!(
                m.contains(&format!("declares {prefix} bytes, but the chunk is 3")),
                "{m}"
            );
        }
        assert_eq!(pipeline.reverse(szip_chunk(3), 0, 1, 3).unwrap(), SZ_PIXELS);
    }

    /// The ceiling is checked before the prefix is allocated, with the real
    /// constant, on a chunk that is valid apart from its prefix, and with no
    /// expected length to catch it first.
    #[test]
    fn szip_prefix_past_the_ceiling_is_refused_before_allocating() {
        let over = u32::try_from(MAX_DECOMPRESSED_CHUNK + 1).unwrap();
        let m = parse_message(unszip(
            &szip_chunk(over),
            &SZ_CD,
            None,
            MAX_DECOMPRESSED_CHUNK,
        ));
        assert!(m.contains("ceiling"), "{m}");

        // The bound is exact: a limit of the chunk's own length passes, one
        // byte less does not.
        assert_eq!(unszip(&szip_chunk(3), &SZ_CD, None, 3).unwrap(), SZ_PIXELS);
        let m = parse_message(unszip(&szip_chunk(3), &SZ_CD, None, 2));
        assert!(m.contains("ceiling"), "{m}");
    }

    /// Which filters let szip know its length. Shuffle keeps it; deflate,
    /// zstd and fletcher32 do not, unless the chunk's mask says they did not
    /// run. Filters after szip never matter.
    #[test]
    fn szip_knows_its_length_only_behind_length_preserving_filters() {
        let cases: [(&[u16], u32, Option<usize>); 7] = [
            (&[], 0, Some(64)),
            (&[FILTER_SHUFFLE], 0, Some(64)),
            (&[FILTER_DEFLATE], 0, None),
            (&[FILTER_ZSTD], 0, None),
            (&[FILTER_FLETCHER32], 0, None),
            (&[FILTER_SHUFFLE, FILTER_DEFLATE], 0, None),
            // Bit 1: deflate was skipped for this chunk.
            (&[FILTER_SHUFFLE, FILTER_DEFLATE], 0b10, Some(64)),
        ];
        for (before, mask, want) in cases {
            let pipeline = szip_pipeline(before, &SZ_CD);
            assert_eq!(
                pipeline.length_before(before.len(), mask, 64),
                want,
                "{before:?} mask {mask:#b}"
            );
        }
        let mut after = szip_pipeline(&[], &SZ_CD);
        after.filters.push(Filter {
            id: FILTER_DEFLATE,
            client_data: vec![4],
        });
        assert_eq!(after.length_before(0, 0, 64), Some(64));
    }

    /// Behind a length-changing filter the prefix is bounded by the ceiling
    /// alone: a prefix that is not the chunk's length still decodes, and what
    /// comes out is handed to the filter before szip.
    #[test]
    fn szip_behind_a_length_changing_filter_is_decoded_not_refused() {
        // fletcher32 ran before szip, so the chunk's length (64) is not what
        // szip was handed and is not checked against the prefix (3). szip
        // decodes, and the error that follows is fletcher32's, on szip's
        // 3-byte output. `hdf5_szip_hand.h5`'s `deflate_szip` is the same rule
        // end to end, on a pipeline libhdf5 reads.
        let pipeline = szip_pipeline(&[FILTER_FLETCHER32], &SZ_CD);
        let m = parse_message(pipeline.reverse(szip_chunk(3), 0, 1, 64));
        assert!(
            m.contains("too short to carry a fletcher32 checksum"),
            "{m}"
        );
    }

    /// D2: the pixel is set by bits per pixel, not by the element. A length
    /// that is not a whole number of pixels is refused before decoding.
    #[test]
    fn szip_length_must_be_a_whole_number_of_pixels() {
        // 16 bits per pixel: 3 bytes is a pixel and a half.
        let m = parse_message(unszip(
            &szip_chunk(3),
            &[0, 2, 16, 1],
            None,
            MAX_DECOMPRESSED_CHUNK,
        ));
        assert!(m.contains("whole number of 2-byte pixels"), "{m}");
        // 64 bits per pixel: 12 bytes is a pixel and a half, though a whole
        // number of 4-byte elements.
        let m = parse_message(unszip(
            &szip_chunk(12),
            &[0, 2, 64, 1],
            None,
            MAX_DECOMPRESSED_CHUNK,
        ));
        assert!(m.contains("whole number of 8-byte pixels"), "{m}");
    }

    /// Behind a length-changing filter the prefix is bounded by the chunk's
    /// length plus an eighth plus 4 KiB, not by the 256 MiB ceiling: a prefix
    /// between the two is refused before it is allocated. At the bound itself
    /// the prefix passes the check and the stream fails later, in the codec.
    #[test]
    fn szip_behind_a_length_changing_filter_is_bounded_by_the_chunk() {
        let expected = 64;
        let bound = expected + expected / 8 + SZIP_SLACK;
        assert_eq!(szip_limit(None, expected), bound);
        assert_eq!(szip_limit(Some(expected), expected), MAX_DECOMPRESSED_CHUNK);
        assert_eq!(szip_limit(None, usize::MAX), MAX_DECOMPRESSED_CHUNK);
        assert!(bound < MAX_DECOMPRESSED_CHUNK);

        let pipeline = szip_pipeline(&[FILTER_FLETCHER32], &SZ_CD);
        let over = u32::try_from(bound + 1).unwrap();
        let m = parse_message(pipeline.reverse(szip_chunk(over), 0, 1, expected));
        assert!(m.contains(&format!("past the {bound}-byte ceiling")), "{m}");
        let at = u32::try_from(bound).unwrap();
        let m = parse_message(pipeline.reverse(szip_chunk(at), 0, 1, expected));
        assert!(m.contains("szip decompress failed"), "{m}");
    }

    #[test]
    fn szip_refuses_a_truncated_or_prefixless_chunk() {
        // One pixel more than the stream holds.
        let m = parse_message(unszip(&szip_chunk(4), &SZ_CD, None, MAX_DECOMPRESSED_CHUNK));
        assert!(m.contains("szip decompress failed"), "{m}");
        // Shorter than the prefix itself.
        let m = parse_message(unszip(&[3, 0, 0], &SZ_CD, None, MAX_DECOMPRESSED_CHUNK));
        assert!(m.contains("size prefix"), "{m}");
    }
}
