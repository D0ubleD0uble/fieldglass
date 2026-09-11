//! Sources that behave the way a transport does, for tests (#708).
//!
//! Every reader on the [`bytes`](crate::bytes) seams needs the same few
//! shapes to test against: something that records what it was asked for,
//! something that serves fewer bytes than it promised, something that holds
//! only the one range a sidecar index named, something that owns nothing the
//! caller can borrow. They were written out again in each of the five test
//! files that check a seam, and each new reader wrote them a sixth time.
//!
//! They are here instead, generic over what they wrap, so a source can be
//! stacked on a buffer, on a store, or on another one of these.
//!
//! # The feature
//!
//! Behind `testing`, which is enabled only from a `[dev-dependencies]` entry —
//! including `fieldglass-core`'s own, so its integration tests see it. Cargo's
//! v2 resolver keeps a dev-dependency's features out of a build that does not
//! build test targets, so nothing a consumer links carries any of this.
//!
//! ```
//! use fieldglass_core::bytes::{ByteRange, ByteSource};
//! use fieldglass_core::testing::Recording;
//!
//! let source = Recording::new(&b"0123456789"[..]);
//! source.prefetch(&[ByteRange::new(0, 4)])?;
//! assert_eq!(&*source.read(ByteRange::new(0, 4))?, b"0123");
//!
//! // What a test can say with this that it could not without: the read came
//! // out of the batch, rather than being a request of its own.
//! assert_eq!(source.prefetches(), [[ByteRange::new(0, 4)]]);
//! assert_eq!(source.reads(), [ByteRange::new(0, 4)]);
//! # Ok::<(), fieldglass_core::FieldglassError>(())
//! ```

use std::borrow::Cow;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};

use crate::bytes::{ByteRange, ByteSource, ObjectSource, SourceIdentity};
use crate::error::FieldglassError;

/// Wraps any source and records what was asked of it.
///
/// Otherwise transparent: every call forwards, including
/// [`identity`](ByteSource::identity) and any
/// [`list_children`](ObjectSource::list_children) override the wrapped store
/// defines, so a decode through this is the decode without it.
///
/// It records **both** seams, because one wrapper over both is what lets the
/// reader tests and the store tests make the same kind of claim. A wrapped
/// [`ByteSource`] only ever fills the range logs and a wrapped
/// [`ObjectSource`] only the key logs; the other side stays empty.
///
/// What a test gets out of it is the property the seam exists for and the one
/// an implementation silently loses first: that an operation **prefetched in
/// one batch and then read exactly that**, rather than merely that it
/// succeeded.
#[derive(Debug, Default)]
pub struct Recording<S> {
    inner: S,
    ranges: RefCell<Vec<ByteRange>>,
    range_batches: RefCell<Vec<Vec<ByteRange>>>,
    /// How many reads had happened when the first batch arrived, so a test can
    /// separate a traversal's reads from the data reads that follow it.
    ranges_before_first_batch: Cell<Option<usize>>,
    keys: RefCell<Vec<String>>,
    key_batches: RefCell<Vec<Vec<String>>>,
    listings: RefCell<Vec<(String, Vec<String>)>>,
}

impl<S> Recording<S> {
    /// Record what is asked of `inner`.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            ranges: RefCell::new(Vec::new()),
            range_batches: RefCell::new(Vec::new()),
            ranges_before_first_batch: Cell::new(None),
            keys: RefCell::new(Vec::new()),
            key_batches: RefCell::new(Vec::new()),
            listings: RefCell::new(Vec::new()),
        }
    }

    /// The source underneath.
    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Every range [`ByteSource::read`] was called with, in order.
    pub fn reads(&self) -> Vec<ByteRange> {
        self.ranges.borrow().clone()
    }

    /// Every [`ByteSource::prefetch`] batch, in order.
    ///
    /// A `Vec` per call rather than one flat list, because "one batch" is
    /// usually the property under test: an operation that prefetched each
    /// range separately would make a remote source issue one request per
    /// section.
    pub fn prefetches(&self) -> Vec<Vec<ByteRange>> {
        self.range_batches.borrow().clone()
    }

    /// The reads that came after the first prefetch batch — a variable's
    /// stored data, as opposed to the traversal that located it.
    pub fn reads_after_batch(&self) -> Vec<ByteRange> {
        match self.ranges_before_first_batch.get() {
            Some(mark) => self.ranges.borrow()[mark..].to_vec(),
            None => Vec::new(),
        }
    }

    /// Every key [`ObjectSource::get`] was called with, in order, including
    /// the ones the store did not hold.
    pub fn gets(&self) -> Vec<String> {
        self.keys.borrow().clone()
    }

    /// Every [`ObjectSource::prefetch`] batch, in order.
    pub fn key_prefetches(&self) -> Vec<Vec<String>> {
        self.key_batches.borrow().clone()
    }

    /// Every listing, as the prefix asked for and the keys that came back.
    ///
    /// [`ObjectSource::list`] and [`ObjectSource::list_children`] both land
    /// here: what a test is usually asking is how much of the store a walk
    /// made it enumerate, and that is the same question either way.
    pub fn listings(&self) -> Vec<(String, Vec<String>)> {
        self.listings.borrow().clone()
    }

    /// Every key any listing returned, deduplicated and sorted.
    ///
    /// The one a walker is held to: a walk that lists a chunk key has made a
    /// bucket enumerate an array to find a metadata document.
    pub fn listed_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .listings
            .borrow()
            .iter()
            .flat_map(|(_, keys)| keys.iter().cloned())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Forget what has been asked of it, keeping the source.
    pub fn clear(&self) {
        self.ranges.borrow_mut().clear();
        self.range_batches.borrow_mut().clear();
        self.ranges_before_first_batch.set(None);
        self.keys.borrow_mut().clear();
        self.key_batches.borrow_mut().clear();
        self.listings.borrow_mut().clear();
    }
}

impl<S: ByteSource> ByteSource for Recording<S> {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        self.inner.identity()
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        if self.ranges_before_first_batch.get().is_none() {
            self.ranges_before_first_batch
                .set(Some(self.ranges.borrow().len()));
        }
        self.range_batches.borrow_mut().push(ranges.to_vec());
        self.inner.prefetch(ranges)
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.ranges.borrow_mut().push(range);
        self.inner.read(range)
    }
}

impl<O: ObjectSource> ObjectSource for Recording<O> {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        self.keys.borrow_mut().push(key.to_string());
        self.inner.get(key)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        let keys = self.inner.list(prefix)?;
        self.listings
            .borrow_mut()
            .push((prefix.to_string(), keys.clone()));
        Ok(keys)
    }

    /// Forwards rather than taking the provided default, so a store that
    /// answers children with a delimiter listing is recorded doing that and
    /// not recorded doing a whole-store `list` this wrapper turned it into.
    fn list_children(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        let keys = self.inner.list_children(prefix)?;
        self.listings
            .borrow_mut()
            .push((prefix.to_string(), keys.clone()));
        Ok(keys)
    }

    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        self.key_batches
            .borrow_mut()
            .push(keys.iter().map(|k| (*k).to_string()).collect());
        self.inner.prefetch(keys)
    }
}

/// Serves fewer bytes than it was asked for, from an address onward.
///
/// A buffer cannot do this and a truncated transfer can, which makes it the
/// one way the seam could silently shorten a field: a decoder that trusts the
/// length it asked for would read past what it got, or answer with a field
/// that looks complete and is not.
#[derive(Debug)]
pub struct Short<S> {
    inner: S,
    from: u64,
    shortfall: u64,
}

impl<S> Short<S> {
    /// Reads starting at or after `from` come back `shortfall` bytes short.
    pub fn new(inner: S, from: u64, shortfall: u64) -> Self {
        Self {
            inner,
            from,
            shortfall,
        }
    }
}

impl<S: ByteSource> ByteSource for Short<S> {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        self.inner.identity()
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let full = self.inner.read(range)?;
        if range.start < self.from || full.is_empty() {
            return Ok(full);
        }
        let keep = full
            .len()
            .saturating_sub(usize::try_from(self.shortfall).unwrap_or(usize::MAX));
        Ok(Cow::Owned(full[..keep].to_vec()))
    }
}

/// A response cut off at an absolute offset: a read running past it comes back
/// short, the way a truncated transfer would, while
/// [`size`](ByteSource::size) still claims the whole source.
#[derive(Debug)]
pub struct CutOff<S> {
    inner: S,
    cut: u64,
}

impl<S> CutOff<S> {
    /// Nothing at or after `cut` is ever served.
    pub fn new(inner: S, cut: u64) -> Self {
        Self { inner, cut }
    }
}

impl<S: ByteSource> ByteSource for CutOff<S> {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        self.inner.identity()
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let served = range.len.min(self.cut.saturating_sub(range.start));
        self.inner.read(ByteRange::new(range.start, served))
    }
}

/// Serves every read in full until armed, then half of every read: a source
/// whose *later* transfers came back truncated.
///
/// The difference from [`CutOff`] is where the truncation starts. This one
/// lets an operation get through its header and fail in its data, which is the
/// order a real transfer fails in and the one a decoder is least prepared for.
#[derive(Debug)]
pub struct Starved<S> {
    inner: S,
    armed: Cell<bool>,
}

impl<S> Starved<S> {
    /// Serving in full, until [`arm`](Self::arm).
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            armed: Cell::new(false),
        }
    }

    /// Serve half of every read from here on.
    pub fn arm(&self) {
        self.armed.set(true);
    }
}

impl<S: ByteSource> ByteSource for Starved<S> {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        self.inner.identity()
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let got = self.inner.read(range)?;
        if self.armed.get() {
            return Ok(Cow::Owned(got[..got.len() / 2].to_vec()));
        }
        Ok(got)
    }
}

/// A source of which only one range was ever fetched — what a host holds after
/// fetching the range a sidecar index gave for one message.
///
/// It knows the whole source's [`size`](ByteSource::size), and refuses any read
/// outside what it holds, so a reader that wandered outside its message says so
/// instead of quietly succeeding against a buffer that happens to hold the rest.
#[derive(Debug)]
pub struct OneRange<S> {
    inner: S,
    held: ByteRange,
}

impl<S> OneRange<S> {
    /// Only `held` was fetched.
    pub fn new(inner: S, held: ByteRange) -> Self {
        Self { inner, held }
    }
}

impl<S: ByteSource> ByteSource for OneRange<S> {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let inside = range.start >= self.held.start
            && range
                .end()
                .is_some_and(|end| end <= self.held.start + self.held.len);
        if !inside {
            return Err(FieldglassError::Parse(format!(
                "{range:?} was never fetched; only {:?} was",
                self.held
            )));
        }
        self.inner.read(range)
    }
}

/// A source that owns nothing the caller can borrow: every read copies.
///
/// This is what a transport that fetched into a `RefCell` is forced to be — a
/// reference cannot be handed out of a borrow guard — so decoding through it
/// proves a reader never depends on [`Cow::Borrowed`]. It names an identity
/// rather than hashing bytes it does not hold, which is also the only kind a
/// transport can offer.
#[derive(Debug)]
pub struct Copying<S> {
    inner: S,
    name: String,
}

impl<S> Copying<S> {
    /// Copy out of `inner`, under `name` as the identity a transport would give.
    pub fn new(inner: S, name: impl Into<String>) -> Self {
        Self {
            inner,
            name: name.into(),
        }
    }
}

impl<S: ByteSource> ByteSource for Copying<S> {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        Some(SourceIdentity::named(self.name.clone(), self.inner.size()))
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        Ok(Cow::Owned(self.inner.read(range)?.into_owned()))
    }
}

/// A cache-backed source that owns nothing until asked, and counts what it
/// fetched.
///
/// The closest thing to an answer for "will the shape survive its first remote
/// implementation" that can be written before there is one: it resolves during
/// [`prefetch`](ByteSource::prefetch), serves reads out of the cache, and still
/// works for a range that was never prefetched, since the batch is advisory.
#[derive(Debug)]
pub struct Fetching<S> {
    inner: S,
    cache: RefCell<HashMap<(u64, u64), Vec<u8>>>,
    fetches: Cell<usize>,
}

impl<S> Fetching<S> {
    /// Holding nothing yet.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            cache: RefCell::new(HashMap::new()),
            fetches: Cell::new(0),
        }
    }

    /// How many "requests" it made: one per batch, plus one per read that
    /// missed the cache.
    pub fn fetches(&self) -> usize {
        self.fetches.get()
    }
}

impl<S: ByteSource> ByteSource for Fetching<S> {
    fn size(&self) -> u64 {
        self.inner.size()
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        // One "request" per batch, which is the whole point of the batch.
        self.fetches.set(self.fetches.get() + 1);
        let mut cache = self.cache.borrow_mut();
        for range in ranges {
            let bytes = self.inner.read(*range)?.into_owned();
            cache.insert((range.start, range.len), bytes);
        }
        Ok(())
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        if let Some(hit) = self.cache.borrow().get(&(range.start, range.len)) {
            return Ok(Cow::Owned(hit.clone()));
        }
        // `read` must work whether or not a range was prefetched — skipping the
        // batch costs latency, not correctness. A remote source would issue a
        // single-range request here.
        self.fetches.set(self.fetches.get() + 1);
        Ok(Cow::Owned(self.inner.read(range)?.into_owned()))
    }
}

/// An [`ObjectSource`] that implements the two required methods and nothing
/// else, so a test can check that the provided ones reach through it.
///
/// [`MemoryObjects`](crate::bytes::MemoryObjects) would not do: it is the
/// implementation the provided methods are usually measured against, and an
/// override it grew later would quietly stop them being measured at all.
#[derive(Debug, Default)]
pub struct Minimal {
    objects: BTreeMap<String, Vec<u8>>,
}

impl<K: Into<String>> FromIterator<(K, Vec<u8>)> for Minimal {
    fn from_iter<I: IntoIterator<Item = (K, Vec<u8>)>>(iter: I) -> Self {
        Self {
            objects: iter.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        }
    }
}

impl ObjectSource for Minimal {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        Ok(self.objects.get(key).map(|bytes| Cow::Borrowed(&bytes[..])))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        Ok(self
            .objects
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}
