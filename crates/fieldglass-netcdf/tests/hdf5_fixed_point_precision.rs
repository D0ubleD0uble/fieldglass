//! HDF5 integers that do not fill their container, and a float that is not
//! IEEE (issue #795).
//!
//! An HDF5 fixed-point datatype carries a bit offset and a bit precision (file
//! format spec IV.A.2.d, "Fixed-Point Property Description"): the value is the
//! `precision` bits starting `offset` bits up, the bits either side are
//! padding, and a signed value's sign bit is the precision's top bit. Reading
//! the whole container instead turned a 16-bit -20 into 65516.
//!
//! `hdf5_fixed_point_precision.h5` is built by
//! `tools/build_hdf5_fixtures.py` (`build_fixed_point_precision`) through
//! `h5py.h5t`, and its oracle holds what h5py (libhdf5) reads back. Every
//! numeric read goes through one helper, so this file checks each place that
//! reads one: dataset values, the Fill Value message they fall back to, the
//! `_FillValue` attribute that masks them, and attribute values.

use std::collections::BTreeMap;

use fieldglass_netcdf::{
    ChildKind, NetcdfBacking, NetcdfReader, list_attributes, list_root_children, root_group_address,
};
use serde_json::Value;

const FIXTURE: &[u8] = include_bytes!("fixtures/hdf5_fixed_point_precision.h5");
const ORACLE: &str = include_str!("fixtures/hdf5_fixed_point_precision.h5.oracle.json");

fn oracle() -> Value {
    serde_json::from_str(ORACLE).expect("oracle parses")
}

fn probe() -> fieldglass_netcdf::Hdf5Probe {
    match NetcdfReader::from_bytes(FIXTURE.to_vec()).unwrap().backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5, got {}", other.label()),
    }
}

/// Decode every root dataset through the reader, keyed by name.
fn decode_all() -> BTreeMap<String, Result<Vec<Option<f64>>, String>> {
    let reader = NetcdfReader::from_bytes(FIXTURE.to_vec()).expect("recognised HDF5");
    list_root_children(FIXTURE, &probe())
        .expect("list children")
        .into_iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .enumerate()
        .map(|(index, c)| {
            (
                c.name,
                reader.decode_variable_raw(index).map_err(|e| e.to_string()),
            )
        })
        .collect()
}

fn oracle_values(entry: &Value) -> Vec<f64> {
    entry["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect()
}

/// The acceptance criterion: every packed integer dataset decodes value for
/// value to what h5py reads back. The masked dataset is left to its own test.
#[test]
fn packed_integers_decode_like_h5py() {
    let oracle = oracle();
    let decoded = decode_all();
    let mut checked = 0;
    for (name, entry) in oracle["objects"].as_object().unwrap() {
        if entry["class"] != "fixed-point" || name == "i32_prec12_masked" {
            continue;
        }
        let got = decoded[name]
            .as_ref()
            .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
        let want: Vec<Option<f64>> = oracle_values(entry).into_iter().map(Some).collect();
        assert_eq!(got, &want, "{name}");
        checked += 1;
    }
    // Not vacuous: eight contiguous layouts plus the fill-value dataset.
    assert_eq!(checked, 9, "fixture changed");
}

/// Each case the fixture exists for is really in it, so the test above is not
/// passing against full-width types libhdf5 quietly substituted.
#[test]
fn the_fixture_holds_what_it_claims() {
    let oracle = oracle();
    let o = &oracle["objects"];
    let spec = |name: &str| {
        let e = &o[name];
        (
            e["size_bytes"].as_u64().unwrap(),
            e["bit_offset"].as_u64().unwrap(),
            e["bit_precision"].as_u64().unwrap(),
            e["byte_order"].as_str().unwrap().to_owned(),
            e["signed"].as_bool(),
        )
    };
    assert_eq!(
        spec("i32_prec16"),
        (4, 0, 16, "little-endian".into(), Some(true))
    );
    assert_eq!(
        spec("i32be_prec12_off4"),
        (4, 4, 12, "big-endian".into(), Some(true))
    );
    assert_eq!(
        spec("u32_prec12_off3"),
        (4, 3, 12, "little-endian".into(), Some(false))
    );
    assert_eq!(
        spec("u16be_prec10_off5"),
        (2, 5, 10, "big-endian".into(), Some(false))
    );
    assert_eq!(
        spec("i8_prec5_off2"),
        (1, 2, 5, "little-endian".into(), Some(true))
    );
    assert_eq!(
        spec("i64_prec40_off8"),
        (8, 8, 40, "little-endian".into(), Some(true))
    );
    assert_eq!(o["i32_prec12_off4_pad_ones"]["pad"][0], "one");
    // -20 at offset 4 with ones below and above: the padding is really there.
    assert_eq!(
        o["i32_prec12_off4_pad_ones"]["stored_hex"]
            .as_str()
            .unwrap()[..8],
        *"cffeffff"
    );
    assert!(
        oracle_values(&o["i32_prec16"]).contains(&-20.0),
        "the issue's own value"
    );
}

/// The issue's reproduction, spelled out: -20 in a 16-bit precision is -20,
/// not 65516.
#[test]
fn a_negative_16_bit_value_is_not_read_as_its_container() {
    let decoded = decode_all();
    let got = decoded["i32_prec16"].as_ref().unwrap();
    assert_eq!(
        &got[..4],
        &[Some(-20.0), Some(5.0), Some(-1.0), Some(300.0)]
    );
}

/// Unwritten chunks read as the Fill Value message's default, which libhdf5
/// stores in the same packed form (-7, 12 bits at offset 4, pad ones).
#[test]
fn the_fill_value_default_is_unpacked_too() {
    let decoded = decode_all();
    assert_eq!(
        decoded["i32_prec12_fill"].as_ref().unwrap(),
        &[-20.0, 5.0, -7.0, -7.0, -7.0, -7.0].map(Some).to_vec()
    );
}

/// A `_FillValue` attribute of the packed type masks the packed data: both
/// sides go through the same unpacking, so -1 matches -1.
#[test]
fn a_packed_fill_value_attribute_masks_packed_data() {
    let oracle = oracle();
    let entry = &oracle["objects"]["i32_prec12_masked"];
    assert_eq!(entry["fill_value_attribute"], -1);
    assert_eq!(oracle_values(entry), [-20.0, -1.0, 5.0]);
    let decoded = decode_all();
    assert_eq!(
        decoded["i32_prec12_masked"].as_ref().unwrap(),
        &[Some(-20.0), None, Some(5.0)]
    );
}

/// Attribute values are read by the same rule, for both the numbers and the
/// display text.
#[test]
fn packed_attributes_decode_like_h5py() {
    let oracle = oracle();
    let p = probe();
    let root = root_group_address(FIXTURE, &p).unwrap();
    let attrs = list_attributes(FIXTURE, root, &p).unwrap();
    for (name, want) in oracle["attributes"].as_object().unwrap() {
        let attr = attrs
            .iter()
            .find(|a| &a.name == name)
            .unwrap_or_else(|| panic!("{name} missing"));
        let want: Vec<f64> = want
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        assert_eq!(attr.values, want, "{name}");
    }
    let by_name: BTreeMap<&str, &str> = attrs
        .iter()
        .map(|a| (a.name.as_str(), a.value.as_str()))
        .collect();
    assert!(
        by_name["attr_i32_prec12_off4"].contains("-20"),
        "{:?}",
        by_name["attr_i32_prec12_off4"]
    );
    assert_eq!(by_name["attr_u32_prec12_off3"], "4095");
}

/// A 24-bit float in a 32-bit container is refused as unsupported, named in
/// the metadata report, rather than read as an IEEE `f32`. h5py reads it, so
/// the oracle records what a reader that applied the layout would return.
#[test]
fn a_non_ieee_float_is_refused_not_misread() {
    let oracle = oracle();
    let entry = &oracle["objects"]["f32_prec24"];
    assert_eq!(entry["bit_precision"], 24);
    assert_eq!(oracle_values(entry), [1.5, -2.25, 0.0]);

    let reader = NetcdfReader::from_bytes(FIXTURE.to_vec()).unwrap();
    let meta = reader.hdf5_metadata().expect("metadata resolves");
    assert!(
        meta.variables.iter().all(|v| v.name != "f32_prec24"),
        "a float this build cannot read must not be listed"
    );
    let skipped: Vec<(&str, &str)> = meta
        .unsupported
        .iter()
        .map(|u| (u.name.as_str(), u.reason.as_str()))
        .collect();
    assert_eq!(skipped.len(), 1, "{skipped:?}");
    assert_eq!(skipped[0].0, "f32_prec24");
    assert!(skipped[0].1.contains("non-IEEE"), "{}", skipped[0].1);

    let decoded = decode_all();
    let err = decoded["f32_prec24"].as_ref().unwrap_err();
    assert!(err.contains("non-IEEE"), "{err}");
}
