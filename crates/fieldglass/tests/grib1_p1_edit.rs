//! Rewriting a GRIB1 message's `P1` octet through the umbrella (#726).
//!
//! The edit used to live in `fieldglass-napi`, which reached the reader's own
//! buffer and its `pds_p1_offset`. That is format knowledge at a binding layer,
//! and it is the last thing keeping the GRIB1 handle tied to a reader it should
//! not hold.

use fieldglass::{Session, grib1::with_p1_octet};

/// A fixture whose time-range indicator makes `P1` the lead time outright, so
/// the octet and the reported forecast are the same number.
///
/// **Not every GRIB1 message is like that**, which is worth knowing before
/// using this function. Time range 10 spends `P1` as the *high* octet of a
/// two-octet value, so setting it to 24 on such a file moves the forecast to
/// `24 × 256 + P2`. The CMC fixture is exactly that case — I picked it first and
/// it read back 6,156 hours. `MessageInfo::p1_octet` is `Some` only where the
/// octet really is the lead time, which is how to tell the two apart.
const FIXTURE: &str = "../fieldglass-grib1/tests/fixtures/hand_second_order_SPD1.grib1";

#[test]
fn the_edit_changes_one_octet_and_the_reread_reports_it() {
    let before = std::fs::read(FIXTURE).expect("the fixture");
    let original = Session::open(before.clone())
        .expect("opens")
        .message(0)
        .expect("a message")
        .forecast_hours;
    assert_eq!(original, Some(24), "the fixture's declared lead time");

    let after = with_p1_octet(&before, 0, 36).expect("the edit");

    // Exactly one octet, which is the claim that keeps this from being a
    // re-encode: everything a reader indexed still sits where it sat.
    assert_eq!(after.len(), before.len(), "the file's length is unchanged");
    let changed: Vec<usize> = before
        .iter()
        .zip(&after)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(changed.len(), 1, "one octet changed, at {changed:?}");
    assert_eq!(after[changed[0]], 36);

    // And the file means what the octet says.
    let reread = Session::open(after).expect("the edited file still opens");
    let edited = reread.message(0).expect("a message");
    assert_eq!(edited.forecast_hours, Some(36));
    assert_eq!(edited.p1_octet, Some(36));
}

/// The refusals, each naming what was wrong.
#[test]
fn it_refuses_an_index_past_the_end_and_a_value_past_an_octet() {
    let bytes = std::fs::read(FIXTURE).expect("the fixture");

    let err = with_p1_octet(&bytes, 99, 12).expect_err("index 99 is past the end");
    assert!(
        format!("{err:?}").contains("NoSuchMessage"),
        "the index is reported as the problem: {err:?}"
    );

    let err = with_p1_octet(&bytes, 0, 256).expect_err("256 does not fit an octet");
    let text = err.to_string();
    assert!(
        text.contains("256") && text.contains("octet"),
        "the value and its limit are both named: {text}"
    );

    // Bytes holding no GRIB1 message at all. The reader indexes them happily
    // and finds nothing — it scans for the `GRIB` marker rather than requiring
    // the file to start with one — so the honest answer is "no message 0", and
    // the count says how many there were. Asserted because I expected a decode
    // failure and it is not one.
    let err = with_p1_octet(b"not a grib file at all", 0, 12).expect_err("no messages");
    assert!(
        format!("{err:?}").contains("NoSuchMessage { index: 0, count: 0 }"),
        "no messages to edit, and the count says so: {err:?}"
    );
}
