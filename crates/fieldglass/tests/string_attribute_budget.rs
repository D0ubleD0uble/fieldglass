//! String attributes the text budget leaves out are reported, not dropped
//! without a word (#1556).
//!
//! One listing reads at most 16 MiB of string attribute text (#1549). Past
//! that, every later variable-length string is left out, and the listing used
//! to skip them quietly: 16 MiB of text early in a file took the `calendar` of
//! a later time axis with it, and its `noleap` times were dated as Gregorian
//! (#1033). A variable whose attributes the budget cut is now left out of the
//! listing and named, with the attributes it lacks, beside the ones whose
//! datatype this build does not read. The global attributes are read first,
//! and since the view has nowhere to say one is missing, those the budget
//! leaves out are dropped and the file opens with the rest.
//!
//! The inputs are built here from `netcdf4_string_attributes.nc` (provenance
//! in the NetCDF fixtures' `NOTICE.md`): a global-heap collection holding one
//! large string is appended, and string attributes are pointed at it, so the
//! text they refer to passes the budget while the file stays small enough to
//! build in a test.

use fieldglass::Session;
use fieldglass_core::array::{ArrayError, AttributeValue, attribute};
use fieldglass_netcdf::{
    FieldglassError, LeftOutCause, NetcdfBacking, NetcdfReader, list_all_children, list_attributes,
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

/// The fixture with `tense`'s twelve notes each 1.5 MiB, eighteen in all:
/// the budget runs out on `tense`, after `lat`, `lon` and `temperature`, and
/// before `time`.
fn variables_past_the_budget() -> Vec<u8> {
    let file = dense_renamed_tense(STRINGS.to_vec());
    pointed_at_a_large_string(file, &[b"dense note"], 3 << 19)
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

/// `file` with the link named `temperature` pointed at the object `tense`
/// names, so that one dataset is reached by two hard links and
/// `temperature`'s own object by none. The links are in a checksummed
/// continuation chunk.
fn temperature_linked_to_tense(mut file: Vec<u8>) -> Vec<u8> {
    let tense = only(&file, b"\x05tense") + 6;
    let target = file[tense..tense + 8].to_vec();
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

#[test]
fn the_variables_the_budget_cut_are_named_with_what_they_lack() {
    let view = reader(variables_past_the_budget())
        .view()
        .expect("the view resolves");
    let left_out: Vec<(&str, &str)> = view
        .unsupported
        .iter()
        .map(|v| (v.name.as_str(), v.reason.as_str()))
        .collect();
    let names: Vec<&str> = left_out.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["tense", "time"], "{left_out:?}");
    let refusal = FieldglassError::Array(ArrayError::ListingTooLarge {
        what: "bytes of HDF5 string attribute text in one listing",
        limit: BUDGET as u64,
    })
    .to_string();
    assert_eq!(
        left_out[1].1,
        format!("attributes `calendar`, `standard_name` not read: {refusal}")
    );
    assert!(
        left_out[0].1.starts_with("attributes `note_") && left_out[0].1.ends_with(&refusal),
        "{}",
        left_out[0].1
    );
    assert!(
        view.unsupported
            .iter()
            .all(|v| v.cause == LeftOutCause::Unsupported)
    );

    // What the budget did not cut is all there: the variables read before
    // it ran out, the dimension `time` names, and the global attributes,
    // which are read first.
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["lat", "lon", "temperature"]);
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

/// A dataset reached by two hard links is left out under both names, not
/// only the first one it was described under.
#[test]
fn a_cut_dataset_is_named_under_each_of_its_links() {
    let view = reader(temperature_linked_to_tense(variables_past_the_budget()))
        .view()
        .expect("the view resolves");
    let mut names: Vec<&str> = view.unsupported.iter().map(|v| v.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        ["temperature", "tense", "time"],
        "{:?}",
        view.unsupported
    );
    let reason = |name: &str| {
        let v = view.unsupported.iter().find(|v| v.name == name).unwrap();
        v.reason.as_str()
    };
    assert_eq!(reason("temperature"), reason("tense"));
    assert!(
        reason("tense").starts_with("attributes `note_"),
        "{}",
        reason("tense")
    );
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["lat", "lon"]);
}

/// A pure-dimension placeholder whose string attributes the budget cut is
/// still a dimension, and is not listed as a variable left out: none of its
/// attributes would be shown, so the cut loses nothing.
#[test]
fn a_cut_pure_dimension_is_not_listed_as_a_variable() {
    let view = reader(time_made_a_pure_dimension(variables_past_the_budget()))
        .view()
        .expect("the view resolves");
    let names: Vec<&str> = view.unsupported.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["tense"], "{:?}", view.unsupported);
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["lat", "lon", "temperature"]);
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
/// named rather than dropped without a word, which dated `time` as standard.
#[test]
fn a_calendar_past_the_object_budget_is_reported() {
    let file = calendar_past_the_object_budget();
    let view = reader(file.clone()).view().expect("the view resolves");
    let left_out: Vec<(&str, &str)> = view
        .unsupported
        .iter()
        .map(|v| (v.name.as_str(), v.reason.as_str()))
        .collect();
    let refusal = FieldglassError::Array(ArrayError::ListingTooLarge {
        what: "HDF5 global-heap objects",
        limit: OBJECT_BUDGET as u64,
    })
    .to_string();
    assert_eq!(
        left_out,
        [(
            "time",
            format!("attribute `calendar` not read: {refusal}").as_str()
        )]
    );
    let session = Session::open(file).expect("the file opens");
    let temperature = session
        .variables()
        .iter()
        .position(|v| v.name == "temperature")
        .expect("temperature is renderable") as u32;
    let time = session.axis_values(temperature, 0).expect("the time axis");
    assert_eq!(time.coordinates, None, "no times without their calendar");
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

/// The #1033 case through the Session: the time axis whose `calendar` the
/// budget cut is not handed out as times with no calendar, which CF reads as
/// `standard`. Its coordinate array is left out and says why, so the axis
/// has no times to date at all.
#[test]
fn a_later_calendar_the_budget_cut_is_not_read_as_standard() {
    let session = Session::open(variables_past_the_budget()).expect("the file opens");
    let temperature = session
        .variables()
        .iter()
        .position(|v| v.name == "temperature")
        .expect("temperature is renderable") as u32;
    let time = session.axis_values(temperature, 0).expect("the time axis");
    assert_eq!(time.dimension, "time");
    assert_eq!(time.length, 3);
    assert_eq!(time.coordinates, None, "no times without their calendar");
    assert_eq!(time.calendar, None);
    assert_eq!(time.units, None);

    let left_out = session.left_out();
    let time = left_out
        .iter()
        .find(|a| a.name == "time")
        .expect("time is left out");
    assert!(time.reason.contains("`calendar`"), "{}", time.reason);
    assert!(
        time.reason.contains("The file itself is fine"),
        "{}",
        time.reason
    );
}

/// An object listed by itself says the budget ran out rather than handing
/// back the attributes it read before that.
#[test]
fn an_attribute_listing_past_the_budget_is_refused() {
    let file = variables_past_the_budget();
    let reader = reader(file.clone());
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        panic!("expected the HDF5 backing");
    };
    let children = list_all_children(file.as_slice(), probe).unwrap();
    let address = |name: &str| {
        children
            .iter()
            .find(|c| c.name == name)
            .unwrap_or_else(|| panic!("{name}"))
            .object_header_address
    };
    // Mapped to a count, so a failure does not print 15 MiB of notes.
    let err = list_attributes(file.as_slice(), address("tense"), probe)
        .map(|listed| listed.len())
        .expect_err("eighteen MiB of notes");
    assert!(
        matches!(
            err,
            FieldglassError::Array(ArrayError::ListingTooLarge { limit, .. })
                if limit == BUDGET as u64
        ),
        "{err:?}"
    );
    // Listed by itself, `time` has the budget to itself.
    let time = list_attributes(file.as_slice(), address("time"), probe).expect("time lists");
    assert!(
        time.iter()
            .any(|a| a.name == "calendar" && a.value == "noleap")
    );
}

/// Global attributes past the budget do not refuse the file: it opens with
/// the globals that fit, and the ones the budget cut are left out. The view
/// has nowhere to say a global attribute is missing, so they are dropped
/// without a report. The variables, read after, find the budget spent and
/// are reported as the budget cut them.
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
    let left_out: Vec<&str> = view.unsupported.iter().map(|v| v.name.as_str()).collect();
    assert!(left_out.contains(&"time"), "{left_out:?}");
    assert!(
        view.unsupported
            .iter()
            .all(|v| v.reason.contains("The file itself is fine")),
        "{:?}",
        view.unsupported
    );
    Session::open(globals_past_the_budget())
        .map(drop)
        .map_err(|err| err.to_string())
        .expect("the session opens");
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
