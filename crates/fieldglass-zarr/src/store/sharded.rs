//! A sharded array read by range: each shard's index, then the inner chunks a
//! region touches, and nothing else of the shard (#1123).
//!
//! A shard can be large. dynamical.org's GFS store puts every lead time of a
//! 726 x 726 tile in one, so a window of one step that fetched whole shards
//! would fetch two hundred steps it never shows. The index says where each
//! inner chunk is, so a host fetches it first, by range, and then exactly the
//! inner chunks it needs.
//!
//! Two questions, both synchronous over what the host has fetched
//! (ADR-0005):
//!
//! 1. [`ZarrStore::shard_indexes`]: where each touched shard's index is. Its
//!    length is fixed by the metadata, so the host can fetch it from the
//!    shard's end (or start) without knowing the shard's size.
//! 2. [`ZarrStore::inner_chunk_ranges`]: once those are in hand, the byte
//!    range of each inner chunk the region touches. Inner chunks the index
//!    says were never written are left out: they are the fill value.
//!
//! A region read over a shard given by range then reads exactly those ranges
//! ([`read_ranged`]), and one over a shard given whole reads it as before.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::ChunkGrid;
use fieldglass_core::bytes::{ByteRange, ObjectSource};

use super::{StoredArray, ZarrStore, keys_of};
use crate::codec::{Codec, compose_transposes};
use crate::metadata::CodecSource;
use crate::{ChunkDecoder, IndexLocation, ShardIndex, ShardLayout, StoredShape};
use serde_json::Value;

/// Where one shard's index is: what a host fetches before any inner chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ShardIndexRange {
    /// The shard's position on the array's chunk grid.
    pub index: Vec<u64>,
    /// The shard's key from the store's root.
    pub key: String,
    /// Which end of the shard the index is at.
    pub location: IndexLocation,
    /// The index's length in bytes: the shard's first this many, or its last.
    pub len: u64,
}

/// One inner chunk of a shard, as the bytes a host fetches for it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InnerChunkRange {
    /// The shard's position on the array's chunk grid.
    pub index: Vec<u64>,
    /// The shard's key from the store's root.
    pub key: String,
    /// The inner chunk's position on the shard's grid of inner chunks, as the
    /// shard's index lays it out: in the transposed axis order when a
    /// `transpose` comes before the sharding codec (#1343).
    pub inner: Vec<u64>,
    /// Its bytes within the shard, as the shard's index states them.
    pub range: ByteRange,
}

impl<O: ObjectSource> ZarrStore<O> {
    /// Where the index of each shard a read of `region` touches is, in
    /// row-major order of the shards: `None` for an array that is not
    /// sharded (#1123).
    ///
    /// Metadata only. A host fetches each index as a range of its shard: the
    /// last [`len`](ShardIndexRange::len) bytes (an HTTP suffix range), or the
    /// first, as [`location`](ShardIndexRange::location) says, and gives the
    /// source the shard's whole size with it.
    ///
    /// # Errors
    ///
    /// Those of [`chunk_keys`](Self::chunk_keys), a `sharding_indexed`
    /// configuration that does not parse or whose inner grid does not divide
    /// the shard, and a shard of more than
    /// [`MAX_SHARD_ENTRIES`](crate::MAX_SHARD_ENTRIES) inner chunks. Not an
    /// inner codec this crate does not decode: locating the inner chunks reads
    /// the index, not them.
    pub fn shard_indexes(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Option<Vec<ShardIndexRange>>, FieldglassError> {
        let stored = self.stored(array)?;
        let Some(sharded) = Sharded::of(stored)? else {
            return Ok(None);
        };
        let covering = stored.meta.grid().chunks_covering(region)?;
        let keys = keys_of(stored, &covering)?;
        let len = sharded.layout.index_len(&sharded.shard_shape)?;
        let location = sharded.layout.index_location();
        Ok(Some(
            covering
                .into_iter()
                .zip(keys)
                .map(|(index, key)| ShardIndexRange {
                    index,
                    key,
                    location,
                    len,
                })
                .collect(),
        ))
    }

    /// The byte range of every inner chunk a read of `region` touches, in
    /// row-major order of the shards and then of the inner chunks in each
    /// (#1123).
    ///
    /// Reads each touched shard's index from the source: fetched by range as
    /// [`shard_indexes`](Self::shard_indexes) names it, or within a shard
    /// given whole. An inner chunk the index marks as never written is left
    /// out, and so is every inner chunk of a shard the store does not hold:
    /// both are the fill value.
    ///
    /// `None` for an array that is not sharded, and while any touched shard's
    /// index is not in the source yet.
    ///
    /// # Errors
    ///
    /// Those of [`shard_indexes`](Self::shard_indexes), and an index that is
    /// not a valid one for its shard: a failed checksum, or an entry that
    /// overflows, runs past the shard or overlaps the index
    /// ([`ShardLayout::read_index`]).
    pub fn inner_chunk_ranges(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Option<Vec<InnerChunkRange>>, FieldglassError> {
        let stored = self.stored(array)?;
        let Some(sharded) = Sharded::of(stored)? else {
            return Ok(None);
        };
        let mut out = Vec::new();
        for (shard, inners) in sharded.touched(stored, region)? {
            let key = shard_key(stored, &shard)?;
            let index = match sharded.index_of(&self.objects, &key)? {
                Held::Absent => continue,
                Held::Owed(_) => return Ok(None),
                Held::Read(index) => index,
            };
            out.extend(inners.into_iter().filter_map(|(inner, position)| {
                Some(InnerChunkRange {
                    index: shard.clone(),
                    key: key.clone(),
                    inner: sharded.stored_position(inner),
                    range: index.entry(position)?,
                })
            }));
        }
        Ok(Some(out))
    }

    /// What a read of `region` of `array` needs that the source was never
    /// given, worded for a refusal: each object as `` `key` ``, and each range
    /// of a shard given by range as ``bytes 10..20 of `key` ``. Empty when the
    /// read has everything it needs (#1123, #1124).
    ///
    /// # Errors
    ///
    /// Those of [`inner_chunk_ranges`](Self::inner_chunk_ranges).
    pub fn unfetched_for(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<String>, FieldglassError> {
        owed(&self.objects, self.stored(array)?, region)
    }
}

/// A sharded array's index layout and shard shape: metadata only, so a plan
/// never builds the inner chunks' codecs.
///
/// A `transpose` before the sharding codec hands it the shard in another axis
/// order (#1343). The layout's inner grid, and so the index, are in that
/// stored order; a region is in the array's own. `order` maps between them.
pub(super) struct Sharded {
    layout: ShardLayout,
    /// The shard's extent in the stored axis order: what the sharding codec
    /// was handed, and what its inner grid divides.
    shard_shape: StoredShape<'static>,
    /// The array axis that became each stored axis, or `None` when the two
    /// orders are the same.
    order: Option<Vec<usize>>,
}

/// What the source holds of one shard's index.
enum Held {
    /// The store holds no such shard: every inner chunk is the fill value.
    Absent,
    /// Not given: the whole shard (`None`), or its index's range.
    Owed(Option<ByteRange>),
    /// Read, and checked against the shard.
    Read(ShardIndex),
}

/// The inner chunks a region touches, by shard: each inner chunk's position
/// on its shard's grid, in the array's axis order, and that position in the
/// index's C order, which is over the stored axes (#1343).
pub(super) type Touched = BTreeMap<Vec<u64>, Vec<(Vec<u64>, usize)>>;

impl Sharded {
    /// `array`'s sharding, or `None` when its chunks are not shards.
    pub(super) fn of(array: &StoredArray) -> Result<Option<Self>, FieldglassError> {
        // Read off the document before any chain is built, so an array that
        // is not sharded plans as it did, whatever its codecs: planning reads
        // metadata, and an inner codec this crate cannot decode is a decode's
        // failure, not a plan's. The codecs *before* the sharding codec are
        // the exception (#1343): they decide where each inner chunk is, so
        // one that is not a `transpose` is refused here, as the decode
        // refuses it.
        let found = match array.meta.codecs() {
            // The last step, as `CodecChain::sharding` has it: a shard is an
            // object only when nothing is applied after the sharding codec.
            // Behind a `gzip`, say, the stored object is a compressed shard,
            // whose index cannot be fetched by range.
            CodecSource::V3 { codecs } => codecs
                .as_array()
                .and_then(|steps| steps.split_last())
                .filter(|(step, _)| {
                    step.get("name").and_then(Value::as_str) == Some("sharding_indexed")
                }),
            CodecSource::V2 { .. } => None,
        };
        let Some((sharding, before)) = found else {
            return Ok(None);
        };
        let layout = ShardLayout::from_v3(sharding.get("configuration").unwrap_or(&Value::Null))?;
        let shard_shape = array
            .meta
            .grid()
            .chunk_shape()
            .iter()
            .map(|&n| {
                usize::try_from(n).map_err(|_| {
                    FieldglassError::Parse(format!(
                        "a shard extent of {n} does not fit this target's pointer width"
                    ))
                })
            })
            .collect::<Result<Vec<usize>, _>>()?;
        // The codecs before the sharding codec decide the axis order it was
        // handed the shard in (#1343): the same rule a decode follows, so a
        // plan and a read agree on where an inner chunk is. Empty, as nearly
        // always, this builds nothing.
        let before = before
            .iter()
            .map(|step| Codec::from_v3(step, array.meta.dtype()))
            .collect::<Result<Vec<_>, _>>()?;
        let order = compose_transposes(&before, shard_shape.len())?;
        let shard_shape = StoredShape::permuted(Cow::Owned(shard_shape), order.as_deref());
        // Checks the grid divides and the index is within its cap, before any
        // arithmetic below relies on it.
        layout.index_len(&shard_shape)?;
        Ok(Some(Self {
            layout,
            shard_shape,
            order,
        }))
    }

    /// The stored axis that array axis `axis` became.
    fn stored_axis(&self, axis: usize) -> usize {
        self.order
            .as_deref()
            .and_then(|order| order.iter().position(|&a| a == axis))
            .unwrap_or(axis)
    }

    /// The array axis that stored axis `k` holds.
    fn array_axis(&self, k: usize) -> usize {
        self.order
            .as_deref()
            .and_then(|order| order.get(k).copied())
            .unwrap_or(k)
    }

    /// The inner chunk shape in the array's own axis order.
    fn inner_shape(&self) -> Vec<u64> {
        let inner = self.layout.chunk_shape();
        (0..inner.len())
            .map(|axis| inner[self.stored_axis(axis)] as u64)
            .collect()
    }

    /// An inner chunk's position on its shard's grid, from the array's axis
    /// order to the stored one the index is laid out in.
    fn stored_position(&self, position: Vec<u64>) -> Vec<u64> {
        match &self.order {
            None => position,
            Some(order) => order.iter().map(|&axis| position[axis]).collect(),
        }
    }

    /// Every inner chunk `region` touches, grouped by shard in row-major
    /// order. An inner chunk past the array's shape (in a ragged last shard)
    /// is never touched, since `region` lies inside the shape.
    pub(super) fn touched(
        &self,
        array: &StoredArray,
        region: &[Range<u64>],
    ) -> Result<Touched, FieldglassError> {
        let inner = self.inner_shape();
        // The inner chunks as a grid over the whole array: the array model's
        // own covering walk, with its rank, bounds and count checks.
        let grid = ChunkGrid::new(array.meta.grid().shape().to_vec(), inner.clone())?;
        // Inner chunks per shard along each of the array's axes.
        let shard_shape = self.shard_shape.as_slice();
        let per: Vec<u64> = (0..shard_shape.len())
            .map(|axis| {
                let k = self.stored_axis(axis);
                (shard_shape[k] / self.layout.chunk_shape()[k]) as u64
            })
            .collect();
        let mut out: Touched = BTreeMap::new();
        for global in grid.chunks_covering(region)? {
            let shard: Vec<u64> = global.iter().zip(&per).map(|(g, p)| g / p).collect();
            let position: Vec<u64> = global.iter().zip(&per).map(|(g, p)| g % p).collect();
            // C order of the stored axes, last fastest, as the index is
            // written. Below the index's own entry count, which `Sharded::of`
            // showed fits.
            let linear = (0..position.len()).fold(0u64, |acc, k| {
                let axis = self.array_axis(k);
                acc * per[axis] + position[axis]
            });
            let linear = usize::try_from(linear).map_err(|_| {
                FieldglassError::Parse("an inner chunk's position overflows".to_string())
            })?;
            out.entry(shard).or_default().push((position, linear));
        }
        Ok(out)
    }

    /// What the source holds of the index of the shard under `key`.
    fn index_of<O: ObjectSource>(&self, objects: &O, key: &str) -> Result<Held, FieldglassError> {
        let Some(size) = objects.object_size(key)? else {
            // No size is no object: absent when the source answers for the
            // key, and owed when it was never given.
            return Ok(if objects.unfetched(&[key]).is_empty() {
                Held::Absent
            } else {
                Held::Owed(None)
            });
        };
        let at = self.layout.index_range(&self.shard_shape, size)?;
        if !objects.unfetched_ranges(&[(key, at)]).is_empty() {
            return Ok(Held::Owed(Some(at)));
        }
        let raw = objects.get_range(key, at)?.ok_or_else(|| {
            FieldglassError::Parse(format!(
                "the source states a size for `{key}` and then holds no object there"
            ))
        })?;
        Ok(Held::Read(self.layout.read_index(
            &raw,
            &self.shard_shape,
            size,
        )?))
    }
}

/// The key of the shard at `index` on `array`'s chunk grid.
fn shard_key(array: &StoredArray, index: &[u64]) -> Result<String, FieldglassError> {
    Ok(keys_of(array, std::slice::from_ref(&index.to_vec()))?
        .pop()
        .unwrap_or_default())
}

/// `range` of `key`, as a refusal names it.
fn describe(key: &str, range: ByteRange) -> String {
    format!(
        "bytes {}..{} of `{key}`",
        range.start,
        range.start.saturating_add(range.len)
    )
}

/// What a read of `region` of `array` needs that `objects` was never given
/// (see [`ZarrStore::unfetched_for`]).
pub(super) fn owed<O: ObjectSource>(
    objects: &O,
    array: &StoredArray,
    region: &[Range<u64>],
) -> Result<Vec<String>, FieldglassError> {
    let covering = array.meta.grid().chunks_covering(region)?;
    let keys = keys_of(array, &covering)?;
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    let unfetched = objects.unfetched(&refs);
    owed_of(objects, array, region, &covering, &keys, &unfetched)
}

/// [`owed`], for a read that has already spelled the covering chunks' keys
/// and asked the source which it was never given: a source that answers
/// every key costs nothing more here (#1123).
pub(super) fn owed_of<O: ObjectSource>(
    objects: &O,
    array: &StoredArray,
    region: &[Range<u64>],
    covering: &[Vec<u64>],
    keys: &[String],
    unfetched: &[String],
) -> Result<Vec<String>, FieldglassError> {
    if unfetched.is_empty() {
        return Ok(Vec::new());
    }
    let whole = |keys: &[String]| keys.iter().map(|k| format!("`{k}`")).collect();
    // Only a shard can be read in part.
    let Some(sharded) = Sharded::of(array)? else {
        return Ok(whole(unfetched));
    };
    let touched = sharded.touched(array, region)?;
    let mut out = Vec::new();
    for (shard, key) in covering.iter().zip(keys) {
        if !unfetched.contains(key) {
            continue;
        }
        let index = match sharded.index_of(objects, key)? {
            Held::Absent => continue,
            Held::Owed(None) => {
                out.push(format!("`{key}`"));
                continue;
            }
            Held::Owed(Some(at)) => {
                out.push(describe(key, at));
                continue;
            }
            Held::Read(index) => index,
        };
        let wanted: Vec<(&str, ByteRange)> = touched
            .get(shard)
            .into_iter()
            .flatten()
            .filter_map(|(_, position)| Some((key.as_str(), index.entry(*position)?)))
            .collect();
        out.extend(
            objects
                .unfetched_ranges(&wanted)
                .into_iter()
                .map(|(key, range)| describe(&key, range)),
        );
    }
    Ok(out)
}

/// Copy into `out` every inner chunk of the shard at `shard` that `region`
/// touches (`touched`, from [`Sharded::touched`] over that region), reading
/// each by range from `objects`, which holds the shard in part. Nothing else
/// of the shard is read.
pub(super) fn read_ranged<O: ObjectSource>(
    objects: &O,
    decoder: &ChunkDecoder,
    (sharded, touched): &(Sharded, Touched),
    shard: &[u64],
    key: &str,
    region: &[Range<u64>],
    out: &mut [Option<f64>],
) -> Result<(), FieldglassError> {
    let index = match sharded.index_of(objects, key)? {
        Held::Absent => return Ok(()),
        // `owed` was asked first and named nothing, so this is a source that
        // changed its answer.
        Held::Owed(_) => {
            return Err(FieldglassError::Parse(format!(
                "the index of `{key}` is no longer in the store's source"
            )));
        }
        Held::Read(index) => index,
    };
    let inner = sharded.inner_shape();
    for (position, linear) in touched.get(shard).into_iter().flatten() {
        let Some(range) = index.entry(*linear) else {
            continue;
        };
        let stored = objects.get_range(key, range)?.ok_or_else(|| {
            FieldglassError::Parse(format!("`{key}` vanished from the store's source"))
        })?;
        // In the array's axis order, as `inner` and `position` are.
        let raw = decoder.decode_inner(&stored, sharded.order.as_deref())?;
        let values: Vec<Option<f64>> = decoder
            .dtype()
            .read_values(&raw)?
            .into_iter()
            .map(Some)
            .collect();
        let origin: Vec<u64> = (0..shard.len())
            .map(|axis| {
                let extent = sharded.shard_shape.as_slice()[sharded.stored_axis(axis)] as u64;
                shard[axis] * extent + position[axis] * inner[axis]
            })
            .collect();
        fieldglass_core::array::copy_block(&values, &inner, &origin, region, out);
    }
    Ok(())
}
