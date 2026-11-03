//! String attributes the text budget leaves out are reported, not dropped
//! without a word (#1556).
//!
//! One listing keeps at most 16 MiB of string attribute text (#1549). A
//! string that does not fit in what is left is left out, and the listing used
//! to skip it quietly: a time axis whose `calendar` did not fit had its
//! `noleap` times dated as Gregorian (#1033). A variable whose attributes the
//! budget cut is now left out of the listing and named, with the attributes
//! it lacks, beside the ones whose datatype this build does not read. The
//! global attributes are read first, and since the view has nowhere to say
//! one is missing, those the budget leaves out are dropped, the file opens
//! with the rest, and the session names them in one line (#1604). A string left out is charged nothing, so a later one that
//! fits still reads (#1568): one oversized global does not cut every
//! variable's attributes.
//!
//! A string whose global-heap reference is damaged is reported the same
//! way, as damage rather than as text past the budget (#1569), and a string
//! past the budget is refused before its text is read, so a file of many
//! references to one large string reads it once.
//!
//! A variable that loses only descriptive text (`long_name`, `comment`, a
//! note and the like), however it lost it, is kept without it: nothing it
//! draws changes. Any other loss leaves it out (#1569, the owner's
//! decision).
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
    assert_eq!(names, ["time"], "{left_out:?}");
    let refusal = FieldglassError::Array(ArrayError::ListingTooLarge {
        what: "bytes of HDF5 string attribute text in one listing",
        limit: BUDGET as u64,
    })
    .to_string();
    // Only what does not fit is cut: `time`'s short `standard_name` and
    // `units` still fit after its `calendar` did not (#1568).
    assert_eq!(
        left_out[0].1,
        format!("attribute `calendar` not read: {refusal}")
    );
    assert!(
        view.unsupported
            .iter()
            .all(|v| v.cause == LeftOutCause::Unsupported)
    );

    // What the budget did not cut is all there: the variables read before
    // it ran out, the dimension `time` names, and the global attributes,
    // which are read first. `tense` lost two notes, the last two in the
    // dense store's order, and notes change nothing drawn, so it is kept
    // without them (#1569).
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["lat", "lon", "temperature", "tense"]);
    let notes: Vec<&str> = view.vars[3]
        .array
        .attributes
        .iter()
        .map(|a| a.name.as_str())
        .filter(|n| n.starts_with("note_"))
        .collect();
    assert_eq!(notes.len(), 10, "{notes:?}");
    assert!(!notes.contains(&"note_08") && !notes.contains(&"note_10"));
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
/// only the first one it was described under. Here `temperature` is linked
/// to `time`'s object, whose `calendar` is damaged.
#[test]
fn a_cut_dataset_is_named_under_each_of_its_links() {
    let view = reader(temperature_linked_to_time(strings_damaged(&[b"noleap"])))
        .view()
        .expect("the view resolves");
    let mut names: Vec<&str> = view.unsupported.iter().map(|v| v.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["temperature", "time"], "{:?}", view.unsupported);
    let reason = |name: &str| {
        let v = view.unsupported.iter().find(|v| v.name == name).unwrap();
        v.reason.as_str()
    };
    assert_eq!(reason("temperature"), reason("time"));
    assert!(
        reason("time").starts_with("attribute `calendar` not read"),
        "{}",
        reason("time")
    );
    let vars: Vec<&str> = view.vars.iter().map(|v| v.array.name.as_str()).collect();
    assert_eq!(vars, ["dense", "lat", "lon"]);
}

/// A pure-dimension placeholder whose string attributes the budget cut is
/// still a dimension, and is not listed as a variable left out: none of its
/// attributes would be shown, so the cut loses nothing.
#[test]
fn a_cut_pure_dimension_is_not_listed_as_a_variable() {
    let view = reader(time_made_a_pure_dimension(variables_past_the_budget()))
        .view()
        .expect("the view resolves");
    // `tense` lost only notes, and is kept (#1569).
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
    // Listed by itself, `time` has the budget to itself, and its 1.5 MiB
    // `calendar` fits.
    let time = list_attributes(file.as_slice(), address("time"), probe).expect("time lists");
    assert!(
        time.iter()
            .any(|a| a.name == "calendar" && a.value.len() == 3 << 19)
    );
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
    assert!(note.contains("`keywords`"), "{note}");
    assert!(!note.contains("`title`"), "{note}");
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
/// dropped, `time`'s `noleap` days were dated as Gregorian. `time` is left
/// out and says which attributes it lost, so the axis has no times to date.
#[test]
fn a_damaged_calendar_is_reported_and_the_axis_not_dated() {
    let session = Session::open(calendar_index_repeated()).expect("the file opens");
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

    let left_out = session.left_out();
    let names: Vec<&str> = left_out.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["time"], "{left_out:?}");
    assert_eq!(
        left_out[0].reason,
        "attributes `calendar`, `standard_name` not read: the file's record of the \
         text is damaged (parse error: global-heap collection holds object index 4 twice)"
    );
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
    let view = NetcdfReader::from_source(&source)
        .expect("the file opens")
        .view()
        .expect("the view resolves");
    let names: Vec<&str> = view.unsupported.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["time"], "{:?}", view.unsupported);
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

/// A variable that loses only descriptive text, here `temperature`'s
/// `long_name`, is kept: nothing it draws changes, so it is drawn without
/// the name rather than not at all (#1569, the owner's decision).
#[test]
fn a_variable_that_loses_only_its_long_name_is_kept() {
    let file = strings_damaged(&[LONG_NAME]);
    let (kept, left_out) = opened(file.clone());
    assert!(kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert!(left_out.is_empty(), "{left_out:?}");
    let names = attribute_names(file, "temperature");
    assert!(!names.iter().any(|n| n == "long_name"), "{names:?}");
    assert!(names.iter().any(|n| n == "units"), "{names:?}");
}

/// Lost beside its `long_name`, a `units` leaves the variable out: without
/// it the values read as unitless.
#[test]
fn a_variable_that_loses_its_units_too_is_left_out() {
    let (kept, left_out) = opened(strings_damaged(&[LONG_NAME, b"K"]));
    assert!(!kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert_eq!(left_out, ["temperature"]);
}

/// A note is descriptive, whatever follows `note`: `dense` keeps its place
/// without `note_00`.
#[test]
fn a_variable_that_loses_a_note_is_kept() {
    let file = strings_damaged(&[b"dense note 0"]);
    let (_, left_out) = opened(file.clone());
    assert!(left_out.is_empty(), "{left_out:?}");
    let names = attribute_names(file, "dense");
    assert!(!names.iter().any(|n| n == "note_00"), "{names:?}");
    assert!(names.iter().any(|n| n == "note_01"), "{names:?}");
}

/// An attribute the list does not name may change what is drawn, so losing
/// one leaves the variable out: here `temperature`'s `flag_meanings_list`.
#[test]
fn a_variable_that_loses_an_attribute_not_on_the_list_is_left_out() {
    let (kept, left_out) = opened(strings_damaged(&[b"good"]));
    assert!(!kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert_eq!(left_out, ["temperature"]);
}

/// Two separate losses on one variable, each judged: `temperature`'s
/// `long_name` past the heap-object cap, which alone would keep it, and its
/// `units` damaged, which does not. Every loss must be descriptive for the
/// variable to stay, so it is left out, named with both.
#[test]
fn a_variable_with_a_descriptive_and_another_loss_is_left_out() {
    let file = pointed_at_a_large_string(strings_damaged(&[b"K"]), &[LONG_NAME], BUDGET + 8);
    let view = reader(file.clone()).view().expect("the view resolves");
    let temperature = view
        .unsupported
        .iter()
        .find(|v| v.name == "temperature")
        .expect("temperature is left out");
    for name in ["`long_name`", "`units`"] {
        assert!(temperature.reason.contains(name), "{}", temperature.reason);
    }
    let (kept, left_out) = opened(file);
    assert!(!kept.iter().any(|n| n == "temperature"), "{kept:?}");
    assert_eq!(left_out, ["temperature"]);
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
        "attribute `long_name` not read: the file's record of the text is damaged"
    );
    for v in session
        .variables()
        .iter()
        .filter(|v| v.name != "temperature")
    {
        assert_eq!(v.attributes_not_read, None, "{}", v.name);
    }
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
        "attribute `title` not read: the file's record of the text is damaged"
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
