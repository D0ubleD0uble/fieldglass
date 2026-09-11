//! Keyed objects read through the `ObjectSource` seam (#680, ADR-0010).
//!
//! The sibling of `fieldglass-netcdf`'s `classic_byte_source.rs`, and it checks
//! the same kind of claim about a different seam. There, a decoder must plan
//! every byte range before it reads one; here, a walker must **list, then
//! prefetch in one batch, then read**, because that is the shape that makes a
//! remote store one round trip instead of one per chunk.
//!
//! A test that only asserted the values came back would pass against a walker
//! that fetched every key separately, or that never prefetched at all — which
//! is precisely the implementation this seam exists to rule out. So what is
//! asserted here is the *order and the batching*, not just the answer.

use fieldglass_core::bytes::{MemoryObjects, ObjectSource};
use fieldglass_core::testing::{Minimal, Recording};
use std::borrow::Cow;

/// A two-level key space, shaped like a Zarr store with a nested group: two
/// arrays under a group, each with a metadata document and some chunks, and one
/// chunk deliberately absent.
fn store() -> Recording<MemoryObjects> {
    Recording::new(MemoryObjects::from_iter([
        ("group/.zgroup", b"{}".to_vec()),
        ("group/temp/.zarray", b"{\"temp\":true}".to_vec()),
        ("group/temp/0.0", b"t00".to_vec()),
        ("group/temp/0.1", b"t01".to_vec()),
        // `group/temp/1.0` is deliberately absent: a sparse array's missing
        // chunk, which is the fill value and not a fault.
        ("group/temp/1.1", b"t11".to_vec()),
        ("group/wind/.zarray", b"{\"wind\":true}".to_vec()),
        ("group/wind/0.0", b"w00".to_vec()),
    ]))
}

/// Everything a walker does, in the order a remote store needs it done.
#[test]
fn a_walk_lists_then_prefetches_once_then_reads() {
    let store = store();

    // 1. Discover. A prefix walk, not a guess at what is there.
    let arrays: Vec<String> = store
        .list("group/")
        .unwrap()
        .into_iter()
        .filter(|key| key.ends_with("/.zarray"))
        .collect();
    assert_eq!(arrays, ["group/temp/.zarray", "group/wind/.zarray"]);

    // 2. One batch, before any read.
    let chunks = ["group/temp/0.0", "group/temp/0.1", "group/temp/1.0"];
    store.prefetch(&chunks).unwrap();

    // 3. Read. The absent chunk answers `None` rather than failing the walk.
    let values: Vec<Option<Vec<u8>>> = chunks
        .iter()
        .map(|key| store.get(key).unwrap().map(Cow::into_owned))
        .collect();
    assert_eq!(
        values,
        [
            Some(b"t00".to_vec()),
            Some(b"t01".to_vec()),
            None, // the sparse chunk
        ]
    );

    // The property that matters, and the one an implementation loses first:
    // **one** prefetch batch, and it came before every read.
    assert_eq!(
        store.key_prefetches().len(),
        1,
        "the walk must batch its fetches"
    );
    assert_eq!(store.key_prefetches()[0], chunks);
    assert_eq!(store.gets(), chunks, "and read exactly what it prefetched");
}

/// Listing is by prefix and is sorted, so two hosts walking the same store hand
/// their reader the same sequence.
#[test]
fn listing_is_by_prefix_and_ordered() {
    let store = store();

    assert_eq!(
        store.list("group/temp/").unwrap(),
        [
            "group/temp/.zarray",
            "group/temp/0.0",
            "group/temp/0.1",
            "group/temp/1.1"
        ]
    );
    // A prefix is a string prefix, not a path component, which is what lets one
    // call cover `group/temp/0.` if a caller wants a row of chunks.
    assert_eq!(
        store.list("group/temp/0.").unwrap(),
        ["group/temp/0.0", "group/temp/0.1"]
    );
    // Everything.
    assert_eq!(store.list("").unwrap().len(), 7);
    // An empty group is a group, not an error.
    assert!(store.list("group/nothing/").unwrap().is_empty());
}

/// `require` is for the keys whose absence is a fault, and it names the key so
/// a store with thousands of them says which one was missing.
#[test]
fn require_is_the_other_half_of_get() {
    let store = store();

    assert_eq!(
        store.require("group/temp/.zarray").unwrap().as_ref(),
        b"{\"temp\":true}"
    );

    let err = store.require("group/temp/1.0").unwrap_err();
    assert!(
        err.to_string().contains("group/temp/1.0"),
        "the error must name the key: {err}"
    );
    // And the same key through `get` is simply absent.
    assert!(store.get("group/temp/1.0").unwrap().is_none());
}

/// Object-safe, which is what lets a walker take `&dyn ObjectSource` and be
/// written once for a directory, a bucket and a map. A trait that was not would
/// force the walker to be generic and every caller to name the type.
#[test]
fn the_trait_is_object_safe() {
    let store = store();
    let erased: &dyn ObjectSource = &store;

    erased.prefetch(&["group/temp/0.0"]).unwrap();
    assert_eq!(
        erased.get("group/temp/0.0").unwrap().as_deref(),
        Some(&b"t00"[..])
    );
    assert_eq!(erased.list("group/wind/").unwrap().len(), 2);
    // The provided method reaches through the vtable too.
    assert!(erased.require("group/wind/.zarray").is_ok());
}

/// The default `prefetch` is a no-op, so an implementation that has nothing to
/// resolve writes two methods and not four — and a reader that prefetches costs
/// it nothing. Every provided method is measured against `Minimal`, which
/// implements `get` and `list` and nothing else.
#[test]
fn the_provided_methods_reach_an_implementation_that_writes_only_two() {
    let bare = Minimal::from_iter([("a", b"x".to_vec()), ("d/e", b"y".to_vec())]);

    bare.prefetch(&["a", "b", "c"]).unwrap();
    assert_eq!(bare.get("a").unwrap().as_deref(), Some(&b"x"[..]));
    assert!(bare.get("b").unwrap().is_none());
    assert!(bare.require("b").is_err());
    assert_eq!(bare.list_children("").unwrap(), ["a", "d/"]);
}

/// The store holds the map and nothing else, however much is asked of it.
///
/// It used to record every `get` into a `RefCell<Vec<String>>` that only an
/// explicit call emptied, so a host using it as its *real* store for an opened
/// directory (#659) grew a string per read for the life of the session. The
/// `Sync` bound is the proof rather than the byte count: a `RefCell` is not
/// `Sync`, so this line fails to compile if any interior mutability comes back.
#[test]
fn the_store_does_not_grow_as_it_is_read() {
    fn assert_sync<T: Sync>() {}
    assert_sync::<MemoryObjects>();

    let store = MemoryObjects::from_iter([("a", b"x".to_vec()), ("b", b"y".to_vec())]);
    for _ in 0..10_000 {
        assert_eq!(store.get("a").unwrap().as_deref(), Some(&b"x"[..]));
        // An absent key too: that was the read the log grew on hardest, since a
        // sparse array asks for chunks it does not hold.
        assert!(store.get("missing").unwrap().is_none());
    }
    assert_eq!(store.len(), 2);
}

/// `list_children` names objects by their key and directories by their prefix,
/// so a walker can descend without enumerating what is below.
///
/// This is the property that decides whether walking a bucket costs a listing
/// per group or a paged walk of every chunk in the store.
#[test]
fn listing_children_names_directories_rather_than_descending_into_them() {
    let store = store();

    assert_eq!(store.list_children("").unwrap(), ["group/"]);
    assert_eq!(
        store.list_children("group/").unwrap(),
        ["group/.zgroup", "group/temp/", "group/wind/"]
    );
    // The leaf: its own document and its chunks, all directly under it.
    assert_eq!(
        store.list_children("group/temp/").unwrap(),
        [
            "group/temp/.zarray",
            "group/temp/0.0",
            "group/temp/0.1",
            "group/temp/1.1"
        ]
    );
    // A prefix that names nothing is an empty list, as `list` is.
    assert!(store.list_children("group/nothing/").unwrap().is_empty());

    // The recorder sees both listings alike, which is what lets a test say how
    // much of a store a walk enumerated.
    assert_eq!(
        store
            .listings()
            .iter()
            .map(|(p, _)| p.as_str())
            .collect::<Vec<_>>(),
        ["", "group/", "group/temp/", "group/nothing/"]
    );
}

/// A key space deeper than one level: a directory is named once however many
/// keys sit under it, and nothing under it is named at all.
#[test]
fn a_directory_is_named_once_however_deep_it_goes() {
    let deep = MemoryObjects::from_iter([
        (".zgroup", b"{}".to_vec()),
        ("temp/.zarray", b"{}".to_vec()),
        ("temp/0/0", b"c".to_vec()),
        ("temp/0/1", b"c".to_vec()),
        ("temp/1/0", b"c".to_vec()),
        ("temp/1/1", b"c".to_vec()),
    ]);

    assert_eq!(deep.list_children("").unwrap(), [".zgroup", "temp/"]);
    assert_eq!(
        deep.list_children("temp/").unwrap(),
        ["temp/.zarray", "temp/0/", "temp/1/"],
        "four chunks behind two directory names"
    );
    // `list` is the other answer, and the reason `list_children` exists.
    assert_eq!(deep.list("temp/").unwrap().len(), 5);
}
