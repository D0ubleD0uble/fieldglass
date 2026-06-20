//! A GRIB1 message with no Grid Description Section is identified by its
//! `grid_number` (WMO ON388 Table B). The reader should resolve that number to
//! the predefined grid's geometry so dimensions and bounds still report.
//!
//! The message is hand-assembled to the section structure only — IS + a 28-byte
//! PDS with the GDS-present flag clear and `grid_number = 2`, a stub BDS, and
//! the `7777` end marker. The scanner parses structure without decoding the
//! BDS, so the stub body is never interpreted.

use fieldglass_grib1::Grib1Reader;

/// Build a single-message GRIB1 stream with no GDS and the given grid number.
fn message_without_gds(grid_number: u8) -> Vec<u8> {
    const BDS_LEN: usize = 12; // opaque stub; the scanner never decodes it
    let total_len = 8 + 28 + BDS_LEN + 4;

    let mut msg = Vec::with_capacity(total_len);
    // Indicator Section: "GRIB", 3-byte total length, edition 1.
    msg.extend_from_slice(b"GRIB");
    msg.extend_from_slice(&[
        (total_len >> 16) as u8,
        (total_len >> 8) as u8,
        total_len as u8,
    ]);
    msg.push(1);

    // Product Definition Section (28 bytes). Only the fields the scanner and
    // predefined lookup read are set; the rest stay zero.
    let mut pds = [0u8; 28];
    pds[0..3].copy_from_slice(&[0, 0, 28]); // section length
    pds[6] = grid_number; // octet 7: grid number
    pds[7] = 0x00; // octet 8: section1 flags — no GDS, no BMS
    msg.extend_from_slice(&pds);

    // Stub BDS + end section.
    msg.extend_from_slice(&[0u8; BDS_LEN]);
    msg.extend_from_slice(b"7777");
    msg
}

#[test]
fn gds_absent_message_resolves_predefined_grid_2() {
    let bytes = message_without_gds(2);
    let reader = Grib1Reader::from_bytes(bytes).expect("message parses");
    let gds = reader.messages[0]
        .gds
        .as_ref()
        .expect("predefined grid 2 fills in the absent GDS");
    assert_eq!(gds.grid_type_name(), "latlon");
    assert_eq!(gds.dimensions(), Some((144, 73)));
    assert_eq!(gds.bounds(), Some((90.0, 0.0, -90.0, 357.5)));
}

#[test]
fn gds_absent_with_unknown_grid_number_stays_none() {
    // grid_number 255 = "no predefined grid"; the message keeps no geometry.
    let bytes = message_without_gds(255);
    let reader = Grib1Reader::from_bytes(bytes).expect("message parses");
    assert!(reader.messages[0].gds.is_none());
}
