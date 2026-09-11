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
use std::cell::RefCell;
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
    /// One in-memory buffer, by where it begins and how far it runs.
    ///
    /// Meaningful only while that buffer is alive: an allocator may hand the
    /// same address out again once it is freed, so an identity that outlives
    /// its bytes can collide with a later buffer of the same length. Whatever
    /// keeps one has to keep it beside the bytes it came from — the HDF5 memo
    /// does, which is what makes the collision unreachable there rather than
    /// merely unlikely.
    Buffer {
        /// Address of the first byte, in this process.
        addr: usize,
        /// Length in bytes, so two buffers that begin together but run to
        /// different ends are still told apart.
        len: u64,
    },
    /// A name the host vouches for: a path, a URL, an object key.
    ///
    /// What a source that is not one contiguous buffer has to use. A sparse map
    /// of prefetched ranges has no single address to offer, and that is exactly
    /// the case where a memo is worth the most, since every walk it saves is a
    /// chain of dependent round-trips (ADR-0005).
    Named {
        /// How the host names this object. It has to be unique among the
        /// sources one reader is handed, and the host is the only thing that
        /// can promise that.
        name: String,
        /// Size in bytes, so a name reused for an object that has since changed
        /// length does not inherit the old one's memo.
        size: u64,
    },
}

impl SourceIdentity {
    /// The identity of an in-memory buffer.
    #[must_use]
    pub fn of_buffer(bytes: &[u8]) -> Self {
        Self::Buffer {
            addr: bytes.as_ptr().addr(),
            len: bytes.len() as u64,
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

/// An [`ObjectSource`] over a map, which also records what was asked of it.
///
/// Two jobs in one type on purpose. The in-memory store is what a test, a
/// fixture and an already-downloaded store all want; the recording is what lets
/// a test say a walker *prefetched before it read* rather than merely that it
/// succeeded, which is the property the seam exists for and the one an
/// implementation silently loses first.
#[derive(Debug, Default)]
pub struct MemoryObjects {
    objects: BTreeMap<String, Vec<u8>>,
    reads: RefCell<Vec<String>>,
    prefetches: RefCell<Vec<Vec<String>>>,
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

    /// Every key [`ObjectSource::get`] was called with, in order, including the
    /// ones that were absent.
    pub fn reads(&self) -> Vec<String> {
        self.reads.borrow().clone()
    }

    /// Every [`ObjectSource::prefetch`] batch, in order.
    ///
    /// A `Vec` per call rather than one flat list, because "one batch" is
    /// usually the property under test: a walker that prefetched each key
    /// separately would make a remote store issue one request per chunk.
    pub fn prefetches(&self) -> Vec<Vec<String>> {
        self.prefetches.borrow().clone()
    }

    /// Forget what has been asked of it, keeping the objects.
    pub fn clear_log(&self) {
        self.reads.borrow_mut().clear();
        self.prefetches.borrow_mut().clear();
    }
}

impl<K: Into<String>> FromIterator<(K, Vec<u8>)> for MemoryObjects {
    fn from_iter<I: IntoIterator<Item = (K, Vec<u8>)>>(iter: I) -> Self {
        Self {
            objects: iter.into_iter().map(|(k, v)| (k.into(), v)).collect(),
            ..Self::default()
        }
    }
}

impl ObjectSource for MemoryObjects {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        self.reads.borrow_mut().push(key.to_string());
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

    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        self.prefetches
            .borrow_mut()
            .push(keys.iter().map(|k| (*k).to_string()).collect());
        Ok(())
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
        assert_eq!(a.identity(), None);
        assert_eq!(b.identity(), None);
        assert!(
            !matches!((a.identity(), b.identity()), (Some(x), Some(y)) if x == y),
            "two silent sources must never be treated as the same bytes"
        );
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
