//! String attributes the text budget leaves out are reported, not dropped
//! without a word (#1556).
//!
//! One listing keeps at most 16 MiB of string attribute text (#1549). A
//! string that does not fit in what is left is left out, and the listing used
//! to skip it quietly: a time axis whose `calendar` did not fit had its
//! `noleap` times dated as Gregorian (#1033). A variable whose attributes the
//! budget cut is still listed, without them, as 0.6.0 promised for anything
//! but a datatype outside the decoded subset (#1681), and the reader names
//! the attributes it lacks, and why, beside it (#1604). The global
//! attributes are read first, and since the view has nowhere to say one is
//! missing, those the budget leaves out are dropped, the file opens with the
//! rest, and the session names them in one line (#1604). A string left out
//! is charged nothing, so a later one that fits still reads (#1568): one
//! oversized global does not cut every variable's attributes.
//!
//! A string whose global-heap reference is damaged is reported the same
//! way, as damage rather than as text past the budget (#1569), and a string
//! past the budget is refused before its text is read, so a file of many
//! references to one large string reads it once.
//!
//! Whatever a variable loses, descriptive text such as `long_name` or a
//! `units` or `calendar`, it is kept without it and the loss is named on it
//! (#1569, #1681). Nothing leaves a variable out for its attributes.
//!
//! The inputs are built here from `netcdf4_string_attributes.nc` (provenance
//! in the NetCDF fixtures' `NOTICE.md`): a global-heap collection holding one
//! large string is appended, and string attributes are pointed at it, so the
//! text they refer to passes the budget while the file stays small enough to
//! build in a test.

use fieldglass::Session;
use fieldglass_core::array::{ArrayError, AttributeValue, attribute};
use fieldglass_netcdf::{
    AttributesNotRead, FieldglassError, NetcdfBacking, NetcdfReader, UnreadAttributes,
    list_all_children, list_attributes, list_attributes_with_unread,
};

const STRINGS: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/netcdf4_string_attributes.nc");

/// The one global-heap collection libhdf5 wrote the fixture's strings into.
const COLLECTION: u64 = 2048;

/// The 16 MiB of string text one listing reads.
const BUDGET: usize = 16 << 20;

/// The Jenkins lookup3 hash HDF5 checksums metadata with (`H5_checksum_lookup3`,
/// initial value 0).
fn lookup3(data: &[u8]) -> u32 {
    fn rot(x: u32, k: u32) -> u32 {
        x.rotate_left(k)
    }
    let mut a = 0xdeadbeef_u32.wrapping_add(data.len() as u32);
    let (mut b, mut c) = (a, a);
    let word = |s: &[u8]| {
        let mut w = [0u8; 4];
        w[..s.len()].copy_from_slice(s);
        u32::from_le_bytes(w)
    };
    let mut k = data;
    while k.len() > 12 {
        a = a.wrapping_add(word(&k[0..4]));
        b = b.wrapping_add(word(&k[4..8]));
        c = c.wrapping_add(word(&k[8..12]));
        a = a.wrapping_sub(c);
        a ^= rot(c, 4);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= rot(a, 6);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= rot(b, 8);
        b = b.wrapping_add(a);
        a = a.wrapping_sub(c);
        a ^= rot(c, 16);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= rot(a, 19);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= rot(b, 4);
        b = b.wrapping_add(a);
        k = &k[12..];
    }
    if k.is_empty() {
        return c;
    }
    let tail = |s: &[u8]| {
        if s.is_empty() {
            0
        } else {
            word(&s[..s.len().min(4)])
        }
    };
    a = a.wrapping_add(tail(k));
    b = b.wrapping_add(tail(k.get(4..).unwrap_or(&[])));
    c = c.wrapping_add(tail(k.get(8..).unwrap_or(&[])));
    c ^= b;
    c = c.wrapping_sub(rot(b, 14));
    a ^= c;
    a = a.wrapping_sub(rot(c, 11));
    b ^= a;
    b = b.wrapping_sub(rot(a, 25));
    c ^= b;
    c = c.wrapping_sub(rot(b, 16));
    a ^= c;
    a = a.wrapping_sub(rot(c, 4));
    b ^= a;
    b = b.wrapping_sub(rot(a, 14));
    c ^= b;
    c = c.wrapping_sub(rot(b, 24));
    c
}

/// Every object in the fixture's collection: index and data.
fn objects(file: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let at = COLLECTION as usize;
    assert_eq!(&file[at..at + 4], b"GCOL");
    let end = at + u64::from_le_bytes(file[at + 8..at + 16].try_into().unwrap()) as usize;
    let mut pos = at + 16;
    let mut out = Vec::new();
    while pos + 16 <= end {
        let index = u16::from_le_bytes([file[pos], file[pos + 1]]);
        if index == 0 {
            break;
        }
        let size = u64::from_le_bytes(file[pos + 8..pos + 16].try_into().unwrap()) as usize;
        out.push((u32::from(index), file[pos + 16..pos + 16 + size].to_vec()));
        pos += 16 + size.next_multiple_of(8);
    }
    out
}

/// The one place `needle` occurs in `file`.
fn only(file: &[u8], needle: &[u8]) -> usize {
    let at: Vec<usize> = file
        .windows(needle.len())
        .enumerate()
        .filter(|(_, w)| *w == needle)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        at.len(),
        1,
        "{:?} occurs once",
        String::from_utf8_lossy(needle)
    );
    at[0]
}

/// A global-heap ID: string length, collection address, object index.
fn heap_id(len: usize, collection: u64, index: u32) -> Vec<u8> {
    let mut id = (len as u32).to_le_bytes().to_vec();
    id.extend_from_slice(&collection.to_le_bytes());
    id.extend_from_slice(&index.to_le_bytes());
    id
}

/// `file` with a collection holding one `size`-byte string of `x` appended,
/// and every string attribute whose text starts with one of `texts` pointed
/// at it.
fn pointed_at_a_large_string(mut file: Vec<u8>, texts: &[&[u8]], size: usize) -> Vec<u8> {
    let objects = objects(&file);
    file.resize(file.len().next_multiple_of(8), 0);
    let large = file.len() as u64;
    let mut collection = b"GCOL".to_vec();
    collection.extend_from_slice(&[1, 0, 0, 0]);
    collection.extend_from_slice(&0u64.to_le_bytes());
    collection.extend_from_slice(&1u16.to_le_bytes());
    collection.extend_from_slice(&[1, 0, 0, 0, 0, 0]);
    collection.extend_from_slice(&(size as u64).to_le_bytes());
    collection.resize(collection.len() + size.next_multiple_of(8), b'x');
    collection.extend_from_slice(&[0u8; 16]);
    let total = collection.len() as u64;
    collection[8..16].copy_from_slice(&total.to_le_bytes());

    let mut pointed = 0;
    for (index, data) in &objects {
        if !texts.iter().any(|t| data.starts_with(t)) {
            continue;
        }
        let at = only(&file, &heap_id(data.len(), COLLECTION, *index));
        patch(&mut file, at, &heap_id(size, large, 1));
        pointed += 1;
    }
    assert!(pointed > 0, "some attribute points at the large string");
    file.extend_from_slice(&collection);
    file
}

/// Every structure signature in the fixture, for finding where one ends.
const SIGNATURES: [&[u8; 4]; 8] = [
    b"OHDR", b"OCHK", b"GCOL", b"FRHP", b"FHDB", b"FHIB", b"BTHD", b"BTLF",
];

/// Write `bytes` at `at`, and re-checksum the version-2 object-header chunk
/// holding them, if one does: a chunk ends in a lookup3 checksum of
/// everything before it. Other structures here are not checksummed.
fn patch(file: &mut [u8], at: usize, bytes: &[u8]) {
    let signature_at = |pos: usize| SIGNATURES.iter().any(|s| file[pos..].starts_with(*s));
    let start = (0..at).rev().find(|&pos| signature_at(pos));
    let next = (at..file.len() - 4)
        .find(|&pos| signature_at(pos))
        .unwrap_or(file.len() - 4);
    let chunk =
        start.filter(|&pos| file[pos..].starts_with(b"OHDR") || file[pos..].starts_with(b"OCHK"));
    let end = chunk.map(|chunk| {
        (at + bytes.len()..=next)
            .find(|&end| {
                lookup3(&file[chunk..end])
                    == u32::from_le_bytes(file[end..end + 4].try_into().unwrap())
            })
            .expect("the chunk's checksum")
    });
    file[at..at + bytes.len()].copy_from_slice(bytes);
    if let (Some(chunk), Some(end)) = (chunk, end) {
        let sum = lookup3(&file[chunk..end]);
        file[end..end + 4].copy_from_slice(&sum.to_le_bytes());
    }
}

/// `file` with its variable `dense` renamed `tense`, so it is listed after
/// `temperature` and before `time`. The root group's links are in its
/// version-2 object header, whose chunk is checksummed.
fn dense_renamed_tense(mut file: Vec<u8>) -> Vec<u8> {
    let at = only(&file, b"\x05dense");
    patch(&mut file, at + 1, b"t");
    file
}

/// The fixture with `tense`'s twelve notes, and `time`'s `calendar`, each
/// 1.5 MiB, nineteen and a half in all. The budget runs out on `tense`,
/// after `lat`, `lon` and `temperature`: ten notes fit and two do not. What
/// is left, under 1 MiB, is not room for `time`'s `calendar`, but is for its
/// short `units` and `standard_name`.
fn variables_past_the_budget() -> Vec<u8> {
    let file = dense_renamed_tense(STRINGS.to_vec());
    pointed_at_a_large_string(file, &[b"dense note", b"noleap"], 3 << 19)
}

/// The fixture with its string globals, `title` and the two `keywords`, each
/// 6 MiB: past the budget before any variable is read.
fn globals_past_the_budget() -> Vec<u8> {
    pointed_at_a_large_string(
        STRINGS.to_vec(),
        &[b"fieldglass NC_STRING", b"one", b"two"],
        6 << 20,
    )
}

/// `file` with the link named `temperature` pointed at the object `time`
/// names, so that one dataset is reached by two hard links and
/// `temperature`'s own object by none. `temperature`'s link is in a
/// checksummed continuation chunk; `time`'s is only read.
fn temperature_linked_to_time(mut file: Vec<u8>) -> Vec<u8> {
    let time = only(&file, b"\x04time") + 5;
    let target = file[time..time + 8].to_vec();
    assert_eq!(target, 409u64.to_le_bytes(), "`time`'s object header");
    let temperature = only(&file, b"\x0btemperature") + 12;
    patch(&mut file, temperature, &target);
    file
}

/// `file` with `time` made the pure-dimension placeholder netCDF-C writes for
/// a dimension with no coordinate variable. Its object header has a 120-byte
/// null message, which becomes a `NAME` attribute holding the placeholder
/// text. Header messages are listed before the dense attributes, so it is
/// the `NAME` read; `time`'s own string attributes stay dense.
fn time_made_a_pure_dimension(mut file: Vec<u8>) -> Vec<u8> {
    const TIME_HEADER: usize = 409;
    const NULL_MESSAGE: usize = 547;
    const BODY: usize = 120;
    assert_eq!(&file[TIME_HEADER..TIME_HEADER + 4], b"OHDR");
    assert_eq!(file[NULL_MESSAGE], 0, "a null message");
    assert_eq!(
        u16::from_le_bytes([file[NULL_MESSAGE + 1], file[NULL_MESSAGE + 2]]),
        BODY as u16
    );
    let text_len = BODY - 26;
    let mut message = vec![0x0c, BODY as u8, 0, 0, 0, 0];
    // Version 3, no flags, name 5, datatype 8 and dataspace 4 bytes, ASCII.
    message.extend_from_slice(&[3, 0, 5, 0, 8, 0, 4, 0, 0]);
    message.extend_from_slice(b"NAME\0");
    // A null-terminated fixed-length string, and a scalar dataspace.
    message.extend_from_slice(&[0x13, 0, 0, 0]);
    message.extend_from_slice(&(text_len as u32).to_le_bytes());
    message.extend_from_slice(&[2, 0, 0, 0]);
    let mut text = format!(
        "This is a netCDF dimension but not a netCDF variable.{:>10}",
        3
    )
    .into_bytes();
    text.resize(text_len, 0);
    message.extend_from_slice(&text);
    assert_eq!(message.len(), 6 + BODY);
    patch(&mut file, NULL_MESSAGE, &message);
    file
}

/// The global-heap object headers one listing scans, across every
/// collection (`MAX_GLOBAL_HEAP_OBJECTS`).
const OBJECT_BUDGET: usize = 1 << 20;

/// `file` with a collection appended that holds `count` empty objects and
/// then `noleap`, and `time`'s `calendar` pointed at that last object. The
/// empty objects' indices repeat, as a valid file's never would, but none is
/// the one looked up, so no lookup is refused for a repeat.
fn calendar_behind_empty_objects(mut file: Vec<u8>, count: usize) -> Vec<u8> {
    let (index, data) = objects(&file)
        .into_iter()
        .find(|(_, data)| data == b"noleap")
        .expect("the calendar's object");
    file.resize(file.len().next_multiple_of(8), 0);
    let large = file.len() as u64;
    let mut collection = b"GCOL".to_vec();
    collection.extend_from_slice(&[1, 0, 0, 0]);
    collection.extend_from_slice(&0u64.to_le_bytes());
    for k in 0..count {
        let index = 2 + (k % 65_000) as u16;
        collection.extend_from_slice(&index.to_le_bytes());
        collection.extend_from_slice(&[1, 0, 0, 0, 0, 0]);
        collection.extend_from_slice(&0u64.to_le_bytes());
    }
    collection.extend_from_slice(&1u16.to_le_bytes());
    collection.extend_from_slice(&[1, 0, 0, 0, 0, 0]);
    collection.extend_from_slice(&(data.len() as u64).to_le_bytes());
    collection.extend_from_slice(&data);
    collection.resize(collection.len().next_multiple_of(8), 0);
    collection.extend_from_slice(&[0u8; 16]);
    let total = collection.len() as u64;
    collection[8..16].copy_from_slice(&total.to_le_bytes());
    let at = only(&file, &heap_id(data.len(), COLLECTION, index));
    patch(&mut file, at, &heap_id(data.len(), large, 1));
    file.extend_from_slice(&collection);
    file
}

/// The calendar behind the whole object budget: the scan for it passes the
/// budget before it gets there.
fn calendar_past_the_object_budget() -> Vec<u8> {
    calendar_behind_empty_objects(STRINGS.to_vec(), OBJECT_BUDGET)
}

fn reader(file: Vec<u8>) -> NetcdfReader<Vec<u8>> {
    NetcdfReader::from_bytes(file).expect("the file opens")
}

/// The text budget's refusal, as a note names it.
fn past_the_text_budget() -> String {
    FieldglassError::Array(ArrayError::ListingTooLarge {
        what: "bytes of HDF5 string attribute text in one listing",
        limit: BUDGET as u64,
    })
    .to_string()
}

#[test]
fn the_variables_the_budget_cut_are_named_with_what_they_lack() {
    let (view, unread) = reader(variables_past_the_budget())
        .view_with_unread()
        .expect("the view resolves");
    // No variable is left out for what it lost (#1681).
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["lat", "lon", "temperature", "tense", "time"]);
    // Only what does not fit is cut: `time`'s short `standard_name` and
    // `units` still fit after its `calendar` did not (#1568).
    let time = &view.vars[4].array;
    assert!(attribute(&time.attributes, "calendar").is_none());
    assert!(attribute(&time.attributes, "units").is_some());
    assert!(attribute(&time.attributes, "standard_name").is_some());
    let refusal = past_the_text_budget();
    assert_eq!(
        unread.variable("time"),
        Some(format!("attribute \"calendar\" not read: {refusal}").as_str())
    );

    // What the budget did not cut is all there: the variables read before
    // it ran out, the dimension `time` names, and the global attributes,
    // which are read first. `tense` lost two notes, the last two in the
    // dense store's order, and is named with them.
    let notes: Vec<&str> = view.vars[3]
        .array
        .attributes
        .iter()
        .map(|a| a.name.as_str())
        .filter(|n| n.starts_with("note_"))
        .collect();
    assert_eq!(notes.len(), 10, "{notes:?}");
    assert!(!notes.contains(&"note_08") && !notes.contains(&"note_10"));
    assert_eq!(
        unread.variable("tense"),
        Some(format!("attributes \"note_08\", \"note_10\" not read: {refusal}").as_str())
    );
    assert_eq!(unread.variables.len(), 2, "{unread:?}");
    assert_eq!(unread.global, None);
    let temperature = &view.vars[2].array;
    assert_eq!(temperature.dimensions, ["time", "lat", "lon"]);
    assert_eq!(
        attribute(&temperature.attributes, "units").and_then(AttributeValue::text),
        Some("K")
    );
    assert!(view.dims.iter().any(|d| d.name == "time" && d.length == 3));
    for name in ["title", "keywords", "history"] {
        assert!(
            view.global_attrs.iter().any(|a| a.name == name),
            "global {name}"
        );
    }
}

/// A dataset reached by two hard links is listed, and named with what it
/// lacks, under both names, not only the first one it was described under.
/// Here `temperature` is linked to `time`'s object, whose `calendar` is
/// damaged.
#[test]
fn a_cut_dataset_is_named_under_each_of_its_links() {
    let (view, unread) = reader(temperature_linked_to_time(strings_damaged(&[b"noleap"])))
        .view_with_unread()
        .expect("the view resolves");
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["dense", "lat", "lon", "temperature", "time"]);
    let names: Vec<&str> = unread.variables.keys().map(String::as_str).collect();
    assert_eq!(names, ["temperature", "time"], "{unread:?}");
    assert_eq!(unread.variable("temperature"), unread.variable("time"));
    assert_eq!(
        unread.variable("time"),
        Some("attribute \"calendar\" not read: the file's record of the text is damaged")
    );
}

/// A pure-dimension placeholder whose string attributes the budget cut is
/// still a dimension, and is not listed as a variable left out: none of its
/// attributes would be shown, so the cut loses nothing.
#[test]
fn a_cut_pure_dimension_is_not_listed_as_a_variable() {
    let view = reader(time_made_a_pure_dimension(variables_past_the_budget()))
        .view()
        .expect("the view resolves");
    // `tense` lost two notes, and is kept (#1569).
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["lat", "lon", "temperature", "tense"]);
    assert_eq!(view.vars[2].array.dimensions, ["time", "lat", "lon"]);
    assert!(view.dims.iter().any(|d| d.name == "time" && d.length == 3));
}

/// Under the budget, the placeholder surgery reads back as netCDF-C's own:
/// `time` is a dimension with no variable.
#[test]
fn the_built_pure_dimension_reads_as_one() {
    let view = reader(time_made_a_pure_dimension(STRINGS.to_vec()))
        .view()
        .expect("the view resolves");
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    assert!(view.vars.iter().all(|v| v.array.name != "time"));
    assert!(view.dims.iter().any(|d| d.name == "time" && d.length == 3));
}

/// A string attribute whose object lies past the global-heap object budget
/// is reported like one past the text budget: the budget is a policy cap
/// (ADR-0016 decision 4), so it is `ListingTooLarge`, and the attribute is
/// named on `time` rather than dropped without a word. `time` stays listed
/// (#1681), so its values still label the axis.
#[test]
fn a_calendar_past_the_object_budget_is_reported() {
    let file = calendar_past_the_object_budget();
    let (view, unread) = reader(file.clone())
        .view_with_unread()
        .expect("the view resolves");
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    let refusal = FieldglassError::Array(ArrayError::ListingTooLarge {
        what: "HDF5 global-heap objects",
        limit: OBJECT_BUDGET as u64,
    })
    .to_string();
    let note = format!("attribute \"calendar\" not read: {refusal}");
    assert_eq!(unread.variable("time"), Some(note.as_str()));
    let session = Session::open(file).expect("the file opens");
    assert!(session.left_out().is_empty(), "{:?}", session.left_out());
    let temperature = session
        .variables()
        .iter()
        .position(|v| v.name == "temperature")
        .expect("temperature is renderable") as u32;
    let time = session.axis_values(temperature, 0).expect("the time axis");
    assert!(time.coordinates.is_some(), "time keeps its values");
    assert_eq!(time.calendar, None, "the calendar was not read");
}

/// The same input with a thousand objects fewer reads whole: the calendar's
/// new home holds `noleap`, read past every empty object before it, so the
/// budget is what the test above meets.
#[test]
fn a_calendar_within_the_object_budget_reads() {
    let file = calendar_behind_empty_objects(STRINGS.to_vec(), OBJECT_BUDGET - 1_000);
    let view = reader(file).view().expect("the view resolves");
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    let time = view.vars.iter().find(|v| v.array.name == "time").unwrap();
    assert_eq!(
        attribute(&time.array.attributes, "calendar").and_then(AttributeValue::text),
        Some("noleap")
    );
}

/// The #1033 case: the time variable whose `calendar` the budget cut is
/// listed, and the reader says on it that its `calendar` was not read and
/// why (#1604, #1681). `time` is a coordinate, which the session does not
/// offer as a variable, so the note is the view's, as the metadata page
/// shows it.
#[test]
fn a_later_calendar_the_budget_cut_is_named_on_its_variable() {
    let file = variables_past_the_budget();
    let session = Session::open(file.clone()).expect("the file opens");
    assert!(session.left_out().is_empty(), "{:?}", session.left_out());
    let note = not_read(file, "time").expect("time names what it lacks");
    assert_eq!(
        note,
        format!(
            "attribute \"calendar\" not read: {}",
            past_the_text_budget()
        )
    );
    assert!(note.contains("The file itself is fine"), "{note}");
    let temperature = session
        .variables()
        .iter()
        .position(|v| v.name == "temperature")
        .expect("temperature is renderable") as u32;
    let time = session.axis_values(temperature, 0).expect("the time axis");
    assert_eq!(time.dimension, "time");
    assert_eq!(time.length, 3);
    assert_eq!(time.calendar, None);
}

/// The object header address of the dataset `name` in `file`.
fn object_address(file: &[u8], probe: &fieldglass_netcdf::Hdf5Probe, name: &str) -> u64 {
    list_all_children(file, probe)
        .unwrap()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("{name}"))
        .object_header_address
}

/// The attributes of `name` as `list_attributes` returns them, as `(name,
/// length of value)` so a failure does not print 15 MiB of notes, and the
/// note `list_attributes_with_unread` gives beside the same list.
fn listed_alone(file: &[u8], name: &str) -> (Vec<(String, usize)>, Option<UnreadAttributes>) {
    let reader = reader(file.to_vec());
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        panic!("expected the HDF5 backing");
    };
    let address = object_address(file, probe, name);
    let short = |attrs: Vec<fieldglass_netcdf::Hdf5Attribute>| -> Vec<(String, usize)> {
        attrs.into_iter().map(|a| (a.name, a.value.len())).collect()
    };
    let listed = list_attributes(file, address, probe)
        .map(short)
        .unwrap_or_else(|err| panic!("{name} lists: {err}"));
    let (with, note) = list_attributes_with_unread(file, address, probe)
        .unwrap_or_else(|err| panic!("{name} lists with its note: {err}"));
    assert_eq!(short(with), listed, "the same list either way");
    (listed, note)
}

/// An object listed by itself returns the attributes that fit the budget
/// and skips the rest, as 0.6.0 skipped what it could not read (#1689);
/// `list_attributes_with_unread` names the ones skipped, and why.
#[test]
fn an_attribute_listing_past_the_budget_returns_what_fit() {
    let file = variables_past_the_budget();
    let (listed, note) = listed_alone(&file, "tense");
    // Ten of the twelve 1.5 MiB notes fit in 16 MiB, read in the order the
    // dense storage holds them, so the two that do not are not the last by
    // name.
    let notes: Vec<&str> = listed
        .iter()
        .filter(|(_, len)| *len == 3 << 19)
        .map(|(name, _)| name.as_str())
        .collect();
    assert_eq!(notes.len(), 10, "{listed:?}");
    assert!(!notes.contains(&"note_08") && !notes.contains(&"note_10"));
    let note = note.expect("the two cut are named");
    assert_eq!(
        note.to_string(),
        format!(
            "attributes \"note_08\", \"note_10\" not read: {}",
            past_the_text_budget()
        )
    );
    assert_eq!(note.names().collect::<Vec<_>>(), ["note_08", "note_10"]);
    assert_eq!(note.read_in_part().count(), 0);
    assert_eq!(note.unnamed(), 0);
    // Listed by itself, `time` has the budget to itself, and its 1.5 MiB
    // `calendar` fits.
    let (time, note) = listed_alone(&file, "time");
    assert!(
        time.contains(&("calendar".to_string(), 3 << 19)),
        "{time:?}"
    );
    assert_eq!(note, None);
}

/// An object some of whose strings name a damaged global-heap entry lists
/// the rest, and `list_attributes_with_unread` names the ones lost as
/// damaged (#1689).
#[test]
fn an_attribute_listing_with_damaged_text_returns_the_rest() {
    let (listed, note) = listed_alone(&calendar_index_repeated(), "time");
    let names: Vec<&str> = listed.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"units"), "{names:?}");
    assert!(!names.contains(&"calendar"), "{names:?}");
    assert!(!names.contains(&"standard_name"), "{names:?}");
    let note = note.expect("the two lost are named");
    assert_eq!(
        note.to_string(),
        "attributes \"calendar\", \"standard_name\" not read: the file's record of the \
         text is damaged"
    );
    assert_eq!(
        note.names().collect::<Vec<_>>(),
        ["calendar", "standard_name"]
    );
    let (_, note) = listed_alone(STRINGS, "time");
    assert_eq!(note, None, "the fixture as built reads whole");
}

/// Global attributes past the budget do not refuse the file: it opens with
/// the globals that fit, and the ones the budget cut are left out. The view
/// has nowhere to say a global attribute is missing, so the session names
/// them in one line beside it (#1604).
#[test]
fn global_attributes_past_the_budget_are_left_out_and_the_file_opens() {
    let view = reader(globals_past_the_budget())
        .view()
        .map_err(|err| err.to_string())
        .expect("the file opens");
    let globals: Vec<&str> = view.global_attrs.iter().map(|a| a.name.as_str()).collect();
    // `title`'s 6 MiB fit; `keywords`, two more of 6 MiB, do not.
    assert_eq!(globals, ["history", "title"]);
    assert_eq!(
        attribute(&view.global_attrs, "history").and_then(AttributeValue::text),
        Some("written by tools/build_netcdf4_string_attribute_fixtures.py")
    );
    let title = attribute(&view.global_attrs, "title").and_then(AttributeValue::text);
    assert_eq!(title.map(str::len), Some(6 << 20));
    let session = Session::open(globals_past_the_budget())
        .map_err(|err| err.to_string())
        .expect("the session opens");
    let note = session
        .global_attributes_not_read()
        .expect("the cut globals are named");
    assert!(note.contains("\"keywords\""), "{note}");
    assert!(!note.contains("\"title\""), "{note}");
}

/// One global too large for what is left of the budget is left out, and
/// charged nothing: the variables, read after it, keep every attribute they
/// have (#1568). Charged, it put the count past the budget, so every later
/// string was refused however short, and every variable with a string
/// attribute was left out, `time` with its `calendar`.
#[test]
fn an_oversized_global_leaves_the_variables_their_attributes() {
    let view = reader(globals_past_the_budget())
        .view()
        .map_err(|err| err.to_string())
        .expect("the file opens");
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    // Every variable is as the fixture has it, attributes and all.
    let plain = reader(STRINGS.to_vec()).view().expect("the fixture opens");
    let described = |vars: &[fieldglass_netcdf::VarView]| {
        vars.iter()
            .map(|v| (v.array.name.clone(), v.array.attributes.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(described(&view.vars), described(&plain.vars));
    let time = view.vars.iter().find(|v| v.array.name == "time").unwrap();
    assert_eq!(
        attribute(&time.array.attributes, "calendar").and_then(AttributeValue::text),
        Some("noleap")
    );
    // The text kept is within the budget: `title`'s 6 MiB and everything
    // short, not `keywords`' 12 MiB too.
    let text: usize = view
        .global_attrs
        .iter()
        .chain(view.vars.iter().flat_map(|v| &v.array.attributes))
        .filter_map(|a| a.value.text())
        .map(str::len)
        .sum();
    assert!(text <= BUDGET, "{text} bytes of text kept");
    assert!(text > 6 << 20, "{text} bytes of text kept");
}

/// The surgery leaves the file valid: under the budget, every attribute it
/// touches reads back as the large string.
#[test]
fn the_built_inputs_read_back_under_the_budget() {
    let file = pointed_at_a_large_string(
        dense_renamed_tense(STRINGS.to_vec()),
        &[b"dense note 0"],
        1 << 20,
    );
    let view = reader(file).view().expect("the view resolves");
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    let tense = view
        .vars
        .iter()
        .find(|v| v.array.name == "tense")
        .expect("dense is renamed tense");
    let note = attribute(&tense.array.attributes, "note_00").and_then(AttributeValue::text);
    assert_eq!(note.map(str::len), Some(1 << 20));
    let time = view.vars.iter().find(|v| v.array.name == "time").unwrap();
    assert_eq!(
        attribute(&time.array.attributes, "calendar").and_then(AttributeValue::text),
        Some("noleap")
    );
}

/// The fixture with object 5, `time`'s `standard_name`, renumbered 4, the
/// index of its `calendar`: the #1557 repeated index, on the time axis. The
/// collection is not checksummed.
fn calendar_index_repeated() -> Vec<u8> {
    const OBJECT_5: usize = 2192;
    let mut file = STRINGS.to_vec();
    let found = objects(&file);
    assert!(found.contains(&(4, b"noleap".to_vec())));
    assert!(found.contains(&(5, b"time".to_vec())));
    assert_eq!(&file[OBJECT_5..OBJECT_5 + 2], &5u16.to_le_bytes());
    patch(&mut file, OBJECT_5, &4u16.to_le_bytes());
    file
}

/// A `calendar` whose global-heap reference is damaged, here an index two
/// objects carry (#1557), is reported, not dropped without a word (#1569):
/// dropped, `time`'s `noleap` days were dated as Gregorian. `time` stays
/// listed (#1681) and says which attributes it lost, and why.
#[test]
fn a_damaged_calendar_is_named_on_its_variable() {
    let session = Session::open(calendar_index_repeated()).expect("the file opens");
    assert!(session.left_out().is_empty(), "{:?}", session.left_out());
    assert_eq!(
        not_read(calendar_index_repeated(), "time").as_deref(),
        Some(
            "attributes \"calendar\", \"standard_name\" not read: the file's record of the \
             text is damaged"
        )
    );
    let temperature = session
        .variables()
        .iter()
        .position(|v| v.name == "temperature")
        .expect("temperature is renderable") as u32;
    let time = session.axis_values(temperature, 0).expect("the time axis");
    assert_eq!(time.dimension, "time");
    assert_eq!(time.length, 3);
    assert_eq!(time.calendar, None);
}

/// Every string attribute pointed at one 12 MiB object, the twelve notes of
/// `dense` and `time`'s `calendar`: the first fits the budget and the rest
/// do not. Each is refused before its object is read, so the view reads it
/// once (#1569). Read first and then refused, every reference read it.
#[test]
fn strings_past_the_budget_are_not_read() {
    const LARGE: usize = 12 << 20;
    let file = pointed_at_a_large_string(STRINGS.to_vec(), &[b"dense note", b"noleap"], LARGE);
    let source = fieldglass_core::testing::Recording::new(file.as_slice());
    let (view, unread) = NetcdfReader::from_source(&source)
        .expect("the file opens")
        .view_with_unread()
        .expect("the view resolves");
    assert!(view.unsupported.is_empty(), "{:?}", view.unsupported);
    let names: Vec<&str> = unread.variables.keys().map(String::as_str).collect();
    assert_eq!(names, ["dense", "time"], "{unread:?}");
    let reads = source.reads();
    let read: u64 = reads.iter().map(|r| r.len).sum();
    let large = reads.iter().filter(|r| r.len >= LARGE as u64).count();
    eprintln!("{read} bytes read, {large} reads of the large object");
    assert_eq!(
        large, 1,
        "the large object is read once ({read} bytes read)"
    );
    assert!(read < 2 * LARGE as u64, "{read} bytes read");
}

/// The fixture with the heap ID of each string in `texts` stating one byte
/// more than its object holds: a damaged reference, so each attribute that
/// names one is lost (#1557, #1569).
fn strings_damaged(texts: &[&[u8]]) -> Vec<u8> {
    let mut file = STRINGS.to_vec();
    for text in texts {
        let (index, data) = objects(&file)
            .into_iter()
            .find(|(_, data)| data == text)
            .unwrap_or_else(|| panic!("{:?}", String::from_utf8_lossy(text)));
        let at = only(&file, &heap_id(data.len(), COLLECTION, index));
        patch(&mut file, at, &heap_id(data.len() + 1, COLLECTION, index));
    }
    file
}

/// `temperature`'s `long_name`.
const LONG_NAME: &[u8] = "température de l’air".as_bytes();

/// The variables a session opens with, and those it leaves out.
fn opened(file: Vec<u8>) -> (Vec<String>, Vec<String>) {
    let session = Session::open(file).expect("the file opens");
    let kept = session.variables().into_iter().map(|v| v.name).collect();
    let left_out = session.left_out().into_iter().map(|a| a.name).collect();
    (kept, left_out)
}

/// What `file`'s view says `variable` lacks.
fn not_read(file: Vec<u8>, variable: &str) -> Option<String> {
    let (_, unread): (_, AttributesNotRead) =
        reader(file).view_with_unread().expect("the view resolves");
    unread.variable(variable).map(str::to_string)
}

/// The note the session lists `variable` with.
fn note_on(session: &Session, variable: &str) -> Option<String> {
    session
        .variables()
        .into_iter()
        .find(|v| v.name == variable)
        .and_then(|v| v.attributes_not_read)
}

/// The attribute names `variable` is listed with.
fn attribute_names(file: Vec<u8>, variable: &str) -> Vec<String> {
    let view = reader(file).view().expect("the view resolves");
    let var = view
        .vars
        .iter()
        .find(|v| v.array.name == variable)
        .unwrap_or_else(|| panic!("{variable} is listed"));
    var.array
        .attributes
        .iter()
        .map(|a| a.name.clone())
        .collect()
}

/// A variable that loses its `long_name`, here `temperature`'s, is kept
/// without it, as every variable that loses an attribute is (#1681), and
/// the loss is named on it rather than dropped without a word (#1569).
#[test]
fn a_variable_that_loses_only_its_long_name_is_kept() {
    let file = strings_damaged(&[LONG_NAME]);
    let (kept, left_out) = opened(file.clone());
    assert!(kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert!(left_out.is_empty(), "{left_out:?}");
    let names = attribute_names(file.clone(), "temperature");
    assert!(!names.iter().any(|n| n == "long_name"), "{names:?}");
    assert!(names.iter().any(|n| n == "units"), "{names:?}");
    assert_eq!(
        not_read(file, "temperature").as_deref(),
        Some("attribute \"long_name\" not read: the file's record of the text is damaged")
    );
}

/// Lost beside its `long_name`, a `units` keeps the variable too (#1681),
/// and the note names both, so a host can say the values are shown without
/// their unit.
#[test]
fn a_variable_that_loses_its_units_too_is_kept_and_named() {
    let file = strings_damaged(&[LONG_NAME, b"K"]);
    let (kept, left_out) = opened(file.clone());
    assert!(kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert!(left_out.is_empty(), "{left_out:?}");
    let names = attribute_names(file.clone(), "temperature");
    assert!(
        !names.iter().any(|n| n == "long_name" || n == "units"),
        "{names:?}"
    );
    let note = "attributes \"long_name\", \"units\" not read: the file's record of the text is \
                damaged";
    assert_eq!(not_read(file.clone(), "temperature").as_deref(), Some(note));
    // And through the session, on the variable it offers.
    let session = Session::open(file).expect("the file opens");
    assert_eq!(note_on(&session, "temperature").as_deref(), Some(note));
}

/// `dense` keeps its place without `note_00`, and the loss is named on it
/// (#1569, #1681).
#[test]
fn a_variable_that_loses_a_note_is_kept() {
    let file = strings_damaged(&[b"dense note 0"]);
    let (_, left_out) = opened(file.clone());
    assert!(left_out.is_empty(), "{left_out:?}");
    let names = attribute_names(file.clone(), "dense");
    assert!(!names.iter().any(|n| n == "note_00"), "{names:?}");
    assert!(names.iter().any(|n| n == "note_01"), "{names:?}");
    assert_eq!(
        not_read(file, "dense").as_deref(),
        Some("attribute \"note_00\" not read: the file's record of the text is damaged")
    );
}

/// Any attribute lost keeps the variable and is named on it (#1681): here
/// `temperature`'s `flag_meanings_list`, which no list of descriptive text
/// names.
#[test]
fn a_variable_that_loses_any_attribute_is_kept_and_named() {
    let file = strings_damaged(&[b"good"]);
    let (kept, left_out) = opened(file.clone());
    assert!(kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert!(left_out.is_empty(), "{left_out:?}");
    let names = attribute_names(file.clone(), "temperature");
    assert!(
        !names.iter().any(|n| n == "flag_meanings_list"),
        "{names:?}"
    );
    assert!(names.iter().any(|n| n == "units"), "{names:?}");
    assert_eq!(
        not_read(file, "temperature").as_deref(),
        Some("attribute \"flag_meanings_list\" not read: the file's record of the text is damaged")
    );
}

/// Two separate losses on one variable: `temperature`'s `long_name` past
/// the heap-object cap, and its `units` damaged. It is kept, and the note
/// names each with its own reason.
#[test]
fn a_variable_with_two_losses_names_each_with_its_reason() {
    let file = pointed_at_a_large_string(strings_damaged(&[b"K"]), &[LONG_NAME], BUDGET + 8);
    let (kept, left_out) = opened(file.clone());
    assert!(kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert!(left_out.is_empty(), "{left_out:?}");
    let note = not_read(file, "temperature").expect("temperature names what it lacks");
    let object = FieldglassError::Array(ArrayError::ListingTooLarge {
        what: "bytes in one HDF5 global-heap object",
        limit: BUDGET as u64,
    })
    .to_string();
    assert_eq!(
        note,
        format!(
            "attribute \"units\" not read: the file's record of the text is damaged; \
             attribute \"long_name\" not read: {object}"
        )
    );
}

/// The fixture's `title`, the global the damaged-text fixture loses.
const TITLE: &[u8] = b"fieldglass NC_STRING attribute fixture";

/// The committed fixture a host's tests open (#1604): `temperature`'s
/// `long_name` and the global `title` damaged, as [`strings_damaged`] damages
/// them. Compared here with what this file builds, so the bytes and the
/// recipe cannot drift apart.
const DAMAGED_TEXT: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/netcdf4_damaged_text.nc");

/// Rewrites the fixture when `FIELDGLASS_UPDATE_FIXTURE` is set, and fails
/// that run, so a recording is never taken for a pass.
#[test]
fn the_damaged_text_fixture_is_what_this_builds() {
    let built = strings_damaged(&[LONG_NAME, TITLE]);
    if std::env::var_os("FIELDGLASS_UPDATE_FIXTURE").is_some() {
        let path = "../fieldglass-netcdf/tests/fixtures/netcdf4_damaged_text.nc";
        std::fs::write(path, &built).expect("the fixture is written");
        panic!("{path} rewritten: unset FIELDGLASS_UPDATE_FIXTURE and run again");
    }
    assert!(
        built == DAMAGED_TEXT,
        "rebuild with FIELDGLASS_UPDATE_FIXTURE=1"
    );
}

/// A variable kept without an attribute it could not read says which, and
/// why, where it is listed (#1604). Before, the attribute was gone from the
/// listing with nothing to say it had been there.
#[test]
fn a_kept_variable_names_the_attribute_it_lacks() {
    let session = Session::open(DAMAGED_TEXT.to_vec()).expect("the file opens");
    let note = temperature_note(&session).expect("temperature names what it lacks");
    // The whole note: the parse error that says where in the bytes is left
    // to the library, and the reader is told only that the text is damaged.
    assert_eq!(
        note,
        "attribute \"long_name\" not read: the file's record of the text is damaged"
    );
    for v in session
        .variables()
        .iter()
        .filter(|v| v.name != "temperature")
    {
        assert_eq!(v.attributes_not_read, None, "{}", v.name);
    }
}

/// A variable that lost an attribute and whose `DIMENSION_LIST` is damaged
/// too has one note naming both, the lost attribute first (#1695).
/// `temperature`'s first axis is its heap ID of one reference, object 29,
/// made to claim two.
#[test]
fn a_damaged_dimension_list_joins_the_note_on_a_lost_attribute() {
    let lost = temperature_note(&Session::open(DAMAGED_TEXT.to_vec()).expect("the file opens"))
        .expect("temperature names what it lacks");
    let mut file = DAMAGED_TEXT.to_vec();
    let at = only(&file, &heap_id(1, COLLECTION, 29));
    patch(&mut file, at, &2u32.to_le_bytes());
    let session = Session::open(file).expect("the file opens");
    assert_eq!(
        temperature_note(&session),
        Some(format!(
            "{lost}; attribute \"DIMENSION_LIST\" not read: the file's record of the \
             variable's dimensions is damaged (DIMENSION_LIST axis length does not match \
             its global-heap object), so its axes are unnamed"
        ))
    );
}

/// The global attributes left out are named in one line, with the reason
/// (#1604), and the rest are listed as before.
#[test]
fn the_global_attributes_left_out_are_named() {
    let session = Session::open(DAMAGED_TEXT.to_vec()).expect("the file opens");
    let note = session
        .global_attributes_not_read()
        .expect("the lost title is named");
    assert_eq!(
        note,
        "attribute \"title\" not read: the file's record of the text is damaged"
    );

    let (view, unread) = reader(DAMAGED_TEXT.to_vec())
        .view_with_unread()
        .expect("the view resolves");
    assert!(!view.global_attrs.iter().any(|a| a.name == "title"));
    assert!(view.global_attrs.iter().any(|a| a.name == "history"));
    assert_eq!(unread.global.as_deref(), Some(note.as_str()));
    assert_eq!(
        unread.variable("temperature"),
        temperature_note(&session).as_deref()
    );

    // An undamaged file has nothing to say.
    let whole = Session::open(STRINGS.to_vec()).expect("the file opens");
    assert_eq!(whole.global_attributes_not_read(), None);
    assert!(
        whole
            .variables()
            .iter()
            .all(|v| v.attributes_not_read.is_none())
    );
}

/// `temperature`'s note as the session lists it.
fn temperature_note(session: &Session) -> Option<String> {
    session
        .variables()
        .into_iter()
        .find(|v| v.name == "temperature")
        .and_then(|v| v.attributes_not_read)
}
