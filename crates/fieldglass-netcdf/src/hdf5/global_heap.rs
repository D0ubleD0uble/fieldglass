//! HDF5 global-heap (`GCOL`) reader (#174, under #33). Variable-length data —
//! a NetCDF-4 `DIMENSION_LIST`, a vlen-string attribute — does not live inline
//! with its dataset/attribute. Instead the inline element is a **global-heap
//! ID** (`length(4) + collection address(offset_size) + object index(4)`) that
//! points into a global-heap collection; this module dereferences that ID to the
//! object's bytes.
//!
//! A collection is signed `GCOL` and holds a run of objects, each with a small
//! header (index, reference count, size) followed by its data padded to an
//! 8-byte boundary. Index 0 is the trailing free-space object that marks the end.
//! Only the single-collection lookup the dimension layer needs is implemented;
//! anything unexpected returns a clear error rather than risk a silent misread,
//! matching `heap.rs`.
//!
//! Reference: HDF5 file format specification version 3, "Global Heap".

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, HashMap};

use super::source::{Fields, FileCursor, charge_metadata, read_at};
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{ByteSource, checked_usize};

const SIG_GLOBAL_HEAP: &[u8; 4] = b"GCOL";

/// Upper bound on object headers scanned across every collection one
/// [`Collections`] reads: guards a malformed size, and bounds the memo.
const MAX_GLOBAL_HEAP_OBJECTS: usize = 1 << 20;

/// The only collection version the specification defines.
const GLOBAL_HEAP_VERSION: u8 = 1;

/// What [`charge_metadata`] counts for the one object a lookup reads, against
/// `MAX_METADATA_BYTES` (#997).
///
/// libhdf5 sizes a new collection to the object it is created for
/// (`H5HG_insert`), so the format puts no ceiling on an object, and the
/// collection's own size is checked only against the end of the file: over a
/// source read by range, the size the host declares. An object is read whole,
/// so it is held to the cap before the read. The objects a lookup scans past
/// are skipped, not read, and are not charged. The `DIMENSION_LIST` objects
/// this reads are a few eight-byte references each.
const OBJECT_BYTES: &str = "bytes in one HDF5 global-heap object";

/// Refuse an object of `size` bytes past `MAX_METADATA_BYTES`.
fn check_object(size: u64) -> Result<usize, FieldglassError> {
    charge_metadata(&mut 0, size, OBJECT_BYTES)?;
    checked_usize(size, "global-heap object size")
}

/// Width of the fixed fields that open both the collection header
/// (`signature(4) + version(1) + reserved(3)`) and each object's header
/// (`index(2) + refcount(2) + reserved(4)`); a length-sized size follows each.
const HEADER_FIXED: usize = 8;

/// Each header, fixed fields and size together, padded to a multiple of eight,
/// so the data after it is aligned. libhdf5 writes them this way
/// (`H5HG_SIZEOF_HDR`, `H5HG_SIZEOF_OBJHDR`); the specification gives both as
/// 16 bytes, which is the same when lengths are 8 bytes. With 4-byte lengths
/// each header carries 4 bytes of padding after its size (#922).
fn header_len(length_size: usize) -> usize {
    (HEADER_FIXED + length_size).next_multiple_of(8)
}

/// Read the bytes of the object with `object_index` from the global-heap
/// collection at `collection_addr`. `length_size` is the superblock's size of
/// lengths (the width of the collection-size and object-size fields).
pub fn read_object<S: ByteSource + ?Sized>(
    source: &S,
    collection_addr: u64,
    object_index: u16,
    length_size: u8,
) -> Result<Vec<u8>, FieldglassError> {
    Collections::default().read_object(source, collection_addr, object_index, length_size)
}

/// The global-heap collections one listing has looked objects up in, each
/// scanned at most once (#921).
///
/// A collection is legitimately shared: netCDF-C puts the `DIMENSION_LIST`
/// references of many variables in one, and a lookup has to scan the
/// collection's objects in order to find one. Scanned afresh per lookup, a
/// collection of `n` objects that `n` variables each refer into cost `n²`
/// object headers. This remembers where each object scanned past lies and
/// where the scan stopped, so a later lookup either finds its object or
/// carries on from there.
///
/// It answers as a fresh scan would. A scan stops at the object it is looking
/// for, so of the objects either side of a malformed one, the one before is
/// found and the one after refused, whichever is asked for first.
///
/// Collections do not overlap: each is a block of the file of its own
/// (`H5HG_create` allocates one per collection), and its declared size says
/// where it ends. A collection header inside a collection already read here
/// is refused (#1549 review). Without that, `n` headers whose first object
/// jumps into one shared run of objects cost that run `n` times, each
/// remembered, from a file the size of the run. All the object headers
/// scanned here are also held together to [`MAX_GLOBAL_HEAP_OBJECTS`].
#[derive(Debug, Default)]
pub(crate) struct Collections {
    by_address: HashMap<u64, Scan>,
    /// Each collection read: start → end, the block its declared size covers.
    blocks: BTreeMap<u64, u64>,
    /// Object headers read so far, across every collection.
    objects: usize,
}

/// How far one collection has been scanned.
#[derive(Debug)]
struct Scan {
    /// The objects scanned past: index → where its data starts, and its size.
    /// The first object with an index is the one a lookup finds.
    found: HashMap<u16, (u64, u64)>,
    /// The file offset of the next object header.
    next: u64,
    /// The collection's bytes from `next` on. A `u64`, as every size here
    /// is: a collection is bounded by the size the source declares, which on
    /// a 32-bit target can be past `usize`.
    remaining: u64,
    /// The run ended: the free-space object, or no room for another header.
    ended: bool,
}

impl Collections {
    /// [`read_object`], scanning each collection at most once across calls.
    pub(crate) fn read_object<S: ByteSource + ?Sized>(
        &mut self,
        source: &S,
        collection_addr: u64,
        object_index: u16,
        length_size: u8,
    ) -> Result<Vec<u8>, FieldglassError> {
        if object_index == 0 {
            return Err(FieldglassError::Parse(
                "global-heap object index 0 is the free-space marker, not an object".into(),
            ));
        }
        let l = length_size as usize;
        let header_len = header_len(l);
        let header_bytes = header_len as u64;
        let Self {
            by_address,
            blocks,
            objects,
        } = self;
        let scan = match by_address.entry(collection_addr) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => {
                let (scan, end) = open(source, collection_addr, l)?;
                claim(blocks, collection_addr, end)?;
                e.insert(scan)
            }
        };
        if let Some(&(at, size)) = scan.found.get(&object_index) {
            let size = check_object(size)?;
            return Ok(read_at(source, at, size)?.into_owned());
        }

        let mut cur = FileCursor::at(source, scan.next)?;
        while !scan.ended && *objects < MAX_GLOBAL_HEAP_OBJECTS {
            if scan.remaining < header_bytes {
                scan.ended = true;
                break;
            }
            let index = cur.u16()?;
            cur.skip(2 + 4)?; // reference count + reserved
            let size = cur.uint(l)?;
            cur.skip(header_len - HEADER_FIXED - l)?; // alignment padding
            // Index 0 is the free-space object that terminates the run.
            if index == 0 {
                scan.ended = true;
                break;
            }
            // The object's data must fit in what the collection has left; this
            // both rejects a corrupt oversized object (a cross-object misread)
            // and keeps the padding below from overflowing. The scan stays at
            // this object, so a later lookup past it is refused the same way.
            let body_budget = scan.remaining - header_bytes;
            let padded = size
                .checked_next_multiple_of(8)
                .filter(|_| size <= body_budget)
                .ok_or_else(|| {
                    FieldglassError::Parse(
                        "global-heap object size exceeds the collection bounds".into(),
                    )
                })?;
            let at = cur.position();
            if index == object_index {
                let data = cur.take(check_object(size)?)?.to_vec();
                scan.passed(index, (at, size), padded, header_bytes);
                *objects += 1;
                return Ok(data);
            }
            // Skipped, not read: an object passed over costs nothing, however
            // large, so it is not held to the cap. `skip` keeps the cursor's
            // window when the next header is inside it, so a run of small
            // objects is read once; re-seating the cursor for each would drop
            // the window and read the rest of the collection again per object.
            // A skip `usize` cannot hold, past 4 GiB on a 32-bit target,
            // re-seats the cursor instead, so that target passes the object
            // as a 64-bit one does.
            match usize::try_from(padded) {
                Ok(padded) => cur.skip(padded)?,
                Err(_) => {
                    let next = at.checked_add(padded).ok_or_else(|| {
                        FieldglassError::Parse(
                            "global-heap object size exceeds the collection bounds".into(),
                        )
                    })?;
                    cur = FileCursor::at(source, next)?;
                }
            }
            scan.passed(index, (at, size), padded, header_bytes);
            *objects += 1;
        }

        Err(FieldglassError::Parse(format!(
            "global-heap object {object_index} not found in collection"
        )))
    }
}

#[cfg(test)]
impl Collections {
    /// Object headers scanned across every collection.
    fn objects_scanned(&self) -> usize {
        self.by_address.values().map(|s| s.found.len()).sum()
    }
}

impl Scan {
    /// Record the object `index` whose data is `size` bytes at `at`, and move
    /// past it and its padding.
    fn passed(&mut self, index: u16, (at, size): (u64, u64), padded: u64, header_len: u64) {
        self.found.entry(index).or_insert((at, size));
        self.next = at.saturating_add(padded);
        self.remaining = self
            .remaining
            .saturating_sub(header_len.saturating_add(padded));
    }
}

/// Record the collection block `start..end`, refusing one that overlaps a
/// block already recorded.
fn claim(blocks: &mut BTreeMap<u64, u64>, start: u64, end: u64) -> Result<(), FieldglassError> {
    // The blocks held are disjoint, so of those that start before `end`, only
    // the last can reach past `start`: every earlier one ends before it begins.
    if let Some((_, &before_end)) = blocks.range(..end).next_back()
        && before_end > start
    {
        return Err(FieldglassError::Parse(
            "global-heap collection overlaps another collection".into(),
        ));
    }
    blocks.insert(start, end);
    Ok(())
}

/// Read a collection's header and start a scan of its objects. Also returns
/// where the collection's block ends.
fn open<S: ByteSource + ?Sized>(
    source: &S,
    collection_addr: u64,
    l: usize,
) -> Result<(Scan, u64), FieldglassError> {
    let mut cur = FileCursor::at(source, collection_addr)?;
    cur.tag(SIG_GLOBAL_HEAP)?;
    let version = cur.byte()?;
    if version != GLOBAL_HEAP_VERSION {
        return Err(FieldglassError::Parse(format!(
            "global-heap collection version {version} is not supported"
        )));
    }
    cur.skip(3)?; // reserved
    // The declared collection size counts the whole collection (header + objects).
    // Clamp it to the bytes actually on disk so a corrupt size can't make the
    // budget arithmetic below overflow or run past the file.
    // `saturating_sub` rather than a bare one: the cursor above already refused
    // an address past the end of the file, so this cannot go negative — but a
    // subtraction that is only correct because of a check several lines away is
    // the kind that survives a reordering and underflows.
    let available = source.size().saturating_sub(collection_addr);
    let collection_size = cur.uint(l)?.min(available);
    // The collection size counts its own header, so the object run is whatever
    // remains after it. An object's header is the same length.
    let header_len = header_len(l);
    cur.skip(header_len - HEADER_FIXED - l)?; // alignment padding
    let scan = Scan {
        found: HashMap::new(),
        next: cur.position(),
        remaining: collection_size.saturating_sub(header_len as u64),
        ended: false,
    };
    // The block takes in at least the header just read, whatever size the
    // collection declares.
    let end = collection_addr.saturating_add(collection_size.max(header_len as u64));
    Ok((scan, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `GCOL` collection holding `objects` as `(index, data)` pairs,
    /// followed by the index-0 free-space terminator.
    fn collection(objects: &[(u16, &[u8])]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(SIG_GLOBAL_HEAP);
        body.push(1); // version
        body.extend_from_slice(&[0, 0, 0]); // reserved
        let size_pos = body.len();
        body.extend_from_slice(&0u64.to_le_bytes()); // collection size (patched)
        for &(index, data) in objects {
            body.extend_from_slice(&index.to_le_bytes());
            body.extend_from_slice(&1u16.to_le_bytes()); // reference count
            body.extend_from_slice(&[0, 0, 0, 0]); // reserved
            body.extend_from_slice(&(data.len() as u64).to_le_bytes());
            body.extend_from_slice(data);
            body.resize(
                body.len() + (data.len().next_multiple_of(8) - data.len()),
                0,
            );
        }
        // Free-space terminator: index 0, then the remaining size (unused here).
        body.extend_from_slice(&0u16.to_le_bytes());
        body.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // refcount + reserved
        body.extend_from_slice(&0u64.to_le_bytes());
        let total = body.len() as u64;
        body[size_pos..size_pos + 8].copy_from_slice(&total.to_le_bytes());
        body
    }

    #[test]
    fn reads_object_by_index() {
        let buf = collection(&[(1, &[0xAA, 0xBB]), (2, &[1, 2, 3, 4, 5])]);
        assert_eq!(read_object(&buf, 0, 1, 8).unwrap(), vec![0xAA, 0xBB]);
        assert_eq!(read_object(&buf, 0, 2, 8).unwrap(), vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn reads_reference_array_object() {
        // A DIMENSION_LIST axis: one 8-byte object-header address.
        let addr = 0x1234u64.to_le_bytes();
        let buf = collection(&[(3, &addr)]);
        let got = read_object(&buf, 0, 3, 8).unwrap();
        assert_eq!(u64::from_le_bytes(got.try_into().unwrap()), 0x1234);
    }

    #[test]
    fn missing_index_errors() {
        let buf = collection(&[(1, &[0xAA])]);
        assert!(read_object(&buf, 0, 9, 8).is_err());
    }

    #[test]
    fn rejects_free_space_index() {
        let buf = collection(&[(1, &[0xAA])]);
        assert!(read_object(&buf, 0, 0, 8).is_err());
    }

    #[test]
    fn bad_signature_errors() {
        let buf = vec![0u8; 64];
        assert!(read_object(&buf, 0, 1, 8).is_err());
    }

    #[test]
    fn oversized_object_size_errors_without_panic() {
        // A corrupt object whose declared size is near u64::MAX must yield a clean
        // error, not a `next_multiple_of` overflow panic or a cross-object read.
        let mut buf = Vec::new();
        buf.extend_from_slice(SIG_GLOBAL_HEAP);
        buf.push(1);
        buf.extend_from_slice(&[0, 0, 0]); // reserved
        buf.extend_from_slice(&64u64.to_le_bytes()); // collection size
        buf.extend_from_slice(&5u16.to_le_bytes()); // object index 5
        buf.extend_from_slice(&1u16.to_le_bytes()); // reference count
        buf.extend_from_slice(&[0, 0, 0, 0]); // reserved
        buf.extend_from_slice(&(u64::MAX - 1).to_le_bytes()); // bogus object size
        buf.resize(64, 0);
        // Look up a different index so the scan reaches the size-budget check.
        assert!(read_object(&buf, 0, 9, 8).is_err());
        // And the matched-object path is bounded too.
        assert!(read_object(&buf, 0, 5, 8).is_err());
    }

    /// Many lookups into one collection scan it once (#921): looked up from
    /// the last object back, a fresh scan per lookup read every object header
    /// before its own, `n²/2` of them.
    #[test]
    fn a_collection_is_scanned_once_however_many_lookups() {
        const N: u16 = 512;
        let data: Vec<[u8; 8]> = (1..=N).map(|i| u64::from(i).to_le_bytes()).collect();
        let objects: Vec<(u16, &[u8])> = (1..=N).zip(&data).map(|(i, d)| (i, &d[..])).collect();
        let buf = collection(&objects);
        let source = fieldglass_core::testing::Recording::new(buf.as_slice());
        let mut collections = Collections::default();
        for i in (1..=N).rev() {
            let got = collections.read_object(&source, 0, i, 8).unwrap();
            assert_eq!(got, u64::from(i).to_le_bytes());
        }
        let read: u64 = source.reads().iter().map(|r| r.len).sum();
        // The collection once, then each object's own eight bytes.
        assert!(
            read <= 2 * buf.len() as u64 + 8 * u64::from(N),
            "{read} bytes read from a {}-byte collection",
            buf.len()
        );
    }

    /// `count` collection headers that each jump, by the size of their first
    /// object, into one shared run of `run` empty objects after them, so each
    /// collection's range takes in the run and every header after its own.
    /// Returns the file and the collections' addresses.
    fn overlapping_collections(count: u64, run: u16) -> (Vec<u8>, Vec<u64>) {
        const BLOCK: u64 = 32; // collection header + its first object's header
        let run_start = count * BLOCK;
        let mut buf = Vec::new();
        let total = run_start + (u64::from(run) + 1) * 16;
        for k in 0..count {
            let at = k * BLOCK;
            buf.extend_from_slice(SIG_GLOBAL_HEAP);
            buf.extend_from_slice(&[1, 0, 0, 0]); // version + reserved
            buf.extend_from_slice(&(total - at).to_le_bytes()); // to the end of the file
            buf.extend_from_slice(&u16::MAX.to_le_bytes()); // an index the run lacks
            buf.extend_from_slice(&[1, 0, 0, 0, 0, 0]); // reference count + reserved
            buf.extend_from_slice(&(run_start - at - BLOCK).to_le_bytes()); // up to the run
        }
        buf.resize(run_start as usize, 0); // the jumped-over bytes
        for index in 1..=run {
            buf.extend_from_slice(&index.to_le_bytes());
            buf.extend_from_slice(&[1, 0, 0, 0, 0, 0]);
            buf.extend_from_slice(&0u64.to_le_bytes()); // an empty object
        }
        buf.extend_from_slice(&[0u8; 16]); // free-space object
        assert_eq!(buf.len() as u64, total);
        (buf, (0..count).map(|k| k * BLOCK).collect())
    }

    /// Collections do not overlap (each is its own block of the file), so a
    /// second header inside one already read is refused rather than scanned:
    /// otherwise `n` headers that each jump into one shared run of objects
    /// cost the run `n` times over, in time and in memory, from a file the
    /// size of the run (#1549 review: 400 of them took 1.29 GB).
    #[test]
    fn overlapping_collections_are_not_each_scanned() {
        const RUN: u16 = 65_000;
        let (buf, addresses) = overlapping_collections(16, RUN);
        let mut collections = Collections::default();
        let mut refused = 0;
        for &at in &addresses {
            // An index none of them holds: a lookup scans the whole run.
            let err = collections.read_object(&buf, at, RUN + 1, 8).unwrap_err();
            refused += usize::from(err.to_string().contains("overlaps"));
        }
        assert_eq!(refused, addresses.len() - 1);
        let scanned = collections.objects_scanned();
        assert!(
            scanned <= usize::from(RUN) + addresses.len(),
            "{scanned} object headers scanned from a run of {RUN}"
        );
    }

    /// The specification defines version 1 of a collection only.
    #[test]
    fn a_collection_of_another_version_is_refused() {
        let mut buf = collection(&[(1, &[0xAA])]);
        assert!(read_object(&buf, 0, 1, 8).is_ok());
        for version in [0, 2, 0xFF] {
            buf[4] = version;
            let err = read_object(&buf, 0, 1, 8).unwrap_err();
            assert!(err.to_string().contains("version"), "{version}: {err}");
        }
    }

    /// The memo answers as a fresh scan does on either side of a malformed
    /// object, in either order of asking: the object before it is found and
    /// the one after it refused.
    #[test]
    fn a_malformed_object_stops_the_scan_the_same_way_whatever_is_asked_first() {
        let mut buf = collection(&[(1, &[0xAA]), (2, &[0xBB]), (3, &[0xCC])]);
        // Object 2's header is at 16 + 24; give it a size past the collection.
        let size_at = 16 + 24 + 8;
        buf[size_at..size_at + 8].copy_from_slice(&(1u64 << 40).to_le_bytes());
        for order in [[1u16, 3], [3, 1]] {
            let mut collections = Collections::default();
            for index in order {
                let memo = collections.read_object(&buf, 0, index, 8);
                let fresh = read_object(&buf, 0, index, 8);
                assert_eq!(memo.is_ok(), fresh.is_ok(), "{order:?} {index}");
                assert_eq!(memo.is_ok(), index == 1, "{order:?} {index}");
            }
        }
    }
}
