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

use crate::codec::CodecChain;
use crate::crc32c;
use crate::dtype::{DType, Endian};
use fieldglass_core::FieldglassError;
use serde_json::Value;

/// Bytes one index entry occupies: an offset and a length, both `uint64`.
const ENTRY_LEN: usize = 16;

/// The sentinel both fields carry for an inner chunk that was never written.
const EMPTY: u64 = u64::MAX;

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
    /// The inner chunk shape, which must divide the shard's own chunk shape.
    pub chunk_shape: Vec<usize>,
    /// The chain each inner chunk's bytes were written through.
    inner: CodecChain,
    /// The chain the index itself was written through. Every codec in it must
    /// be fixed-size, which is what makes the index's length computable
    /// without reading it.
    index: CodecChain,
    /// Which end of the object the index is at.
    location: IndexLocation,
}

/// One decoded shard: the inner chunks, in the C order of the inner grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard {
    chunks_per_shard: Vec<usize>,
    chunks: Vec<Option<Vec<u8>>>,
}

impl Shard {
    /// How many inner chunks the grid holds along each axis.
    #[must_use]
    pub fn chunks_per_shard(&self) -> &[usize] {
        &self.chunks_per_shard
    }

    /// One inner chunk's decoded bytes, or `None` when the shard holds no
    /// chunk there and the array's fill value stands in.
    ///
    /// `index` walks the inner grid in C order — last axis fastest — which is
    /// the order the shard index itself is written in.
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
}

impl Sharding {
    /// Read the codec's `configuration` object.
    pub(crate) fn from_v3(config: &Value, dtype: DType) -> Result<Self, FieldglassError> {
        let chunk_shape = read_shape(config.get("chunk_shape"), "sharding_indexed chunk_shape")?;
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
            inner,
            index,
            location,
        })
    }

    /// The byte order the inner chunks' own `bytes` codec states.
    ///
    /// The elements of a sharded array live in its inner chunks, so this is
    /// where its byte order is stated — the array's outer chain holds only this
    /// codec.
    pub(crate) fn inner_endian(&self) -> Option<Endian> {
        self.inner.declared_endian()
    }

    /// How many bytes the index occupies for a grid of `entries` inner chunks.
    ///
    /// Computable without reading the shard precisely because the index chain
    /// may hold only fixed-size codecs: `bytes` changes nothing and `crc32c`
    /// appends four. A variable-size codec there is forbidden by the
    /// specification and is refused rather than guessed at.
    fn index_bytes(&self, entries: usize) -> Result<usize, FieldglassError> {
        let mut size = entries
            .checked_mul(ENTRY_LEN)
            .ok_or_else(|| FieldglassError::Parse("shard index size overflows".to_string()))?;
        for step in self.index.steps() {
            match step {
                crate::codec::Codec::Bytes { .. } => {}
                crate::codec::Codec::Crc32c => size += crc32c::CHECKSUM_LEN,
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

    /// Decode a shard object into its inner chunks.
    ///
    /// `outer_shape` is the array's own chunk shape — the shard's extent —
    /// which must be a whole number of inner chunks along every axis.
    pub(crate) fn decode(
        &self,
        stored: &[u8],
        outer_shape: &[usize],
        element_size: usize,
    ) -> Result<Shard, FieldglassError> {
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
        let entries: usize = chunks_per_shard.iter().product();
        let index_bytes = self.index_bytes(entries)?;

        let raw_index = match self.location {
            IndexLocation::Start => stored.get(..index_bytes),
            IndexLocation::End => stored
                .len()
                .checked_sub(index_bytes)
                .and_then(|s| stored.get(s..)),
        }
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "a shard of {} bytes cannot hold a {index_bytes}-byte index for \
                 {entries} inner chunks",
                stored.len()
            ))
        })?;
        // The index chain, backwards. `expected` is the bare entry array,
        // which is what the chain has to come out as.
        let index = self
            .index
            .decode(raw_index, entries * ENTRY_LEN, &[entries, 2], 8)?;
        let big_endian = self.index.declared_endian() == Some(Endian::Big);

        let inner_elements: usize = self.chunk_shape.iter().product();
        let inner_bytes = inner_elements
            .checked_mul(element_size)
            .ok_or_else(|| FieldglassError::Parse("inner chunk size overflows".to_string()))?;

        let mut chunks = Vec::with_capacity(entries);
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
                chunks.push(None);
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
            let start = usize::try_from(offset).ok();
            let end =
                start.and_then(|s| usize::try_from(length).ok().and_then(|l| s.checked_add(l)));
            let bytes = start
                .zip(end)
                .and_then(|(s, e)| stored.get(s..e))
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "a shard index entry names bytes {offset}..{} of a {}-byte shard",
                        offset.saturating_add(length),
                        stored.len()
                    ))
                })?;
            chunks.push(Some(self.inner.decode(
                bytes,
                inner_bytes,
                &self.chunk_shape,
                element_size,
            )?));
        }

        Ok(Shard {
            chunks_per_shard,
            chunks,
        })
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
        let decoded = spec("start").decode(&shard, &[4, 4], 1).unwrap();
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
        let decoded = spec("end").decode(&shard, &[4, 4], 1).unwrap();
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
        assert_eq!(defaulted.location, IndexLocation::End);
    }

    /// The all-ones sentinel is an absent chunk, and a zero-length chunk at
    /// offset zero is a real one — the two must not be confused.
    #[test]
    fn the_empty_sentinel_is_distinguished_from_a_zero_length_chunk() {
        let body: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6, 7];
        let placements = [(0u64, 4), (EMPTY, EMPTY), (4, 4), (EMPTY, EMPTY)];
        let shard = build_shard(IndexLocation::End, &placements, body);
        let decoded = spec("end").decode(&shard, &[4, 4], 1).unwrap();
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
        assert!(spec("end").decode(&shard, &[4, 4], 1).is_err());
    }

    /// The index's own checksum is checked; a corrupt index is refused rather
    /// than used to slice arbitrary ranges out of the object.
    #[test]
    fn a_corrupt_index_is_refused() {
        let placements = [(0u64, 4), (4, 4), (8, 4), (12, 4)];
        let mut shard = build_shard(IndexLocation::End, &placements, (0..16u8).collect());
        let len = shard.len();
        shard[len - 20] ^= 0xFF;
        let err = spec("end").decode(&shard, &[4, 4], 1).unwrap_err();
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
        assert!(spec("end").decode(&shard, &[4, 4], 1).is_err());
    }

    /// A shard whose extent is not a whole number of inner chunks has no
    /// well-defined grid.
    #[test]
    fn refuses_a_grid_that_does_not_divide() {
        let shard = build_shard(IndexLocation::End, &[(0, 4)], (0..4u8).collect());
        assert!(spec("end").decode(&shard, &[3, 2], 1).is_err());
        assert!(spec("end").decode(&shard, &[4], 1).is_err());
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
        let err = sharding.decode(&[0u8; 128], &[4, 4], 1).unwrap_err();
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
}
