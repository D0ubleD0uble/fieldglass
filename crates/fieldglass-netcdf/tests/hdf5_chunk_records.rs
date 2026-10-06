//! A chunk index is held to the chunk grid before any chunk is read (#837,
//! ADR-0013).
//!
//! The fixtures come from `tools/build_hdf5_duplicate_chunk_fixture.py`;
//! provenance in `tests/fixtures/NOTICE.md`. Each holds a `(1,)` or `(2,)`
//! `uint8` dataset `v` under a version-1 chunk B-tree.
//!
//! Decoding goes through a recording source, so "read once" is counted, not
//! timed: the duplicated index used to inflate its one 16 MB chunk per record,
//! about 10.6 s on the fuzz build for these 16 records.

use fieldglass_core::ByteSource;
use fieldglass_core::testing::Recording;
use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};

/// One element in one gzip chunk of 16 Mi elements: legal for an extendable
/// dataset, and placed without walking the rest of the chunk.
const OVERSIZED: &[u8] = include_bytes!("fixtures/hdf5_oversized_chunk.h5");
/// The same file with an index naming that chunk 16 times (4 × 4 B-tree).
const DUPLICATED: &[u8] = include_bytes!("fixtures/hdf5_duplicate_chunk_records.h5");
/// `(16, 1)` with one stored chunk that the index names at all 16 origins.
const SHARED: &[u8] = include_bytes!("fixtures/hdf5_shared_chunk_records.h5");
/// [`SHARED`] with record `i`'s filter mask raised by `2i`: bits above the one
/// gzip filter, which decode identically.
const SHARED_MASKS: &[u8] = include_bytes!("fixtures/hdf5_shared_chunk_records_masks.h5");
/// [`SHARED`] with record `i`'s stored size raised by `i` bytes.
const SHARED_SIZES: &[u8] = include_bytes!("fixtures/hdf5_shared_chunk_records_sizes.h5");
/// [`CONFLICTING`] with its first two records swapped; libhdf5 reads `[7, 9]`.
const CONFLICTING_SWAPPED: &[u8] =
    include_bytes!("fixtures/hdf5_conflicting_chunk_records_swapped.h5");
/// `[1, 2, 3, 4]` in chunks of two, with a record at origin 1, off the grid.
const OFF_GRID: &[u8] = include_bytes!("fixtures/hdf5_off_grid_chunk_record.h5");
/// `[7, 9]` plus a record at origin 5, outside the shape, addressed past the
/// end of the file.
const OUTSIDE: &[u8] = include_bytes!("fixtures/hdf5_outside_chunk_record.h5");
/// `[7, 9]` with an index naming chunks A and B both at origin 0.
const CONFLICTING: &[u8] = include_bytes!("fixtures/hdf5_conflicting_chunk_records.h5");

/// Decode `v`, the only root dataset, through `source`.
fn decode_v<S: ByteSource>(bytes: &[u8], source: &S) -> Result<Vec<Option<f64>>, String> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        panic!("fixture is not HDF5");
    };
    let children: Vec<u64> = list_root_children(source, probe)
        .expect("list children")
        .into_iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .map(|c| c.object_header_address)
        .collect();
    assert_eq!(children.len(), 1, "one dataset, `v`");
    fieldglass_netcdf::hdf5::values::read_dataset_values(source, children[0], probe)
        .map_err(|e| e.to_string())
}

#[test]
fn an_oversized_chunk_places_its_one_element() {
    // h5py reads [7] (`hdf5_oversized_chunk.h5.oracle.json`).
    let source = Recording::new(OVERSIZED);
    assert_eq!(decode_v(OVERSIZED, &source), Ok(vec![Some(7.0)]));
}

#[test]
fn identical_chunk_records_are_read_once() {
    let source = Recording::new(DUPLICATED);
    // libhdf5 reads [7] too: every record names the same chunk.
    assert_eq!(decode_v(DUPLICATED, &source), Ok(vec![Some(7.0)]));

    // The chunk fetch is one batch naming the chunk once, and the chunk is read
    // once, where it used to be read and inflated for each of its 16 records.
    let batches = source.prefetches();
    let plan = batches.last().expect("the chunk fetch is planned");
    assert_eq!(plan.len(), 1, "one chunk in the plan: {plan:?}");
    let reads = source.reads().iter().filter(|r| **r == plan[0]).count();
    assert_eq!(reads, 1, "the chunk's bytes are read once");
}

#[test]
fn a_chunk_named_at_many_origins_is_read_once() {
    // Legal, and libhdf5 reads sixteen 7s. Each origin gets the chunk's first
    // element, so every value is 7, but the chunk is inflated once rather than
    // once per origin.
    let source = Recording::new(SHARED);
    assert_eq!(decode_v(SHARED, &source), Ok(vec![Some(7.0); 16]));
    let batches = source.prefetches();
    let plan = batches.last().expect("the chunk fetch is planned");
    assert_eq!(plan.len(), 1, "one stored chunk in the plan: {plan:?}");
    let reads = source.reads().iter().filter(|r| **r == plan[0]).count();
    assert_eq!(reads, 1, "the chunk's bytes are read once");
}

#[test]
fn filter_mask_bits_past_the_pipeline_do_not_split_a_chunk() {
    // libhdf5 reads sixteen 7s. The pipeline has one filter, so only mask bit
    // 0 means anything, and the chunk is still inflated once (#888).
    let source = Recording::new(SHARED_MASKS);
    assert_eq!(decode_v(SHARED_MASKS, &source), Ok(vec![Some(7.0); 16]));
    let batches = source.prefetches();
    let plan = batches.last().expect("the chunk fetch is planned");
    assert_eq!(plan.len(), 1, "one stored chunk in the plan: {plan:?}");
    let reads = source.reads().iter().filter(|r| **r == plan[0]).count();
    assert_eq!(reads, 1, "the chunk's bytes are read once");
}

#[test]
fn one_stored_chunk_with_several_sizes_is_refused() {
    // libhdf5 reads sixteen 7s: zlib stops at its stream's end, so the extra
    // bytes change nothing for gzip. Reading each stated size would inflate the
    // chunk once per record, and for another pipeline (fletcher32's trailing
    // checksum) the sizes would decode differently, so the index is refused:
    // a known divergence (ADR-0013).
    let source = Recording::new(SHARED_SIZES);
    let err = decode_v(SHARED_SIZES, &source).expect_err("sixteen sizes for one chunk");
    assert!(err.contains("twice with different storage"), "{err}");
    // The stored chunk is 16,318 bytes; every metadata read is far smaller.
    let decoded = source.reads().iter().filter(|r| r.len >= 16_000).count();
    assert_eq!(decoded, 0, "no chunk is read before the refusal");
}

#[test]
fn two_chunks_at_one_origin_are_refused() {
    // libhdf5 2.0.0 reads [9, 9], the last record at origin 0, and [7, 9] with
    // the first two records swapped (`hdf5_conflicting_chunk_records.h5
    // .oracle.json`). Which value origin 0 holds is not in the file, so the
    // reader refuses rather than pick one: the divergence ADR-0013 records.
    let source = Recording::new(CONFLICTING);
    let err = decode_v(CONFLICTING, &source).expect_err("an ambiguous index");
    assert!(err.contains("two different chunks at origin [0]"), "{err}");
}

#[test]
fn the_public_reader_decodes_the_duplicated_index() {
    // The same answer through `decode_variable_raw`, the entry point the issue
    // names.
    let reader = NetcdfReader::from_bytes(DUPLICATED.to_vec()).expect("parse");
    // `v` is the only dataset, so it is variable 0.
    let values = reader.decode_variable_raw(0).expect("decodes");
    assert_eq!(values, vec![Some(7.0)]);
}

#[test]
fn the_swapped_conflict_is_refused_too() {
    // The evidence for the divergence: the same records in another order make
    // libhdf5 read `[7, 9]` instead of `[9, 9]`
    // (`hdf5_conflicting_chunk_records_swapped.h5.oracle.json`). The reader's
    // answer does not depend on the order: both are refused.
    let source = Recording::new(CONFLICTING_SWAPPED);
    let err = decode_v(CONFLICTING_SWAPPED, &source).expect_err("an ambiguous index");
    assert!(err.contains("two different chunks at origin [0]"), "{err}");
}

#[test]
fn an_off_grid_origin_inside_the_shape_is_refused() {
    // libhdf5 refuses it too: "bad coordinate offset"
    // (`hdf5_off_grid_chunk_record.h5.oracle.json`).
    let source = Recording::new(OFF_GRID);
    let err = decode_v(OFF_GRID, &source).expect_err("origin 1, chunk edge 2");
    assert!(err.contains("not on the chunk grid"), "{err}");
}

#[test]
fn a_record_outside_the_shape_is_never_read() {
    // libhdf5 reads `[7, 9]` (`hdf5_outside_chunk_record.h5.oracle.json`). The
    // record at origin 5 names an address past the end of the file, so
    // reading it would fail; nothing reaches it.
    let source = Recording::new(OUTSIDE);
    assert_eq!(decode_v(OUTSIDE, &source), Ok(vec![Some(7.0), Some(9.0)]));
    let len = OUTSIDE.len() as u64;
    let past_end: Vec<_> = source
        .reads()
        .into_iter()
        .filter(|r| r.start >= len)
        .collect();
    assert!(past_end.is_empty(), "read past the end: {past_end:?}");
}
