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

use std::collections::BTreeMap;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::ChunkGrid;
use fieldglass_core::bytes::{ByteRange, ObjectSource};

use super::{StoredArray, ZarrStore, keys_of};
use crate::metadata::CodecSource;
use crate::{ChunkDecoder, IndexLocation, ShardIndex, ShardLayout};
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
    /// The inner chunk's position on the shard's grid of inner chunks.
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
                    inner,
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
pub(super) struct Sharded {
    layout: ShardLayout,
    shard_shape: Vec<usize>,
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
/// on its shard's grid, and that position in the index's C order.
pub(super) type Touched = BTreeMap<Vec<u64>, Vec<(Vec<u64>, usize)>>;

impl Sharded {
    /// `array`'s sharding, or `None` when its chunks are not shards.
    pub(super) fn of(array: &StoredArray) -> Result<Option<Self>, FieldglassError> {
        // Read off the document before any chain is built, so an array that
        // is not sharded plans as it did, whatever its codecs: planning reads
        // metadata, and a codec this crate cannot decode is a decode's
        // failure, not a plan's.
        let config = match array.meta.codecs() {
            CodecSource::V3 { codecs } => codecs.as_array().and_then(|steps| {
                steps
                    .iter()
                    .find(|step| {
                        step.get("name").and_then(Value::as_str) == Some("sharding_indexed")
                    })
                    .map(|step| step.get("configuration").unwrap_or(&Value::Null))
            }),
            CodecSource::V2 { .. } => None,
        };
        let Some(config) = config else {
            return Ok(None);
        };
        let layout = ShardLayout::from_v3(config)?;
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
        // Checks the grid divides and the index is within its cap, before any
        // arithmetic below relies on it.
        layout.index_len(&shard_shape)?;
        Ok(Some(Self {
            layout,
            shard_shape,
        }))
    }

    fn inner_shape(&self) -> Vec<u64> {
        self.layout
            .chunk_shape()
            .iter()
            .map(|&n| n as u64)
            .collect()
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
        let per: Vec<u64> = self
            .shard_shape
            .iter()
            .zip(self.layout.chunk_shape())
            .map(|(&shard, &inner)| (shard / inner) as u64)
            .collect();
        let mut out: Touched = BTreeMap::new();
        for global in grid.chunks_covering(region)? {
            let shard: Vec<u64> = global.iter().zip(&per).map(|(g, p)| g / p).collect();
            let position: Vec<u64> = global.iter().zip(&per).map(|(g, p)| g % p).collect();
            // C order, last axis fastest, as the index is written. Below the
            // index's own entry count, which `Sharded::of` showed fits.
            let linear = position
                .iter()
                .zip(&per)
                .fold(0u64, |acc, (i, p)| acc * p + i);
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
    if unfetched.is_empty() {
        return Ok(Vec::new());
    }
    let whole = |keys: &[String]| keys.iter().map(|k| format!("`{k}`")).collect();
    // Only a shard can be read in part.
    let Some(sharded) = Sharded::of(array)? else {
        return Ok(whole(&unfetched));
    };
    let touched = sharded.touched(array, region)?;
    let mut out = Vec::new();
    for (shard, key) in covering.iter().zip(&keys) {
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
    let sharding = decoder.chain().sharding().ok_or_else(|| {
        FieldglassError::WrongLayout("this array's chunks are not shards".to_string())
    })?;
    let element_size = decoder.dtype().size;
    for (position, linear) in touched.get(shard).into_iter().flatten() {
        let Some(range) = index.entry(*linear) else {
            continue;
        };
        let stored = objects.get_range(key, range)?.ok_or_else(|| {
            FieldglassError::Parse(format!("`{key}` vanished from the store's source"))
        })?;
        let raw = sharding.decode_inner(&stored, element_size)?;
        let values: Vec<Option<f64>> = decoder
            .dtype()
            .read_values(&raw)?
            .into_iter()
            .map(Some)
            .collect();
        let origin: Vec<u64> = shard
            .iter()
            .zip(&sharded.shard_shape)
            .zip(position.iter().zip(&inner))
            .map(|((s, &extent), (p, i))| s * extent as u64 + p * i)
            .collect();
        fieldglass_core::array::copy_block(&values, &inner, &origin, region, out);
    }
    Ok(())
}
