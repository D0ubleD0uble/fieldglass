//! NetCDF-4 dimension-scale resolution (#174, under #33; decision 0003) against
//! the bundled NetCDF-4 fixtures, pinned to what the canonical `netCDF4` library
//! (`ncdump -h`) reports — committed as `*.oracle.json`.
//!
//! `netcdf4_dimscale.nc` is built by `tools/build_netcdf4_dimscale_fixture.py`
//! and covers every classification the resolver makes: an unlimited dimension
//! with a coordinate variable, regular coordinate variables, a pure dimension
//! (the placeholder scale with no values), a multi-dimensional data variable
//! whose `DIMENSION_LIST` must resolve to ordered names, and a variable that
//! references the pure dimension.

use std::collections::BTreeMap;

use fieldglass_netcdf::{Hdf5Metadata, NetcdfReader};
use serde_json::Value;

const DIMSCALE: &[u8] = include_bytes!("fixtures/netcdf4_dimscale.nc");
const DIMSCALE_ORACLE: &str = include_str!("fixtures/netcdf4_dimscale.nc.oracle.json");
const DUMMY: &[u8] = include_bytes!("fixtures/netcdf4_hdf5_dummy.nc");

fn resolve(bytes: &[u8]) -> Hdf5Metadata {
    NetcdfReader::from_bytes(bytes.to_vec())
        .expect("recognised NetCDF-4")
        .hdf5_metadata()
        .expect("dimension-scale resolution")
}

#[test]
fn dimensions_match_the_ncdump_oracle() {
    let meta = resolve(DIMSCALE);
    let oracle: Value = serde_json::from_str(DIMSCALE_ORACLE).expect("oracle parses");

    // name -> (length, unlimited), order-independent.
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
    // The unlimited record dimension is resolved as such.
    assert_eq!(got["time"], (2, true));
    // The pure dimension (no coordinate variable) is present as a dimension.
    assert_eq!(got["nv"], (2, false));
}

#[test]
fn variables_match_the_ncdump_oracle() {
    let meta = resolve(DIMSCALE);
    let oracle: Value = serde_json::from_str(DIMSCALE_ORACLE).expect("oracle parses");

    // name -> (nc_type, ordered dims, is_coordinate).
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

    // The pure dimension `nv` is a dimension, never a variable.
    assert!(
        !got.contains_key("nv"),
        "pure dimension must not be a variable"
    );
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

    // Spot-check the multi-dimensional data variable's ordered DIMENSION_LIST.
    assert_eq!(got["temperature"].1, vec!["time", "lat", "lon"]);
    assert_eq!(got["lat_bnds"].1, vec!["lat", "nv"]);
}

#[test]
fn machinery_attributes_are_hidden_user_metadata_kept() {
    let meta = resolve(DIMSCALE);
    let temp = meta
        .variables
        .iter()
        .find(|v| v.name == "temperature")
        .expect("temperature variable");
    let attr_names: Vec<&str> = temp.attributes.iter().map(|a| a.name.as_str()).collect();
    // NetCDF-4 machinery is filtered out...
    for hidden in ["DIMENSION_LIST", "CLASS", "NAME", "_Netcdf4Dimid"] {
        assert!(!attr_names.contains(&hidden), "{hidden} should be hidden");
    }
    // ...but real CF metadata survives.
    assert!(attr_names.contains(&"units"), "units kept: {attr_names:?}");
}

#[test]
fn pure_dimension_placeholder_resolves_in_the_dummy() {
    // The committed dummy has a single pure dimension `x` (no coordinate
    // variable) and one data variable `v(x)` — the placeholder + DIMENSION_LIST
    // path end to end.
    let meta = resolve(DUMMY);
    let dims: Vec<&str> = meta.dimensions.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(dims, vec!["x"]);
    assert_eq!(meta.dimensions[0].length, 10);

    let var_names: Vec<&str> = meta.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(
        var_names,
        vec!["v"],
        "x is a pure dimension, not a variable"
    );
    assert_eq!(meta.variables[0].dimensions, vec!["x"]);
    assert!(!meta.variables[0].is_coordinate);
}
