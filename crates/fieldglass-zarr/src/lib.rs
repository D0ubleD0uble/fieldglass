//! Reads Zarr v2 and v3 stores from the bytes you hand it.
//!
//! A Zarr store keeps each array as a grid of chunks, and each chunk as its
//! own small object, usually compressed. This crate reads a store's layout,
//! reads any region of an array by fetching only the chunks that region
//! covers, and undoes each chunk's compression to get its numbers back. It
//! never opens files or makes network requests itself, so the same code reads
//! a local folder, objects fetched over HTTP, or a cloud bucket.
//!
//! These pages are for writing code against the crate. The
//! [crate's README](https://crates.io/crates/fieldglass-zarr) covers what it
//! reads and has short runnable examples, one per question.
//!
//! # Reading a whole store
//!
//! [`ZarrStore`] walks a store, either edition, consolidated or not, and
//! describes its groups, arrays and shared dimensions in `fieldglass-core`'s
//! array model. It reads through the [`ObjectSource`] trait, so a directory, a
//! bucket and a browser's map of fetched objects are the same store to it. It
//! also implements [`ArraySource`], the same interface the NetCDF readers
//! present, so a program that reads one kind of container reads the others
//! the same way.
//!
//! Values come back as stored, or in physical units through
//! [`ArraySource::read_region_physical`], which applies the array's
//! `scale_factor`, `add_offset` and `_FillValue` from the attributes
//! [`ZarrStore`] reads.
//!
//! # Decoding one chunk you already hold
//!
//! For the low-level path, `ChunkDecoder` reads an array's metadata document
//! and decodes the bytes of one chunk:
//!
//! ```
//! use fieldglass_zarr::ChunkDecoder;
//!
//! // A `.zarray`, as zarr-python writes one. `compressor: null` means the
//! // chunk is the raw little-endian elements.
//! let zarray = r#"{
//!     "zarr_format": 2, "shape": [4, 6], "chunks": [2, 3],
//!     "dtype": "<f4", "order": "C", "fill_value": 0.0,
//!     "compressor": null, "filters": null
//! }"#;
//! let decoder = ChunkDecoder::from_v2_metadata(zarray)?;
//!
//! let chunk: Vec<u8> = (0..6u32).flat_map(|i| (i as f32).to_le_bytes()).collect();
//! assert_eq!(decoder.decode_raw_values(&chunk)?, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
//! # Ok::<(), fieldglass_zarr::FieldglassError>(())
//! ```
//!
//! # What it doesn't do
//!
//! It doesn't fetch anything. The objects arrive in an [`ObjectSource`] the
//! host filled, the same way every other Fieldglass reader is handed its bytes
//! (ADR-0005). Working out which byte ranges of another object a chunk sits
//! in, for example an entry in a kerchunk reference document, is
//! `fieldglass-fetchplan`'s job.
//!
//! It doesn't write. The compressors only exist in the forward direction under
//! `#[cfg(test)]`, where a round trip checks each one.
//!
//! It doesn't read text, dates, structured records, complex numbers or 16-bit
//! decimals, and it doesn't decode a codec outside the table below. These are
//! refused by name instead of being misread.
//!
//! # Codecs
//!
//! | Codec | v2 `id` | v3 `name` | Notes |
//! |---|---|---|---|
//! | blosc | `blosc` | `blosc` | Container plus one of blosclz, lz4, lz4hc, zlib, zstd; byte or bit shuffle. Snappy is refused. |
//! | zlib | `zlib` | | RFC 1950. |
//! | gzip | `gzip` | `gzip` | RFC 1952, header fields and all. |
//! | zstd | `zstd` | `zstd` | Bounded window and output. |
//! | LZ4 | `lz4` | | The numcodecs framing: a four-byte length, then a block. |
//! | shuffle | `shuffle` | | Byte transpose; v3 reaches it through blosc. |
//! | bytes | | `bytes` | States the element byte order. |
//! | transpose | | `transpose` | Axis permutation; v2's `order: "F"` is the same thing. |
//! | crc32c | | `crc32c` | Castagnoli, appended little-endian. |
//! | sharding | | `sharding_indexed` | See `Shard`. |
//!
//! The codecs sit behind the default `codecs` feature, so a program that only
//! needs to read an array's metadata can leave the decompressors out.

#![forbid(unsafe_code)]

pub mod base64;

// The decode half. Behind `codecs` (on by default) so a consumer that only
// reads an array's metadata links no decompressor — see the feature's own
// comment in Cargo.toml. `dtype` and `metadata` stay ungated: reading a
// document is not decoding one.
#[cfg(feature = "codecs")]
pub mod blosc;
#[cfg(feature = "codecs")]
pub mod blosclz;
#[cfg(feature = "codecs")]
pub mod codec;
#[cfg(feature = "codecs")]
pub mod crc32c;
#[cfg(feature = "codecs")]
pub mod deflate;
pub mod dtype;
#[cfg(feature = "codecs")]
pub mod lz4;
pub mod metadata;
#[cfg(feature = "codecs")]
pub mod shard;
#[cfg(feature = "codecs")]
pub mod shuffle;
// The store walker. Not behind `codecs`: listing what a store holds reads
// documents only, and a region read without the feature says so rather than
// failing to compile.
pub mod store;
#[cfg(feature = "codecs")]
pub mod zstd;

#[cfg(feature = "codecs")]
pub use codec::{Codec, CodecChain};
pub use dtype::{DType, Endian, ScalarKind};
pub use metadata::{ArrayMetadata, CodecSource, ElementOrder};
// Re-exported for the reason `FieldglassError` is: a consumer reading an
// array's metadata gets the shared model's types without a `fieldglass-core`
// line of its own, and so cannot take one without `default-features = false`.
/// The shared allocation cap this crate's own cap is built from (#707), so a
/// consumer of this crate alone can name the bar every reader is held to
/// without a `fieldglass-core` line of its own.
pub use fieldglass_core::MAX_FIELD_POINTS;
pub use fieldglass_core::array::{ArrayError, ChunkGrid, ChunkKeyEncoding};
/// The shape a container states an array it left out in (#709), and the code
/// it gives for why (#1129), so a consumer of this crate alone can read
/// `ArraySource::left_out` without a `fieldglass-core` line of its own.
pub use fieldglass_core::{LeftOut, LeftOutCause};
// What `ZarrStore` is read through and what it reads into, so a consumer holding
// a store can name the trait, the tree and the seam without a `fieldglass-core`
// line of its own (#658).
pub use fieldglass_core::array::{ArrayDescription, ArraySource, Group};
pub use fieldglass_core::bytes::{ByteRange, ObjectSource};
pub use store::ZarrStore;
#[cfg(feature = "codecs")]
pub use store::{InnerChunkRange, ShardIndexRange};
// Re-exported so a consumer needs no direct `fieldglass-core` line in its
// manifest, and so cannot take one without `default-features = false` — which
// would re-enable `render` and `fs` across the whole dependency graph (#537).
pub use fieldglass_core::FieldglassError;
#[cfg(feature = "codecs")]
pub use shard::{
    IndexLocation, MAX_SHARD_ENTRIES, Shard, ShardIndex, ShardLayout, Sharding, StoredShape,
};

/// The decode side of one Zarr array: its element type, its chunk shape, and
/// the codec chain its chunks were written through.
///
/// Built from an array's metadata document — a v2 `.zarray` or a v3
/// `zarr.json` — and then applied to as many chunks as the caller has. Nothing
/// about it is per-chunk, so a host builds one per array and reuses it.
#[cfg(feature = "codecs")]
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkDecoder {
    dtype: DType,
    chunk_shape: Vec<usize>,
    chain: CodecChain,
    fill_value: Option<f64>,
}

#[cfg(feature = "codecs")]
impl ChunkDecoder {
    /// Build the decode side from an already-read metadata document.
    ///
    /// The reading is [`ArrayMetadata`]'s, in either edition, so nothing about
    /// which fields a document has or what they are called is decided twice.
    /// What happens here is what only the decode side needs: turning the codec
    /// configuration into a chain, and settling the byte order.
    pub fn from_metadata(meta: &ArrayMetadata) -> Result<Self, FieldglassError> {
        // The grid is `u64` because an array's declared shape is a file's
        // number and a browser's pointer is 32 bits wide (#561); a chunk shape
        // that does not fit one is a chunk nothing could hold in memory anyway,
        // so it is refused here rather than narrowed silently.
        let chunk_shape = meta
            .grid()
            .chunk_shape()
            .iter()
            .map(|extent| {
                usize::try_from(*extent).map_err(|_| {
                    FieldglassError::Parse(format!(
                        "a chunk extent of {extent} does not fit this target's pointer width"
                    ))
                })
            })
            .collect::<Result<Vec<usize>, _>>()?;

        let dtype = meta.dtype();
        let chain = match meta.codecs() {
            CodecSource::V2 {
                order,
                compressor,
                filters,
            } => {
                let mut steps = Vec::new();
                // Fortran order is the innermost stage: the elements were laid
                // out that way before any filter saw them. It is the same
                // operation as a v3 `transpose` with the axes reversed, and is
                // carried as one.
                if *order == ElementOrder::Fortran {
                    steps.push(Codec::Transpose {
                        order: (0..chunk_shape.len()).rev().collect(),
                    });
                }
                let rest = CodecChain::from_v2(compressor.as_ref(), filters.as_ref(), dtype.size)?;
                steps.extend(rest.steps().iter().cloned());
                CodecChain::new(steps)
            }
            CodecSource::V3 { codecs } => CodecChain::from_v3(codecs, dtype)?,
        };

        // Refused here rather than at the first shard: an array whose shards
        // cannot be put back in its own axis order must not read as one that
        // can (#1343). A plain sharded chain allocates nothing for it.
        chain.shard_order(chunk_shape.len())?;

        // A multi-byte element type has no order until the `bytes` codec gives
        // it one, and a chain without that codec cannot say which order its
        // bytes are in. Defaulting to little would be right nearly always,
        // which is what makes it a bad default. v2 states the order in the
        // dtype itself, so this only ever bites v3.
        let dtype = match (meta.zarr_format(), dtype.size > 1, chain.declared_endian()) {
            (3, true, None) => {
                return Err(FieldglassError::Parse(
                    "a Zarr v3 array of a multi-byte type states no `bytes` codec, so \
                     nothing states the byte order its elements were written in"
                        .to_string(),
                ));
            }
            (_, _, Some(endian)) => dtype.with_endian(endian),
            _ => dtype,
        };

        Ok(Self {
            dtype,
            chunk_shape,
            chain,
            fill_value: meta.fill_value(),
        })
    }

    /// Read a Zarr v2 `.zarray` document.
    ///
    /// `order: "F"` is honored by decoding into C order, so a caller indexes
    /// every array the same way whichever order it was written in.
    pub fn from_v2_metadata(json: &str) -> Result<Self, FieldglassError> {
        Self::from_metadata(&ArrayMetadata::from_v2(json)?)
    }

    /// Read a Zarr v3 `zarr.json` document for an array.
    pub fn from_v3_metadata(json: &str) -> Result<Self, FieldglassError> {
        Self::from_metadata(&ArrayMetadata::from_v3(json)?)
    }

    /// The array's element type, with the byte order its chunks were written
    /// in.
    #[must_use]
    pub fn dtype(&self) -> DType {
        self.dtype
    }

    /// The chunk shape, in elements, last axis fastest.
    #[must_use]
    pub fn chunk_shape(&self) -> &[usize] {
        &self.chunk_shape
    }

    /// The codec chain, in the order the encoder applied it.
    #[must_use]
    pub fn chain(&self) -> &CodecChain {
        &self.chain
    }

    /// The array's fill value, when it states a numeric one.
    ///
    /// The string spellings both editions use for what JSON cannot hold —
    /// `"NaN"`, `"Infinity"`, `"-Infinity"`, and v3's hex bit patterns — are
    /// read as the numbers they name. `None` covers "no fill value" and one no
    /// number expresses, such as a structured dtype's base64 blob, which is not
    /// invented here.
    #[must_use]
    pub fn fill_value(&self) -> Option<f64> {
        self.fill_value
    }

    /// How many bytes one whole chunk decodes to.
    fn chunk_bytes(&self) -> Result<usize, FieldglassError> {
        self.chunk_shape
            .iter()
            .try_fold(self.dtype.size, |acc, &n| acc.checked_mul(n))
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "a chunk of {:?} {}-byte elements overflows an address",
                    self.chunk_shape, self.dtype.size
                ))
            })
    }

    /// Decode one stored chunk to its raw element bytes, in C order.
    ///
    /// The bytes are in the array's stored byte order; [`Self::dtype`] says
    /// which. Use [`Self::decode_raw_values`] to get numbers.
    pub fn decode(&self, stored: &[u8]) -> Result<Vec<u8>, FieldglassError> {
        self.chain.decode(
            stored,
            self.chunk_bytes()?,
            &self.chunk_shape,
            self.dtype.size,
        )
    }

    /// Decode one stored chunk to numbers, in C order.
    ///
    /// **These are the raw stored values.** The codec chain is reversed and the
    /// elements are read out at their declared type; nothing else is applied.
    /// In particular an array written by xarray carries CF `scale_factor` /
    /// `add_offset` / `_FillValue` in its `.zattrs`, and a packed `int16` array
    /// comes back here as integer codes rather than physical units — the same
    /// split `fieldglass-netcdf` draws between
    /// `decode_variable_raw` and `decode_variable_physical`, and for the same
    /// reason: CF is a convention over the container, not part of it.
    ///
    /// This crate has no physical-units counterpart yet because it cannot read
    /// `.zattrs` — it is handed one chunk's bytes and never the store (#658 is
    /// the walker that will have them). The name says raw now so that the two
    /// crates cannot land on opposite defaults once it does.
    pub fn decode_raw_values(&self, stored: &[u8]) -> Result<Vec<f64>, FieldglassError> {
        self.dtype.read_values(&self.decode(stored)?)
    }

    /// Whether this array's chunks are shards holding a grid of inner chunks.
    #[must_use]
    pub fn is_sharded(&self) -> bool {
        self.chain.sharding().is_some()
    }

    /// Decode one stored shard into its inner chunks.
    ///
    /// Only for an array whose chain ends in `sharding_indexed`; anything else
    /// is a chunk and goes through [`Self::decode`]. Each returned chunk is
    /// raw element bytes in C order, or `None` where the shard holds nothing
    /// and [`Self::fill_value`] stands in.
    ///
    /// The inner grid and each inner chunk are in the array's own axis order,
    /// also when a `transpose` before the sharding codec stored them in
    /// another (#1343). An inner chunk's shape is then the sharding codec's
    /// `chunk_shape` put back in the array's axis order.
    pub fn decode_shard(&self, stored: &[u8]) -> Result<Shard, FieldglassError> {
        let sharding = self.sharding()?;
        let order = self.chain.shard_order(self.chunk_shape.len())?;
        // The sharding codec was handed the shard in this order, and its
        // inner grid divides that shape. Borrowed, so nothing is allocated,
        // when no `transpose` precedes it.
        let outer = StoredShape::permuted(
            std::borrow::Cow::Borrowed(&self.chunk_shape),
            order.as_deref(),
        );
        let shard = sharding.decode(stored, &outer, self.dtype.size)?;
        match order {
            None => Ok(shard),
            Some(order) => shard.untransposed(&order, sharding.chunk_shape(), self.dtype.size),
        }
    }

    /// Decode one inner chunk of a shard, read by range, to its raw element
    /// bytes in C order of the array's own axes (#1123, #1343).
    pub(crate) fn decode_inner(
        &self,
        stored: &[u8],
        order: Option<&[usize]>,
    ) -> Result<Vec<u8>, FieldglassError> {
        let sharding = self.sharding()?;
        let raw = sharding.decode_inner(stored, self.dtype.size)?;
        let Some(order) = order else {
            return Ok(raw);
        };
        // The inner chunk shape as the array's axes see it.
        let mut shape = vec![0; order.len()];
        for (&axis, &extent) in order.iter().zip(sharding.chunk_shape()) {
            *shape.get_mut(axis).ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "a transpose order names axis {axis} of a {}-dimensional inner chunk",
                    order.len()
                ))
            })? = extent;
        }
        crate::codec::untranspose(&raw, &shape, order, self.dtype.size)
    }

    fn sharding(&self) -> Result<&Sharding, FieldglassError> {
        self.chain.sharding().ok_or_else(|| {
            FieldglassError::WrongLayout(
                "this array's chunks are not shards; decode them with `decode`".to_string(),
            )
        })
    }
}

/// Compiles and runs the README's usage snippet as a doc test, so the crate's
/// front page cannot drift from the API it describes (#539).
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeSnippet;
