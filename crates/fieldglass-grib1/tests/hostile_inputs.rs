//! Smoke tests for hostile / malformed inputs. The reader must surface a
//! structured `FieldglassError::Parse` (or return zero messages) for these —
//! never panic and never silently misinterpret garbage as valid data.
//!
//! These cover the failure modes most likely to arrive over the VS Code
//! `workspace.fs.readFile` API in the wild: truncated downloads, files of the
//! wrong format, files with `GRIB` substrings inside binary payloads, empty
//! buffers, and length-field mismatches.

use fieldglass_core::FieldglassError;
use fieldglass_grib1::Grib1Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib");

#[test]
fn empty_buffer_yields_zero_messages() {
    let reader = Grib1Reader::from_bytes(Vec::new()).expect("empty buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_too_short_for_indicator_yields_zero_messages() {
    // Anything under 8 bytes can't be a complete IS — the scanner should
    // return cleanly with no messages rather than out-of-bounds-indexing.
    let reader = Grib1Reader::from_bytes(b"GR".to_vec()).expect("short buffer parses");
    assert_eq!(reader.message_count(), 0);

    let reader = Grib1Reader::from_bytes(b"GRIB".to_vec()).expect("4-byte buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_with_no_grib_marker_yields_zero_messages() {
    let buf = b"this is just some random bytes, not GRIB at all".to_vec();
    let reader = Grib1Reader::from_bytes(buf).expect("non-grib bytes parse");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn grib_substring_inside_payload_does_not_misparse() {
    // A buffer that contains the literal "GRIB" substring but not as a real
    // message header. The scanner must skip past it without crashing or
    // claiming a phantom message.
    let mut buf = Vec::new();
    buf.extend_from_slice(b"some prefix GRIB but not a real message header padding");
    let reader = Grib1Reader::from_bytes(buf).expect("buffer with GRIB substring parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn truncated_message_returns_parse_error() {
    // Take a real message and lop off the trailing half so the IS-declared
    // total length runs past the end of the buffer.
    let mut buf = FIXTURE.to_vec();
    buf.truncate(FIXTURE.len() / 2);

    let Err(err) = Grib1Reader::from_bytes(buf) else {
        panic!("truncated buffer must error");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected FieldglassError::Parse, got {err:?}"
    );
}

#[test]
fn missing_7777_trailer_returns_parse_error() {
    // Replace the last 4 bytes of a real message with garbage so the End
    // Section validator trips.
    let mut buf = FIXTURE.to_vec();
    let len = buf.len();
    buf[len - 4..].copy_from_slice(b"AAAA");

    let Err(err) = Grib1Reader::from_bytes(buf) else {
        panic!("trailer-corrupt buffer must error");
    };
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error");
    };
    assert!(
        msg.contains("7777"),
        "error should mention the 7777 marker, got: {msg}"
    );
}

#[test]
fn wrong_grib_edition_byte_skips_message() {
    // The fixture is GRIB edition 1. Patch byte 7 (the edition octet) to a
    // value the GRIB1 reader doesn't handle. The scanner is supposed to
    // skip non-edition-1 messages forward by one byte rather than panic.
    let mut buf = FIXTURE.to_vec();
    buf[7] = 2; // pretend it's GRIB2
    let reader = Grib1Reader::from_bytes(buf).expect("non-edition-1 buffer parses cleanly");
    assert_eq!(
        reader.message_count(),
        0,
        "GRIB1 reader should ignore edition-2 messages"
    );
}

#[test]
fn decode_grid_for_out_of_range_index_returns_error() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let err = reader
        .decode_message_values(99)
        .expect_err("out-of-range index must error");
    assert!(
        matches!(err, FieldglassError::OutOfRange),
        "expected FieldglassError::OutOfRange, got {err:?}"
    );
}
