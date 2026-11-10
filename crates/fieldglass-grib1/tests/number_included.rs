//! N, the number of products a statistic combines (PDS octets 22-23), and the
//! display that reads it (#1623).

use fieldglass_grib1::{Grib1Reader, forecast_display, forecast_display_with_number_included};

const FIXTURE: &[u8] = include_bytes!("fixtures/j_consecutive_latlon.grib1");

/// The fixture with PDS octets 19-21 (P1, P2, time range) and 22-23 (N)
/// restamped, after `lead` bytes that are no GRIB message. Section 0 is 8
/// octets, so PDS octet n sits at 8 + n - 1 from the message's start.
fn stamped(lead: usize, time_range: u8, p1: u8, p2: u8, n: [u8; 2]) -> Vec<u8> {
    let mut bytes = vec![0u8; lead];
    bytes.extend_from_slice(FIXTURE);
    let at = |octet: usize| lead + 8 + octet - 1;
    bytes[at(19)] = p1;
    bytes[at(20)] = p2;
    bytes[at(21)] = time_range;
    bytes[at(22)..=at(23)].copy_from_slice(&n);
    bytes
}

#[test]
fn n_is_the_16_bit_value_of_octets_22_and_23() {
    // Octet 22 is the high byte. The fixture leaves both at 0, as a message
    // that is no statistic writes them.
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("reader");
    assert_eq!(reader.number_included(0).expect("reads"), 0);
    let reader = Grib1Reader::from_bytes(stamped(0, 113, 6, 6, [0x01, 0x02])).expect("reader");
    assert_eq!(reader.number_included(0).expect("reads"), 0x0102);
    let reader = Grib1Reader::from_bytes(stamped(0, 113, 6, 6, [0xFF, 0xFF])).expect("reader");
    assert_eq!(reader.number_included(0).expect("reads"), 0xFFFF);
}

#[test]
fn n_is_read_where_the_message_starts_in_the_file() {
    // A message that does not start the file, found by the scan or named by
    // its offset, reads its own octets rather than the file's first ones.
    let bytes = stamped(13, 113, 6, 0, [0x00, 0x01]);
    let scanned = Grib1Reader::from_bytes(bytes.clone()).expect("reader");
    assert_eq!(scanned.messages[0].byte_offset, 13);
    assert_eq!(scanned.number_included(0).expect("reads"), 1);
    let at = Grib1Reader::from_message_at(bytes, 13).expect("reader");
    assert_eq!(at.number_included(0).expect("reads"), 1);
}

#[test]
fn an_index_past_the_end_is_an_error() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("reader");
    assert!(reader.number_included(1).is_err());
}

#[test]
fn only_the_display_given_n_reads_one_product() {
    let reader = Grib1Reader::from_bytes(stamped(0, 113, 6, 0, [0, 1])).expect("reader");
    let pds = &reader.messages[0].pds;
    let n = reader.number_included(0).expect("reads");
    assert_eq!(
        forecast_display_with_number_included(pds, n),
        "average of one +6h forecast"
    );
    // `forecast_display` has no N, so it shows the step of 0 raw, as 0.6.1
    // did, and so does the hours number.
    assert_eq!(forecast_display(pds), "time range 113, P1 6h, P2 0h");
    assert_eq!(fieldglass_grib1::offset_hours(pds), None);
    // Any other N shows what `forecast_display` shows.
    for n in [0u16, 2, 0xFFFF] {
        assert_eq!(
            forecast_display_with_number_included(pds, n),
            forecast_display(pds),
            "N {n}"
        );
    }
}
