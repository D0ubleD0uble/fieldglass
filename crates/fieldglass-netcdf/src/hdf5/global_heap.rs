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

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use super::source::{Fields, FileCursor, charge_metadata, read_at};
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{ByteSource, checked_usize};

const SIG_GLOBAL_HEAP: &[u8; 4] = b"GCOL";

/// Upper bound on objects scanned in one collection — guards a malformed size.
const MAX_GLOBAL_HEAP_OBJECTS: usize = 1 << 20;

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
#[derive(Debug, Default)]
pub(crate) struct Collections {
    by_address: HashMap<u64, Scan>,
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
    /// Object headers read so far, against [`MAX_GLOBAL_HEAP_OBJECTS`].
    objects: usize,
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
        let scan = match self.by_address.entry(collection_addr) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(e) => e.insert(open(source, collection_addr, l)?),
        };
        if let Some(&(at, size)) = scan.found.get(&object_index) {
            let size = check_object(size)?;
            return Ok(read_at(source, at, size)?.into_owned());
        }

        let mut cur = FileCursor::at(source, scan.next)?;
        while !scan.ended && scan.objects < MAX_GLOBAL_HEAP_OBJECTS {
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
                return Ok(data);
            }
            // Skipped, not read: an object passed over costs nothing, however
            // large, so it is not held to the cap. It is skipped in steps a
            // 32-bit `usize` holds, so that target passes an object past 4 GiB
            // as a 64-bit one does rather than refusing the skip; a small one
            // is one step inside the cursor's window, which stays held.
            let mut left = padded;
            while left > 0 {
                let step = left.min(u64::from(u32::MAX));
                cur.skip(checked_usize(step, "global-heap object size")?)?;
                left -= step;
            }
            scan.passed(index, (at, size), padded, header_bytes);
        }

        Err(FieldglassError::Parse(format!(
            "global-heap object {object_index} not found in collection"
        )))
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
        self.objects += 1;
    }
}

/// Read a collection's header and start a scan of its objects.
fn open<S: ByteSource + ?Sized>(
    source: &S,
    collection_addr: u64,
    l: usize,
) -> Result<Scan, FieldglassError> {
    let mut cur = FileCursor::at(source, collection_addr)?;
    cur.tag(SIG_GLOBAL_HEAP)?;
    cur.skip(4)?; // version (1) + reserved (3)
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
    Ok(Scan {
        found: HashMap::new(),
        next: cur.position(),
        remaining: collection_size.saturating_sub(header_len as u64),
        objects: 0,
        ended: false,
    })
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
