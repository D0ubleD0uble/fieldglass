//! NetCDF-4 nested-group resolution (#219) against the bundled grouped fixture,
//! pinned to what the canonical `netCDF4` library reports — committed as
//! `netcdf4_grouped.nc.oracle.json`.
//!
//! `netcdf4_grouped.nc` (built by `tools/build_netcdf4_grouped_fixture.py`) is a
//! Sentinel-5P-style layout: a root `time` coordinate variable, a `/PRODUCT`
//! group with its own `scanline` / `ground_pixel` grid and variables, a variable
//! (`/PRODUCT/qa_value`) whose `DIMENSION_LIST` mixes the ancestor root `time`
//! dimension with its group's own dimensions, and a two-level-deep
//! `/PRODUCT/SUPPORT_DATA/surface_altitude`. Objects in groups are presented with
//! path-qualified names.

use std::collections::BTreeMap;

use fieldglass_netcdf::{Hdf5Metadata, NetcdfReader};
use serde_json::Value;

const GROUPED: &[u8] = include_bytes!("fixtures/netcdf4_grouped.nc");
const GROUPED_ORACLE: &str = include_str!("fixtures/netcdf4_grouped.nc.oracle.json");

fn resolve(bytes: &[u8]) -> Hdf5Metadata {
    NetcdfReader::from_bytes(bytes.to_vec())
        .expect("recognised NetCDF-4")
        .hdf5_metadata()
        .expect("nested-group resolution")
}

#[test]
fn dimensions_across_groups_match_the_oracle() {
    let meta = resolve(GROUPED);
    let oracle: Value = serde_json::from_str(GROUPED_ORACLE).expect("oracle parses");

    let got: BTreeMap<&str, (u64, bool)> = meta
        .dimensions
        .iter()
        .map(|d| (d.name.as_str(), (d.length, d.is_unlimited)))
        .collect();
    let want: BTreeMap<String, (u64, bool)> = oracle["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap().to_string(),
                (
                    d["length"].as_u64().unwrap(),
                    d["unlimited"].as_bool().unwrap(),
                ),
            )
        })
        .collect();

    assert_eq!(got.len(), want.len(), "dimension count");
    for (name, expected) in &want {
        assert_eq!(got.get(name.as_str()), Some(expected), "dimension {name}");
    }
    // The nested-group dimension is path-qualified; the root one stays bare.
    assert_eq!(got["/PRODUCT/scanline"], (3, false));
    assert_eq!(got["time"], (2, false));
}

#[test]
fn variables_across_groups_match_the_oracle() {
    let meta = resolve(GROUPED);
    let oracle: Value = serde_json::from_str(GROUPED_ORACLE).expect("oracle parses");

    let got: BTreeMap<&str, (&str, Vec<&str>, bool)> = meta
        .variables
        .iter()
        .map(|v| {
            (
                v.name.as_str(),
                (
                    v.nc_type.name(),
                    v.dimensions.iter().map(String::as_str).collect(),
                    v.is_coordinate,
                ),
            )
        })
        .collect();
    let want: BTreeMap<String, (String, Vec<String>, bool)> = oracle["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["name"].as_str().unwrap().to_string(),
                (
                    v["nc_type"].as_str().unwrap().to_string(),
                    v["dimensions"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|d| d.as_str().unwrap().to_string())
                        .collect(),
                    v["is_coordinate"].as_bool().unwrap(),
                ),
            )
        })
        .collect();

    assert_eq!(got.len(), want.len(), "variable count");
    for (name, (nc_type, dims, is_coord)) in &want {
        let actual = got
            .get(name.as_str())
            .unwrap_or_else(|| panic!("missing variable {name}"));
        assert_eq!(actual.0, nc_type, "{name} nc_type");
        assert_eq!(
            &actual.1,
            &dims.iter().map(String::as_str).collect::<Vec<_>>(),
            "{name} dims"
        );
        assert_eq!(actual.2, *is_coord, "{name} is_coordinate");
    }

    // A nested variable is present with its path-qualified name...
    assert!(got.contains_key("/PRODUCT/latitude"), "nested var listed");
    // ...a two-level-deep one is qualified through both groups...
    assert!(
        got.contains_key("/PRODUCT/SUPPORT_DATA/surface_altitude"),
        "two-level nested var listed"
    );
    // ...and the ancestor-scoping variable mixes the root `time` dimension with
    // its own group's dimensions in order (the netCDF dimension-visibility rule).
    assert_eq!(
        got["/PRODUCT/qa_value"].1,
        vec!["time", "/PRODUCT/scanline", "/PRODUCT/ground_pixel"]
    );
}

#[test]
fn nested_variable_decodes_through_the_reader() {
    // The decode index recorded on a nested variable must point at that dataset
    // through `variable_shape` / `decode_variable_values`, so a grouped variable
    // renders like a root one.
    let reader = NetcdfReader::from_bytes(GROUPED.to_vec()).expect("recognised NetCDF-4");
    let meta = reader.hdf5_metadata().expect("nested-group resolution");
    let var = |name: &str| {
        meta.variables
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} variable"))
    };

    // `/PRODUCT/qa_value` is 2 × 3 × 4 (time × scanline × ground_pixel), arange.
    let qa = var("/PRODUCT/qa_value");
    assert_eq!(
        reader.variable_shape(qa.decode_index).unwrap(),
        vec![2, 3, 4]
    );
    let values = reader.decode_variable_values(qa.decode_index).unwrap();
    assert_eq!(
        values.iter().map(|v| v.unwrap()).collect::<Vec<_>>(),
        (0..24).map(|i| i as f64).collect::<Vec<_>>()
    );

    // The two-level-deep variable decodes to its own 3 × 4 grid.
    let alt = var("/PRODUCT/SUPPORT_DATA/surface_altitude");
    assert_eq!(reader.variable_shape(alt.decode_index).unwrap(), vec![3, 4]);
    assert_eq!(
        reader
            .decode_variable_values(alt.decode_index)
            .unwrap()
            .len(),
        12
    );
}
