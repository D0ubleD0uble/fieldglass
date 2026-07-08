//! HDF5 / NetCDF-4 variable value decode (issues #121, #216) against the bundled
//! fixtures, pinned to the values `h5py` reports. Covers contiguous storage of
//! every numeric class and byte order, a scalar, and chunked datasets across
//! every chunk index the reader decodes: the version-1 B-tree, and the
//! version-4 single-chunk, fixed-array, extensible-array, v2-B-tree (all
//! filtered + unfiltered), and implicit indexes, including the version-5 layout
//! message
//! libhdf5 2.0 writes for a filtered chunk. String datasets must report a clean
//! error rather than panic.

use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};
use std::collections::BTreeMap;

const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
const V2_LINKINFO: &[u8] = include_bytes!("fixtures/hdf5_v2_linkinfo.h5");
const V4_CHUNK_INDEX: &[u8] = include_bytes!("fixtures/hdf5_v4_chunk_index.h5");
const EA_CHUNK_INDEX: &[u8] = include_bytes!("fixtures/hdf5_ea_chunk_index.h5");
const EA_FILTERED: &[u8] = include_bytes!("fixtures/hdf5_ea_filtered.h5");
const IMPLICIT_INDEX: &[u8] = include_bytes!("fixtures/hdf5_implicit_index.h5");
const V2_BTREE_INDEX: &[u8] = include_bytes!("fixtures/hdf5_v2_btree_index.h5");
const DUMMY: &[u8] = include_bytes!("fixtures/netcdf4_hdf5_dummy.nc");

/// Decode every root dataset, mapping name → Result of present-or-None values.
fn decode_all(bytes: &[u8]) -> BTreeMap<String, Result<Vec<Option<f64>>, String>> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
    let probe = match &reader.backing {
        NetcdfBacking::Hdf5(p) => p.clone(),
        other => panic!("expected HDF5 backing, got {}", other.label()),
    };
    let datasets: Vec<String> = list_root_children(bytes, &probe)
        .expect("list children")
        .into_iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .map(|c| c.name)
        .collect();

    datasets
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let decoded = reader
                .decode_variable_values(index)
                .map_err(|e| e.to_string());
            (name.clone(), decoded)
        })
        .collect()
}

/// Present values only — panics if any point came back masked (`None`).
fn present(result: &Result<Vec<Option<f64>>, String>) -> Vec<f64> {
    result
        .as_ref()
        .expect("decode succeeded")
        .iter()
        .map(|v| v.expect("no masked points expected"))
        .collect()
}

#[test]
fn decodes_contiguous_numeric_datasets() {
    let got = decode_all(V1_SYMBOLTABLE);
    assert_eq!(present(&got["scalar_i32"]), vec![42.0]);
    assert_eq!(present(&got["temp_be_i32"]), vec![0.0, 1.0, 2.0, 3.0, 4.0]);
    assert_eq!(
        present(&got["temp_f32"]),
        vec![0.0, 1.5, 3.0, 4.5, 6.0, 7.5, 9.0, 10.5]
    );
    // linspace(0, 1, 6) in IEEE doubles — compared with a tolerance since 0.2
    // and 0.6 aren't exact.
    let f64s = present(&got["temp_f64"]);
    let expected = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
    assert_eq!(f64s.len(), expected.len());
    for (got, want) in f64s.iter().zip(expected) {
        assert!((got - want).abs() < 1e-12, "got {got}, want {want}");
    }
    assert_eq!(
        present(&got["temp_i32"]),
        (0..12).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn masks_against_typed_float_fill_value() {
    // `masked` is all fill, with a non-round float `_FillValue` attribute
    // (-9999.55 in f32). Masking must compare the decoded element against the
    // *typed* fill, not the rounded display string — every point is missing.
    let got = decode_all(V1_SYMBOLTABLE);
    let masked = got["masked"].as_ref().expect("masked decodes");
    assert_eq!(masked.len(), 6);
    assert!(
        masked.iter().all(|v| v.is_none()),
        "every point equals the float _FillValue, so all should mask: {masked:?}"
    );
}

#[test]
fn decodes_chunked_btree_v1_dataset() {
    // `record` is chunked with a version-1 B-tree index in the earliest-libver
    // fixture: chunks=(4,), values 0..3.
    let got = decode_all(V1_SYMBOLTABLE);
    assert_eq!(present(&got["record"]), vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn decodes_compressed_chunked_btree_v1_dataset() {
    // `compressed` is an 8×8 float field stored in 4×4 chunks through deflate +
    // shuffle, indexed by a version-1 B-tree (earliest libver). It exercises the
    // whole chunked read path: B-tree walk → filter-pipeline reverse → scatter.
    let got = decode_all(V1_SYMBOLTABLE);
    assert_eq!(
        present(&got["compressed"]),
        (0..64).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn rejects_string_dataset() {
    let got = decode_all(V1_SYMBOLTABLE);
    let err = got["label"].as_ref().unwrap_err();
    assert!(err.contains("text"), "unexpected error: {err}");
}

#[test]
fn decodes_v4_filtered_fixed_array_dataset() {
    // The v110 ("latest format") fixture stores `chunked` — a fixed-shape 10×10
    // float field in 5×5 gzip chunks — with a version-4 layout indexed by a
    // filtered Fixed Array. It exercises the whole v4 path: parse the v4 layout
    // message → walk the Fixed Array header + data block → filter reverse →
    // scatter. Values are arange(100), pinned to h5py.
    let got = decode_all(V2_LINKINFO);
    // A contiguous dataset in the same file still decodes.
    assert_eq!(present(&got["dense_attrs"]), vec![0.0, 1.0, 2.0]);
    assert_eq!(
        present(&got["chunked"]),
        (0..100).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_single_chunk_dataset() {
    // In the latest-format fixture, a dataset whose chunk shape equals its
    // dataset shape is stored as one chunk under the version-4 Single Chunk
    // index — inline in the layout message, with no external index structure.
    // Values are arange(16).
    let got = decode_all(V4_CHUNK_INDEX);
    assert_eq!(
        present(&got["single_chunk"]),
        (0..16).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v5_filtered_single_chunk_dataset() {
    // libhdf5 2.0 writes a *filtered* single chunk with a data-layout message
    // version 5 (not the version 4 the unfiltered case uses). For the chunked
    // class, version 5 is encoded byte-for-byte like version 4, so `single_chunk_
    // filtered` — a 4×4 gzip+shuffle field stored as one chunk with the filtered
    // size and mask inline in the layout message — decodes to arange(16).
    let got = decode_all(V4_CHUNK_INDEX);
    assert_eq!(
        present(&got["single_chunk_filtered"]),
        (0..16).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_unfiltered_fixed_array_dataset() {
    // `fixed_array` is a fixed-shape 8×8 field in 4×4 chunks with no filters,
    // indexed by an unfiltered version-4 Fixed Array (each element is just a
    // chunk address). Values are arange(64).
    let got = decode_all(V4_CHUNK_INDEX);
    assert_eq!(
        present(&got["fixed_array"]),
        (0..64).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_implicit_index_dataset() {
    // `implicit` is a fixed-shape 8×8 field in 4×4 chunks, unfiltered and early-
    // allocated, so libhdf5 indexes it with the version-4 Implicit index: the
    // four chunks are stored contiguously from a base address with no on-disk
    // index. The reader locates chunk `i` at `base + i * chunk_bytes`. Values are
    // arange(64).
    let got = decode_all(IMPLICIT_INDEX);
    assert_eq!(
        present(&got["implicit"]),
        (0..64).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_implicit_index_partial_edge_chunks() {
    // `implicit_partial` is 5×7 in 4×4 chunks: a 2×2 grid of full-size chunks
    // whose right and bottom chunks hang past the dataset bounds. Exercises the
    // implicit index together with edge-chunk clipping on scatter. Values are
    // arange(35).
    let got = decode_all(IMPLICIT_INDEX);
    assert_eq!(
        present(&got["implicit_partial"]),
        (0..35).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_extensible_array_direct_data_blocks() {
    // `ea_direct` is a 1-D unlimited dataset of 150 chunks (600 elements), which
    // spans super blocks 0–3 whose growing-size data blocks (16, 32, 32, 64, …)
    // are addressed directly from the extensible-array index block — no secondary
    // block. Exercises the super-block doubling walk. Values are arange(600).
    let got = decode_all(EA_CHUNK_INDEX);
    assert_eq!(
        present(&got["ea_direct"]),
        (0..600).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_extensible_array_secondary_block() {
    // `ea_secondary` is a 1-D unlimited dataset of 280 chunks (1120 elements),
    // large enough that libhdf5 allocates a secondary block for super block 4.
    // The reader must walk the index block's secondary-block pointer to reach the
    // data-block addresses beyond the direct slots. Values are arange(1120).
    let got = decode_all(EA_CHUNK_INDEX);
    assert_eq!(
        present(&got["ea_secondary"]),
        (0..1120).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_extensible_array_index_block_only() {
    // `record` in the v110 fixture is a 1-D unlimited-dimension dataset with a
    // single 4-element chunk, so its one chunk address lives directly in the
    // extensible-array index block (no data blocks). Values 0..3.
    let got = decode_all(V2_LINKINFO);
    assert_eq!(present(&got["record"]), vec![0.0, 1.0, 2.0, 3.0]);
}

#[test]
fn decodes_v4_filtered_extensible_array_index_block_only() {
    // `ea_filtered_iblock` is a 1-D unlimited gzip+shuffle dataset of 4 chunks
    // (16 elements): few enough that every chunk address lives directly in the
    // extensible-array index block, so no data blocks are walked. Its elements
    // are the *filtered* form (client id 1: address + on-disk size + filter mask),
    // and each chunk passes back through the filter pipeline. Values arange(16).
    let got = decode_all(EA_FILTERED);
    assert_eq!(
        present(&got["ea_filtered_iblock"]),
        (0..16).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_filtered_extensible_array_direct_data_blocks() {
    // `ea_filtered_direct` is a 1-D unlimited gzip+shuffle dataset of 150 chunks
    // (600 elements) spanning super blocks 0–3, whose data blocks are addressed
    // directly from the index block (no secondary block). Exercises reading the
    // filtered element triple inside data blocks across the doubling walk.
    let got = decode_all(EA_FILTERED);
    assert_eq!(
        present(&got["ea_filtered_direct"]),
        (0..600).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_filtered_extensible_array_secondary_block() {
    // `ea_filtered_secondary` is a 1-D unlimited gzip+shuffle dataset of 280
    // chunks (1120 elements), large enough that libhdf5 allocates a secondary
    // block for super block 4. The reader walks the secondary-block pointer to
    // the data-block addresses beyond the index block's direct slots, reading a
    // filtered element from each. Values arange(1120).
    let got = decode_all(EA_FILTERED);
    assert_eq!(
        present(&got["ea_filtered_secondary"]),
        (0..1120).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_v2_btree_unfiltered_dataset() {
    // `bt2` has two unlimited dimensions (4×4 in 2×2 chunks), so libhdf5 indexes
    // its chunks with a version-2 B-tree (chunk index type 5) rather than a fixed
    // or extensible array. Its type-10 records carry each chunk's scaled chunk-grid
    // coordinate, which the reader multiplies by the chunk edge to place the
    // chunk. Values are arange(16), pinned to h5py.
    let got = decode_all(V2_BTREE_INDEX);
    assert_eq!(
        present(&got["bt2"]),
        (0..16).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_v2_btree_multi_chunk_dataset() {
    // `bt2_multi` (8×8 in 2×2 chunks → 16 chunks) keeps several records per B-tree
    // leaf, so the whole record set is gathered and scattered by scaled offset.
    // Values are arange(64).
    let got = decode_all(V2_BTREE_INDEX);
    assert_eq!(
        present(&got["bt2_multi"]),
        (0..64).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_v4_v2_btree_filtered_dataset() {
    // `bt2_filtered` (4×4 in 2×2 gzip+shuffle chunks) uses the filtered type-11
    // records: address + on-disk chunk size + filter mask + scaled offsets. Each
    // chunk passes back through the filter pipeline before scatter. The on-disk
    // size field width is derived from the record width the B-tree advertises.
    // Values are arange(16).
    let got = decode_all(V2_BTREE_INDEX);
    assert_eq!(
        present(&got["bt2_filtered"]),
        (0..16).map(|i| i as f64).collect::<Vec<_>>()
    );
}

#[test]
fn decodes_dummy_netcdf4_dimension_variables() {
    // The dummy NetCDF-4 file's `v` (int32 0..9) and `x` (big-endian f32, all
    // zero) are contiguous dimension variables.
    let got = decode_all(DUMMY);
    assert_eq!(
        present(&got["v"]),
        (0..10).map(|i| i as f64).collect::<Vec<_>>()
    );
    assert_eq!(present(&got["x"]), vec![0.0; 10]);
}
