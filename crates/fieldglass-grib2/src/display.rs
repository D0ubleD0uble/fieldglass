//! Level and forecast-time strings for a §4 product.
//!
//! `level_value_str`, `level_type_str`, `forecast_hours` and `forecast_display`
//! are the GRIB1 crate's four counterparts **by name and by column** — they
//! fill the same four places in a message list. They are not the same function
//! twice, and two differences are worth knowing before reading one as the
//! other:
//!
//! * GRIB1 prints `"—"` for a fixed-surface type, where GRIB2 prints the
//!   surface's own name ("Ground or water surface"): GRIB2's Code Table 4.5
//!   entry *is* the description, and blanking it would lose the only thing the
//!   column has to say.
//! * GRIB1 renders a layer as its two bounds. GRIB2 states the bottom of a
//!   layer in `second_surface`, and these functions never read it, so a layer
//!   product reports its top surface alone — as it did when this code lived in
//!   `fieldglass-napi`, and as both hosts still display it.
//!
//! These lived in `fieldglass-napi` until #545, which is why the umbrella crate
//! and the napi host had grown a copy each and had drifted: one rendered a
//! twenty-four-hour lead `+24h` and the other `+24 Hour`, for the same message,
//! in the same release. A standalone GRIB2 consumer got neither, while a GRIB1
//! consumer got all four from its format crate.
//!
//! The rules two editions genuinely share — saturating the hours column, and
//! keeping the producer's number and label for a unit that has no hours — are
//! [`fieldglass_core::lead_time`]'s, and that module records what stays here
//! and why.

use fieldglass_core::lead_time::{lead_label, saturating_hours};

use crate::pds::HorizontalProductCommon;
use crate::tables::{lookup_fixed_surface, lookup_time_range_unit};

/// The first fixed surface's value, rendered.
///
/// `"—"` when the surface type is the WMO missing sentinel, the decoded value
/// when the surface carries one, and the surface's own name when it does not:
/// "Ground or water surface" has no height to print. The unit hint is in
/// [`level_type_str`], which is the column beside it.
///
/// Reads `first_surface` only. For a layer product that is the *top* surface;
/// the bottom is in `second_surface` and no host renders it today.
#[must_use]
pub fn level_value_str(common: &HorizontalProductCommon) -> String {
    let surface = &common.first_surface;
    if surface.is_missing() {
        return "—".to_string();
    }
    match surface.value() {
        Some(v) => format!("{v}"),
        None => lookup_fixed_surface(surface.surface_type).to_string(),
    }
}

/// The first fixed surface's name (Code Table 4.5).
///
/// GRIB2 states the unit inside the surface name — "Specified height above
/// ground (m)" — where GRIB1 needs a separate `level_unit` to reunite the two
/// columns, so there is no GRIB2 counterpart to that function.
#[must_use]
pub fn level_type_str(common: &HorizontalProductCommon) -> String {
    lookup_fixed_surface(common.first_surface.surface_type).to_string()
}

/// The lead time in whole hours, or `None` for a unit with no fixed length in
/// hours: the calendar units (month, year, decade, normal, century), the
/// missing sentinel, and every code Table 4.4 reserves or leaves to local use.
/// [`forecast_display`] renders those with whatever label the table gives them,
/// which for an unmodelled code is "Unknown time-range unit".
///
/// A **coarse** sort key, not the exact lead: a sub-hour unit truncates toward
/// zero, so a 0/15/30/45-minute nowcast series — MRMS states its lead in
/// minutes — answers `0` for every step. The exact value is always in
/// [`forecast_display`], which keeps the producer's own unit.
///
/// The units are WMO Code Table 4.4, which is *not* GRIB1's ON388 Table 4 even
/// where the numbers overlap: `13` is a second here and fifteen minutes there.
#[must_use]
pub fn forecast_hours(common: &HorizontalProductCommon) -> Option<i32> {
    let raw = common.forecast_time;
    let hours = match common.forecast_time_unit {
        0 => raw / 60,     // minute
        1 => raw,          // hour
        2 => raw * 24,     // day
        10 => raw * 3,     // 3 hours
        11 => raw * 6,     // 6 hours
        12 => raw * 12,    // 12 hours
        13 => raw / 3_600, // second
        _ => return None,
    };
    Some(saturating_hours(hours))
}

/// The lead time, rendered — `"+24h"`, `"+30 Minute"`, `"+6 Month"`.
///
/// Only the hour unit renders as hours. Every other unit keeps the producer's
/// own number and label, so the exact lead survives even where
/// [`forecast_hours`] rounds it away; that is the display half of the rule
/// [`fieldglass_core::lead_time::lead_label`] carries.
///
/// The `+` is unconditional, so a negative lead — §4 states forecast time in
/// sign-magnitude, and a message really can say −6 — reads `"+-6h"`. Kept as it
/// was in `fieldglass-napi`, because both hosts have shown it that way since
/// GRIB2 products were first listed and the column is a lead time either way.
#[must_use]
pub fn forecast_display(common: &HorizontalProductCommon) -> String {
    // Deliberately not `forecast_hours`: a lead stated in days shows as days,
    // unlike GRIB1, which converts everything convertible. And deliberately
    // un-narrowed — the string has no column to overflow, so a lead past
    // `i32` prints in full here even though `forecast_hours` saturates it.
    let hours = (common.forecast_time_unit == 1).then_some(common.forecast_time);
    format!(
        "+{}",
        lead_label(
            hours,
            common.forecast_time,
            lookup_time_range_unit(common.forecast_time_unit),
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pds::FixedSurface;

    fn common(unit: u8, forecast_time: i64) -> HorizontalProductCommon {
        HorizontalProductCommon {
            parameter_category: 0,
            parameter_number: 0,
            generating_process_type: 2,
            background_process_id: 0,
            forecast_process_id: 0,
            obs_cutoff_hours: 0,
            obs_cutoff_minutes: 0,
            forecast_time_unit: unit,
            forecast_time,
            first_surface: surface(1, Some(0), Some(0)),
            second_surface: surface(255, None, None),
        }
    }

    fn surface(
        surface_type: u8,
        scale_factor: Option<i8>,
        scaled_value: Option<i64>,
    ) -> FixedSurface {
        FixedSurface {
            surface_type,
            scale_factor,
            scaled_value,
        }
    }

    fn at_surface(s: FixedSurface) -> HorizontalProductCommon {
        let mut c = common(1, 0);
        c.first_surface = s;
        c
    }

    #[test]
    fn forecast_hours_normalises_each_convertible_unit() {
        // (unit, raw) -> hours. Table 4.4: 0 min, 1 hour, 2 day, 10/11/12 the
        // 3/6/12-hour units, 13 second.
        for (unit, raw, want) in [
            (0u8, 60i64, 1i32),
            (1, 24, 24),
            (2, 2, 48),
            (10, 2, 6),
            (11, 2, 12),
            (12, 2, 24),
            (13, 7200, 2),
        ] {
            assert_eq!(
                forecast_hours(&common(unit, raw)),
                Some(want),
                "unit {unit} raw {raw}"
            );
        }
    }

    /// Only the hour unit renders as `+Nh`; every other unit keeps the
    /// producer's own wording, so the exact lead time is never lost even when
    /// the hours column rounds it away.
    #[test]
    fn forecast_display_keeps_the_producers_unit() {
        assert_eq!(forecast_display(&common(1, 24)), "+24h");
        assert_eq!(forecast_display(&common(0, 30)), "+30 Minute");
        assert_eq!(forecast_display(&common(13, 90)), "+90 Second");
        // eccc states a one-hour lead in minutes; the display shows what it said.
        assert_eq!(forecast_display(&common(0, 60)), "+60 Minute");
        // A day is convertible, and still shows as a day.
        assert_eq!(forecast_display(&common(2, 2)), "+2 Day");
    }

    /// The documented coarseness, pinned so it is a choice rather than a
    /// surprise: a sub-hour nowcast series collapses to one hours value, and
    /// the display string is the only exact record of the step.
    #[test]
    fn forecast_hours_truncates_sub_hour_leads_toward_zero() {
        for raw in [0i64, 15, 30, 45, 59] {
            assert_eq!(forecast_hours(&common(0, raw)), Some(0), "raw {raw}");
        }
        assert_eq!(forecast_hours(&common(0, 60)), Some(1));
        assert_eq!(forecast_hours(&common(0, 119)), Some(1));
        // Distinct steps, distinct displays — the exactness lives here.
        assert_ne!(
            forecast_display(&common(0, 15)),
            forecast_display(&common(0, 45)),
        );
        // Negative (sign-magnitude on the wire) truncates toward zero too.
        assert_eq!(forecast_hours(&common(0, -90)), Some(-1));
        // And the display's `+` is unconditional, so a negative lead reads
        // `+-6h`. Pinned because it is inherited rather than chosen: both hosts
        // have shown it this way since GRIB2 products were first listed.
        assert_eq!(forecast_display(&common(1, -6)), "+-6h");
    }

    /// A unit with no clean hour conversion yields no hours, but must still
    /// report the raw value and its label rather than inventing a lead time.
    #[test]
    fn forecast_leaves_unconvertible_units_to_the_display_string() {
        for (unit, label) in [
            (3u8, "Month"),
            (4, "Year"),
            (7, "Century (100 years)"),
            (255, "Missing"),
        ] {
            assert_eq!(forecast_hours(&common(unit, 5)), None, "unit {unit}");
            assert_eq!(forecast_display(&common(unit, 5)), format!("+5 {label}"));
        }
    }

    /// A lead time too large for `i32` is still a large lead time. Falling back
    /// to 0 would file a nonsense far-future step next to the analysis, which is
    /// the one reading the hours column exists to support.
    #[test]
    fn forecast_hours_saturates_instead_of_reporting_zero_hours() {
        // Days: 2e9 days * 24 overflows i32 by a wide margin.
        assert_eq!(
            forecast_hours(&common(2, 2_000_000_000)),
            Some(i32::MAX),
            "an unrepresentable lead saturates, it does not become 0"
        );
        assert_eq!(
            forecast_display(&common(2, 2_000_000_000)),
            "+2000000000 Day"
        );
        assert_eq!(forecast_hours(&common(2, -2_000_000_000)), Some(i32::MIN));
        // The display path does *not* clamp: it prints the lead the message
        // stated. Only a struct literal can reach this — §4 encodes the
        // forecast time in 31 bits of magnitude, so no message can state a lead
        // outside `i32` — but the two columns disagreeing here on purpose is
        // the thing to keep, and it is what `fieldglass-napi` did before #545.
        assert_eq!(
            forecast_display(&common(1, i64::from(i32::MAX) + 1)),
            "+2147483648h"
        );
        assert_eq!(
            forecast_hours(&common(1, i64::from(i32::MAX) + 1)),
            Some(i32::MAX)
        );
    }

    #[test]
    fn a_missing_surface_has_no_level_to_print() {
        let c = at_surface(surface(255, None, None));
        assert_eq!(level_value_str(&c), "—");
        assert_eq!(level_type_str(&c), "Missing");
    }

    /// A surface with no scaled value is named rather than numbered, in both
    /// columns — "Ground or water surface" has no height.
    #[test]
    fn a_valueless_surface_is_named_in_the_value_column() {
        let c = at_surface(surface(1, None, None));
        assert_eq!(level_value_str(&c), "Ground or water surface");
        assert_eq!(level_type_str(&c), "Ground or water surface");
    }

    #[test]
    fn a_scaled_surface_value_is_decoded_before_it_is_printed() {
        // 2 m above ground, and a 850 hPa isobaric surface stated in pascals.
        let c = at_surface(surface(103, Some(0), Some(2)));
        assert_eq!(level_value_str(&c), "2");
        assert_eq!(level_type_str(&c), "Specified height above ground (m)");
        let c = at_surface(surface(100, Some(-2), Some(850)));
        assert_eq!(level_value_str(&c), "85000");
        assert_eq!(level_type_str(&c), "Isobaric surface (Pa)");
    }

    /// The committed corpus, so the four functions are pinned against real
    /// products rather than only against hand-built commons.
    #[test]
    fn the_committed_fixtures_render_their_level_and_lead() {
        for (bytes, level, level_type, hours, display) in [
            (
                include_bytes!("../tests/fixtures/regular_latlon_surface.grib2").as_slice(),
                "2",
                "Specified height above ground (m)",
                Some(0),
                "+0h",
            ),
            (
                include_bytes!("../tests/fixtures/eta_lambert_msg0.grib2").as_slice(),
                // A surface that *does* carry a scaled value of zero, which is
                // not the same as one that carries none: this prints `0`, and
                // `a_valueless_surface_is_named_in_the_value_column` covers the
                // other reading.
                "0",
                "Mean sea level",
                Some(24),
                "+24h",
            ),
        ] {
            let reader = crate::Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
            let common = reader.messages[0]
                .pds
                .common()
                .expect("fixture uses a horizontal product template");
            assert_eq!(level_value_str(common), level);
            assert_eq!(level_type_str(common), level_type);
            assert_eq!(forecast_hours(common), hours);
            assert_eq!(forecast_display(common), display);
        }
    }
}
