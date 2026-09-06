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
//! assert_eq!(decoder.decode_values(&chunk)?, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
//! # Ok::<(), fieldglass_zarr::FieldglassError>(())
//! ```
//!
//! # What this crate is not
//!
//! **It performs no I/O.** A chunk arrives as bytes the caller already has, the
//! same way every other Fieldglass decoder works (ADR-0005 decision 1). Walking
//! a directory or a bucket to find the chunks is the host's job, and it is the
//! only part of reading a Zarr store that differs between a filesystem, an
//! object store and a browser.
//!
//! **It does not address chunks.** Which key holds which region of the array —
//! `temp/0.1` under v2's dimension separator, `temp/c/0/1` under v3's chunk key
//! encoding, or an entry in a kerchunk reference document — is
//! `fieldglass-fetchplan`'s question, and it answers it for the remote case
//! too, as byte ranges. This crate reads the chunk *shape*, because a decode
//! cannot check its own output length or reverse a transpose without it, and
//! nothing else about the array's layout.
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

pub mod blosc;
pub mod blosclz;
pub mod codec;
pub mod crc32c;
pub mod deflate;
pub mod dtype;
pub mod lz4;
pub mod shard;
pub mod shuffle;
pub mod zstd;

use fieldglass_core::FieldglassError as CoreError;
use serde_json::Value;

pub use codec::{Codec, CodecChain};
pub use dtype::{DType, Endian, ScalarKind};
// Re-exported so a consumer needs no direct `fieldglass-core` line in its
// manifest, and so cannot take one without `default-features = false` — which
// would re-enable `render` and `fs` across the whole dependency graph (#537).
pub use fieldglass_core::FieldglassError;
pub use shard::{IndexLocation, Shard, Sharding};

/// The decode side of one Zarr array: its element type, its chunk shape, and
/// the codec chain its chunks were written through.
///
/// Built from an array's metadata document — a v2 `.zarray` or a v3
/// `zarr.json` — and then applied to as many chunks as the caller has. Nothing
/// about it is per-chunk, so a host builds one per array and reuses it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkDecoder {
    dtype: DType,
    chunk_shape: Vec<usize>,
    chain: CodecChain,
    fill_value: Option<f64>,
}

impl ChunkDecoder {
    /// Read a Zarr v2 `.zarray` document.
    ///
    /// `order: "F"` is honoured by decoding into C order, so a caller indexes
    /// every array the same way whichever order it was written in. It is the
    /// same operation as a v3 `transpose` with the axes reversed, and is
    /// carried as one.
    pub fn from_v2_metadata(json: &str) -> Result<Self, CoreError> {
        let meta: Value = serde_json::from_str(json)
            .map_err(|e| CoreError::Parse(format!("Zarr v2 .zarray is not JSON: {e}")))?;
        if let Some(format) = meta.get("zarr_format").and_then(Value::as_u64)
            && format != 2
        {
            return Err(CoreError::Parse(format!(
                "this document states zarr_format {format}, not 2"
            )));
        }
        let dtype =
            DType::parse_v2(meta.get("dtype").and_then(Value::as_str).ok_or_else(|| {
                CoreError::Parse("Zarr v2 .zarray states no `dtype`".to_string())
            })?)?;
        let chunk_shape = shard::read_shape(meta.get("chunks"), "Zarr v2 `chunks`")?;

        let mut steps = Vec::new();
        // Fortran order is the innermost stage: the elements were laid out that
        // way before any filter saw them.
        match meta.get("order").and_then(Value::as_str) {
            None | Some("C") => {}
            Some("F") => steps.push(Codec::Transpose {
                order: (0..chunk_shape.len()).rev().collect(),
            }),
            Some(other) => {
                return Err(CoreError::Parse(format!(
                    "Zarr v2 .zarray states order {other:?}, which is neither C nor F"
                )));
            }
        }
        let rest = CodecChain::from_v2(meta.get("compressor"), meta.get("filters"), dtype.size)?;
        steps.extend(rest.steps().iter().cloned());

        Ok(Self {
            dtype,
            chunk_shape,
            chain: CodecChain::new(steps),
            fill_value: meta.get("fill_value").and_then(Value::as_f64),
        })
    }

    /// Read a Zarr v3 `zarr.json` document for an array.
    pub fn from_v3_metadata(json: &str) -> Result<Self, CoreError> {
        let meta: Value = serde_json::from_str(json)
            .map_err(|e| CoreError::Parse(format!("Zarr v3 zarr.json is not JSON: {e}")))?;
        if let Some(format) = meta.get("zarr_format").and_then(Value::as_u64)
            && format != 3
        {
            return Err(CoreError::Parse(format!(
                "this document states zarr_format {format}, not 3"
            )));
        }
        if let Some(node) = meta.get("node_type").and_then(Value::as_str)
            && node != "array"
        {
            return Err(CoreError::WrongLayout(format!(
                "this zarr.json describes a {node}, not an array"
            )));
        }
        let dtype = DType::parse_v3(meta.get("data_type").and_then(Value::as_str).ok_or_else(
            || CoreError::Parse("Zarr v3 zarr.json states no `data_type`".to_string()),
        )?)?;

        let grid = meta.get("chunk_grid").ok_or_else(|| {
            CoreError::Parse("Zarr v3 zarr.json states no `chunk_grid`".to_string())
        })?;
        match grid.get("name").and_then(Value::as_str) {
            Some("regular") => {}
            Some(other) => {
                return Err(CoreError::UnsupportedSection(format!(
                    "Zarr v3 chunk grid {other:?} is not decoded (only `regular` is)"
                )));
            }
            None => {
                return Err(CoreError::Parse(
                    "Zarr v3 `chunk_grid` states no name".to_string(),
                ));
            }
        }
        let chunk_shape = shard::read_shape(
            grid.get("configuration").and_then(|c| c.get("chunk_shape")),
            "Zarr v3 `chunk_shape`",
        )?;

        let chain = CodecChain::from_v3(
            meta.get("codecs").ok_or_else(|| {
                CoreError::Parse("Zarr v3 zarr.json states no `codecs`".to_string())
            })?,
            dtype,
        )?;
        // A multi-byte element type has no order until the `bytes` codec gives
        // it one, and a chain without that codec cannot say which order its
        // bytes are in. Defaulting to little would be right nearly always,
        // which is what makes it a bad default.
        let dtype = match (dtype.size > 1, chain.declared_endian()) {
            (true, None) => {
                return Err(CoreError::Parse(
                    "a Zarr v3 array of a multi-byte type states no `bytes` codec, so \
                     nothing states the byte order its elements were written in"
                        .to_string(),
                ));
            }
            (_, Some(endian)) => dtype.with_endian(endian),
            (false, None) => dtype,
        };

        Ok(Self {
            dtype,
            chunk_shape,
            chain,
            fill_value: meta.get("fill_value").and_then(Value::as_f64),
        })
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
    /// `None` covers both "no fill value" and one this crate does not read as a
    /// number — v2 spells a NaN as the string `"NaN"`, and a structured dtype's
    /// fill value is a base64 blob. Neither is a number, and neither is
    /// invented here.
    #[must_use]
    pub fn fill_value(&self) -> Option<f64> {
        self.fill_value
    }

    /// How many bytes one whole chunk decodes to.
    fn chunk_bytes(&self) -> Result<usize, CoreError> {
        self.chunk_shape
            .iter()
            .try_fold(self.dtype.size, |acc, &n| acc.checked_mul(n))
            .ok_or_else(|| {
                CoreError::Parse(format!(
                    "a chunk of {:?} {}-byte elements overflows an address",
                    self.chunk_shape, self.dtype.size
                ))
            })
    }

    /// Decode one stored chunk to its raw element bytes, in C order.
    ///
    /// The bytes are in the array's stored byte order; [`Self::dtype`] says
    /// which. Use [`Self::decode_values`] to get numbers.
    pub fn decode(&self, stored: &[u8]) -> Result<Vec<u8>, CoreError> {
        self.chain.decode(
            stored,
            self.chunk_bytes()?,
            &self.chunk_shape,
            self.dtype.size,
        )
    }

    /// Decode one stored chunk to numbers, in C order.
    pub fn decode_values(&self, stored: &[u8]) -> Result<Vec<f64>, CoreError> {
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
    pub fn decode_shard(&self, stored: &[u8]) -> Result<Shard, CoreError> {
        let sharding = self.chain.sharding().ok_or_else(|| {
            CoreError::WrongLayout(
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
