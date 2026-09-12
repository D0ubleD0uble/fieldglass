//! A kerchunk reference document, plus the objects a host fetched, as an
//! [`ObjectSource`] (#705, ADR-0010).
//!
//! A reference document maps Zarr keys onto three things: bytes written inline,
//! a whole object, or a `[url, offset, length]` range of somebody else's object.
//! That is a store's worth of keys with no store — the metadata documents are in
//! the document itself, and the chunks are slices of archives nobody rewrote.
//!
//! Paired with a [`ByteSource`] per URL, it *is* an [`ObjectSource`], which means
//! [`ZarrStore`] reads a NetCDF archive or a GRIB collection described by
//! kerchunk with **no reader change**. That is what ADR-0010 means by "planned
//! and decoded with no reader change", and it is why the #658 amendment puts Zarr
//! at the IO level rather than beside the format crates.
//!
//! # A source per URL, not a buffer per key
//!
//! The host brings one [`ByteSource`] for each object the document points into —
//! which is what it already has under ADR-0005, whether that is a file it opened,
//! ranges it fetched over HTTP, or an object from a bucket. A ranged key is then
//! a `read` of that source, and a key the document addresses inline needs no
//! source at all.
//!
//! The alternative — asking the host for whole objects keyed by URL — would
//! defeat the point: a reference document exists so a reader can take a
//! twenty-four-byte chunk out of a multi-gigabyte archive without holding the
//! archive.
//!
//! [`ZarrStore`]: https://docs.rs/fieldglass-zarr/latest/fieldglass_zarr/struct.ZarrStore.html

use std::borrow::Cow;
use std::collections::BTreeMap;

use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{ByteRange, ByteSource, ObjectSource, read_exact};

use crate::kerchunk::KerchunkRefs;

/// A reference document read through the objects it points into.
///
/// Built with [`new`](Self::new) and one [`insert`](Self::insert) per URL the
/// document names. [`urls`](Self::urls) says which those are, so a host can ask
/// the document what to fetch before it fetches anything.
#[derive(Debug)]
pub struct KerchunkObjects<S> {
    refs: KerchunkRefs,
    objects: BTreeMap<String, S>,
}

impl<S> KerchunkObjects<S> {
    /// The document, with no objects behind it yet.
    ///
    /// Useful on its own: every metadata document a reference file carries is
    /// written inline, so a store's *structure* reads with no object at all. Only
    /// a chunk read needs one.
    pub fn new(refs: KerchunkRefs) -> Self {
        Self {
            refs,
            objects: BTreeMap::new(),
        }
    }

    /// Give the source for one URL, replacing any already given for it.
    pub fn insert(&mut self, url: impl Into<String>, source: S) -> &mut Self {
        self.objects.insert(url.into(), source);
        self
    }

    /// Every URL the document addresses a range or a whole object in, sorted.
    ///
    /// What a host fetches, and the reason it can fetch before it reads: the
    /// document names its objects, so nothing has to be discovered by trying.
    pub fn urls(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .refs
            .keys()
            .filter_map(|key| self.refs.range_of(key).map(|item| item.key))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// The document underneath.
    pub fn refs(&self) -> &KerchunkRefs {
        &self.refs
    }
}

impl<S: ByteSource> KerchunkObjects<S> {
    /// The range one key occupies in one of the objects, with both ends known.
    ///
    /// `Ok(None)` for a key the document does not address as a fetch — absent, or
    /// inline. `Err` when it *is* a fetch and the object behind it was not given:
    /// that is the host's mistake and the error names the range, because a silent
    /// empty object would read as a fill value and put wrong numbers on a screen.
    fn located(&self, key: &str) -> Result<Option<(&S, ByteRange)>, FieldglassError> {
        let Some(item) = self.refs.range_of(key) else {
            return Ok(None);
        };
        let Some(source) = self.objects.get(&item.key) else {
            return Err(FieldglassError::Parse(format!(
                "the reference document puts {key:?} at {} of {:?}, and no source was given for \
                 that object — call `KerchunkObjects::insert` for every URL `urls()` lists",
                item.range
                    .http_range_header()
                    .unwrap_or_else(|| "the whole object".to_string()),
                item.key
            )));
        };
        let range = item
            .range
            .close(source.size())
            .map_err(|e| FieldglassError::Parse(e.to_string()))?;
        Ok(Some((source, range)))
    }
}

impl<S: ByteSource> ObjectSource for KerchunkObjects<S> {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        // Inline first: a reference document exists so the metadata documents
        // answer without a fetch, and `range_of` says `None` for them anyway.
        if let Some(bytes) = self.refs.inline(key) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        match self.located(key)? {
            // `read_exact`, not `read`: a transport that served short would
            // otherwise hand back a chunk the decoder sizes from its metadata
            // and reads past. That is `FieldglassError::ShortRead` (#707), which
            // a host can retry, rather than a malformed-chunk report.
            Some((source, range)) => Ok(Some(read_exact(source, range)?)),
            None => Ok(None),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        // The document's keys are already sorted, so the seam's ordering
        // contract costs a filter rather than a sort.
        Ok(self
            .refs
            .keys()
            .filter(|key| key.starts_with(prefix))
            .map(str::to_string)
            .collect())
    }

    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        // The ranges the document already plans, grouped by the object they fall
        // in, and handed to each source as one batch. That is the whole reason a
        // reference document is worth having over a transport: the reader knows
        // every byte it wants before it asks for any of them.
        //
        // A key with no source is *not* an error here. `prefetch` is advisory on
        // this seam, so a batch that mentions an object the host did not bring
        // should cost the caller nothing until it actually reads that key — where
        // `located` names it.
        let mut batches: BTreeMap<String, Vec<ByteRange>> = BTreeMap::new();
        for key in keys {
            let Some(item) = self.refs.range_of(key) else {
                continue; // absent, or inline: neither is a fetch
            };
            // `located` re-reads the document, and `unwrap_or(None)` is the
            // advisory half: a key whose object the host did not bring drops out
            // here and is named when it is actually read.
            let Some((_, range)) = self.located(key).unwrap_or(None) else {
                continue;
            };
            batches.entry(item.key).or_default().push(range);
        }
        for (url, ranges) in batches {
            if let Some(source) = self.objects.get(&url) {
                source.prefetch(&ranges)?;
            }
        }
        Ok(())
    }
}
