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

use super::heap::Cursor;
use fieldglass_core::FieldglassError;

const SIG_GLOBAL_HEAP: &[u8; 4] = b"GCOL";

/// Upper bound on objects scanned in one collection — guards a malformed size.
const MAX_GLOBAL_HEAP_OBJECTS: usize = 1 << 20;

/// Width of a global-heap object's fixed header that precedes its data:
/// `index(2) + refcount(2) + reserved(4)`, then a length-sized object size.
const OBJECT_HEADER_FIXED: usize = 8;

/// Read the bytes of the object with `object_index` from the global-heap
/// collection at `collection_addr`. `length_size` is the superblock's size of
/// lengths (the width of the collection-size and object-size fields).
pub fn read_object(
    bytes: &[u8],
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
    let start = usize::try_from(collection_addr)
        .map_err(|_| FieldglassError::Parse("global-heap address too large".into()))?;
    let mut cur = Cursor::at(bytes, collection_addr)?;
    cur.tag(SIG_GLOBAL_HEAP)?;
    cur.skip(4)?; // version (1) + reserved (3)
    // The declared collection size counts the whole collection (header + objects).
    // Clamp it to the bytes actually on disk so a corrupt size can't make the
    // budget arithmetic below overflow or run past the file.
    let available = bytes.len() - start;
    let collection_size = (cur.uint(l)? as usize).min(available);
    // The collection size counts the 8-byte signature/version/reserved block plus
    // the length field, so the object run is whatever remains after the header.
    let header_len = OBJECT_HEADER_FIXED + l;
    let mut remaining = collection_size.saturating_sub(header_len);

    for _ in 0..MAX_GLOBAL_HEAP_OBJECTS {
        if remaining < header_len {
            break;
        }
        let index = cur.u16()?;
        cur.skip(2 + 4)?; // reference count + reserved
        let size = cur.uint(l)? as usize;
        // Index 0 is the free-space object that terminates the run.
        if index == 0 {
            break;
        }
        // The object's data must fit in what the collection has left; this both
        // rejects a corrupt oversized object (a cross-object misread) and keeps
        // the padding below from overflowing.
        let body_budget = remaining - header_len;
        if size > body_budget {
            return Err(FieldglassError::Parse(
                "global-heap object size exceeds the collection bounds".into(),
            ));
        }
        let padded = size.next_multiple_of(8);
        if index == object_index {
            return Ok(cur.take(size)?.to_vec());
        }
        cur.skip(padded)?;
        remaining = remaining.saturating_sub(header_len + padded);
    }

    Err(FieldglassError::Parse(format!(
        "global-heap object {object_index} not found in collection"
    )))
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
}
