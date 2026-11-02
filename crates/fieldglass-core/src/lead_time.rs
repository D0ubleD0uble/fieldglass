//! The rules both GRIB editions share for showing a forecast's lead time.
//!
//! GRIB1 and GRIB2 both state a lead time as a number plus a unit code. Each
//! edition turns its unit code into a length in seconds from its own table,
//! and these functions do the rest, so the same lead reads the same in both
//! (#1565):
//!
//! - [`lead_text`] writes a lead: in hours when it is a whole number of hours
//!   (`24h`, and `48h` for two days), otherwise in minutes (`30min`), or in
//!   seconds where it is not whole minutes (`90s`). A unit with no fixed
//!   length in hours (a month, a year) keeps the producer's own number and
//!   unit (`6 Month`).
//! - [`span_text`] writes a range between two offsets in one shared unit
//!   (`0–6h`, `60–90min`).
//! - [`lead_hours`] is the number for a display column of whole hours: the
//!   lead rounded to the nearest hour, a half hour away from zero, so `30min`
//!   is 1 and `90min` is 2. It is a coarse sort key; the text carries the
//!   exact lead.
//! - [`saturating_hours`] fits a lead in hours into the 32-bit number that
//!   column holds. A lead too large stops at the largest value rather than
//!   wrapping around to a small or negative one, so it still sorts last.
//!
//! # Which offset the hours number is
//!
//! A product's hours number is the offset from the reference time to the
//! start of what the product describes. For a forecast that is its lead. For
//! an analysis, and for a statistic over analyses that starts at the
//! reference time, it is 0, which is what a GRIB2 analysis states. For a
//! product over a period it is the start of the period: a GRIB2 statistic's
//! forecast time is the start of its window, and a GRIB1 average from P1 to
//! P2 is at P1, or at −P1 for one that starts before the reference time. A
//! unit with no fixed length has no hours number.
//!
//! The start rather than the end, because the start is what GRIB2 states, and
//! a statistic's text says where its window ends.
//!
//! The rest stays in each edition's crate, because the editions differ:
//!
//! - The unit tables. GRIB1's Table 4 and GRIB2's Code Table 4.4 reuse the
//!   same small numbers for different units: `13` is fifteen minutes in GRIB1
//!   and one second in GRIB2.
//! - What each GRIB1 time-range code (Code Table 5) says the product is, and
//!   so where it starts.

/// Narrow a lead time already expressed in whole hours into the `i32` a display
/// column carries, saturating rather than wrapping.
///
/// A lead too large for `i32` is still a *large* lead: saturating keeps it
/// sorted at the far end of the column, where wrapping would file a nonsense
/// far-future step next to the analysis and a `0` fallback would hide it there.
///
/// ```
/// use fieldglass_core::lead_time::saturating_hours;
/// assert_eq!(saturating_hours(24), 24);
/// assert_eq!(saturating_hours(i64::MAX), i32::MAX);
/// assert_eq!(saturating_hours(i64::MIN), i32::MIN);
/// ```
#[must_use]
pub fn saturating_hours(hours: i64) -> i32 {
    i32::try_from(hours).unwrap_or(if hours < 0 { i32::MIN } else { i32::MAX })
}

/// Render one lead time: `"{hours}h"` when the caller has an hours value for
/// it, and `"{value} {unit_label}"` when it does not.
///
/// Neither edition calls this for a lead with a length any more: [`lead_text`]
/// takes the length in seconds, so a sub-hour lead keeps its minutes. It
/// remains for callers of 0.6.0, and [`lead_text`] uses its `None` arm.
///
/// `hours` is the caller's answer to "is this lead expressible in hours, and if
/// so how many" — not a conversion this function performs. A unit with no fixed
/// length in hours has no such answer, and passing `None` is what keeps a
/// six-month mean from being printed as `+6h`.
///
/// ```
/// use fieldglass_core::lead_time::lead_label;
/// assert_eq!(lead_label(Some(24), 1, "Day"), "24h");
/// assert_eq!(lead_label(None, 6, "Month"), "6 Month");
/// ```
#[must_use]
pub fn lead_label(hours: Option<i64>, value: i64, unit_label: &str) -> String {
    match hours {
        Some(h) => format!("{h}h"),
        None => format!("{value} {unit_label}"),
    }
}

/// The coarsest of hours, minutes and seconds that states every one of
/// `seconds` exactly, as its divisor and suffix.
fn exact_unit(seconds: &[i64]) -> (i64, &'static str) {
    [(3_600, "h"), (60, "min")]
        .into_iter()
        .find(|(d, _)| seconds.iter().all(|s| s % d == 0))
        .unwrap_or((1, "s"))
}

/// A length of time given in seconds, as text: `24h` when it is a whole
/// number of hours, `30min` when it is a whole number of minutes, and `90s`
/// otherwise.
///
/// The spelling depends on the length alone, never on the unit the producer
/// stated it in, so a two-day lead reads `48h` whether the message said 2
/// days or 48 hours, and 120 minutes reads `2h`. A sub-hour length keeps its
/// exact value rather than being rounded to hours, which made a 15-minute
/// interval read `0h` (#1554).
///
/// ```
/// use fieldglass_core::lead_time::duration_text;
/// assert_eq!(duration_text(172_800), "48h");
/// assert_eq!(duration_text(1_800), "30min");
/// assert_eq!(duration_text(90), "90s");
/// ```
#[must_use]
pub fn duration_text(seconds: i64) -> String {
    let (d, suffix) = exact_unit(&[seconds]);
    format!("{}{suffix}", seconds / d)
}

/// A lead as text: [`duration_text`] of its length when the unit has a fixed
/// length, and the producer's own number and unit label when it has none.
///
/// `seconds` is the caller's answer to "how long is this lead", from its own
/// unit table: `None` for the calendar units (month, year, decade, normal,
/// century) and for every code the table reserves or leaves to local use.
/// Passing `None` is what keeps a six-month mean from being printed as `+6h`,
/// and a local-use unit renders with whatever name the table gives it.
///
/// The sign and any surrounding wording belong to the caller: GRIB1 renders
/// some averages as `−{a} to −{b} average` around this, and both editions
/// prepend the `+` of a plain forecast.
///
/// ```
/// use fieldglass_core::lead_time::lead_text;
/// assert_eq!(lead_text(Some(86_400), 1, "Day"), "24h");
/// assert_eq!(lead_text(Some(1_800), 30, "Minute"), "30min");
/// assert_eq!(lead_text(None, 6, "Month"), "6 Month");
/// ```
#[must_use]
pub fn lead_text(seconds: Option<i64>, value: i64, unit_label: &str) -> String {
    match seconds {
        Some(s) => duration_text(s),
        None => lead_label(None, value, unit_label),
    }
}

/// The range from `start` to `end`, both in seconds, as text in one unit: the
/// coarsest of hours, minutes and seconds that states both ends exactly
/// (`0–6h`, `60–90min`). One unit for both ends keeps a range from reading
/// `1h–90min`.
///
/// ```
/// use fieldglass_core::lead_time::span_text;
/// assert_eq!(span_text(0, 21_600), "0–6h");
/// assert_eq!(span_text(3_600, 5_400), "60–90min");
/// ```
#[must_use]
pub fn span_text(start: i64, end: i64) -> String {
    let (d, suffix) = exact_unit(&[start, end]);
    format!("{}–{}{suffix}", start / d, end / d)
}

/// A lead in seconds as the whole hours a display column holds: rounded to
/// the nearest hour, a half hour away from zero, then narrowed by
/// [`saturating_hours`].
///
/// Rounded rather than truncated, so a 90-minute lead is 2 rather than 1, and
/// no lead is more than half an hour from its number. A 0/15/30/45-minute
/// series still shares hours (0, 0, 1, 1): the number is a coarse sort key,
/// and the exact lead is in [`lead_text`].
///
/// ```
/// use fieldglass_core::lead_time::lead_hours;
/// assert_eq!(lead_hours(1_800), 1);
/// assert_eq!(lead_hours(1_799), 0);
/// assert_eq!(lead_hours(-5_400), -2);
/// ```
#[must_use]
pub fn lead_hours(seconds: i64) -> i32 {
    let (hours, rest) = (seconds / 3_600, seconds % 3_600);
    // `rest` has the sign of `seconds` and is under an hour in size, so
    // neither the `abs` nor the step can overflow.
    let hours = if rest.abs() >= 1_800 {
        hours + rest.signum()
    } else {
        hours
    };
    saturating_hours(hours)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representable_hours_pass_through_unchanged() {
        for h in [
            0i64,
            1,
            -1,
            24,
            -24,
            204,
            i64::from(i32::MAX),
            i64::from(i32::MIN),
        ] {
            assert_eq!(i64::from(saturating_hours(h)), h, "{h}");
        }
    }

    /// The bound this exists for: one past each end must land *on* the end, not
    /// wrap to the other one.
    #[test]
    fn a_lead_past_the_column_saturates_rather_than_wrapping() {
        assert_eq!(saturating_hours(i64::from(i32::MAX) + 1), i32::MAX);
        assert_eq!(saturating_hours(i64::from(i32::MIN) - 1), i32::MIN);
        assert_eq!(saturating_hours(i64::MAX), i32::MAX);
        assert_eq!(saturating_hours(i64::MIN), i32::MIN);
    }

    /// An hours value is printed as hours whatever the producer's own unit was,
    /// because that is the caller saying it converted the number.
    #[test]
    fn an_hours_value_wins_over_the_producers_unit() {
        assert_eq!(lead_label(Some(48), 2, "Day"), "48h");
        assert_eq!(lead_label(Some(0), 30, "Minute"), "0h");
        assert_eq!(lead_label(Some(-12), -12, "Hour"), "-12h");
        // Not narrowed on the way through: a lead past the display column is
        // still printed in full, because narrowing is `saturating_hours`'s job
        // and a string has no column to overflow.
        assert_eq!(
            lead_label(Some(i64::from(i32::MAX) + 1), 0, "Hour"),
            "2147483648h"
        );
    }

    /// The rule the module exists for: no hours value means the producer's own
    /// number and label survive, rather than a lead time being invented for a
    /// unit that has none.
    #[test]
    fn no_hours_value_keeps_the_producers_number_and_label() {
        assert_eq!(lead_label(None, 6, "Month"), "6 Month");
        assert_eq!(lead_label(None, 5, "Century"), "5 Century");
        assert_eq!(lead_label(None, 0, "Missing"), "0 Missing");
        // Distinct steps stay distinguishable, which is the point of keeping
        // the producer's number rather than a rounded one.
        assert_ne!(
            lead_label(None, 15, "Minute"),
            lead_label(None, 45, "Minute")
        );
    }

    /// Each length takes the coarsest unit that states it exactly, at each
    /// boundary between the three.
    #[test]
    fn a_length_reads_in_the_coarsest_exact_unit() {
        for (seconds, text) in [
            (0, "0h"),
            (3_600, "1h"),
            (172_800, "48h"),
            (7_200, "2h"),
            (60, "1min"),
            (1_800, "30min"),
            (5_400, "90min"),
            (3_660, "61min"),
            (1, "1s"),
            (90, "90s"),
            (3_601, "3601s"),
            (-1_800, "-30min"),
            (-21_600, "-6h"),
            (-90, "-90s"),
        ] {
            assert_eq!(duration_text(seconds), text, "{seconds} s");
        }
        // Past `i32` hours, still printed in full: a string has no column.
        assert_eq!(
            duration_text(i64::MAX - i64::MAX % 3_600),
            "2562047788015215h"
        );
    }

    #[test]
    fn a_lead_with_a_length_ignores_the_producers_label() {
        assert_eq!(lead_text(Some(86_400), 1, "Day"), "24h");
        assert_eq!(lead_text(Some(900), 1, "15 minutes"), "15min");
        assert_eq!(lead_text(None, 6, "Month"), "6 Month");
        assert_eq!(
            lead_text(None, 3, "time-range unit 99"),
            "3 time-range unit 99"
        );
    }

    /// Both ends share the unit that states both exactly, so a range never
    /// mixes units.
    #[test]
    fn a_range_reads_both_ends_in_one_unit() {
        assert_eq!(span_text(0, 21_600), "0–6h");
        assert_eq!(span_text(3_600, 5_400), "60–90min");
        assert_eq!(span_text(5_400, 3_600), "90–60min");
        assert_eq!(span_text(0, 90), "0–90s");
        assert_eq!(span_text(60, 3_600), "1–60min");
    }

    /// The rounding rule at each side of every half hour, in both signs.
    #[test]
    fn hours_round_to_the_nearest_with_halves_away_from_zero() {
        for (seconds, hours) in [
            (0, 0),
            (1_799, 0),
            (1_800, 1),
            (3_599, 1),
            (3_600, 1),
            (5_399, 1),
            (5_400, 2),
            (-1_799, 0),
            (-1_800, -1),
            (-5_400, -2),
            (86_400, 24),
        ] {
            assert_eq!(lead_hours(seconds), hours, "{seconds} s");
        }
        assert_eq!(lead_hours(i64::MAX), i32::MAX);
        assert_eq!(lead_hours(i64::MIN), i32::MIN);
    }
}
