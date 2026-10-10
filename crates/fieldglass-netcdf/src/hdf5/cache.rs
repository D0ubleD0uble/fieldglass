//! Per-file traversal memo for the HDF5 backing (issue #414).
//!
//! The deep walk is re-derived from scratch on every call today: decoding one
//! variable re-walks the whole group tree to find its object header, walks that
//! header for the dataspace/datatype/layout, then walks it *again* for the
//! `_FillValue` attributes; resolving metadata walks every dataset twice more.
//! For a file with `D` datasets that is `O(D)` object-header parses per decode,
//! all of them recomputing bytes that have not changed.
//!
//! In memory that is only wasted CPU. Over a byte-range transport (the Phase B
//! remote seam) each parse is a chain of dependent reads, so it is the whole
//! cost — which is why the memo lives here rather than being left to the OS
//! page cache.
//!
//! The memo hangs off [`Hdf5Probe`](super::Hdf5Probe), the per-file handle the
//! traversal functions already thread everywhere, so nothing about their
//! signatures changes. It is keyed by **file offset**, which makes it valid only
//! for the byte slice the probe was built from — the pre-existing contract for a
//! probe, now load-bearing (see [`Hdf5Cache::header`]).
//!
//! Which slice that is comes from [`ByteSource::identity`] (#681). It used to be
//! the slice's *length*, which is not an identity: two files of equal size
//! passed the guard, and the second was served the first's root address, child
//! list, object headers and chunk records — a wrong answer rather than an error.
//!
//! Everything here is a pure memo: a cache hit and a cache miss must produce the
//! same value, so a full miss (a budget-refused entry, or one the identity guard
//! declines to serve) is only ever slower, never wrong.
//!
//! Failures are deliberately not remembered. A malformed header reached from D
//! places is re-parsed and re-fails D times, which is what the code did before
//! and keeps the walk bounded the same way (`MAX_CHUNKS`, `MAX_BTREE_NODES`)
//! rather than pinning an error to an address forever.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use super::BtreeK;
use super::group::GroupChild;
use super::object_header::{self, ObjectHeader};
use super::values::ChunkRecord;
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{ByteSource, SourceIdentity};

/// Ceiling on the message bytes retained by the object-header memo, across all
/// headers of one file.
///
/// Object headers are a small fraction of a file, but "small fraction" is not
/// "bounded": a file can be mostly header (many datasets, or large inline
/// attributes), and the reader already holds the whole file in memory — the
/// peak-memory problem #411 is about. Past the budget we stop *inserting* and
/// keep serving what is already cached; the traversal still returns the right
/// answer, just without the memo. 64 MiB is far above any real NetCDF-4 header
/// set and far below a level that would matter next to the file itself.
const HEADER_BYTE_BUDGET: usize = 64 << 20;

/// Ceiling on the decompressed chunk bytes [`Hdf5Cache::expanded_chunk`] keeps
/// across one file (#939).
///
/// A region read inflates every chunk it overlaps. A chunk usually spans more
/// than one plane — netCDF-C's default chunking of a `(time, lat, lon)`
/// variable puts 24 time steps in each — so a time scrub that read each plane
/// on its own would inflate the same chunks once per frame. Holding the last
/// few lets the next frame reuse them. 64 MiB holds the chunks one plane of a
/// 0.25° global grid covers at netCDF-C's default chunking, and those of a
/// 0.5° grid cut into 64 small chunks a plane; libhdf5's own cache is 1 MiB
/// per dataset and netCDF-C raises it to tens. It is a memo of a pure
/// function bounded by size, not by use, which is the kind ADR-0011 lets a
/// library keep; the slices drawn from the chunks are the host's to retain.
pub(crate) const CHUNK_BYTE_BUDGET: usize = 64 << 20;

/// The smallest decompressed chunk the memo keeps.
///
/// Holding a chunk costs a map slot, an `Arc` and a `Vec` besides its bytes,
/// and its memo is only worth that when inflating it again would cost more. A
/// chunk under 16 KiB inflates in tens of microseconds, about what the
/// bookkeeping of a large map costs per lookup, and admitting such chunks is
/// what let 64 MiB hold a million entries (#939 review: 1 Mi chunks of 64
/// bytes). Real scrub chunking is far above it: netCDF-C's default chunk of a
/// 0.5° hourly field is 6 MB, and the perf corpus's smallest spanning chunk is
/// 256 KiB.
pub(crate) const MIN_KEPT_CHUNK_BYTES: usize = 16 << 10;

/// What each kept chunk is charged on top of its bytes: its map slot, its
/// place in the use order, the `Arc` and the `Vec` header.
const ENTRY_OVERHEAD_BYTES: usize = 128;

/// The most chunks the memo keeps, whatever their size: the budget over the
/// smallest chunk it keeps, so 4,096.
pub(crate) const MAX_KEPT_CHUNKS: usize = CHUNK_BYTE_BUDGET / MIN_KEPT_CHUNK_BYTES;

/// What names one decompressed chunk: the dataset whose filters reversed it
/// (by object-header address), and the stored chunk (address, stored size,
/// and the filter mask over the pipeline's own filters, as the read compares
/// storage).
pub(crate) type ExpandedKey = (u64, u64, u32, u32);

/// The decompressed-chunk memo: chunks by key, each with the tick it was last
/// used at, the keys in the order they were last used, and the bytes charged
/// against [`CHUNK_BYTE_BUDGET`].
///
/// The use order is a `BTreeMap` from tick to key, so the least recently used
/// is its first entry: a lookup, a keep and each eviction are `O(log n)`, not
/// a scan of every entry (#939 review).
#[derive(Debug, Default)]
pub(crate) struct ExpandedStore {
    by_key: HashMap<ExpandedKey, (Arc<Vec<u8>>, u64)>,
    by_use: std::collections::BTreeMap<u64, ExpandedKey>,
    bytes: usize,
    tick: u64,
    /// Entries dropped to make room. Instrumentation for tests.
    evicted: u64,
}

impl ExpandedStore {
    /// What one kept chunk is charged.
    fn charge(chunk: &[u8]) -> usize {
        chunk.len() + ENTRY_OVERHEAD_BYTES
    }

    /// The chunk `key` names, if kept, marked as the most recently used.
    pub(crate) fn get(&mut self, key: &ExpandedKey) -> Option<Arc<Vec<u8>>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.by_key.get_mut(key)?;
        self.by_use.remove(&entry.1);
        self.by_use.insert(tick, *key);
        entry.1 = tick;
        Some(Arc::clone(&entry.0))
    }

    /// Keep `chunk` as the most recently used, dropping the least recently
    /// used past [`CHUNK_BYTE_BUDGET`] or [`MAX_KEPT_CHUNKS`]. A chunk under
    /// [`MIN_KEPT_CHUNK_BYTES`] or over the whole budget is not kept.
    pub(crate) fn keep(&mut self, key: ExpandedKey, chunk: &Arc<Vec<u8>>) {
        if chunk.len() < MIN_KEPT_CHUNK_BYTES || Self::charge(chunk) > CHUNK_BYTE_BUDGET {
            return;
        }
        self.tick += 1;
        let tick = self.tick;
        if let Some((old, used)) = self.by_key.insert(key, (Arc::clone(chunk), tick)) {
            self.bytes -= Self::charge(&old);
            self.by_use.remove(&used);
        }
        self.by_use.insert(tick, key);
        self.bytes += Self::charge(chunk);
        while self.bytes > CHUNK_BYTE_BUDGET || self.by_key.len() > MAX_KEPT_CHUNKS {
            let Some((_, oldest)) = self.by_use.pop_first() else {
                break;
            };
            if let Some((gone, _)) = self.by_key.remove(&oldest) {
                self.bytes -= Self::charge(&gone);
                self.evicted += 1;
            }
        }
    }
}

/// Chunk-index address paired with the dataset's rank — see
/// [`Hdf5Cache::chunks`] for why the rank is part of the key.
type ChunkIndexKey = (u64, usize);

/// The object-header memo and the bytes it is holding, behind one lock so the
/// two cannot disagree — two threads that miss on the same offset would
/// otherwise each add its size to a separately-locked total.
#[derive(Debug, Default)]
struct HeaderStore {
    by_offset: HashMap<u64, Arc<ObjectHeader>>,
    /// Retained message bytes, against [`HEADER_BYTE_BUDGET`].
    bytes: usize,
}

/// The memo itself. Every field is independently locked: the maps are filled on
/// different call paths and never taken together.
///
/// `Mutex` rather than `RefCell` for the same reason as the napi handles — the
/// reader has to stay `Send` for `#[napi]`, and at zero contention a lock is one
/// uncontended atomic.
#[derive(Debug, Default)]
pub(crate) struct Hdf5Cache {
    /// Root-group object-header address, from the superblock.
    root: Mutex<Option<u64>>,
    /// The B-tree "K" values, from the superblock or its extension.
    btree_k: Mutex<Option<BtreeK>>,
    /// Whole-file depth-first child list — the order that defines every
    /// dataset's decode index.
    children: Mutex<Option<Arc<Vec<GroupChild>>>>,
    /// Parsed object headers by file offset, with their retained size.
    headers: Mutex<HeaderStore>,
    /// Which bytes this memo was populated against, empty before the first
    /// use. See [`Hdf5Cache::usable`].
    bound: OnceLock<SourceIdentity>,
    /// Chunk records by `(chunk-index address, dataset rank)`. Rank is part of
    /// the key because a record's `offset` vector is rank-length: a malformed
    /// file that pointed two datasets of different rank at one index address
    /// would otherwise be served offsets of the wrong length.
    chunks: Mutex<HashMap<ChunkIndexKey, Arc<Vec<ChunkRecord>>>>,
    /// Decompressed chunks, least recently used dropped first past
    /// [`CHUNK_BYTE_BUDGET`]. Boxed so the probe, which a reader's backing
    /// enum holds inline, does not grow by a map for it.
    expanded: Box<Mutex<ExpandedStore>>,
    /// Structure walks actually performed, as opposed to served from the memo:
    /// object-header parses plus chunk-index collections. Instrumentation only —
    /// see [`Hdf5Probe::traversals`](super::Hdf5Probe::traversals).
    traversals: AtomicU64,
}

impl Hdf5Cache {
    /// Whether the memo may answer for `source`.
    ///
    /// Every entry here is keyed by *file offset*, which only means anything
    /// relative to the slice it was read from. A probe was always tied to its
    /// own file — it carries that file's superblock offset sizes — but before
    /// the memo, pairing one with another file's bytes merely parsed the wrong
    /// layout and usually failed. Unguarded, a memo would instead hand back the
    /// *first* file's structure for the second, which is a worse failure: a
    /// wrong answer instead of an error. This is that guard.
    ///
    /// The first lookup binds the memo to its source's
    /// [identity](ByteSource::identity); a later lookup against any other is
    /// served without the memo — correct, just uncached. Identity is what makes
    /// that a real discriminator: length alone let two files of exactly equal
    /// size alias (#681), which is the one mistake a reader holding several
    /// files would actually make.
    ///
    /// A source that will not identify itself is never served from the memo,
    /// because there is no answer that is safe: two anonymous sources may be
    /// the same bytes or may not, and guessing wrong is the wrong answer again.
    fn usable<S: ByteSource + ?Sized>(&self, source: &S) -> bool {
        let Some(identity) = source.identity() else {
            return false;
        };
        // `get_or_init` is the bind: the first caller stores its identity, and
        // one that lost the race reads the winner's and compares against it.
        *self.bound.get_or_init(|| identity.clone()) == identity
    }

    /// The object header at `offset`, parsing it only on the first request.
    ///
    /// `source` must be the file the probe was built from. That was already the
    /// contract — a probe carries another file's offset sizes otherwise — but a
    /// memo makes a mismatch return *stale* data rather than a parse error, so
    /// it is worth stating.
    pub(crate) fn header<S: ByteSource + ?Sized>(
        &self,
        source: &S,
        offset: u64,
        offset_size: u8,
        length_size: u8,
    ) -> Result<Arc<ObjectHeader>, FieldglassError> {
        if !self.usable(source) {
            self.traversals.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::new(object_header::walk(
                source,
                offset,
                offset_size,
                length_size,
            )?));
        }
        if let Some(hit) = self
            .headers
            .lock()
            .expect("hdf5 header cache poisoned")
            .by_offset
            .get(&offset)
        {
            return Ok(Arc::clone(hit));
        }

        // Parsed outside the lock: a malformed header can walk a long
        // continuation chain, and holding the map meanwhile would serialise
        // unrelated lookups for no gain. Two threads can therefore race to the
        // same offset; the loser adopts the winner's copy below, so callers
        // still share one header rather than two.
        self.traversals.fetch_add(1, Ordering::Relaxed);
        let header = Arc::new(object_header::walk(
            source,
            offset,
            offset_size,
            length_size,
        )?);
        let retained: usize = header.messages.iter().map(|m| m.body.len()).sum();

        let mut store = self.headers.lock().expect("hdf5 header cache poisoned");
        if let Some(won) = store.by_offset.get(&offset) {
            return Ok(Arc::clone(won));
        }
        if store.bytes + retained <= HEADER_BYTE_BUDGET {
            store.bytes += retained;
            store.by_offset.insert(offset, Arc::clone(&header));
        }
        Ok(header)
    }

    /// The whole-file depth-first child list, walked only once.
    pub(crate) fn children<S: ByteSource + ?Sized, F>(
        &self,
        source: &S,
        build: F,
    ) -> Result<Arc<Vec<GroupChild>>, FieldglassError>
    where
        F: FnOnce() -> Result<Vec<GroupChild>, FieldglassError>,
    {
        if !self.usable(source) {
            return Ok(Arc::new(build()?));
        }
        if let Some(hit) = self
            .children
            .lock()
            .expect("hdf5 child cache poisoned")
            .as_ref()
        {
            return Ok(Arc::clone(hit));
        }
        let built = Arc::new(build()?);
        *self.children.lock().expect("hdf5 child cache poisoned") = Some(Arc::clone(&built));
        Ok(built)
    }

    /// The root-group object-header address, read from the superblock once.
    pub(crate) fn root<S: ByteSource + ?Sized, F>(
        &self,
        source: &S,
        build: F,
    ) -> Result<u64, FieldglassError>
    where
        F: FnOnce() -> Result<u64, FieldglassError>,
    {
        if !self.usable(source) {
            return build();
        }
        if let Some(hit) = *self.root.lock().expect("hdf5 root cache poisoned") {
            return Ok(hit);
        }
        let built = build()?;
        *self.root.lock().expect("hdf5 root cache poisoned") = Some(built);
        Ok(built)
    }

    /// The B-tree "K" values, read from the superblock once.
    pub(crate) fn btree_k<S: ByteSource + ?Sized, F>(
        &self,
        source: &S,
        build: F,
    ) -> Result<BtreeK, FieldglassError>
    where
        F: FnOnce() -> Result<BtreeK, FieldglassError>,
    {
        if !self.usable(source) {
            return build();
        }
        if let Some(hit) = *self.btree_k.lock().expect("hdf5 B-tree K cache poisoned") {
            return Ok(hit);
        }
        let built = build()?;
        *self.btree_k.lock().expect("hdf5 B-tree K cache poisoned") = Some(built);
        Ok(built)
    }

    /// The chunk records behind one dataset's chunk index, collected once.
    pub(crate) fn chunk_records<S: ByteSource + ?Sized, F>(
        &self,
        source: &S,
        index_address: u64,
        rank: usize,
        build: F,
    ) -> Result<Arc<Vec<ChunkRecord>>, FieldglassError>
    where
        F: FnOnce() -> Result<Vec<ChunkRecord>, FieldglassError>,
    {
        if !self.usable(source) {
            self.traversals.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::new(build()?));
        }
        let key = (index_address, rank);
        if let Some(hit) = self
            .chunks
            .lock()
            .expect("hdf5 chunk cache poisoned")
            .get(&key)
        {
            return Ok(Arc::clone(hit));
        }
        self.traversals.fetch_add(1, Ordering::Relaxed);
        let built = Arc::new(build()?);
        self.chunks
            .lock()
            .expect("hdf5 chunk cache poisoned")
            .insert(key, Arc::clone(&built));
        Ok(built)
    }

    /// A decompressed chunk already held, marked as the most recently used.
    ///
    /// `None` when the memo may not answer for `source`, as for every other
    /// entry here.
    pub(crate) fn expanded_chunk<S: ByteSource + ?Sized>(
        &self,
        source: &S,
        key: ExpandedKey,
    ) -> Option<Arc<Vec<u8>>> {
        if !self.usable(source) {
            return None;
        }
        self.expanded
            .lock()
            .expect("hdf5 expanded-chunk cache poisoned")
            .get(&key)
    }

    /// Keep a decompressed chunk; see [`ExpandedStore::keep`] for what is kept
    /// and what is dropped.
    pub(crate) fn keep_expanded_chunk<S: ByteSource + ?Sized>(
        &self,
        source: &S,
        key: ExpandedKey,
        chunk: &Arc<Vec<u8>>,
    ) {
        if !self.usable(source) {
            return;
        }
        self.expanded
            .lock()
            .expect("hdf5 expanded-chunk cache poisoned")
            .keep(key, chunk);
    }

    /// Structure walks performed rather than served from the memo.
    pub(crate) fn traversals(&self) -> u64 {
        self.traversals.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(bytes: usize) -> Arc<Vec<u8>> {
        Arc::new(vec![7u8; bytes])
    }

    fn key(i: u64) -> ExpandedKey {
        (1, i, 0, 0)
    }

    /// Chunks too small to be worth holding are not held, however many there
    /// are: the #939 review's file of 1 Mi 64-byte chunks filled the memo with
    /// a million entries and made every later keep scan them.
    #[test]
    fn small_chunks_are_not_kept() {
        let mut store = ExpandedStore::default();
        for i in 0..100_000 {
            store.keep(key(i), &chunk(64));
        }
        assert!(store.by_key.is_empty());
        assert_eq!(store.bytes, 0);
        assert!(store.get(&key(5)).is_none());
    }

    /// Past the budget the least recently used goes, one entry per entry
    /// kept, and a hit moves its chunk to the back of the order. The work is
    /// counted rather than timed: each keep evicts at most what it displaces.
    #[test]
    fn the_least_recently_used_go_first_one_per_keep() {
        let mut store = ExpandedStore::default();
        let size = MIN_KEPT_CHUNK_BYTES;
        let fits = CHUNK_BYTE_BUDGET / (size + ENTRY_OVERHEAD_BYTES);
        assert!(fits < MAX_KEPT_CHUNKS);
        for i in 0..fits as u64 {
            store.keep(key(i), &chunk(size));
        }
        assert_eq!(store.by_key.len(), fits);
        assert_eq!(store.evicted, 0);

        // Touch the oldest, then keep one more: the second oldest goes.
        assert!(store.get(&key(0)).is_some());
        store.keep(key(fits as u64), &chunk(size));
        assert_eq!(store.evicted, 1);
        assert!(store.get(&key(0)).is_some(), "a hit is the most recent");
        assert!(store.get(&key(1)).is_none(), "the least recent went");

        // Many more: one eviction per keep, never a rescan, and the books
        // agree with what is held.
        let extra = 10 * fits as u64;
        for i in 0..extra {
            store.keep(key(1_000_000 + i), &chunk(size));
        }
        assert_eq!(store.evicted, 1 + extra);
        assert_eq!(store.by_key.len(), fits);
        assert_eq!(store.by_use.len(), fits);
        assert_eq!(store.bytes, fits * (size + ENTRY_OVERHEAD_BYTES));
        assert!(store.bytes <= CHUNK_BYTE_BUDGET);
    }

    /// Re-keeping a chunk replaces it rather than counting it twice.
    #[test]
    fn keeping_a_chunk_again_replaces_it() {
        let mut store = ExpandedStore::default();
        store.keep(key(1), &chunk(MIN_KEPT_CHUNK_BYTES));
        store.keep(key(1), &chunk(MIN_KEPT_CHUNK_BYTES));
        assert_eq!(store.by_key.len(), 1);
        assert_eq!(store.by_use.len(), 1);
        assert_eq!(store.bytes, MIN_KEPT_CHUNK_BYTES + ENTRY_OVERHEAD_BYTES);
    }
}
