//! The codec chain: what a stored chunk has to be run through, backwards, to
//! become an array again.
//!
//! The two editions state the chain differently and mean the same thing.
//!
//! * **v2** splits it in two. `filters` is a list applied to the array before
//!   serialization, and `compressor` is a single codec applied to the bytes
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
use fieldglass_core::output_limits;
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
    /// An axis permutation applied before serialization. `order[k]` is the
    /// array axis that became stored axis `k`.
    Transpose {
        /// The permutation, as long as the array has dimensions.
        order: Vec<usize>,
    },
    /// v3's `sharding_indexed`: many inner chunks in one stored object, with an
    /// index saying where each is. A shard is decoded through
    /// [`crate::ChunkDecoder::decode_shard`] rather than as a plain chunk.
    ///
    /// It is an array→bytes codec, so the spec lets codecs stand on both
    /// sides of it:
    ///
    /// - **Before it**, array→array codecs. `transpose` is the one this crate
    ///   reads: the shard's inner grid, and each inner chunk, are laid out in
    ///   the transposed axis order, and a read puts the values back in the
    ///   array's own (#1343). Any other codec before it is refused when the
    ///   metadata is read.
    /// - **After it**, bytes→bytes codecs over the whole shard, such as a
    ///   `gzip`. The array is then not read as sharded
    ///   ([`CodecChain::sharding`] is `None`), and its chunks fail to decode
    ///   as plain ones (#1303).
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
    ///
    /// Only a sharding codec with nothing after it makes the array's chunks
    /// shards a read can index: see [`Codec::Sharding`].
    #[must_use]
    pub fn sharding(&self) -> Option<&Sharding> {
        match self.split_at_sharding() {
            Some((_, spec, [])) => Some(spec),
            _ => None,
        }
    }

    /// The chain split around its last `sharding_indexed` codec: the steps
    /// the encoder applied before it, the codec itself, and the steps after
    /// it, all in encode order. `None` for a chain with no sharding codec.
    #[must_use]
    pub fn split_at_sharding(&self) -> Option<(&[Codec], &Sharding, &[Codec])> {
        let at = self
            .steps
            .iter()
            .rposition(|step| matches!(step, Codec::Sharding(_)))?;
        let Codec::Sharding(spec) = &self.steps[at] else {
            return None;
        };
        Some((&self.steps[..at], spec, &self.steps[at + 1..]))
    }

    /// The one axis permutation the steps before this chain's sharding codec
    /// apply to a shard of `rank` dimensions (#1343): `order[k]` is the array
    /// axis that became the shard's stored axis `k`, as for
    /// [`Codec::Transpose`]. `None` when the shard is stored in the array's
    /// own axis order, which includes a chain that is not sharded.
    ///
    /// This is the route between the array's axis order and the one the
    /// public [`ShardLayout`](crate::ShardLayout) and
    /// [`Sharding`] methods work in. Their `outer_shape` is
    /// the array's chunk shape permuted into stored order,
    /// `outer[k] = chunk_shape[order[k]]`, which
    /// [`StoredShape::new`](crate::StoredShape::new) builds from this chain
    /// (#1368); the inner shape, inner grid and an
    /// inner chunk's bytes they return are in stored order, and `order` maps
    /// each stored axis back to the array's.
    ///
    /// Several transposes compose into one. Allocates only when there is a
    /// transpose to compose, so a plain sharded read pays nothing for it.
    ///
    /// # Errors
    ///
    /// A codec before the sharding codec that is not a `transpose`, which is
    /// the only array→array codec this crate reads, and an order that is not
    /// a permutation of `rank` axes.
    pub fn shard_order(&self, rank: usize) -> Result<Option<Vec<usize>>, FieldglassError> {
        match self.split_at_sharding() {
            Some((before, _, _)) => compose_transposes(before, rank),
            None => Ok(None),
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
    /// count times the element width. A compressed chunk is attacker-controlled
    /// and its ratio unbounded, so without a ceiling a few hundred bytes on
    /// disk can name an arbitrary allocation. Each decompression step is held
    /// to the most its own output can be ([`Self::ceilings`]), which is
    /// `expected` itself unless a codec ahead of it in the chain grows the
    /// bytes, and the result must be exactly `expected` at the end.
    pub(crate) fn decode(
        &self,
        stored: &[u8],
        expected: usize,
        shape: &[usize],
        element_size: usize,
    ) -> Result<Vec<u8>, FieldglassError> {
        // On the stack for any real chain: an allocation per chunk shows in
        // every Zarr row of the perf gate.
        let mut inline = [0; INLINE_STEPS];
        let mut spilled = Vec::new();
        let ceilings = if self.steps.len() <= INLINE_STEPS {
            &mut inline[..self.steps.len()]
        } else {
            spilled.resize(self.steps.len(), 0);
            &mut spilled[..]
        };
        self.ceilings(expected, ceilings);
        let mut data = stored.to_vec();
        for (step, &limit) in self.steps.iter().zip(ceilings.iter()).rev() {
            data = match step {
                Codec::Blosc => blosc::decompress(&data, limit)?,
                Codec::Zlib => deflate::inflate_zlib(&data, limit)?,
                Codec::Gzip => deflate::inflate_gzip(&data, limit)?,
                Codec::Zstd => zstd::decompress(&data, limit)?,
                Codec::Lz4 => lz4::decompress_numcodecs(&data, limit)?,
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

    /// Fill `out`, one entry per step, with the most each step may decode to,
    /// in encode order: what the encoder could have handed it (#1012).
    ///
    /// v3 lets bytes→bytes codecs come in any order, so a step's input is not
    /// always the chunk. Behind `[bytes, crc32c, zstd]`, zstd decodes to the
    /// chunk and its 4-byte checksum; behind `[bytes, zstd, gzip]`, gzip
    /// decodes to a zstd frame, which on data that does not compress is longer
    /// than the chunk. So the chain is walked forward from `expected`, each
    /// step growing the length by its worst case as an encoder
    /// ([`Codec::growth`]).
    ///
    /// The growth compounds, and the chain is the metadata's to choose: a few
    /// hundred `gzip` entries would lift the ceiling past any allocation. So no
    /// ceiling is more than twice `expected` and [`CHAIN_SLACK`] bytes, which
    /// holds any real chain: on data that does not compress a compressor adds
    /// well under 1% and a few dozen bytes of framing.
    fn ceilings(&self, expected: usize, out: &mut [usize]) {
        let most = expected.saturating_mul(2).saturating_add(CHAIN_SLACK);
        let mut len = expected as u64;
        for (step, ceiling) in self.steps.iter().zip(out) {
            *ceiling = len.min(most as u64) as usize;
            len = step.growth(len);
        }
    }
}

/// The longest chain whose ceilings [`CodecChain::decode`] holds on the
/// stack. numcodecs and zarr-python chains are a handful of steps; a longer
/// one still decodes, with its ceilings on the heap.
const INLINE_STEPS: usize = 8;

/// The room a chain's ceilings have past twice the chunk ([`CodecChain::ceilings`]):
/// the fixed framing of many codecs on a chunk too small for twice its length
/// to hold them. zstd's margin is the largest, at most 64 bytes.
const CHAIN_SLACK: usize = 4096;

impl Codec {
    /// The codec's name as the metadata spells it, for a refusal.
    fn name(&self) -> &'static str {
        match self {
            Self::Blosc => "`blosc`",
            Self::Zlib => "`zlib`",
            Self::Gzip => "`gzip`",
            Self::Zstd => "`zstd`",
            Self::Lz4 => "`lz4`",
            Self::Crc32c => "`crc32c`",
            Self::Shuffle { .. } => "`shuffle`",
            Self::Bytes { .. } => "`bytes`",
            Self::Transpose { .. } => "`transpose`",
            Self::Sharding(_) => "`sharding_indexed`",
        }
    }

    /// The most this codec writes when handed `len` bytes on encode.
    ///
    /// - `crc32c` appends its 4-byte checksum.
    /// - `shuffle`, `bytes` and `transpose` reorder and keep the length.
    /// - `blosc` falls back to copying a buffer it cannot shrink, behind its
    ///   16-byte header: `BLOSC_MAX_OVERHEAD` in `blosc.h`.
    /// - `zlib` and `gzip` are zlib's `deflateBound`
    ///   ([`output_limits::deflate_compress_bound`]) and their wrapper: 6 bytes for
    ///   zlib (RFC 1950 §2.2), 18 for a gzip member with no optional header
    ///   fields (RFC 1952 §2.3), which is what numcodecs writes.
    /// - `zstd` is `ZSTD_COMPRESSBOUND` ([`output_limits::zstd_compress_bound`]).
    /// - numcodecs `lz4` is `LZ4_COMPRESSBOUND`, `len + len/255 + 16`, behind
    ///   its 4-byte length.
    /// - A shard is never decoded as a plain chunk, so its growth is moot.
    fn growth(&self, len: u64) -> u64 {
        match self {
            Self::Crc32c => len.saturating_add(4),
            Self::Shuffle { .. } | Self::Bytes { .. } | Self::Transpose { .. } => len,
            Self::Blosc => len.saturating_add(16),
            Self::Zlib => output_limits::deflate_compress_bound(len).saturating_add(6),
            Self::Gzip => output_limits::deflate_compress_bound(len).saturating_add(18),
            Self::Zstd => output_limits::zstd_compress_bound(len),
            Self::Lz4 => len.saturating_add(len / 255).saturating_add(16 + 4),
            Self::Sharding(_) => len,
        }
    }
}

/// The steps before a `sharding_indexed` codec, `before`, composed into one
/// axis permutation of `rank` axes: see [`CodecChain::shard_order`].
pub(crate) fn compose_transposes(
    before: &[Codec],
    rank: usize,
) -> Result<Option<Vec<usize>>, FieldglassError> {
    let mut composed: Option<Vec<usize>> = None;
    for step in before {
        let Codec::Transpose { order } = step else {
            return Err(FieldglassError::UnsupportedSection(format!(
                "a Zarr v3 array states codec {} before `sharding_indexed`; only \
                 `transpose` is read there",
                step.name()
            )));
        };
        check_permutation(order, rank)?;
        // The earlier steps' stored axis `order[k]` became this step's stored
        // axis `k`, so the array axis behind it is the earlier composition's
        // entry there.
        composed = Some(match composed {
            None => order.clone(),
            Some(earlier) => order.iter().map(|&axis| earlier[axis]).collect(),
        });
    }
    // The identity, spelled out, is no permutation at all.
    Ok(composed.filter(|order| order.iter().enumerate().any(|(k, &axis)| k != axis)))
}

/// Refuse a transpose `order` that is not a permutation of `rank` axes.
fn check_permutation(order: &[usize], rank: usize) -> Result<(), FieldglassError> {
    if order.len() != rank {
        return Err(FieldglassError::Parse(format!(
            "a transpose order of {} axes cannot apply to a {rank}-dimensional chunk",
            order.len()
        )));
    }
    let mut seen = vec![false; rank];
    for &axis in order {
        let slot = seen.get_mut(axis).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "a transpose order names axis {axis} of a {rank}-dimensional chunk"
            ))
        })?;
        if std::mem::replace(slot, true) {
            return Err(FieldglassError::Parse(format!(
                "a transpose order names axis {axis} twice, so it is not a permutation"
            )));
        }
    }
    Ok(())
}

/// Undo an axis permutation: the stored array has shape `shape[order]`, and
/// this returns it in the array's own axis order.
///
/// C order throughout — last axis fastest — in both the stored and the returned
/// layout, because that is what both editions serialize and what every consumer
/// downstream indexes.
pub(crate) fn untranspose(
    data: &[u8],
    shape: &[usize],
    order: &[usize],
    element_size: usize,
) -> Result<Vec<u8>, FieldglassError> {
    check_permutation(order, shape.len())?;

    let stored_shape: Vec<usize> = order.iter().map(|&axis| shape[axis]).collect();
    // Strides over the *stored* layout, in elements, last axis fastest.
    let mut stored_strides = vec![1usize; stored_shape.len()];
    for axis in (0..stored_shape.len().saturating_sub(1)).rev() {
        stored_strides[axis] = stored_strides[axis + 1] * stored_shape[axis + 1];
    }

    // The chunk's shape is the metadata's number, not the data's: a few bytes
    // under a large chunk would otherwise have the whole chunk zeroed before
    // the first copy found them short (#986). A transposed chunk holds every
    // element exactly once, so its length must be the chunk's.
    let count: usize = shape.iter().product();
    let bytes = count.checked_mul(element_size);
    if bytes != Some(data.len()) {
        return Err(FieldglassError::Parse(format!(
            "a transposed chunk of {} bytes cannot hold {count} elements of \
             {element_size} bytes",
            data.len()
        )));
    }
    let mut out = vec![0u8; data.len()];
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

    fn ceilings(chain: &CodecChain, expected: usize) -> Vec<usize> {
        let mut out = vec![0; chain.steps().len()];
        chain.ceilings(expected, &mut out);
        out
    }

    /// Each step is held to what the steps ahead of it in the chain could
    /// have handed it, not to the chunk (#1012).
    #[test]
    fn each_step_is_held_to_its_own_output() {
        let chain = |steps: Vec<Codec>| ceilings(&CodecChain::new(steps), 24);
        let bytes = || Codec::Bytes {
            endian: Some(Endian::Little),
        };
        // The defaults: everything behind the chunk itself.
        assert_eq!(chain(vec![bytes(), Codec::Zstd]), [24, 24]);
        // `crc32c` strips what it is handed, so its own ceiling goes unused.
        assert_eq!(
            chain(vec![bytes(), Codec::Zstd, Codec::Crc32c]),
            [24, 24, 87]
        );
        // zstd decodes to the chunk and its checksum.
        assert_eq!(
            chain(vec![bytes(), Codec::Crc32c, Codec::Zstd]),
            [24, 24, 28]
        );
        // gzip decodes to a zstd frame of up to `ZSTD_COMPRESSBOUND(24)`.
        assert_eq!(
            chain(vec![bytes(), Codec::Zstd, Codec::Gzip]),
            [24, 24, 24 + 63]
        );
        assert_eq!(
            chain(vec![
                Codec::Shuffle { element_size: 4 },
                Codec::Crc32c,
                Codec::Blosc
            ]),
            [24, 24, 28]
        );
        // `LZ4_COMPRESSBOUND(1020)` is 1020 + 4 + 16, behind a 4-byte length.
        assert_eq!(
            ceilings(&CodecChain::new(vec![Codec::Lz4, Codec::Zlib]), 1_020),
            [1_020, 1_044]
        );
    }

    /// The growth compounds, but a chain of any length cannot lift a ceiling
    /// past twice the chunk and the slack.
    #[test]
    fn a_long_chain_cannot_lift_the_ceiling_without_bound() {
        let ceilings = ceilings(&CodecChain::new(vec![Codec::Gzip; 1_000]), 1 << 20);
        assert_eq!(ceilings[0], 1 << 20);
        assert_eq!(ceilings.last(), Some(&((2 << 20) + CHAIN_SLACK)));
        assert!(ceilings.windows(2).all(|w| w[0] <= w[1]));
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

    /// Two transposes before the sharding codec compose in the order the
    /// encoder applied them (#1343). `[2, 0, 1]` then `[1, 0, 2]` do not
    /// commute: applied in that order, stored axis `k` holds array axis
    /// `[0, 2, 1][k]`; the other way round it would be `[2, 1, 0]`.
    ///
    /// Checked by hand: after `[2, 0, 1]` the stored axes hold array axes
    /// (2, 0, 1); `[1, 0, 2]` then takes that tuple's entries 1, 0 and 2,
    /// which are (0, 2, 1).
    #[test]
    fn transposes_before_sharding_compose_in_encode_order() {
        let transpose = |order: Vec<usize>| Codec::Transpose { order };
        let composed = compose_transposes(&[transpose(vec![2, 0, 1]), transpose(vec![1, 0, 2])], 3);
        assert_eq!(composed.unwrap(), Some(vec![0, 2, 1]));
        let reversed = compose_transposes(&[transpose(vec![1, 0, 2]), transpose(vec![2, 0, 1])], 3);
        assert_eq!(reversed.unwrap(), Some(vec![2, 1, 0]));
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
