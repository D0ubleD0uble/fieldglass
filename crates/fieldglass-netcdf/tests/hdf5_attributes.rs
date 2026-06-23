//! Decodes HDF5 attributes for both bundled fixtures (issue #40) and pins the
//! result against the committed oracle / `ncdump -h`.
//!
//! Covers global (root-group) and per-dataset attributes, inline message
//! versions 1 (v1 fixture) and 3 (v2 fixture), and dense fractal-heap storage
//! (the v2 fixture's `dense_attrs` dataset).

use fieldglass_netcdf::{NetcdfBacking, NetcdfReader, list_attributes, list_root_children};
use std::collections::BTreeMap;

const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
const V2_LINKINFO: &[u8] = include_bytes!("fixtures/hdf5_v2_linkinfo.h5");
const BTREEV2_MULTILEVEL: &[u8] = include_bytes!("fixtures/hdf5_btreev2_multilevel.h5");
const CHILD_INDIRECT: &[u8] = include_bytes!("fixtures/hdf5_child_indirect.h5");

fn probe(bytes: &[u8]) -> fieldglass_netcdf::Hdf5Probe {
    match NetcdfReader::from_bytes(bytes.to_vec()).unwrap().backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5, got {}", other.label()),
    }
}

/// Attributes (name → display value) of the root group.
fn global_attrs(bytes: &[u8]) -> BTreeMap<String, String> {
    let p = probe(bytes);
    let root = fieldglass_netcdf::root_group_address(bytes, &p).unwrap();
    list_attributes(bytes, root, &p)
        .unwrap()
        .into_iter()
        .map(|a| (a.name, a.value))
        .collect()
}

/// Attributes of a named child dataset.
fn dataset_attrs(bytes: &[u8], name: &str) -> BTreeMap<String, String> {
    let p = probe(bytes);
    let child = list_root_children(bytes, &p)
        .unwrap()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no dataset {name}"));
    list_attributes(bytes, child.object_header_address, &p)
        .unwrap()
        .into_iter()
        .map(|a| (a.name, a.value))
        .collect()
}

fn expect(map: &BTreeMap<String, String>, pairs: &[(&str, &str)]) {
    let got: BTreeMap<&str, &str> = map.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let want: BTreeMap<&str, &str> = pairs.iter().copied().collect();
    assert_eq!(got, want);
}

/// Global attributes are identical in both fixtures (scale/title/version).
#[test]
fn global_attributes_match_oracle() {
    for bytes in [V1_SYMBOLTABLE, V2_LINKINFO] {
        expect(
            &global_attrs(bytes),
            &[
                ("scale", "0.25"),
                ("title", "fieldglass HDF5 fixture"),
                ("version", "5"),
            ],
        );
    }
}

/// `temp_f64` carries a string and two numeric attributes in both fixtures.
#[test]
fn per_dataset_attributes_match_oracle() {
    for bytes in [V1_SYMBOLTABLE, V2_LINKINFO] {
        expect(
            &dataset_attrs(bytes, "temp_f64"),
            &[("units", "meters"), ("valid_max", "1"), ("valid_min", "0")],
        );
    }
}

/// The v2 fixture's `dense_attrs` dataset stores 12 attributes in a fractal
/// heap — exercising the dense (Attribute Info → B-tree v2) path.
#[test]
fn dense_attributes_decode() {
    let attrs = dataset_attrs(V2_LINKINFO, "dense_attrs");
    assert_eq!(attrs.len(), 12);
    for i in 0..12 {
        assert_eq!(attrs[&format!("attr_{i:02}")], i.to_string());
    }
}

/// Datasets without attributes return an empty list, not an error.
#[test]
fn dataset_without_attributes_is_empty() {
    assert!(dataset_attrs(V1_SYMBOLTABLE, "scalar_i32").is_empty());
}

/// `many_attrs` carries 700 dense attributes, enough to push the attribute
/// name-index B-tree v2 to depth 2 (per the oracle). Reading every one back
/// proves the reader descends internal nodes instead of erroring — the gap that
/// blocked metadata-heavy operational NetCDF-4 files (#33). Each `a{i:04}` maps
/// to `int32 i`, so the whole set is checked against a generated expectation.
#[test]
fn multilevel_btree_v2_attributes_decode() {
    let attrs = dataset_attrs(BTREEV2_MULTILEVEL, "many_attrs");
    let want: BTreeMap<String, String> = (0..700)
        .map(|i| (format!("a{i:04}"), i.to_string()))
        .collect();
    assert_eq!(attrs, want);
}

/// 512 kilobyte-sized dense attributes overflow the attribute fractal heap's
/// direct-block rows into a *child indirect block* (doubling-table rows beyond
/// `max_direct_block_size`). Enumerating every attribute name therefore has to
/// dereference heap IDs that resolve through the child indirect block — the path
/// `heap.rs` previously refused. This is the real-libhdf5 backstop for the
/// metadata-heaviest corpus files (#123); the hand-built `heap.rs` unit tests
/// pin the byte layout. Each `a{i:04}` is `int32[256]` with `value[k] = i + k`.
#[test]
fn child_indirect_fractal_heap_attributes_decode() {
    let attrs = dataset_attrs(CHILD_INDIRECT, "many_attrs");
    // Every attribute name resolves — including the ones whose records live in
    // the child indirect block.
    let want_names: std::collections::BTreeSet<String> =
        (0..512).map(|i| format!("a{i:04}")).collect();
    let got_names: std::collections::BTreeSet<String> = attrs.keys().cloned().collect();
    assert_eq!(got_names, want_names, "all 512 dense attributes enumerate");

    // Each attribute carries a *distinct* array value (`value[k] = i + k`), so
    // these assertions would catch a heap object resolved to the wrong block —
    // not just a missing name. Sample across the direct rows and the child
    // indirect block.
    let want_value = |i: i64| {
        (0..256)
            .map(|k| (i + k).to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    for i in [0i64, 255, 256, 511] {
        assert_eq!(
            attrs[&format!("a{i:04}")],
            want_value(i),
            "value of a{i:04}"
        );
    }
}
