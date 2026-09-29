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
pub(crate) struct SharedBytes<S = Vec<u8>>(pub(crate) Rc<Recording<S>>);

impl<S: ByteSource> ByteSource for SharedBytes<S> {
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

/// A range recording, whatever it wraps.
pub(crate) trait RangeLog {
    fn prefetches(&self) -> Vec<Vec<ByteRange>>;
    fn reads(&self) -> Vec<ByteRange>;
    fn reads_before_each_batch(&self) -> Vec<usize>;
    fn clear(&self);
}

impl<S> RangeLog for Recording<S> {
    fn prefetches(&self) -> Vec<Vec<ByteRange>> {
        Recording::prefetches(self)
    }

    fn reads(&self) -> Vec<ByteRange> {
        Recording::reads(self)
    }

    fn reads_before_each_batch(&self) -> Vec<usize> {
        Recording::reads_before_each_batch(self)
    }

    fn clear(&self) {
        Recording::clear(self);
    }
}

/// Where the I/O tier reads its numbers from.
pub(crate) enum Recorder {
    /// Not recorded: prepared [`Via::Memory`](crate::Via::Memory), or a codec.
    None,
    /// An open scenario prepared for recording, whose source does not exist yet.
    Pending,
    /// A single-object source, recorded by range.
    Ranges(Rc<dyn RangeLog>),
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
            Recorder::Ranges(r) => {
                tally_ranges(&r.prefetches(), &r.reads_before_each_batch(), &r.reads())
            }
            Recorder::Objects(r) => tally_objects(
                r.inner(),
                &r.key_prefetches(),
                &r.gets_before_each_batch(),
                &r.gets(),
                r.listings().len(),
            ),
        }
    }
}

/// Requests and distinct bytes for a range-addressed source.
///
/// A read is a request of its own unless a prefetch batch that came before it
/// covered it entirely: a batch that arrives after the read did not save it a
/// round trip. `before[j]` is how many reads came before batch `j`, so a scrub
/// that batches once per frame has each frame's reads matched against its own
/// and earlier batches, never the next frame's.
fn tally_ranges(batches: &[Vec<ByteRange>], before: &[usize], reads: &[ByteRange]) -> Io {
    let uncovered = reads
        .iter()
        .enumerate()
        .filter(|&(i, read)| {
            let (start, end) = span(read);
            !batches
                .iter()
                .zip(before)
                .take_while(|&(_, &mark)| mark <= i)
                .flat_map(|(batch, _)| batch.iter().map(span))
                .any(|(s, e)| s <= start && end <= e)
        })
        .count() as u64;
    let mut spans: Vec<(u64, u64)> = batches
        .iter()
        .flatten()
        .map(span)
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

/// Requests and distinct bytes for a keyed store: [`tally_ranges`] by key.
fn tally_objects(
    store: &MemoryObjects,
    batches: &[Vec<String>],
    before: &[usize],
    gets: &[String],
    listings: usize,
) -> Io {
    let uncovered = gets
        .iter()
        .enumerate()
        .filter(|&(i, key)| {
            !batches
                .iter()
                .zip(before)
                .take_while(|&(_, &mark)| mark <= i)
                .any(|(batch, _)| batch.contains(key))
        })
        .count() as u64;
    let batched: BTreeSet<&str> = batches.iter().flatten().map(String::as_str).collect();
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
        let io = tally_ranges(&[], &[], &[r(0, 10), r(5, 10), r(0, 10), r(100, 1)]);
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
        let io = tally_ranges(&[vec![r(0, 100)]], &[0], &[r(10, 10), r(90, 20)]);
        assert_eq!(
            io,
            Io {
                bytes: 110,
                requests: 2
            }
        );
    }

    #[test]
    fn a_read_before_the_batch_is_still_a_request() {
        let r = |start, len| ByteRange::new(start, len);
        // The first read came before the batch that covers it.
        let io = tally_ranges(&[vec![r(0, 100)]], &[1], &[r(10, 10), r(20, 10)]);
        assert_eq!(
            io,
            Io {
                bytes: 100,
                requests: 2
            }
        );
    }

    #[test]
    fn a_recorded_read_before_a_prefetch_is_counted() {
        let recording = Rc::new(Recording::new(vec![0u8; 200]));
        let recorder = Recorder::Ranges(Rc::clone(&recording) as Rc<dyn RangeLog>);
        recording.read(ByteRange::new(0, 10)).unwrap();
        recording.prefetch(&[ByteRange::new(0, 100)]).unwrap();
        recording.read(ByteRange::new(20, 10)).unwrap();
        assert_eq!(recorder.tally().requests, 2);
    }

    #[test]
    fn a_read_only_a_later_batch_covers_is_a_request() {
        // Two operations over one recording, each reading a header and then
        // batching: the second header read came after the first batch, but
        // only the second batch covers it.
        let recording = Rc::new(Recording::new(vec![0u8; 200]));
        let recorder = Recorder::Ranges(Rc::clone(&recording) as Rc<dyn RangeLog>);
        for start in [0, 100] {
            recording.read(ByteRange::new(start, 10)).unwrap();
            recording.prefetch(&[ByteRange::new(start, 100)]).unwrap();
            recording.read(ByteRange::new(start + 20, 10)).unwrap();
        }
        assert_eq!(
            recorder.tally(),
            Io {
                bytes: 200,
                requests: 2 + 2
            }
        );
    }

    #[test]
    fn a_get_only_a_later_batch_covers_is_a_request() {
        let store = MemoryObjects::from_iter([("a", vec![0; 3]), ("b", vec![0; 5])]);
        let recording = Rc::new(Recording::new(store));
        let recorder = Recorder::Objects(Rc::clone(&recording));
        recording.get("b").unwrap();
        recording.prefetch(&["a"]).unwrap();
        recording.get("a").unwrap();
        recording.prefetch(&["b"]).unwrap();
        recording.get("b").unwrap();
        assert_eq!(
            recorder.tally(),
            Io {
                bytes: 8,
                requests: 2 + 1
            }
        );
    }

    #[test]
    fn objects_count_distinct_present_keys() {
        let store = MemoryObjects::from_iter([("a", vec![0; 3]), ("b", vec![0; 5])]);
        let gets = ["a", "a", "b", "missing"].map(String::from);
        let io = tally_objects(&store, &[vec!["a".into()]], &[0], &gets, 2);
        assert_eq!(
            io,
            Io {
                bytes: 8,
                requests: 1 + 2 + 2
            }
        );
    }
}
