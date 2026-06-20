//! HDF5 / NetCDF-4 variable value decode (issue #121) against the bundled
//! fixtures, pinned to the values `h5py` reports. Covers contiguous storage of
//! every numeric class and byte order, a scalar, and a chunked (version-1
//! B-tree) dataset. String datasets and datasets stored with a Data Layout the
//! reader doesn't decode yet (the v2 fixture's version-4 chunk indexes) must
//! report a clean error rather than panic.

use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};
use std::collections::BTreeMap;

const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
const V2_LINKINFO: &[u8] = include_bytes!("fixtures/hdf5_v2_linkinfo.h5");
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
fn version_4_chunk_index_errors_cleanly() {
    // The v110 fixture stores its chunked datasets with version-4 layout
    // (fixed / extensible array indexes), which this reader doesn't decode yet.
    // It must surface an error, never panic.
    let got = decode_all(V2_LINKINFO);
    // Contiguous datasets in the same file still decode.
    assert_eq!(present(&got["dense_attrs"]), vec![0.0, 1.0, 2.0]);
    assert!(got["chunked"].is_err(), "v4-indexed chunk should error");
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
