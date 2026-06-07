//! Smoke tests for malformed / out-of-spec GRIB2 inputs. The reader must
//! surface a structured `FieldglassError::Parse` (or return zero messages)
//! for these — never panic, over-read, or silently misinterpret garbage as
//! valid data.
//!
//! These cover the failure modes most likely to arrive over the VS Code
//! `workspace.fs.readFile` API in the wild: truncated downloads, files of the
//! wrong format/edition, files with `GRIB` substrings inside binary payloads,
//! empty buffers, and section-length-field mismatches. The section-length
//! regressions below were found by reading the same scan-plus-decode path the
//! `fuzz/` `decode` target exercises.

use fieldglass_core::FieldglassError;
use fieldglass_grib2::Grib2Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/regular_latlon_surface.grib2");

#[test]
fn empty_buffer_yields_zero_messages() {
    let reader = Grib2Reader::from_bytes(Vec::new()).expect("empty buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_too_short_for_indicator_yields_zero_messages() {
    // Anything under the 16-byte IS can't be a complete message — the scanner
    // should return cleanly with no messages rather than out-of-bounds-index.
    let reader = Grib2Reader::from_bytes(b"GR".to_vec()).expect("short buffer parses");
    assert_eq!(reader.message_count(), 0);

    let reader = Grib2Reader::from_bytes(b"GRIB".to_vec()).expect("4-byte buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_with_no_grib_marker_yields_zero_messages() {
    let buf = b"this is just some random bytes, not GRIB at all".to_vec();
    let reader = Grib2Reader::from_bytes(buf).expect("non-grib bytes parse");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn grib_substring_inside_payload_does_not_misparse() {
    // A buffer that contains the literal "GRIB" substring but not as a real
    // message header. The scanner must skip past it (the edition byte won't be
    // 2) without crashing or claiming a phantom message.
    let buf = b"some prefix GRIB but not a real edition-2 message header padding".to_vec();
    let reader = Grib2Reader::from_bytes(buf).expect("buffer with GRIB substring parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn truncated_message_returns_parse_error() {
    // Take a real message and lop off the trailing half so the IS-declared
    // total length runs past the end of the buffer.
    let mut buf = FIXTURE.to_vec();
    buf.truncate(FIXTURE.len() / 2);

    let Err(err) = Grib2Reader::from_bytes(buf) else {
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

    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("trailer-corrupt buffer must error");
    };
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("7777"),
        "error should mention the 7777 marker, got: {msg}"
    );
}

#[test]
fn wrong_grib_edition_byte_skips_message() {
    // The fixture is GRIB edition 2. Patch the edition octet (byte 7) to 1 so
    // it looks like GRIB1. The scanner is supposed to skip non-edition-2
    // messages forward by one byte rather than panic.
    let mut buf = FIXTURE.to_vec();
    buf[7] = 1; // pretend it's GRIB1
    let reader = Grib2Reader::from_bytes(buf).expect("non-edition-2 buffer parses cleanly");
    assert_eq!(
        reader.message_count(),
        0,
        "GRIB2 reader should ignore edition-1 messages"
    );
}

#[test]
fn implausible_total_length_returns_parse_error() {
    // A well-formed IS magic + edition 2, but a total length (octets 9–16,
    // u64-BE) of 0 — smaller than the IS itself. Must error, not underflow.
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(b"GRIB");
    buf[7] = 2; // edition 2
    // total_length stays 0.
    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("implausible total length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

#[test]
fn decode_for_out_of_range_index_returns_error() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let err = reader
        .decode_message_values(99)
        .expect_err("out-of-range index must error");
    assert!(
        matches!(err, FieldglassError::OutOfRange),
        "expected FieldglassError::OutOfRange, got {err:?}"
    );
}

/// Overwrite the 4-byte big-endian section-length field that starts the section
/// at `section_start` with `new_len`, returning the mutated buffer.
fn with_section_length(base: &[u8], section_start: usize, new_len: u32) -> Vec<u8> {
    let mut buf = base.to_vec();
    buf[section_start..section_start + 4].copy_from_slice(&new_len.to_be_bytes());
    buf
}

/// Scanner regression: a §6 BMS that declares a length larger than the bytes
/// remaining in the message previously advanced the cursor past `msg_end`, so
/// slicing for the following §7 header (`&data[cursor..msg_end]`) panicked on
/// an inverted range. It must surface as a structured parse error instead.
#[test]
fn oversized_bms_section_length_returns_parse_error() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let (bms_start, _) = reader.messages[0].bms_range;

    let buf = with_section_length(FIXTURE, bms_start, u32::MAX);
    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("oversized BMS length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// Scanner regression: a §7 DS that declares a length running past the buffer
/// previously recorded a `ds_range` whose end exceeded `data.len()`, so the
/// decode-time slice `&self.data[ds_start..ds_end]` panicked. Validating the
/// declared length while scanning rejects it up front.
#[test]
fn oversized_ds_section_length_returns_parse_error() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let (ds_start, _) = reader.messages[0].ds_range;

    let buf = with_section_length(FIXTURE, ds_start, u32::MAX);
    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("oversized DS length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// Decode regression (found by the `decode` fuzz target): this message's §3
/// grid template names a 16 × 8388639 ≈ 134-million-point grid — under the
/// MAX_GRID_POINTS cap — while the GDS's own "number of data points" field
/// (and the rest of the file) describes a tiny 16 × 31 grid. The constant-field
/// simple-packing path (`bits_per_value == 0`) then tried to allocate
/// `vec![Some(f64); 134_218_224]` (~2 GiB) for a file carrying no such data,
/// which libFuzzer reported as an out-of-memory. Decode must now reject the
/// dimensions/`num_data_points` mismatch up front. Byte-for-byte the libFuzzer
/// artifact (`fuzz/artifacts/decode/oom-…`), kept as the canonical regression.
#[rustfmt::skip]
const FUZZ_OOM_GRID_NP_MISMATCH: &[u8] = &[
    71, 82, 73, 66, 255, 255, 0, 2, 0, 0, 0, 0, 0, 0, 0, 191, 0, 0, 0, 21, 1, 0, 98, 0, 0, 4, 0, 1,
    7, 215, 3, 23, 12, 0, 0, 0, 2, 0, 0, 0, 84, 3, 0, 0, 0, 1, 240, 0, 0, 0, 1, 6, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 16, 0, 128, 0, 31, 0, 0,
    0, 0, 255, 255, 255, 255, 3, 147, 135, 0, 0, 0, 0, 0, 48, 0, 0, 0, 0, 1, 201, 195, 128, 0, 30,
    132, 128, 0, 30, 132, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 34, 4, 0, 0, 0, 0,
    0, 0, 0, 255, 128, 0, 0, 0, 1, 0, 0, 0, 0, 1, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 0, 0, 0, 21, 5, 0, 0, 1, 240, 0, 0, 67, 136, 147, 51, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6,
    255, 0, 0, 0, 5, 7, 55, 55, 55, 55,
];

#[test]
fn grid_dimensions_disagreeing_with_num_data_points_rejected_before_allocation() {
    let reader =
        Grib2Reader::from_bytes(FUZZ_OOM_GRID_NP_MISMATCH.to_vec()).expect("scan succeeds");
    let err = reader
        .decode_message_values(0)
        .expect_err("dimensions/num_data_points mismatch must error, not OOM");
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("disagree") && msg.contains("data points"),
        "error should name the dimensions/num_data_points mismatch, got: {msg}"
    );
}
