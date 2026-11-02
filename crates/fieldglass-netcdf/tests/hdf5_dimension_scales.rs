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
        .expect("recognized NetCDF-4")
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

/// The decode index recorded on each variable must point at the matching dataset
/// through `decode_variable_raw` / `variable_shape`, even though pure
/// dimensions shift it away from the variable's position in the name-sorted
/// `variables` list. In `netcdf4_dimscale.nc` the full dataset order is
/// `lat, lat_bnds, lon, nv(pure dim), temperature, time`, so `temperature` sits
/// at decode index 4 while it is only the 4th *variable* (index 3) — using the
/// list position instead would decode the `nv` placeholder.
#[test]
fn decode_index_round_trips_through_the_reader() {
    let reader = NetcdfReader::from_bytes(DIMSCALE.to_vec()).expect("recognized NetCDF-4");
    let meta = reader.hdf5_metadata().expect("dimension-scale resolution");

    let var = |name: &str| {
        meta.variables
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} variable"))
    };

    // The data variable: 2 × 3 × 4 (time × lat × lon), 24 elements.
    let temperature = var("temperature");
    assert_eq!(
        reader.variable_shape(temperature.decode_index).unwrap(),
        vec![2, 3, 4],
        "decode_index must resolve temperature's own dataspace"
    );
    assert_eq!(
        reader
            .decode_variable_raw(temperature.decode_index)
            .unwrap()
            .len(),
        24
    );

    // The coordinate variables resolve to their 1-D axes.
    assert_eq!(
        reader.variable_shape(var("lat").decode_index).unwrap(),
        vec![3]
    );
    assert_eq!(
        reader.variable_shape(var("lon").decode_index).unwrap(),
        vec![4]
    );
}

// Malformed global-heap data under `DIMENSION_LIST` (#1557), written into a
// copy of `hdf5_no_userblock_earliest.h5`, whose object headers carry no
// checksum. `v` is on scales `y` and `x`; its `DIMENSION_LIST` holds two heap
// IDs of count(4), collection address(8) and object index(4), at 1776 and
// 1792, naming objects 1 (`y`) and 2 (`x`) of the collection at 2160. Each
// object's header is index(2), reference count(2), reserved(4), size(8). A
// malformed `DIMENSION_LIST` reference refuses the listing, as one naming a
// missing object always has.
const EARLIEST: &[u8] = include_bytes!("fixtures/hdf5_no_userblock_earliest.h5");

/// `EARLIEST` with each `(at, was, bytes)` written in, after checking the
/// fixture holds `was` there.
fn earliest_patched(edits: &[(usize, &[u8], &[u8])]) -> Vec<u8> {
    let mut file = EARLIEST.to_vec();
    for &(at, was, bytes) in edits {
        assert_eq!(
            &file[at..at + was.len()],
            was,
            "the fixture's bytes at {at}"
        );
        file[at..at + bytes.len()].copy_from_slice(bytes);
    }
    file
}

/// The parse error resolving `file` fails with.
fn refusal(file: Vec<u8>) -> String {
    match NetcdfReader::from_bytes(file).unwrap().hdf5_metadata() {
        Ok(meta) => panic!(
            "resolved: {:?}",
            meta.variables
                .iter()
                .map(|v| (&v.name, &v.dimensions))
                .collect::<Vec<_>>()
        ),
        Err(fieldglass_core::FieldglassError::Parse(m)) => m,
        Err(other) => panic!("{other:?}"),
    }
}

fn v_dimensions(file: &[u8]) -> Vec<String> {
    resolve(file)
        .variables
        .into_iter()
        .find(|v| v.name == "v")
        .expect("v")
        .dimensions
}

/// A collection where two objects carry one index is refused (HDF5 File
/// Format Specification, Global Heap: "Each object has a unique
/// identification number within a collection"). `x`'s reference, object 2,
/// is renumbered 1, `y`'s, and `v`'s second axis names 1. Before #1557 the
/// first object won and `v` read as `(y, y)`. libhdf5 2.0.0 keeps the last
/// object and reads it as `(x, x)`; this diverges from it in the spec's
/// favor.
#[test]
fn a_dimension_list_collection_with_a_duplicate_index_is_refused() {
    assert_eq!(v_dimensions(EARLIEST), ["y", "x"]);
    let file = earliest_patched(&[
        (2200, &2u16.to_le_bytes(), &1u16.to_le_bytes()),
        (1792 + 12, &2u32.to_le_bytes(), &1u32.to_le_bytes()),
    ]);
    assert_eq!(
        refusal(file),
        "global-heap collection holds object index 1 twice"
    );
}

/// An axis's heap ID states how many references its object holds, eight
/// bytes each; a count either side of the object's size is refused, as
/// libhdf5 2.0.0 refuses it ("Expected global heap object size does not
/// match"). Before #1557 the first reference was read whatever the count.
#[test]
fn a_dimension_list_count_that_differs_from_its_object_is_refused() {
    const WHY: &str = "DIMENSION_LIST axis length does not match its global-heap object";
    // `v`'s `y` axis claims two references in an 8-byte object.
    let longer = earliest_patched(&[(1776, &1u32.to_le_bytes(), &2u32.to_le_bytes())]);
    assert_eq!(refusal(longer), WHY);
    // `x`'s object, the last before the free space, grown to 16 bytes over
    // the free-space object's header, under an ID of one. libhdf5 2.0.0 had
    // not returned from reading this file's `DIMENSION_LIST` after 60 s.
    let shorter = earliest_patched(&[(2200 + 8, &8u64.to_le_bytes(), &16u64.to_le_bytes())]);
    assert_eq!(refusal(shorter), WHY);
}
