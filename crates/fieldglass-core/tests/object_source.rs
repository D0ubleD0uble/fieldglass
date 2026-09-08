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

use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{MemoryObjects, ObjectSource};
use std::borrow::Cow;

/// A two-level key space, shaped like a Zarr store with a nested group: two
/// arrays under a group, each with a metadata document and some chunks, and one
/// chunk deliberately absent.
fn store() -> MemoryObjects {
    MemoryObjects::from_iter([
        ("group/.zgroup", b"{}".to_vec()),
        ("group/temp/.zarray", b"{\"temp\":true}".to_vec()),
        ("group/temp/0.0", b"t00".to_vec()),
        ("group/temp/0.1", b"t01".to_vec()),
        // `group/temp/1.0` is deliberately absent: a sparse array's missing
        // chunk, which is the fill value and not a fault.
        ("group/temp/1.1", b"t11".to_vec()),
        ("group/wind/.zarray", b"{\"wind\":true}".to_vec()),
        ("group/wind/0.0", b"w00".to_vec()),
    ])
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
        store.prefetches().len(),
        1,
        "the walk must batch its fetches"
    );
    assert_eq!(store.prefetches()[0], chunks);
    assert_eq!(store.reads(), chunks, "and read exactly what it prefetched");
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
/// resolve writes two methods and not three — and a reader that prefetches
/// costs it nothing.
#[test]
fn prefetch_defaults_to_doing_nothing() {
    struct Bare;
    impl ObjectSource for Bare {
        fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
            Ok((key == "a").then(|| Cow::Borrowed(&b"x"[..])))
        }
        fn list(&self, _prefix: &str) -> Result<Vec<String>, FieldglassError> {
            Ok(vec!["a".to_string()])
        }
    }

    let bare = Bare;
    bare.prefetch(&["a", "b", "c"]).unwrap();
    assert_eq!(bare.get("a").unwrap().as_deref(), Some(&b"x"[..]));
    assert!(bare.get("b").unwrap().is_none());
    assert!(bare.require("b").is_err());
}
