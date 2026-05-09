//! Tests for `Grib1Reader::from_bytes` / `scan_messages` — the top-level
//! scanner that walks IS total-length offsets to locate every GRIB1 message
//! in a buffer. Uses the real CMC fixture as a building block so each
//! synthetic case is exercised against a genuinely-shaped message.

use fieldglass_grib1::Grib1Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib");

#[test]
fn scans_two_concatenated_messages() {
    let mut buf = Vec::with_capacity(FIXTURE.len() * 2);
    buf.extend_from_slice(FIXTURE);
    buf.extend_from_slice(FIXTURE);

    let reader = Grib1Reader::from_bytes(buf).expect("two-message buffer parses");
    assert_eq!(reader.message_count(), 2);
    assert_eq!(reader.messages[0].byte_offset, 0);
    assert_eq!(reader.messages[1].byte_offset, FIXTURE.len());

    // Both messages decode to the same field.
    let a = reader.decode_message_values(0).expect("decode first");
    let b = reader.decode_message_values(1).expect("decode second");
    assert_eq!(a, b);
}

#[test]
fn skips_leading_non_grib_bytes() {
    let mut buf = b"junk before the real message: ".to_vec();
    let prefix_len = buf.len();
    buf.extend_from_slice(FIXTURE);

    let reader = Grib1Reader::from_bytes(buf).expect("parses past leading junk");
    assert_eq!(reader.message_count(), 1);
    assert_eq!(reader.messages[0].byte_offset, prefix_len);
}

#[test]
fn rejects_message_missing_7777_trailer() {
    let mut buf = FIXTURE.to_vec();
    let len = buf.len();
    // Corrupt the End Section so the trailer no longer reads "7777".
    buf[len - 4..].copy_from_slice(b"XXXX");

    let err = match Grib1Reader::from_bytes(buf) {
        Err(e) => e,
        Ok(_) => panic!("missing 7777 should fail parse"),
    };
    assert!(
        err.to_string().contains("7777"),
        "error should mention the missing trailer; got: {err}",
    );
}

#[test]
fn pds_p1_offset_round_trips_through_byte_buffer() {
    // The fixture's parsed `p1` value should equal the byte at `pds_p1_offset()`
    // — and patching that byte should be visible after re-parsing.
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let off = msg.pds_p1_offset();
    let original_p1 = msg.pds.p1;
    assert_eq!(
        FIXTURE[off], original_p1,
        "byte at offset must equal parsed p1"
    );

    let mut patched = FIXTURE.to_vec();
    patched[off] = original_p1.wrapping_add(7);
    let reparsed = Grib1Reader::from_bytes(patched).expect("patched fixture parses");
    assert_eq!(reparsed.messages[0].pds.p1, original_p1.wrapping_add(7));
}

#[test]
fn rejects_truncated_message_shorter_than_declared_length() {
    // Drop the last byte: IS still claims total_length = FIXTURE.len(), but
    // only FIXTURE.len() - 1 bytes remain.
    let truncated = FIXTURE[..FIXTURE.len() - 1].to_vec();
    let err = match Grib1Reader::from_bytes(truncated) {
        Err(e) => e,
        Ok(_) => panic!("truncated buffer should fail parse"),
    };
    let msg = err.to_string();
    assert!(
        msg.contains("claims length") || msg.contains("only"),
        "error should mention the length mismatch; got: {msg}",
    );
}
