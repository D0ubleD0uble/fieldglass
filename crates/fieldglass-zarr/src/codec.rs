//! The codec chain: what a stored chunk has to be run through, backwards, to
//! become an array again.
//!
//! The two editions state the chain differently and mean the same thing.
//!
//! * **v2** splits it in two. `filters` is a list applied to the array before
//!   serialisation, and `compressor` is a single codec applied to the bytes
//!   afterwards. Together they are one ordered pipeline: filters, then the
//!   compressor.
//! * **v3** states one `codecs` list, and gives each codec a declared kind —
//!   array→array (`transpose`), array→bytes (`bytes`, `sharding_indexed`), and
//!   bytes→bytes (`blosc`, `gzip`, `zstd`, `crc32c`). The list is in the same
//!   order the encoder applied them.
//!
//! Decoding is the list reversed. Nothing here re-derives a codec's parameters
//! from the array metadata: blosc's container states its own element size,
//! block size and inner compressor, so a `.zarray` whose `compressor.cname`
//! disagrees with the bytes is the bytes' word that counts.
//!
//! **This crate reads the codec half of the metadata and nothing else.** Which
//! chunk key covers which region of the array is
//! [`fieldglass-fetchplan`](https://docs.rs/fieldglass-fetchplan)'s question,
//! and walking a store to find the keys is a host's. The chunk *shape* is read
//! here because decoding needs it — a transpose has to know the axes it is
//! permuting, and the number of elements a chunk should hold is the only
//! statement of how long the decode must come out.

use crate::dtype::{DType, Endian};
use crate::shard::Sharding;
use crate::{blosc, crc32c, deflate, lz4, shuffle, zstd};
use fieldglass_core::FieldglassError;
use serde_json::Value;

/// One stage of a chain, in the direction the encoder applied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    /// The blosc meta-compressor. Self-describing: its own header states the
    /// element size, the inner compressor and which shuffle ran, so the
    /// configuration in the metadata is not consulted.
    Blosc,
    /// An RFC 1950 zlib stream — the numcodecs `zlib` codec.
    Zlib,
    /// An RFC 1952 gzip member — the numcodecs `gzip` codec and v3's `gzip`.
    Gzip,
    /// A zstd frame.
    Zstd,
    /// The numcodecs `lz4` codec: a four-byte little-endian length, then an
    /// LZ4 block.
    Lz4,
    /// A trailing CRC-32C over everything before it.
    Crc32c,
    /// The byte-transposition filter, over elements of this many bytes.
    Shuffle {
        /// The element width the transpose groups by.
        element_size: usize,
    },
    /// v3's array→bytes codec. Carries no transformation of its own beyond
    /// stating the byte order the elements were written in.
    Bytes {
        /// The stored byte order, or `None` for a one-byte element type, where
        /// the codec's `endian` is absent.
        endian: Option<Endian>,
    },
    /// An axis permutation applied before serialisation. `order[k]` is the
    /// array axis that became stored axis `k`.
    Transpose {
        /// The permutation, as long as the array has dimensions.
        order: Vec<usize>,
    },
    /// v3's `sharding_indexed`: many inner chunks in one stored object, with an
    /// index saying where each is. Terminal — nothing wraps it, and a shard is
    /// decoded through [`crate::ChunkDecoder::decode_shard`] rather than as a
    /// plain chunk.
    Sharding(Box<Sharding>),
}

impl Codec {
    /// Parse one v2 codec object: `{"id": "...", ...}`.
    ///
    /// `element_size` is the array's own element width, which the `shuffle`
    /// filter falls back to when it states none.
    pub(crate) fn from_v2(value: &Value, element_size: usize) -> Result<Self, FieldglassError> {
        let id = value.get("id").and_then(Value::as_str).ok_or_else(|| {
            FieldglassError::Parse(
                "a Zarr v2 codec object states no `id`, so nothing names what it is".to_string(),
            )
        })?;
        match id {
            "blosc" => Ok(Self::Blosc),
            "zlib" => Ok(Self::Zlib),
            "gzip" => Ok(Self::Gzip),
            "zstd" => Ok(Self::Zstd),
            "lz4" => Ok(Self::Lz4),
            "shuffle" => Ok(Self::Shuffle {
                element_size: value
                    .get("elementsize")
                    .and_then(Value::as_u64)
                    .and_then(|n| usize::try_from(n).ok())
                    .unwrap_or(element_size),
            }),
            other => Err(unsupported(other, "numcodecs id")),
        }
    }

    /// Parse one v3 codec object: `{"name": "...", "configuration": {...}}`.
    pub(crate) fn from_v3(value: &Value, dtype: DType) -> Result<Self, FieldglassError> {
        let name = value.get("name").and_then(Value::as_str).ok_or_else(|| {
            FieldglassError::Parse(
                "a Zarr v3 codec object states no `name`, so nothing names what it is".to_string(),
            )
        })?;
        let config = value.get("configuration");
        match name {
            "blosc" => Ok(Self::Blosc),
            "gzip" => Ok(Self::Gzip),
            "zstd" => Ok(Self::Zstd),
            "crc32c" => Ok(Self::Crc32c),
            "bytes" => {
                let endian = match config.and_then(|c| c.get("endian")).and_then(Value::as_str) {
                    // The spec's default, and what zarr-python writes when it
                    // writes nothing.
                    None | Some("little") => Endian::Little,
                    Some("big") => Endian::Big,
                    Some(other) => {
                        return Err(FieldglassError::Parse(format!(
                            "Zarr v3 `bytes` codec states endian {other:?}"
                        )));
                    }
                };
                Ok(Self::Bytes {
                    endian: (dtype.size > 1).then_some(endian),
                })
            }
            "transpose" => {
                let order = config
                    .and_then(|c| c.get("order"))
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        FieldglassError::Parse(
                            "Zarr v3 `transpose` codec states no `order`".to_string(),
                        )
                    })?
                    .iter()
                    .map(|v| {
                        v.as_u64()
                            .and_then(|n| usize::try_from(n).ok())
                            .ok_or_else(|| {
                                FieldglassError::Parse(
                                    "Zarr v3 `transpose` order holds a value that is not an \
                                     axis number"
                                        .to_string(),
                                )
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::Transpose { order })
            }
            "sharding_indexed" => Ok(Self::Sharding(Box::new(Sharding::from_v3(
                config.ok_or_else(|| {
                    FieldglassError::Parse(
                        "Zarr v3 `sharding_indexed` codec states no configuration".to_string(),
                    )
                })?,
                dtype,
            )?))),
            other => Err(unsupported(other, "v3 codec name")),
        }
    }
}

fn unsupported(name: &str, kind: &str) -> FieldglassError {
    FieldglassError::UnsupportedSection(format!(
        "Zarr {kind} {name:?} is not decoded (blosc, zlib, gzip, zstd, lz4, shuffle, bytes, \
         transpose, crc32c and sharding_indexed are)"
    ))
}

/// An ordered pipeline, held in the direction the encoder applied it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodecChain {
    steps: Vec<Codec>,
}

impl CodecChain {
    /// A chain from an explicit list, in encode order.
    #[must_use]
    pub fn new(steps: Vec<Codec>) -> Self {
        Self { steps }
    }

    /// The stages, in the order the encoder applied them.
    #[must_use]
    pub fn steps(&self) -> &[Codec] {
        &self.steps
    }

    /// The `sharding_indexed` codec at the end of this chain, if there is one.
    #[must_use]
    pub fn sharding(&self) -> Option<&Sharding> {
        match self.steps.last() {
            Some(Codec::Sharding(spec)) => Some(spec),
            _ => None,
        }
    }

    /// Read a v2 array's `filters` then `compressor` as one pipeline.
    pub(crate) fn from_v2(
        compressor: Option<&Value>,
        filters: Option<&Value>,
        element_size: usize,
    ) -> Result<Self, FieldglassError> {
        let mut steps = Vec::new();
        // Filters run first on encode, so they are undone last on decode.
        if let Some(list) = filters.filter(|v| !v.is_null()) {
            let list = list.as_array().ok_or_else(|| {
                FieldglassError::Parse("Zarr v2 `filters` is not a list".to_string())
            })?;
            for filter in list {
                steps.push(Codec::from_v2(filter, element_size)?);
            }
        }
        if let Some(codec) = compressor.filter(|v| !v.is_null()) {
            steps.push(Codec::from_v2(codec, element_size)?);
        }
        Ok(Self { steps })
    }

    /// Read a v3 array's `codecs` list.
    pub(crate) fn from_v3(codecs: &Value, dtype: DType) -> Result<Self, FieldglassError> {
        let list = codecs
            .as_array()
            .ok_or_else(|| FieldglassError::Parse("Zarr v3 `codecs` is not a list".to_string()))?;
        let steps = list
            .iter()
            .map(|codec| Codec::from_v3(codec, dtype))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { steps })
    }

    /// The byte order the chain's `bytes` codec states, if it has one.
    ///
    /// A sharded array's own chain holds only `sharding_indexed`; the elements
    /// live in the inner chunks, so the `bytes` codec is in the *inner* chain
    /// and the order is read from there. Missing that is not a subtle failure —
    /// nothing states the order at all, and every sharded array is refused.
    pub(crate) fn declared_endian(&self) -> Option<Endian> {
        self.steps.iter().find_map(|step| match step {
            Codec::Bytes { endian } => *endian,
            Codec::Sharding(spec) => spec.inner_endian(),
            _ => None,
        })
    }

    /// Run the chain backwards over one stored chunk.
    ///
    /// `expected` is how many bytes the chunk must come out as — the element
    /// count times the element width — and is the ceiling every decompression
    /// step is bounded by. A compressed chunk is attacker-controlled and its
    /// ratio unbounded, so without a ceiling a few hundred bytes on disk can
    /// name an arbitrary allocation. Taking it from the array's own geometry
    /// makes the bound exact rather than a guessed constant.
    pub(crate) fn decode(
        &self,
        stored: &[u8],
        expected: usize,
        shape: &[usize],
        element_size: usize,
    ) -> Result<Vec<u8>, FieldglassError> {
        let mut data = stored.to_vec();
        for step in self.steps.iter().rev() {
            data = match step {
                Codec::Blosc => blosc::decompress(&data, expected)?,
                Codec::Zlib => deflate::inflate_zlib(&data, expected)?,
                Codec::Gzip => deflate::inflate_gzip(&data, expected)?,
                Codec::Zstd => zstd::decompress(&data, expected)?,
                Codec::Lz4 => lz4::decompress_numcodecs(&data, expected)?,
                Codec::Crc32c => crc32c::verify_and_strip(&data)?,
                Codec::Shuffle { element_size } => shuffle::unshuffle(&data, *element_size),
                // The byte order it states is carried on the element type, not
                // applied to the buffer: the values are read in that order
                // rather than swapped in place.
                Codec::Bytes { .. } => data,
                Codec::Transpose { order } => untranspose(&data, shape, order, element_size)?,
                Codec::Sharding(_) => {
                    return Err(FieldglassError::WrongLayout(
                        "this array's chunks are shards; decode them with `decode_shard`, \
                         which reads the index that says where each inner chunk is"
                            .to_string(),
                    ));
                }
            };
        }
        if data.len() != expected {
            return Err(FieldglassError::Parse(format!(
                "chunk decoded to {} bytes, not the {expected} its shape and element type \
                 require",
                data.len()
            )));
        }
        Ok(data)
    }
}

/// Undo an axis permutation: the stored array has shape `shape[order]`, and
/// this returns it in the array's own axis order.
///
/// C order throughout — last axis fastest — in both the stored and the returned
/// layout, because that is what both editions serialise and what every consumer
/// downstream indexes.
fn untranspose(
    data: &[u8],
    shape: &[usize],
    order: &[usize],
    element_size: usize,
) -> Result<Vec<u8>, FieldglassError> {
    if order.len() != shape.len() {
        return Err(FieldglassError::Parse(format!(
            "a transpose order of {} axes cannot apply to a {}-dimensional chunk",
            order.len(),
            shape.len()
        )));
    }
    let mut seen = vec![false; shape.len()];
    for &axis in order {
        let slot = seen.get_mut(axis).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "a transpose order names axis {axis} of a {}-dimensional chunk",
                shape.len()
            ))
        })?;
        if std::mem::replace(slot, true) {
            return Err(FieldglassError::Parse(format!(
                "a transpose order names axis {axis} twice, so it is not a permutation"
            )));
        }
    }

    let stored_shape: Vec<usize> = order.iter().map(|&axis| shape[axis]).collect();
    // Strides over the *stored* layout, in elements, last axis fastest.
    let mut stored_strides = vec![1usize; stored_shape.len()];
    for axis in (0..stored_shape.len().saturating_sub(1)).rev() {
        stored_strides[axis] = stored_strides[axis + 1] * stored_shape[axis + 1];
    }

    let count: usize = shape.iter().product();
    let mut out = vec![0u8; count * element_size];
    let mut index = vec![0usize; shape.len()];
    for target in 0..count {
        // `index` walks the array's own axes in C order; the source element is
        // at the same coordinates read through the permutation.
        let source: usize = order
            .iter()
            .enumerate()
            .map(|(stored_axis, &array_axis)| index[array_axis] * stored_strides[stored_axis])
            .sum();
        let (from, to) = (source * element_size, target * element_size);
        out.get_mut(to..to + element_size)
            .zip(data.get(from..from + element_size))
            .map(|(dst, src)| dst.copy_from_slice(src))
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "a transposed chunk of {} bytes cannot hold {count} elements of \
                     {element_size} bytes",
                    data.len()
                ))
            })?;
        // Odometer increment over the array's own axes.
        for axis in (0..shape.len()).rev() {
            index[axis] += 1;
            if index[axis] < shape[axis] {
                break;
            }
            index[axis] = 0;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dtype::ScalarKind;
    use serde_json::json;

    fn f32_dtype() -> DType {
        DType {
            kind: ScalarKind::Float,
            size: 4,
            endian: Some(Endian::Little),
        }
    }

    /// The v2 halves compose into one pipeline in encode order: filters first,
    /// compressor last. Reversing that would run the compressor's output
    /// through a decompressor.
    #[test]
    fn a_v2_chain_puts_the_filters_before_the_compressor() {
        let chain = CodecChain::from_v2(
            Some(&json!({"id": "zstd", "level": 3})),
            Some(&json!([{"id": "shuffle", "elementsize": 4}])),
            4,
        )
        .unwrap();
        assert_eq!(
            chain.steps(),
            &[Codec::Shuffle { element_size: 4 }, Codec::Zstd]
        );
    }

    /// A `shuffle` filter that states no element size takes the array's.
    #[test]
    fn a_shuffle_filter_falls_back_to_the_arrays_element_size() {
        let chain = CodecChain::from_v2(None, Some(&json!([{"id": "shuffle"}])), 8).unwrap();
        assert_eq!(chain.steps(), &[Codec::Shuffle { element_size: 8 }]);
    }

    /// A null compressor is "no compression", not a malformed document.
    #[test]
    fn a_null_compressor_is_an_empty_chain() {
        let chain = CodecChain::from_v2(Some(&Value::Null), Some(&Value::Null), 4).unwrap();
        assert!(chain.steps().is_empty());
    }

    /// The v3 `bytes` codec is where a v3 array's byte order comes from, and
    /// it is absent for a one-byte element type.
    #[test]
    fn the_bytes_codec_carries_the_byte_order() {
        let chain = CodecChain::from_v3(
            &json!([{"name": "bytes", "configuration": {"endian": "big"}}]),
            f32_dtype(),
        )
        .unwrap();
        assert_eq!(chain.declared_endian(), Some(Endian::Big));

        // No configuration at all: the spec's default is little.
        let defaulted = CodecChain::from_v3(&json!([{"name": "bytes"}]), f32_dtype()).unwrap();
        assert_eq!(defaulted.declared_endian(), Some(Endian::Little));

        let one_byte = CodecChain::from_v3(
            &json!([{"name": "bytes", "configuration": {"endian": "big"}}]),
            DType {
                kind: ScalarKind::Uint,
                size: 1,
                endian: None,
            },
        )
        .unwrap();
        assert_eq!(one_byte.declared_endian(), None);
    }

    #[test]
    fn refuses_a_codec_it_cannot_reverse() {
        let err = CodecChain::from_v2(Some(&json!({"id": "bz2"})), None, 4).unwrap_err();
        assert!(matches!(err, FieldglassError::UnsupportedSection(_)));
        let err = CodecChain::from_v3(&json!([{"name": "vlen-utf8"}]), f32_dtype()).unwrap_err();
        assert!(matches!(err, FieldglassError::UnsupportedSection(_)));
        // A codec object with no name at all.
        assert!(CodecChain::from_v3(&json!([{}]), f32_dtype()).is_err());
        assert!(CodecChain::from_v2(Some(&json!({})), None, 4).is_err());
    }

    /// A transpose really moves elements, and the inverse is the one that
    /// restores them — not the permutation itself, which is only its own
    /// inverse for a swap of two axes.
    #[test]
    fn untranspose_restores_a_three_axis_permutation() {
        // Shape (2, 3, 4), elements numbered in C order.
        let shape = [2usize, 3, 4];
        let count: usize = shape.iter().product();
        let original: Vec<u8> = (0..count as u8).collect();
        // order = [2, 0, 1]: stored axis 0 is array axis 2, and so on.
        let order = [2usize, 0, 1];
        let stored_shape = [4usize, 2, 3];

        // Build what the encoder would have written.
        let mut stored = vec![0u8; count];
        for i in 0..shape[0] {
            for j in 0..shape[1] {
                for k in 0..shape[2] {
                    let from = (i * shape[1] + j) * shape[2] + k;
                    let to = (k * stored_shape[1] + i) * stored_shape[2] + j;
                    stored[to] = original[from];
                }
            }
        }
        assert_ne!(stored, original, "the permutation must actually move bytes");
        assert_eq!(untranspose(&stored, &shape, &order, 1).unwrap(), original);
    }

    /// Fortran order is the reversed-axes transpose, which is how a v2 array
    /// with `order: "F"` is undone.
    #[test]
    fn untranspose_handles_the_fortran_order_case() {
        // Shape (2, 3) in Fortran order: columns first.
        let stored = [0u8, 3, 1, 4, 2, 5];
        assert_eq!(
            untranspose(&stored, &[2, 3], &[1, 0], 1).unwrap(),
            vec![0, 1, 2, 3, 4, 5]
        );
    }

    #[test]
    fn untranspose_refuses_an_order_that_is_not_a_permutation() {
        assert!(untranspose(&[0u8; 6], &[2, 3], &[0, 0], 1).is_err());
        assert!(untranspose(&[0u8; 6], &[2, 3], &[0, 2], 1).is_err());
        assert!(untranspose(&[0u8; 6], &[2, 3], &[0], 1).is_err());
        // A buffer too short for the shape must not index out of bounds.
        assert!(untranspose(&[0u8; 3], &[2, 3], &[1, 0], 1).is_err());
    }

    /// The chain refuses to decode a shard as a chunk rather than returning
    /// the index bytes as if they were data.
    #[test]
    fn a_sharded_chain_refuses_a_plain_chunk_decode() {
        let chain = CodecChain::from_v3(
            &json!([{
                "name": "sharding_indexed",
                "configuration": {
                    "chunk_shape": [2, 3],
                    "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
                    "index_codecs": [
                        {"name": "bytes", "configuration": {"endian": "little"}},
                        {"name": "crc32c"}
                    ]
                }
            }]),
            f32_dtype(),
        )
        .unwrap();
        assert!(chain.sharding().is_some());
        let err = chain.decode(&[0u8; 16], 24, &[2, 3], 4).unwrap_err();
        assert!(
            matches!(err, FieldglassError::WrongLayout(_)),
            "got {err:?}"
        );
    }
}
