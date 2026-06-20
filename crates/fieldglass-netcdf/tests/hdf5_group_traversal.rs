//! Enumerates the root group's children for both bundled HDF5 fixtures (issue
//! #38) and pins the result against `h5dump -n`.
//!
//! The v1 fixture (`libver='earliest'`) exercises the legacy **symbol-table**
//! path (local heap + B-tree v1 + `SNOD`); the v2 fixture (`libver='v110'`)
//! exercises the modern **dense link** path (fractal heap + B-tree v2).

use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader};

const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
const V2_LINKINFO: &[u8] = include_bytes!("fixtures/hdf5_v2_linkinfo.h5");

fn child_names(bytes: &[u8]) -> Vec<(String, ChildKind, u64)> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised HDF5");
    let probe = match reader.backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5 backing, got {}", other.label()),
    };
    fieldglass_netcdf::list_root_children(bytes, &probe)
        .expect("list root children")
        .into_iter()
        .map(|c| (c.name, c.kind, c.object_header_address))
        .collect()
}

/// v1 symbol-table layout: 9 datasets, matching `h5dump -n`.
#[test]
fn v1_root_children() {
    let children = child_names(V1_SYMBOLTABLE);
    let names: Vec<&str> = children.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "compressed",
            "label",
            "masked",
            "record",
            "scalar_i32",
            "temp_be_i32",
            "temp_f32",
            "temp_f64",
            "temp_i32",
        ],
    );
    assert!(children.iter().all(|c| c.1 == ChildKind::Dataset));
}

/// v2 dense-link layout: 10 datasets, matching `h5dump -n`.
#[test]
fn v2_root_children() {
    let children = child_names(V2_LINKINFO);
    let names: Vec<&str> = children.iter().map(|c| c.0.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "chunked",
            "dense_attrs",
            "label",
            "masked",
            "record",
            "scalar_i32",
            "temp_be_i32",
            "temp_f32",
            "temp_f64",
            "temp_i32",
        ],
    );
    assert!(children.iter().all(|c| c.1 == ChildKind::Dataset));
}

/// Every child address points at a walkable object header.
#[test]
fn child_addresses_are_real_object_headers() {
    for bytes in [V1_SYMBOLTABLE, V2_LINKINFO] {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).unwrap();
        let probe = match reader.backing {
            NetcdfBacking::Hdf5(p) => p,
            _ => unreachable!(),
        };
        for (name, _, addr) in child_names(bytes) {
            fieldglass_netcdf::hdf5::object_header::walk(
                bytes,
                addr,
                probe.offset_size,
                probe.length_size,
            )
            .unwrap_or_else(|e| panic!("child {name} @ {addr:#x} should be a real header: {e}"));
        }
    }
}
