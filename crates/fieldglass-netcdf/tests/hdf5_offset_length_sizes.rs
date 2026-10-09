//! An earliest-format file whose Size of Offsets and Size of Lengths differ
//! lists, resolves and decodes (#922).
//!
//! libhdf5 writes a symbol-table entry's link-name offset and a group B-tree
//! key at Size of Lengths (`H5G_ent_decode`, `H5G_node_decode_key`). The
//! reader took both at Size of Offsets, so with sizes (8, 4) it read the root
//! group's header address four bytes early and failed the file.
//!
//! Fixtures from `tools/build_hdf5_size_fixtures.py`, sizes (8, 4) and (4, 8):
//! more members than one symbol-table node holds, a header continuation, an
//! unlimited dimension, dimension scales, a gzip-chunked dataset and a nested
//! group. The oracles hold what h5py reads back.

use std::collections::BTreeMap;

use fieldglass_netcdf::{
    ChildKind, Hdf5Attribute, NetcdfBacking, NetcdfReader, list_attributes, list_root_children,
};
use serde_json::Value;

const CASES: [(&[u8], &str); 2] = [
    (
        include_bytes!("fixtures/hdf5_sizes_o8_l4.h5"),
        include_str!("fixtures/hdf5_sizes_o8_l4.h5.oracle.json"),
    ),
    (
        include_bytes!("fixtures/hdf5_sizes_o4_l8.h5"),
        include_str!("fixtures/hdf5_sizes_o4_l8.h5.oracle.json"),
    ),
];

/// Attributes the dimension-scale machinery writes, which the reader filters
/// out of a variable's user attributes.
const MACHINERY: [&str; 4] = ["CLASS", "NAME", "DIMENSION_LIST", "REFERENCE_LIST"];

/// An attribute as the oracle records it: text for a string, the number for a
/// numeric scalar.
fn attribute_value(a: &Hdf5Attribute) -> Value {
    match a.first_value() {
        Some(v) => Value::from(v),
        None => Value::from(a.value.clone()),
    }
}

fn user_attributes(oracle: &Value, path: &str) -> BTreeMap<String, Value> {
    oracle["attributes"]
        .get(path)
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(k, _)| !MACHINERY.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn as_map(attrs: &[Hdf5Attribute]) -> BTreeMap<String, Value> {
    attrs
        .iter()
        .map(|a| (a.name.clone(), attribute_value(a)))
        .collect()
}

#[test]
fn mismatched_sizes_list_resolve_and_decode() {
    for (bytes, oracle) in CASES {
        let oracle: Value = serde_json::from_str(oracle).unwrap();
        let source = oracle["source"].as_str().unwrap();
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        let NetcdfBacking::Hdf5(probe) = &reader.backing else {
            panic!("{source}: expected HDF5");
        };
        // The fixture is what it claims: an earliest-format superblock with
        // the two sizes the builder asked for.
        assert_eq!(probe.superblock_version, 0, "{source}");
        assert_eq!(
            (u64::from(probe.offset_size), u64::from(probe.length_size)),
            (
                oracle["size_of_offsets"].as_u64().unwrap(),
                oracle["size_of_lengths"].as_u64().unwrap()
            ),
            "{source}"
        );
        assert!(
            oracle["many_attrs_header_chunks"].as_u64().unwrap() > 1,
            "{source}: many_attrs should need a header continuation"
        );

        let meta = reader
            .hdf5_metadata()
            .unwrap_or_else(|e| panic!("{source}: {e}"));
        let datasets = oracle["datasets"].as_object().unwrap();

        // Every dataset h5py reads is a variable or a dimension here, and
        // nothing else is.
        let mut names: Vec<&str> = meta.variables.iter().map(|v| v.name.as_str()).collect();
        names.sort_unstable();
        let mut want: Vec<&str> = datasets.keys().map(String::as_str).collect();
        want.sort_unstable();
        assert_eq!(names, want, "{source}");
        assert!(
            meta.unsupported.is_empty(),
            "{source}: {:?}",
            meta.unsupported
        );

        for var in &meta.variables {
            let d = &datasets[&var.name];
            let values: Vec<Option<f64>> = d["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64())
                .collect();
            let got = reader
                .decode_variable_raw(var.decode_index)
                .unwrap_or_else(|e| panic!("{source}: {}: {e}", var.name));
            assert_eq!(got, values, "{source}: {}", var.name);
            assert_eq!(
                as_map(&var.attributes),
                user_attributes(&oracle, &format!("/{}", var.name.trim_start_matches('/'))),
                "{source}: {} attributes",
                var.name
            );
        }

        // `v`'s dimensions resolve through its DIMENSION_LIST, whose
        // references sit in the global heap.
        let v = meta.variables.iter().find(|v| v.name == "v").unwrap();
        assert_eq!(v.dimensions, ["time", "x"], "{source}");
        let dims: BTreeMap<&str, (u64, bool)> = meta
            .dimensions
            .iter()
            .map(|d| (d.name.as_str(), (d.length, d.is_unlimited)))
            .collect();
        // `time` was created with `maxshape=(None,)`. Its maximum is stored as
        // all ones at Size of Lengths. h5py reads it back as unlimited when
        // that is 8 bytes and as 4294967295 when it is 4: libhdf5 compares
        // the decoded value with a 64-bit all-ones `H5S_UNLIMITED`. The reader
        // takes all ones at the stated width as unlimited either way.
        let time_max = datasets["time"]["maxshape"][0].as_i64().unwrap();
        let all_ones = (1i64 << (8 * probe.length_size.min(7))) - 1;
        assert!(
            time_max == -1 || time_max == all_ones,
            "{source}: {time_max}"
        );
        assert_eq!(dims.get("time"), Some(&(3, true)), "{source}");
        assert_eq!(dims.get("x"), Some(&(4, false)), "{source}");

        assert_eq!(
            as_map(&meta.global_attributes),
            user_attributes(&oracle, "/"),
            "{source}: root attributes"
        );
        let g = list_root_children(bytes, probe)
            .unwrap()
            .into_iter()
            .find(|c| c.name == "g")
            .expect("group g");
        assert_eq!(g.kind, ChildKind::Group, "{source}");
        let g_attrs = list_attributes(bytes, g.object_header_address, probe).unwrap();
        assert_eq!(as_map(&g_attrs), user_attributes(&oracle, "/g"), "{source}");

        // Pinned as well as read from the oracle, so a regenerated oracle
        // cannot quietly drop what the fixture is for.
        assert_eq!(meta.variables.len(), 16, "{source}");
        let many = meta
            .variables
            .iter()
            .find(|v| v.name == "many_attrs")
            .unwrap();
        assert_eq!(many.attributes.len(), 30, "{source}");
    }
}
