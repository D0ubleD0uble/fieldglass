//! Alternate-row ("boustrophedon") scanning is undone by the decoder itself.
//!
//! §3 Flag Table 3.4 bit 4 says adjacent rows scan in opposite directions. It
//! is the one scanning-mode flag that cannot be folded into the projection: the
//! sign flags move where a row *starts*, but this reverses every second row
//! inside the field, so the decoded values themselves have to be put back in
//! raster order. Until #541 that was done by the napi layer after decode, which
//! meant a crates.io consumer calling `from_bytes` → `decode_message_values`
//! got every other row backwards with nothing to indicate it.
//!
//! The oracle is eccodes' own geoiterator: `grib_get_data` applies the same
//! flip in `transform_iterator_data` while pairing values with coordinates, and
//! `alternate_row_lambert_expected.json` records what it printed. eccodes'
//! `values` key does *not* — that is the separate, opt-in
//! `swapScanningAlternativeRows` — which is why the snapshot cross-check in
//! `eccodes_reference.rs` compares against storage order and this test does
//! not. See `tools/build_grib2_alternate_row_fixture.py`.

use fieldglass_grib2::Grib2Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/alternate_row_lambert.grib2");
const ORACLE: &str = include_str!("fixtures/alternate_row_lambert_expected.json");

fn oracle() -> serde_json::Value {
    serde_json::from_str(ORACLE).expect("oracle parses")
}

fn numbers(v: &serde_json::Value, key: &str) -> Vec<f64> {
    v[key]
        .as_array()
        .unwrap_or_else(|| panic!("oracle has no `{key}` array"))
        .iter()
        .map(|n| n.as_f64().expect("oracle entry is a number"))
        .collect()
}

/// Byte offset of §3's scanning-mode octet (octet 65 of the GDS) within a
/// single-message GRIB2 file. Walked rather than hard-coded so the test says
/// what it is doing and survives a fixture rebuild that shifts the sections.
fn scanning_mode_offset(bytes: &[u8]) -> usize {
    let mut off = 16; // past §0
    loop {
        assert!(off + 5 <= bytes.len(), "ran off the end looking for §3");
        assert_ne!(&bytes[off..off + 4], b"7777", "message has no §3");
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        if bytes[off + 4] == 3 {
            return off + 64;
        }
        off += len;
    }
}

/// The decoded field is in raster order, matching eccodes' geoiterator exactly.
///
/// A permutation is invisible to count/min/max/mean, so this compares every
/// point rather than statistics: values `1..=30` in storage order become
/// `1..6, 12..7, 13..18, 24..19, 25..30`, and any other parity, off-by-one or
/// whole-field reversal lands somewhere else.
#[test]
fn boustrophedon_field_decodes_in_raster_order() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let values = reader.decode_message_values(0).expect("decode succeeds");

    let oracle = oracle();
    let want = numbers(&oracle, "regularised");
    let got: Vec<f64> = values
        .iter()
        .map(|v| v.expect("no bitmap, so every point is present"))
        .collect();
    assert_eq!(got, want, "decoded row order disagrees with grib_get_data");

    // The fixture really is scanned the way the oracle assumes, so a rebuild
    // that lost the flag would fail here rather than passing vacuously.
    let sm = reader.messages[0]
        .gds
        .scanning_mode()
        .expect("Lambert grid states a scanning mode");
    assert_eq!(u64::from(sm), oracle["scanningMode"].as_u64().unwrap());
    assert_eq!(sm & fieldglass_grib2::SCAN_ALTERNATE_ROWS, 0x10);
}

/// …and it is not simply the stored order.
///
/// The assertion above would pass just as well if the oracle had recorded the
/// storage order and the decoder did nothing, so pin the difference: the two
/// orders are the same multiset and differ in exactly the 12 points that sit on
/// the two odd rows.
#[test]
fn the_flip_is_not_a_no_op() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let values = reader.decode_message_values(0).expect("decode succeeds");

    let oracle = oracle();
    let stored = numbers(&oracle, "stored");
    let regularised = numbers(&oracle, "regularised");
    assert_ne!(
        stored, regularised,
        "the fixture's rows are already in order"
    );

    let (ni, nj) = (
        oracle["ni"].as_u64().unwrap() as usize,
        oracle["nj"].as_u64().unwrap() as usize,
    );
    let moved = stored
        .iter()
        .zip(&regularised)
        .filter(|(a, b)| a != b)
        .count();
    assert_eq!(moved, ni * (nj / 2), "only the odd rows should have moved");

    // Row 0 is stored in the nominal direction and must be untouched; row 1 is
    // the one that is stored east-to-west.
    assert_eq!(
        values[..ni],
        stored[..ni].iter().copied().map(Some).collect::<Vec<_>>()[..]
    );
    let mut row1: Vec<Option<f64>> = stored[ni..2 * ni].iter().copied().map(Some).collect();
    row1.reverse();
    assert_eq!(values[ni..2 * ni], row1[..]);
}

/// A grid that alternates its rows *and* stores columns is refused, not
/// silently mis-ordered.
///
/// §3 bit 3 (j-consecutive) makes the stored run a column, so the reversal bit
/// 4 asks for is not a contiguous slice of the decoded field and the row flip
/// would scramble it. The napi layer used to skip the flip in that case and
/// return the raw field, which is indistinguishable to a caller from a correct
/// decode. No real message sets both; this patches the fixture's scanning-mode
/// octet to make one.
#[test]
fn alternate_rows_with_j_consecutive_is_rejected() {
    let mut bytes = FIXTURE.to_vec();
    let off = scanning_mode_offset(&bytes);
    assert_eq!(
        bytes[off], 80,
        "expected the fixture's scanning mode at {off}"
    );
    bytes[off] |= fieldglass_grib2::SCAN_J_CONSECUTIVE;

    let reader = Grib2Reader::from_bytes(bytes).expect("still parses");
    assert_eq!(reader.messages[0].gds.scanning_mode(), Some(80 | 0x20));
    let err = reader
        .decode_message_values(0)
        .expect_err("a layout this cannot regularise must not decode");
    let text = err.to_string();
    assert!(
        text.contains("alternate-row") && text.contains("j-consecutive"),
        "error should name both flags, got: {text}"
    );
}
