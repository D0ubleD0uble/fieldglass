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
        // A unit with no fixed length keeps the producer's number and unit,
        // spelled in lowercase in both editions (#1595). The calendar units
        // are the same codes in GRIB1's Table 4 and GRIB2's Table 4.4.
        (
            "6 months",
            grib1_forecast(3, 6),
            grib2_forecast(3, 6),
            "+6 month",
            None,
        ),
        (
            "the analysis time in months",
            grib1_forecast(3, 0),
            grib2_forecast(3, 0),
            "+0 month",
            Some(0),
        ),
        (
            "2 years",
            grib1_forecast(4, 2),
            grib2_forecast(4, 2),
            "+2 year",
            None,
        ),
        (
            "a decade",
            grib1_forecast(5, 1),
            grib2_forecast(5, 1),
            "+1 decade",
            None,
        ),
        (
            "a 30-year normal",
            grib1_forecast(6, 1),
            grib2_forecast(6, 1),
            "+1 normal (30 years)",
            None,
        ),
        (
            "a century",
            grib1_forecast(7, 1),
            grib2_forecast(7, 1),
            "+1 century",
            None,
        ),
        // The missing unit is 255 in both editions. GRIB2's Code Table 4.4
        // names it "Missing"; GRIB1's ON388 Table 4 does not name it, and
        // fieldglass reads it as missing there too, with the same label
        // (#1605). It has no hours at any value, 0 included (#1595).
        (
            "0 in the missing unit",
            grib1_forecast(255, 0),
            grib2_forecast(255, 0),
            "+0 Missing",
            None,
        ),
        (
            "6 in the missing unit",
            grib1_forecast(255, 6),
            grib2_forecast(255, 6),
            "+6 Missing",
            None,
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

    // WMO 4 from P1 = P2 = 0: a first step's empty accumulation, which
    // starts at the reference time. Its GRIB2 counterpart is the committed
    // ECMWF IFS fixture, template 4.8 with forecast time 0 and a period of
    // length 0, and both read as hour 0.
    let wmo_4_empty = grib1(98, 1, 0, 0, 4);
    assert_eq!(wmo_4_empty.forecast.as_deref(), Some("0–0h accum"));
    let grib2_empty = message(include_bytes!("fixtures/ecmwf_ifs_0p25_str.grib2").to_vec());
    assert_eq!(grib2_empty.forecast_hours, Some(0), "GRIB2 4.8 of length 0");
    assert_eq!(
        wmo_4_empty.forecast_hours, grib2_empty.forecast_hours,
        "WMO 4 from 0 to 0"
    );

    // WMO 6: an average from reference − P1 to reference − P2 starts before
    // the reference time.
    let wmo_6 = grib1(98, 1, 12, 6, 6);
    assert_eq!(wmo_6.forecast.as_deref(), Some("−12h to −6h average"));
    assert_eq!(wmo_6.forecast_hours, Some(-12), "WMO 6");
}

/// A lead whose time unit is missing has no hours, even at 0: the message
/// does not say what its number counts, so no lead is made up for it (owner
/// decision on #1595, 2026-11-02). The unit is 255 in both editions: GRIB2's
/// Code Table 4.4 names it "Missing", and GRIB1's ON388 Table 4 does not name
/// it at all; 255 is the all-ones value GRIB writes in a missing octet, and
/// fieldglass reads it as missing in GRIB1 to match GRIB2. A GRIB1 analysis
/// keeps its 0, because where it starts does not depend on the unit.
#[test]
fn a_lead_in_a_missing_unit_has_no_hours_in_either_edition() {
    for value in [0u8, 6] {
        let g1 = grib1_forecast(255, value);
        let g2 = grib2_forecast(255, u32::from(value));
        assert_eq!(g1.forecast_hours, None, "GRIB1, {value} missing units");
        assert_eq!(g2.forecast_hours, None, "GRIB2, {value} missing units");
    }
    // A GRIB1 period from P1 to P2 starts at P1, so a start in a missing unit
    // is missing too.
    assert_eq!(grib1(98, 255, 0, 6, 3).forecast_hours, None, "WMO 3");

    // The text keeps the producer's number and the label "Missing", in both
    // editions (#1605): in GRIB1 that covers every time-range code's text,
    // not only a plain forecast's. A period from P1 to P2 in the missing
    // unit has no known length, and a step between products in it says
    // nothing about how far apart they are, so both show raw (#1567).
    for m in [grib1_forecast(255, 0), grib2_forecast(255, 0)] {
        assert_eq!(m.forecast.as_deref(), Some("+0 Missing"));
    }
    for (code, text) in [
        (4u8, "time range 4, P1 0 Missing, P2 6 Missing"),
        (113, "time range 113, P1 0 Missing, P2 6 Missing"),
        (8, "time range 8, P1 0 Missing, P2 6 Missing"),
    ] {
        assert_eq!(
            grib1(98, 255, 0, 6, code).forecast.as_deref(),
            Some(text),
            "GRIB1 time range {code}"
        );
    }

    // The analysis codes start at the reference time whatever the unit.
    for (centre, code) in [
        (98u8, 1u8),
        (98, 51),
        (98, 118),
        (98, 123),
        (98, 124),
        (78, 13),
        // WMO 113 and 114 at P1 = 0: an average and an accumulation of
        // initialized analyses.
        (98, 113),
        (98, 114),
        // JMA local 132 at P1 = 0: a variance of analyses.
        (34, 132),
    ] {
        assert_eq!(
            grib1(centre, 255, 0, 1, code).forecast_hours,
            Some(0),
            "GRIB1 time range {code} at center {centre} in a missing unit"
        );
    }
    // At any other P1 they are forecasts, which start at P1.
    for (centre, code) in [(98u8, 113u8), (98, 114), (34, 132)] {
        assert_eq!(
            grib1(centre, 255, 6, 1, code).forecast_hours,
            None,
            "GRIB1 time range {code} at center {centre}, P1 6, in a missing unit"
        );
    }
}
