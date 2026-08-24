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
//! # What is deliberately not here
//!
//! **An identity.** ADR-0005 records that a `ByteSource` will need one stronger
//! than length, because the HDF5 traversal memo keys everything by file offset
//! and today aliases equal-length files. That memo is not migrating yet, so an
//! `identity()` here would be a method with no caller and no test — exactly the
//! signature-argued-in-advance the ADR set out to avoid. It arrives with the
//! HDF5 reader.
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

/// A half-open byte range `[start, start + len)` in a source.
///
/// `u64` rather than `usize` because the range describes a *file*, which may be
/// larger than a 32-bit address space even where the decode of any one slab is
/// not. Converting to `usize` is the reader's business, at the point it has
/// bounded the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteRange {
    pub start: u64,
    pub len: u64,
}

impl ByteRange {
    pub fn new(start: u64, len: u64) -> Self {
        Self { start, len }
    }

    /// One past the last byte, or `None` on overflow — which for a range built
    /// from a file's own header is a malformed file, not an internal error.
    pub fn end(&self) -> Option<u64> {
        self.start.checked_add(self.len)
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

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        slice_of(self, range)
    }
}

impl ByteSource for Vec<u8> {
    fn size(&self) -> u64 {
        self.len() as u64
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
    fn a_reference_forwards_to_its_source() {
        let data = vec![1u8, 2, 3, 4];
        let by_ref: &Vec<u8> = &data;
        assert_eq!(by_ref.size(), 4);
        assert_eq!(&*by_ref.read(ByteRange::new(1, 2)).unwrap(), &[2, 3]);
    }
}
