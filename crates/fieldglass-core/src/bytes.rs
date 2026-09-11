//! The byte-access seam ([ADR-0005], #438).
//!
//! Readers take a whole file today and index it. The remote work — HTTP range
//! (#247), object stores (#252), Zarr (#246), and files too large to hold
//! (#114) — needs them to say *which* bytes they want instead, so a transport
//! can fetch exactly those.
//!
//! [ADR-0005] fixes the shape that takes: **resolve the ranges an operation
//! needs, fetch them in one batch, then decode synchronously from the result.**
//! Decoders stay sync all the way down, because blocking inside a read is
//! impossible on a browser's main thread and a wasm build is a named goal.
//!
//! That is why this trait has two halves. [`ByteSource::prefetch`] is the batch
//! resolve — a remote source issues its requests there and caches what comes
//! back. [`ByteSource::read`] is the synchronous read that decode actually
//! calls, and for an in-memory source it is a slice, not a copy.
//!
//! [`ByteSource::identity`] is the third half. ADR-0005 decision 2 asked for an
//! identity stronger than length, because a reader that memoises what it found
//! in a file has to know the next call is about the same file; the HDF5
//! traversal memo keys everything by file offset, and while length was the only
//! discriminator it served one file's structure for another of equal size
//! (#681). [`SourceIdentity`] is that discriminator.
//!
//! # What is deliberately not here
//!
//! **Borrowing the host's buffer.** Removing the last copy at the napi boundary
//! needs the reader to borrow the napi `Buffer`, which makes the handle
//! self-referential. [`ByteSource::read`] returning [`Cow`] is what leaves room
//! for that: an implementation that owns its bytes borrows them out, and one
//! that just fetched hands over what it fetched.
//!
//! # What a remote implementation will look like, and what it costs
//!
//! Worth knowing before writing one, because it is a consequence of this shape
//! rather than of any transport. `read` takes `&self`, so a source that fetches
//! during [`prefetch`](ByteSource::prefetch) has to cache behind interior
//! mutability — and a reference cannot be handed out of a `RefCell` guard. So a
//! cache-backed source **cannot** return `Cow::Borrowed`; it clones out of its
//! cache on every read.
//!
//! That is the right trade and not an oversight: against a network fetch a
//! memcpy is nothing, and paying it there is what lets the in-memory path stay a
//! plain slice. `crates/fieldglass-netcdf/tests/classic_byte_source.rs` has a
//! cache-backed source that never borrows, decoding identically, so the shape is
//! known to work before the first transport exists.
//!
//! Two other things such an implementation must do: know its
//! [`size`](ByteSource::size) up front — an HTTP `HEAD` or a `Content-Range` —
//! and keep `read` working for a range that was never prefetched, since the
//! batch is advisory.
//!
//! [ADR-0005]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md

use crate::error::FieldglassError;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// A half-open byte range `[start, start + len)` in a source.
///
/// `u64` rather than `usize` because the range describes a *file*, which may be
/// larger than a 32-bit address space even where the decode of any one slab is
/// not. Converting to `usize` is the reader's business, at the point it has
/// bounded the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteRange {
    /// Offset of the first byte from the start of the source.
    pub start: u64,
    /// Length of the range in bytes.
    pub len: u64,
}

impl ByteRange {
    /// A range of `len` bytes beginning at `start`.
    pub fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }

    /// One past the last byte, or `None` on overflow — which for a range built
    /// from a file's own header is a malformed file, not an internal error.
    pub fn end(&self) -> Option<u64> {
        self.start.checked_add(self.len)
    }
}

/// How much of each end of a buffer goes into its identity.
///
/// Both ends rather than one: a container writes its header at the front and
/// its end-of-file mark at the back, so two different files of one size
/// disagree at one end or the other long before they disagree in the middle.
/// 256 is comfortably past HDF5's superblock and NetCDF classic's header start,
/// and small enough that hashing it on every memo lookup is not worth measuring
/// against the walk it saves.
const SAMPLE_BYTES: usize = 256;

/// FNV-1a over the bytes at each end of `bytes`, as [`SourceIdentity::Buffer`]
/// carries. Not cryptographic: the threat is two files in one session being
/// confused for each other, not an adversary choosing them.
fn sample_of(bytes: &[u8]) -> u64 {
    let head = &bytes[..bytes.len().min(SAMPLE_BYTES)];
    // Overlaps `head` for a buffer shorter than twice the sample, which only
    // hashes those bytes twice.
    let tail = &bytes[bytes.len().saturating_sub(SAMPLE_BYTES)..];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in head.iter().chain(tail) {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h
}

/// What tells one [`ByteSource`]'s bytes from another's.
///
/// [ADR-0005] decision 2 asked for an identity "stronger than length", and this
/// is it. Length alone is not one: two files of the same size are
/// indistinguishable by it, which is how the HDF5 traversal memo came to serve
/// one file's structure for another (#681).
///
/// **Equality is the whole contract.** Equal identities mean the same bytes, so
/// work remembered against one may be reused for the other; unequal identities
/// mean they may differ, and the work is done again. A source that cannot say
/// answers `None` from [`ByteSource::identity`], which is never reused for
/// anything — the safe answer, and only ever slower.
///
/// [ADR-0005]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum SourceIdentity {
    /// One in-memory buffer: where it begins, how far it runs, and what is at
    /// each end of it.
    ///
    /// Address and length alone would not do. An allocator may hand the same
    /// address out again once a buffer is freed, and a caller may refill one
    /// buffer with a different file in place — neither moves the address or
    /// changes the length, and both are how a memo would go back to answering
    /// for the wrong file. So a sample of the bytes goes in too.
    ///
    /// It is a hash of a bounded sample, so it can collide in principle: two
    /// buffers at one address, of one length, agreeing at both ends are treated
    /// as the same bytes. That rules out every mistake a caller makes by
    /// accident, which is what this is for; it is not a proof, and a source
    /// that can offer one should answer [`Named`](Self::Named) with a version
    /// in the name instead.
    Buffer {
        /// Address of the first byte, in this process.
        addr: usize,
        /// Length in bytes, so two buffers that begin together but run to
        /// different ends are still told apart.
        len: u64,
        /// FNV-1a over the bytes at each end — see the variant doc for what it
        /// catches and what it cannot.
        sample: u64,
    },
    /// A name the host vouches for: a path, a URL, an object key.
    ///
    /// What a source that is not one contiguous buffer has to use. A sparse map
    /// of prefetched ranges has no single address to offer, and that is exactly
    /// the case where a memo is worth the most, since every walk it saves is a
    /// chain of dependent round-trips (ADR-0005).
    ///
    /// **The name has to change when the bytes do**, and only the host can
    /// promise that. A key overwritten with a different object of the same
    /// length is the same failure as an equal-length file, one layer up, and no
    /// inspection of the name would catch it — so a host that has a version,
    /// an ETag or a modification time should put it in the name rather than
    /// leave the promise to the path alone.
    Named {
        /// How the host names this object, version and all.
        name: String,
        /// Size in bytes, so a name reused for an object that has since changed
        /// length does not inherit the old one's memo even where the host
        /// forgot to version it.
        size: u64,
    },
}

impl SourceIdentity {
    /// The identity of an in-memory buffer.
    ///
    /// Costs a hash of at most 256 bytes from each end, which is what keeps it
    /// callable on every memo lookup rather than once per open.
    #[must_use]
    pub fn of_buffer(bytes: &[u8]) -> Self {
        Self::Buffer {
            addr: bytes.as_ptr().addr(),
            len: bytes.len() as u64,
            sample: sample_of(bytes),
        }
    }

    /// The identity of an object the host can name.
    #[must_use]
    pub fn named(name: impl Into<String>, size: u64) -> Self {
        Self::Named {
            name: name.into(),
            size,
        }
    }
}

/// Somewhere bytes come from.
///
/// The blanket implementations for `[u8]` and `Vec<u8>` are what make migration
/// incremental: a reader can move to this trait without any of its callers
/// changing, because the buffer they already pass is a `ByteSource`.
pub trait ByteSource {
    /// Total size in bytes.
    fn size(&self) -> u64;

    /// Whether the source holds no bytes at all.
    fn is_empty(&self) -> bool {
        self.size() == 0
    }

    /// Which bytes these are, when the source can say.
    ///
    /// How a reader that memoises what it found at a file offset asks whether
    /// the next call is about the same file. See [`SourceIdentity`] for what
    /// equality promises.
    ///
    /// The default declines, because there is nothing a source can answer from
    /// [`size`](Self::size) alone that is not the length aliasing this exists to
    /// end — and an identity that is wrong is worse than none, since it turns a
    /// repeated walk into a wrong answer. Declining costs only the walk.
    fn identity(&self) -> Option<SourceIdentity> {
        None
    }

    /// Resolve a batch of ranges before they are read.
    ///
    /// This is where a remote source does its work: one request per contiguous
    /// run, or one multi-range request, with the result cached for the [`read`]
    /// calls that follow. An in-memory source has nothing to do, which is why
    /// the default is a no-op — and why a reader that calls `prefetch` costs
    /// nothing locally.
    ///
    /// Calling it is advisory. `read` must work whether or not a range was
    /// prefetched; skipping the call only costs latency.
    ///
    /// [`read`]: ByteSource::read
    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        let _ = ranges;
        Ok(())
    }

    /// Read one range.
    ///
    /// Borrowed when the source already holds the bytes, so the in-memory path
    /// is a slice and not a copy. Errors when the range runs past the end —
    /// every range here derives from a file's own header, so out of bounds
    /// means the file said something untrue about itself.
    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError>;
}

/// Somewhere *keyed* objects come from.
///
/// [`ByteSource`] models one object addressed by byte range: a GRIB file, a
/// NetCDF file, an archive somebody range-fetches into. A great deal of
/// meteorological data is not shaped like that. A Zarr store is a key for every
/// chunk and every metadata document; a directory is a key per file; a kerchunk
/// reference document is a key per chunk pointing into somebody else's object.
/// Reaching those needs a different seam, and this is it.
///
/// The two are siblings rather than one wrapping the other. A caller that has
/// an object and wants part of it uses `ByteSource`; a caller that has a name
/// and wants the object uses this. Nothing here is layered on ranges, because a
/// key-addressed store has no offsets to speak of.
///
/// # The rules are `ByteSource`'s
///
/// **Synchronous**, because fetching is the host's and ADR-0005 decision 1
/// keeps the library out of it: an implementation that reaches the network does
/// its waiting behind [`prefetch`](Self::prefetch), and every read after that is
/// a lookup.
///
/// **[`prefetch`](Self::prefetch) is advisory.** [`get`](Self::get) works
/// whether or not a key was prefetched; skipping the call costs latency and
/// nothing else. That is what lets a walker be written once and run against an
/// in-memory store, a directory and a bucket.
///
/// # An absent key is not an error
///
/// [`get`](Self::get) answers `Ok(None)` for a key the store does not hold,
/// because for the thing this seam exists to read, absence is ordinary and
/// frequent: a sparse Zarr array stores no object for a chunk that is entirely
/// fill value, and the missing chunks are the point rather than a fault. A
/// signature that made absence an error would push every caller into matching
/// on an error kind to recover the normal case.
///
/// [`require`](Self::require) is the other half, for the keys that must be
/// there — an array's own metadata document — and it is a provided method so
/// the two cannot disagree about what absence means.
///
/// ```
/// use fieldglass_core::bytes::{MemoryObjects, ObjectSource};
///
/// let store = MemoryObjects::from_iter([
///     ("temp/.zarray", b"{}".to_vec()),
///     ("temp/0.0", b"chunk".to_vec()),
/// ]);
///
/// // One batch before any read, which is where a remote store does its work.
/// store.prefetch(&["temp/.zarray", "temp/0.0"])?;
///
/// assert_eq!(store.get("temp/0.0")?.as_deref(), Some(&b"chunk"[..]));
/// // Absent, not broken: this chunk is the fill value.
/// assert!(store.get("temp/0.1")?.is_none());
/// assert_eq!(store.list("temp/")?, ["temp/.zarray", "temp/0.0"]);
/// # Ok::<(), fieldglass_core::FieldglassError>(())
/// ```
pub trait ObjectSource {
    /// The object stored under `key`, or `None` if the store does not hold one.
    ///
    /// Borrowed when the store already has the bytes, so an in-memory read is a
    /// slice and not a copy — the same reason [`ByteSource::read`] borrows.
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError>;

    /// Every key the store holds that begins with `prefix`, in sorted order.
    ///
    /// Sorted so a walk is reproducible: two hosts listing the same store hand
    /// their reader the same sequence, and a test can assert on it. A prefix
    /// that matches nothing is an empty list, not an error — an empty group is
    /// a group.
    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError>;

    /// The *immediate* children under `prefix`, in sorted order.
    ///
    /// Objects directly under it by their whole key, and child "directories"
    /// as the prefix they are — `temp/`, ending in a slash. A caller tells the
    /// two apart with [`str::ends_with`].
    ///
    /// This is the listing a walker wants, and [`list`](Self::list) is not it.
    /// `list` returns every key beneath the prefix, so walking a store with it
    /// enumerates every chunk of every array to find a handful of metadata
    /// documents. That is nothing on a directory and millions of keys on a
    /// bucket.
    ///
    /// The default filters `list`, so no implementation has to change and an
    /// in-memory store pays a pass over its keys. A bucket-backed one
    /// overrides it with a delimiter listing, which is the same answer for one
    /// request instead of a paged walk of the whole store.
    ///
    /// ```
    /// use fieldglass_core::bytes::{MemoryObjects, ObjectSource};
    ///
    /// let store = MemoryObjects::from_iter([
    ///     (".zgroup", b"{}".to_vec()),
    ///     ("temp/.zarray", b"{}".to_vec()),
    ///     ("temp/0/0", b"chunk".to_vec()),
    ///     ("temp/1/0", b"chunk".to_vec()),
    /// ]);
    ///
    /// assert_eq!(store.list_children("")?, [".zgroup", "temp/"]);
    /// // The array's own documents, and its chunk rows as directories — the
    /// // chunks themselves are never named.
    /// assert_eq!(store.list_children("temp/")?, ["temp/.zarray", "temp/0/", "temp/1/"]);
    /// # Ok::<(), fieldglass_core::FieldglassError>(())
    /// ```
    fn list_children(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        let mut children: Vec<String> = Vec::new();
        for key in self.list(prefix)? {
            // `list` promises sorted keys, and every key sharing a first
            // segment after the prefix shares a literal prefix — so they are
            // contiguous and comparing against the last one is the whole
            // deduplication.
            let child = match key.get(prefix.len()..).and_then(|r| r.split_once('/')) {
                Some((segment, _)) => format!("{prefix}{segment}/"),
                None => key,
            };
            if children.last() != Some(&child) {
                children.push(child);
            }
        }
        Ok(children)
    }

    /// Resolve a batch of keys before they are read.
    ///
    /// Where a remote store does its work: one request per key, or one batched
    /// request, with the result held for the [`get`](Self::get) calls that
    /// follow. An in-memory store has nothing to do, which is why the default
    /// is a no-op and why a reader that calls it costs nothing locally.
    ///
    /// Advisory, as [`ByteSource::prefetch`] is: a key that was not prefetched
    /// still reads.
    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        let _ = keys;
        Ok(())
    }

    /// The object stored under `key`, erroring when the store does not hold it.
    ///
    /// For the keys whose absence really is a fault — an array's metadata
    /// document, a group's — so that a caller does not write the same
    /// `ok_or_else` at each of them, and so every reader words it identically.
    fn require(&self, key: &str) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.get(key)?.ok_or_else(|| {
            FieldglassError::Parse(format!("this store holds no object under {key:?}"))
        })
    }
}

/// An [`ObjectSource`] over a map.
///
/// What a test, a fixture and an already-downloaded store all want: the objects
/// and nothing else. It used to record what was asked of it as well, which made
/// it a store that grew a string per read forever — fine for a test that reads a
/// dozen keys, and a leak for the host that uses it as its *real* store for an
/// opened directory (#659). Recording is `fieldglass_core::testing::Recording`
/// now — named rather than linked, because that module is behind the `testing`
/// feature and this item is not — and it records for any source rather than
/// only for this one.
#[derive(Debug, Default)]
pub struct MemoryObjects {
    objects: BTreeMap<String, Vec<u8>>,
}

impl MemoryObjects {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one object, replacing anything under that key.
    pub fn insert(&mut self, key: impl Into<String>, bytes: Vec<u8>) {
        self.objects.insert(key.into(), bytes);
    }

    /// How many objects the store holds.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the store holds nothing.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }
}

impl<K: Into<String>> FromIterator<(K, Vec<u8>)> for MemoryObjects {
    fn from_iter<I: IntoIterator<Item = (K, Vec<u8>)>>(iter: I) -> Self {
        Self {
            objects: iter.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }
}

/// So a caller holding `&O` can pass it where an `ObjectSource` is wanted —
/// the same forwarding [`ByteSource`] has, which lets a reader borrow its store
/// rather than own it and a test keep the store to inspect afterwards.
impl<O: ObjectSource + ?Sized> ObjectSource for &O {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        (**self).get(key)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        (**self).list(prefix)
    }

    fn list_children(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        (**self).list_children(prefix)
    }

    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        (**self).prefetch(keys)
    }
}

impl ObjectSource for MemoryObjects {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        Ok(self.objects.get(key).map(|bytes| Cow::Borrowed(&bytes[..])))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        // `BTreeMap` is already in key order, so the sorted contract costs a
        // filter rather than a sort.
        Ok(self
            .objects
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// Narrow a length, count or offset that came out of a file to `usize`.
///
/// This is the "at the point it has bounded the value" step [`ByteRange`]
/// describes. A file field is `u64` because the file says so; `usize` is 32 bits
/// wide on `wasm32-unknown-unknown`, the browser target the crate is built for.
/// A bare `as usize` there wraps, and the wrapped value can still pass the
/// bounds check that follows it — so the reader goes on to slice the wrong
/// bytes and answer confidently with the wrong field. Failing the parse instead
/// keeps a 32-bit host from disagreeing silently with a 64-bit one.
///
/// `what` names the field for the error message, e.g. `"NetCDF vsize"`.
///
/// Only a value that can genuinely exceed `u32::MAX` needs this. A field read
/// from four bytes or fewer already fits `usize` on every target Rust supports,
/// since `usize` is at least 32 bits wide; it is the 8-byte HDF5 lengths and
/// offsets, and products of several dimensions, that can overflow.
pub fn checked_usize(value: u64, what: &str) -> Result<usize, FieldglassError> {
    usize::try_from(value).map_err(|_| {
        FieldglassError::Parse(format!(
            "{what} {value} does not fit this target's address space"
        ))
    })
}

/// A [`FileCursor`]'s first window.
///
/// Small on purpose. Most structures a traversal walks are a few dozen bytes —
/// an HDF5 B-tree node prefix, a GRIB section header — and a generous first
/// window would read a hundred times what they need.
pub const FIRST_WINDOW_BYTES: usize = 64;

/// The largest window a [`FileCursor`] or [`find_forward`] will grow to.
///
/// Each refill doubles, so a cursor that keeps going reaches a structure of `N`
/// bytes in `log2(N/64)` reads having fetched under `2N`, rather than one read
/// per field (a round trip per integer over a transport) or one fixed large
/// window per structure (a hundredfold over-read on the small ones). Both axes
/// matter and this is the shape that bounds both.
pub const MAX_WINDOW_BYTES: usize = 64 << 10;

/// Read exactly the bytes `range` names.
///
/// "Exactly" is the whole point. [`ByteSource::read`] bounds-checks the range
/// against the source's own size, which an in-memory buffer can always honour;
/// a transport with a truncated response cannot, and every caller goes on to
/// index the result at offsets the file told it about. So the length is checked
/// once, here, rather than at each call site that would otherwise have to.
///
/// A range longer than this target can address is refused rather than
/// narrowed — see [`checked_usize`].
pub fn read_exact<S: ByteSource + ?Sized>(
    source: &S,
    range: ByteRange,
) -> Result<Cow<'_, [u8]>, FieldglassError> {
    let len = checked_usize(range.len, "byte range length")?;
    let got = source.read(range)?;
    if got.len() != len {
        return Err(FieldglassError::ShortRead {
            at: range.start,
            got: got.len() as u64,
            wanted: range.len,
        });
    }
    Ok(got)
}

/// [`read_exact`] for a length already in hand as a `usize`.
pub fn read_at<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    len: usize,
) -> Result<Cow<'_, [u8]>, FieldglassError> {
    read_exact(source, ByteRange::new(addr, len as u64))
}

/// Read up to `len` bytes at `addr`, stopping at the end of the source.
///
/// For the reads whose length is not yet known — a superblock prefix, an object
/// header whose size field is inside the bytes being read — where running short
/// is ordinary and the parse that follows does its own bounds checking. An
/// `addr` past the end is still an error: that is a bad address, not a short
/// read.
pub fn read_up_to<S: ByteSource + ?Sized>(
    source: &S,
    addr: u64,
    len: usize,
) -> Result<Cow<'_, [u8]>, FieldglassError> {
    let size = source.size();
    if addr > size {
        return Err(FieldglassError::Parse(format!(
            "address {addr} past end of file ({size} bytes)"
        )));
    }
    let available = size - addr;
    source.read(ByteRange::new(addr, (len as u64).min(available)))
}

/// The window sizes a scan for a terminator steps through, up to `ceiling`.
///
/// Same argument as [`FileCursor`]'s: a name is usually a dozen bytes, so
/// asking for the ceiling every time would fetch it over a transport to read
/// ten bytes out of it.
pub fn scan_windows(ceiling: usize) -> impl Iterator<Item = usize> {
    std::iter::successors(Some(FIRST_WINDOW_BYTES.min(ceiling)), move |&w| {
        (w < ceiling).then(|| w.saturating_mul(4).min(ceiling))
    })
}

/// The first offset at or after `from` where `span` bytes satisfy `hit`.
///
/// The answer a byte-at-a-time search gives, over a source that charges for
/// every read: `hit` is asked about every position `p` with `p + span` inside
/// the source, in order, and the first `p` it accepts is returned. `None` when
/// no position qualifies, including when fewer than `span` bytes remain.
///
/// The bytes come in growing windows, overlapping by `span - 1` so a match
/// straddling two of them is still seen. Skipping `G` bytes of garbage costs
/// `O(log G)` reads up to [`MAX_WINDOW_BYTES`] and one read per window after
/// that, where a naive search over a transport would cost one per byte.
///
/// A `span` of zero is refused: every position would trivially match nothing
/// at all, which is a caller bug rather than an answer.
pub fn find_forward<S: ByteSource + ?Sized>(
    source: &S,
    from: u64,
    span: usize,
    mut hit: impl FnMut(&[u8]) -> bool,
) -> Result<Option<u64>, FieldglassError> {
    if span == 0 {
        return Err(FieldglassError::Parse(
            "a forward search needs a non-empty span".to_string(),
        ));
    }
    let size = source.size();
    let mut at = from;
    let mut window = FIRST_WINDOW_BYTES.max(span);
    loop {
        let available = size.saturating_sub(at);
        if available < span as u64 {
            return Ok(None);
        }
        // `available >= span`, so `want >= span` and the subtraction below
        // cannot underflow.
        let want = (window as u64).min(available) as usize;
        let bytes = read_at(source, at, want)?;
        if let Some(i) = bytes.windows(span).position(&mut hit) {
            return Ok(Some(at + i as u64));
        }
        at += (want - span + 1) as u64;
        window = window.saturating_mul(2).min(MAX_WINDOW_BYTES.max(span));
    }
}

/// A forward cursor over a [`ByteSource`], at an absolute address, that reads
/// in windows rather than fields.
///
/// A structure walk asks for two bytes here and eight there. One
/// [`ByteSource::read`] per field would be free over a buffer and a round trip
/// per integer over a network, which is the cost ADR-0005 exists to keep
/// visible rather than to hide. So a cursor refills in **windows**, and a
/// B-tree node or a run of section headers costs a handful of reads rather than
/// forty.
///
/// The window **grows** rather than starting large, because both mistakes are
/// real. One read per field is a round trip per integer; one fixed 4 KiB window
/// per structure read six times the whole file on a 31 KB HDF5 fixture, because
/// most structures are a few dozen bytes. Starting at [`FIRST_WINDOW_BYTES`] and
/// doubling to [`MAX_WINDOW_BYTES`] bounds the round trips on a long structure
/// and the over-read on a short one at once.
///
/// A window is clamped at the cursor's end — the end of the source, or of the
/// one structure [`within`](Self::within) bounds it to — so the last structure
/// never asks for bytes that are not there, and a read larger than the window is
/// still served in one call.
///
/// Written for the HDF5 traversal (#682) and hoisted here when the GRIB scan
/// needed the same thing (#697).
pub struct FileCursor<'a, S: ?Sized> {
    source: &'a S,
    /// Where the cursor stops: the source's size, or the end of the structure it
    /// was bounded to. Cached so every bounds check is local.
    end: u64,
    /// Address of byte zero of `window`.
    base: u64,
    window: Cow<'a, [u8]>,
    /// Position within `window`.
    pos: usize,
    /// How much the next refill asks for — see [`MAX_WINDOW_BYTES`].
    next_window: usize,
}

/// The cursor's place, not its bytes: a window can be 64 KiB, and a source need
/// not be `Debug` at all.
impl<S: ?Sized> std::fmt::Debug for FileCursor<'_, S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCursor")
            .field("position", &self.base.saturating_add(self.pos as u64))
            .field("end", &self.end)
            .field("window", &self.window.len())
            .finish()
    }
}

impl<'a, S: ByteSource + ?Sized> FileCursor<'a, S> {
    /// A cursor positioned at address `addr`, running to the end of the source.
    ///
    /// Nothing is read yet — the first field read fills the window. The address
    /// itself is checked here, so a structure pointer past the end of the file
    /// is refused at the seek rather than at whatever it would have read.
    pub fn at(source: &'a S, addr: u64) -> Result<Self, FieldglassError> {
        Self::within(source, addr, source.size())
    }

    /// A cursor positioned at `addr` that treats `end` as the end of the file.
    ///
    /// For a walk over one structure whose extent is already known — a GRIB
    /// message, whose total length its indicator states — so no window runs
    /// into the next one, and the clamped reads below stop where the structure
    /// does rather than where the source does.
    pub fn within(source: &'a S, addr: u64, end: u64) -> Result<Self, FieldglassError> {
        if end > source.size() || addr > end {
            return Err(FieldglassError::Parse("address past end of file".into()));
        }
        Ok(Self {
            source,
            end,
            base: addr,
            window: Cow::Borrowed(&[]),
            pos: 0,
            next_window: FIRST_WINDOW_BYTES,
        })
    }

    /// The address the cursor is about to read from.
    ///
    /// Saturating rather than bare: this cannot overflow under the cursor's own
    /// invariant (`base <= end`, and the window never runs past it), but that
    /// last step rests on a [`ByteSource`] returning no *more* than it was asked
    /// for, which is a contract and not a type.
    pub fn position(&self) -> u64 {
        self.base.saturating_add(self.pos as u64)
    }

    /// How many bytes lie between the cursor and its end.
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.position())
    }

    /// Make at least `n` bytes available from the current position.
    ///
    /// Refills when they are not already in the window, growing what it asks
    /// for as a structure turns out to be long — see [`MAX_WINDOW_BYTES`].
    fn need(&mut self, n: usize) -> Result<(), FieldglassError> {
        if self
            .pos
            .checked_add(n)
            .is_some_and(|e| e <= self.window.len())
        {
            return Ok(());
        }
        let addr = self.position();
        let available = self.remaining();
        if (n as u64) > available {
            return Err(FieldglassError::Parse("read past end of file".into()));
        }
        let want = (n.max(self.next_window) as u64).min(available);
        self.next_window = self.next_window.saturating_mul(2).min(MAX_WINDOW_BYTES);
        // `base`/`pos` move with the window rather than after it, so the three
        // never disagree — including on the error return below. A cursor whose
        // position pointed into a window it no longer held would be a trap for
        // the first caller that kept it past an error.
        self.window = self.source.read(ByteRange::new(addr, want))?;
        self.base = addr;
        self.pos = 0;
        // A source that served short would leave the window smaller than the
        // parse is about to index. Catching it here is what keeps every reader
        // above from having to.
        if self.window.len() < n {
            return Err(FieldglassError::ShortRead {
                at: addr,
                got: self.window.len() as u64,
                wanted: n as u64,
            });
        }
        Ok(())
    }

    /// Take `n` bytes, borrowed from the window the cursor is holding.
    ///
    /// Tied to the cursor rather than to the file, because a source that is not
    /// one contiguous buffer has nothing file-lived to lend. Callers that keep
    /// the bytes copy them out.
    pub fn take(&mut self, n: usize) -> Result<&[u8], FieldglassError> {
        self.need(n)?;
        let out = &self.window[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    /// Look at up to `n` bytes without consuming them, stopping at the
    /// cursor's end.
    ///
    /// For a parser that takes "the rest of the structure" and checks a length
    /// field inside it: handing it `min(n, remaining)` bytes reproduces exactly
    /// what slicing a whole-file buffer to the structure's end would have, so
    /// a length that overruns is reported with the same count it always was.
    pub fn peek_up_to(&mut self, n: usize) -> Result<&[u8], FieldglassError> {
        let n = (n as u64).min(self.remaining()) as usize;
        self.need(n)?;
        Ok(&self.window[self.pos..self.pos + n])
    }

    /// Advance `n` bytes without reading them.
    ///
    /// Skipping does not fetch: a field or a whole section the reader does not
    /// want costs nothing over a transport. Skipping past the end is still an
    /// error, because the structure said it had bytes there.
    pub fn skip(&mut self, n: usize) -> Result<(), FieldglassError> {
        if self
            .pos
            .checked_add(n)
            .is_some_and(|e| e <= self.window.len())
        {
            self.pos += n;
            return Ok(());
        }
        let addr = self
            .position()
            .checked_add(n as u64)
            .filter(|&a| a <= self.end)
            .ok_or_else(|| FieldglassError::Parse("skip past end of file".into()))?;
        self.base = addr;
        self.pos = 0;
        self.window = Cow::Borrowed(&[]);
        Ok(())
    }

    /// Take four bytes and require them to be `signature`.
    pub fn tag(&mut self, signature: &[u8; 4]) -> Result<(), FieldglassError> {
        let got = self.take(4)?;
        if got != signature {
            return Err(FieldglassError::Parse(format!(
                "expected signature {:?}, got {:?}",
                std::str::from_utf8(signature).unwrap_or("?"),
                String::from_utf8_lossy(got)
            )));
        }
        Ok(())
    }
}

/// Shared bounds check, so every in-memory implementation reports a range past
/// the end the same way.
fn slice_of(bytes: &[u8], range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
    let end = range.end().ok_or_else(|| {
        FieldglassError::Parse(format!(
            "byte range [{}, +{}) overflows u64",
            range.start, range.len
        ))
    })?;
    let (start, end) = (usize::try_from(range.start).ok(), usize::try_from(end).ok());
    match (start, end) {
        // `end > len` and the `usize` conversion failing are the same answer
        // for an in-memory source: the bytes are not there.
        (Some(start), Some(end)) if end <= bytes.len() => Ok(Cow::Borrowed(&bytes[start..end])),
        _ => Err(FieldglassError::Parse(format!(
            "byte range [{}, +{}) exceeds source size {}",
            range.start,
            range.len,
            bytes.len()
        ))),
    }
}

impl ByteSource for [u8] {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn identity(&self) -> Option<SourceIdentity> {
        Some(SourceIdentity::of_buffer(self))
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        slice_of(self, range)
    }
}

impl ByteSource for Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn identity(&self) -> Option<SourceIdentity> {
        Some(SourceIdentity::of_buffer(self))
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        slice_of(self, range)
    }
}

/// So a caller holding `&S` can pass it where a `ByteSource` is wanted, which
/// is what lets a reader borrow its source rather than own it.
impl<S: ByteSource + ?Sized> ByteSource for &S {
    fn size(&self) -> u64 {
        (**self).size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        (**self).identity()
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        (**self).prefetch(ranges)
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        (**self).read(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slice_reads_borrowed_not_copied() {
        let data: Vec<u8> = (0..64u8).collect();
        let got = data.read(ByteRange::new(8, 4)).expect("in range");
        assert_eq!(&*got, &[8, 9, 10, 11]);
        // The whole point of `Cow` here: the in-memory path must not allocate,
        // or migrating a reader to the trait would cost a copy per slab.
        assert!(
            matches!(got, Cow::Borrowed(_)),
            "an in-memory source must lend its bytes, not clone them"
        );
        assert!(std::ptr::eq(got.as_ptr(), data[8..].as_ptr()));
    }

    #[test]
    fn a_range_past_the_end_is_an_error_not_a_panic() {
        let data = vec![0u8; 16];
        for range in [
            ByteRange::new(16, 1),
            ByteRange::new(0, 17),
            ByteRange::new(u64::MAX, 1),
            ByteRange::new(1, u64::MAX),
        ] {
            assert!(
                data.read(range).is_err(),
                "{range:?} should be rejected, not sliced"
            );
        }
        // The boundary itself is fine, and so is an empty range at it.
        assert!(data.read(ByteRange::new(16, 0)).is_ok());
        assert!(data.read(ByteRange::new(0, 16)).is_ok());
    }

    #[test]
    fn prefetch_is_a_no_op_that_still_has_to_be_callable() {
        let data = vec![0u8; 8];
        // Ranges that would fail to read are not an error to prefetch: the call
        // is advisory, and a source is free to ignore it entirely.
        assert!(
            data.prefetch(&[ByteRange::new(0, 4), ByteRange::new(99, 4)])
                .is_ok()
        );
    }

    #[test]
    fn checked_usize_passes_through_what_the_target_can_address() {
        assert_eq!(checked_usize(0, "n").unwrap(), 0);
        assert_eq!(checked_usize(4_294_967_295, "n").unwrap(), 4_294_967_295);
    }

    #[test]
    fn checked_usize_refuses_to_wrap_a_value_a_32_bit_target_cannot_hold() {
        // 0x1_0000_0000 fits `u64` and does not fit a 32-bit `usize`. The
        // assertion has to hold on both widths, because the point of the helper
        // is that the two disagree with an error rather than with a value: a
        // 64-bit host returns the number unchanged, a 32-bit one refuses. What
        // is ruled out on either is the wrap `as usize` would have given.
        for value in [1u64 << 32, (1u64 << 32) + 7, u64::MAX] {
            match checked_usize(value, "field") {
                Ok(n) => {
                    assert_eq!(usize::BITS, 64, "only a 64-bit target can hold {value}");
                    assert_eq!(n as u64, value, "a value that fits must not be altered");
                }
                Err(err) => {
                    assert_eq!(usize::BITS, 32);
                    assert!(
                        err.to_string().contains("does not fit"),
                        "unexpected message: {err}"
                    );
                }
            }
        }
    }

    #[test]
    fn two_buffers_of_equal_length_have_different_identities() {
        // The whole point of #681: length alone said these were the same file.
        let (a, b) = (vec![0u8; 32], vec![1u8; 32]);
        assert_eq!(a.size(), b.size());
        assert_ne!(a.identity(), b.identity());
        // And a buffer is equal to itself, or no memo could ever hit.
        assert_eq!(a.identity(), a.identity());
    }

    #[test]
    fn a_buffer_and_a_slice_of_it_are_the_same_source() {
        // A reader holds a `Vec` and hands its traversal a `&[u8]`. Those are
        // the same bytes, so they have to be the same identity or every memo
        // keyed on this would miss on every call.
        let data = vec![7u8; 16];
        let as_slice: &[u8] = &data;
        assert_eq!(data.identity(), as_slice.identity());
    }

    #[test]
    fn a_source_that_will_not_say_is_never_reused() {
        struct Silent(Vec<u8>);
        impl ByteSource for Silent {
            fn size(&self) -> u64 {
                self.0.len() as u64
            }
            fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
                slice_of(&self.0, range)
            }
        }
        // The default declines, and a decline must not compare equal to another
        // decline — that is what keeps an unidentified source out of a memo.
        let (a, b) = (Silent(vec![0; 8]), Silent(vec![0; 8]));
        // Byte-identical and the same length, so anything derived from `size()`
        // would call them one source. The default says nothing instead, and a
        // memo asks `Some(x) == Some(y)`, which no `None` can satisfy.
        assert_eq!(a.identity(), None);
        assert_eq!(b.identity(), None);
    }

    #[test]
    fn refilling_one_buffer_in_place_is_a_different_source() {
        // The address does not move and the length does not change, so an
        // identity built from those two alone would go on answering for the
        // first file. A reader that recycles one buffer across files is an
        // ordinary thing to write, which is why this is not a hypothetical.
        let mut buf = vec![0xAAu8; 4096];
        let before = buf.identity();
        buf.copy_from_slice(&[0xBBu8; 4096]);
        let after = buf.identity();
        assert_ne!(before, after, "refilling a buffer must change its identity");

        // And putting the original contents back gets the original identity:
        // it is the bytes that are being identified, not the event of writing.
        buf.copy_from_slice(&[0xAAu8; 4096]);
        assert_eq!(buf.identity(), before);
    }

    #[test]
    fn two_buffers_that_differ_only_in_the_middle_are_told_apart() {
        // The sample is taken from both ends, so this is the case it is
        // weakest at. Guard the boundary it does cover: a difference inside
        // either sampled end is caught however far in it sits.
        let base = vec![9u8; SAMPLE_BYTES * 2];
        for at in [0, 1, SAMPLE_BYTES - 1, SAMPLE_BYTES * 2 - 1] {
            let mut other = base.clone();
            other[at] = 0;
            assert_ne!(
                sample_of(&base),
                sample_of(&other),
                "a byte changed at {at} must change the sample"
            );
        }
    }

    #[test]
    fn a_short_buffer_still_samples_without_panicking() {
        // Head and tail overlap below twice the sample, and a buffer can be
        // shorter than the sample or empty. None of that may index out of
        // bounds.
        for len in [0usize, 1, SAMPLE_BYTES - 1, SAMPLE_BYTES, SAMPLE_BYTES + 1] {
            let bytes = vec![1u8; len];
            let _ = bytes.identity();
        }
        assert_ne!(sample_of(&[1u8, 2]), sample_of(&[2u8, 1]));
    }

    #[test]
    fn a_named_source_is_identified_by_name_and_size() {
        let a = SourceIdentity::named("s3://bucket/one.nc", 4096);
        assert_eq!(a, SourceIdentity::named("s3://bucket/one.nc", 4096));
        assert_ne!(a, SourceIdentity::named("s3://bucket/two.nc", 4096));
        // Same name, different size: the object changed, so the memo must not
        // carry over.
        assert_ne!(a, SourceIdentity::named("s3://bucket/one.nc", 8192));
        // A name is not a buffer, whatever the numbers.
        assert_ne!(a, SourceIdentity::of_buffer(&[0u8; 4096]));
    }

    #[test]
    fn a_reference_forwards_to_its_source() {
        let data = vec![1u8, 2, 3, 4];
        let by_ref: &Vec<u8> = &data;
        assert_eq!(by_ref.size(), 4);
        assert_eq!(&*by_ref.read(ByteRange::new(1, 2)).unwrap(), &[2, 3]);
        // Identity forwards too, or a reader that borrows its source would be
        // a different source from the one its caller holds.
        assert_eq!(by_ref.identity(), data.identity());
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    use crate::testing::{Recording, Short};

    fn word<S: ByteSource>(cur: &mut FileCursor<'_, S>) -> u32 {
        let b = cur.take(4).expect("word in range");
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    }

    #[test]
    fn a_cursor_cannot_be_placed_past_the_end_of_the_file() {
        let bytes: Vec<u8> = vec![0u8; 64];
        assert!(FileCursor::at(&bytes, 4096).is_err());
        // The end of the file is a legal position — it is where a zero-length
        // structure sits — and only reading from it fails.
        let mut at_end = FileCursor::at(&bytes, 64).expect("end is a position");
        assert!(at_end.take(1).is_err());
    }

    /// Reads that cross the window boundary have to see the same bytes a single
    /// window would, or every structure larger than a window is decoded from
    /// the wrong offsets.
    #[test]
    fn refilling_does_not_move_the_file_position() {
        // Each 4-byte little-endian word holds its own index, so a value read
        // at any offset names where it came from.
        let mut bytes = Vec::new();
        for i in 0..4096u32 {
            bytes.extend_from_slice(&i.to_le_bytes());
        }
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        for i in 0..4096u32 {
            assert_eq!(word(&mut cur), i);
        }
        assert!(cur.take(1).is_err(), "the file ends here");

        // Skipping across windows lands in the same place as reading across
        // them.
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        cur.skip(4 * 3000).expect("in range");
        assert_eq!(word(&mut cur), 3000);
        assert_eq!(cur.position(), 4 * 3001);
    }

    /// A read wider than the window is served whole rather than clipped to it.
    #[test]
    fn a_read_larger_than_the_window_is_one_read() {
        let bytes: Vec<u8> = (0..u8::MAX).cycle().take(MAX_WINDOW_BYTES * 3).collect();
        let mut cur = FileCursor::at(&bytes, 0).expect("in range");
        let got = cur.take(MAX_WINDOW_BYTES * 2).expect("in range");
        assert_eq!(got.len(), MAX_WINDOW_BYTES * 2);
        assert_eq!(got, &bytes[..MAX_WINDOW_BYTES * 2]);
    }

    /// The window grows, so a long run is a handful of reads rather than one
    /// per field — and a short one does not pay for a run it never makes.
    #[test]
    fn the_window_doubles_rather_than_starting_large() {
        let source = Recording::new(vec![0u8; 1 << 20]);

        // A short structure: one small read, not one large one.
        let mut cur = FileCursor::at(&source, 0).expect("in range");
        for _ in 0..8 {
            cur.take(4).expect("in range");
        }
        let lens: Vec<u64> = source.reads().iter().map(|r| r.len).collect();
        assert_eq!(
            lens,
            [FIRST_WINDOW_BYTES as u64],
            "32 bytes of fields should cost one 64-byte read"
        );

        // A long one: a few doubling reads, not 4096 one-field ones.
        source.clear();
        let mut cur = FileCursor::at(&source, 0).expect("in range");
        for _ in 0..4096 {
            cur.take(4).expect("in range");
        }
        let reads = source.reads();
        assert!(
            reads.len() <= 10,
            "16 KiB of fields took {} reads: {reads:?}",
            reads.len()
        );
        let fetched: u64 = reads.iter().map(|r| r.len).sum();
        assert!(
            fetched < 4 * 4096 * 2,
            "fetched {fetched} bytes for 16384 bytes of fields"
        );
    }

    /// `read_up_to` is for the reads whose length is not known yet, so a short
    /// file shortens the read rather than failing it — but a bad address still
    /// fails.
    #[test]
    fn read_up_to_shortens_at_the_end_of_the_file() {
        let bytes: Vec<u8> = vec![7u8; 10];
        assert_eq!(read_up_to(&bytes, 6, 16).expect("short read").len(), 4);
        assert_eq!(read_up_to(&bytes, 10, 16).expect("empty read").len(), 0);
        assert!(read_up_to(&bytes, 11, 1).is_err());
        assert!(read_at(&bytes, 6, 16).is_err());
    }

    /// The length check `read_exact` exists for: a source that serves short is
    /// an error, not a short slice the caller then indexes past.
    #[test]
    fn a_source_that_serves_short_is_an_error_not_a_short_slice() {
        let source = Short::new(vec![1u8; 32], 0, 1);
        let err = read_exact(&source, ByteRange::new(4, 8)).expect_err("served 7 of 8");
        // The variant, not the prose: a host branches on the shape to decide
        // whether to retry a truncated transfer or report a corrupt file, and a
        // substring match would keep passing if this became `Parse` again.
        assert!(
            matches!(
                err,
                FieldglassError::ShortRead {
                    at: 4,
                    got: 7,
                    wanted: 8
                }
            ),
            "unexpected error: {err}"
        );
        // The cursor holds the same line on its refill. A short window that
        // still covers the read is fine; one that does not cover it is not.
        let mut cur = FileCursor::at(&source, 0).expect("in range");
        assert!(cur.take(4).is_ok(), "31 of 64 bytes still covers four");
        let mut cur = FileCursor::at(&source, 0).expect("in range");
        let err = cur.take(32).expect_err("31 of 32 bytes does not cover 32");
        assert!(
            matches!(
                err,
                FieldglassError::ShortRead {
                    at: 0,
                    got: 31,
                    wanted: 32
                }
            ),
            "the cursor refill must raise the same variant: {err}"
        );
    }

    /// A cursor bounded to one structure never reads past it, however much
    /// file follows, and its clamped peek stops there too.
    #[test]
    fn a_bounded_cursor_stops_at_its_end_not_the_sources() {
        let source = Recording::new((0..=255u8).collect::<Vec<u8>>());
        let mut cur = FileCursor::within(&source, 10, 20).expect("in range");
        assert_eq!(cur.remaining(), 10);
        let peeked = cur.peek_up_to(100).expect("clamped").to_vec();
        assert_eq!(peeked, (10..20u8).collect::<Vec<_>>());
        // Peeking does not consume.
        assert_eq!(cur.position(), 10);
        assert_eq!(cur.take(3).expect("in range"), &[10, 11, 12]);
        assert!(cur.take(8).is_err(), "only seven bytes are left");
        assert!(cur.skip(8).is_err());
        cur.skip(7).expect("to the end exactly");
        assert_eq!(cur.peek_up_to(5).expect("empty at the end"), &[] as &[u8]);
        for r in source.reads().iter() {
            assert!(
                r.start >= 10 && r.end().is_some_and(|e| e <= 20),
                "read {r:?} left the structure"
            );
        }

        assert!(
            FileCursor::within(&source, 0, 257).is_err(),
            "end past the source"
        );
        assert!(
            FileCursor::within(&source, 21, 20).is_err(),
            "start past the end"
        );
    }

    /// The contract `find_forward` is written to: the answer a byte-at-a-time
    /// search gives. Checked against one, from every start, over data with the
    /// pattern planted on and across the first few window boundaries.
    #[test]
    fn a_forward_search_agrees_with_a_byte_at_a_time_one() {
        let mut bytes = vec![0u8; 1000];
        for at in [0usize, 61, 62, 63, 64, 125, 190, 191, 500, 996] {
            bytes[at..at + 4].copy_from_slice(b"GRIB");
        }
        let hit = |w: &[u8]| &w[..4] == b"GRIB";
        for span in [4usize, 8, 16, 100] {
            for from in 0..bytes.len() as u64 + 2 {
                let naive = (from as usize..)
                    .take_while(|p| p + span <= bytes.len())
                    .find(|&p| hit(&bytes[p..p + span]))
                    .map(|p| p as u64);
                let got = find_forward(&bytes, from, span, hit).expect("in memory");
                assert_eq!(got, naive, "span {span}, from {from}");
            }
        }
        assert!(find_forward(&bytes, 0, 0, |_| true).is_err());
    }

    /// Garbage costs windows, not bytes: a megabyte before the match is a
    /// couple of dozen reads, where a naive search over a transport would be a
    /// million.
    #[test]
    fn a_forward_search_over_garbage_reads_in_growing_windows() {
        let mut bytes = vec![0u8; 1 << 20];
        let at = bytes.len() - 10;
        bytes[at..at + 4].copy_from_slice(b"GRIB");
        let source = Recording::new(bytes);
        let got = find_forward(&source, 0, 8, |w| &w[..4] == b"GRIB").expect("in memory");
        assert_eq!(got, Some(at as u64));
        let reads = source.reads();
        // Ten doublings from 64 B to 64 KiB, then sixteen full windows.
        assert!(
            reads.len() <= 30,
            "{} reads for 1 MiB of garbage",
            reads.len()
        );
        assert!(reads.iter().all(|r| r.len <= MAX_WINDOW_BYTES as u64));
    }
}
