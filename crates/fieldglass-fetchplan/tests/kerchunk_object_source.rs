//! A reference document read as a store, through `ZarrStore` (#705, ADR-0010).
//!
//! `kerchunk_seam.rs` proves the halves meet: a range this crate plans holds a
//! chunk the decoder can turn into numbers. This proves the claim ADR-0010
//! actually makes — that a kerchunk document **is** an `ObjectSource`, so the
//! store walker reads an archive described by one **with no reader change**.
//! Nothing in `fieldglass-zarr` knows what a reference document is.
//!
//! The object is the same `temp.bin` the seam test uses: the store's four chunks
//! laid end to end with seven bytes of `0xa5` between them, because a real
//! reference document points into a file that was never a Zarr store. The gaps
//! are what make the stated offsets the only way to be right, and `0xa5` filler
//! means an off-by-one range fails to decode rather than yielding plausible
//! numbers.
//!
//! The oracle is the array the fixtures were written from, stated here rather
//! than read back through a second Zarr store: two stores sharing this crate's
//! decoder would agree with each other while both being wrong.

use fieldglass_core::bytes::{ByteRange, ObjectSource};
use fieldglass_core::testing::{OneRange, Recording};
use fieldglass_fetchplan::{KerchunkObjects, KerchunkRefs};
use fieldglass_zarr::{ArraySource, ZarrStore};

const URL: &str = "s3://example-bucket/temp.bin";

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("tests/fixtures/zarr/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn refs() -> KerchunkRefs {
    let text = String::from_utf8(fixture("kerchunk_refs.json")).expect("the document is UTF-8");
    KerchunkRefs::parse(&text).expect("the document parses")
}

/// The array the fixtures were written from: `arange(24) * 0.5`, as 4x6.
///
/// A ramp of halves is exact in float32 and every value is distinct, so a chunk
/// read from the wrong offset is a wrong number and not a plausible one.
fn source_value(row: u64, column: u64) -> f64 {
    (row * 6 + column) as f64 * 0.5
}

/// The whole array, read through `ZarrStore` over a reference document.
#[test]
fn a_reference_document_reads_as_a_store() {
    let mut objects = KerchunkObjects::new(refs());
    // The document says which objects it needs before anything is fetched, which
    // is the property that lets a host fetch ahead.
    assert_eq!(objects.urls(), vec![URL.to_string()]);
    objects.insert(URL, fixture("temp.bin"));

    let store = ZarrStore::open(objects).expect("a reference document is a store");
    let source: &dyn ArraySource = &store;
    assert_eq!(
        source
            .group()
            .arrays_qualified()
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>(),
        vec!["temp".to_string()],
        "the walker found the array through the document's inline metadata"
    );
    // The dimension names came from the inlined `.zattrs`, so the CF layer read
    // the document too and not only the chunks.
    let array = source.array("temp").expect("listed");
    assert_eq!(array.dimensions, vec!["y".to_string(), "x".to_string()]);
    assert!(store.left_out().is_empty(), "{:?}", store.left_out());

    let values = source
        .read_region("temp", &[0..4, 0..6])
        .expect("every chunk is addressed");
    let expected: Vec<Option<f64>> = (0..4)
        .flat_map(|row| (0..6).map(move |column| Some(source_value(row, column))))
        .collect();
    assert_eq!(values, expected, "the whole array, across all four chunks");
}

/// A region inside one chunk reads that chunk and no other.
///
/// The point of a reference document over a transport: a twenty-four-byte slice
/// of a multi-gigabyte archive costs the chunks it covers and nothing else.
#[test]
fn a_region_reads_only_the_chunks_it_covers() {
    let mut objects = KerchunkObjects::new(refs());
    let recording = Recording::new(fixture("temp.bin"));
    objects.insert(URL, &recording);

    let store = ZarrStore::open(&objects).expect("opens");
    recording.clear();

    // Chunks are 2x3, so rows 0..2 and columns 0..3 is chunk (0, 0) alone.
    let values = (&store as &dyn ArraySource)
        .read_region("temp", &[0..2, 0..3])
        .expect("one chunk");
    assert_eq!(
        values,
        (0..2)
            .flat_map(|row| (0..3).map(move |column| Some(source_value(row, column))))
            .collect::<Vec<_>>()
    );

    // One range, and it is the one the document states for that chunk: offset 0,
    // 33 bytes. Not the whole object, and not the three chunks beside it.
    assert_eq!(recording.reads(), [ByteRange::new(0, 33)]);
    assert_eq!(recording.prefetches(), [[ByteRange::new(0, 33)]]);
}

/// A chunk whose object the host did not bring is an error naming the range,
/// never a silent empty object.
///
/// The acceptance criterion, and the reason it matters: `ObjectSource::get`
/// answers `None` for an absent key because a sparse array's missing chunk is
/// its fill value. A *present* key the host failed to fetch is the opposite
/// case, and answering `None` there would put fill values on a screen and call
/// them data.
#[test]
fn an_unfetched_object_is_an_error_not_an_absent_chunk() {
    // No object is ever inserted, so the parameter is named rather than inferred.
    let objects: KerchunkObjects<Vec<u8>> = KerchunkObjects::new(refs());

    // The metadata is inline, so the store still opens with no object at all.
    let store = ZarrStore::open(&objects).expect("the structure is in the document");
    let err = (&store as &dyn ArraySource)
        .read_region("temp", &[0..2, 0..3])
        .expect_err("no object was given");
    let message = err.to_string();
    assert!(
        message.contains(URL),
        "the error must name the object: {message}"
    );
    assert!(
        message.contains("bytes=0-32") || message.contains("insert"),
        "and the range, or how to supply it: {message}"
    );

    // And directly through the seam, so the claim does not depend on the walker.
    assert!(objects.get("temp/0.0").is_err());
    // An absent key is still absent, not an error: the two cases stay distinct.
    assert!(
        objects
            .get("temp/9.9")
            .expect("absent, not broken")
            .is_none()
    );
    // Inline keys answer with no object.
    assert!(objects.get("temp/.zarray").expect("inline").is_some());
}

/// A host that fetched only one chunk's range reads that chunk and refuses the
/// others, rather than reading whatever happens to be at the offset.
#[test]
fn a_host_holding_one_range_reads_that_chunk_and_refuses_the_rest() {
    let mut objects = KerchunkObjects::new(refs());
    // `OneRange` reports the whole object's size and serves only what was
    // fetched, which is exactly what a host holding one HTTP range has.
    let held = ByteRange::new(0, 33);
    objects.insert(URL, OneRange::new(fixture("temp.bin"), held));

    let store = ZarrStore::open(&objects).expect("opens");
    let source: &dyn ArraySource = &store;

    assert_eq!(
        source.read_region("temp", &[0..2, 0..3]).expect("held"),
        (0..2)
            .flat_map(|row| (0..3).map(move |column| Some(source_value(row, column))))
            .collect::<Vec<_>>()
    );
    // Chunk (0, 1) is at offset 40, which was never fetched.
    let err = source
        .read_region("temp", &[0..2, 3..6])
        .expect_err("that range was never fetched");
    assert!(
        err.to_string().contains("never fetched"),
        "unexpected error: {err}"
    );
}

/// `prefetch` reports the ranges the document already plans, grouped per object.
///
/// This is what makes a reference document worth having over a transport: every
/// byte a read wants is known before any of it is asked for, so a remote source
/// issues one batch instead of one request per chunk.
#[test]
fn prefetch_states_the_ranges_the_document_plans() {
    let mut objects = KerchunkObjects::new(refs());
    let recording = Recording::new(fixture("temp.bin"));
    objects.insert(URL, &recording);

    objects
        .prefetch(&["temp/0.0", "temp/1.1", "temp/.zarray", "temp/9.9"])
        .expect("advisory");

    // One batch, holding the two ranged keys' ranges and nothing for the inline
    // document or the absent chunk.
    assert_eq!(
        recording.prefetches(),
        [[ByteRange::new(0, 33), ByteRange::new(120, 33)]],
        "one batch, the planned ranges, in document order"
    );
    assert!(
        recording.reads().is_empty(),
        "prefetch resolves; it does not read"
    );
}
