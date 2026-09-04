//! A NetCDF-4 file that mixes decodable variables with datasets whose HDF5
//! datatype is outside the decoded subset (#550).
//!
//! `hdf5/datatype.rs` maps three classes onto `NcType` — fixed-point, IEEE
//! floating point, fixed-length string. Compound, enum, variable-length,
//! opaque and array are not decoded, and files that carry one alongside
//! ordinary fields are routine: station-record files, OMI / TROPOMI granules,
//! anything written through `nc_def_compound`. Such a dataset used to fail
//! metadata resolution for the whole file, so a host showed nothing rather
//! than everything but the one variable.
//!
//! `netcdf4_unsupported_type.nc` is built by
//! `tools/build_netcdf4_unsupported_type_fixture.py`; its oracle records what
//! the canonical `netCDF4` library sees, including which variables carry a
//! datatype this reader declines.

use std::collections::BTreeMap;

use fieldglass_core::FieldglassError;
use fieldglass_netcdf::{DatasetView, Hdf5Metadata, NetcdfReader};
use serde_json::Value;

const MIXED: &[u8] = include_bytes!("fixtures/netcdf4_unsupported_type.nc");
const MIXED_ORACLE: &str = include_str!("fixtures/netcdf4_unsupported_type.nc.oracle.json");
const CLASSIC: &[u8] = include_bytes!("fixtures/netcdf_classic_dummy.nc");

fn reader() -> NetcdfReader {
    NetcdfReader::from_bytes(MIXED.to_vec()).expect("recognised NetCDF-4")
}

fn resolve() -> Hdf5Metadata {
    reader().hdf5_metadata().expect("metadata resolves")
}

fn oracle() -> Value {
    serde_json::from_str(MIXED_ORACLE).expect("oracle parses")
}

/// Every variable the oracle says has a decodable datatype is listed, and no
/// variable it says does not. This is the acceptance criterion: the file
/// resolves at all, and it resolves to *everything but* the two datasets whose
/// type this build has no `NcType` for.
#[test]
fn the_decodable_variables_are_listed_and_the_others_are_not() {
    let meta = resolve();
    let oracle = oracle();

    let listed: BTreeMap<&str, &str> = meta
        .variables
        .iter()
        .map(|v| (v.name.as_str(), v.nc_type.name()))
        .collect();

    let mut expected_listed = 0;
    for var in oracle["variables"].as_array().unwrap() {
        let name = var["name"].as_str().unwrap();
        let nc_type = var["nc_type"].as_str().unwrap();
        if var["decodable_datatype"].as_bool().unwrap() {
            expected_listed += 1;
            assert_eq!(
                listed.get(name),
                Some(&nc_type),
                "{name} should be listed as {nc_type}"
            );
        } else {
            assert!(
                !listed.contains_key(name),
                "{name} carries a {nc_type} datatype and must not be listed as a variable"
            );
        }
    }
    assert_eq!(
        listed.len(),
        expected_listed,
        "listed variables: {listed:?}"
    );
    // Not vacuous: the fixture has to contain both kinds for this to mean
    // anything.
    assert!(
        expected_listed >= 2,
        "fixture must carry decodable variables"
    );
    assert_eq!(meta.unsupported.len(), 2, "fixture must carry skipped ones");
}

/// The skipped datasets are reported by name, with a reason that says which
/// HDF5 class stopped them — not a bare "unsupported".
#[test]
fn each_skipped_dataset_is_reported_with_its_class() {
    let meta = resolve();
    let by_name: BTreeMap<&str, &str> = meta
        .unsupported
        .iter()
        .map(|u| (u.name.as_str(), u.reason.as_str()))
        .collect();

    assert_eq!(
        by_name.keys().copied().collect::<Vec<_>>(),
        ["station_info", "visits"]
    );
    // Class 6 is compound, class 9 variable-length: the two the fixture writes.
    assert!(
        by_name["station_info"].contains("class 6 (compound)"),
        "reason was {:?}",
        by_name["station_info"]
    );
    assert!(
        by_name["visits"].contains("class 9 (variable-length)"),
        "reason was {:?}",
        by_name["visits"]
    );
}

/// Skipping must not renumber the variables that follow. A skipped dataset
/// keeps its slot in the whole-file dataset list, which is the index space
/// `decode_variable_values` walks — so `temperature`'s `decode_index` still
/// decodes `temperature`, not its neighbour.
///
/// The fixture is built with both undecodable datasets ahead of `temperature`
/// for exactly this reason: closing the gap would shift it and the test would
/// read the wrong array.
#[test]
fn a_skipped_dataset_does_not_shift_the_decode_indices_after_it() {
    let reader = reader();
    let meta = reader.hdf5_metadata().expect("metadata resolves");
    let oracle = oracle();

    for (name, key) in [("time", "time"), ("temperature", "temperature")] {
        let var = meta
            .variables
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} is listed"));
        let decoded = reader
            .decode_variable_values(var.decode_index)
            .unwrap_or_else(|e| panic!("{name} decodes: {e}"));
        let want: Vec<f64> = oracle["values"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let got: Vec<f64> = decoded.iter().map(|v| v.expect("no fill values")).collect();
        assert_eq!(got, want, "{name} at decode index {}", var.decode_index);
    }

    // And the skipped datasets still occupy indices of their own: the two
    // listed variables plus the pure `station` dimension plus the two skips is
    // five datasets, so at least one index in range decodes to neither.
    let indices: Vec<usize> = meta.variables.iter().map(|v| v.decode_index).collect();
    assert_eq!(indices.len(), 2);
    assert!(
        indices.iter().max().copied().unwrap_or(0) >= 2,
        "the skips sit below the listed variables: {indices:?}"
    );
}

/// The neutral view carries the report too, so a host that never touches
/// `Hdf5Metadata` still learns what it is not being shown.
#[test]
fn the_dataset_view_carries_the_report() {
    let view = reader().view().expect("view resolves");
    assert_eq!(
        view.vars
            .iter()
            .map(|v| v.name.as_str())
            .collect::<Vec<_>>(),
        ["temperature", "time"]
    );
    assert_eq!(
        view.unsupported
            .iter()
            .map(|u| u.name.as_str())
            .collect::<Vec<_>>(),
        ["station_info", "visits"]
    );
}

/// A classic file has no such datatypes, so its view reports none — the field
/// is not a NetCDF-4 detail leaking into the neutral surface.
#[test]
fn a_classic_view_reports_nothing_unsupported() {
    let reader = NetcdfReader::from_bytes(CLASSIC.to_vec()).expect("recognised classic");
    let view = reader.view().expect("classic view");
    assert!(!view.vars.is_empty());
    assert!(view.unsupported.is_empty());
}

/// `DatasetView::default()` — the whole-file fallback — is still the empty
/// view in every field, including the new one.
#[test]
fn the_empty_view_reports_nothing_unsupported() {
    assert!(DatasetView::default().unsupported.is_empty());
}

/// Asking a classic file for HDF5 metadata is a wrong-method condition, not a
/// parse failure, and now says so in the type. A consumer can tell the two
/// apart without reading the message.
#[test]
fn hdf5_metadata_on_a_classic_backing_is_not_a_parse_error() {
    let reader = NetcdfReader::from_bytes(CLASSIC.to_vec()).expect("recognised classic");
    match reader.hdf5_metadata() {
        Err(FieldglassError::WrongLayout(msg)) => {
            assert!(msg.contains("NetCDF-4 / HDF5"), "message was {msg:?}");
        }
        other => panic!("expected WrongLayout, got {other:?}"),
    }
}
