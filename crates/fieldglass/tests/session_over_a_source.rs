//! `Session` opened over a source rather than a buffer (#709, ADR-0005).
//!
//! `Session::open` takes the whole file, which is the right shape when the host
//! has it and the wrong one for the work ADR-0005 exists for: a file too large to
//! hold (#114), an archive over HTTP range requests (#247), an object in a bucket
//! (#252). Those hosts have a `ByteSource`, and reaching a reader with one used to
//! mean going around this crate — the host divergence ADR-0006 exists to prevent.
//!
//! What is checked here is that the two ways in **agree**: the same messages, the
//! same metadata, the same decoded values, to the bit. A test that only asserted
//! the source path succeeded would pass against a reader that silently decoded
//! something else.

use fieldglass::Session;
use fieldglass_core::bytes::ByteRange;
use fieldglass_core::testing::OneRange;

/// Committed single-message GRIB2 fixtures laid end to end, which is the shape a
/// NODD object has — and the shape a sidecar index points into.
const PARTS: &[&str] = &[
    "../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2",
    "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2",
    "../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2",
];

/// The concatenated object, and where each message begins in it.
fn object() -> (Vec<u8>, Vec<u64>) {
    let mut bytes = Vec::new();
    let mut offsets = Vec::new();
    for part in PARTS {
        offsets.push(bytes.len() as u64);
        // Relative, like every other fixture path in this workspace: the CI
        // wasm32-wasip1 run preopens the crate directory and its parent.
        bytes.extend(std::fs::read(part).unwrap_or_else(|e| panic!("{part}: {e}")));
    }
    (bytes, offsets)
}

/// A field's values as raw bits, so the comparison is exact.
fn bits(values: &fieldglass::Values) -> Vec<u64> {
    match values {
        fieldglass::Values::F32(v) => v.iter().map(|x| u64::from(x.to_bits())).collect(),
        fieldglass::Values::F64(v) => v.iter().map(|x| x.to_bits()).collect(),
        // `Values` is `#[non_exhaustive]`; a width added later must come past
        // this test rather than silently compare as nothing.
        other => panic!("a values width this test does not compare: {other:?}"),
    }
}

/// Opening from a source is opening from the bytes, message for message.
#[test]
fn open_source_agrees_with_open() {
    let (bytes, offsets) = object();

    let from_bytes = Session::open(bytes.clone()).expect("the object opens");
    let from_source = Session::open_source(bytes.clone()).expect("and so does the source");

    assert_eq!(from_source.count(), offsets.len() as u32);
    assert_eq!(from_source.count(), from_bytes.count());
    assert_eq!(from_source.format(), from_bytes.format());
    assert_eq!(from_source.addressing(), from_bytes.addressing());

    for i in 0..from_bytes.count() {
        assert_eq!(
            from_source.message(i).expect("metadata"),
            from_bytes.message(i).expect("metadata"),
            "message {i} described differently"
        );
    }
}

/// One message decodes from a source that holds **only that message's range**,
/// and to the same values the whole-file decode gives.
///
/// This is the acceptance criterion for `open_message_at`, and the source is what
/// makes it one: `OneRange` refuses every read outside what it holds, so a reader
/// that wandered past its message fails here rather than quietly succeeding
/// against a buffer that happens to hold the rest of the file.
#[test]
fn a_message_decodes_from_a_source_holding_only_its_range() {
    let (bytes, offsets) = object();
    let whole = Session::open(bytes.clone()).expect("the object opens");

    for (i, &offset) in offsets.iter().enumerate() {
        let end = offsets.get(i + 1).copied().unwrap_or(bytes.len() as u64);
        let held = ByteRange::new(offset, end - offset);
        let source = OneRange::new(bytes.clone(), held);

        let one = Session::open_message_at(source, offset)
            .unwrap_or_else(|e| panic!("message {i} at {offset}: {e:?}"));

        // The session holds exactly that message, whatever its place in the file.
        assert_eq!(one.count(), 1, "message {i}");
        assert_eq!(
            one.message(0).expect("metadata").parameter,
            whole.message(i as u32).expect("metadata").parameter,
            "message {i}: a different parameter through the index"
        );

        // The values, to the bit. Compared through the wire enum's own bytes
        // rather than as floats, so a masked NaN compares as itself and a
        // sign-flipped zero does not slip through.
        let a = one.decode(0, &Default::default()).expect("decode of one");
        let b = whole
            .decode(i as u32, &Default::default())
            .expect("decode of the object");
        assert_eq!(
            bits(&a.values),
            bits(&b.values),
            "message {i}: decoded differently from its own range"
        );
        assert_eq!(a.mask, b.mask, "message {i}: different mask");
    }
}

/// The edition is detected **at the offset**, not at the start of the file.
///
/// A sidecar index points into an archive whose first bytes are another message,
/// and need not even be the same edition. So this object is a GRIB2 message
/// followed by a GRIB1 one: opened as a whole it is GRIB2, and opened at the
/// second offset it has to be GRIB1. A detection that looked at byte zero would
/// return GRIB2 here and then fail to find a GRIB2 message at the offset —
/// which is the bug this asserts against, in the one shape where getting it
/// wrong cannot look like getting it right.
///
/// The source holds **only** the GRIB1 message's range, so a read before the
/// offset is refused rather than merely observed.
#[test]
fn the_edition_is_detected_at_the_offset_not_at_the_start() {
    let head = std::fs::read("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2")
        .expect("a GRIB2 fixture");
    let tail = std::fs::read("../fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1")
        .expect("a GRIB1 fixture");
    let offset = head.len() as u64;
    let mut bytes = head;
    bytes.extend_from_slice(&tail);

    // Whole-file: GRIB2, because that is what byte zero says.
    assert_eq!(
        Session::open(bytes.clone()).expect("opens").format(),
        fieldglass::api::SourceFormat::Grib2
    );

    // At the offset: GRIB1, from a source that holds nothing before it.
    let held = ByteRange::new(offset, bytes.len() as u64 - offset);
    let one = Session::open_message_at(OneRange::new(bytes, held), offset)
        .expect("the GRIB1 message opens from its own range");
    assert_eq!(one.count(), 1);
    assert_eq!(one.format(), fieldglass::api::SourceFormat::Grib1);
    assert!(one.decode(0, &Default::default()).is_ok(), "and it decodes");
}

/// A container that is not a message stream is refused by name, and the refusal
/// says which call to make instead.
#[test]
fn a_non_message_container_is_refused_with_the_call_to_make() {
    let netcdf = std::fs::read("../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc")
        .expect("the fixture");
    let err = Session::open_source(netcdf).expect_err("NetCDF is not a message stream");
    let message = err.message();
    assert!(
        message.contains("Session::open"),
        "the refusal must name the call to make: {message}"
    );

    let err = Session::open_source(b"not a container at all".to_vec())
        .expect_err("and neither is nonsense");
    assert_eq!(err.code(), "unsupported_format", "{}", err.message());
}

/// A source shorter than the detection prefix says "no container", not "short
/// read": too few bytes to identify is an answer about the bytes.
#[test]
fn a_source_too_short_to_identify_is_an_unsupported_format() {
    let err = Session::open_source(b"GR".to_vec()).expect_err("two bytes identify nothing");
    assert_eq!(err.code(), "unsupported_format", "{}", err.message());
}
