//! A group holding soft links lists its hard links in both file formats
//! (#914). An earliest-format symbol-table entry for a soft link has cache
//! type 2 and an undefined header address, and the reader dereferenced it,
//! failing the whole group; the link-message path already skipped soft links.
//!
//! Fixtures from `tools/build_hdf5_soft_link_fixture.py`: a dataset `a`, a
//! soft link `s` to it and a dangling soft link `d`; the oracles hold what
//! h5py lists and which members are hard links.

use fieldglass_netcdf::{NetcdfBacking, NetcdfReader, list_root_children};
use serde_json::Value;

const CASES: [(&[u8], &str); 2] = [
    (
        include_bytes!("fixtures/hdf5_soft_links_earliest.h5"),
        include_str!("fixtures/hdf5_soft_links_earliest.h5.oracle.json"),
    ),
    (
        include_bytes!("fixtures/hdf5_soft_links_latest.h5"),
        include_str!("fixtures/hdf5_soft_links_latest.h5.oracle.json"),
    ),
];

#[test]
fn both_formats_list_the_hard_links_and_skip_soft_ones() {
    for (bytes, oracle) in CASES {
        let oracle: Value = serde_json::from_str(oracle).unwrap();
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        let NetcdfBacking::Hdf5(p) = &reader.backing else {
            panic!("expected HDF5");
        };
        let mut names: Vec<String> = list_root_children(bytes, p)
            .unwrap_or_else(|e| panic!("{}: {e}", oracle["source"]))
            .into_iter()
            .map(|c| c.name)
            .collect();
        names.sort();
        let hard: Vec<String> = oracle["hard_links"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, hard, "{}", oracle["source"]);
        // And the one dataset decodes.
        assert_eq!(
            reader.decode_variable_raw(0).expect("a decodes"),
            vec![Some(0.0), Some(1.0), Some(2.0)]
        );
    }
}
