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
// checksum. `v` [4, 5] is on scales `y` and `x`; its `DIMENSION_LIST` holds
// two heap IDs of count(4), collection address(8) and object index(4), at
// 1776 and 1792, naming objects 1 (`y`) and 2 (`x`) of the collection at
// 2160. Each object's header is index(2), reference count(2), reserved(4),
// size(8). `c` [6, 8] has no `DIMENSION_LIST`, so it is on anonymous axes. A
// `DIMENSION_LIST` that cannot be read for certain is damage to its variable,
// not to the file (#1695): the variable is kept on anonymous axes, numbered
// after every other variable's, and the note beside it says why.
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

/// The note on a variable whose `DIMENSION_LIST` met `why`.
fn damaged_note(why: &str) -> String {
    format!(
        "attribute \"DIMENSION_LIST\" not read: the file's record of the variable's \
         dimensions is damaged ({why}), so its axes are unnamed"
    )
}

/// Each variable's dimensions in `meta`, by name.
fn dimensions_by_variable(meta: &Hdf5Metadata) -> BTreeMap<&str, Vec<&str>> {
    meta.variables
        .iter()
        .map(|v| {
            let dims = v.dimensions.iter().map(String::as_str).collect();
            (v.name.as_str(), dims)
        })
        .collect()
}

/// Every dimension in `meta`, with its length.
fn dimension_table(meta: &Hdf5Metadata) -> Vec<(&str, u64)> {
    meta.dimensions
        .iter()
        .map(|d| (d.name.as_str(), d.length))
        .collect()
}

/// That `damaged`, `whole` with `v`'s `DIMENSION_LIST` damaged so that it
/// meets `why`, opens with `v` on anonymous axes numbered after every other
/// variable's, its values unchanged, the note on it naming the damage, and
/// everything else as `whole` has it.
fn assert_falls_back(whole: &[u8], damaged: Vec<u8>, why: &str) {
    let whole = NetcdfReader::from_bytes(whole.to_vec()).unwrap();
    let (whole_meta, whole_unread) = whole.hdf5_metadata_with_unread().unwrap();
    assert!(whole_unread.is_empty(), "{whole_unread:?}");
    let reader = NetcdfReader::from_bytes(damaged).unwrap();
    let (meta, unread) = reader
        .hdf5_metadata_with_unread()
        .expect("a damaged DIMENSION_LIST does not fail the file");

    let mut want = dimensions_by_variable(&whole_meta);
    // The other variables' phony axes come first, so `v`'s take the next two.
    let next = whole_meta
        .dimensions
        .iter()
        .filter(|d| d.name.starts_with("phony_dim_"))
        .count();
    let (y, x) = (
        format!("phony_dim_{next}"),
        format!("phony_dim_{}", next + 1),
    );
    want.insert("v", vec![y.as_str(), x.as_str()]);
    assert_eq!(dimensions_by_variable(&meta), want);
    let mut table = dimension_table(&whole_meta);
    table.extend([(y.as_str(), 4), (x.as_str(), 5)]);
    assert_eq!(dimension_table(&meta), table);

    assert_eq!(unread.variable("v"), Some(damaged_note(why).as_str()));
    assert_eq!(unread.variables.len(), 1, "{unread:?}");
    assert_eq!(unread.global, None);

    let index = |meta: &Hdf5Metadata| {
        meta.variables
            .iter()
            .find(|v| v.name == "v")
            .unwrap()
            .decode_index
    };
    assert_eq!(
        reader.decode_variable_raw(index(&meta)).unwrap(),
        whole.decode_variable_raw(index(&whole_meta)).unwrap()
    );

    // The view opens on the same axes, with the same note.
    let (view, view_unread) = reader.view_with_unread().expect("the view opens");
    let v = view.vars.iter().find(|v| v.array.name == "v").unwrap();
    assert_eq!(v.array.dimensions, [y, x]);
    assert_eq!(view_unread, unread);
    reader.view().expect("the view opens");
}

/// A `DIMENSION_LIST` reference whose index two objects carry names neither
/// object for certain (HDF5 File Format Specification, Global Heap: "Each
/// object has a unique identification number within a collection"). `x`'s
/// reference, object 2, is renumbered 1, `y`'s, and `v`'s second axis names
/// object 1. Before #1557 the first object won and `v` read as `(y, y)`;
/// libhdf5 2.0.0 keeps the last object and reads it as `(x, x)`. This
/// diverges from both in the spec's favor: `v`'s axes are anonymous, and the
/// note says why. From #1557 until #1695 the file did not open.
#[test]
fn a_dimension_list_index_two_objects_carry_falls_back_to_phony_dimensions() {
    let file = earliest_patched(&[
        (2200, &2u16.to_le_bytes(), &1u16.to_le_bytes()),
        (1792 + 12, &2u32.to_le_bytes(), &1u32.to_le_bytes()),
    ]);
    assert_falls_back(
        EARLIEST,
        file,
        "global-heap collection holds object index 1 twice",
    );
}

/// An axis's heap ID states how many references its object holds, eight
/// bytes each; a count either side of the object's size is not read, as
/// libhdf5 2.0.0 refuses it ("Expected global heap object size does not
/// match"). Before #1557 the first reference was read whatever the count.
#[test]
fn a_dimension_list_count_that_differs_from_its_object_falls_back_to_phony_dimensions() {
    const WHY: &str = "DIMENSION_LIST axis length does not match its global-heap object";
    // `v`'s `y` axis claims two references in an 8-byte object.
    let longer = earliest_patched(&[(1776, &1u32.to_le_bytes(), &2u32.to_le_bytes())]);
    assert_falls_back(EARLIEST, longer, WHY);
    // `x`'s object, the last before the free space, grown to 16 bytes over
    // the free-space object's header, under an ID of one. libhdf5 2.0.0 had
    // not returned from reading this file's `DIMENSION_LIST` after 60 s.
    let shorter = earliest_patched(&[(2200 + 8, &8u64.to_le_bytes(), &16u64.to_le_bytes())]);
    assert_falls_back(EARLIEST, shorter, WHY);
}

/// A second collection inside the first one's free space, at 2416, holding
/// an object 2 that names `x` as the first collection's does, and `v`'s `x`
/// axis pointed at it. The two blocks overlap, so neither collection's
/// objects can be trusted to be the ones the writer put there; read alone,
/// the second would resolve `v` as `(y, x)`. Only `v`'s `DIMENSION_LIST`
/// names it, so only `v` falls back.
#[test]
fn a_dimension_list_in_an_overlapping_collection_falls_back_to_phony_dimensions() {
    const INNER: usize = 2416;
    let mut collection = b"GCOL\x01\0\0\0".to_vec();
    collection.extend_from_slice(&64u64.to_le_bytes());
    collection.extend_from_slice(&2u16.to_le_bytes()); // index
    collection.extend_from_slice(&[0; 6]); // reference count + reserved
    collection.extend_from_slice(&8u64.to_le_bytes()); // size
    collection.extend_from_slice(&0x1870u64.to_le_bytes()); // `x`'s header
    let free = vec![0; collection.len()];
    let file = earliest_patched(&[
        (INNER, &free, &collection),
        (
            1792 + 4,
            &2160u64.to_le_bytes(),
            &(INNER as u64).to_le_bytes(),
        ),
    ]);
    assert_falls_back(
        EARLIEST,
        file,
        "global-heap collection overlaps another collection",
    );
}

/// The anonymous axes of a variable whose list is damaged are numbered after
/// every other variable's, so the damage renumbers no other variable's axes.
/// `c` renamed `z`, which sorts after `v`: numbered in name order, `v` would
/// take `phony_dim_0` and `phony_dim_1` from it.
#[test]
fn a_damaged_dimension_list_renumbers_no_other_variables_axes() {
    // `c`'s name in the root group's local heap.
    let renamed = earliest_patched(&[(744, b"c\0", b"z\0")]);
    let whole = resolve(&renamed);
    assert_eq!(
        dimensions_by_variable(&whole).get("z"),
        Some(&vec!["phony_dim_0", "phony_dim_1"])
    );
    let mut damaged = renamed.clone();
    damaged[1776..1780].copy_from_slice(&2u32.to_le_bytes());
    assert_falls_back(
        &renamed,
        damaged,
        "DIMENSION_LIST axis length does not match its global-heap object",
    );
}
