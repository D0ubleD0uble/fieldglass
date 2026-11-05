//! `MessageInfo.forecast_hours` keeps the values and the `None` set 0.6.0 gave
//! it, and `MessageInfo.offset_hours` carries the time-range-aware number
//! beside it (#1694).
//!
//! 0.6.0 filled `forecast_hours` from each format crate's `forecast_hours`:
//! for GRIB1, P1 in hours whatever the time-range indicator says; for GRIB2,
//! the forecast time in hours, truncated toward zero, with no hours for a
//! unit of no fixed length. Each case restamps a committed fixture and reads
//! it through [`Session::message`], the path every host takes.

use fieldglass::{MessageInfo, Session};

const GRIB1: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");
const GRIB2: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/complex_regular_latlon.grib2");
/// A template 4.44 message whose forecast time is 70,000 minutes, 1166⅔
/// hours: the one committed fixture whose lead is not whole hours.
const GRIB2_70000_MIN: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/constituent_4_44_four_octet_time.grib2");

/// DWD, whose Code Table 5 names code 13.
const CENTRE_DWD: u8 = 78;

fn message(bytes: Vec<u8>) -> MessageInfo {
    Session::open(bytes)
        .expect("opens")
        .message(0)
        .expect("message 0")
}

/// The GRIB1 fixture with PDS octets 5 (center) and 18-21 (unit, P1, P2,
/// time range) restamped. Section 0 is 8 octets, so PDS octet n sits at
/// 8 + n - 1.
fn grib1(centre: u8, unit: u8, p1: u8, p2: u8, time_range: u8) -> MessageInfo {
    let mut bytes = GRIB1.to_vec();
    bytes[8 + 5 - 1] = centre;
    bytes[8 + 18 - 1] = unit;
    bytes[8 + 19 - 1] = p1;
    bytes[8 + 20 - 1] = p2;
    bytes[8 + 21 - 1] = time_range;
    message(bytes)
}

/// The GRIB2 fixture, template 4.0, with its unit of time range (§4 octet
/// 18) and forecast time (octets 19-22) restamped.
fn grib2(unit: u8, forecast_time: u32) -> MessageInfo {
    let mut bytes = GRIB2.to_vec();
    let s4 = section_start(&bytes, 4);
    assert_eq!(
        (bytes[s4 + 7], bytes[s4 + 8]),
        (0, 0),
        "fixture precondition: template 4.0"
    );
    bytes[s4 + 17] = unit;
    bytes[s4 + 18..s4 + 22].copy_from_slice(&forecast_time.to_be_bytes());
    message(bytes)
}

/// The byte offset of GRIB2 section `number`, walking the section lengths
/// from the end of the 16-octet §0.
fn section_start(bytes: &[u8], number: u8) -> usize {
    let mut at = 16;
    loop {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if bytes[at + 4] == number {
            return at;
        }
        at += len;
    }
}

/// A GRIB1 message always states P1, so 0.6.0 always had hours for it in a
/// unit of fixed length, whatever the time range: an unnamed code, a
/// statistic whose step is zero, an analysis, a period before the reference
/// time. `offset_hours` keeps what `forecast_hours` reported before #1694.
#[test]
fn grib1_forecast_hours_is_p1_in_hours_whatever_the_time_range() {
    // (what, center, time range, P1, P2, forecast_hours, offset_hours)
    let cases = [
        ("unnamed 8", 98u8, 8u8, 6u8, 18u8, 6, None),
        ("unnamed 200", 98, 200, 12, 0, 12, None),
        ("WMO 123 with P2 = 0", 98, 123, 6, 0, 6, None),
        ("WMO 1, an analysis", 98, 1, 6, 0, 6, Some(0)),
        ("WMO 51, a climatological mean", 98, 51, 6, 0, 6, Some(0)),
        ("WMO 6, from −P1 to −P2", 98, 6, 18, 6, 18, Some(-18)),
        ("WMO 7, from −P1 to +P2", 98, 7, 6, 18, 6, Some(-6)),
        ("DWD 13, an analysis", CENTRE_DWD, 13, 6, 0, 6, Some(0)),
        ("WMO 0, a forecast", 98, 0, 24, 0, 24, Some(24)),
    ];
    for (what, centre, code, p1, p2, hours, offset) in cases {
        let info = grib1(centre, 1, p1, p2, code);
        assert_eq!(info.forecast_hours, Some(hours), "{what}");
        assert_eq!(info.offset_hours, offset, "{what}");
    }
}

/// GRIB2 in 0.6.0: a sub-hour lead truncated toward zero, and no hours for a
/// lead in a month, even a lead of 0. `offset_hours` rounds, and has the
/// reference time's 0 for a zero lead in any unit that is not missing.
#[test]
fn grib2_forecast_hours_truncates_and_has_no_hours_for_a_calendar_unit() {
    // (what, unit, forecast time, forecast_hours, offset_hours)
    let cases = [
        ("30 minutes", 0u8, 30u32, Some(0), Some(1)),
        ("90 minutes", 0, 90, Some(1), Some(2)),
        ("0 months", 3, 0, None, Some(0)),
        ("6 months", 3, 6, None, None),
        ("24 hours", 1, 24, Some(24), Some(24)),
    ];
    for (what, unit, value, hours, offset) in cases {
        let info = grib2(unit, value);
        assert_eq!(info.forecast_hours, hours, "{what}");
        assert_eq!(info.offset_hours, offset, "{what}");
    }

    // The committed fixture as it is: 0.6.0 reported 1166.
    let info = message(GRIB2_70000_MIN.to_vec());
    assert_eq!(info.forecast.as_deref(), Some("+70000min"));
    assert_eq!(info.forecast_hours, Some(1_166));
    assert_eq!(info.offset_hours, Some(1_167));
}

/// The new key crosses the wire beside the old one, as `offsetHours`.
#[test]
fn offset_hours_is_on_the_wire_as_offset_hours() {
    let info = grib1(98, 1, 6, 0, 1);
    let json = serde_json::to_value(&info).expect("serializes");
    assert_eq!(json["forecastHours"], 6);
    assert_eq!(json["offsetHours"], 0);
}

/// `lead_hours.grib1`, the conformance suite's fixture for the two numbers,
/// is what `tools/build_conformance_lead_time_fixture.py` writes: the GRIB1
/// fixture twice, restamped to an analysis at P1 6 and to the unnamed time
/// range 200 at P1 12. Its messages read as the suite records them.
#[test]
fn the_lead_hours_fixture_is_the_builders_output() {
    let restamp = |p1: u8, time_range: u8| {
        let mut bytes = GRIB1.to_vec();
        bytes[8 + 19 - 1] = p1;
        bytes[8 + 21 - 1] = time_range;
        bytes
    };
    let built = [restamp(6, 1), restamp(12, 200)].concat();
    let committed = include_bytes!("fixtures/lead_hours.grib1");
    assert_eq!(
        built, committed,
        "rerun tools/build_conformance_lead_time_fixture.py"
    );

    let session = Session::open(committed.to_vec()).expect("opens");
    assert_eq!(session.count(), 2);
    let analysis = session.message(0).expect("message 0");
    assert_eq!(
        (analysis.forecast_hours, analysis.offset_hours),
        (Some(6), Some(0))
    );
    let unnamed = session.message(1).expect("message 1");
    assert_eq!(
        (unnamed.forecast_hours, unnamed.offset_hours),
        (Some(12), None)
    );
}
