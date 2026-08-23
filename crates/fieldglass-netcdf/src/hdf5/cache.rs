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
//! Everything here is a pure memo: a cache hit and a cache miss must produce the
//! same value, so a full miss (a budget-refused entry, or one the slice-length
//! guard declines to serve) is only ever slower, never wrong.
//!
//! Failures are deliberately not remembered. A malformed header reached from D
//! places is re-parsed and re-fails D times, which is what the code did before
//! and keeps the walk bounded the same way (`MAX_CHUNKS`, `MAX_BTREE_NODES`)
//! rather than pinning an error to an address forever.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::group::GroupChild;
use super::object_header::{self, ObjectHeader};
use super::values::ChunkRecord;
use fieldglass_core::FieldglassError;

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

/// Chunk-index address paired with the dataset's rank — see
/// [`Hdf5Cache::chunks`] for why the rank is part of the key.
type ChunkIndexKey = (u64, usize);

/// `bound_len` before any lookup has bound it.
///
/// Zero, not `usize::MAX`, so that `#[derive(Default)]` on the cache produces an
/// unbound memo rather than one bound to a zero-length slice. Nothing is lost:
/// an HDF5 file is at least a superblock, so a real lookup never arrives with a
/// length of zero, and one that did could not be served from a memo anyway.
const UNBOUND: usize = 0;

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
    /// Whole-file depth-first child list — the order that defines every
    /// dataset's decode index.
    children: Mutex<Option<Arc<Vec<GroupChild>>>>,
    /// Parsed object headers by file offset, with their retained size.
    headers: Mutex<HeaderStore>,
    /// Length of the byte slice this memo was populated against, or
    /// [`UNBOUND`] before the first use. See [`Hdf5Cache::usable`].
    bound_len: AtomicUsize,
    /// Chunk records by `(chunk-index address, dataset rank)`. Rank is part of
    /// the key because a record's `offset` vector is rank-length: a malformed
    /// file that pointed two datasets of different rank at one index address
    /// would otherwise be served offsets of the wrong length.
    chunks: Mutex<HashMap<ChunkIndexKey, Arc<Vec<ChunkRecord>>>>,
    /// Structure walks actually performed, as opposed to served from the memo:
    /// object-header parses plus chunk-index collections. Instrumentation only —
    /// see [`Hdf5Probe::traversals`](super::Hdf5Probe::traversals).
    traversals: AtomicU64,
}

impl Hdf5Cache {
    /// Whether the memo may answer for a slice of `bytes_len` bytes.
    ///
    /// Every entry here is keyed by *file offset*, which only means anything
    /// relative to the slice it was read from. A probe was always tied to its
    /// own file — it carries that file's superblock offset sizes — but before
    /// the memo, pairing one with another file's bytes merely parsed the wrong
    /// layout and usually failed. Now it could silently return the *first*
    /// file's structure for the second, which is a worse failure: a wrong
    /// answer instead of an error.
    ///
    /// The first lookup binds the memo to its slice length; a later lookup at a
    /// different length is served without the memo — correct, just uncached.
    /// Length is a cheap discriminator, not a proof: two files of exactly equal
    /// length still alias, and the doc on [`Hdf5Probe`](super::Hdf5Probe) says
    /// so. It costs one atomic and rules out the mistake anyone would actually
    /// make.
    fn usable(&self, bytes_len: usize) -> bool {
        // The common case by far is an already-bound memo being asked about its
        // own file, so check with a plain load and keep the read-modify-write
        // for the one call that actually binds.
        match self.bound_len.load(Ordering::Relaxed) {
            UNBOUND => match self.bound_len.compare_exchange(
                UNBOUND,
                bytes_len,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => true,
                // Lost the race to bind; whoever won decides.
                Err(bound) => bound == bytes_len,
            },
            bound => bound == bytes_len,
        }
    }

    /// The object header at `offset`, parsing it only on the first request.
    ///
    /// `bytes` must be the slice the probe was built from. That was already the
    /// contract — a probe carries another file's offset sizes otherwise — but a
    /// memo makes a mismatch return *stale* data rather than a parse error, so
    /// it is worth stating.
    pub(crate) fn header(
        &self,
        bytes: &[u8],
        offset: u64,
        offset_size: u8,
        length_size: u8,
    ) -> Result<Arc<ObjectHeader>, FieldglassError> {
        if !self.usable(bytes.len()) {
            self.traversals.fetch_add(1, Ordering::Relaxed);
            return Ok(Arc::new(object_header::walk(
                bytes,
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
            bytes,
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
    pub(crate) fn children<F>(
        &self,
        bytes_len: usize,
        build: F,
    ) -> Result<Arc<Vec<GroupChild>>, FieldglassError>
    where
        F: FnOnce() -> Result<Vec<GroupChild>, FieldglassError>,
    {
        if !self.usable(bytes_len) {
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
    pub(crate) fn root<F>(&self, bytes_len: usize, build: F) -> Result<u64, FieldglassError>
    where
        F: FnOnce() -> Result<u64, FieldglassError>,
    {
        if !self.usable(bytes_len) {
            return build();
        }
        if let Some(hit) = *self.root.lock().expect("hdf5 root cache poisoned") {
            return Ok(hit);
        }
        let built = build()?;
        *self.root.lock().expect("hdf5 root cache poisoned") = Some(built);
        Ok(built)
    }

    /// The chunk records behind one dataset's chunk index, collected once.
    pub(crate) fn chunk_records<F>(
        &self,
        bytes_len: usize,
        index_address: u64,
        rank: usize,
        build: F,
    ) -> Result<Arc<Vec<ChunkRecord>>, FieldglassError>
    where
        F: FnOnce() -> Result<Vec<ChunkRecord>, FieldglassError>,
    {
        if !self.usable(bytes_len) {
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

    /// Structure walks performed rather than served from the memo.
    pub(crate) fn traversals(&self) -> u64 {
        self.traversals.load(Ordering::Relaxed)
    }
}
