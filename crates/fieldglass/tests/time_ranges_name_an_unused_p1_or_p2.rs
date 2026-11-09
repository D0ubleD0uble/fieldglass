//! A GRIB1 time range names the P1 or P2 its message states but its code
//! does not read (#1674).
//!
//! DWD's local time-range code 13 is "Fields from analyses valid at reference
//! time for P1 = 0" (eccodes 2.34.1 `grib1/local/edzw/5.table`; DWD's
//! COSMO-DE-EPS database description, Table 11: "Atmosphärenfelder aus
//! Analyse gültig für Referenzzeit (wenn P1 = 0)"). WMO's code 1 is an
//! initialized analysis "(P1=0)", 118 a variance of analyses each with
//! "forecast period P1=0", and 123 and 124 statistics over analyses
//! "starting at the reference time". None of them defines a P1 other than 0.
//! The Fcst text read `analysis` for a code-13 message with P1 = 6h, so the
//! P1 the file states vanished. It now reads `analysis (P1 6h not used)`.
//!
//! P2 goes the same way. An analysis reads no P2, and neither does a forecast
//! (0, and DWD's 11 and 14), so a P2 there is named too:
//! `analysis (P2 6h not used)`, `+6h (P2 18h not used)`. DWD writes code 13
//! with a P2 for some parameters, which eccodes reads as a statistic from P1
//! to P2 (`grib1/localConcepts/edzw/stepType.def`).
//!
//! `forecast_hours` (P1 in hours, as 0.6.0 defined it) and `offset_hours`
//! (0 for the analyses whatever P1 holds, as 0.6.1 documented it) keep
//! their values.
//!
//! Each case restamps the committed `j_consecutive_latlon.grib1` and reads it
//! through [`Session::message`], the path every host takes.

use fieldglass::{MessageInfo, Session};

const GRIB1: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");

const CENTRE_ECMWF: u8 = 98;
const CENTRE_DWD: u8 = 78;
const HOUR: u8 = 1;

/// The fixture with PDS octet 5 (center) and octets 18-21 (unit, P1, P2,
/// time range) restamped, built the way RELEASE-TEST-PLAN's
/// `grib1_time_ranges.grib1` builds each message: off ECMWF, the ECMWF local
/// PDS extension (octets 29-52) is dropped and both lengths fixed, so another
/// center's local definition does not misread it. `stamp(78, 1, 6, 0, 13)`
/// is byte for byte message 14 of that file (164 octets).
fn stamp(centre: u8, unit: u8, p1: u8, p2: u8, code: u8) -> Vec<u8> {
    const P: usize = 8;
    let mut b = GRIB1.to_vec();
    b[P + 4] = centre;
    b[P + 17] = unit;
    b[P + 18] = p1;
    b[P + 19] = p2;
    b[P + 20] = code;
    if centre != CENTRE_ECMWF {
        b[P + 3] = 2;
        b[P + 8] = 11;
        b[P + 2] = 28;
        b.drain(P + 28..P + 52);
        let len = u32::try_from(b.len()).expect("small message").to_be_bytes();
        b[4..7].copy_from_slice(&len[1..]);
    }
    b
}

fn message(bytes: Vec<u8>) -> MessageInfo {
    Session::open(bytes)
        .expect("opens")
        .message(0)
        .expect("message 0")
}

/// The Fcst text, `forecast_hours` and `offset_hours` of one message.
fn time(centre: u8, p1: u8, p2: u8, code: u8) -> (Option<String>, Option<i32>, Option<i32>) {
    let m = message(stamp(centre, HOUR, p1, p2, code));
    (m.forecast, m.forecast_hours, m.offset_hours)
}

#[test]
fn the_release_test_plan_message_is_the_committed_fixture_restamped() {
    // Message 14 of 0.6.1's `grib1_time_ranges.grib1` (sha256
    // 1c06a8d5fb81c64d7832dcab4adb659610ed0f8098c016423008dcf68ddcebbc),
    // octet by octet: section 0 with the shortened total length, the
    // 28-octet PDS (DWD, table 2, unit hour, P1 6, P2 0, code 13), and the
    // fixture's GDS onwards unchanged.
    let b = stamp(CENTRE_DWD, HOUR, 6, 0, 13);
    assert_eq!(b.len(), 164);
    assert_eq!(b[..8], [71, 82, 73, 66, 0, 0, 164, 1], "section 0");
    assert_eq!(
        b[8..36],
        [
            0, 0, 28, 2, 78, 128, 255, 128, 11, 1, 0, 0, 7, 3, 23, 12, 0, 1, 6, 0, 13, 0, 0, 0, 21,
            0, 0, 0
        ],
        "PDS"
    );
    assert_eq!(b[36..], GRIB1[8 + 52..], "GDS, BDS and end section");
}

#[test]
fn a_dwd_code_13_analysis_names_the_p1_and_p2_it_does_not_use() {
    // Message 14 of `grib1_time_ranges.grib1`. eccodes 2.34.1 reads it as
    // timeRangeIndicator 13, P1 6, P2 0, stepType instant, startStep and
    // endStep 6.
    assert_eq!(
        time(CENTRE_DWD, 6, 0, 13),
        (
            Some("analysis (P1 6h not used)".to_string()),
            Some(6),
            Some(0)
        )
    );
    // At P1 = P2 = 0, what DWD's table defines, it is a plain analysis.
    assert_eq!(
        time(CENTRE_DWD, 0, 0, 13),
        (Some("analysis".to_string()), Some(0), Some(0))
    );
    // A P2 alone, and both.
    assert_eq!(
        time(CENTRE_DWD, 0, 6, 13),
        (
            Some("analysis (P2 6h not used)".to_string()),
            Some(0),
            Some(0)
        )
    );
    assert_eq!(
        time(CENTRE_DWD, 6, 18, 13),
        (
            Some("analysis (P1 6h, P2 18h not used)".to_string()),
            Some(6),
            Some(0)
        )
    );
    // Code 13 is DWD's alone: at ECMWF it stays an unnamed code, raw.
    assert_eq!(
        time(CENTRE_ECMWF, 6, 0, 13),
        (
            Some("time range 13, P1 6h, P2 0h".to_string()),
            Some(6),
            None
        )
    );
}

#[test]
fn an_analysis_names_the_p1_and_p2_it_does_not_use() {
    // WMO code 1, at any center, reads neither field.
    for centre in [CENTRE_ECMWF, CENTRE_DWD] {
        for (p1, p2, text) in [
            (0u8, 0u8, "analysis"),
            (6, 0, "analysis (P1 6h not used)"),
            (0, 18, "analysis (P2 18h not used)"),
            (6, 18, "analysis (P1 6h, P2 18h not used)"),
        ] {
            assert_eq!(
                time(centre, p1, p2, 1),
                (Some(text.to_string()), Some(i32::from(p1)), Some(0)),
                "center {centre}, P1 {p1}, P2 {p2}"
            );
        }
    }
}

#[test]
fn a_statistic_over_analyses_names_the_p1_it_does_not_use() {
    // 118, 123 and 124 read P2 as their step, and not P1.
    let cases = [
        (
            CENTRE_ECMWF,
            118u8,
            "variance of initialized analyses issued every 12h",
        ),
        (CENTRE_ECMWF, 123, "average of analyses every 12h"),
        (CENTRE_DWD, 124, "accum of analyses every 12h"),
    ];
    for (centre, code, at_zero) in cases {
        assert_eq!(
            time(centre, 0, 12, code),
            (Some(at_zero.to_string()), Some(0), Some(0)),
            "center {centre} code {code}, P1 0"
        );
        assert_eq!(
            time(centre, 6, 12, code),
            (
                Some(format!("{at_zero} (P1 6h not used)")),
                Some(6),
                Some(0)
            ),
            "center {centre} code {code}, P1 6h"
        );
    }
}

#[test]
fn a_forecast_names_the_p2_it_does_not_use() {
    // Code 0, and DWD's 11 and 14, read P1 and not P2.
    for (centre, code, at_zero) in [
        (CENTRE_ECMWF, 0u8, "+6h"),
        (CENTRE_DWD, 0, "+6h"),
        (CENTRE_DWD, 11, "+6h initialized (IDFI)"),
        (CENTRE_DWD, 14, "+6h (IFS on GME grid)"),
    ] {
        assert_eq!(
            time(centre, 6, 0, code),
            (Some(at_zero.to_string()), Some(6), Some(6)),
            "center {centre} code {code}, P2 0"
        );
        assert_eq!(
            time(centre, 6, 18, code),
            (
                Some(format!("{at_zero} (P2 18h not used)")),
                Some(6),
                Some(6)
            ),
            "center {centre} code {code}, P2 18h"
        );
    }
    // At P1 = 0 too.
    assert_eq!(
        time(CENTRE_ECMWF, 0, 6, 0).0.as_deref(),
        Some("+0h (P2 6h not used)")
    );
}

#[test]
fn codes_that_read_p1_and_p2_are_unchanged() {
    // Code 10 spends both octets on P1, and 113 reads P1 as the lead and P2
    // as the step.
    assert_eq!(time(CENTRE_DWD, 0, 6, 10).0.as_deref(), Some("+6h"));
    assert_eq!(
        time(CENTRE_ECMWF, 6, 24, 113).0.as_deref(),
        Some("average of +6h forecasts issued every 24h")
    );
}
