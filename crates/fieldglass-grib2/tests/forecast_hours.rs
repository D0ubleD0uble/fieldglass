//! `forecast_hours` keeps the promise 0.6.0 made for it (#1680): `None` for a
//! unit with no fixed length in hours, the calendar units, the missing
//! sentinel and every reserved or local-use code of Table 4.4, a lead of 0
//! included. `offset_hours` is the number whose zero lead is 0 hours in those
//! units, the one the `fieldglass` crate reports.
//!
//! Each case restamps the committed fixture's §4 and reads it back through
//! [`Grib2Reader`], so the unit and forecast time come off the wire.

use fieldglass_grib2::{Grib2Reader, forecast_display, forecast_hours, offset_hours};

const FIXTURE: &[u8] = include_bytes!("fixtures/complex_regular_latlon.grib2");

/// The byte offset of section `number`, walking the section lengths from the
/// end of the 16-octet §0.
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

/// `(forecast_hours, offset_hours, forecast_display)` of the fixture with its
/// unit of time range (§4 octet 18, Code Table 4.4) and forecast time (octets
/// 19-22) restamped.
fn lead(unit: u8, forecast_time: u32) -> (Option<i32>, Option<i32>, String) {
    let mut bytes = FIXTURE.to_vec();
    let s4 = section_start(&bytes, 4);
    assert_eq!(
        (bytes[s4 + 7], bytes[s4 + 8]),
        (0, 0),
        "fixture precondition: template 4.0"
    );
    bytes[s4 + 17] = unit;
    bytes[s4 + 18..s4 + 22].copy_from_slice(&forecast_time.to_be_bytes());
    let reader = Grib2Reader::from_bytes(bytes).expect("fixture parses");
    let common = reader.messages[0].pds.common().expect("template 4.0");
    assert_eq!(common.forecast_time_unit, unit, "the stamp reached §4");
    (
        forecast_hours(common),
        offset_hours(common),
        forecast_display(common),
    )
}

#[test]
fn a_zero_lead_in_a_unit_with_no_fixed_length_has_no_forecast_hours() {
    // Calendar units 3 to 7, reserved 8, 9, 14 and 191, local-use 192 and
    // 254, and the missing sentinel 255.
    for unit in [3u8, 4, 5, 6, 7, 8, 9, 14, 191, 192, 254, 255] {
        let (hours, _, _) = lead(unit, 0);
        assert_eq!(hours, None, "unit {unit}, lead 0");
        let (hours, offset, _) = lead(unit, 6);
        assert_eq!((hours, offset), (None, None), "unit {unit}, lead 6");
    }
}

#[test]
fn a_zero_lead_in_a_unit_with_no_fixed_length_is_an_offset_of_zero() {
    // The reference time itself, whatever unit states it; the missing unit
    // says nothing about what the number counts, so it has none (#1595).
    for unit in [3u8, 4, 5, 6, 7, 8, 9, 14, 191, 192, 254] {
        assert_eq!(lead(unit, 0).1, Some(0), "unit {unit}");
    }
    assert_eq!(lead(255, 0).1, None);
    // What the column shows does not depend on which number a host reads.
    assert_eq!(lead(3, 0).2, "+0 month");
}

#[test]
fn a_lead_in_a_unit_with_a_fixed_length_is_the_same_in_both() {
    // (unit, forecast time, hours): minute, hour, day, 3/6/12 hours, second.
    for (unit, value, want) in [
        (0u8, 0u32, 0i32),
        (0, 30, 1),
        (0, 90, 2),
        (1, 0, 0),
        (1, 24, 24),
        (2, 2, 48),
        (10, 2, 6),
        (11, 2, 12),
        (12, 2, 24),
        (13, 7_200, 2),
    ] {
        let (hours, offset, _) = lead(unit, value);
        assert_eq!(hours, Some(want), "unit {unit}, lead {value}");
        assert_eq!(offset, Some(want), "unit {unit}, lead {value}");
    }
}
