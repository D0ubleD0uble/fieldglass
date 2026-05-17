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

/// Build a minimal valid GRIB2 message: IS + IDS + minimal §3 GDS + minimal
/// §4 PDS + ES. `include_lus` controls whether a tiny empty §2 LUS is
/// inserted between the IDS and the GDS, exercising the LUS-present and
/// LUS-absent reader paths off a single helper.
fn build_message(include_lus: bool) -> Vec<u8> {
    let ids_len: u32 = 21;
    let lus_len: u32 = if include_lus { 5 } else { 0 };
    let gds_len: u32 = 72; // §3 template 3.0 — 5-byte header + 67-byte body
    let pds_len: u32 = 34; // §4 template 4.0 — 9-byte header + 25-byte payload
    let drs_len: u32 = 21; // §5 template 5.0 — 11-byte header + 10-byte payload
    let bms_len: u32 = 6; // §6 — 5-byte header + 1-byte indicator (255 = no bitmap)
    let ds_len: u32 = 6; // §7 — 5-byte header + 1 dummy byte (1-point grid, 8 bits/value)
    let total_len: u64 = 16
        + ids_len as u64
        + lus_len as u64
        + gds_len as u64
        + pds_len as u64
        + drs_len as u64
        + bms_len as u64
        + ds_len as u64
        + 4;

    let mut buf = Vec::with_capacity(total_len as usize);
    // IS
    buf.extend_from_slice(b"GRIB");
    buf.extend_from_slice(&[0, 0]);
    buf.push(0); // discipline
    buf.push(2); // edition
    buf.extend_from_slice(&total_len.to_be_bytes());
    // IDS
    buf.extend_from_slice(&ids_len.to_be_bytes());
    buf.push(1);
    buf.extend_from_slice(&98u16.to_be_bytes());
    buf.extend_from_slice(&0u16.to_be_bytes());
    buf.push(5);
    buf.push(0);
    buf.push(1);
    buf.extend_from_slice(&2024u16.to_be_bytes());
    buf.push(1);
    buf.push(1);
    buf.push(0);
    buf.push(0);
    buf.push(0);
    buf.push(0);
    buf.push(1);
    if include_lus {
        // Empty §2.
        buf.extend_from_slice(&5u32.to_be_bytes());
        buf.push(2);
    }
    // §3 GDS — template 3.0, 1-point lat/lon at the equator/prime meridian.
    buf.extend_from_slice(&gds_len.to_be_bytes());
    buf.push(3); // section number
    buf.push(0); // source
    buf.extend_from_slice(&1u32.to_be_bytes()); // num data points
    buf.push(0); // optional list octet size
    buf.push(0); // interp
    buf.extend_from_slice(&0u16.to_be_bytes()); // template = 3.0
    // Template 3.0 payload (58 bytes).
    let mut payload = vec![0u8; 58];
    payload[0] = 6; // shape_of_earth
    payload[16..20].copy_from_slice(&1u32.to_be_bytes()); // Ni
    payload[20..24].copy_from_slice(&1u32.to_be_bytes()); // Nj
    // La1, Lo1, La2, Lo2 left at 0.
    payload[49..53].copy_from_slice(&1_000_000u32.to_be_bytes()); // Di = 1°
    payload[53..57].copy_from_slice(&1_000_000u32.to_be_bytes()); // Dj
    buf.extend_from_slice(&payload);
    // §4 PDS — template 4.0 with an all-zero horizontal common block.
    buf.extend_from_slice(&pds_len.to_be_bytes());
    buf.push(4); // section number
    buf.extend_from_slice(&0u16.to_be_bytes()); // NV
    buf.extend_from_slice(&0u16.to_be_bytes()); // template 4.0
    buf.extend_from_slice(&[0u8; 25]); // horizontal common
    // §5 DRS — template 5.0, R=0, E=0, D=0, 8 bits/value.
    buf.extend_from_slice(&drs_len.to_be_bytes());
    buf.push(5); // section number
    buf.extend_from_slice(&1u32.to_be_bytes()); // num data points
    buf.extend_from_slice(&0u16.to_be_bytes()); // template 5.0
    buf.extend_from_slice(&0.0_f32.to_be_bytes()); // R
    buf.extend_from_slice(&0u16.to_be_bytes()); // E
    buf.extend_from_slice(&0u16.to_be_bytes()); // D
    buf.push(8); // bits per value
    buf.push(0); // original field type
    // §6 BMS — indicator 255 = no bitmap.
    buf.extend_from_slice(&bms_len.to_be_bytes());
    buf.push(6); // section number
    buf.push(255); // no bitmap
    // §7 DS — 1 byte of packed data (one 8-bit value = 0).
    buf.extend_from_slice(&ds_len.to_be_bytes());
    buf.push(7); // section number
    buf.push(0); // packed value X = 0 → decoded value = R + 0 = 0
    buf.extend_from_slice(b"7777");
    assert_eq!(buf.len() as u64, total_len);
    buf
}

#[test]
fn message_without_lus_parses_with_none_range() {
    // Real GRIB2 messages frequently omit §2; the reader must not require it.
    let bytes = build_message(false);
    let reader = Grib2Reader::from_bytes(bytes).expect("parse synthetic message");
    let msg = &reader.messages[0];
    assert!(msg.lus_range.is_none());
    assert_eq!(msg.ids.centre, 98);
    assert_eq!(msg.ids.year, 2024);
    assert_eq!(msg.gds.template_number, 0);
}

#[test]
fn message_with_lus_parses_both_lus_and_gds() {
    let bytes = build_message(true);
    let reader = Grib2Reader::from_bytes(bytes).expect("parse synthetic message");
    let msg = &reader.messages[0];
    assert!(msg.lus_range.is_some());
    assert_eq!(msg.gds.template_number, 0);
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
fn wrong_section_in_place_of_gds_rejected() {
    // §3 is required immediately after §1/§2. Flip the GDS section-number
    // byte to claim it's actually §4 — the walker must reject it with a
    // §3-specific error rather than mis-classifying the section.
    let mut bytes = build_message(false);
    // §3 starts at IS_LEN (16) + IDS_LEN (21) = 37; its section-number byte
    // is at offset 37 + 4 = 41.
    bytes[41] = 4;
    let err = match Grib2Reader::from_bytes(bytes) {
        Ok(_) => panic!("wrong §3 must error"),
        Err(e) => e,
    };
    let s = err.to_string();
    assert!(s.contains("expected GDS"), "error mentions GDS, got: {s}");
}

#[test]
fn wrong_section_in_place_of_pds_rejected() {
    // §4 is required after §3. Flip the PDS section-number byte to claim
    // it's §5 — the walker must reject with a §4-specific error.
    let mut bytes = build_message(false);
    // §4 starts at IS_LEN (16) + IDS_LEN (21) + GDS_LEN (72) = 109; its
    // section-number byte is at offset 109 + 4 = 113.
    bytes[113] = 5;
    let err = match Grib2Reader::from_bytes(bytes) {
        Ok(_) => panic!("wrong §4 must error"),
        Err(e) => e,
    };
    let s = err.to_string();
    assert!(s.contains("expected PDS"), "error mentions PDS, got: {s}");
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
