//! Zarr chunk codecs: what a stored chunk has to be run through to become
//! numbers.
//!
//! Zarr keeps an array as a grid of chunks, each its own object in a store, and
//! each written through a chain of codecs the array's metadata names. This
//! crate reads that chain and reverses it. Both editions are covered — v2's
//! `filters` plus `compressor`, and v3's `codecs` list — including blosc and
//! its five inner compressors, the byte and bit transposes, gzip and zlib and
//! zstd and LZ4, and v3's `sharding_indexed`, which puts a grid of chunks in
//! one object behind an index.
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
//! # What this crate is not
//!
//! # Reading a whole store
//!
//! [`ZarrStore`] walks a store — groups, arrays, shared dimensions, either
//! edition, consolidated or not — into `fieldglass-core`'s array model, and
//! reads any region of an array by fetching exactly the chunks it covers
//! (#658). It reads through the [`ObjectSource`] seam, so a directory, a bucket
//! and a browser's map of fetched objects are the same store to it, and it
//! implements [`ArraySource`], the array-level IO seam every container of
//! named arrays presents: Zarr is a layout of chunks under keys, not a file
//! format, and it sits where the NetCDF readers do.
//!
//! # What this crate is not
//!
//! **It fetches nothing.** The objects arrive in an [`ObjectSource`] the host
//! filled, the same way every other Fieldglass reader is handed its bytes
//! (ADR-0005 decision 1). Which store it is — a directory, a bucket, a map —
//! is the only part of reading one that differs between a filesystem, an
//! object store and a browser, and it is the host's.
//!
//! **It plans no remote fetches.** Turning a chunk into a byte range in some
//! other object — a kerchunk reference document — is `fieldglass-fetchplan`'s
//! question. The chunk grid and the key encoding both crates spell keys with
//! are `fieldglass-core`'s (#677), and this crate is the one parser of an
//! array's metadata document (#686).
//!
//! **It applies no CF conventions.** [`ChunkDecoder::decode_raw_values`]
//! returns the values as stored. An array written by xarray carries
//! `scale_factor`, `add_offset` and `_FillValue` in its `.zattrs`, and a packed
//! `int16` array reads back here as integer codes rather than physical units —
//! the split `fieldglass-netcdf` draws between `decode_variable_raw` and
//! `decode_variable_physical`, for the same reason: CF is a convention over the
//! container rather than part of it, and the reference implementations
//! disagree about applying it (libnetcdf never does; netcdf4-python and xarray
//! do by default). This crate is the libnetcdf analogue for a chunk. A whole
//! array's physical values are [`ArraySource::read_region_physical`], which
//! applies core's one CF rule from the attributes [`ZarrStore`] reads.
//!
//! **It decodes; it never encodes.** The forward direction of each transform
//! exists only under `#[cfg(test)]`, where a round trip is the one check that
//! does not simply restate the inverse it is testing.
//!
//! # Codecs
//!
//! | Codec | v2 `id` | v3 `name` | Notes |
//! |---|---|---|---|
//! | blosc | `blosc` | `blosc` | Container plus one of blosclz, lz4, lz4hc, zlib, zstd; byte or bit shuffle. Snappy is refused. |
//! | zlib | `zlib` | — | RFC 1950. |
//! | gzip | `gzip` | `gzip` | RFC 1952, header fields and all. |
//! | zstd | `zstd` | `zstd` | Bounded window and output. |
//! | LZ4 | `lz4` | — | The numcodecs framing: a four-byte length, then a block. |
//! | shuffle | `shuffle` | — | Byte transpose; v3 reaches it through blosc. |
//! | bytes | — | `bytes` | States the element byte order. |
//! | transpose | — | `transpose` | Axis permutation; v2's `order: "F"` is the same thing. |
//! | crc32c | — | `crc32c` | Castagnoli, appended little-endian. |
//! | sharding | — | `sharding_indexed` | See [`Shard`]. |
//!
//! Anything else — `bz2`, `vlen-utf8`, the extension codecs — is refused by
//! name rather than mis-decoded.

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
pub use fieldglass_core::array::{ArrayError, ChunkGrid, ChunkKeyEncoding};
// What `ZarrStore` is read through and what it reads into, so a consumer holding
// a store can name the trait, the tree and the seam without a `fieldglass-core`
// line of its own (#658).
pub use fieldglass_core::array::{ArrayDescription, ArraySource, Group};
pub use fieldglass_core::bytes::ObjectSource;
pub use store::ZarrStore;
// Re-exported so a consumer needs no direct `fieldglass-core` line in its
// manifest, and so cannot take one without `default-features = false` — which
// would re-enable `render` and `fs` across the whole dependency graph (#537).
pub use fieldglass_core::FieldglassError;
#[cfg(feature = "codecs")]
pub use shard::{IndexLocation, Shard, Sharding};

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
    /// `order: "F"` is honoured by decoding into C order, so a caller indexes
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
    pub fn decode_shard(&self, stored: &[u8]) -> Result<Shard, FieldglassError> {
        let sharding = self.chain.sharding().ok_or_else(|| {
            FieldglassError::WrongLayout(
                "this array's chunks are not shards; decode them with `decode`".to_string(),
            )
        })?;
        sharding.decode(stored, &self.chunk_shape, self.dtype.size)
    }
}

/// Compiles and runs the README's usage snippet as a doc test, so the crate's
/// front page cannot drift from the API it describes (#539).
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeSnippet;
