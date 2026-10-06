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
