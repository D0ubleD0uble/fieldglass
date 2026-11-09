//! A GRIB1 statistic over one product reads as that product (#1623).
//!
//! WMO's Code Table 5 codes 113 to 125, and JMA's 132, combine N products
//! whose reference times, or leads, step by P2. N is PDS octets 22-23. With
//! N = 1 there is no second product for P2 to place, so a P2 of 0 is not a
//! step of nothing: the message is the one product. Before N was read, P2 = 0
//! showed the raw form whatever N said (#1554).
//!
//! Only the display text changes in 0.6.2. `offset_hours` keeps its `None`
//! for a step of zero, and `forecast_hours` its 0.6.0 value, until #1684.
//!
//! Each case restamps the committed fixture and reads it through
//! [`Session::message`], the path every host takes.

use fieldglass::{MessageInfo, Session};

const GRIB1: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");

/// The fixture at `centre`, with PDS octets 18-21 (unit hours, P1, P2 = 0,
/// time range) and 22-23 (N) restamped. Section 0 is 8 octets, so PDS octet
/// n sits at 8 + n - 1.
fn one_step_of_zero(centre: u8, time_range: u8, p1: u8, n: u16) -> MessageInfo {
    stamped(centre, 1, time_range, p1, 0, n)
}

fn stamped(centre: u8, unit: u8, time_range: u8, p1: u8, p2: u8, n: u16) -> MessageInfo {
    let mut bytes = GRIB1.to_vec();
    assert_eq!(
        (bytes[8 + 21 - 1], &bytes[8 + 22 - 1..8 + 23]),
        (0, &[0u8, 0][..]),
        "fixture precondition: time range 0 with N = 0"
    );
    bytes[8 + 5 - 1] = centre;
    bytes[8 + 18 - 1] = unit;
    bytes[8 + 19 - 1] = p1;
    bytes[8 + 20 - 1] = p2;
    bytes[8 + 21 - 1] = time_range;
    bytes[8 + 22 - 1..8 + 23].copy_from_slice(&n.to_be_bytes());
    Session::open(bytes)
        .expect("opens")
        .message(0)
        .expect("message 0")
}

const WMO: u8 = 74;
const NCEP: u8 = 7;
const JMA: u8 = 34;
const ECMWF: u8 = 98;

/// Each code with N = 1 and P2 = 0, and what it reads as. The lead is P1 for
/// every code that reads P1 as a forecast period, and the codes that let
/// P1 = 0 stand for an analysis say so there.
const ONE_PRODUCT: &[(u8, u8, u8, &str)] = &[
    (WMO, 113, 6, "average of one +6h forecast"),
    (WMO, 113, 0, "average of one initialized analysis"),
    (WMO, 114, 6, "accum of one +6h forecast"),
    (WMO, 114, 0, "accum of one initialized analysis"),
    (WMO, 115, 6, "average of one +6h forecast"),
    (WMO, 116, 6, "accum of one +6h forecast"),
    (WMO, 117, 6, "average of one +6h forecast"),
    (WMO, 118, 0, "variance of one initialized analysis"),
    (WMO, 119, 6, "std dev of one +6h forecast"),
    (WMO, 123, 0, "average of one analysis"),
    (WMO, 124, 0, "accum of one analysis"),
    (WMO, 125, 6, "std dev of the tendency of one +6h forecast"),
    (JMA, 132, 6, "variance of one +6h forecast"),
    (JMA, 132, 0, "variance of one analysis"),
    // The other centers read WMO's codes the same way.
    (NCEP, 113, 6, "average of one +6h forecast"),
    (ECMWF, 123, 0, "average of one analysis"),
];

#[test]
fn a_statistic_over_one_product_reads_as_that_product() {
    for &(centre, code, p1, text) in ONE_PRODUCT {
        let info = one_step_of_zero(centre, code, p1, 1);
        assert_eq!(
            info.forecast.as_deref(),
            Some(text),
            "center {centre} code {code} P1 {p1}"
        );
    }
}

#[test]
fn only_the_display_changes() {
    // The numbers keep their 0.6.1 values: `offset_hours` is `None` for a
    // step of zero whatever N says, and `forecast_hours` is P1 in hours.
    for &(centre, code, p1, _) in ONE_PRODUCT {
        let one = one_step_of_zero(centre, code, p1, 1);
        let unread = one_step_of_zero(centre, code, p1, 0);
        assert_eq!(one.offset_hours, None, "center {centre} code {code}");
        assert_eq!(
            one.forecast_hours,
            Some(i32::from(p1)),
            "center {centre} code {code}"
        );
        assert_eq!(one.forecast_hours, unread.forecast_hours);
    }
}

#[test]
fn any_other_n_with_a_step_of_zero_stays_raw() {
    // N = 0 is what a message that is no statistic writes, 2 is more than
    // one product 0 apart, and 0xFFFF is the all-ones missing value.
    for &(centre, code, p1, _) in ONE_PRODUCT {
        for n in [0u16, 2, 3, 0x0100, 0x0101, 0xFFFF] {
            assert_eq!(
                one_step_of_zero(centre, code, p1, n).forecast.as_deref(),
                Some(format!("time range {code}, P1 {p1}h, P2 0h").as_str()),
                "center {centre} code {code} N {n}"
            );
        }
    }
}

#[test]
fn n_is_read_only_where_p2_steps_between_products() {
    // JMA's 132 is not NCEP's, whose 132 is a climatology from P1 to P2.
    // A period code's P2 is where its period ends, so N says nothing there,
    // and 120 to 122 are reserved.
    for (centre, code, text) in [
        (NCEP, 132u8, "time range 132, P1 6h, P2 0h"),
        (WMO, 132, "time range 132, P1 6h, P2 0h"),
        (WMO, 3, "time range 3, P1 6h, P2 0h"),
        (ECMWF, 128, "time range 128, P1 6h, P2 0h"),
        (WMO, 120, "time range 120, P1 6h, P2 0h"),
        (WMO, 0, "+6h"),
    ] {
        assert_eq!(
            one_step_of_zero(centre, code, 6, 1).forecast.as_deref(),
            Some(text),
            "center {centre} code {code}"
        );
    }
}

#[test]
fn a_step_the_message_states_reads_as_before() {
    // N is read only to tell one product from a step of 0, so a P2 the
    // message states keeps the text it had, whatever N says.
    for n in [0u16, 1, 2] {
        let info = stamped(WMO, 1, 113, 6, 6, n);
        assert_eq!(
            info.forecast.as_deref(),
            Some("average of +6h forecasts issued every 6h"),
            "N {n}"
        );
        assert_eq!(info.offset_hours, Some(6), "N {n}");
    }
}

#[test]
fn the_p1_an_analysis_statistic_does_not_read_is_still_named() {
    // 118, 123 and 124 start at the reference time and do not read P1, so
    // a nonzero P1 is named rather than dropped (#1674), one product or many.
    for (code, text) in [
        (
            118u8,
            "variance of one initialized analysis (P1 6h not used)",
        ),
        (123, "average of one analysis (P1 6h not used)"),
        (124, "accum of one analysis (P1 6h not used)"),
    ] {
        assert_eq!(
            one_step_of_zero(WMO, code, 6, 1).forecast.as_deref(),
            Some(text),
            "code {code}"
        );
    }
}

#[test]
fn one_product_keeps_the_unit_of_its_lead() {
    // A sub-hour unit shows minutes, as any lead does, and a lead in the
    // missing unit (255) says `Missing`, as code 0's `+6 Missing` does: with
    // one product there is no step whose length is unknown.
    assert_eq!(
        stamped(WMO, 0, 113, 90, 0, 1).forecast.as_deref(),
        Some("average of one +90min forecast")
    );
    assert_eq!(
        stamped(WMO, 255, 115, 6, 0, 1).forecast.as_deref(),
        Some("average of one +6 Missing forecast")
    );
    assert_eq!(
        stamped(WMO, 255, 113, 0, 0, 1).forecast.as_deref(),
        Some("average of one initialized analysis")
    );
}
