//! The two rules a forecast lead time's display shares across GRIB editions.
//!
//! GRIB1 and GRIB2 both state a lead time as a number plus a unit code, and
//! both hosts render it the same way: as whole hours when the lead has a
//! representation in hours, and otherwise as the producer's own number beside
//! the producer's own label. Both also refuse to let a lead too large for the
//! `i32` display column wrap into a small — or negative — one.
//!
//! Those two rules live here so there is one of each. What deliberately does
//! **not** live here is everything that is genuinely per-edition:
//!
//! * **The unit tables.** WMO ON388 Table 4 (GRIB1) and Code Table 4.4 (GRIB2)
//!   are different tables that reuse the same small integers: `13` is fifteen
//!   minutes in GRIB1 and one second in GRIB2, `14` is thirty minutes in GRIB1
//!   and has no GRIB2 counterpart, and a second is `254` in GRIB1 and `13` in
//!   GRIB2. A shared table would decode one edition's lead with the other's
//!   meaning.
//! * **Which leads count as hours.** GRIB1 converts every convertible unit, so
//!   a lead stated in days displays in hours. GRIB2 keeps the producer's unit
//!   for everything but the hour unit itself, so a nowcast series stated in
//!   minutes stays legible step by step. Both callers say which they mean by
//!   what they pass as `hours`.
//! * **The rounding policy.** GRIB1 rounds to the nearest hour, so a
//!   thirty-minute unit cannot call ninety minutes `+1h` by truncation; GRIB2
//!   truncates toward zero and documents its hours column as a coarse sort key.
//!   Unifying those would silently change one edition's numbers.

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
/// `hours` is the caller's answer to "is this lead expressible in hours, and if
/// so how many" — not a conversion this function performs. A unit with no fixed
/// length in hours (month, year, decade, normal, century) has no such answer,
/// and passing `None` is what keeps a six-month mean from being printed as
/// `+6h`.
///
/// The sign and any surrounding wording belong to the caller: GRIB1 renders
/// accumulation bounds as `−{a} to −{b} average` around this, and both editions
/// prepend the `+` of a plain forecast.
///
/// ```
/// use fieldglass_core::lead_time::lead_label;
/// assert_eq!(lead_label(Some(24), 1, "Day"), "24h");
/// assert_eq!(lead_label(None, 6, "Month"), "6 Month");
/// ```
#[must_use]
pub fn lead_label(hours: Option<i32>, value: i64, unit_label: &str) -> String {
    match hours {
        Some(h) => format!("{h}h"),
        None => format!("{value} {unit_label}"),
    }
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
}
