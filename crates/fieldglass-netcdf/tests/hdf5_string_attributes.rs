//! NetCDF-4 attributes stored as variable-length strings (`NC_STRING`, #1549).
//!
//! An `NC_STRING` attribute is an HDF5 variable-length string: the attribute
//! holds a global-heap reference per element and the text lives in a
//! global-heap collection. netCDF4-python writes one through
//! `setncattr_string`; h5netcdf and h5py write every `str` attribute that way.
//! They used to be dropped, so `calendar = "noleap"` read as absent (#1033)
//! and an h5py file lost its `units`.
//!
//! The fixtures are built by `tools/build_netcdf4_string_attribute_fixtures.py`,
//! and the oracle is every attribute as netCDF4-python reads it back.

use std::collections::BTreeMap;

use fieldglass_core::array::{Attribute, AttributeValue};
use fieldglass_netcdf::{NetcdfReader, list_attributes, root_group_address};
use serde_json::Value;

const NETCDF4: &[u8] = include_bytes!("fixtures/netcdf4_string_attributes.nc");
const H5PY: &[u8] = include_bytes!("fixtures/h5py_string_attributes.h5");
const ORACLE: &str = include_str!("fixtures/netcdf4_string_attributes.oracle.json");

const FIXTURES: [(&str, &[u8]); 2] = [
    ("netcdf4_string_attributes.nc", NETCDF4),
    ("h5py_string_attributes.h5", H5PY),
];

/// One string as `ncdump` quotes it in a netCDF-4 file: netCDF-C's
/// `pr_att_string` (ncdump/ncdump.c), transcribed from its `switch`.
fn ncdump_quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What an oracle attribute should read as. A string is text. A list of
/// strings is several values the attribute model has no variant for, so it is
/// kept as the elements `ncdump` prints for it, each quoted and joined by
/// `", "`, with no brackets.
fn expected(value: &Value) -> AttributeValue {
    match value {
        Value::String(text) => AttributeValue::Text(text.clone()),
        Value::Array(items) => AttributeValue::Opaque(
            items
                .iter()
                .map(|item| ncdump_quoted(item.as_str().expect("oracle list of strings")))
                .collect::<Vec<_>>()
                .join(", "),
        ),
        other => panic!("oracle holds only strings and lists of them: {other}"),
    }
}

/// A list whose strings hold a newline and a tab shows them escaped, as
/// `ncdump` prints them, not raw.
#[test]
fn control_characters_in_a_string_list_are_escaped() {
    let view = NetcdfReader::from_bytes(H5PY.to_vec())
        .expect("recognized NetCDF-4")
        .view()
        .expect("view resolves");
    let got = read(&view.global_attrs)["control_list"];
    assert_eq!(
        got,
        &AttributeValue::Opaque(r#""line1\nline2", "tab\there""#.into())
    );
}

fn read(attributes: &[Attribute]) -> BTreeMap<&str, &AttributeValue> {
    attributes
        .iter()
        .map(|a| (a.name.as_str(), &a.value))
        .collect()
}

fn want(oracle: &Value) -> BTreeMap<&str, AttributeValue> {
    oracle
        .as_object()
        .expect("oracle attributes are an object")
        .iter()
        .map(|(name, value)| (name.as_str(), expected(value)))
        .collect()
}

fn assert_same(file: &str, object: &str, got: &[Attribute], oracle: &Value) {
    let got: BTreeMap<&str, AttributeValue> = read(got)
        .into_iter()
        .map(|(name, value)| (name, value.clone()))
        .collect();
    assert_eq!(got, want(oracle), "{file}: attributes of {object}");
}

/// Every global and variable attribute, `NC_STRING` and `NC_CHAR` alike, reads
/// as netCDF4-python reads it, in both writers' layouts: netCDF-C's (one-element
/// arrays, dense storage on `dense`) and h5py's (scalar strings, and a
/// fixed-length string array beside a variable-length one).
#[test]
fn every_string_attribute_reads_as_netcdf4_python_reads_it() {
    let oracle: Value = serde_json::from_str(ORACLE).expect("oracle parses");
    for (file, bytes) in FIXTURES {
        let view = NetcdfReader::from_bytes(bytes.to_vec())
            .expect("recognized NetCDF-4")
            .view()
            .expect("view resolves");
        let expected = &oracle[file];
        assert_same(
            file,
            "the root group",
            &view.global_attrs,
            &expected["global"],
        );
        let variables = expected["variables"].as_object().unwrap();
        assert_eq!(
            view.vars.len(),
            variables.len(),
            "{file}: every variable is listed"
        );
        for var in &view.vars {
            assert_same(
                file,
                &var.array.name,
                &var.array.attributes,
                &variables[var.array.name.as_str()],
            );
        }
    }
}

/// The #1033 case: a `calendar` written as `NC_STRING` reaches the variable's
/// attributes as text, by both writers. Before #1549 it read as absent, so a
/// `noleap` axis was dated on the Gregorian calendar.
#[test]
fn an_nc_string_calendar_reaches_the_time_variable() {
    for (file, bytes) in FIXTURES {
        let view = NetcdfReader::from_bytes(bytes.to_vec())
            .unwrap()
            .view()
            .unwrap();
        let time = view
            .vars
            .iter()
            .find(|v| v.array.name == "time")
            .unwrap_or_else(|| panic!("{file}: no time variable"));
        assert_eq!(
            fieldglass_core::array::attribute(&time.array.attributes, "calendar")
                .and_then(AttributeValue::text),
            Some("noleap"),
            "{file}"
        );
    }
}

/// The public attribute listing returns the text too, beside the netCDF type
/// the attribute is presented as: `char`, the one text type `NcType` has.
#[test]
fn the_attribute_listing_returns_the_text() {
    let reader = NetcdfReader::from_bytes(NETCDF4.to_vec()).unwrap();
    let fieldglass_netcdf::NetcdfBacking::Hdf5(probe) = &reader.backing else {
        panic!("expected the HDF5 backing");
    };
    let root = root_group_address(NETCDF4, probe).unwrap();
    let title = list_attributes(NETCDF4, root, probe)
        .unwrap()
        .into_iter()
        .find(|a| a.name == "title")
        .expect("title is listed");
    assert_eq!(title.value, "fieldglass NC_STRING attribute fixture");
    assert_eq!(title.datatype.nc_type, fieldglass_netcdf::NcType::Char);
    assert!(title.values.is_empty());
}

/// A string-typed `missing_value` (netCDF4-python's issue 1152 file, the
/// bundled `netcdf4_hdf5_dummy.nc`) now reads as the text netCDF4-python
/// returns, `'-1.e+34f'`, and masks nothing: a sentinel is a number of the
/// variable's type, and text is not one.
#[test]
fn a_string_missing_value_reads_as_text_and_masks_nothing() {
    const DUMMY: &[u8] = include_bytes!("fixtures/netcdf4_hdf5_dummy.nc");
    let reader = NetcdfReader::from_bytes(DUMMY.to_vec()).unwrap();
    let view = reader.view().unwrap();
    let v = view
        .vars
        .iter()
        .find(|v| v.array.name == "v")
        .expect("the dummy's `v`");
    assert_eq!(
        fieldglass_core::array::attribute(&v.array.attributes, "missing_value"),
        Some(&AttributeValue::Text("-1.e+34f".into()))
    );
    assert_eq!(
        reader.decode_variable_physical(v.decode_index).unwrap(),
        reader.decode_variable_raw(v.decode_index).unwrap()
    );
}
