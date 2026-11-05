//! A NetCDF-4 file whose `DIMENSION_LIST` references sit in many global-heap
//! collections opens as 0.6.0 opened it (#1698).
//!
//! 0.6.0 held each collection to 1,048,576 object headers. From #1549 the
//! reader held every collection one resolution reads to that many together,
//! so a valid file whose references named collections holding more objects
//! than that in all was refused with `ListingTooLarge`, where 0.6.0 opened
//! it on its named axes. libhdf5 writes such files: it grows a collection to
//! 64 KiB, 2,730 short strings, and puts a variable's references in the
//! collection with room, which string data written after them fills.
//!
//! `hdf5_dimension_lists.h5` holds nine variables on `y` and `x`, 18
//! references in one collection. Here each reference moves into a collection
//! of its own, appended to the file, with 65,534 other objects after it, all
//! with distinct indices: 1,179,630 objects in the collections named. The
//! file's object headers carry no checksum, so a reference is rewritten in
//! place.

use fieldglass::Session;
use fieldglass_netcdf::NetcdfReader;

const FIXTURE: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/hdf5_dimension_lists.h5");

/// The object budget the reader holds one collection to.
const OBJECT_BUDGET: usize = 1 << 20;

/// The most objects a collection holds with distinct indices: an index is a
/// `u16`, and 0 is the free-space object.
const DISTINCT: usize = 65_535;

/// The one collection libhdf5 wrote: its address, and its objects, each as
/// its index and data.
fn the_collection(file: &[u8]) -> (u64, Vec<(u16, Vec<u8>)>) {
    let at = file
        .windows(4)
        .position(|w| w == b"GCOL")
        .expect("a collection");
    let size = u64::from_le_bytes(file[at + 8..at + 16].try_into().unwrap()) as usize;
    let mut objects = Vec::new();
    let mut p = at + 16;
    while p + 16 <= at + size {
        let index = u16::from_le_bytes([file[p], file[p + 1]]);
        let len = u64::from_le_bytes(file[p + 8..p + 16].try_into().unwrap()) as usize;
        if index == 0 {
            break;
        }
        objects.push((index, file[p + 16..p + 16 + len].to_vec()));
        p += 16 + len.next_multiple_of(8);
    }
    (at as u64, objects)
}

/// A `DIMENSION_LIST` heap ID: count of references, collection address,
/// object index.
fn heap_id(count: u32, collection: u64, index: u32) -> Vec<u8> {
    let mut id = count.to_le_bytes().to_vec();
    id.extend_from_slice(&collection.to_le_bytes());
    id.extend_from_slice(&index.to_le_bytes());
    id
}

/// The one place `needle` occurs in `file`.
fn only(file: &[u8], needle: &[u8]) -> usize {
    let found: Vec<usize> = file
        .windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(at, _)| at)
        .collect();
    assert_eq!(found.len(), 1, "{needle:?} occurs {} times", found.len());
    found[0]
}

/// A collection of `objects`, each its index and data, and the free-space
/// object that ends the run.
fn collection<'a>(objects: impl IntoIterator<Item = (u16, &'a [u8])>) -> Vec<u8> {
    let mut c = b"GCOL".to_vec();
    c.extend_from_slice(&[1, 0, 0, 0]); // version + reserved
    c.extend_from_slice(&0u64.to_le_bytes()); // size, patched below
    for (index, data) in objects {
        c.extend_from_slice(&index.to_le_bytes());
        c.extend_from_slice(&[1, 0, 0, 0, 0, 0]); // reference count + reserved
        c.extend_from_slice(&(data.len() as u64).to_le_bytes());
        c.extend_from_slice(data);
        c.resize(c.len().next_multiple_of(8), 0);
    }
    c.extend_from_slice(&[0u8; 16]); // free-space object
    let total = c.len() as u64;
    c[8..16].copy_from_slice(&total.to_le_bytes());
    c
}

/// Eight zero bytes: the data of an object no reference names.
const FILLER: &[u8] = &[0; 8];

/// The fixture with the references at `which` (positions in its collection)
/// each moved, as object 1, into a collection of its own appended to the
/// file, which `make` builds from the reference's data. Returns the file and
/// how many objects the collections the references now name hold.
fn moved(which: &[usize], make: impl Fn(&[u8]) -> Vec<u8>) -> (Vec<u8>, usize) {
    let mut file = FIXTURE.to_vec();
    let (at, objects) = the_collection(&file);
    assert_eq!(objects.len(), 18, "nine variables on two axes");
    let mut named = if which.len() < objects.len() {
        objects.len()
    } else {
        0
    };
    for &k in which {
        let (index, data) = &objects[k];
        file.resize(file.len().next_multiple_of(8), 0);
        let new = file.len() as u64;
        // The references are in the fixture's own bytes, before any
        // collection appended here.
        let id = only(&file[..FIXTURE.len()], &heap_id(1, at, u32::from(*index)));
        file[id..id + 16].copy_from_slice(&heap_id(1, new, 1));
        let c = make(data);
        named += the_collection(&c).1.len();
        file.extend_from_slice(&c);
    }
    (file, named)
}

/// Each variable's dimensions, as `Session` lists them.
fn dims(session: &Session) -> Vec<(String, Vec<(String, u64)>)> {
    session
        .variables()
        .into_iter()
        .map(|v| {
            let dims = v.dims.iter().map(|d| (d.name.clone(), d.length)).collect();
            (v.name, dims)
        })
        .collect()
}

/// Every variable on `y` and `x`, as the fixture lists them.
fn on_named_axes() -> Vec<(String, Vec<(String, u64)>)> {
    (0..9)
        .map(|k| {
            let dims = vec![("y".to_string(), 2), ("x".to_string(), 3)];
            (format!("v{k}"), dims)
        })
        .collect()
}

/// Every reference in a collection of its own, followed by 65,534 other
/// objects, every index distinct: a valid file, holding more objects in
/// the collections it names than one collection is held to. It opens on
/// its named axes, as in 0.6.0, where from #1549 it was refused.
#[test]
fn dimension_lists_in_collections_past_the_old_shared_budget_open_on_named_axes() {
    let every: Vec<usize> = (0..18).collect();
    let (file, named) = moved(&every, |data| {
        let fillers = (2..=u16::MAX).map(|index| (index, FILLER));
        collection([(1, data)].into_iter().chain(fillers))
    });
    assert_eq!(named, 18 * DISTINCT);
    assert!(named > OBJECT_BUDGET, "{named} objects");

    let whole = Session::open(FIXTURE.to_vec()).expect("the fixture opens");
    assert_eq!(dims(&whole), on_named_axes());
    let session = Session::open(file.clone()).expect("the file opens, as in 0.6.0");
    assert_eq!(dims(&session), on_named_axes());
    for v in session.variables() {
        assert_eq!(v.attributes_not_read, None, "{}", v.name);
    }

    let reader = NetcdfReader::from_bytes(file).unwrap();
    let (meta, unread) = reader.hdf5_metadata_with_unread().expect("it resolves");
    assert!(unread.is_empty(), "{unread:?}");
    let on_scales = meta
        .variables
        .iter()
        .filter(|v| v.dimensions == ["y", "x"])
        .count();
    assert_eq!(on_scales, 9);
}

/// The fixture with its first reference moved behind `fillers` objects in a
/// collection of its own, their indices running 2 to 65,001 over and over,
/// as a valid file's never would.
fn first_reference_behind(fillers: usize) -> Vec<u8> {
    moved(&[0], |data| {
        let fillers = (0..fillers).map(|k| (2 + (k % 65_000) as u16, FILLER));
        collection(fillers.chain([(1, data)]))
    })
    .0
}

/// One collection is still held to the budget: a reference behind that
/// many objects fails the file as a listing cap, as 0.6.0 failed it, and one
/// behind one fewer is found. This bounds the scan of one collection; that
/// collections do not overlap bounds them together.
#[test]
fn one_collection_is_still_held_to_the_object_budget() {
    let err = match Session::open(first_reference_behind(OBJECT_BUDGET)) {
        Ok(_) => panic!("a reference past the budget opens"),
        Err(e) => e.to_string(),
    };
    let budget = format!("more than {OBJECT_BUDGET} HDF5 global-heap objects in one collection");
    assert!(err.contains(&budget), "{err}");

    let session = Session::open(first_reference_behind(OBJECT_BUDGET - 1)).expect("the file opens");
    assert_eq!(dims(&session), on_named_axes());
}
