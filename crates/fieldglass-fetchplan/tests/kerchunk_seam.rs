//! The seam: a range this crate planned holds a chunk `fieldglass-zarr` can
//! decode.
//!
//! Each crate is tested on its own elsewhere — the codecs against stores
//! `zarr-python` wrote, the addressing against hand-written documents — and
//! neither of those says anything about the one place they meet. This does:
//! a chunk index goes in, a byte range comes out, the bytes at that range go to
//! the decoder, and the values that come back are the ones the array was
//! written from.
//!
//! **The object is a concatenation with gaps in it.** `temp.bin` holds the
//! `zstd` store's four chunks laid end to end with seven bytes between them,
//! because a real reference document points into a file that was never a Zarr
//! store — a NetCDF4 or GRIB archive, whose chunks are separated by headers
//! this crate never sees. Without the gaps a planner that multiplied the chunk
//! index by a length would pass; with them the stated offset is the only way to
//! be right. The filler is `0xa5` rather than zero, so an off-by-one range
//! fails to decode instead of yielding plausible numbers.
//!
//! The chunks are **zstd-compressed** for the same reason. Raw chunks are
//! little-endian float32, and a seam test over those would pass against a
//! reader that never called the codec crate at all.
//!
//! Since #686 this crate takes `fieldglass-zarr` for the metadata parser with
//! `default-features = false`, so the codecs the decode half needs are **not**
//! in the library's dependency tree — only in this test's, through
//! `dev-dependencies`. That is the split working: the planner reads a document
//! without linking a decompressor, and the seam test links one on purpose.
//!
//! See [`fixtures/NOTICE.md`](fixtures/NOTICE.md) for how the fixtures are
//! generated.

use fieldglass_fetchplan::{ArrayMetadata, FetchPlanError, KerchunkRefs, PlanRange};
use fieldglass_zarr::ChunkDecoder;

/// Read a committed fixture, by a path `wasmtime --dir=.` can open. See
/// `real_sidecars.rs`, which explains why this is not `CARGO_MANIFEST_DIR`.
fn fixture(name: &str) -> String {
    let path = format!("tests/fixtures/zarr/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = format!("tests/fixtures/zarr/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// The array the fixtures were written from: `arange(24) * 0.5`, as 4x6.
///
/// Stated here rather than read back out of the store, so the oracle and the
/// decoder do not share an implementation. A ramp of halves is exact in
/// float32 and every value is distinct, so a chunk fetched from the wrong
/// offset is a wrong number and not a plausible one.
fn source_value(row: u64, column: u64) -> f64 {
    (row * 6 + column) as f64 * 0.5
}

/// Stand in for the host: resolve a reference document's URL to bytes.
///
/// The document names `s3://example-bucket/temp.bin`, which is opaque to the
/// planner — it hands the string back for a host to fetch. This test's "host"
/// maps it to the object committed beside it.
fn fetch(url: &str, range: &PlanRange) -> Vec<u8> {
    assert_eq!(
        url, "s3://example-bucket/temp.bin",
        "the fixtures address one object"
    );
    let object = fixture_bytes("temp.bin");
    let closed = range
        .close(object.len() as u64)
        .expect("a planned range closes against the object it addresses");
    let start = usize::try_from(closed.start).expect("a fixture offset fits a pointer");
    let length = usize::try_from(closed.len).expect("a fixture length fits a pointer");
    object[start..start + length].to_vec()
}

/// One chunk, end to end: index in, values out.
fn decode_chunk(refs: &KerchunkRefs, meta: &ArrayMetadata, index: &[u64]) -> Vec<f64> {
    let item = refs
        .chunk_at("temp", meta, index)
        .expect("the index is on the grid")
        .unwrap_or_else(|| panic!("the document addresses chunk {index:?}"));

    let bytes = fetch(&item.key, &item.range);
    let metadata = refs.inline("temp/.zarray").expect("the array's metadata");
    let decoder = ChunkDecoder::from_v2_metadata(std::str::from_utf8(metadata).unwrap())
        .expect("the inlined .zarray is the one the store was written with");
    decoder
        .decode_raw_values(&bytes)
        .unwrap_or_else(|e| panic!("decoding chunk {index:?}: {e}"))
}

/// The acceptance criterion for #660, stated as one test: every chunk the
/// document addresses decodes, through the planned range, to the values the
/// array was written from.
#[test]
fn every_planned_range_decodes_to_the_values_the_store_holds() {
    let refs = KerchunkRefs::parse(&fixture("kerchunk_refs.json")).expect("the document parses");
    let meta = refs.array("temp").expect("the array's metadata is inline");

    assert_eq!(meta.grid().shape(), &[4, 6]);
    assert_eq!(meta.grid().chunk_shape(), &[2, 3]);
    assert_eq!(meta.grid().grid_shape(), vec![2, 2]);
    assert_eq!(refs.arrays(), vec!["temp".to_string()]);

    for chunk_row in 0..2 {
        for chunk_column in 0..2 {
            let values = decode_chunk(&refs, &meta, &[chunk_row, chunk_column]);
            // A chunk is its own 2x3 block of the array, in C order.
            let expected: Vec<f64> = (0..2)
                .flat_map(|r| {
                    (0..3).map(move |c| source_value(chunk_row * 2 + r, chunk_column * 3 + c))
                })
                .collect();
            assert_eq!(
                values, expected,
                "chunk {chunk_row}.{chunk_column} decoded to the wrong values"
            );
        }
    }
}

/// The ranges are not a uniform stride, so a planner that derived an offset
/// instead of reading the stated one would be wrong. Pinned rather than
/// implied, because it is the property that makes the test above meaningful.
#[test]
fn the_planned_offsets_are_the_documents_own_and_not_a_stride() {
    let refs = KerchunkRefs::parse(&fixture("kerchunk_refs.json")).expect("the document parses");
    let meta = refs.array("temp").expect("the array's metadata is inline");

    let offsets: Vec<u64> = [[0, 0], [0, 1], [1, 0], [1, 1]]
        .iter()
        .map(|index| {
            refs.chunk_at("temp", &meta, index)
                .unwrap()
                .unwrap()
                .range
                .offset()
        })
        .collect();

    assert_eq!(offsets, vec![0, 40, 80, 120]);
    // Every chunk is 33 bytes, so the 40-byte stride is a gap and not a length:
    // `index * length` would give 0, 33, 66, 99 and decode three chunks wrong.
    for index in [[0, 0], [0, 1], [1, 0], [1, 1]] {
        let item = refs.chunk_at("temp", &meta, &index).unwrap().unwrap();
        assert_eq!(item.range.length(), Some(33));
    }
}

/// A region asks for exactly the chunks it touches, and they decode to the
/// slice of the array they cover.
#[test]
fn a_region_plans_the_chunks_it_touches_and_they_hold_that_region() {
    let refs = KerchunkRefs::parse(&fixture("kerchunk_refs.json")).expect("the document parses");
    let meta = refs.array("temp").expect("the array's metadata is inline");

    // The left half of the array: all four rows, the first three columns. Two
    // chunks, not four.
    let plan = refs
        .chunks_covering("temp", &meta, &[0..4, 0..3])
        .expect("the region is inside the array");
    assert_eq!(plan.len(), 2);
    assert_eq!(plan[0].range.offset(), 0);
    assert_eq!(plan[1].range.offset(), 80);

    assert_eq!(
        decode_chunk(&refs, &meta, &[0, 0]),
        vec![0.0, 0.5, 1.0, 3.0, 3.5, 4.0]
    );
    assert_eq!(
        decode_chunk(&refs, &meta, &[1, 0]),
        vec![6.0, 6.5, 7.0, 9.0, 9.5, 10.0]
    );
}

/// The templated document addresses the same object by the same ranges, so the
/// two must plan identically — that is what makes `{{u}}` a spelling rather
/// than a second dialect.
#[test]
fn the_templated_document_plans_what_the_plain_one_does() {
    let plain = KerchunkRefs::parse(&fixture("kerchunk_refs.json")).expect("the plain document");
    let templated =
        KerchunkRefs::parse(&fixture("kerchunk_templates.json")).expect("the templated document");

    let meta = plain.array("temp").expect("the array's metadata");
    for index in [[0, 0], [0, 1], [1, 0], [1, 1]] {
        let a = plain.chunk_at("temp", &meta, &index).unwrap().unwrap();
        let b = templated.chunk_at("temp", &meta, &index).unwrap().unwrap();
        assert_eq!(a.key, b.key, "chunk {index:?} resolves to a different URL");
        assert_eq!(a.range, b.range, "chunk {index:?} plans a different range");
    }

    // And the substituted URL is a URL, not a template a host has to finish.
    let item = templated.chunk_at("temp", &meta, &[0, 0]).unwrap().unwrap();
    assert_eq!(item.key, "s3://example-bucket/temp.bin");
}

/// A document naming a feature the crate does not read is refused outright,
/// rather than planning the `refs` it can see and quietly omitting the rest.
#[test]
fn a_gen_document_is_refused_rather_than_half_read() {
    let err =
        KerchunkRefs::parse(&fixture("kerchunk_gen.json")).expect_err("a `gen` block is not read");
    assert!(
        matches!(err, FetchPlanError::UnsupportedFeature { .. }),
        "{err:?}"
    );
    // The message names what was unsupported, which is what a host reports.
    assert!(err.to_string().contains("gen"), "{err}");
}

/// The two metadata dialects describe the same grid, which is the invariant
/// that lets one addressing path serve both editions.
#[test]
fn the_committed_metadata_documents_describe_the_same_grid() {
    let v2 = ArrayMetadata::parse(&fixture("v2_zarray.json")).expect("the v2 document");
    let v3 = ArrayMetadata::parse(&fixture("v3_zarr.json")).expect("the v3 document");

    assert_eq!(v2.zarr_format(), 2);
    assert_eq!(v3.zarr_format(), 3);
    assert_eq!(v2.grid().shape(), v3.grid().shape());
    assert_eq!(v2.grid().chunk_shape(), v3.grid().chunk_shape());
    assert_eq!(v2.grid().grid_shape(), v3.grid().grid_shape());

    // They spell the same chunk differently, and both spellings are what the
    // committed stores actually use as file names.
    assert_eq!(v2.chunk_key(&[1, 1]).unwrap(), "1.1");
    assert_eq!(v3.chunk_key(&[1, 1]).unwrap(), "c/1/1");
}
