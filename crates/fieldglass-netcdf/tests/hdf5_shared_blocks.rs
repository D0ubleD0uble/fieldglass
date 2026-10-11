//! A block that many objects or groups name is refused at the second name,
//! rather than read once per object, and a deep chain of nested groups lists
//! at the cost of its path rather than its square (#921).
//!
//! Fixtures from `tools/build_hdf5_shared_block_fixtures.py`; provenance in
//! `tests/fixtures/NOTICE.md`. Each test reads the committed bytes through the
//! real listing, counts the reads of the shared block with [`Recording`], and
//! bounds what the listing holds at once with a tracking allocator.
//!
//! Measured before the fix, release build, peak RSS of the whole process
//! (the PR records the table): the continuation fixture listed in 27 ms at
//! 67 MB, the attribute fixture resolved in 103 ms at 103 MB, the link fixture
//! listed in 36 ms at 36 MB, the local-heap fixture resolved in 15 ms at
//! 25 MB, and the chain listed in 28 ms at 62 MB. At four times the objects
//! (or depth) the same shapes took up to 671 ms and 590 MB.
//!
//! A valid file whose names pass the 64 MiB a listing holds is refused as
//! [`ArrayError::ListingTooLarge`], which says the file is fine.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass_core::array::ArrayError;
use fieldglass_core::testing::Recording;
use fieldglass_netcdf::{
    FieldglassError, Hdf5Probe, NetcdfBacking, NetcdfReader, list_all_children,
    resolve_hdf5_metadata,
};

/// 256 datasets whose headers all continue into one 256 KiB chunk.
const CONTINUATION: &[u8] = include_bytes!("fixtures/hdf5_shared_continuation.h5");
/// 256 datasets whose Attribute Info names `owner`'s dense storage: 64
/// attributes of 3,000 bytes.
const ATTRIBUTES: &[u8] = include_bytes!("fixtures/hdf5_shared_attribute_storage.h5");
/// 256 groups whose Link Info names `owner`'s dense storage: 64 links named
/// with 1,000 characters.
const LINKS: &[u8] = include_bytes!("fixtures/hdf5_shared_link_storage.h5");
/// 128 groups whose symbol tables name the root's local heap, each with one
/// member named by its 60,000-character name.
const LOCAL_HEAP: &[u8] = include_bytes!("fixtures/hdf5_shared_local_heap.h5");
/// 600 nested groups named with 200 characters, and one dataset at the bottom.
const CHAIN: &[u8] = include_bytes!("fixtures/hdf5_deep_group_chain.h5");
/// One dataset with twelve attributes in dense storage, linked as `v` and `w`.
const HARD_LINKED: &[u8] = include_bytes!("fixtures/hdf5_hard_linked_dense_attributes.h5");
/// 100 nested groups named with 360 characters, and one dataset with 2,001
/// names at the bottom: 72 MB of path-qualified names.
const PAST_NAME_CAP: &[u8] = include_bytes!("fixtures/hdf5_deep_group_chain_past_name_cap.h5");

struct Tracking;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn note(delta: isize) {
    // `try_with`: the allocator also runs while thread-locals are torn down.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = LIVE.try_with(|live| {
                live.set(live.get() + delta);
                let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
            });
        }
    });
}

// SAFETY: every method forwards to `System` unchanged; tracking touches only
// const-initialised thread-locals, which never allocate.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        // SAFETY: the caller's contract for `alloc`, passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size as isize - layout.size() as isize);
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(-(layout.size() as isize));
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// The most bytes live at once, counted from zero when `f` starts, while `f`
/// runs on this thread. What `f` returns is dropped before the count stops.
fn peak_while<R>(f: impl FnOnce() -> R) -> (isize, R) {
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    (PEAK.with(Cell::get), result)
}

const MB: isize = 1 << 20;

fn probe(bytes: &[u8]) -> Hdf5Probe {
    match NetcdfReader::from_bytes(bytes.to_vec()).unwrap().backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5, got {}", other.label()),
    }
}

/// Reads of at least `len` bytes: those that could have held the shared block.
fn reads_of(source: &Recording<&[u8]>, len: u64) -> usize {
    source.reads().iter().filter(|r| r.len >= len).count()
}

fn refused(result: Result<impl std::fmt::Debug, FieldglassError>, what: &str) {
    let err = result.expect_err("a block named by two objects is refused");
    assert!(err.to_string().contains(what), "{err}");
}

#[test]
fn headers_continuing_into_one_chunk_are_refused() {
    let p = probe(CONTINUATION);
    let source = Recording::new(CONTINUATION);
    let (peak, listed) = peak_while(|| list_all_children(&source, &p));
    refused(listed, "object-header chunk");
    // The first header reads the chunk; the second reads it before the
    // listing sees that it is the first one's. It was read 256 times.
    assert!(
        reads_of(&source, 256 << 10) <= 2,
        "{:?}",
        source.reads().len()
    );
    // Before: 64 MiB of copies held in the header memo.
    assert!(peak < 4 * MB, "peak {peak}");
}

#[test]
fn datasets_sharing_one_dense_attribute_store_are_refused() {
    let p = probe(ATTRIBUTES);
    let source = Recording::new(ATTRIBUTES);
    // Listing the objects reads no attributes.
    assert_eq!(list_all_children(&source, &p).unwrap().len(), 257);
    source.clear();
    let (peak, resolved) = peak_while(|| resolve_hdf5_metadata(&source, &p));
    refused(resolved, "fractal-heap header");
    // `owner`'s 64 attributes, each read once. Every dataset read all 64,
    // twice: 32,896 reads.
    assert!(reads_of(&source, 3000) <= 64, "{}", reads_of(&source, 3000));
    // Before: 103 MB, 49 MB of it the attribute values copied per dataset.
    assert!(peak < 4 * MB, "peak {peak}");
}

#[test]
fn groups_sharing_one_dense_link_store_are_refused() {
    let p = probe(LINKS);
    let source = Recording::new(LINKS);
    let (peak, listed) = peak_while(|| list_all_children(&source, &p));
    refused(listed, "fractal-heap header");
    // `owner`'s 64 link names, each read once, and its B-tree leaf. Every
    // group read all 64.
    let names: Vec<_> = source
        .reads()
        .into_iter()
        .filter(|r| r.len >= 1000)
        .map(|r| (r.start, r.len))
        .collect();
    let distinct: std::collections::HashSet<_> = names.iter().collect();
    assert_eq!(distinct.len(), names.len(), "no block is read twice");
    assert!(names.len() <= 65, "{}", names.len());
    // Before: 16,449 objects listed under 16.6 MB of names.
    assert!(peak < 4 * MB, "peak {peak}");
}

#[test]
fn groups_sharing_one_local_heap_are_refused() {
    let p = probe(LOCAL_HEAP);
    let source = Recording::new(LOCAL_HEAP);
    let (peak, listed) = peak_while(|| list_all_children(&source, &p));
    refused(listed, "local heap");
    // The root reads its long name; no group reads it again. Every group did.
    assert!(
        reads_of(&source, 60_000) <= 1,
        "{}",
        reads_of(&source, 60_000)
    );
    // Before: 129 objects under 7.7 MB of names.
    assert!(peak < 2 * MB, "peak {peak}");
}

#[test]
fn a_deep_chain_of_groups_lists_at_the_cost_of_its_path() {
    let p = probe(CHAIN);
    let source = Recording::new(CHAIN);
    let (peak, listed) = peak_while(|| list_all_children(&source, &p));
    let listed = listed.expect("a valid file lists");
    // h5py lists the same one dataset, 600 groups down.
    let path: String = (0..600)
        .map(|i| format!("/{i:05}{}", "g".repeat(195)))
        .collect();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, format!("{path}/v"));
    // The path is 120,600 bytes. Every level held its own copy of the path
    // above it, 36 MB in all, and the listing peaked at 62 MB of RSS.
    assert!(peak < 4 * MB, "peak {peak}");

    // It resolves and decodes too.
    let reader = NetcdfReader::from_bytes(CHAIN.to_vec()).unwrap();
    let meta = reader.hdf5_metadata().unwrap();
    assert_eq!(meta.variables.len(), 1);
    assert_eq!(meta.variables[0].name, format!("{path}/v"));
    let values = reader.decode_variable_raw(0).unwrap();
    assert_eq!(values, vec![Some(0.0), Some(1.0), Some(2.0), Some(3.0)]);
}

#[test]
fn one_object_reached_by_two_names_is_not_two_objects_sharing_a_store() {
    // h5py lists `v` and `w` with the same twelve attributes. The object is
    // described once and listed under both names, so its dense storage is
    // claimed once.
    let reader = NetcdfReader::from_bytes(HARD_LINKED.to_vec()).unwrap();
    let meta = reader
        .hdf5_metadata()
        .expect("a hard link is not a shared store");
    let names: Vec<&str> = meta.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["v", "w"]);
    let want: Vec<String> = (0..12).map(|i| format!("a{i:02}")).collect();
    for v in &meta.variables {
        let attrs: Vec<&str> = v.attributes.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(attrs, want, "{}", v.name);
        assert_eq!(v.attributes[11].values, [11.0], "{}", v.name);
    }
    assert_eq!(meta.variables[0].decode_index, 0);
    assert_eq!(meta.variables[1].decode_index, 1);
}

#[test]
fn names_past_what_a_listing_holds_are_too_large_not_corrupt() {
    // h5py visits all 101 objects; the file is valid, only larger than one
    // listing keeps, so the refusal says so rather than reporting a parse
    // error.
    let p = probe(PAST_NAME_CAP);
    let err = list_all_children(PAST_NAME_CAP, &p).expect_err("past the name cap");
    assert!(
        matches!(
            err,
            FieldglassError::Array(ArrayError::ListingTooLarge {
                limit: 67_108_864,
                ..
            })
        ),
        "{err:?}"
    );
    assert!(err.to_string().contains("The file itself is fine"), "{err}");
}
