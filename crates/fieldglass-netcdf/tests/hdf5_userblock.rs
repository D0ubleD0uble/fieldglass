//! An HDF5 file that begins with a userblock opens, lists and decodes the same
//! as its twin without one (#936).
//!
//! A userblock puts the superblock at 512, 1024, … and every address in the
//! file is relative to it. The reader checked the signature at byte 0 only, and
//! past that used each address as a file offset.
//!
//! Fixtures from `tools/build_hdf5_userblock_fixtures.py`: the same root (a
//! contiguous `v` with two attributes on the dimension scales `y` and `x`, a
//! chunked and deflated `c`) written by
//! h5py with `userblock_size=512` and without, in the earliest and the latest
//! format. The oracles hold what h5py reads back.

use fieldglass_core::{Format, detect_from_bytes};
use fieldglass_netcdf::{
    FieldglassError, NetcdfBacking, NetcdfReader, list_root_children, root_group_address,
};
use serde_json::Value;

struct Case {
    userblock: &'static [u8],
    userblock_oracle: &'static str,
    twin: &'static [u8],
    twin_oracle: &'static str,
}

const CASES: [Case; 2] = [
    Case {
        userblock: include_bytes!("fixtures/hdf5_userblock_earliest.h5"),
        userblock_oracle: include_str!("fixtures/hdf5_userblock_earliest.h5.oracle.json"),
        twin: include_bytes!("fixtures/hdf5_no_userblock_earliest.h5"),
        twin_oracle: include_str!("fixtures/hdf5_no_userblock_earliest.h5.oracle.json"),
    },
    Case {
        userblock: include_bytes!("fixtures/hdf5_userblock_latest.h5"),
        userblock_oracle: include_str!("fixtures/hdf5_userblock_latest.h5.oracle.json"),
        twin: include_bytes!("fixtures/hdf5_no_userblock_latest.h5"),
        twin_oracle: include_str!("fixtures/hdf5_no_userblock_latest.h5.oracle.json"),
    },
];

/// Everything a host sees of one file: each variable's name, dimensions,
/// attributes and decoded values, sorted by name.
type Contents = Vec<(String, Vec<String>, String, Vec<Option<f64>>)>;

fn contents(reader: &NetcdfReader) -> Contents {
    let view = reader.view().expect("view");
    let mut out: Vec<_> = view
        .vars
        .iter()
        .map(|v| {
            (
                v.name().to_string(),
                v.array.dimensions.clone(),
                format!("{:?}", v.array.attributes),
                reader.decode_variable_raw(v.decode_index).expect("decodes"),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A JSON array of strings.
fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn base_address(reader: &NetcdfReader) -> u64 {
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        panic!("expected HDF5");
    };
    probe.base_address
}

#[test]
fn a_file_with_a_userblock_reads_as_its_twin_without_one() {
    for case in CASES {
        let oracle: Value = serde_json::from_str(case.userblock_oracle).unwrap();
        let twin_oracle: Value = serde_json::from_str(case.twin_oracle).unwrap();
        let what = oracle["source"].as_str().unwrap();
        // The fixtures are what they say: the signature after 512 bytes, and
        // the superblock's stored base address agreeing with it.
        assert_eq!(oracle["signature_offset"], 512, "{what}");
        assert_eq!(oracle["stored_base_address"], 512, "{what}");
        assert_eq!(twin_oracle["signature_offset"], 0, "{what}");

        let reader = NetcdfReader::from_bytes(case.userblock.to_vec())
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let twin = NetcdfReader::from_bytes(case.twin.to_vec()).expect("twin opens");
        assert_eq!(base_address(&reader), 512, "{what}");
        assert_eq!(base_address(&twin), 0, "{what}");

        let got = contents(&reader);
        assert_eq!(got, contents(&twin), "{what}");

        // And against h5py, not only against the twin.
        let names: Vec<&str> = got.iter().map(|(n, ..)| n.as_str()).collect();
        assert_eq!(names, strings(&oracle["members"]), "{what}");
        assert_eq!(names, ["c", "v", "x", "y"], "{what}");
        for (name, dimensions, attributes, values) in &got {
            let expected: Vec<Option<f64>> = oracle["values"][name]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64())
                .collect();
            assert_eq!(values, &expected, "{what}: {name}");
            if name == "v" {
                // The dimension names come from `DIMENSION_LIST`, references
                // read from the global heap at an address the file states.
                assert_eq!(dimensions, &strings(&oracle["v_dims"]), "{what}");
                assert_eq!(dimensions, &["y", "x"], "{what}");
                let units = oracle["v_units"].as_str().unwrap();
                let scale = oracle["v_scale"].as_f64().unwrap();
                assert_eq!(
                    attributes,
                    &format!(
                        "[Attribute {{ name: \"scale\", value: Numbers([{scale:?}]) }}, \
                         Attribute {{ name: \"units\", value: Text(\"{units}\") }}]"
                    ),
                    "{what}"
                );
            }
        }

        // A region of the chunked variable reads only the chunks it overlaps,
        // following the chunk index's addresses.
        let view = reader.view().unwrap();
        let c = view.vars.iter().find(|v| v.name() == "c").unwrap();
        let region = reader
            .decode_region_raw(c.decode_index, &[2..5, 3..7])
            .expect("region decodes");
        let expected: Vec<Option<f64>> = (2..5)
            .flat_map(|r| (3..7).map(move |col| Some(f64::from(r * 8 + col - 20))))
            .collect();
        assert_eq!(region, expected, "{what}");
    }
}

#[test]
fn format_detection_finds_the_signature_after_a_userblock() {
    for case in CASES {
        assert_eq!(detect_from_bytes(case.userblock), Format::NetCdf);
        // From the prefix a host is told to pass, as well as the whole file.
        let window = &case.userblock[..fieldglass_core::DETECT_WINDOW.min(case.userblock.len())];
        assert_eq!(detect_from_bytes(window), Format::NetCdf);
    }
}

#[test]
fn a_signature_past_the_last_offset_searched_is_not_hdf5() {
    // A 32 KiB userblock: the next offset after the last one searched.
    let mut bytes = vec![0u8; 32768];
    bytes.extend_from_slice(CASES[0].twin);
    assert_eq!(detect_from_bytes(&bytes), Format::Unknown);
    let err = NetcdfReader::from_bytes(bytes).unwrap_err();
    assert!(
        matches!(err, FieldglassError::InvalidMagic { .. }),
        "unexpected error: {err}"
    );
}

#[test]
fn the_low_level_walk_takes_the_file_as_the_probe_addresses_it() {
    for case in CASES {
        let reader = NetcdfReader::from_bytes(case.userblock.to_vec()).unwrap();
        let NetcdfBacking::Hdf5(probe) = &reader.backing else {
            panic!("expected HDF5");
        };
        // Unwrapped, the superblock is not at 0: an error naming the fix, not
        // a walk of the userblock's bytes.
        let err = root_group_address(case.userblock, probe).unwrap_err();
        assert!(
            err.to_string().contains("Hdf5Probe::addressed"),
            "unexpected error: {err}"
        );
        let file = probe.addressed(case.userblock);
        let mut names: Vec<String> = list_root_children(&file, probe)
            .expect("lists")
            .into_iter()
            .map(|c| c.name)
            .collect();
        names.sort();
        assert_eq!(names, ["c", "v", "x", "y"]);
    }
}

#[test]
fn the_traversal_memo_holds_across_calls_on_a_file_with_a_userblock() {
    // The addressed view has an identity of its own, so the memo binds to it
    // and a second pass walks nothing again. A view that declined an identity
    // would still read correctly, but re-walk the file on every call.
    for case in CASES {
        let reader = NetcdfReader::from_bytes(case.userblock.to_vec()).unwrap();
        let NetcdfBacking::Hdf5(probe) = &reader.backing else {
            panic!("expected HDF5");
        };
        let first = contents(&reader);
        let walked = probe.traversals();
        assert!(walked > 0);
        assert_eq!(contents(&reader), first);
        assert_eq!(probe.traversals(), walked, "the second pass re-walked");
    }
}
