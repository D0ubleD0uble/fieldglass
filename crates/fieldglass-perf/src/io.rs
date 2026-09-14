//! What an operation asked its source for, reduced to what a transport pays.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::rc::Rc;

use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{ByteRange, ByteSource, MemoryObjects, ObjectSource, SourceIdentity};
use fieldglass_core::testing::Recording;

/// The I/O tier's two numbers for one operation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Io {
    /// Distinct bytes the operation needed: the union of the ranges it read or
    /// prefetched, or the sizes of the distinct objects it fetched. What a
    /// caching transport downloads.
    pub bytes: u64,
    /// Round trips: each prefetch batch, each read or `get` no earlier batch
    /// covered, and each listing. What an uncached transport issues.
    pub requests: u64,
}

/// A [`Recording`] the harness keeps a handle on after the session takes the
/// source.
///
/// A session owns what it is given, and core forwards the seams only through
/// `&S`, which a `'static` session cannot hold, and `Arc`, which asks for a
/// `Sync` the recording's `RefCell`s are not. `Rc` is the sharing that fits a
/// single-threaded harness, and these two newtypes are the forwarding that makes
/// an `Rc` a source. Every method forwards, so the recording sees exactly what
/// the reader asked.
pub(crate) struct SharedBytes(pub(crate) Rc<Recording<Vec<u8>>>);

impl ByteSource for SharedBytes {
    fn size(&self) -> u64 {
        self.0.size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        self.0.identity()
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        self.0.prefetch(ranges)
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.0.read(range)
    }
}

/// [`SharedBytes`] for a keyed store.
pub(crate) struct SharedObjects(pub(crate) Rc<Recording<MemoryObjects>>);

impl ObjectSource for SharedObjects {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        self.0.get(key)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        self.0.list(prefix)
    }

    fn list_children(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        self.0.list_children(prefix)
    }

    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        self.0.prefetch(keys)
    }
}

/// Where the I/O tier reads its numbers from.
pub(crate) enum Recorder {
    /// Not recorded: prepared [`Via::Memory`](crate::Via::Memory), or a codec.
    None,
    /// An open scenario prepared for recording, whose source does not exist yet.
    Pending,
    /// A single-object source, recorded by range.
    Ranges(Rc<Recording<Vec<u8>>>),
    /// A keyed store, recorded by key.
    Objects(Rc<Recording<MemoryObjects>>),
    /// A reader with no source seam, handed the whole file of this many bytes.
    Whole(u64),
}

impl Recorder {
    /// Drop what preparation asked for, so only the operation is counted.
    pub(crate) fn forget(&self) {
        match self {
            Recorder::Ranges(r) => r.clear(),
            Recorder::Objects(r) => r.clear(),
            _ => {}
        }
    }

    pub(crate) fn tally(&self) -> Io {
        match self {
            Recorder::None | Recorder::Pending => Io::default(),
            Recorder::Whole(bytes) => Io {
                bytes: *bytes,
                requests: 1,
            },
            Recorder::Ranges(r) => tally_ranges(&r.prefetches(), &r.reads()),
            Recorder::Objects(r) => tally_objects(
                r.inner(),
                &r.key_prefetches(),
                &r.gets(),
                r.listings().len(),
            ),
        }
    }
}

/// Requests and distinct bytes for a range-addressed source.
///
/// A read is a request of its own unless an earlier prefetch batch covered it
/// entirely — the order matters, since a batch that arrives after the read did
/// not save it a round trip. The reads and batches are not in one interleaved
/// log, so this treats every batch as preceding every read, which is the
/// generous reading; `Recording` keeps the split point if a reader is ever
/// found to read before it batches.
fn tally_ranges(batches: &[Vec<ByteRange>], reads: &[ByteRange]) -> Io {
    let batched: Vec<(u64, u64)> = batches.iter().flatten().map(span).collect();
    let uncovered = reads
        .iter()
        .map(span)
        .filter(|&(start, end)| !batched.iter().any(|&(s, e)| s <= start && end <= e))
        .count() as u64;
    let mut spans: Vec<(u64, u64)> = batched
        .iter()
        .copied()
        .chain(reads.iter().map(span))
        .collect();
    Io {
        bytes: union_len(&mut spans),
        requests: batches.len() as u64 + uncovered,
    }
}

fn span(range: &ByteRange) -> (u64, u64) {
    (range.start, range.start.saturating_add(range.len))
}

/// Total length of the union of half-open spans.
fn union_len(spans: &mut [(u64, u64)]) -> u64 {
    spans.sort_unstable();
    let mut total = 0;
    let mut current: Option<(u64, u64)> = None;
    for &(start, end) in spans.iter() {
        match current {
            Some((s, e)) if start <= e => current = Some((s, e.max(end))),
            Some((s, e)) => {
                total += e - s;
                current = Some((start, end));
            }
            None => current = Some((start, end)),
        }
    }
    total + current.map_or(0, |(s, e)| e - s)
}

fn tally_objects(
    store: &MemoryObjects,
    batches: &[Vec<String>],
    gets: &[String],
    listings: usize,
) -> Io {
    let batched: BTreeSet<&str> = batches.iter().flatten().map(String::as_str).collect();
    let uncovered = gets
        .iter()
        .filter(|key| !batched.contains(key.as_str()))
        .count() as u64;
    let distinct: BTreeSet<&str> = batched
        .iter()
        .copied()
        .chain(gets.iter().map(String::as_str))
        .collect();
    let bytes = distinct
        .iter()
        .filter_map(|key| store.get(key).ok().flatten().map(|b| b.len() as u64))
        .sum();
    Io {
        bytes,
        requests: batches.len() as u64 + uncovered + listings as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_and_repeated_ranges_count_once() {
        let r = |start, len| ByteRange::new(start, len);
        let io = tally_ranges(&[], &[r(0, 10), r(5, 10), r(0, 10), r(100, 1)]);
        assert_eq!(
            io,
            Io {
                bytes: 16,
                requests: 4
            }
        );
    }

    #[test]
    fn a_read_inside_a_batch_is_not_a_request() {
        let r = |start, len| ByteRange::new(start, len);
        let io = tally_ranges(&[vec![r(0, 100)]], &[r(10, 10), r(90, 20)]);
        assert_eq!(
            io,
            Io {
                bytes: 110,
                requests: 2
            }
        );
    }

    #[test]
    fn objects_count_distinct_present_keys() {
        let store = MemoryObjects::from_iter([("a", vec![0; 3]), ("b", vec![0; 5])]);
        let gets = ["a", "a", "b", "missing"].map(String::from);
        let io = tally_objects(&store, &[vec!["a".into()]], &gets, 2);
        assert_eq!(
            io,
            Io {
                bytes: 8,
                requests: 1 + 2 + 2
            }
        );
    }
}
