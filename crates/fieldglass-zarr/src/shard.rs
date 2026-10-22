//! Zarr v3's `sharding_indexed` codec: many chunks in one stored object.
//!
//! Sharding exists because the natural chunk for a *reader* — a few megabytes,
//! sized for a cache — is a terrible object for a store to hold millions of.
//! A shard is one object holding a regular grid of inner chunks plus an index
//! saying where each one starts and how long it is, so a host fetches one
//! object and can still range-read a single inner chunk out of it.
//!
//! Three properties of the index are load-bearing, and each has its own test:
//!
//! * **The index is the only statement of where a chunk is.** The specification
//!   says outright that the order of the chunk content within the shard is the
//!   writer's choice and that gaps between chunks are legal, so a decoder that
//!   inferred a position from the grid order would read the wrong bytes on a
//!   conforming shard.
//! * **The index has an entry for every inner chunk the grid could hold**,
//!   including ones that fall outside the array's own shape. Sizing it from the
//!   chunks that exist would read the index short.
//! * **An absent chunk is both fields set to `2^64 − 1`**, not a zero length.
//!   A zero-length chunk at offset zero is a legal thing to write.
//!
//! # Axis order
//!
//! A `transpose` before `sharding_indexed` hands the sharding codec each shard
//! in the transposed ("stored") axis order (#1343). [`ShardLayout`],
//! [`Sharding`], [`ShardIndex`] and the inner chunks [`Sharding::decode_inner`]
//! returns all work in that stored order: the inner shape, the shard's extent,
//! the inner grid and the C order of an inner chunk's bytes.
//! [`CodecChain::shard_order`] gives the permutation (`order[k]` is the array
//! axis that became stored axis `k`; `None` means the orders agree).
//!
//! The shard's extent is passed as a [`StoredShape`], not a bare shape (#1368).
//! [`StoredShape::new`] takes the array's own chunk shape and its codec chain
//! and does the permutation, so a caller cannot hand the layout the array's
//! order by mistake: a `&[usize]` does not compile there.
//!
//! ```
//! use fieldglass_zarr::{ChunkDecoder, StoredShape};
//! # fn main() -> Result<(), fieldglass_zarr::FieldglassError> {
//! # let meta = r#"{"zarr_format": 3, "node_type": "array", "shape": [6, 8],
//! #   "data_type": "float32", "fill_value": 0,
//! #   "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [4, 8]}},
//! #   "chunk_key_encoding": {"name": "default"},
//! #   "codecs": [
//! #     {"name": "transpose", "configuration": {"order": [1, 0]}},
//! #     {"name": "sharding_indexed", "configuration": {"chunk_shape": [4, 2],
//! #       "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
//! #       "index_codecs": [{"name": "bytes", "configuration": {"endian": "little"}},
//! #                        {"name": "crc32c"}]}}]}"#;
//! // An array of 4x8 chunks, transposed before it is sharded into 4x2 inner
//! // chunks: the sharding codec sees each shard as 8x4, a 2x2 inner grid.
//! let decoder = ChunkDecoder::from_v3_metadata(meta)?;
//! let layout = decoder.chain().sharding().expect("sharded").layout();
//! let shape = StoredShape::new(decoder.chunk_shape(), decoder.chain())?;
//! assert_eq!(shape.as_slice(), [8, 4]);
//! // Four inner chunks of sixteen bytes each, and a four-byte checksum.
//! assert_eq!(layout.index_len(&shape)?, 4 * 16 + 4);
//! # Ok(())
//! # }
//! ```
//!
//! The array's own chunk shape, which before #1368 named a 1x4 grid for this
//! 2x2 one without complaint, is no longer accepted:
//!
//! ```compile_fail,E0308
//! # use fieldglass_zarr::ChunkDecoder;
//! # fn main() -> Result<(), fieldglass_zarr::FieldglassError> {
//! # let meta = r#"{"zarr_format": 3, "node_type": "array", "shape": [6, 8],
//! #   "data_type": "float32", "fill_value": 0,
//! #   "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [4, 8]}},
//! #   "chunk_key_encoding": {"name": "default"},
//! #   "codecs": [
//! #     {"name": "transpose", "configuration": {"order": [1, 0]}},
//! #     {"name": "sharding_indexed", "configuration": {"chunk_shape": [4, 2],
//! #       "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
//! #       "index_codecs": [{"name": "bytes", "configuration": {"endian": "little"}},
//! #                        {"name": "crc32c"}]}}]}"#;
//! let decoder = ChunkDecoder::from_v3_metadata(meta)?;
//! let layout = decoder.chain().sharding().expect("sharded").layout();
//! layout.index_len(decoder.chunk_shape())?;
//! # Ok(())
//! # }
//! ```
//!
//! [`ChunkDecoder::decode_shard`](crate::ChunkDecoder::decode_shard) and
//! [`ZarrStore`](crate::ZarrStore)'s reads undo the permutation, so they
//! return the array's order. The [`ShardIndex`] a layout reads, and an inner
//! chunk from [`Sharding::decode_inner`], stay in the stored order.

use std::borrow::Cow;

use crate::codec::CodecChain;
use crate::crc32c;
use crate::dtype::{DType, Endian};
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::ByteRange;
use serde_json::Value;

/// Bytes one index entry occupies: an offset and a length, both `uint64`.
const ENTRY_LEN: usize = 16;

/// The sentinel both fields carry for an inner chunk that was never written.
const EMPTY: u64 = u64::MAX;

/// The most inner chunks one shard may hold (#1123): the array model's cap on
/// the chunks one plan names, so an index is at most 16 MiB. Real stores hold
/// tens to a few thousand (dynamical.org's: 72 to 1,518).
/// Spelled out rather than re-exported from core, which would put a core name
/// in this crate's public surface; a test holds the two equal.
pub const MAX_SHARD_ENTRIES: u64 = 1 << 20;

/// A shard's extent in the sharding codec's stored axis order (#1368): what
/// [`ShardLayout::index_len`], [`ShardLayout::index_range`] and
/// [`ShardLayout::read_index`] take.
///
/// Built only from the array's own chunk shape and its codec chain, which
/// says what `transpose` precedes the sharding codec, so it is in the stored
/// order whatever that order is. See the [module notes](self#axis-order).
///
/// Borrows the chunk shape when no `transpose` precedes the codec, so
/// building one for an untransposed array allocates nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredShape<'a>(Cow<'a, [usize]>);

impl<'a> StoredShape<'a> {
    /// The extent the sharding codec in `chain` sees a shard of the array's
    /// `chunk_shape` as: `chunk_shape` permuted by
    /// [`CodecChain::shard_order`], `stored[k] = chunk_shape[order[k]]`.
    ///
    /// # Errors
    ///
    /// Those of [`CodecChain::shard_order`]: a codec before the sharding codec
    /// that is not a `transpose`, and a transpose that is not a permutation of
    /// `chunk_shape.len()` axes.
    pub fn new(chunk_shape: &'a [usize], chain: &CodecChain) -> Result<Self, FieldglassError> {
        let order = chain.shard_order(chunk_shape.len())?;
        Ok(Self::permuted(Cow::Borrowed(chunk_shape), order.as_deref()))
    }

    /// `shape`, in the array's axis order, put in the stored order by
    /// `order`, which the caller has already checked is a permutation of its
    /// axes (as [`CodecChain::shard_order`] does).
    pub(crate) fn permuted(shape: Cow<'a, [usize]>, order: Option<&[usize]>) -> Self {
        match order {
            None => Self(shape),
            Some(order) => Self(order.iter().map(|&axis| shape[axis]).collect()),
        }
    }

    /// The extent along each stored axis.
    #[must_use]
    pub fn as_slice(&self) -> &[usize] {
        &self.0
    }
}

/// Which end of the shard the index sits at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IndexLocation {
    /// The index precedes the chunk data.
    Start,
    /// The index follows it — the specification's default, and what
    /// zarr-python writes.
    #[default]
    End,
}

/// A `sharding_indexed` codec's configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sharding {
    /// The chain each inner chunk's bytes were written through.
    inner: CodecChain,
    /// The inner grid, the index chain and where the index is.
    layout: ShardLayout,
}

/// What locates a shard's inner chunks (#1123): the inner chunk shape, the
/// chain the index was written through, and which end of the shard it is at.
///
/// # Axis order
///
/// Every shape here is in the sharding codec's stored axis order, which
/// differs from the array's when a `transpose` precedes the codec (#1343): see
/// the [module notes](self#axis-order). The shard's extent comes in as a
/// [`StoredShape`], built from the array's chunk shape and codec chain, so
/// the array's own order cannot be passed by mistake (#1368).
///
/// The metadata half of [`Sharding`], read without the inner chunks' own
/// codecs, so a shard's index can be found and read, and its inner chunks
/// located, whatever those codecs are: planning a read is not decoding one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardLayout {
    /// The inner chunk shape.
    chunk_shape: Vec<usize>,
    /// The chain the index itself was written through. Every codec in it must
    /// be fixed-size, which is what makes the index's length computable
    /// without reading it.
    index: CodecChain,
    /// Which end of the object the index is at.
    location: IndexLocation,
}

/// One decoded shard: the inner chunks, in the C order of the inner grid.
///
/// What [`ChunkDecoder::decode_shard`](crate::ChunkDecoder::decode_shard)
/// returns is in the array's own axis order. What `Sharding::decode`
/// returns is in the stored order, which is the same only when no `transpose`
/// precedes the sharding codec (#1343).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard {
    chunks_per_shard: Vec<usize>,
    chunks: Vec<Option<Vec<u8>>>,
}

impl Shard {
    /// How many inner chunks the grid holds along each axis, in the same axis
    /// order as the shard itself: the array's for a shard from
    /// [`ChunkDecoder::decode_shard`](crate::ChunkDecoder::decode_shard). It
    /// matches the grid of [`ShardIndex::chunks_per_shard`] only when no
    /// `transpose` precedes the sharding codec.
    #[must_use]
    pub fn chunks_per_shard(&self) -> &[usize] {
        &self.chunks_per_shard
    }

    /// One inner chunk's decoded bytes, or `None` when the shard holds no
    /// chunk there and the array's fill value stands in.
    ///
    /// `index` walks the inner grid in C order, last axis fastest, in the
    /// order of [`Self::chunks_per_shard`]. That is the array's order for a
    /// shard from [`ChunkDecoder::decode_shard`](crate::ChunkDecoder::decode_shard).
    /// The shard index is written in the stored order, so the two agree only
    /// when no `transpose` precedes the sharding codec.
    #[must_use]
    pub fn chunk(&self, index: usize) -> Option<&[u8]> {
        self.chunks.get(index)?.as_deref()
    }

    /// How many inner chunks the grid holds in total, present or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Whether the grid holds no inner chunks at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// This shard, decoded in a transposed axis order, as the array's own
    /// (#1343): `order[k]` is the array axis that became stored axis `k`, and
    /// `stored_inner` is the inner chunk shape in the stored order, as the
    /// sharding codec states it.
    ///
    /// The inner grid is put in the array's axis order and walked in C order
    /// there, and each inner chunk is untransposed, so a caller places them as
    /// it would a shard that was never transposed.
    pub(crate) fn untransposed(
        self,
        order: &[usize],
        stored_inner: &[usize],
        element_size: usize,
    ) -> Result<Self, FieldglassError> {
        let rank = order.len();
        if self.chunks_per_shard.len() != rank || stored_inner.len() != rank {
            return Err(FieldglassError::Parse(format!(
                "a transpose of {rank} axes cannot apply to a shard of {} dimensions",
                self.chunks_per_shard.len()
            )));
        }
        // Array axis `order[k]` is stored axis `k`.
        let mut per = vec![0; rank];
        let mut inner = vec![0; rank];
        for (k, &axis) in order.iter().enumerate() {
            per[axis] = self.chunks_per_shard[k];
            inner[axis] = stored_inner[k];
        }
        let mut stored: Vec<Option<Vec<u8>>> = self.chunks;
        let mut chunks = Vec::with_capacity(stored.len());
        let mut position = vec![0usize; rank];
        for _ in 0..stored.len() {
            // The same inner chunk's place in the stored grid's C order.
            let linear = order.iter().enumerate().fold(0, |acc, (k, &axis)| {
                acc * self.chunks_per_shard[k] + position[axis]
            });
            let chunk = match stored.get_mut(linear).and_then(Option::take) {
                None => None,
                Some(bytes) => Some(crate::codec::untranspose(
                    &bytes,
                    &inner,
                    order,
                    element_size,
                )?),
            };
            chunks.push(chunk);
            // Odometer over the array's own axes.
            for axis in (0..rank).rev() {
                position[axis] += 1;
                if position[axis] < per[axis] {
                    break;
                }
                position[axis] = 0;
            }
        }
        Ok(Self {
            chunks_per_shard: per,
            chunks,
        })
    }
}

impl Sharding {
    /// Read the codec's `configuration` object.
    pub(crate) fn from_v3(config: &Value, dtype: DType) -> Result<Self, FieldglassError> {
        let layout = ShardLayout::from_v3(config)?;
        let inner = CodecChain::from_v3(
            config.get("codecs").ok_or_else(|| {
                FieldglassError::Parse(
                    "a `sharding_indexed` codec states no inner `codecs`".to_string(),
                )
            })?,
            dtype,
        )?;
        if inner.sharding().is_some() {
            return Err(FieldglassError::UnsupportedSection(
                "a shard nested inside a shard is not decoded".to_string(),
            ));
        }
        Ok(Self { inner, layout })
    }
}

impl ShardLayout {
    /// Read the metadata half of a `sharding_indexed` codec's
    /// `configuration`: everything but the inner `codecs`.
    pub fn from_v3(config: &Value) -> Result<Self, FieldglassError> {
        let chunk_shape = read_shape(config.get("chunk_shape"), "sharding_indexed chunk_shape")?;
        let index = CodecChain::from_v3(
            config.get("index_codecs").ok_or_else(|| {
                FieldglassError::Parse(
                    "a `sharding_indexed` codec states no `index_codecs`".to_string(),
                )
            })?,
            // The index is `uint64`, whatever the array's own element type is.
            DType {
                kind: crate::dtype::ScalarKind::Uint,
                size: 8,
                endian: Some(Endian::Little),
            },
        )?;
        let location = match config.get("index_location").and_then(Value::as_str) {
            None | Some("end") => IndexLocation::End,
            Some("start") => IndexLocation::Start,
            Some(other) => {
                return Err(FieldglassError::Parse(format!(
                    "a `sharding_indexed` codec states index_location {other:?}"
                )));
            }
        };
        Ok(Self {
            chunk_shape,
            index,
            location,
        })
    }

    /// The inner chunk shape, in the stored axis order: the sharding codec's
    /// own `chunk_shape`, not put back in the array's order when a `transpose`
    /// precedes it. See [`CodecChain::shard_order`].
    #[must_use]
    pub fn chunk_shape(&self) -> &[usize] {
        &self.chunk_shape
    }

    /// Which end of the shard its index sits at.
    #[must_use]
    pub fn index_location(&self) -> IndexLocation {
        self.location
    }

    /// How many inner chunks a shard of `outer_shape` holds along each axis,
    /// and in all: the number of entries its index has.
    fn grid(&self, outer_shape: &StoredShape<'_>) -> Result<(Vec<usize>, usize), FieldglassError> {
        let outer_shape = outer_shape.as_slice();
        if outer_shape.len() != self.chunk_shape.len() {
            return Err(FieldglassError::Parse(format!(
                "a shard of {} dimensions holds inner chunks of {}",
                outer_shape.len(),
                self.chunk_shape.len()
            )));
        }
        let mut chunks_per_shard = Vec::with_capacity(outer_shape.len());
        for (axis, (&outer, &inner)) in outer_shape.iter().zip(&self.chunk_shape).enumerate() {
            if inner == 0 || !outer.is_multiple_of(inner) {
                return Err(FieldglassError::Parse(format!(
                    "a shard of {outer} along axis {axis} is not a whole number of \
                     {inner}-element inner chunks"
                )));
            }
            chunks_per_shard.push(outer / inner);
        }
        // Checked: two metadata numbers multiplied over every axis wrap long
        // before they describe anything a store could hold.
        let entries = chunks_per_shard
            .iter()
            .try_fold(1usize, |acc, &n| acc.checked_mul(n))
            .ok_or_else(|| FieldglassError::Parse("a shard's inner grid overflows".to_string()))?;
        // The index's length is what a host is told to fetch, so it is held
        // to a size a host should fetch: past this, metadata could ask for a
        // 16 TiB index, or one longer than a JS number counts exactly.
        if entries as u64 > MAX_SHARD_ENTRIES {
            return Err(FieldglassError::UnsupportedSection(format!(
                "a shard of {entries} inner chunks; at most {MAX_SHARD_ENTRIES} are read, an \
                 index of {} MiB",
                (MAX_SHARD_ENTRIES * ENTRY_LEN as u64) >> 20
            )));
        }
        Ok((chunks_per_shard, entries))
    }

    /// How many bytes the index occupies for a grid of `entries` inner chunks.
    ///
    /// Computable without reading the shard precisely because the index chain
    /// may hold only fixed-size codecs: `bytes` changes nothing and `crc32c`
    /// appends four. A variable-size codec there is forbidden by the
    /// specification and is refused rather than guessed at.
    fn index_bytes(&self, entries: usize) -> Result<usize, FieldglassError> {
        let overflow = || FieldglassError::Parse("shard index size overflows".to_string());
        let mut size = entries.checked_mul(ENTRY_LEN).ok_or_else(overflow)?;
        for step in self.index.steps() {
            match step {
                crate::codec::Codec::Bytes { .. } => {}
                crate::codec::Codec::Crc32c => {
                    size = size
                        .checked_add(crc32c::CHECKSUM_LEN)
                        .ok_or_else(overflow)?;
                }
                other => {
                    return Err(FieldglassError::UnsupportedSection(format!(
                        "a shard index states codec {other:?}, whose encoded size is not \
                         fixed — the specification allows only fixed-size codecs there"
                    )));
                }
            }
        }
        Ok(size)
    }

    /// How many bytes the index of a shard of `outer_shape` occupies: sixteen
    /// per inner chunk the grid could hold, and four more when the index chain
    /// ends in `crc32c` (#1123).
    ///
    /// `outer_shape` is the shard as the sharding codec sees it, in the stored
    /// axis order: see [`StoredShape`].
    ///
    /// What a host fetches before any inner chunk: the last this many bytes of
    /// the shard, or the first, as [`Self::index_location`] says.
    pub fn index_len(&self, outer_shape: &StoredShape<'_>) -> Result<u64, FieldglassError> {
        let (_, entries) = self.grid(outer_shape)?;
        Ok(self.index_bytes(entries)? as u64)
    }

    /// Where the index of a shard `shard_size` bytes long sits in it.
    ///
    /// `outer_shape` is in the stored axis order, as for [`Self::index_len`].
    ///
    /// # Errors
    ///
    /// A shard too short to hold its index, and what [`Self::index_len`]
    /// refuses.
    pub fn index_range(
        &self,
        outer_shape: &StoredShape<'_>,
        shard_size: u64,
    ) -> Result<ByteRange, FieldglassError> {
        let (_, entries) = self.grid(outer_shape)?;
        self.index_at(entries, shard_size)
    }

    /// Where the index of `entries` inner chunks sits in a shard
    /// `shard_size` bytes long: [`Self::index_range`] for a grid already in
    /// hand.
    fn index_at(&self, entries: usize, shard_size: u64) -> Result<ByteRange, FieldglassError> {
        let len = self.index_bytes(entries)? as u64;
        let start = match self.location {
            IndexLocation::Start => (len <= shard_size).then_some(0),
            IndexLocation::End => shard_size.checked_sub(len),
        };
        start
            .map(|start| ByteRange::new(start, len))
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "a shard of {shard_size} bytes cannot hold its {len}-byte index"
                ))
            })
    }

    /// Read a shard's index: where in the shard each inner chunk is, or that
    /// it is absent (#1123).
    ///
    /// `outer_shape` is in the stored axis order, as for [`Self::index_len`],
    /// and the returned [`ShardIndex`] is on the grid that shape gives: entry
    /// `position` walks the stored grid in C order, not the array's.
    ///
    /// `raw_index` is the bytes [`Self::index_range`] names, and `shard_size`
    /// the whole shard's length, which a host learns with them. The one
    /// parser of an index, whether the whole shard is in hand or only its
    /// index: [`ChunkDecoder::decode_shard`](crate::ChunkDecoder::decode_shard)
    /// reads it through this too.
    ///
    /// Every entry is held to the shard: its end may not overflow or pass
    /// `shard_size`, and its bytes may not overlap the index's own. Two inner
    /// chunks sharing bytes are not refused, since the specification leaves
    /// the layout of chunk content to the writer and says nothing against a
    /// writer storing two equal chunks once; each is read on its own.
    ///
    /// # Errors
    ///
    /// An index of the wrong length, one whose checksum fails, an entry that
    /// sets only one of its fields to the absent sentinel, and an entry
    /// outside the shard or over its index.
    pub fn read_index(
        &self,
        raw_index: &[u8],
        outer_shape: &StoredShape<'_>,
        shard_size: u64,
    ) -> Result<ShardIndex, FieldglassError> {
        let (chunks_per_shard, entries) = self.grid(outer_shape)?;
        let mut out = Vec::with_capacity(entries);
        self.each_entry(raw_index, entries, shard_size, |entry| {
            out.push(entry);
            Ok(())
        })?;
        Ok(ShardIndex {
            chunks_per_shard,
            entries: out,
        })
    }

    /// Check `raw_index` as the index of `entries` inner chunks in a shard
    /// `shard_size` bytes long, and hand `each` every entry in order: the one
    /// walk [`Self::read_index`] collects and the whole-shard decode consumes
    /// as it goes, so a shard decoded whole builds no list of entries.
    fn each_entry(
        &self,
        raw_index: &[u8],
        entries: usize,
        shard_size: u64,
        mut each: impl FnMut(Option<ByteRange>) -> Result<(), FieldglassError>,
    ) -> Result<(), FieldglassError> {
        let at = self.index_at(entries, shard_size)?;
        if raw_index.len() as u64 != at.len {
            return Err(FieldglassError::Parse(format!(
                "a shard index for {entries} inner chunks is {} bytes, not {}",
                raw_index.len(),
                at.len
            )));
        }
        // The index chain, backwards. `expected` is the bare entry array,
        // which is what the chain has to come out as.
        let index = self
            .index
            .decode(raw_index, entries * ENTRY_LEN, &[entries, 2], 8)?;
        let big_endian = self.index.declared_endian() == Some(Endian::Big);
        let index_end = at.end().unwrap_or(u64::MAX);

        for entry in index.as_chunks::<ENTRY_LEN>().0 {
            let read = |at: usize| {
                let word: [u8; 8] = entry[at..at + 8].try_into().expect("eight bytes");
                if big_endian {
                    u64::from_be_bytes(word)
                } else {
                    u64::from_le_bytes(word)
                }
            };
            let (offset, length) = (read(0), read(8));
            if offset == EMPTY && length == EMPTY {
                each(None)?;
                continue;
            }
            // A half-sentinel is not a spelling the specification gives, and
            // reading it as a real range would fetch from byte 2^64.
            if offset == EMPTY || length == EMPTY {
                return Err(FieldglassError::Parse(
                    "a shard index entry sets only one of its offset and length to the \
                     empty-chunk sentinel"
                        .to_string(),
                ));
            }
            let end = offset
                .checked_add(length)
                .filter(|&end| end <= shard_size)
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "a shard index entry names bytes {offset}..{} of a {shard_size}-byte \
                         shard",
                        offset.saturating_add(length)
                    ))
                })?;
            // Chunk bytes over the index would be read as both.
            if length > 0 && offset < index_end && at.start < end {
                return Err(FieldglassError::Parse(format!(
                    "a shard index entry names bytes {offset}..{end}, over the index itself \
                     at {}..{index_end}",
                    at.start
                )));
            }
            each(Some(ByteRange::new(offset, length)))?;
        }
        Ok(())
    }
}

impl Sharding {
    /// The byte order the inner chunks' own `bytes` codec states.
    ///
    /// The elements of a sharded array live in its inner chunks, so this is
    /// where its byte order is stated — the array's outer chain holds only this
    /// codec.
    pub(crate) fn inner_endian(&self) -> Option<Endian> {
        self.inner.declared_endian()
    }

    /// The inner chunk shape, which must divide the shard's own chunk shape,
    /// in the stored axis order (see [`ShardLayout::chunk_shape`]).
    ///
    /// The layout's, rather than a copy of it: a chain is built per read, and
    /// a second vector there was an allocation and 48 bytes of peak heap on
    /// every sharded read (#1123).
    #[must_use]
    pub fn chunk_shape(&self) -> &[usize] {
        &self.layout.chunk_shape
    }

    /// Where this codec's index is and how it is read: the part of it that
    /// is metadata, which locating inner chunks needs and decoding them does
    /// not.
    #[must_use]
    pub fn layout(&self) -> &ShardLayout {
        &self.layout
    }

    /// Which end of the shard its index sits at.
    #[must_use]
    pub fn index_location(&self) -> IndexLocation {
        self.layout.location
    }

    /// See [`ShardLayout::index_len`]. `outer_shape` is in the stored axis
    /// order.
    pub fn index_len(&self, outer_shape: &StoredShape<'_>) -> Result<u64, FieldglassError> {
        self.layout.index_len(outer_shape)
    }

    /// See [`ShardLayout::index_range`]. `outer_shape` is in the stored axis
    /// order.
    pub fn index_range(
        &self,
        outer_shape: &StoredShape<'_>,
        shard_size: u64,
    ) -> Result<ByteRange, FieldglassError> {
        self.layout.index_range(outer_shape, shard_size)
    }

    /// See [`ShardLayout::read_index`]. `outer_shape` is in the stored axis
    /// order, and the index is on the stored grid.
    pub fn read_index(
        &self,
        raw_index: &[u8],
        outer_shape: &StoredShape<'_>,
        shard_size: u64,
    ) -> Result<ShardIndex, FieldglassError> {
        self.layout.read_index(raw_index, outer_shape, shard_size)
    }

    /// Decode one inner chunk's stored bytes to its raw element bytes, in C
    /// order: what [`Self::read_index`] located, read by range (#1123).
    ///
    /// The bytes are in C order of the *stored* axes, so their shape is
    /// [`Self::chunk_shape`]. When a `transpose` precedes the codec they are
    /// not in the array's order; undo it with [`CodecChain::shard_order`].
    pub fn decode_inner(
        &self,
        stored: &[u8],
        element_size: usize,
    ) -> Result<Vec<u8>, FieldglassError> {
        let inner_bytes = self
            .layout
            .chunk_shape
            .iter()
            .try_fold(element_size, |acc, &n| acc.checked_mul(n))
            .ok_or_else(|| FieldglassError::Parse("inner chunk size overflows".to_string()))?;
        self.inner
            .decode(stored, inner_bytes, &self.layout.chunk_shape, element_size)
    }

    /// Decode a shard object into its inner chunks.
    ///
    /// `outer_shape` is the shard's extent as this codec sees it, in the stored
    /// axis order, which must be a whole number of inner chunks along every
    /// axis. The [`Shard`] returned is in the stored order too.
    pub(crate) fn decode(
        &self,
        stored: &[u8],
        outer_shape: &StoredShape<'_>,
        element_size: usize,
    ) -> Result<Shard, FieldglassError> {
        let size = stored.len() as u64;
        // The grid once, and the entries consumed as the parser checks them:
        // a shard decoded whole allocates what it did before the index could
        // be read on its own (#1123).
        let (chunks_per_shard, entries) = self.layout.grid(outer_shape)?;
        let at = self.layout.index_at(entries, size)?;
        // In bounds: `index_at` placed it inside `size`.
        let raw_index = slice(stored, at);
        let mut chunks = Vec::with_capacity(entries);
        self.layout.each_entry(raw_index, entries, size, |entry| {
            chunks.push(match entry {
                None => None,
                // In bounds: the parser held every entry to `size`.
                Some(range) => Some(self.decode_inner(slice(stored, range), element_size)?),
            });
            Ok(())
        })?;
        Ok(Shard {
            chunks_per_shard,
            chunks,
        })
    }
}

/// `range` of `bytes`, which the caller has already held to `bytes.len()`.
fn slice(bytes: &[u8], range: ByteRange) -> &[u8] {
    let start = usize::try_from(range.start).unwrap_or(usize::MAX);
    let end = range
        .end()
        .and_then(|end| usize::try_from(end).ok())
        .unwrap_or(usize::MAX);
    bytes.get(start..end).unwrap_or_default()
}

/// Where each inner chunk of one shard is, as the shard's index states it
/// (#1123): what [`Sharding::read_index`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShardIndex {
    chunks_per_shard: Vec<usize>,
    entries: Vec<Option<ByteRange>>,
}

impl ShardIndex {
    /// How many inner chunks the grid holds along each axis, in the stored axis
    /// order: [`ShardLayout::read_index`] was given a stored-order shape.
    #[must_use]
    pub fn chunks_per_shard(&self) -> &[usize] {
        &self.chunks_per_shard
    }

    /// Where inner chunk `position` is in the shard, or `None` when the shard
    /// holds none there (the fill value stands in) or `position` is past the
    /// grid. `position` walks the stored inner grid in C order, as the index does.
    #[must_use]
    pub fn entry(&self, position: usize) -> Option<ByteRange> {
        self.entries.get(position).copied().flatten()
    }

    /// How many entries the index holds, present or not.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Read a JSON array of non-negative integers as a shape.
pub(crate) fn read_shape(value: Option<&Value>, what: &str) -> Result<Vec<usize>, FieldglassError> {
    let list = value
        .and_then(Value::as_array)
        .ok_or_else(|| FieldglassError::Parse(format!("{what} is missing or is not a list")))?;
    if list.is_empty() {
        return Err(FieldglassError::Parse(format!("{what} names no axes")));
    }
    list.iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "{what} holds a value that is not a non-negative length"
                    ))
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dtype::ScalarKind;
    use serde_json::json;

    /// A shard extent already in the stored order, as the tests below
    /// state it: no `transpose` precedes these sharding codecs.
    fn shape(dims: &[usize]) -> StoredShape<'_> {
        StoredShape::permuted(Cow::Borrowed(dims), None)
    }

    fn u8_dtype() -> DType {
        DType {
            kind: ScalarKind::Uint,
            size: 1,
            endian: None,
        }
    }

    fn spec(index_location: &str) -> Sharding {
        Sharding::from_v3(
            &json!({
                "chunk_shape": [2, 2],
                "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
                "index_codecs": [
                    {"name": "bytes", "configuration": {"endian": "little"}},
                    {"name": "crc32c"}
                ],
                "index_location": index_location,
            }),
            u8_dtype(),
        )
        .unwrap()
    }

    /// Build a shard by hand: four 2x2 chunks of one-byte elements, placed in
    /// an order the index has to be believed for.
    fn build_shard(location: IndexLocation, placements: &[(u64, u64)], body: Vec<u8>) -> Vec<u8> {
        let mut index = Vec::new();
        for &(offset, length) in placements {
            index.extend_from_slice(&offset.to_le_bytes());
            index.extend_from_slice(&length.to_le_bytes());
        }
        index.extend_from_slice(&crc32c::checksum(&index).to_le_bytes());
        match location {
            IndexLocation::Start => {
                let mut out = index;
                out.extend_from_slice(&body);
                out
            }
            IndexLocation::End => {
                let mut out = body;
                out.extend_from_slice(&index);
                out
            }
        }
    }

    /// The index is believed, not the grid order. Here chunk 0 is stored last
    /// and chunk 3 first, which is legal and is what a parallel writer
    /// produces.
    #[test]
    fn chunk_positions_come_from_the_index_not_from_the_order_they_appear_in() {
        // Index sits first, so the body starts at 4*16 + 4 = 68.
        let base = 68u64;
        let body: Vec<u8> = vec![
            30, 31, 32, 33, // chunk 3
            20, 21, 22, 23, // chunk 2
            10, 11, 12, 13, // chunk 1
            0, 1, 2, 3, // chunk 0
        ];
        let placements = [(base + 12, 4), (base + 8, 4), (base + 4, 4), (base, 4)];
        let shard = build_shard(IndexLocation::Start, &placements, body);
        let decoded = spec("start").decode(&shard, &shape(&[4, 4]), 1).unwrap();
        assert_eq!(decoded.chunks_per_shard(), &[2, 2]);
        assert_eq!(decoded.chunk(0), Some(&[0u8, 1, 2, 3][..]));
        assert_eq!(decoded.chunk(1), Some(&[10u8, 11, 12, 13][..]));
        assert_eq!(decoded.chunk(3), Some(&[30u8, 31, 32, 33][..]));
    }

    /// The index at the end is the specification's default, and is what
    /// zarr-python writes.
    #[test]
    fn an_index_at_the_end_is_found_there() {
        let body: Vec<u8> = (0..16u8).collect();
        let placements = [(0u64, 4), (4, 4), (8, 4), (12, 4)];
        let shard = build_shard(IndexLocation::End, &placements, body);
        let decoded = spec("end").decode(&shard, &shape(&[4, 4]), 1).unwrap();
        assert_eq!(decoded.chunk(2), Some(&[8u8, 9, 10, 11][..]));

        // And the default when the field is absent at all.
        let defaulted = Sharding::from_v3(
            &json!({
                "chunk_shape": [2, 2],
                "codecs": [{"name": "bytes"}],
                "index_codecs": [{"name": "bytes"}, {"name": "crc32c"}],
            }),
            u8_dtype(),
        )
        .unwrap();
        assert_eq!(defaulted.index_location(), IndexLocation::End);
    }

    /// The all-ones sentinel is an absent chunk, and a zero-length chunk at
    /// offset zero is a real one — the two must not be confused.
    #[test]
    fn the_empty_sentinel_is_distinguished_from_a_zero_length_chunk() {
        let body: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6, 7];
        let placements = [(0u64, 4), (EMPTY, EMPTY), (4, 4), (EMPTY, EMPTY)];
        let shard = build_shard(IndexLocation::End, &placements, body);
        let decoded = spec("end").decode(&shard, &shape(&[4, 4]), 1).unwrap();
        assert_eq!(decoded.chunk(0), Some(&[0u8, 1, 2, 3][..]));
        assert_eq!(decoded.chunk(1), None);
        assert_eq!(decoded.chunk(2), Some(&[4u8, 5, 6, 7][..]));
        assert_eq!(decoded.chunk(3), None);
        assert_eq!(decoded.len(), 4);
        assert!(!decoded.is_empty());
    }

    /// Half a sentinel is not a spelling the format has, and reading it as a
    /// range would name byte 2^64.
    #[test]
    fn refuses_a_half_written_sentinel() {
        let placements = [(0u64, 4), (EMPTY, 4), (4, 4), (8, 4)];
        let shard = build_shard(IndexLocation::End, &placements, (0..12u8).collect());
        assert!(spec("end").decode(&shard, &shape(&[4, 4]), 1).is_err());
    }

    /// The index's own checksum is checked; a corrupt index is refused rather
    /// than used to slice arbitrary ranges out of the object.
    #[test]
    fn a_corrupt_index_is_refused() {
        let placements = [(0u64, 4), (4, 4), (8, 4), (12, 4)];
        let mut shard = build_shard(IndexLocation::End, &placements, (0..16u8).collect());
        let len = shard.len();
        shard[len - 20] ^= 0xFF;
        let err = spec("end").decode(&shard, &shape(&[4, 4]), 1).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("crc32c mismatch")),
            "got {err:?}"
        );
    }

    /// An entry pointing outside the object must be an error, not a panic.
    #[test]
    fn refuses_an_entry_that_points_outside_the_shard() {
        let placements = [(0u64, 4), (4, 4), (8, 4), (900, 4)];
        let shard = build_shard(IndexLocation::End, &placements, (0..16u8).collect());
        assert!(spec("end").decode(&shard, &shape(&[4, 4]), 1).is_err());
    }

    /// A shard whose extent is not a whole number of inner chunks has no
    /// well-defined grid.
    #[test]
    fn refuses_a_grid_that_does_not_divide() {
        let shard = build_shard(IndexLocation::End, &[(0, 4)], (0..4u8).collect());
        assert!(spec("end").decode(&shard, &shape(&[3, 2]), 1).is_err());
        assert!(spec("end").decode(&shard, &shape(&[4]), 1).is_err());
    }

    /// A variable-size codec in the index chain would make the index's length
    /// unknowable, which is why the specification forbids it.
    #[test]
    fn refuses_a_compressor_in_the_index_chain() {
        let sharding = Sharding::from_v3(
            &json!({
                "chunk_shape": [2, 2],
                "codecs": [{"name": "bytes"}],
                "index_codecs": [{"name": "bytes"}, {"name": "gzip", "configuration": {"level": 5}}],
            }),
            u8_dtype(),
        )
        .unwrap();
        let err = sharding
            .decode(&[0u8; 128], &shape(&[4, 4]), 1)
            .unwrap_err();
        assert!(
            matches!(err, FieldglassError::UnsupportedSection(_)),
            "got {err:?}"
        );
    }

    #[test]
    fn refuses_a_shard_nested_in_a_shard() {
        let err = Sharding::from_v3(
            &json!({
                "chunk_shape": [2, 2],
                "codecs": [{
                    "name": "sharding_indexed",
                    "configuration": {
                        "chunk_shape": [1, 1],
                        "codecs": [{"name": "bytes"}],
                        "index_codecs": [{"name": "bytes"}, {"name": "crc32c"}]
                    }
                }],
                "index_codecs": [{"name": "bytes"}, {"name": "crc32c"}],
            }),
            u8_dtype(),
        )
        .unwrap_err();
        assert!(matches!(err, FieldglassError::UnsupportedSection(_)));
    }

    /// The index read from its own bytes alone, as a host that fetched only
    /// the index has it, says what the whole-shard decode reads (#1123).
    #[test]
    fn an_index_read_on_its_own_locates_the_chunks_the_shard_decode_reads() {
        for (name, location, base) in [
            ("end", IndexLocation::End, 0u64),
            ("start", IndexLocation::Start, 68),
        ] {
            let placements = [(base + 8, 4), (EMPTY, EMPTY), (base, 4), (base + 4, 4)];
            let body: Vec<u8> = (0..12u8).collect();
            let shard = build_shard(location, &placements, body);
            let sharding = spec(name);
            let size = shard.len() as u64;
            let at = sharding.index_range(&shape(&[4, 4]), size).unwrap();
            assert_eq!(at.len, 68);
            assert_eq!(
                at.start,
                if location == IndexLocation::End {
                    size - 68
                } else {
                    0
                }
            );
            let raw = &shard[at.start as usize..(at.start + at.len) as usize];
            let index = sharding.read_index(raw, &shape(&[4, 4]), size).unwrap();
            assert_eq!(index.len(), 4);
            assert_eq!(index.entry(1), None);
            assert_eq!(index.entry(9), None);
            let whole = sharding.decode(&shard, &shape(&[4, 4]), 1).unwrap();
            for position in [0, 2, 3] {
                let range = index.entry(position).unwrap();
                let bytes = &shard[range.start as usize..(range.start + range.len) as usize];
                assert_eq!(
                    sharding.decode_inner(bytes, 1).unwrap(),
                    whole.chunk(position).unwrap()
                );
            }
        }
    }

    /// Hostile entries, each refused by the one parser both paths share: an
    /// end that overflows, one past the shard, and one over the index.
    #[test]
    fn entries_outside_the_shard_or_over_its_index_are_refused() {
        let index_at_end = |placements: &[(u64, u64)]| {
            build_shard(IndexLocation::End, placements, (0..16u8).collect())
        };
        let refused = |shard: &[u8], location: &str, what: &str| {
            let sharding = spec(location);
            let err = sharding.decode(shard, &shape(&[4, 4]), 1).unwrap_err();
            assert!(matches!(err, FieldglassError::Parse(_)), "{what}: {err:?}");
            // And the same refusal reading the index alone.
            let size = shard.len() as u64;
            let at = sharding.index_range(&shape(&[4, 4]), size).unwrap();
            let raw = &shard[at.start as usize..(at.start + at.len) as usize];
            assert!(
                sharding.read_index(raw, &shape(&[4, 4]), size).is_err(),
                "{what}"
            );
        };
        refused(
            &index_at_end(&[(0, 4), (4, 4), (8, 4), (u64::MAX - 1, 4)]),
            "end",
            "overflow",
        );
        refused(
            &index_at_end(&[(0, 4), (4, 4), (8, 4), (80, 5)]),
            "end",
            "past the end",
        );
        // The index of this shard is its last 68 bytes, from byte 16.
        //
        // A known divergence from zarr-python (3.3.0), which reads such an
        // entry and decodes the index's own bytes as the chunk's values
        // (recorded with its reproduction in tests/fixtures/NOTICE.md). The
        // specification lays the index and the chunk content out as separate
        // parts of the shard and states no case where they share bytes, so an
        // entry over the index is a malformed or hostile index, and reading it
        // would show index bytes as data.
        refused(
            &index_at_end(&[(0, 4), (4, 4), (8, 4), (14, 4)]),
            "end",
            "over the index",
        );
        // At the start, the index is bytes 0..68.
        let shard = build_shard(
            IndexLocation::Start,
            &[(68, 4), (72, 4), (76, 4), (60, 4)],
            (0..12u8).collect(),
        );
        refused(&shard, "start", "over an index at the start");
        // Two chunks sharing bytes is not refused: the layout is the writer's.
        let shared = index_at_end(&[(0, 4), (0, 4), (8, 4), (12, 4)]);
        let decoded = spec("end").decode(&shared, &shape(&[4, 4]), 1).unwrap();
        assert_eq!(decoded.chunk(0), decoded.chunk(1));
    }

    /// The index's length is what a host fetches, so it must be exactly what
    /// the grid and the chain say; bytes of any other length are refused, as
    /// is a shard too short to hold it.
    #[test]
    fn an_index_of_the_wrong_length_or_a_shard_too_short_is_refused() {
        let sharding = spec("end");
        assert_eq!(sharding.index_len(&shape(&[4, 4])).unwrap(), 4 * 16 + 4);
        assert!(sharding.index_range(&shape(&[4, 4]), 67).is_err());
        assert_eq!(
            sharding.index_range(&shape(&[4, 4]), 68).unwrap(),
            ByteRange::new(0, 68)
        );
        assert!(
            sharding
                .read_index(&[0u8; 67], &shape(&[4, 4]), 100)
                .is_err()
        );
        assert!(
            sharding
                .read_index(&[0u8; 69], &shape(&[4, 4]), 100)
                .is_err()
        );
        assert_eq!(
            spec("start").index_range(&shape(&[4, 4]), 100).unwrap(),
            ByteRange::new(0, 68)
        );
        // Without `crc32c` the index is the bare entries.
        let bare = Sharding::from_v3(
            &json!({
                "chunk_shape": [2, 2],
                "codecs": [{"name": "bytes"}],
                "index_codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
            }),
            u8_dtype(),
        )
        .unwrap();
        assert_eq!(bare.index_len(&shape(&[4, 4])).unwrap(), 64);
    }

    /// An inner grid whose entry count overflows is refused, not wrapped into
    /// a small index that would then be believed.
    #[test]
    fn an_inner_grid_that_overflows_is_refused() {
        let sharding = Sharding::from_v3(
            &json!({
                "chunk_shape": [1, 1, 1, 1],
                "codecs": [{"name": "bytes"}],
                "index_codecs": [{"name": "bytes"}, {"name": "crc32c"}],
            }),
            u8_dtype(),
        )
        .unwrap();
        assert_eq!(
            MAX_SHARD_ENTRIES,
            fieldglass_core::array::MAX_PLANNED_CHUNKS
        );
        // At the cap the index is 16 MiB and four bytes; one past it is
        // refused, so metadata cannot ask a host for a larger index.
        let grid = |n: usize| [n, 1024, 1, 1];
        assert_eq!(
            sharding.index_len(&shape(&grid(1024))).unwrap(),
            MAX_SHARD_ENTRIES * 16 + 4
        );
        assert!(matches!(
            sharding.index_len(&shape(&grid(1025))),
            Err(FieldglassError::UnsupportedSection(_))
        ));
        let huge = usize::MAX / 2 + 1;
        assert!(sharding.index_len(&shape(&[huge, 2, 1, 1])).is_err());
        assert!(
            sharding
                .index_len(&shape(&[usize::MAX / 8, 1, 1, 1]))
                .is_err()
        );
    }
}
