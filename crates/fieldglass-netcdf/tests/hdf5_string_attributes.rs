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
use fieldglass_netcdf::{AttributesNotRead, NetcdfReader, list_attributes, root_group_address};
use serde_json::Value;

const NETCDF4: &[u8] = include_bytes!("fixtures/netcdf4_string_attributes.nc");
const H5PY: &[u8] = include_bytes!("fixtures/h5py_string_attributes.h5");
const ORACLE: &str = include_str!("fixtures/netcdf4_string_attributes.oracle.json");
/// Built by `tools/build_netcdf4_string_list_fixture.py` (#1570).
const LISTS: &[u8] = include_bytes!("fixtures/netcdf4_string_lists.nc");
/// Every attribute of `LISTS` and of `NETCDF4`, with the type netCDF-C's
/// `nc_inq_atttype` reports for it and the value netCDF4-python reads.
const LISTS_ORACLE: &str = include_str!("fixtures/netcdf4_string_lists.oracle.json");

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
/// the attribute is presented as: `char`, the one text type `NcType` has, and
/// the type netCDF-C reports for it, `string` (#1570).
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
    assert_eq!(title.type_name(), "string");
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

// Malformed global-heap data under string attributes (#1557), written into a
// copy of the h5py fixture. Its one collection is at 2048; each object's
// header is index(2), reference count(2), reserved(4), size(8), and each
// attribute element is a heap ID of length(4), collection address(8) and
// object index(4).

/// Overwrite `at` in `file` with `bytes`, after checking it holds `was`.
fn patch(file: &mut [u8], at: usize, was: &[u8], bytes: &[u8]) {
    assert_eq!(
        &file[at..at + was.len()],
        was,
        "the fixture's bytes at {at}"
    );
    file[at..at + bytes.len()].copy_from_slice(bytes);
}

/// A heap ID into the collection: `len` bytes of object `index`.
fn heap_id(len: u32, index: u32) -> Vec<u8> {
    let mut id = len.to_le_bytes().to_vec();
    id.extend_from_slice(&2048u64.to_le_bytes());
    id.extend_from_slice(&index.to_le_bytes());
    id
}

/// Every attribute of the root group and each variable, by object and name.
fn every_attribute(bytes: &[u8]) -> BTreeMap<(String, String), AttributeValue> {
    let view = NetcdfReader::from_bytes(bytes.to_vec())
        .expect("recognized NetCDF-4")
        .view()
        .expect("the view resolves");
    let mut all = BTreeMap::new();
    let objects = std::iter::once(("/".to_string(), &view.global_attrs)).chain(
        view.vars
            .iter()
            .map(|v| (v.array.name.clone(), &v.array.attributes)),
    );
    for (object, attributes) in objects {
        for a in attributes {
            all.insert((object.clone(), a.name.clone()), a.value.clone());
        }
    }
    all
}

fn key(object: &str, name: &str) -> (String, String) {
    (object.into(), name.into())
}

/// `attributes` less each `(object, name)` in `lost`, `/` for a global one.
fn without(
    mut attributes: BTreeMap<(String, String), AttributeValue>,
    lost: &[(&str, &str)],
) -> BTreeMap<(String, String), AttributeValue> {
    attributes.retain(|(o, name), _| !lost.contains(&(o.as_str(), name.as_str())));
    attributes
}

/// Each variable the view leaves out, and why.
fn left_out(bytes: &[u8]) -> Vec<(String, String)> {
    NetcdfReader::from_bytes(bytes.to_vec())
        .expect("recognized NetCDF-4")
        .view()
        .expect("the view resolves")
        .unsupported
        .into_iter()
        .map(|v| (v.name, v.reason))
        .collect()
}

/// What the view reports not read on `bytes`.
fn not_read(bytes: &[u8]) -> AttributesNotRead {
    NetcdfReader::from_bytes(bytes.to_vec())
        .expect("recognized NetCDF-4")
        .view_with_unread()
        .expect("the view resolves")
        .1
}

/// What the view says of the attributes `names` when their global-heap
/// references are damaged: for the file's reader, without the parse error
/// that says where in the bytes (#1604).
fn damaged(names: &str) -> String {
    format!("{names} not read: the file's record of the text is damaged")
}

/// Two objects carrying one index leave a heap ID naming that index
/// ambiguous (HDF5 File Format Specification, Global Heap: "Each object has
/// a unique identification number within a collection"), so every attribute
/// that names it is left out, while the rest of the file, and every other
/// string in the same collection, reads. Here `x` (object 4) is renumbered 9,
/// the index of `K`, and `vlen_list`'s `x` element names 9. Before #1557 the
/// first object won and `units` read as `x`. libhdf5 2.0.0 keeps the last
/// object instead, and reads `vlen_list` as `["K", "yz"]`. Both misread;
/// this diverges from libhdf5 in the spec's favor.
///
/// `temperature`, whose `units` is lost, stays listed without it, as 0.6.0
/// promised for anything but a datatype outside the decoded subset (#1681),
/// and the report beside the view says it lacks `units` and why (#1569,
/// #1604). It still reads as unitless, as when `units` was dropped quietly
/// before #1569, but now the note says why. The globals left out are named
/// there too.
#[test]
fn a_duplicate_object_index_skips_the_attributes_that_name_it() {
    let well_formed = every_attribute(H5PY);
    assert_eq!(
        well_formed[&key("temperature", "units")],
        AttributeValue::Text("K".into())
    );

    let mut file = H5PY.to_vec();
    patch(&mut file, 2168, &4u16.to_le_bytes(), &9u16.to_le_bytes());
    patch(&mut file, 2312, &9u16.to_le_bytes(), &9u16.to_le_bytes());
    patch(&mut file, 1072, &heap_id(1, 4), &heap_id(1, 9));
    assert_eq!(
        every_attribute(&file),
        without(well_formed, &[("temperature", "units"), ("/", "vlen_list")]),
        "only the attributes naming 9 are lost"
    );
    assert_eq!(left_out(&file), [], "no variable is left out");
    let unread = not_read(&file);
    assert_eq!(
        unread.variable("temperature"),
        Some(damaged("attribute \"units\"").as_str())
    );
    assert_eq!(unread.variables.len(), 1, "{unread:?}");
    assert_eq!(
        unread.global.as_deref(),
        Some(damaged("attribute \"vlen_list\"").as_str())
    );
}

/// A repeated index among the strings of a netCDF-C file, whose one
/// collection also holds every `DIMENSION_LIST` reference (objects 29 to 32),
/// loses the strings that name it and leaves the dimensions resolving, as
/// before #1557 (the #1561 review's case). `dense`'s `note_01` (object 15)
/// is renumbered 14, `note_00`'s index: `note_00` is now ambiguous, and
/// `note_01` names an index no object carries. `dense` is kept without
/// them, as every variable that loses an attribute is (#1569, #1681).
#[test]
fn a_duplicate_string_index_leaves_the_dimensions_resolving() {
    let mut want = every_attribute(NETCDF4);
    want.retain(|(object, name), _| {
        !(object == "dense" && ["note_00", "note_01"].contains(&name.as_str()))
    });
    let mut file = NETCDF4.to_vec();
    patch(&mut file, 2480, &15u16.to_le_bytes(), &14u16.to_le_bytes());
    assert_eq!(every_attribute(&file), want);
    assert_eq!(left_out(&file), []);
    let dimensions = |bytes: &[u8]| {
        NetcdfReader::from_bytes(bytes.to_vec())
            .unwrap()
            .view()
            .unwrap()
            .vars
            .iter()
            .map(|v| (v.array.name.clone(), v.array.dimensions.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(dimensions(&file), dimensions(NETCDF4));
    assert_eq!(
        not_read(&file).variable("dense"),
        Some(damaged("attributes \"note_00\", \"note_01\"").as_str()),
        "the kept variable names what it lacks, without the parse error (#1604)"
    );
}

/// A string's heap ID states its length, and the object it names is the
/// string, so a length either side of the object's size is a malformed ID
/// and the attribute is lost. The specification describes no length
/// field; libhdf5, which writes it, refuses both directions ("Expected global
/// heap object size does not match"), and so does this. `calendar`'s
/// `noleap` is six bytes, object 8; before #1557 a length of 5 read `nolea`.
/// `time` stays listed without it (#1681), and the report beside the view
/// says it lacks `calendar` and why (#1569, #1604). Its `noleap` days are
/// still dated as Gregorian, as when it was dropped quietly before #1569, but
/// the note now says the calendar was not read.
#[test]
fn a_string_length_that_differs_from_its_object_is_skipped() {
    let well_formed = every_attribute(H5PY);
    assert_eq!(
        well_formed[&key("time", "calendar")],
        AttributeValue::Text("noleap".into())
    );
    for len in [5, 7] {
        let mut file = H5PY.to_vec();
        patch(&mut file, 1952, &heap_id(6, 8), &heap_id(len, 8));
        assert_eq!(
            every_attribute(&file),
            without(well_formed.clone(), &[("time", "calendar")]),
            "{len}: only time's calendar is lost"
        );
        assert_eq!(left_out(&file), [], "{len}");
        let unread = not_read(&file);
        assert_eq!(
            unread.variable("time"),
            Some(damaged("attribute \"calendar\"").as_str()),
            "{len}"
        );
        assert_eq!(unread.variables.len(), 1, "{len}: {unread:?}");
        assert_eq!(unread.global, None, "{len}");
    }
}

/// The type of every attribute as netCDF-C names it, `{name: type}`.
fn oracle_types(oracle: &Value) -> BTreeMap<&str, &str> {
    oracle
        .as_object()
        .expect("oracle attributes are an object")
        .iter()
        .map(|(name, a)| (name.as_str(), a["type"].as_str().expect("a type name")))
        .collect()
}

fn types(attributes: &[fieldglass_netcdf::Hdf5Attribute]) -> BTreeMap<&str, &str> {
    attributes
        .iter()
        .map(|a| (a.name.as_str(), a.type_name()))
        .collect()
}

/// Every attribute reports the type netCDF-C reports for it (#1570): an
/// `NC_STRING` attribute is `string`, where it used to report `char`, and an
/// `NC_CHAR` or numeric one beside it keeps its type. Both netCDF4-python
/// fixtures, so the one-element and multi-element lists, the scalar strings
/// and the dense storage on `dense` are all held to the oracle.
///
/// The h5py fixture holds fixed-length strings with dimensions (`fixed_list`,
/// and `one_element`, a 1-D list of one), which netCDF-C reports as
/// `NC_STRING`: for a fixed-length string the type follows the dataspace, and
/// only one without dimensions is `NC_CHAR`, as the netCDF4-python fixtures'
/// `char` attributes are.
#[test]
fn every_attribute_reports_the_type_netcdf_c_reports() {
    let oracle: Value = serde_json::from_str(LISTS_ORACLE).expect("oracle parses");
    for (file, bytes) in [
        ("netcdf4_string_lists.nc", LISTS),
        ("netcdf4_string_attributes.nc", NETCDF4),
        ("h5py_string_attributes.h5", H5PY),
    ] {
        let meta = NetcdfReader::from_bytes(bytes.to_vec())
            .expect("recognized NetCDF-4")
            .hdf5_metadata()
            .expect("metadata resolves");
        let expected = &oracle[file];
        assert_eq!(
            types(&meta.global_attributes),
            oracle_types(&expected["global"]),
            "{file}: global attribute types"
        );
        let variables = expected["variables"].as_object().unwrap();
        assert_eq!(meta.variables.len(), variables.len(), "{file}: variables");
        for var in &meta.variables {
            assert_eq!(
                types(&var.attributes),
                oracle_types(&variables[var.name.as_str()]),
                "{file}: attribute types of {}",
                var.name
            );
        }
    }
}

/// The string attributes of the list fixture read as netCDF4-python reads
/// them: a one-element list (`tas:units = ["K"]`) is the string itself, and a
/// list of several, such as the two calendars on `time`, is every element as
/// `ncdump` prints it. That is what a host is handed as the calendar or the
/// units: text that names no calendar and no unit, so the time is not dated
/// (#1570).
#[test]
fn a_string_list_reads_as_one_string_or_as_every_element() {
    let oracle: Value = serde_json::from_str(LISTS_ORACLE).expect("oracle parses");
    let view = NetcdfReader::from_bytes(LISTS.to_vec())
        .expect("recognized NetCDF-4")
        .view()
        .expect("view resolves");
    // The oracle's string-valued attributes, `{name: value}`, as `want` reads.
    let strings = |o: &Value| -> Value {
        o.as_object()
            .unwrap()
            .iter()
            .filter(|(_, a)| match &a["value"] {
                Value::String(_) => true,
                Value::Array(items) => items.iter().all(Value::is_string),
                _ => false,
            })
            .map(|(name, a)| (name.clone(), a["value"].clone()))
            .collect::<serde_json::Map<_, _>>()
            .into()
    };
    let text_only = |attributes: &[Attribute]| -> Vec<Attribute> {
        attributes
            .iter()
            .filter(|a| !matches!(a.value, AttributeValue::Numbers(_)))
            .cloned()
            .collect()
    };
    let file = "netcdf4_string_lists.nc";
    let expected = &oracle[file];
    assert_same(
        file,
        "the root group",
        &text_only(&view.global_attrs),
        &strings(&expected["global"]),
    );
    for var in &view.vars {
        assert_same(
            file,
            &var.array.name,
            &text_only(&var.array.attributes),
            &strings(&expected["variables"][var.array.name.as_str()]),
        );
    }
    // The cases by name, so a wrong oracle cannot pass them.
    let attr = |var: &str, name: &str| {
        let v = view.vars.iter().find(|v| v.array.name == var).unwrap();
        fieldglass_core::array::attribute(&v.array.attributes, name)
            .unwrap()
            .clone()
    };
    assert_eq!(attr("tas", "units"), AttributeValue::Text("K".into()));
    assert_eq!(
        attr("time", "calendar"),
        AttributeValue::Opaque(r#""noleap", "360_day""#.into())
    );
    assert_eq!(
        attr("step", "units"),
        AttributeValue::Opaque(r#""hours since 2000-01-01", "days since 2000-01-01""#.into())
    );
}
