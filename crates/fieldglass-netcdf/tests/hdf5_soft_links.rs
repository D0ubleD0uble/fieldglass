//! A group holding soft links lists its hard links in both file formats
//! (#914). An earliest-format symbol-table entry for a soft link has cache
//! type 2 and an undefined header address, and the reader dereferenced it,
//! failing the whole group; the link-message path already skipped soft links.
//!
//! Fixtures from `tools/build_hdf5_soft_link_fixture.py`: datasets `a`, `m`
//! and `z`, a soft link `s` to `a` and a dangling soft link `d`; the oracles
//! hold what h5py lists and which members are hard links. Sorted by name the
//! entries run `a`, `d`, `m`, `s`, `z`, so a reader that stops at the first
//! soft link instead of skipping it lists only `a` and fails here (#919).

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
        // Pinned as well as read from the oracle, so a regenerated oracle
        // cannot quietly drop the members past the soft links.
        assert_eq!(names, ["a", "m", "z"], "{}", oracle["source"]);
        // And every dataset decodes, including those listed after a soft link.
        let view = reader.view().expect("view");
        let mut decoded: Vec<(String, Vec<Option<f64>>)> = view
            .vars
            .iter()
            .map(|v| {
                (
                    v.name().to_string(),
                    reader.decode_variable_raw(v.decode_index).expect("decodes"),
                )
            })
            .collect();
        decoded.sort_by(|x, y| x.0.cmp(&y.0));
        let expected: Vec<(String, Vec<Option<f64>>)> = [("a", 0..3), ("m", 10..14), ("z", 20..25)]
            .into_iter()
            .map(|(n, r)| (n.to_string(), r.map(|v| Some(f64::from(v))).collect()))
            .collect();
        assert_eq!(decoded, expected, "{}", oracle["source"]);
    }
}
