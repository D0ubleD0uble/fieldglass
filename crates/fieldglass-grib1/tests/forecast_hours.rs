//! `forecast_hours` keeps the promise 0.6.0 made for it (#1680): "Convert the
//! PDS time unit + P1 to a number of forecast hours", whatever the time-range
//! indicator says. `offset_hours` is where the product starts, the number the
//! `fieldglass` crate reports.
//!
//! Each case restamps the committed fixture's PDS and reads it back through
//! [`Grib1Reader`], so the unit, P1, P2 and time range come off the wire.

use fieldglass_grib1::{Grib1Reader, ProductDefinition, forecast_hours, offset_hours};

const FIXTURE: &[u8] = include_bytes!("fixtures/j_consecutive_latlon.grib1");

/// DWD, whose Code Table 5 names code 13.
const CENTRE_DWD: u8 = 78;

/// The fixture with PDS octets 5 (center) and 18-21 (unit, P1, P2, time
/// range) restamped. Section 0 is 8 octets, so PDS octet n sits at 8 + n - 1.
fn stamped(centre: u8, unit: u8, p1: u8, p2: u8, time_range: u8) -> ProductDefinition {
    let mut bytes = FIXTURE.to_vec();
    bytes[8 + 5 - 1] = centre;
    bytes[8 + 18 - 1] = unit;
    bytes[8 + 19 - 1] = p1;
    bytes[8 + 20 - 1] = p2;
    bytes[8 + 21 - 1] = time_range;
    let reader = Grib1Reader::from_bytes(bytes).expect("reader");
    let pds = reader.messages.first().expect("one message").pds;
    assert_eq!(
        (pds.originating_centre, pds.time_unit, pds.time_range),
        (centre, unit, time_range),
        "the stamp reached the PDS"
    );
    pds
}

#[test]
fn forecast_hours_is_p1_in_hours_at_every_kind_of_time_range() {
    // (what, center, time range, P1, P2, forecast_hours, offset_hours)
    let cases = [
        ("WMO 1, an analysis", 98u8, 1u8, 6u8, 0u8, 6, Some(0)),
        ("WMO 6, from −P1 to −P2", 98, 6, 18, 6, 18, Some(-18)),
        ("WMO 7, from −P1 to +P2", 98, 7, 6, 18, 6, Some(-6)),
        ("DWD 13, an analysis", CENTRE_DWD, 13, 6, 0, 6, Some(0)),
        ("unnamed 8", 98, 8, 6, 18, 6, None),
        ("unnamed 200", 98, 200, 12, 0, 12, None),
        ("WMO 123 with no step", 98, 123, 6, 0, 6, None),
        ("WMO 0, a forecast", 98, 0, 24, 0, 24, Some(24)),
    ];
    for (what, centre, code, p1, p2, hours, offset) in cases {
        let pds = stamped(centre, 1, p1, p2, code);
        assert_eq!(forecast_hours(&pds), Some(hours), "{what}");
        assert_eq!(offset_hours(&pds), offset, "{what}");
    }
}

#[test]
fn forecast_hours_converts_p1_through_its_unit() {
    // 15-minute units (ON388 Table 4 code 13) at an analysis: 8 × 15 min is
    // 2 hours, though the analysis starts at 0.
    let pds = stamped(98, 13, 8, 0, 1);
    assert_eq!(
        (forecast_hours(&pds), offset_hours(&pds)),
        (Some(2), Some(0))
    );
    // Days: P1 = 2 is 48 hours at code 6, whose start is −48.
    let pds = stamped(98, 2, 2, 1, 6);
    assert_eq!(
        (forecast_hours(&pds), offset_hours(&pds)),
        (Some(48), Some(-48))
    );
    // Code 10 spends P1 and P2 on one 16-bit P1: 0x012C is 300 hours.
    let pds = stamped(98, 1, 1, 44, 10);
    assert_eq!(
        (forecast_hours(&pds), offset_hours(&pds)),
        (Some(300), Some(300))
    );
}

#[test]
fn forecast_hours_has_none_for_a_unit_with_no_fixed_length_even_at_zero() {
    // A month (3), a year (4), the missing unit (255) and an unnamed one
    // (200), at an analysis with P1 = 0. The analysis still starts at 0 in
    // each, since where it starts does not use P1.
    for unit in [3u8, 4, 200] {
        let pds = stamped(98, unit, 0, 0, 1);
        assert_eq!(forecast_hours(&pds), None, "unit {unit}");
        assert_eq!(offset_hours(&pds), Some(0), "unit {unit}");
    }
    let pds = stamped(98, 255, 0, 0, 1);
    assert_eq!((forecast_hours(&pds), offset_hours(&pds)), (None, Some(0)));
}
