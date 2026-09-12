//! A type-erased `ByteSource` is a `ByteSource` (#709).
//!
//! `Box<dyn ByteSource>` is what lets one reader instantiation serve every host.
//! Without it `Session` would need a type parameter on its whole API, or a second
//! monomorphisation of every reader per source type — bundle weight the browser
//! build pays for. So the two properties this file checks are that the trait is
//! object-safe at all, and that boxing is *transparent*: the same answers, the
//! same identity, and the prefetch still reaching the source underneath.

use fieldglass_core::bytes::{ByteRange, ByteSource};
use fieldglass_core::testing::Recording;

const BYTES: &[u8] = b"0123456789abcdef";

/// Object-safe, which is the whole premise. A trait that was not would fail to
/// compile here rather than somewhere deep in the umbrella.
#[test]
fn the_trait_is_object_safe_and_boxing_forwards_every_method() {
    let buffer = BYTES.to_vec();
    // The identity *of this buffer*, taken before it is boxed. Not of an equal
    // one: `SourceIdentity::Buffer` carries the heap address as well as the
    // length and the sampled ends, so two `Vec`s of identical content are
    // deliberately *not* equal — that is what stops one file's memo serving
    // another's (#681). Moving the `Vec` into the box does not move its heap
    // allocation, so this is the same source either way.
    let want = buffer.identity();
    let boxed: Box<dyn ByteSource> = Box::new(buffer);

    assert_eq!(boxed.size(), 16);
    assert!(!boxed.is_empty());
    assert_eq!(
        &*boxed.read(ByteRange::new(4, 4)).expect("in range"),
        b"4567"
    );
    // Identity forwards, or a reader that memoises what it found in a file would
    // treat the boxed source as a different file from the one it wraps.
    assert_eq!(boxed.identity(), want);
    boxed.prefetch(&[ByteRange::new(0, 4)]).expect("advisory");
    // Out of bounds is still out of bounds through the box.
    assert!(boxed.read(ByteRange::new(12, 8)).is_err());
}

/// A box around a recorder records, which is the composition a host actually
/// builds: `Box<dyn ByteSource>` over whatever it fetched with.
#[test]
fn a_boxed_source_still_reaches_the_one_underneath() {
    let recording = Recording::new(BYTES.to_vec());
    // Borrowed rather than moved, so the log is still readable afterwards — the
    // `&S` forwarding impl and this one composing is the point.
    let boxed: Box<dyn ByteSource + '_> = Box::new(&recording);

    boxed.prefetch(&[ByteRange::new(0, 8)]).expect("advisory");
    assert_eq!(
        &*boxed.read(ByteRange::new(0, 8)).expect("in range"),
        b"01234567"
    );

    assert_eq!(recording.prefetches(), [[ByteRange::new(0, 8)]]);
    assert_eq!(recording.reads(), [ByteRange::new(0, 8)]);
}

/// Boxing twice is still a source, so a host composing wrappers cannot hit a
/// depth the trait stops working at.
#[test]
fn boxing_nests() {
    let inner: Box<dyn ByteSource> = Box::new(BYTES.to_vec());
    let outer: Box<Box<dyn ByteSource>> = Box::new(inner);
    assert_eq!(outer.size(), 16);
    assert_eq!(&*outer.read(ByteRange::new(1, 2)).expect("in range"), b"12");
}
