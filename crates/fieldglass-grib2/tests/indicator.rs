//! Integration tests for the GRIB2 Indicator Section parser and the
//! top-level [`Grib2Reader`] message scanner.
//!
//! Fixture: `reduced_gaussian_pressure_level.grib2`, sourced from the public
//! ECMWF eccodes test data corpus
//! (`https://get.ecmwf.int/test-data/eccodes/data/`).

use fieldglass_grib2::{Grib2Reader, INDICATOR_SECTION_LEN, lookup_discipline, parse_indicator};

const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

#[test]
fn parses_fixture_indicator_section() {
    let is = parse_indicator(FIXTURE).expect("parse IS");
    assert_eq!(is.edition, 2);
    // The reduced-Gaussian fixture is a meteorological pressure-level field.
    assert_eq!(is.discipline, 0);
    assert_eq!(lookup_discipline(is.discipline), "Meteorological products");
    assert_eq!(is.total_length, FIXTURE.len() as u64);
}

#[test]
fn reader_enumerates_single_message() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    assert_eq!(reader.message_count(), 1);
    let msg = &reader.messages[0];
    assert_eq!(msg.message_index, 0);
    assert_eq!(msg.byte_offset, 0);
    assert_eq!(msg.is.edition, 2);
    assert_eq!(msg.is.discipline, 0);
    assert_eq!(msg.is.total_length, FIXTURE.len() as u64);
}

#[test]
fn reader_enumerates_concatenated_blob() {
    // Two back-to-back copies of the fixture must surface as two messages.
    let mut blob = Vec::with_capacity(FIXTURE.len() * 2);
    blob.extend_from_slice(FIXTURE);
    blob.extend_from_slice(FIXTURE);

    let reader = Grib2Reader::from_bytes(blob).expect("read concatenated blob");
    assert_eq!(reader.message_count(), 2);

    assert_eq!(reader.messages[0].byte_offset, 0);
    assert_eq!(reader.messages[0].message_index, 0);

    assert_eq!(reader.messages[1].byte_offset, FIXTURE.len());
    assert_eq!(reader.messages[1].message_index, 1);

    for msg in &reader.messages {
        assert_eq!(msg.is.edition, 2);
        assert_eq!(msg.is.total_length, FIXTURE.len() as u64);
    }
}

#[test]
fn rejects_edition_mismatch_on_direct_parse() {
    let mut bytes = [0u8; 16];
    bytes[0..4].copy_from_slice(b"GRIB");
    bytes[6] = 0; // discipline
    bytes[7] = 1; // edition — wrong
    bytes[8..16].copy_from_slice(&16u64.to_be_bytes());

    let err = parse_indicator(&bytes).expect_err("must reject edition 1");
    let msg = err.to_string();
    assert!(
        msg.contains("edition"),
        "error should mention edition, got: {msg}"
    );
}

#[test]
fn reader_skips_non_grib2_magic_runs() {
    // A GRIB1 message body sharing the same `GRIB` magic (edition byte = 1)
    // must not be treated as GRIB2 — the scanner skips past it.
    let mut blob = Vec::new();
    let mut grib1 = [0u8; INDICATOR_SECTION_LEN];
    grib1[0..4].copy_from_slice(b"GRIB");
    grib1[7] = 1;
    blob.extend_from_slice(&grib1);
    blob.extend_from_slice(FIXTURE);

    let reader = Grib2Reader::from_bytes(blob).expect("read mixed blob");
    assert_eq!(reader.message_count(), 1);
    assert_eq!(reader.messages[0].is.edition, 2);
}

#[test]
fn truncated_message_returns_parse_error() {
    // Drop the trailing bytes of the fixture so the declared total_length
    // exceeds the buffer.
    let truncated = &FIXTURE[..FIXTURE.len() - 100];
    let err = match Grib2Reader::from_bytes(truncated.to_vec()) {
        Ok(_) => panic!("truncated body must error"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("only") || msg.contains("remain") || msg.contains("length"),
        "error should mention truncation, got: {msg}"
    );
}

#[test]
fn missing_end_marker_returns_parse_error() {
    // Corrupt the trailing "7777" so the End Section check fires.
    let mut blob = FIXTURE.to_vec();
    let n = blob.len();
    blob[n - 1] = b'X';

    let err = match Grib2Reader::from_bytes(blob) {
        Ok(_) => panic!("bad ES must error"),
        Err(e) => e,
    };
    let msg = err.to_string();
    assert!(
        msg.contains("7777"),
        "error should mention 7777, got: {msg}"
    );
}
