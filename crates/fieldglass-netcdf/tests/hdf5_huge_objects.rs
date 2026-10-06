//! Dense attribute and link storage holding a huge fractal-heap object (#899).
//!
//! An attribute or link message larger than its heap's maximum managed size
//! (4 KB) is stored outside the heap's blocks and found through the heap's
//! huge-object B-tree. The reader refused every non-managed object, which
//! failed all of an object's attributes, and the file with them: a NetCDF-4
//! file with ten global attributes and a long `history` did not open.
//!
//! Fixtures from `tools/build_netcdf4_huge_attribute_fixture.py`; provenance
//! in `tests/fixtures/NOTICE.md`; the oracles hold what netCDF4-python and
//! h5py read back.

use fieldglass_netcdf::{NetcdfBacking, NetcdfReader, list_attributes, list_root_children};
use serde_json::Value;
use std::collections::BTreeMap;

const ATTRS: &[u8] = include_bytes!("fixtures/netcdf4_huge_attributes.nc");
const ATTRS_ORACLE: &str = include_str!("fixtures/netcdf4_huge_attributes.nc.oracle.json");
const LINKS: &[u8] = include_bytes!("fixtures/hdf5_huge_link_name.h5");
const LINKS_ORACLE: &str = include_str!("fixtures/hdf5_huge_link_name.h5.oracle.json");
/// Dataset `v` with ten dense attributes, two of whose records name one huge
/// object.
const SHARED_HUGE: &[u8] = include_bytes!("fixtures/hdf5_shared_huge_attribute.h5");

fn probe(bytes: &[u8]) -> fieldglass_netcdf::Hdf5Probe {
    match NetcdfReader::from_bytes(bytes.to_vec()).unwrap().backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5, got {}", other.label()),
    }
}

/// Name → string value of every string attribute in an oracle object.
fn strings(oracle: &Value) -> BTreeMap<String, String> {
    oracle
        .as_object()
        .expect("an attribute object")
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                v.as_str().expect("a string attribute").to_string(),
            )
        })
        .collect()
}

fn attrs_at(bytes: &[u8], addr: u64) -> BTreeMap<String, String> {
    let p = probe(bytes);
    list_attributes(bytes, addr, &p)
        .expect("the attributes list")
        .into_iter()
        .map(|a| (a.name, a.value))
        .collect()
}

#[test]
fn a_huge_global_attribute_is_read() {
    let oracle: Value = serde_json::from_str(ATTRS_ORACLE).unwrap();
    let want = strings(&oracle["global_attributes"]);
    assert_eq!(
        want["history"].len(),
        5600,
        "the oracle's history is the huge one"
    );
    let p = probe(ATTRS);
    let root = fieldglass_netcdf::root_group_address(ATTRS, &p).unwrap();
    let got = attrs_at(ATTRS, root);
    for (name, value) in &want {
        assert_eq!(got.get(name), Some(value), "global attribute {name}");
    }
    // netCDF-C adds `_NCProperties`, which netCDF4-python hides; nothing else.
    let extra: Vec<&String> = got.keys().filter(|k| !want.contains_key(*k)).collect();
    assert!(
        extra.iter().all(|k| k.starts_with('_')),
        "unexpected attributes {extra:?}"
    );
}

#[test]
fn a_huge_variable_attribute_is_read_and_the_variable_decodes() {
    let oracle: Value = serde_json::from_str(ATTRS_ORACLE).unwrap();
    let want = strings(&oracle["variables"]["t"]["attributes"]);
    assert!(
        want["comment"].len() > 4096,
        "the oracle's comment is the huge one"
    );
    let p = probe(ATTRS);
    let t = list_root_children(ATTRS, &p)
        .unwrap()
        .into_iter()
        .find(|c| c.name == "t")
        .expect("variable t");
    let got = attrs_at(ATTRS, t.object_header_address);
    for (name, value) in &want {
        assert_eq!(got.get(name), Some(value), "attribute t:{name}");
    }

    // And the file opens: its metadata builds and `t` decodes.
    let reader = NetcdfReader::from_bytes(ATTRS.to_vec()).expect("the file opens");
    let index = reader
        .hdf5_metadata()
        .expect("the metadata builds")
        .variables
        .iter()
        .position(|v| v.name == "t")
        .expect("t is listed");
    assert_eq!(
        reader.decode_variable_raw(index).expect("t decodes"),
        vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0)]
    );
}

#[test]
fn a_huge_link_name_is_read() {
    let oracle: Value = serde_json::from_str(LINKS_ORACLE).unwrap();
    let want: Vec<String> = oracle["g_links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(want.iter().any(|n| n.len() == 5000));
    let p = probe(LINKS);
    let g = list_root_children(LINKS, &p)
        .unwrap()
        .into_iter()
        .find(|c| c.name == "g")
        .expect("group g");
    let mut got: Vec<String> =
        fieldglass_netcdf::hdf5::group::list_group_children(LINKS, g.object_header_address, &p)
            .expect("g's links list")
            .into_iter()
            .map(|c| c.name)
            .collect();
    got.sort();
    assert_eq!(got, want);
}

#[test]
fn two_records_naming_one_huge_object_read_it_once() {
    // Each record's object used to be read in full: 15,000 records naming one
    // 1 MB attribute made a 1.9 MB file use 34 GB. A listing now reads each
    // object once and refuses a second name for it.
    let source = fieldglass_core::testing::Recording::new(SHARED_HUGE);
    let p = probe(SHARED_HUGE);
    let v = list_root_children(SHARED_HUGE, &p)
        .unwrap()
        .into_iter()
        .find(|c| c.name == "v")
        .expect("dataset v");
    let err = list_attributes(&source, v.object_header_address, &p)
        .expect_err("two names for one huge object");
    assert!(err.to_string().contains("huge heap object"), "{err}");
    let big_reads = source.reads().iter().filter(|r| r.len >= 5000).count();
    assert_eq!(big_reads, 1, "the 5,000-byte object is read once");
}

const HUGE_ONLY: &[u8] = include_bytes!("fixtures/hdf5_huge_only_heaps.h5");
const HUGE_ONLY_ORACLE: &str = include_str!("fixtures/hdf5_huge_only_heaps.h5.oracle.json");

/// `(first character, length)` of each string an oracle object records.
fn shapes(oracle: &Value) -> BTreeMap<String, (char, usize)> {
    oracle
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let first = v[0].as_str().unwrap().chars().next().unwrap();
            (k.clone(), (first, v[1].as_u64().unwrap() as usize))
        })
        .collect()
}

#[test]
fn heaps_holding_only_huge_objects_are_read() {
    // libhdf5 gives a heap its root block on the first managed insert, so a
    // heap of only huge objects has none, and the reader refused it (#907).
    let oracle: Value = serde_json::from_str(HUGE_ONLY_ORACLE).unwrap();
    let p = probe(HUGE_ONLY);
    let children = list_root_children(HUGE_ONLY, &p).expect("the root lists");
    for name in ["one", "nine"] {
        let child = children.iter().find(|c| c.name == name).expect(name);
        let got: BTreeMap<String, (char, usize)> = attrs_at(HUGE_ONLY, child.object_header_address)
            .into_iter()
            .map(|(k, v)| (k, (v.chars().next().unwrap_or(' '), v.len())))
            .collect();
        assert_eq!(got, shapes(&oracle[name]), "dataset {name}");
    }
    let g = children.iter().find(|c| c.name == "g").expect("group g");
    let mut links: Vec<(char, usize)> =
        fieldglass_netcdf::hdf5::group::list_group_children(HUGE_ONLY, g.object_header_address, &p)
            .expect("g's links list")
            .into_iter()
            .map(|c| (c.name.chars().next().unwrap(), c.name.len()))
            .collect();
    links.sort();
    let want: Vec<(char, usize)> = oracle["g_links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v[0].as_str().unwrap().chars().next().unwrap(),
                v[1].as_u64().unwrap() as usize,
            )
        })
        .collect();
    assert_eq!(links, want);
}
