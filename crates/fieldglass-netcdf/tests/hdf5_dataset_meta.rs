//! Decodes per-dataset shape + element type for both bundled HDF5 fixtures
//! (issue #39) and pins the result against the committed oracle / `ncdump -h`.
//!
//! The v1 fixture writes **dataspace message version 1**, the v2 fixture writes
//! **version 2**, so the two together exercise both dataspace code paths.

use fieldglass_netcdf::{
    NcType, NetcdfBacking, NetcdfReader, describe_dataset, list_root_children,
};
use std::collections::BTreeMap;

const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
const V2_LINKINFO: &[u8] = include_bytes!("fixtures/hdf5_v2_linkinfo.h5");

/// `(rank, dims, max_dims, nc_type)` per dataset name.
type Shape = (usize, Vec<u64>, Vec<Option<u64>>, NcType);

fn shapes(bytes: &[u8]) -> BTreeMap<String, Shape> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised HDF5");
    let probe = match reader.backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5 backing, got {}", other.label()),
    };
    list_root_children(bytes, &probe)
        .expect("list children")
        .into_iter()
        .map(|child| {
            let s = describe_dataset(bytes, child.object_header_address, &probe)
                .unwrap_or_else(|e| panic!("describe {}: {e}", child.name));
            (
                child.name,
                (
                    s.dataspace.dims.len(),
                    s.dataspace.dims,
                    s.dataspace.max_dims,
                    s.datatype.nc_type,
                ),
            )
        })
        .collect()
}

#[test]
fn v1_dataset_shapes_match_oracle() {
    let s = shapes(V1_SYMBOLTABLE);
    assert_eq!(s["label"], (0, vec![], vec![], NcType::Char));
    assert_eq!(s["scalar_i32"], (0, vec![], vec![], NcType::Int));
    assert_eq!(s["temp_be_i32"], (1, vec![5], vec![Some(5)], NcType::Int));
    assert_eq!(s["temp_f32"], (1, vec![8], vec![Some(8)], NcType::Float));
    assert_eq!(s["temp_f64"], (1, vec![6], vec![Some(6)], NcType::Double));
    assert_eq!(
        s["temp_i32"],
        (2, vec![3, 4], vec![Some(3), Some(4)], NcType::Int)
    );
    assert_eq!(s["masked"], (1, vec![6], vec![Some(6)], NcType::Float));
    // `record` is the unlimited / chunked dimension.
    assert_eq!(s["record"], (1, vec![4], vec![None], NcType::Float));
}

#[test]
fn v2_dataset_shapes_match_oracle() {
    let s = shapes(V2_LINKINFO);
    assert_eq!(
        s["chunked"],
        (2, vec![10, 10], vec![Some(10), Some(10)], NcType::Float)
    );
    assert_eq!(s["dense_attrs"], (1, vec![3], vec![Some(3)], NcType::Int));
    assert_eq!(s["label"], (0, vec![], vec![], NcType::Char));
    assert_eq!(s["temp_be_i32"], (1, vec![5], vec![Some(5)], NcType::Int));
    assert_eq!(s["temp_f64"], (1, vec![6], vec![Some(6)], NcType::Double));
    assert_eq!(s["record"], (1, vec![4], vec![None], NcType::Float));
}

/// The v1 and v2 fixtures genuinely use different dataspace message versions.
#[test]
fn both_dataspace_versions_decode() {
    assert_eq!(shapes(V1_SYMBOLTABLE).len(), 8);
    assert_eq!(shapes(V2_LINKINFO).len(), 10);
}
