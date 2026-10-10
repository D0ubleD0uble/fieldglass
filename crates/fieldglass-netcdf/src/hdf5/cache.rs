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

/// What names one decompressed chunk: the dataset whose filters reversed it
/// (by object-header address), and the stored chunk (address, stored size,
/// and the filter mask over the pipeline's own filters, as the read compares
/// storage).
pub(crate) type ExpandedKey = (u64, u64, u32, u32);

/// The decompressed-chunk memo: chunks by key, each with the tick it was last
/// used at, and the bytes they hold, against [`CHUNK_BYTE_BUDGET`].
#[derive(Debug, Default)]
struct ExpandedStore {
    by_key: HashMap<ExpandedKey, (Arc<Vec<u8>>, u64)>,
    bytes: usize,
    tick: u64,
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
        let mut store = self
            .expanded
            .lock()
            .expect("hdf5 expanded-chunk cache poisoned");
        store.tick += 1;
        let tick = store.tick;
        let entry = store.by_key.get_mut(&key)?;
        entry.1 = tick;
        Some(Arc::clone(&entry.0))
    }

    /// Keep a decompressed chunk, dropping the least recently used past
    /// [`CHUNK_BYTE_BUDGET`]. A chunk larger than the whole budget is not kept.
    pub(crate) fn keep_expanded_chunk<S: ByteSource + ?Sized>(
        &self,
        source: &S,
        key: ExpandedKey,
        chunk: &Arc<Vec<u8>>,
    ) {
        if !self.usable(source) || chunk.len() > CHUNK_BYTE_BUDGET {
            return;
        }
        let mut store = self
            .expanded
            .lock()
            .expect("hdf5 expanded-chunk cache poisoned");
        store.tick += 1;
        let tick = store.tick;
        if let Some((old, _)) = store.by_key.insert(key, (Arc::clone(chunk), tick)) {
            store.bytes -= old.len();
        }
        store.bytes += chunk.len();
        while store.bytes > CHUNK_BYTE_BUDGET {
            // The oldest entry. A scan, since a full store holds at most a few
            // thousand chunks and eviction happens once per chunk read.
            let Some(oldest) = store
                .by_key
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(k, _)| *k)
            else {
                break;
            };
            if let Some((gone, _)) = store.by_key.remove(&oldest) {
                store.bytes -= gone.len();
            }
        }
    }

    /// Structure walks performed rather than served from the memo.
    pub(crate) fn traversals(&self) -> u64 {
        self.traversals.load(Ordering::Relaxed)
    }
}
