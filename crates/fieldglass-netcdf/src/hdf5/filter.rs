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
//! * **szip** (filter id 4) — CCSDS 121.0 adaptive entropy coding. The coder
//!   and libsz's framing are `fieldglass_aec::sz`; the HDF5 part (the
//!   `cd_values` order and the size prefix) is here (#421).
//!
//! Any other filter (nbit, scale-offset, …) is recognized by id and rejected
//! with a clear error rather than silently mis-decoded.
//!
//! Reference: HDF5 file format specification version 3, "Data Storage - Filter
//! Pipeline Message".

use super::object_header::{read_uint_le, read_usize_le};
use fieldglass_aec::AecError;
use fieldglass_aec::sz::{self, SzParams};
use fieldglass_core::FieldglassError;
use fieldglass_core::array::ArrayError;
use fieldglass_core::output_limits;
use ruzstd::decoding::errors::FrameDecoderError;
// The shuffle filter's inverse is the byte transpose blosc and Zarr use too, so
// it lives once in core, which proves it (#203). A chunk that is not a whole
// number of elements keeps its trailing bytes in place, as libhdf5 does.
use fieldglass_core::byte_shuffle::unshuffle;

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
///
/// A chunk whose own decoded length is past this is refused before it is read
/// ([`check_chunk_size`], #970): the length comes from the chunk's dimensions,
/// which the file states and which may legally dwarf the dataset. So the
/// ceiling bounds a codec only where nothing records the length it is
/// handed, behind a filter that changes it ([`FilterPipeline::limit_behind`]),
/// and there grown by what those filters add, up to twice it (#1016).
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
/// is deliberately tighter than `ruzstd`'s 128 MiB default: the guarantee
/// should be ours, and stated, rather than inherited from an upstream default
/// that a version bump could quietly widen.
///
/// Each frame is held to less when its chunk is smaller: the window may be
/// the codec's output limit, or 8 MiB if that is more
/// ([`output_limits::zstd_window_limit`], #989). Past that a frame is refused as
/// [`ArrayError::WindowTooLarge`], which says the file is fine, unless it is
/// single-segment and so decodes past the limit.
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
    /// before any filter ran. A codec that every filter before it keeps the
    /// length for must produce exactly that: szip checks its size prefix
    /// against it, and deflate and zstd stop there (#813). Behind a filter
    /// that changes the length, all three are bounded instead by the chunk's
    /// length grown by the worst case of each filter before them
    /// (`filter_growth`): 4 bytes for fletcher32, zlib's `deflateBound` for
    /// deflate, `ZSTD_COMPRESSBOUND` for zstd, and for szip the longest
    /// stream its parameters allow, at most 33 times; never more than twice
    /// the ceiling. The caller must still check that the result is exactly
    /// `expected_len`.
    ///
    /// An `expected_len` past 256 MiB (`MAX_DECOMPRESSED_CHUNK`) is refused as
    /// [`ArrayError::ChunkTooLarge`] before any filter runs, unless the mask
    /// skips every filter and the chunk passes through as stored. A chunk
    /// reader refuses it before reading the chunk (`check_chunk_size`);
    /// this is the backstop for any other caller, and is what keeps every
    /// codec's exact length within the ceiling.
    pub fn reverse(
        &self,
        mut data: Vec<u8>,
        filter_mask: u32,
        element_size: usize,
        expected_len: usize,
    ) -> Result<Vec<u8>, FieldglassError> {
        let runs = |index: usize| filter_mask & (1u32 << index) == 0;
        if expected_len > MAX_DECOMPRESSED_CHUNK && (0..self.filters.len()).any(runs) {
            return Err(chunk_too_large(expected_len as u64));
        }
        // An szip's parameters bound the codec before it (`limit_behind`), so
        // a set the codec does not take is refused before anything runs.
        for (index, filter) in self.filters.iter().enumerate() {
            if filter.id == FILTER_SZIP && runs(index) {
                szip_params(&filter.client_data)?;
            }
        }
        for (index, filter) in self.filters.iter().enumerate().rev() {
            if filter_mask & (1u32 << index) != 0 {
                continue; // filter was not applied to this chunk
            }
            data = match filter.id {
                FILTER_DEFLATE => {
                    let exact = self.length_before(index, filter_mask, expected_len);
                    let behind = self.limit_behind(index, filter_mask, expected_len);
                    inflate_bounded(&data, codec_limit(exact, behind))?
                }
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
                FILTER_ZSTD => {
                    let exact = self.length_before(index, filter_mask, expected_len);
                    let behind = self.limit_behind(index, filter_mask, expected_len);
                    unzstd_bounded(&data, codec_limit(exact, behind))?
                }
                FILTER_SZIP => {
                    let exact = self.length_before(index, filter_mask, expected_len);
                    let behind = self.limit_behind(index, filter_mask, expected_len);
                    unszip(&data, &filter.client_data, exact, szip_limit(exact, behind))?
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

    /// `len` grown by the worst case ([`filter_growth`]) of each filter in
    /// `filters[..upto]` that `filter_mask` leaves running, in write order.
    fn grown(&self, upto: usize, filter_mask: u32, len: u64) -> u64 {
        self.filters[..upto]
            .iter()
            .enumerate()
            .filter(|&(i, _)| filter_mask & (1u32 << i) == 0)
            .fold(len, |len, (_, f)| filter_growth(f, len))
    }

    /// The most filter `index` may be handed back when
    /// [`Self::length_before`] cannot say exactly: the chunk's length grown by
    /// the worst case of every filter before it that ran ([`Self::grown`]),
    /// and never more than twice `MAX_DECOMPRESSED_CHUNK`.
    ///
    /// A chunk at the ceiling may pass it here, as it may stored
    /// ([`Self::stored_limit`]): netCDF-C always puts fletcher32 first
    /// (`NC4_hdf5_addfilter` in `libhdf5/hdf5filter.c`), so a 256 MiB chunk
    /// under fletcher32 and deflate inflates to 256 MiB and 4 bytes (#1016).
    /// The cap of twice the ceiling is `stored_limit`'s: the pipeline is the
    /// file's to choose, so an szip ahead of a codec, whose parameters may
    /// allow 33 times the chunk, cannot lift the bound past it.
    fn limit_behind(&self, index: usize, filter_mask: u32, expected_len: usize) -> usize {
        let most = 2 * MAX_DECOMPRESSED_CHUNK;
        // In range: at most `most`, which fits a 32-bit `usize`.
        self.grown(index, filter_mask, expected_len as u64)
            .min(most as u64) as usize
    }

    /// The most a chunk or heap block of `decoded` bytes may be stored as,
    /// when the filters `filter_mask` leaves running wrote it and `ceiling` is
    /// what it may decode to (#1016).
    ///
    /// Each filter that ran grows the length by at most [`filter_growth`], in
    /// write order ([`Self::grown`]). Never less than `ceiling`, the
    /// stored-size cap #997 set, so nothing it read is refused now. Never more
    /// than twice `ceiling`, so a hostile size still cannot ask for an
    /// unbounded read. Twice holds fletcher32, deflate or zstd alone on a
    /// chunk at the ceiling, and every szip libhdf5 writes: it never
    /// picks a scanline shorter than a block, which keeps a stream under twice
    /// its input, and its szip filter is optional, so a chunk szip would grow
    /// is stored unfiltered with its mask bit set. Only a writer outside
    /// libhdf5's limits, whose szip may reach 33 times (ADR-0012 decision 6),
    /// is refused as too large.
    pub(super) fn stored_limit(&self, filter_mask: u32, decoded: u64, ceiling: u64) -> u64 {
        self.grown(self.filters.len(), filter_mask, decoded)
            .clamp(ceiling, ceiling.saturating_mul(2))
    }
}

/// The most filter `f` may write when handed `len` bytes: its worst case as
/// an encoder within the format writes it. The one table of filter growth,
/// for a stored size ([`FilterPipeline::stored_limit`]) and for a codec's
/// output behind other filters ([`FilterPipeline::limit_behind`]).
///
/// - shuffle reorders bytes and keeps the length.
/// - fletcher32 appends its 4-byte checksum (`FLETCHER_LEN` in libhdf5).
/// - deflate: zlib's `deflateBound` for any parameters
///   ([`output_limits::deflate_compress_bound`]), plus 6 bytes of zlib wrapper
///   (RFC 1950 §2.2: a 2-byte header and the Adler-32). Stored blocks on data
///   that does not compress, as zlib writes at its defaults, add about
///   20 KiB at 256 MiB, well inside it. libhdf5 keeps such output rather than
///   skipping the filter.
/// - zstd: `ZSTD_COMPRESSBOUND` ([`output_limits::zstd_compress_bound`]), `len +
///   len/256` plus a margin of at most 64 bytes under 128 KiB.
/// - szip: [`szip_growth`], from its parameters.
/// - Any other filter is refused once the chunk is read
///   ([`FilterPipeline::reverse`]), and is given deflate's room until then.
fn filter_growth(f: &Filter, len: u64) -> u64 {
    // The wrapper `deflateBound` adds when it cannot see the stream's
    // parameters: zlib's 2-byte header and Adler-32.
    let deflate = |len: u64| output_limits::deflate_compress_bound(len).saturating_add(6);
    match f.id {
        FILTER_SHUFFLE => len,
        FILTER_FLETCHER32 => len.saturating_add(FLETCHER32_LEN as u64),
        FILTER_ZSTD => output_limits::zstd_compress_bound(len),
        // Past `usize` only on a 32-bit target, where the cap of twice the
        // ceiling bounds it anyway.
        FILTER_SZIP => {
            usize::try_from(len).map_or(u64::MAX, |len| szip_growth(&f.client_data, len) as u64)
        }
        _ => deflate(len),
    }
}

/// Refuse, before it is read, a chunk of `chunk_dims` whose `chunk_bytes`
/// decoded no reader here holds at once (#970).
///
/// A filtered chunk is decoded whole however little of it a read wants, and
/// its dimensions are the file's own numbers, which may legally dwarf the
/// dataset. Two bounds, each a valid file asked for too much, so a host
/// reports `unsupported` rather than a corrupt file: more values than one
/// field holds is [`ArrayError::FieldTooLarge`], the check a Zarr chunk meets
/// too, and more than [`MAX_DECOMPRESSED_CHUNK`] bytes is
/// [`ArrayError::ChunkTooLarge`]. Which one binds depends on the element
/// width: they agree for four-byte elements, the element cap is the tighter
/// below that and the byte cap above.
///
/// An unfiltered chunk needs neither: it is read at its stored size, which
/// must be the chunk's own and is checked to be before the read (#997).
pub(super) fn check_chunk_size(
    chunk_dims: &[u64],
    chunk_bytes: usize,
) -> Result<(), FieldglassError> {
    fieldglass_core::array::field_element_count(chunk_dims)?;
    if chunk_bytes > MAX_DECOMPRESSED_CHUNK {
        return Err(chunk_too_large(chunk_bytes as u64));
    }
    Ok(())
}

/// Refuse a filtered chunk stored as more than its pipeline could make of
/// `chunk_bytes`, before it is planned or read (#997, #1016).
///
/// A filtered chunk is read whole at the size its index states, up to 4 GiB
/// in a B-tree key and more in a single-chunk layout, and nothing else bounds
/// that read but the size a ranged source declares. The chunk has passed
/// [`check_chunk_size`], so it decodes to at most `MAX_DECOMPRESSED_CHUNK`,
/// and its stored size is held to what the filters that ran on it could grow
/// that to ([`FilterPipeline::stored_limit`]). At the ceiling a few bytes
/// matter: a 256 MiB chunk with fletcher32 is stored as 256 MiB and 4 bytes,
/// and libhdf5 reads it. Past the limit it is too large rather than corrupt,
/// as the decoded refusal is, and the error says it is stored that size
/// ([`ArrayError::StoredChunkTooLarge`]). The size stays a `u64` to the
/// check, so a 32-bit target refuses it the same way.
///
/// The caller checks first that the chunk lies inside the file: one that runs
/// past the end of a buffer is corrupt, whatever its size (#1017).
pub(super) fn check_stored_chunk_size(
    pipeline: &FilterPipeline,
    filter_mask: u32,
    chunk_bytes: usize,
    stored: u64,
) -> Result<(), FieldglassError> {
    let limit = pipeline.stored_limit(
        filter_mask,
        chunk_bytes as u64,
        MAX_DECOMPRESSED_CHUNK as u64,
    );
    if stored > limit {
        return Err(ArrayError::StoredChunkTooLarge {
            bytes: stored,
            limit: MAX_DECOMPRESSED_CHUNK as u64,
        }
        .into());
    }
    Ok(())
}

/// [`ArrayError::ChunkTooLarge`] for a chunk of `bytes` decoded.
pub(super) fn chunk_too_large(bytes: u64) -> FieldglassError {
    ArrayError::ChunkTooLarge {
        bytes,
        limit: MAX_DECOMPRESSED_CHUNK as u64,
    }
    .into()
}

/// The most an szip size prefix may declare before it is allocated.
///
/// When the exact length is known the prefix must equal it, which `unszip`
/// checks next, so the ceiling is only a backstop: [`FilterPipeline::reverse`]
/// has already refused an exact length past it (#970). When it is not, it is
/// `behind`, from [`FilterPipeline::limit_behind`].
fn szip_limit(exact: Option<usize>, behind: usize) -> usize {
    match exact {
        Some(_) => MAX_DECOMPRESSED_CHUNK,
        None => behind,
    }
}

/// The most deflate or zstd may decompress one chunk to (#813): the chunk's
/// own length when every filter before the codec keeps it, and `behind`, from
/// [`FilterPipeline::limit_behind`], when one changes it.
///
/// [`MAX_DECOMPRESSED_CHUNK`] alone let a 12-byte chunk inflate to 256 MiB
/// before the caller's length check refused it, and on wasm that is a real
/// allocation, not a lazily mapped one. The chunk's own length is never past
/// it: [`FilterPipeline::reverse`] refuses that chunk before any codec runs
/// (#970), where it used to inflate 256 MiB and fail as a corrupt chunk.
/// `behind` may pass it by the growth of the filters ahead of the codec, up to
/// twice it, so a chunk at the ceiling behind fletcher32 still decodes (#1016).
fn codec_limit(exact: Option<usize>, behind: usize) -> Limit {
    match exact {
        Some(len) => Limit::Length(len),
        None => Limit::Ceiling(behind),
    }
}

/// How far a bounded codec may decompress one chunk, and why, so a chunk that
/// runs past it says which bound it broke.
#[derive(Clone, Copy, Debug)]
enum Limit {
    /// The chunk's own length, known because every filter before the codec
    /// keeps it. Running past it is a corrupt chunk.
    Length(usize),
    /// A ceiling on a length nothing outside the stream records.
    Ceiling(usize),
}

impl Limit {
    fn bytes(self) -> usize {
        match self {
            Limit::Length(n) | Limit::Ceiling(n) => n,
        }
    }

    /// How large a codec's output buffer starts, for `input_len` bytes of
    /// stream that can decode to at most `most(input_len)` bytes.
    ///
    /// The chunk's own length when it is known: a valid chunk comes out
    /// exactly that long, so one zeroed allocation holds it and nothing is
    /// copied. Growing toward it instead would hold the old buffer and the
    /// new one at once on each step, up to about twice the chunk for a moment
    /// (#981). But that length comes from the chunk's dimensions, not the
    /// stream, so it is bounded by what the stream could fill: a 12-byte
    /// stream in a chunk stating 256 MiB had all 256 MiB zeroed before the
    /// length check refused it (#986). A valid chunk is never past `most`, so
    /// it still starts at its length. A ceiling is not a length, so there the
    /// first guess is `miniz_oxide`'s own, twice the input, and the buffer
    /// doubles from it.
    fn first_guess(self, input_len: usize, most: fn(usize) -> usize) -> usize {
        match self {
            Limit::Length(n) => n.min(most(input_len)),
            Limit::Ceiling(_) => input_len.saturating_mul(2),
        }
    }

    /// The error for a `codec` stream that decompresses past this limit.
    fn exceeded(self, codec: &str) -> FieldglassError {
        FieldglassError::Parse(match self {
            Limit::Length(n) => {
                format!("{codec} chunk decompresses past the chunk's {n}-byte length")
            }
            Limit::Ceiling(n) => format!("{codec} chunk decompresses past the {n}-byte ceiling"),
        })
    }
}

/// Fixed headroom in [`szip_growth`] for the 4-byte size prefix and the last
/// byte's padding.
const SZIP_SLACK: usize = 4096;

/// The most an szip filter's output may be when it codes `len` bytes: the
/// longest stream its parameters allow from an encoder that chooses each
/// block's option as libaec's does ([`SzParams::max_compressed_len`]), and
/// never more than [`SZIP_MAX_GROWTH`] times `len`, plus [`SZIP_SLACK`] for
/// the size prefix and the last byte's padding.
///
/// The parameters make the bound tight. libhdf5 never picks a scanline
/// shorter than a block (`set_local`), so padding at most doubles one, and a
/// block costs at most 3 + 32·8 bits for 32 bytes: a real chunk's stream is
/// at most about twice the chunk, and about 1.01 times it when the scanline
/// is a whole number of blocks. A codec before szip may inflate only that
/// far, not 33 times (#975). The factor of
/// 33 still caps a block of over 32 pixels with scanlines shorter than it,
/// which ADR-0012 decision 6 keeps refused behind a codec. Parameters the
/// codec refuses never reach here: [`FilterPipeline::reverse`] refuses them
/// first.
fn szip_growth(cd_values: &[u32], len: usize) -> usize {
    let worst = len.saturating_mul(SZIP_MAX_GROWTH);
    szip_params(cd_values)
        .map_or(worst, |p| p.max_compressed_len(len).min(worst))
        .saturating_add(SZIP_SLACK)
}

/// The most an szip stream can be, as a multiple of the bytes it codes,
/// whatever its parameters. [`szip_growth`] takes the tighter of this and
/// what the filter's own parameters allow.
///
/// libsz takes the shortest option for a block that is not all zeros, so
/// none of those is longer than the uncompressed one: an ID of at most 5 bits
/// plus `J·n` bits for `J` pixels of `n` bits. A one-block zero run with a
/// reference sample is `ID + n + 2` bits, longer only at 1-bit pixels in
/// blocks of 2, where it is still far under this factor (CCSDS 121.0-B §5.1;
/// libsz never sets the restricted
/// option, so the ID is 3 bits up to 8 bits per pixel, 4 to 16, 5 beyond:
/// `fieldglass-aec`'s `decode.rs`). The standard leaves that choice to the
/// encoder, so this bounds encoders that choose the way libsz does; ADR-0012
/// decision 6 records the rest. libsz pads every scanline to whole
/// blocks, so the worst case is one pixel per scanline coded as a whole block:
/// `J·n + ID` bits for an input pixel of `8·⌈n/8⌉` bits. With `J ≤ 32` that is
/// at most `(32·8 + 3) / 8 ≈ 32.4` (8-bit pixels), which this rounds up. The
/// 4-byte size prefix and the last byte's padding are in [`SZIP_SLACK`].
///
/// `J ≤ 32` is libhdf5's: `H5Pset_szip` refuses a larger block, and its
/// `set_local` never picks a scanline shorter than a block, which keeps real
/// files to about twice their input. `fieldglass_aec::sz` decodes blocks of
/// up to 256 pixels, for other writers, so a file from one of those with
/// blocks over 32 and scanlines shorter than a block could exceed this, and
/// a codec after its szip would then be refused. No writer we know of makes
/// one; ADR-0012 decision 6 records it. `hdf5_szip_growth.h5` pins a libsz
/// stream 26 times its chunk, the most libsz produced in a sweep.
const SZIP_MAX_GROWTH: usize = 33;

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
///   still a whole number of those (ADR-0012 decision D2);
/// - at most what the stream can code
///   ([`SzParams::max_decompressed_len`]), so a short stream declaring a
///   large chunk allocates nothing for it (#975).
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
    let params = szip_params(cd_values)?;
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
             at {} bits per pixel",
            params.bits_per_pixel()
        )));
    }
    // The prefix is the file's number, not the stream's: a few bytes of
    // stream can declare a chunk of up to 256 MiB. No stream codes more than
    // its length allows (CCSDS 121.0-B-3 Tables 3-2 and 5-1), so a longer
    // one is refused before its buffer is allocated (#975).
    let most = params.max_decompressed_len(stream.len());
    if len > most {
        return Err(FieldglassError::Parse(format!(
            "szip chunk declares {len} bytes, but its {}-byte stream can decode to at most {most} bytes",
            stream.len()
        )));
    }

    let mut out = vec![0u8; len];
    sz::decompress(stream, &params, &mut out).map_err(szip_error)?;
    Ok(out)
}

/// An szip filter's parameters from its `cd_values`, which are in libhdf5's
/// order `(mask, pixels per block, bits per pixel, pixels per scanline)`:
/// [`SzParams`] takes libsz's, with the middle two swapped.
fn szip_params(cd_values: &[u32]) -> Result<SzParams, FieldglassError> {
    let &[mask, ppb, bpp, pps] = cd_values else {
        return Err(FieldglassError::Parse(format!(
            "szip filter carries {} client data values, expected {SZIP_CD_VALUES} \
             (mask, pixels per block, bits per pixel, pixels per scanline)",
            cd_values.len()
        )));
    };
    SzParams::new(mask, bpp, ppb, pps).map_err(szip_error)
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

/// Inflate a zlib stream (the HDF5 deflate filter's on-disk form) to at most
/// `limit` bytes, which [`codec_limit`] sets per chunk.
///
/// The unbounded `decompress_to_vec_zlib` would let a crafted chunk name its
/// own allocation size. `miniz_oxide`'s bounded `_with_limit` form caps what
/// it writes but grows its buffer with `Vec::resize`, whose amortized growth
/// can reserve nearly twice the limit on the last step (#981). So the loop is
/// ours, around the same decoder with the same flags, and grows the buffer
/// with [`output_limits::grow_toward`]: nothing it allocates is larger than `limit`.
fn inflate_bounded(data: &[u8], limit: Limit) -> Result<Vec<u8>, FieldglassError> {
    use miniz_oxide::inflate::TINFLStatus;
    use miniz_oxide::inflate::core::{DecompressorOxide, decompress, inflate_flags};

    // The output is one slice that never wraps, so a back-reference can reach
    // anything already written.
    let flags = inflate_flags::TINFL_FLAG_PARSE_ZLIB_HEADER
        | inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF;
    let hint = limit.first_guess(data.len(), output_limits::inflate_bound);
    let mut out = Vec::new();
    // A zero limit leaves the buffer empty, which only an empty stream fits.
    output_limits::grow_toward(&mut out, hint, limit.bytes());
    let mut state = Box::<DecompressorOxide>::default();
    let mut input = data;
    let mut written = 0;
    loop {
        let (status, consumed, produced) = decompress(&mut state, input, &mut out, written, flags);
        written += produced;
        input = input.get(consumed..).unwrap_or_default();
        match status {
            TINFLStatus::Done => {
                out.truncate(written);
                return Ok(out);
            }
            TINFLStatus::HasMoreOutput => {
                if !output_limits::grow_toward(&mut out, hint, limit.bytes()) {
                    return Err(limit.exceeded("deflate"));
                }
            }
            // Only the status: the partial output that `DecompressError`'s
            // `Debug` used to print could put megabytes into the message.
            failed => {
                return Err(FieldglassError::Parse(format!(
                    "deflate (zlib) inflate failed: {failed:?}"
                )));
            }
        }
    }
}

/// Decompress a zstd frame (the HDF5 zstd filter's on-disk form) to at most
/// `limit` bytes, which [`codec_limit`] sets per chunk.
///
/// Streamed rather than decoded in one shot so the output can be capped, into
/// a buffer [`output_limits::read_to_end_within`] grows: `read_to_end`'s amortized
/// growth could reserve nearly twice the limit (#981).
fn unzstd_bounded(data: &[u8], limit: Limit) -> Result<Vec<u8>, FieldglassError> {
    // Two ceilings, because a crafted frame has two ways to ask for memory.
    // The window bounds the buffer sized from the frame header, which the
    // decoder fills before it hands back any output and so is invisible to
    // the output bound; `limit` bounds the output itself. The window follows
    // the limit, between 8 MiB and [`MAX_ZSTD_WINDOW`], so a frame cannot make
    // the decoder hold far more than the chunk (#989).
    let window = output_limits::zstd_window_limit(limit.bytes(), MAX_ZSTD_WINDOW);
    let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(data, window)
        .map_err(|e| match e {
            FrameDecoderError::WindowSizeTooBig { requested, .. } => {
                // A single-segment frame's window is its content size, so one
                // past the limit decodes past it: the chunk the output bound
                // refuses, refused before it is decoded.
                if output_limits::zstd_single_segment(data)
                    && requested > u64::try_from(limit.bytes()).unwrap_or(u64::MAX)
                {
                    limit.exceeded("zstd")
                } else {
                    ArrayError::WindowTooLarge {
                        codec: "zstd",
                        window: requested,
                        limit: window,
                    }
                    .into()
                }
            }
            e => FieldglassError::Parse(format!("zstd frame header: {e}")),
        })?;
    let hint = limit.first_guess(data.len(), output_limits::unzstd_bound);
    output_limits::read_to_end_within(decoder, hint, limit.bytes())
        .map_err(|e| FieldglassError::Parse(format!("zstd decompress failed: {e}")))?
        .ok_or_else(|| limit.exceeded("zstd"))
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
    /// honors that generally, and this pins it for the filter whose whole job
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

        assert_eq!(
            unzstd_bounded(&frame, Limit::Ceiling(big.len())).unwrap(),
            big
        );
        let err = unzstd_bounded(&frame, Limit::Ceiling(64)).unwrap_err();
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

        assert_eq!(
            inflate_bounded(&stream, Limit::Ceiling(big.len())).unwrap(),
            big
        );
        assert!(inflate_bounded(&stream, Limit::Ceiling(64)).is_err());
    }

    /// The loop around `miniz_oxide`'s core decoder gives what its own
    /// `decompress_to_vec_zlib` gives, at every limit from the exact length
    /// up, and the buffer never holds more than the limit (#981). The
    /// payloads cover a stored block, a run that expands a thousandfold, and
    /// data that barely compresses, at lengths either side of each doubling.
    #[test]
    fn inflate_matches_miniz_and_reserves_within_the_limit() {
        let mut noise = 0x2545_F491_u32;
        let mut payloads: Vec<Vec<u8>> = vec![Vec::new(), vec![7], vec![0; 1 << 20]];
        for len in [1_000, 4_095, 4_097, 65_537, 300_001] {
            payloads.push(
                (0..len)
                    .map(|_| {
                        noise ^= noise << 13;
                        noise ^= noise >> 17;
                        noise ^= noise << 5;
                        (noise % 7) as u8
                    })
                    .collect(),
            );
        }
        for payload in &payloads {
            for level in [0, 6, 10] {
                let stream = miniz_oxide::deflate::compress_to_vec_zlib(payload, level);
                let reference = miniz_oxide::inflate::decompress_to_vec_zlib(&stream).unwrap();
                assert_eq!(&reference, payload);
                for limit in [payload.len(), payload.len() + 1, payload.len() * 3 + 5] {
                    let got = inflate_bounded(&stream, Limit::Length(limit)).unwrap();
                    assert_eq!(got, reference, "{} bytes at level {level}", payload.len());
                    assert!(got.capacity() <= limit, "capacity {}", got.capacity());
                }
                if let Some(short) = payload.len().checked_sub(1) {
                    let err = inflate_bounded(&stream, Limit::Length(short)).unwrap_err();
                    assert!(
                        matches!(&err, FieldglassError::Parse(m) if m.contains("past the chunk's")),
                        "{err:?}"
                    );
                }
            }
        }
    }

    /// A corrupt stream names the decoder's status and nothing more. Before
    /// #981 the message was `DecompressError`'s `Debug`, which printed every
    /// byte inflated before the failure.
    #[test]
    fn a_corrupt_deflate_stream_reports_its_status_not_its_output() {
        let mut stream = miniz_oxide::deflate::compress_to_vec_zlib(&[0u8; 100_000], 6);
        let at = stream.len() - 2;
        stream[at] ^= 0xFF; // inside the Adler-32 trailer
        let err = inflate_bounded(&stream, Limit::Length(100_000)).unwrap_err();
        let FieldglassError::Parse(m) = err else {
            panic!("{err:?}")
        };
        assert_eq!(m, "deflate (zlib) inflate failed: Adler32Mismatch");
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
        let err = unzstd_bounded(&frame, Limit::Ceiling(MAX_DECOMPRESSED_CHUNK)).unwrap_err();
        assert!(
            matches!(
                &err,
                FieldglassError::Array(ArrayError::WindowTooLarge {
                    window: 0x200_0000_0000,
                    ..
                })
            ),
            "expected the header to be rejected, got {err:?}"
        );

        // The ceiling is *ours*, not whatever the decoder defaults to. This
        // frame is otherwise complete and valid — same header, then one raw
        // block of four bytes — and declares a 72 MiB window, inside `ruzstd`'s
        // 128 MiB default but outside our 64 MiB one, which is all a chunk past
        // 64 MiB gets. Under the default it decodes to `AA BB CC DD`; it fails
        // here only because we set the bound, so a future bump that widened
        // the upstream default could not pass this test unnoticed.
        let err =
            unzstd_bounded(&windowed(0x81), Limit::Ceiling(MAX_DECOMPRESSED_CHUNK)).unwrap_err();
        assert!(
            matches!(
                &err,
                FieldglassError::Array(ArrayError::WindowTooLarge {
                    codec: "zstd",
                    window: 0x480_0000,
                    limit: MAX_ZSTD_WINDOW,
                })
            ),
            "expected our window bound to refuse this frame, got {err:?}"
        );

        let ok = ruzstd::encoding::compress_to_vec(
            [0u8; 128].as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        );
        assert!(
            unzstd_bounded(&ok, Limit::Ceiling(MAX_DECOMPRESSED_CHUNK)).is_ok(),
            "an ordinary frame must still decode"
        );
    }

    /// A valid frame of one raw 4-byte block, `AA BB CC DD`, behind a frame
    /// header stating no content size and the window `descriptor`
    /// (`exponent << 3 | mantissa`, RFC 8878 section 3.1.1.1.2).
    fn windowed(descriptor: u8) -> [u8; 13] {
        [
            0x28, 0xB5, 0x2F, 0xFD,       // magic
            0x00,       // frame header descriptor: no content size, no dictionary
            descriptor, // window descriptor
            0x21, 0x00, 0x00, // block header: last block, raw, 4 bytes
            0xAA, 0xBB, 0xCC, 0xDD,
        ]
    }

    /// The window follows the chunk (#989): a 4-byte chunk accepts a frame
    /// stating 8 MiB, the RFC's recommended minimum, and refuses one stating
    /// 9 MiB as a window too large, which says the file is fine. The chunk's
    /// own length and a ceiling behind another filter bound it alike.
    #[test]
    fn a_small_chunk_takes_an_8_mib_window_and_no_more() {
        for limit in [Limit::Length(4), Limit::Ceiling(4)] {
            // Exponent 13: 8 MiB, and with mantissa 1, 9 MiB.
            assert_eq!(
                unzstd_bounded(&windowed(0x68), limit).unwrap(),
                [0xAA, 0xBB, 0xCC, 0xDD]
            );
            let err = unzstd_bounded(&windowed(0x69), limit).unwrap_err();
            assert!(
                matches!(
                    &err,
                    FieldglassError::Array(ArrayError::WindowTooLarge {
                        window: 0x90_0000,
                        limit: 0x80_0000,
                        ..
                    })
                ),
                "got {err:?}"
            );
        }
        // A ceiling of 9 MiB takes it.
        assert!(unzstd_bounded(&windowed(0x69), Limit::Ceiling(9 << 20)).is_ok());
    }

    /// A single-segment frame's window is its content size, so one stating
    /// more than the chunk is the chunk the output bound refuses, refused from
    /// its header with the same error.
    #[test]
    fn a_single_segment_frame_past_the_chunk_is_refused_as_past_it() {
        let frame = [
            0x28u8, 0xB5, 0x2F, 0xFD, // magic
            0xA0, // single-segment, 4-byte content size
            0x00, 0x00, 0x00, 0x01, // Frame_Content_Size: 16 MiB
            0x21, 0x00, 0x00, // block header: last block, raw, 4 bytes
            0xAA, 0xBB, 0xCC, 0xDD,
        ];
        let err = unzstd_bounded(&frame, Limit::Length(4096)).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m)
                if m == "zstd chunk decompresses past the chunk's 4096-byte length"),
            "got {err:?}"
        );
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
        let mut filters: Vec<Filter> = before.iter().map(|&id| filter_of(id)).collect();
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
    /// length grown by that filter, here fletcher32's 4 bytes, not by the
    /// 256 MiB ceiling: a prefix between the two is refused before it is
    /// allocated. At the bound itself the prefix passes that check, and the
    /// stream's own length refuses it next: 8 bytes of stream code at most
    /// 455 pixels here (#975).
    #[test]
    fn szip_behind_a_length_changing_filter_is_bounded_by_the_chunk() {
        let expected = 1024;
        let bound = expected + 4;
        assert_eq!(szip_limit(None, bound), bound);
        assert_eq!(szip_limit(Some(expected), bound), MAX_DECOMPRESSED_CHUNK);

        let pipeline = szip_pipeline(&[FILTER_FLETCHER32], &SZ_CD);
        let over = u32::try_from(bound + 1).unwrap();
        let m = parse_message(pipeline.reverse(szip_chunk(over), 0, 1, expected));
        assert!(m.contains(&format!("past the {bound}-byte ceiling")), "{m}");
        let at = u32::try_from(bound).unwrap();
        let m = parse_message(pipeline.reverse(szip_chunk(at), 0, 1, expected));
        assert!(
            m.contains("but its 8-byte stream can decode to at most 455 bytes"),
            "{m}"
        );
    }

    /// The prefix is held to what the stream can code before anything is
    /// allocated for it (#975): `SZ_STREAM`'s 8 bytes are 64 bits, at most
    /// `⌊64·64 / 9⌋ = 455` blocks of 2 with one pixel each, so 455 bytes.
    /// That is the most it may state, with or without the length known.
    #[test]
    fn szip_prefix_is_held_to_what_the_stream_can_code() {
        let most = SzParams::new(0, 8, 2, 1)
            .unwrap()
            .max_decompressed_len(SZ_STREAM.len());
        assert_eq!(most, 455);
        for exact in [None, Some(456)] {
            let m = parse_message(unszip(&szip_chunk(456), &SZ_CD, exact, 4096));
            assert!(
                m.contains(
                    "declares 456 bytes, but its 8-byte stream can decode to at most 455 bytes"
                ),
                "{exact:?}: {m}"
            );
        }
        // At the bound the prefix passes, and the codec finds the stream short.
        let m = parse_message(unszip(&szip_chunk(455), &SZ_CD, Some(455), 4096));
        assert!(m.contains("szip decompress failed"), "{m}");
        // Behind shuffle, 256 MiB stated by a short stream is refused the
        // same way, not zeroed first.
        let ceiling = u32::try_from(MAX_DECOMPRESSED_CHUNK).unwrap();
        let m = parse_message(szip_pipeline(&[FILTER_SHUFFLE], &SZ_CD).reverse(
            szip_chunk(ceiling),
            0,
            1,
            MAX_DECOMPRESSED_CHUNK,
        ));
        assert!(m.contains("can decode to at most 455 bytes"), "{m}");
    }

    /// The szip `cd_values` libhdf5 wrote for `hdf5_szip_hostile.h5`'s
    /// `uint8` chunk of `(2048, 2048)`: EC, blocks of 32, 8 bits, scanlines of
    /// 2048. Its scanlines are whole blocks, as libhdf5's always are.
    const HDF5_SZ_CD: [u32; 4] = [141, 32, 8, 2048];

    /// A filter of `id` with one client value, or [`HDF5_SZ_CD`] for szip.
    fn filter_of(id: u16) -> Filter {
        let client_data = if id == FILTER_SZIP {
            HDF5_SZ_CD.to_vec()
        } else {
            vec![1]
        };
        Filter { id, client_data }
    }

    /// A pipeline of `ids` in write order, built by [`filter_of`].
    fn pipeline_of(ids: &[u16]) -> FilterPipeline {
        FilterPipeline {
            filters: ids.iter().map(|&id| filter_of(id)).collect(),
        }
    }

    /// A filtered chunk's stored size may pass the decoded ceiling by what
    /// the filters that ran on it add, and no further (#1016).
    #[test]
    fn a_stored_chunk_may_pass_the_ceiling_by_its_pipelines_growth() {
        let max = MAX_DECOMPRESSED_CHUNK as u64;
        let limit = |ids: &[u16], mask: u32, decoded: u64| {
            pipeline_of(ids).stored_limit(mask, decoded, max)
        };
        // fletcher32 adds exactly its checksum.
        assert_eq!(limit(&[FILTER_FLETCHER32], 0, max), max + 4);
        // shuffle adds nothing, so the limit is the ceiling #997 set.
        assert_eq!(limit(&[FILTER_SHUFFLE], 0, max), max);
        // deflate: zlib's fixed-block `deflateBound` and its wrapper, which
        // holds the 20,491 bytes zlib's stored blocks add at the ceiling.
        let deflate = max + max / 8 + max / 256 + max / 512 + 10;
        assert_eq!(limit(&[FILTER_DEFLATE], 0, max), deflate);
        assert!(deflate > max + 20_491);
        // zstd: `ZSTD_COMPRESSBOUND`.
        assert_eq!(limit(&[FILTER_ZSTD], 0, max), max + max / 256);
        // Growth folds in write order: fletcher32 after deflate grows
        // deflate's bound by its 4 bytes.
        assert_eq!(
            limit(&[FILTER_SHUFFLE, FILTER_DEFLATE, FILTER_FLETCHER32], 0, max),
            deflate + 4
        );
        // A filter its mask skipped adds nothing.
        assert_eq!(
            limit(&[FILTER_DEFLATE, FILTER_FLETCHER32], 0b01, max),
            max + 4
        );
        // A small chunk keeps the ceiling, as before: nothing #997 read is
        // refused now.
        assert_eq!(limit(&[FILTER_FLETCHER32], 0, 64), max);
        // Never past twice the ceiling, however a pipeline could grow: an
        // szip of 8-bit pixels one to a scanline in blocks of 32 is about 32
        // times its input.
        let mut wide = pipeline_of(&[FILTER_SZIP]);
        wide.filters[0].client_data = vec![0, 32, 8, 1];
        assert_eq!(wide.stored_limit(0, max, max), 2 * max);
        // libhdf5's own szip at a whole number of blocks a scanline fits.
        let szip = limit(&[FILTER_SZIP], 0, max);
        assert!(szip > max && szip < max + max / 50, "{szip}");
    }

    /// The stored-size check reads its limit from the pipeline, and reports
    /// the decoded ceiling as the bound the file asked too much of.
    #[test]
    fn check_stored_chunk_size_allows_fletcher32_at_the_ceiling() {
        let pipeline = pipeline_of(&[FILTER_FLETCHER32]);
        let max = MAX_DECOMPRESSED_CHUNK;
        check_stored_chunk_size(&pipeline, 0, max, max as u64 + 4).unwrap();
        let err = check_stored_chunk_size(&pipeline, 0, max, max as u64 + 5).unwrap_err();
        assert!(
            matches!(
                err,
                FieldglassError::Array(ArrayError::StoredChunkTooLarge { bytes, limit })
                    if bytes == max as u64 + 5 && limit == max as u64
            ),
            "{err:?}"
        );
    }

    /// 1 MiB of zeros: a stream of a few hundred bytes whose output is far past
    /// any small chunk's length and far below the 256 MiB ceiling, which is the
    /// gap #813 closes.
    const BOMB_LEN: usize = 1 << 20;

    fn zlib_bomb() -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; BOMB_LEN], 6)
    }

    fn zstd_bomb() -> Vec<u8> {
        ruzstd::encoding::compress_to_vec(
            vec![0u8; BOMB_LEN].as_slice(),
            ruzstd::encoding::CompressionLevel::Fastest,
        )
    }

    /// Deflate and zstd stop at the chunk's own length when every filter
    /// before them keeps it (#813). The chunk is valid apart from inflating
    /// past that length; it used to come back at its full 1 MiB, and only the
    /// caller's length check refused it, after the allocation.
    #[test]
    fn deflate_and_zstd_stop_at_the_chunks_length() {
        for (name, id, stream) in [
            ("deflate", FILTER_DEFLATE, zlib_bomb()),
            ("zstd", FILTER_ZSTD, zstd_bomb()),
        ] {
            assert!(stream.len() < 4096, "{name}: the stream is small");
            for before in [&[][..], &[FILTER_SHUFFLE][..]] {
                let mut ids = before.to_vec();
                ids.push(id);
                let pipeline = pipeline_of(&ids);
                let m = parse_message(pipeline.reverse(stream.clone(), 0, 1, 64));
                assert!(
                    m.contains("chunk decompresses past the chunk's 64-byte length"),
                    "{name} behind {before:?}: {m}"
                );
                // The same stream is still decoded for a chunk of its length.
                let out = pipeline.reverse(stream.clone(), 0, 1, BOMB_LEN).unwrap();
                assert_eq!(out.len(), BOMB_LEN, "{name} behind {before:?}");
            }
        }
    }

    /// A chunk whose own length is past the ceiling is refused as too large
    /// before any codec runs, whichever codec and whatever comes before it
    /// (#970). deflate and zstd used to inflate to the ceiling and fail as a
    /// corrupt chunk; szip refused its prefix as past the ceiling. At the
    /// ceiling itself the filters run.
    #[test]
    fn a_chunk_past_the_ceiling_is_refused_before_any_codec_runs() {
        let over = MAX_DECOMPRESSED_CHUNK + 1;
        let over_prefix = u32::try_from(over).unwrap();
        for (name, ids, stream) in [
            ("deflate", vec![FILTER_DEFLATE], zlib_bomb()),
            ("zstd", vec![FILTER_ZSTD], zstd_bomb()),
            (
                "shuffle + deflate",
                vec![FILTER_SHUFFLE, FILTER_DEFLATE],
                zlib_bomb(),
            ),
            (
                "deflate + fletcher32",
                vec![FILTER_DEFLATE, FILTER_FLETCHER32],
                zlib_bomb(),
            ),
        ] {
            let err = pipeline_of(&ids)
                .reverse(stream.clone(), 0, 1, over)
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    FieldglassError::Array(ArrayError::ChunkTooLarge { bytes, limit })
                        if bytes == over as u64 && limit == MAX_DECOMPRESSED_CHUNK as u64
                ),
                "{name}: {err:?}"
            );
            // At the ceiling the filters run, and what they make of the
            // stream is the caller's to check.
            let at = pipeline_of(&ids).reverse(stream, 0, 1, MAX_DECOMPRESSED_CHUNK);
            assert!(
                !matches!(at, Err(FieldglassError::Array(_))),
                "{name}: {:?}",
                at.map(|v| v.len())
            );
        }
        // A chunk every filter skipped passes through as stored: nothing is
        // inflated, so its size is the file's, not the reader's, to bound.
        let raw = vec![7u8; 16];
        let skipped =
            pipeline_of(&[FILTER_SHUFFLE, FILTER_DEFLATE]).reverse(raw.clone(), 0b11, 1, over);
        assert_eq!(skipped.unwrap(), raw);
        // szip with its prefix set to the chunk's length, so only the size
        // could refuse it, behind nothing and behind shuffle.
        for before in [&[][..], &[FILTER_SHUFFLE][..]] {
            let err = szip_pipeline(before, &SZ_CD)
                .reverse(szip_chunk(over_prefix), 0, 1, over)
                .unwrap_err();
            assert!(
                matches!(
                    err,
                    FieldglassError::Array(ArrayError::ChunkTooLarge { .. })
                ),
                "szip behind {before:?}: {err:?}"
            );
        }
    }

    /// The up-front check's two bounds are each inclusive: a chunk of exactly
    /// `MAX_FIELD_POINTS` values, or exactly the ceiling in bytes, passes, and
    /// one more of either is refused, each with its own error (#970).
    #[test]
    fn the_chunk_size_check_is_exact_on_both_bounds() {
        let most = fieldglass_core::MAX_FIELD_POINTS as u64;
        assert!(check_chunk_size(&[most], MAX_DECOMPRESSED_CHUNK).is_ok());
        assert!(check_chunk_size(&[1 << 13, 1 << 13], 1 << 28).is_ok());
        assert!(matches!(
            check_chunk_size(&[most + 1], 0),
            Err(FieldglassError::Array(ArrayError::FieldTooLarge { elements, .. }))
                if elements == most + 1
        ));
        // Saturating, so a product past `u64` is refused rather than wrapped.
        assert!(matches!(
            check_chunk_size(&[u64::MAX, 2], 0),
            Err(FieldglassError::Array(ArrayError::FieldTooLarge { .. }))
        ));
        assert!(matches!(
            check_chunk_size(&[1], MAX_DECOMPRESSED_CHUNK + 1),
            Err(FieldglassError::Array(ArrayError::ChunkTooLarge { bytes, .. }))
                if bytes == MAX_DECOMPRESSED_CHUNK as u64 + 1
        ));
        // Eight-byte elements meet the byte bound first: 40 Mi values is
        // inside the field cap and 320 MiB.
        assert!(matches!(
            check_chunk_size(&[5120, 8192], 5120 * 8192 * 8),
            Err(FieldglassError::Array(ArrayError::ChunkTooLarge { .. }))
        ));
    }

    /// Behind a filter that changes the length, the codec's output is bounded
    /// the way szip's prefix is: the chunk's length grown by that filter's
    /// worst case, 4 bytes for fletcher32, 82 for deflate and 127 for zstd on
    /// a 64-byte chunk.
    #[test]
    fn deflate_and_zstd_behind_a_length_changing_filter_are_bounded_by_the_chunk() {
        let expected = 64;
        for (name, id, stream) in [
            ("deflate", FILTER_DEFLATE, zlib_bomb()),
            ("zstd", FILTER_ZSTD, zstd_bomb()),
        ] {
            for (before, bound) in [
                (FILTER_FLETCHER32, 68),
                (FILTER_DEFLATE, 82),
                (FILTER_ZSTD, 127),
            ] {
                let pipeline = pipeline_of(&[before, id]);
                let m = parse_message(pipeline.reverse(stream.clone(), 0, 1, expected));
                assert!(
                    m.contains(&format!("past the {bound}-byte ceiling")),
                    "{name} behind {before}: {m}"
                );
            }
            // With the filter before it skipped for this chunk, the length is
            // known again.
            let m = parse_message(pipeline_of(&[FILTER_FLETCHER32, id]).reverse(
                stream.clone(),
                0b1,
                1,
                expected,
            ));
            assert!(
                m.contains("chunk decompresses past the chunk's 64-byte length"),
                "{name}: {m}"
            );
        }
    }

    /// An szip earlier in the pipeline does not lift the bound to 256 MiB:
    /// the pipeline is the file's to choose, so a codec behind szip that did
    /// would reopen #813 for anyone who puts one there. Review of #832 found
    /// both of these taking the full ceiling.
    #[test]
    fn an_szip_ahead_does_not_reopen_the_ceiling() {
        let bound = szip_growth(&HDF5_SZ_CD, 64);
        // A deflate bomb behind szip stops at szip's worst-case growth, not
        // after inflating 1 MiB.
        let m = parse_message(pipeline_of(&[FILTER_SZIP, FILTER_DEFLATE]).reverse(
            zlib_bomb(),
            0,
            1,
            64,
        ));
        assert!(m.contains(&format!("past the {bound}-byte ceiling")), "{m}");
        // An outer szip prefix of 200 MiB behind another szip is refused
        // before it is allocated, as it was before #813.
        let m = parse_message(szip_pipeline(&[FILTER_SZIP], &SZ_CD).reverse(
            szip_chunk(200 << 20),
            0,
            1,
            64,
        ));
        assert!(m.contains(&format!("past the {bound}-byte ceiling")), "{m}");
    }

    /// The bound behind length-changing filters is their worst-case growth
    /// ([`filter_growth`]) chained in write order; a filter skipped for this
    /// chunk does not count.
    #[test]
    fn the_bound_behind_filters_chains_their_worst_growth() {
        let szip = |len: usize| szip_growth(&HDF5_SZ_CD, len);
        // deflate's bound on 64 bytes, then zstd's on that.
        let (deflate, zstd) = (82, 82 + (128 * 1024 - 82) / 2048);
        for (ids, mask, want) in [
            (&[FILTER_SZIP, FILTER_DEFLATE][..], 0, szip(64)),
            (&[FILTER_SZIP, FILTER_ZSTD][..], 0, szip(64)),
            (&[FILTER_SZIP, FILTER_SZIP][..], 0, szip(64)),
            (
                &[FILTER_FLETCHER32, FILTER_SZIP, FILTER_DEFLATE][..],
                0,
                szip(68),
            ),
            (
                &[FILTER_SHUFFLE, FILTER_SZIP, FILTER_DEFLATE][..],
                0,
                szip(64),
            ),
            (&[FILTER_SZIP, FILTER_DEFLATE][..], 0b1, 64),
            (&[FILTER_FLETCHER32, FILTER_DEFLATE][..], 0, 68),
            (&[FILTER_DEFLATE, FILTER_ZSTD, FILTER_SZIP][..], 0, zstd),
            (&[FILTER_DEFLATE, FILTER_SZIP][..], 0, deflate),
        ] {
            let pipeline = pipeline_of(ids);
            let last = ids.len() - 1;
            assert_eq!(
                pipeline.limit_behind(last, mask, 64),
                want,
                "{ids:?} mask {mask:#b}"
            );
        }
        // Saturates at twice the ceiling rather than overflowing.
        let deep = pipeline_of(&[FILTER_SZIP; 8]);
        assert_eq!(
            deep.limit_behind(7, 0, usize::MAX / 2),
            2 * MAX_DECOMPRESSED_CHUNK
        );
    }

    /// A chunk of exactly the ceiling under fletcher32, shuffle and deflate,
    /// the order netCDF-C writes (fletcher32 first, `NC4_hdf5_addfilter`),
    /// inflates to the ceiling and 4 bytes, so deflate's bound is that, not
    /// the bare ceiling, which refused it (#1016).
    #[test]
    fn deflate_behind_fletcher32_at_the_ceiling_may_pass_it_by_four_bytes() {
        let max = MAX_DECOMPRESSED_CHUNK;
        let pipeline = pipeline_of(&[FILTER_FLETCHER32, FILTER_SHUFFLE, FILTER_DEFLATE]);
        let behind = pipeline.limit_behind(2, 0, max);
        assert_eq!(behind, max + 4);
        assert!(matches!(
            codec_limit(pipeline.length_before(2, 0, max), behind),
            Limit::Ceiling(n) if n == max + 4
        ));
        // Past it, by what the stream holds, is still refused.
        let m = parse_message(inflate_bounded(&zlib_bomb(), Limit::Ceiling(68)));
        assert!(m.contains("past the 68-byte ceiling"), "{m}");
    }

    /// The bound refuses only what is too long. A real fletcher32-then-deflate
    /// chunk, whose deflate output is four bytes longer than the chunk, still
    /// decodes.
    #[test]
    fn deflate_behind_fletcher32_still_decodes() {
        let body: Vec<u8> = (0..64u8).collect();
        let mut with_sum = body.clone();
        with_sum.extend_from_slice(&fletcher32(&body).to_le_bytes());
        let chunk = miniz_oxide::deflate::compress_to_vec_zlib(&with_sum, 6);
        let pipeline = pipeline_of(&[FILTER_FLETCHER32, FILTER_DEFLATE]);
        assert_eq!(pipeline.reverse(chunk, 0, 1, body.len()).unwrap(), body);
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

    /// szip's growth comes from its parameters (#975). With libhdf5's, whose
    /// scanlines are whole blocks, a block of 32 bytes is at most 3 + 256
    /// bits, so the stream is at most 259/256 of the chunk, and a codec
    /// before szip gets that, not 33 times it. An 8 MiB chunk used to let it
    /// inflate to the 256 MiB ceiling.
    #[test]
    fn szip_growth_follows_its_parameters() {
        let chunk = 8 << 20;
        assert_eq!(
            szip_growth(&HDF5_SZ_CD, chunk),
            chunk / 256 * 259 + SZIP_SLACK
        );
        let pipeline = pipeline_of(&[FILTER_SZIP, FILTER_DEFLATE]);
        assert_eq!(
            pipeline.limit_behind(1, 0, chunk),
            szip_growth(&HDF5_SZ_CD, chunk)
        );
        assert!(pipeline.limit_behind(1, 0, chunk) < 9 << 20);
        // 1-bit pixels in blocks of 2 with preprocessing: libaec writes a
        // one-block zero run as 3 + 1 + 1 + 1 bits, more than the 3 + 2 of
        // an uncompressed block, so 64 pixels may take 24 bytes, not 20.
        assert_eq!(szip_growth(&[32 | 128, 2, 1, 2], 64), 24 + SZIP_SLACK);
        // One pixel per scanline at blocks of 32: each 8-bit pixel is a whole
        // block, 259 bits, which is under 33 times.
        assert_eq!(
            szip_growth(&[0, 32, 8, 1], 64),
            (64 * 259_usize).div_ceil(8) + SZIP_SLACK
        );
        // Blocks of 256 with one pixel per scanline could be 256 times the
        // chunk; the factor of 33 still caps them (ADR-0012 decision 6).
        assert_eq!(
            szip_growth(&[0, 256, 8, 1], 64),
            64 * SZIP_MAX_GROWTH + SZIP_SLACK
        );
    }

    /// szip's parameters are checked before any filter runs, since they bound
    /// the codec before it: a set the codec does not take is unsupported,
    /// and the deflate bomb behind it is never inflated.
    #[test]
    fn szip_parameters_are_checked_before_the_codec_ahead_of_it() {
        let pipeline = FilterPipeline {
            filters: vec![
                Filter {
                    id: FILTER_SZIP,
                    client_data: vec![0, 7, 8, 1],
                },
                filter_of(FILTER_DEFLATE),
            ],
        };
        let err = pipeline.reverse(zlib_bomb(), 0, 1, 64).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::UnsupportedSection(m) if m.contains("szip")),
            "{err:?}"
        );
        // Skipped for this chunk, its parameters do not matter.
        let err = pipeline.reverse(zlib_bomb(), 0b1, 1, 64).unwrap_err();
        assert!(err.to_string().contains("64-byte length"), "{err:?}");
    }
}
