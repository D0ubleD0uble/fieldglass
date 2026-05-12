//! Integration coverage for §1 IDS + §2 LUS parsing on the public ECMWF
//! `reduced_gaussian_pressure_level.grib2` fixture.

use fieldglass_grib2::{
    Grib2Reader, lookup_centre, lookup_data_type, lookup_production_status,
    lookup_reference_time_significance,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

#[test]
fn fixture_ids_fields_decode() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];

    // Sourced from `xxd` of the fixture's IDS (octets 17..=37 of the file).
    assert_eq!(msg.ids.section_length, 21);
    assert_eq!(msg.ids.centre, 98);
    assert_eq!(msg.ids.sub_centre, 0);
    assert_eq!(msg.ids.master_tables_version, 5);
    assert_eq!(msg.ids.local_tables_version, 0);
    assert_eq!(msg.ids.reference_time_significance, 1);
    assert_eq!(msg.ids.year, 2008);
    assert_eq!(msg.ids.month, 2);
    assert_eq!(msg.ids.day, 6);
    assert_eq!(msg.ids.hour, 12);
    assert_eq!(msg.ids.minute, 0);
    assert_eq!(msg.ids.second, 0);
    assert_eq!(msg.ids.production_status, 0);
    assert_eq!(msg.ids.data_type, 255);
}

#[test]
fn fixture_ids_renders_iso8601_reference_time() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    assert_eq!(msg.ids.reference_time_iso8601(), "2008-02-06T12:00:00Z");
}

#[test]
fn fixture_centre_lookup_resolves_ecmwf() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    assert_eq!(
        lookup_centre(msg.ids.centre),
        Some("European Centre for Medium-Range Weather Forecasts (ECMWF)")
    );
}

#[test]
fn fixture_table_lookups_resolve() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    assert_eq!(
        lookup_reference_time_significance(msg.ids.reference_time_significance),
        "Start of forecast"
    );
    assert_eq!(
        lookup_production_status(msg.ids.production_status),
        "Operational products"
    );
    assert_eq!(lookup_data_type(msg.ids.data_type), "Missing");
}

#[test]
fn fixture_includes_local_use_section() {
    // The ECMWF reduced-Gaussian fixture has a §2 LUS following the IDS.
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    let (start, end) = msg.lus_range.expect("fixture has LUS");
    // §2 starts immediately after IS (16) + IDS (21) = byte offset 37.
    assert_eq!(start, 37);
    // Section header places `length` in octets 1..=4 → bytes 37..=40 of the file.
    let declared_len = u32::from_be_bytes([
        FIXTURE[start],
        FIXTURE[start + 1],
        FIXTURE[start + 2],
        FIXTURE[start + 3],
    ]);
    assert_eq!(end - start, declared_len as usize);
    // Section number byte (octet 5) must be 2.
    assert_eq!(FIXTURE[start + 4], 2);
}

// ---------------------------------------------------------------------------
// Synthetic-message coverage for reader paths that the real fixture skips:
// the §2-absent branch and the IDS-related error paths.
// ---------------------------------------------------------------------------

/// Build a minimal valid GRIB2 message: IS + IDS + (optionally) §3 sentinel +
/// ES. Returns the message bytes. The §3 sentinel is just a 5-byte section
/// header advertising section 3 — the reader is supposed to leave it alone
/// since LUS handling stops at "is the next byte section 2?".
fn build_message(include_section3_after_ids: bool) -> Vec<u8> {
    let ids_len: u32 = 21;
    let s3_len: u32 = if include_section3_after_ids { 5 } else { 0 };
    let total_len: u64 = 16 + ids_len as u64 + s3_len as u64 + 4; // IS + IDS + §3? + ES

    let mut buf = Vec::with_capacity(total_len as usize);
    // IS
    buf.extend_from_slice(b"GRIB");
    buf.extend_from_slice(&[0, 0]); // reserved
    buf.push(0); // discipline
    buf.push(2); // edition
    buf.extend_from_slice(&total_len.to_be_bytes());
    // IDS — 21 bytes, section number 1
    buf.extend_from_slice(&ids_len.to_be_bytes());
    buf.push(1); // section number
    buf.extend_from_slice(&98u16.to_be_bytes()); // centre = ECMWF
    buf.extend_from_slice(&0u16.to_be_bytes()); // sub-centre
    buf.push(5); // master tables
    buf.push(0); // local tables
    buf.push(1); // ref-time significance
    buf.extend_from_slice(&2024u16.to_be_bytes()); // year
    buf.push(1); // month
    buf.push(1); // day
    buf.push(0); // hour
    buf.push(0); // minute
    buf.push(0); // second
    buf.push(0); // production status
    buf.push(1); // data type
    if include_section3_after_ids {
        // 5-byte section header advertising §3.
        buf.extend_from_slice(&5u32.to_be_bytes());
        buf.push(3);
    }
    buf.extend_from_slice(b"7777");
    assert_eq!(buf.len() as u64, total_len);
    buf
}

#[test]
fn message_without_lus_parses_with_none_range() {
    // Real GRIB2 messages frequently omit §2; the reader must not require it.
    let bytes = build_message(true);
    let reader = Grib2Reader::from_bytes(bytes).expect("parse synthetic message");
    let msg = &reader.messages[0];
    assert!(msg.lus_range.is_none());
    assert_eq!(msg.ids.centre, 98);
    assert_eq!(msg.ids.year, 2024);
}

#[test]
fn wrong_section_after_is_rejected() {
    // Replace the IDS section-number byte (octet 21 of the file = IS_LEN + 4)
    // with 3. The walker must reject the message rather than mis-classifying.
    let mut bytes = build_message(false);
    bytes[16 + 4] = 3; // section number byte of the would-be IDS
    let err = match Grib2Reader::from_bytes(bytes) {
        Ok(_) => panic!("wrong §1 must error"),
        Err(e) => e,
    };
    let s = err.to_string();
    assert!(s.contains("expected IDS"), "error mentions IDS, got: {s}");
}

#[test]
fn message_with_no_room_for_ids_rejected() {
    // Hand-crafted IS that declares total_length = IS_LEN + ES_LEN, so only
    // 4 bytes remain after the IS — too few for even a section header. The
    // section-header parser rejects it with a "requires 5 bytes" error.
    let total_len: u64 = 16 + 4;
    let mut bytes = Vec::with_capacity(total_len as usize);
    bytes.extend_from_slice(b"GRIB");
    bytes.extend_from_slice(&[0, 0]);
    bytes.push(0);
    bytes.push(2);
    bytes.extend_from_slice(&total_len.to_be_bytes());
    bytes.extend_from_slice(b"7777");

    let err = match Grib2Reader::from_bytes(bytes) {
        Ok(_) => panic!("no-room-for-IDS must error"),
        Err(e) => e,
    };
    let s = err.to_string();
    assert!(
        s.contains("section header requires"),
        "error names the section-header shortage, got: {s}"
    );
}
