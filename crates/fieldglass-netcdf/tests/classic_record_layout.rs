//! NetCDF classic record layout, cross-checked against netCDF4 (#204).
//!
//! Record `r` of a record variable starts at `begin + r * recsize`. The spec
//! makes `recsize` the sum of every record variable's padded `vsize`, except
//! that when there is exactly one record variable its records are not padded
//! at all, and "readers should ignore vsize and assume no padding" because the
//! writer still stores the padded size. These fixtures were written by
//! libnetcdf; `tools/build_netcdf_record_layout_fixtures.py` generates them and
//! the oracle.

use fieldglass_netcdf::{NetcdfBacking, NetcdfReader, classic};
use serde_json::Value;

const ORACLE: &str = include_str!("fixtures/record_layout.oracle.json");

const FIXTURES: [(&str, &[u8]); 5] = [
    (
        "record_single_short_cdf1.nc",
        include_bytes!("fixtures/record_single_short_cdf1.nc"),
    ),
    (
        "record_single_short_cdf2.nc",
        include_bytes!("fixtures/record_single_short_cdf2.nc"),
    ),
    (
        "record_single_short_cdf5.nc",
        include_bytes!("fixtures/record_single_short_cdf5.nc"),
    ),
    (
        "record_single_ubyte_cdf5.nc",
        include_bytes!("fixtures/record_single_ubyte_cdf5.nc"),
    ),
    (
        "record_mixed_cdf1.nc",
        include_bytes!("fixtures/record_mixed_cdf1.nc"),
    ),
];

fn header(reader: &NetcdfReader) -> &classic::ClassicHeader {
    match &reader.backing {
        NetcdfBacking::Classic(header) => header,
        other => panic!("expected a classic backing, got {other:?}"),
    }
}

/// Every numeric variable of every fixture decodes to what netCDF4 read.
#[test]
fn every_record_layout_fixture_matches_netcdf4() {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    for (name, bytes) in FIXTURES {
        let expected = oracle["files"][name]
            .as_object()
            .unwrap_or_else(|| panic!("{name}: no oracle entry"));
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect(name);
        let header = header(&reader);
        assert!(!expected.is_empty(), "{name}: oracle lists no variables");
        for (var_name, want) in expected {
            let index = header
                .variables
                .iter()
                .position(|v| &v.name == var_name)
                .unwrap_or_else(|| panic!("{name}: no variable {var_name}"));
            let shape: Vec<u64> = want["shape"]
                .as_array()
                .unwrap()
                .iter()
                .map(|d| d.as_u64().unwrap())
                .collect();
            let values: Vec<Option<f64>> = want["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| Some(x.as_f64().unwrap()))
                .collect();
            assert_eq!(
                classic::variable_shape(header, index).unwrap(),
                shape,
                "{name}/{var_name} shape"
            );
            let got = reader
                .decode_variable_raw(index)
                .unwrap_or_else(|e| panic!("{name}/{var_name}: {e}"));
            assert_eq!(got, values, "{name}/{var_name} values");
        }
    }
}

/// The single-record-variable files store the padded `vsize` the spec tells
/// writers to store, and the plan ignores it: records sit `x * 2` (short) or
/// `x * 1` (ubyte) bytes apart, not `vsize` apart.
#[test]
fn a_lone_record_variable_is_planned_unpadded_despite_its_padded_vsize() {
    for (name, bytes) in &FIXTURES[..4] {
        let header = classic::parse_header(bytes).unwrap();
        let var = &header.variables[0];
        let elem = if name.contains("ubyte") { 1 } else { 2 };
        let slab: u64 = 3 * elem;
        assert_eq!(var.vsize, 4 * slab.div_ceil(4), "{name}: vsize is padded");
        let plan = classic::variable_plan(&header, 0).unwrap();
        let starts: Vec<u64> = plan.iter().map(|r| r.start).collect();
        assert_eq!(
            starts,
            vec![var.begin, var.begin + slab, var.begin + 2 * slab],
            "{name}"
        );
        assert!(plan.iter().all(|r| r.len == slab), "{name}");
        // The last record ends exactly at the end of the file: no padding.
        assert_eq!(var.begin + 3 * slab, bytes.len() as u64, "{name}");
    }
}

/// With more than one record variable every slab is padded to 4 bytes, the
/// `char` variable included, so the stride is 4 + 4 + 4.
#[test]
fn several_record_variables_are_planned_padded() {
    let (_, bytes) = FIXTURES[4];
    let header = classic::parse_header(bytes).unwrap();
    for name in ["a", "b"] {
        let index = header
            .variables
            .iter()
            .position(|v| v.name == name)
            .unwrap();
        let begin = header.variables[index].begin;
        let plan = classic::variable_plan(&header, index).unwrap();
        let starts: Vec<u64> = plan.iter().map(|r| r.start).collect();
        assert_eq!(starts, vec![begin, begin + 12, begin + 24], "{name}");
    }
}
