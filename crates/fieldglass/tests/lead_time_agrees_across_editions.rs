//! The same lead time reads the same in both GRIB editions (#1565).
//!
//! GRIB1 and GRIB2 state a lead as a number and a unit code, from two
//! different unit tables. Before #1565 the two editions spelled a sub-hour
//! lead differently (`+30min` against `+30 Minute`), and rounded the hours
//! number differently (GRIB1 to the nearest hour, GRIB2 toward zero), so one
//! 30-minute step had a lead of 1 hour in one edition and 0 in the other.
//!
//! Each case here states one lead in each edition's own units, restamps the
//! committed fixtures with it, and reads both through [`Session::message`],
//! the path every host takes.

use fieldglass::{MessageInfo, Session};

const GRIB1: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");
const GRIB2: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/complex_regular_latlon.grib2");

fn message(bytes: Vec<u8>) -> MessageInfo {
    Session::open(bytes)
        .expect("opens")
        .message(0)
        .expect("message 0")
}

/// A GRIB1 message at `centre` with PDS octets 18-21 (unit, P1, P2, time
/// range) restamped. Section 0 is 8 octets, so PDS octet n sits at 8 + n - 1.
fn grib1(centre: u8, unit: u8, p1: u8, p2: u8, time_range: u8) -> MessageInfo {
    let mut bytes = GRIB1.to_vec();
    bytes[8 + 5 - 1] = centre;
    bytes[8 + 18 - 1] = unit;
    bytes[8 + 19 - 1] = p1;
    bytes[8 + 20 - 1] = p2;
    bytes[8 + 21 - 1] = time_range;
    message(bytes)
}

/// A plain GRIB1 forecast (time range 0) of `p1` in Table 4 unit `unit`.
fn grib1_forecast(unit: u8, p1: u8) -> MessageInfo {
    grib1(98, unit, p1, 0, 0)
}

/// The GRIB2 fixture, whose §4 is template 4.0, with its unit of time range
/// (§4 octet 18, Code Table 4.4) and forecast time (octets 19-22) restamped.
fn grib2_forecast(unit: u8, forecast_time: u32) -> MessageInfo {
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

/// Every lead stated in both editions' units reads the same text and the same
/// hours number. The GRIB1 and GRIB2 unit codes differ for the same unit (a
/// second is 254 in GRIB1 and 13 in GRIB2, where GRIB1's 13 is fifteen
/// minutes), which is why each case names both.
#[test]
fn equal_leads_read_the_same_in_both_editions() {
    // (what, GRIB1 message, GRIB2 message, text, hours)
    let cases = [
        (
            "30 minutes",
            grib1_forecast(0, 30),
            grib2_forecast(0, 30),
            "+30min",
            Some(1),
        ),
        (
            "45 minutes as 3 × 15",
            grib1_forecast(13, 3),
            grib2_forecast(0, 45),
            "+45min",
            Some(1),
        ),
        (
            "15 minutes",
            grib1_forecast(13, 1),
            grib2_forecast(0, 15),
            "+15min",
            Some(0),
        ),
        (
            "90 minutes as 3 × 30",
            grib1_forecast(14, 3),
            grib2_forecast(0, 90),
            "+90min",
            Some(2),
        ),
        (
            "90 seconds",
            grib1_forecast(254, 90),
            grib2_forecast(13, 90),
            "+90s",
            Some(0),
        ),
        (
            "2 hours as 8 × 15 minutes",
            grib1_forecast(13, 8),
            grib2_forecast(0, 120),
            "+2h",
            Some(2),
        ),
        // Time range 10 spends P1 and P2 on one 16-bit P1: 7200 = 0x1C20.
        (
            "2 hours in seconds",
            grib1(98, 254, 0x1C, 0x20, 10),
            grib2_forecast(13, 7_200),
            "+2h",
            Some(2),
        ),
        (
            "a whole number of hours",
            grib1_forecast(1, 24),
            grib2_forecast(1, 24),
            "+24h",
            Some(24),
        ),
        (
            "2 days",
            grib1_forecast(2, 2),
            grib2_forecast(2, 2),
            "+48h",
            Some(48),
        ),
        (
            "one 6-hour unit",
            grib1_forecast(11, 1),
            grib2_forecast(11, 1),
            "+6h",
            Some(6),
        ),
        (
            "the analysis time",
            grib1_forecast(1, 0),
            grib2_forecast(1, 0),
            "+0h",
            Some(0),
        ),
    ];
    for (what, g1, g2, text, hours) in cases {
        assert_eq!(g1.forecast.as_deref(), Some(text), "GRIB1, {what}");
        assert_eq!(g2.forecast.as_deref(), Some(text), "GRIB2, {what}");
        assert_eq!(g1.forecast_hours, hours, "GRIB1 hours, {what}");
        assert_eq!(g2.forecast_hours, hours, "GRIB2 hours, {what}");
    }
}

/// A product that is not a forecast, or that covers a period, has the hours
/// of its start from the reference time. An analysis, or a statistic over
/// analyses starting at the reference time, is at 0 whatever P1 says, which
/// is the 0 a GRIB2 analysis states in its forecast time.
#[test]
fn a_product_that_is_not_a_forecast_has_the_hours_of_its_start() {
    let grib2_analysis = grib2_forecast(1, 0);
    assert_eq!(grib2_analysis.forecast_hours, Some(0));

    // WMO 123: the average of analyses every P2, starting at the reference
    // time. P1 plays no part, so a producer's 6 there is not a lead.
    let wmo_123 = grib1(98, 1, 6, 24, 123);
    assert_eq!(
        wmo_123.forecast.as_deref(),
        Some("average of analyses every 24h")
    );
    assert_eq!(
        wmo_123.forecast_hours, grib2_analysis.forecast_hours,
        "WMO 123"
    );

    // DWD local 13: fields from analyses valid at the reference time.
    let dwd_13 = grib1(78, 1, 6, 0, 13);
    assert_eq!(dwd_13.forecast.as_deref(), Some("analysis"));
    assert_eq!(
        dwd_13.forecast_hours, grib2_analysis.forecast_hours,
        "DWD 13"
    );

    // WMO 1: an initialized analysis, P1 = 0 by definition.
    let wmo_1 = grib1(98, 1, 6, 0, 1);
    assert_eq!(wmo_1.forecast.as_deref(), Some("analysis"));
    assert_eq!(wmo_1.forecast_hours, Some(0), "WMO 1");

    // A start at the reference time is 0 hours in any unit, a month included,
    // in both editions (GRIB1 month is Table 4 code 3, GRIB2 Table 4.4 code 3).
    let grib2_month_zero = grib2_forecast(3, 0);
    assert_eq!(grib2_month_zero.forecast_hours, Some(0), "GRIB2, 0 months");
    for (code, p1) in [(1u8, 0u8), (51, 0), (123, 0)] {
        let g1 = grib1(98, 3, p1, 1, code);
        assert_eq!(
            g1.forecast_hours, grib2_month_zero.forecast_hours,
            "GRIB1 time range {code} in months"
        );
    }

    // WMO 3: an average from reference + P1 to reference + P2 starts at P1,
    // as a GRIB2 statistic's forecast time is the start of its window.
    let wmo_3 = grib1(98, 1, 6, 12, 3);
    assert_eq!(wmo_3.forecast.as_deref(), Some("6–12h average"));
    assert_eq!(wmo_3.forecast_hours, Some(6), "WMO 3");

    // WMO 6: an average from reference − P1 to reference − P2 starts before
    // the reference time.
    let wmo_6 = grib1(98, 1, 12, 6, 6);
    assert_eq!(wmo_6.forecast.as_deref(), Some("−12h to −6h average"));
    assert_eq!(wmo_6.forecast_hours, Some(-12), "WMO 6");
}
