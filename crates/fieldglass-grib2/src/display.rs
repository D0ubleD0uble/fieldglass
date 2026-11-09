//! Level and forecast-time strings for a §4 product.
//!
//! `level_value_str`, `level_type_str`, `forecast_hours` and `forecast_display`
//! are the GRIB1 crate's four counterparts **by name**, and so is
//! `offset_hours`. The `fieldglass` crate's `MessageInfo.forecast_hours` and
//! `offset_hours` come from the functions of those names in both editions.
//! They are not the same function twice, and two differences are worth
//! knowing before reading one as the other:
//!
//! * GRIB1 has no level value for a fixed-surface type, where GRIB2 prints the
//!   surface's own name ("Ground or water surface"): GRIB2's Code Table 4.5
//!   entry *is* the description, and blanking it would lose the only thing the
//!   column has to say.
//!
//!   The rule is by surface type, never by whether the message wrote a value:
//!   a surface that names a place (the ground, mean sea level, the tropopause)
//!   always shows its name, even when the producer writes a zero beside it, as
//!   HRRR does and ECMWF does not (#1396). Only a coordinate (a height, a
//!   pressure, a hybrid level number) shows its number. Which types are which
//!   is `tables::is_named_surface`.
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
//! How a lead reads, and which whole hours it counts as, are
//! [`fieldglass_core::lead_time`]'s, so the same lead reads the same in both
//! editions (#1565). That module records what stays here and why: chiefly the
//! unit table, Code Table 4.4.
//!
//! A statistically processed product (4.8, 4.9, 4.10, 4.11, 4.12 and the
//! chemistry and aerosol templates 4.42, 4.43, 4.46 and 4.47) has a window,
//! not a lead: [`product_forecast_display`] says what was done over which
//! hours, `0–6h accum` as GRIB1 says it (#1566). [`forecast_display`] reads
//! the forecast time alone, which for those products is where the window
//! starts.

use fieldglass_core::lead_time::{
    MISSING_TIME_UNIT, calendar_unit_label, duration_text, lead_text, saturating_hours, span_text,
};

use crate::pds::{
    HorizontalProductCommon, ProductDefinitionSection, StatisticalProcessing, TimeRangeSpec,
};
use crate::tables::{
    is_named_surface, lookup_fixed_surface, lookup_time_range_unit, time_range_unit_seconds,
};

/// The first fixed surface's value, rendered.
///
/// `None` when the surface type is the WMO missing sentinel, so a host chooses
/// its own placeholder (#775). The surface's own name for a surface that names
/// a place rather than a coordinate ("Ground or water surface", "Mean sea
/// level"), whether or not the message states a value for it (#1396). The
/// decoded value for any other surface that carries one, and the name again
/// when it carries none. The unit hint is in [`level_type_str`], which is the
/// column beside it.
///
/// Reads `first_surface` only. For a layer product that is the *top* surface;
/// the bottom is in `second_surface` and no host renders it today.
#[must_use]
pub fn level_value_str(common: &HorizontalProductCommon) -> Option<String> {
    let surface = &common.first_surface;
    if surface.is_missing() {
        return None;
    }
    if is_named_surface(surface.surface_type) {
        return Some(surface_name(surface.surface_type));
    }
    Some(match surface.value() {
        Some(v) => format!("{v}"),
        None => surface_name(surface.surface_type),
    })
}

/// A Code Table 4.5 surface type's name, or `"Fixed surface <n>"` for a code
/// the table does not name (an unassigned or local-use one), so the code
/// survives into the column (#774).
fn surface_name(code: u8) -> String {
    lookup_fixed_surface(code).map_or_else(|| format!("Fixed surface {code}"), str::to_string)
}

/// A Code Table 4.4 unit's label as a lead's text spells it, or
/// `"time-range unit <n>"` for a code the table does not name, so a lead
/// stated in it still says which unit (#774).
///
/// A calendar unit reads as [`calendar_unit_label`] spells it for both
/// editions, so a 6-month lead reads `+6 month` in GRIB1 and GRIB2 alike
/// (#1595). Every other label is the table's own.
fn unit_label(code: u8) -> String {
    calendar_unit_label(code)
        .or_else(|| lookup_time_range_unit(code))
        .map_or_else(|| format!("time-range unit {code}"), str::to_string)
}

/// The first fixed surface's name (Code Table 4.5).
///
/// GRIB2 states the unit inside the surface name — "Specified height above
/// ground (m)" — where GRIB1 needs a separate `level_unit` to reunite the two
/// columns, so there is no GRIB2 counterpart to that function.
#[must_use]
pub fn level_type_str(common: &HorizontalProductCommon) -> String {
    surface_name(common.first_surface.surface_type)
}

/// The lead time in whole hours, or `None` for a unit with no fixed length in
/// hours: the calendar units (month, year, decade, normal, century), the
/// missing sentinel, and every code Table 4.4 reserves or leaves to local use.
/// That holds for a lead of 0 too. [`forecast_display`] renders those with
/// whatever label the table gives them, and with the code itself
/// (`"time-range unit 99"`) where it gives none.
///
/// A **coarse** sort key, not the exact lead: a sub-hour unit truncates toward
/// zero, so a 0/15/30/45-minute nowcast series (MRMS states its lead in
/// minutes) answers `0` for every step, and 70,000 minutes is 1166 hours. The
/// exact value is always in [`forecast_display`], which keeps the producer's
/// lead.
///
/// The units are WMO Code Table 4.4, which is *not* GRIB1's ON388 Table 4 even
/// where the numbers overlap: `13` is a second here and fifteen minutes there.
///
/// [`offset_hours`] is the same lead with two differences: it rounds a
/// sub-hour lead to the nearest hour, so 30 minutes is 1 there, and a lead of
/// 0 is 0 hours in a calendar, reserved or local-use unit there.
#[must_use]
pub fn forecast_hours(common: &HorizontalProductCommon) -> Option<i32> {
    // Only a struct literal can overflow `i64` seconds: §4 states the
    // forecast time in 31 bits of magnitude. Truncated, as 0.6.0 promised
    // (#1694); `saturating_hours` narrows.
    time_range_unit_seconds(common.forecast_time_unit)
        .map(|per| saturating_hours(common.forecast_time.saturating_mul(per) / 3_600))
}

/// The hours from the reference time to the start of what the product
/// describes, in whole hours, or `None` for a nonzero lead in a unit with no
/// fixed length in hours: the calendar units (month, year, decade, normal,
/// century) and every code Table 4.4 reserves or leaves to local use. A lead
/// of 0 is 0 hours in those units. `None` for any lead in the missing unit
/// (255), 0 included: the message does not say what the number counts, so no
/// lead is made up for it (#1595).
/// [`forecast_display`] renders those with whatever label the table gives them,
/// and with the code itself (`"time-range unit 99"`) where it gives none.
///
/// The forecast time, which for a statistic over a time interval is the start
/// of the interval: the rule GRIB1's `offset_hours` follows too, which
/// [`fieldglass_core::lead_time`] states. So a 0–6 h accumulation is 0, a
/// 12–36 h one 12, and a statistic over runs at a forecast time of 0 (an
/// average of analyses) is 0, as GRIB1's codes 113 and 123 are.
///
/// It stays the start when [`product_forecast_display`] shows a statistic
/// raw, which is where the editions part: GRIB1 has no offset for a
/// product it shows raw, a statistic whose step is zero included (code 123
/// with P2 = 0). The exception is a GRIB1 statistic over analyses with a step
/// (P2 > 0) in the missing time unit, such as `time range 123, P1 0 Missing, P2 6
/// Missing`: it is shown raw and keeps its 0, since where it starts does not
/// use the step. [`fieldglass_core::lead_time`] records why.
///
/// It differs from [`forecast_hours`] in rounding a sub-hour lead to the
/// nearest hour, a half hour away from zero
/// ([`fieldglass_core::lead_time::lead_hours`]), so a 0/15/30/45-minute series
/// answers 0, 0, 1, 1 where that one truncates, and in giving a lead of 0 in a
/// calendar, reserved or local-use unit 0 hours rather than `None`. It is the
/// number the `fieldglass` crate reports as a message's `offset_hours`.
#[must_use]
pub fn offset_hours(common: &HorizontalProductCommon) -> Option<i32> {
    if common.forecast_time_unit == MISSING_TIME_UNIT {
        return None;
    }
    // Only a struct literal can overflow `i64` seconds: §4 states the
    // forecast time in 31 bits of magnitude. `offset_hours` saturates.
    fieldglass_core::lead_time::offset_hours(
        common.forecast_time,
        time_range_unit_seconds(common.forecast_time_unit),
    )
}

/// The lead time, rendered: `"+24h"`, `"+30min"`, `"+90s"`, `"+6 month"`.
///
/// The forecast time alone. For a statistically processed product that is
/// the start of its window, so a 0–6 h accumulation reads `+0h` here; a
/// message list wants [`product_forecast_display`], which shows the window.
///
/// Spelled by [`fieldglass_core::lead_time::lead_text`], the same as GRIB1: in
/// hours when the lead is a whole number of hours, whatever unit the producer
/// stated it in (two days read `+48h`, 60 minutes `+1h`), and otherwise in
/// minutes, or seconds, so the exact lead survives where [`forecast_hours`]
/// and [`offset_hours`] lose it to whole hours. A unit with no fixed length
/// keeps the producer's own number and the unit's label, a calendar unit's in
/// lowercase as in GRIB1 (#1595).
///
/// A negative lead (§4 states forecast time in sign-magnitude, and a message
/// really can say −6) is written with a true minus (U+2212), `"−6h"`, as the
/// ends of a statistic's window are. Before 0.6.2 it read `"+-6h"`.
#[must_use]
pub fn forecast_display(common: &HorizontalProductCommon) -> String {
    let unit = common.forecast_time_unit;
    // Un-narrowed: the string has no column to overflow, so a lead past `i32`
    // hours prints in full here even though `forecast_hours` saturates it. A
    // struct literal past `i64` seconds keeps the producer's number and unit.
    let seconds =
        time_range_unit_seconds(unit).and_then(|per| common.forecast_time.checked_mul(per));
    let lead = common.forecast_time;
    let label = unit_label(unit);
    match lead.checked_abs() {
        // The magnitude carries the text; `signed` picks the sign.
        Some(magnitude) => signed(
            lead < 0,
            lead_text(seconds.map(i64::abs), magnitude, &label),
        ),
        // Only a struct literal holds `i64::MIN`; keep its number as it is.
        None => format!("+{}", lead_text(seconds, lead, &label)),
    }
}

/// What the forecast column shows for a product: its lead, or for a
/// statistically processed product the window and what was done over it.
/// `None` for a template that carries no forecast time.
///
/// A product with no time-range loop reads as [`forecast_display`]. One with
/// a loop (Templates 4.8, 4.9, 4.10, 4.11, 4.12, 4.42, 4.43, 4.46 and 4.47)
/// reads as GRIB1's periods do, by WMO FM 92 Code Tables 4.10 and 4.11:
///
/// - One time range whose successive fields share a reference time and step
///   forward in forecast time (Code Table 4.11 type 2) is a window from the
///   forecast time, where §4 Note 2 puts the start, to the forecast time
///   plus its length: `0–6h accum`, `6–12h min`. The time increment is how
///   often it was sampled inside the window, so it does not move either end.
///   A window of no length reads as one, `0–0h accum`, as GRIB1's
///   accumulation from P1 = P2 = 0 does.
/// - One time range whose successive fields share a forecast time and come
///   from runs one increment apart (type 1) is a statistic over runs:
///   `average of analyses every 6h over 720h` for a forecast time of 0, and
///   `average of +24h forecasts issued every 24h over 720h` otherwise, in
///   GRIB1's words for codes 113 and 123.
/// - Two time ranges, the outer one of type 1 over an inner one of type 2,
///   are a statistic over runs of a statistic over each run's window, as in
///   CFSR's monthly means of 6-hour accumulations. Note 4 of Template 4.8
///   puts the outer range first: the reference time steps by its increment,
///   and each run's inner range is processed from the forecast time. They
///   read as GRIB1's NCEP codes 128 to 131 and 137 to 140 do, with the
///   length of the runs after them as a statistic over runs has it:
///   `average of 0–6h accums issued every 6h over 1 month` (#1622). The
///   inner window follows the rules of a type 2 window above, except that
///   one of no length is raw, as GRIB1's codes over several products are.
/// - Anything else is shown raw, with every time range as the message
///   states it, so it never reads as a window it may not be: any other
///   number of time ranges, any other type or pair of types (3 to 5,
///   reserved, local or missing, or two of type 2), a time unit that is
///   missing, a window whose ends have no common unit (a start of 12 hours
///   and a length of a month), and a statistic over runs that are not a
///   step apart or span no time. For example `+0h, statistics: average over
///   1 month, increment 6h of type 2 (same run); accum over 6h, increment
///   0h of type 2 (same run)`. It lists the first two time ranges and counts
///   the rest (`and 253 more`), since a loop can state 255.
///
/// A window that starts before the reference time (§4 states the forecast
/// time in sign-magnitude) reads as GRIB1's codes 6 and 7 do, each end
/// signed: `−6h to −3h max`.
///
/// The statistical process is a word of Code Table 4.10's, spelled as GRIB1
/// spells the ones both editions have (`average`, `accum`, `diff`), and a
/// code the table does not name keeps its number (`statistical process
/// 200`).
#[must_use]
pub fn product_forecast_display(pds: &ProductDefinitionSection) -> Option<String> {
    let common = pds.common()?;
    Some(match pds.stats() {
        None => forecast_display(common),
        Some(stats) => {
            statistic_display(common, stats).unwrap_or_else(|| raw_statistic(common, stats))
        }
    })
}

/// The window, or the runs, a statistic covers; `None` for a loop that is
/// shown raw (see [`product_forecast_display`]).
fn statistic_display(
    common: &HorizontalProductCommon,
    stats: &StatisticalProcessing,
) -> Option<String> {
    match stats.specs.as_slice() {
        [spec] => {
            let process = process_word(spec.stat_process);
            match spec.time_increment_type {
                2 => Some(format!("{} {process}", window_span(common, spec)?)),
                1 => over_runs(common, spec, &process),
                _ => None,
            }
        }
        [outer, inner] => runs_over_windows(common, outer, inner),
        _ => None,
    }
}

/// An outer time range of type 1 over an inner one of type 2: `average of
/// 0–6h accums issued every 6h over 1 month`. `None` for any other pair of
/// types, for an outer range [`over_runs`] would show raw, and for an inner
/// window [`window_span`] cannot write or that covers no time, as GRIB1
/// shows a code over several products raw when its period covers none.
///
/// The inner averages and accumulations are named in GRIB1's words for
/// NCEP's codes (`averages`, `accums`), and any other process by its word
/// and `products`, as GRIB1 names ECMWF's products of code 128: `max of
/// 0–24h max products issued every 24h over 720h`.
fn runs_over_windows(
    common: &HorizontalProductCommon,
    outer: &TimeRangeSpec,
    inner: &TimeRangeSpec,
) -> Option<String> {
    if (outer.time_increment_type, inner.time_increment_type) != (1, 2) || inner.stat_length == 0 {
        return None;
    }
    let (every, over) = run_series(common, outer)?;
    let window = window_span(common, inner)?;
    let products = match inner.stat_process {
        0 => "averages".to_string(),
        1 => "accums".to_string(),
        other => format!("{} products", process_word(other)),
    };
    Some(format!(
        "{} of {window} {products} issued every {every} over {over}",
        process_word(outer.stat_process)
    ))
}

/// `value` units of Table 4.4 `unit` in seconds: 0 in any unit that is not
/// missing, since 0 is the reference time itself, and otherwise `None` for a
/// unit with no fixed length or a product past `i64`.
fn seconds_of(unit: u8, value: i64) -> Option<i64> {
    if unit == MISSING_TIME_UNIT {
        return None;
    }
    if value == 0 {
        return Some(0);
    }
    time_range_unit_seconds(unit).and_then(|per| value.checked_mul(per))
}

/// A value in Table 4.4 `unit` as text, by the rule a lead follows.
fn value_text(unit: u8, value: i64) -> String {
    let seconds = time_range_unit_seconds(unit).and_then(|per| value.checked_mul(per));
    lead_text(seconds, value, &unit_label(unit))
}

/// The window from the forecast time to the forecast time plus the length:
/// `0–6h`, both ends in one unit, or `0–1 month` in a unit with no fixed
/// length. A window that starts before the reference time reads as GRIB1's
/// codes 6 and 7 do, each end signed and in its own unit, `−6h to −3h`.
/// `None` when either unit is missing, or the two ends have no unit in
/// common.
fn window_span(common: &HorizontalProductCommon, spec: &TimeRangeSpec) -> Option<String> {
    let (start_unit, start) = (common.forecast_time_unit, common.forecast_time);
    let (length_unit, length) = (spec.stat_length_unit, i64::from(spec.stat_length));
    if start_unit == MISSING_TIME_UNIT || length_unit == MISSING_TIME_UNIT {
        return None;
    }
    if let (Some(s), Some(l)) = (
        seconds_of(start_unit, start),
        seconds_of(length_unit, length),
    ) {
        let e = s.checked_add(l)?;
        if s < 0 {
            let end = |x: i64| Some(signed(x < 0, duration_text(x.checked_abs()?)));
            return Some(format!("{} to {}", end(s)?, end(e)?));
        }
        return Some(span_text(s, e));
    }
    // No length in seconds: one end has a calendar or unnamed unit. The ends
    // still share it when both are stated in it, or when the start is 0.
    if start_unit == length_unit || start == 0 {
        let e = start.checked_add(length)?;
        let label = unit_label(length_unit);
        if start < 0 {
            let end = |x: i64| Some(signed(x < 0, format!("{} {label}", x.checked_abs()?)));
            return Some(format!("{} to {}", end(start)?, end(e)?));
        }
        return Some(format!("{start}–{e} {label}"));
    }
    None
}

/// `text` after a true minus sign (U+2212) when `negative`, and a `+`
/// otherwise, as GRIB1 writes the ends of its codes 6 and 7.
fn signed(negative: bool, text: String) -> String {
    format!("{}{text}", if negative { '\u{2212}' } else { '+' })
}

/// A statistic over runs one increment apart, each at the same forecast
/// time (Code Table 4.11 type 1). `None` when the increment or the length
/// is zero, since runs no time apart, or over no time, are not a series, or
/// when a unit is missing; [`product_forecast_display`] then shows the loop
/// raw. The text follows GRIB1, which shows a statistic whose step is zero
/// raw too (#1554), but the hours do not: see [`offset_hours`].
fn over_runs(
    common: &HorizontalProductCommon,
    spec: &TimeRangeSpec,
    process: &str,
) -> Option<String> {
    let (every, over) = run_series(common, spec)?;
    let lead = common.forecast_time;
    Some(if lead == 0 {
        format!("{process} of analyses every {every} over {over}")
    } else {
        let lead = signed(
            lead < 0,
            value_text(common.forecast_time_unit, lead.checked_abs()?),
        );
        format!("{process} of {lead} forecasts issued every {every} over {over}")
    })
}

/// How far apart a type 1 range's runs are and how long they go on for, as
/// text: `("6h", "1 month")`. `None` when the increment or the length is
/// zero, or when it or the forecast time has a missing unit (see
/// [`over_runs`]).
fn run_series(common: &HorizontalProductCommon, spec: &TimeRangeSpec) -> Option<(String, String)> {
    let units = [
        common.forecast_time_unit,
        spec.stat_length_unit,
        spec.increment_unit,
    ];
    if units.contains(&MISSING_TIME_UNIT) || spec.increment == 0 || spec.stat_length == 0 {
        return None;
    }
    Some((
        value_text(spec.increment_unit, i64::from(spec.increment)),
        value_text(spec.stat_length_unit, i64::from(spec.stat_length)),
    ))
}

/// How many time ranges the raw form lists before it counts the rest. A
/// loop can state 255, and a message list has one line for it.
const RAW_RANGES_SHOWN: usize = 2;

/// Every time range as the message states it, after the forecast time: the
/// first [`RAW_RANGES_SHOWN`], then how many more.
fn raw_statistic(common: &HorizontalProductCommon, stats: &StatisticalProcessing) -> String {
    if stats.specs.is_empty() {
        return format!("{}, no statistical time range", raw_lead(common));
    }
    let mut ranges: Vec<String> = stats
        .specs
        .iter()
        .take(RAW_RANGES_SHOWN)
        .map(|spec| {
            format!(
                "{} over {}, increment {} of type {}",
                process_word(spec.stat_process),
                value_text(spec.stat_length_unit, i64::from(spec.stat_length)),
                value_text(spec.increment_unit, i64::from(spec.increment)),
                increment_type(spec.time_increment_type)
            )
        })
        .collect();
    let rest = stats.specs.len().saturating_sub(RAW_RANGES_SHOWN);
    if rest > 0 {
        ranges.push(format!("and {rest} more"));
    }
    format!("{}, statistics: {}", raw_lead(common), ranges.join("; "))
}

/// The forecast time ahead of a raw loop, signed as the windows are: `+6h`,
/// or `−6h` before the reference time where [`forecast_display`] would
/// write `+-6h`.
fn raw_lead(common: &HorizontalProductCommon) -> String {
    let lead = common.forecast_time;
    lead.checked_abs().map_or_else(
        || forecast_display(common),
        |magnitude| signed(lead < 0, value_text(common.forecast_time_unit, magnitude)),
    )
}

/// A Code Table 4.11 type of time increment, with a short gloss for the
/// types the table names: `1 (successive runs)`, `2 (same run)`.
fn increment_type(code: u8) -> String {
    let gloss = match code {
        1 => "successive runs",
        2 => "same run",
        3 => "runs forward, valid time fixed",
        4 => "runs back, valid time fixed",
        5 => "floating subinterval",
        255 => "missing",
        _ => return code.to_string(),
    };
    format!("{code} ({gloss})")
}

/// A Code Table 4.10 statistical process as the forecast column names it.
/// The three GRIB1's Code Table 5 also has are spelled as GRIB1 spells them:
/// `average` (code 3 there), `accum` (4) and `diff` (5, P2 minus P1, which
/// is 4.10's end minus start).
fn process_word(code: u8) -> String {
    match code {
        0 => "average",
        1 => "accum",
        2 => "max",
        3 => "min",
        4 => "diff",
        5 => "root mean square",
        6 => "std dev",
        7 => "covariance",
        8 => "diff (start minus end)",
        9 => "ratio",
        10 => "standardized anomaly",
        11 => "sum",
        12 => "return period",
        13 => "median",
        100 => "severity",
        101 => "mode",
        102 => "index processing",
        // Code Table 4.10 names 255 "Missing", as every table names it.
        255 => "statistical process missing",
        other => return format!("statistical process {other}"),
    }
    .to_string()
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
    fn forecast_hours_normalizes_each_convertible_unit() {
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
            assert_eq!(
                offset_hours(&common(unit, raw)),
                Some(want),
                "unit {unit} raw {raw}"
            );
        }
    }

    /// The spelling follows the lead's length, not the producer's unit, so a
    /// lead reads as it does in GRIB1 (#1565). The exact sub-hour value
    /// survives where the hours numbers lose it.
    #[test]
    fn forecast_display_spells_the_lead_by_its_length() {
        assert_eq!(forecast_display(&common(1, 24)), "+24h");
        assert_eq!(forecast_display(&common(0, 30)), "+30min");
        assert_eq!(forecast_display(&common(13, 90)), "+90s");
        assert_eq!(forecast_display(&common(13, 150)), "+150s");
        // eccc states a one-hour lead in minutes; it reads as the hour it is.
        assert_eq!(forecast_display(&common(0, 60)), "+1h");
        assert_eq!(forecast_display(&common(0, 90)), "+90min");
        assert_eq!(forecast_display(&common(2, 2)), "+48h");
        assert_eq!(forecast_display(&common(10, 2)), "+6h");
        assert_eq!(forecast_display(&common(13, 7_200)), "+2h");
    }

    /// The documented coarseness, pinned so it is a choice rather than a
    /// surprise: `forecast_hours` truncates a sub-hour lead toward zero, as
    /// 0.6.0 promised (#1694), and `offset_hours` rounds it to the nearest
    /// hour, a half hour away from zero. The display string is the only exact
    /// record of the step.
    #[test]
    fn forecast_hours_truncates_and_offset_hours_rounds_sub_hour_leads() {
        // (minutes, forecast_hours, offset_hours)
        for (raw, truncated, rounded) in [
            (0i64, 0, 0),
            (15, 0, 0),
            (29, 0, 0),
            (30, 0, 1),
            (45, 0, 1),
            (59, 0, 1),
            (60, 1, 1),
            (89, 1, 1),
            (90, 1, 2),
            (70_000, 1_166, 1_167),
        ] {
            assert_eq!(
                forecast_hours(&common(0, raw)),
                Some(truncated),
                "raw {raw}"
            );
            assert_eq!(offset_hours(&common(0, raw)), Some(rounded), "raw {raw}");
        }
        assert_eq!(forecast_hours(&common(13, 1_799)), Some(0));
        assert_eq!(forecast_hours(&common(13, 1_800)), Some(0));
        assert_eq!(forecast_hours(&common(13, 3_599)), Some(0));
        assert_eq!(forecast_hours(&common(13, 3_600)), Some(1));
        assert_eq!(offset_hours(&common(13, 1_799)), Some(0));
        assert_eq!(offset_hours(&common(13, 1_800)), Some(1));
        // Distinct steps, distinct displays — the exactness lives here.
        assert_ne!(
            forecast_display(&common(0, 15)),
            forecast_display(&common(0, 45)),
        );
        // Negative (sign-magnitude on the wire): toward zero in one, away
        // from zero in the other.
        assert_eq!(forecast_hours(&common(0, -90)), Some(-1));
        assert_eq!(offset_hours(&common(0, -90)), Some(-2));
        assert_eq!(forecast_hours(&common(0, -29)), Some(0));
        // And the display's `+` is unconditional, so a negative lead reads
        // `+-6h`. Pinned because it is inherited rather than chosen: both hosts
        // have shown it this way since GRIB2 products were first listed.
        assert_eq!(forecast_display(&common(1, -6)), "\u{2212}6h");
        assert_eq!(forecast_display(&common(0, -90)), "\u{2212}90min");
    }

    /// A unit with no clean hour conversion yields no hours, but must still
    /// report the raw value and its label rather than inventing a lead time.
    /// A calendar unit is spelled in lowercase, as GRIB1 spells it (#1595).
    #[test]
    fn forecast_leaves_unconvertible_units_to_the_display_string() {
        // (unit, label, offset hours at a forecast time of 0)
        for (unit, label, at_zero) in [
            (3u8, "month", Some(0)),
            (4, "year", Some(0)),
            (5, "decade", Some(0)),
            (6, "normal (30 years)", Some(0)),
            (7, "century", Some(0)),
            // A missing unit does not say what the number counts, so even 0
            // is no lead (owner decision on #1595). The text keeps the
            // table's label.
            (255, "Missing", None),
        ] {
            assert_eq!(forecast_hours(&common(unit, 5)), None, "unit {unit}");
            assert_eq!(offset_hours(&common(unit, 5)), None, "unit {unit}");
            assert_eq!(forecast_display(&common(unit, 5)), format!("+5 {label}"));
            // `forecast_hours` keeps its 0.6.0 promise: no hours for a unit
            // with no fixed length, at 0 as at any other lead (#1680).
            assert_eq!(forecast_hours(&common(unit, 0)), None, "unit {unit}");
            // A forecast time of 0 in a calendar unit is the reference time,
            // so its start is 0 hours, as in GRIB1 (#1565 review).
            assert_eq!(offset_hours(&common(unit, 0)), at_zero, "unit {unit}");
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
        assert_eq!(forecast_display(&common(2, 2_000_000_000)), "+48000000000h");
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
        // Past `i64` seconds, which only a struct literal reaches: the hours
        // saturate and the display keeps the producer's number and unit.
        assert_eq!(forecast_hours(&common(2, i64::MAX)), Some(i32::MAX));
        assert_eq!(forecast_hours(&common(2, i64::MIN)), Some(i32::MIN));
        assert_eq!(
            forecast_display(&common(2, i64::MAX)),
            format!("+{} Day", i64::MAX)
        );
    }

    #[test]
    fn a_missing_surface_has_no_level_to_print() {
        let c = at_surface(surface(255, None, None));
        assert_eq!(level_value_str(&c), None);
        assert_eq!(level_type_str(&c), "Missing");
    }

    /// A surface type the table does not name keeps its code in both columns,
    /// whether WMO leaves it unassigned (50) or delegates it to centers for
    /// local use (242, NCEP's convective cloud bottom), rather than reading as
    /// the same placeholder for every such code (#774).
    #[test]
    fn an_unnamed_surface_keeps_its_code() {
        for code in [50u8, 242] {
            let c = at_surface(surface(code, None, None));
            assert_eq!(level_type_str(&c), format!("Fixed surface {code}"));
            assert_eq!(
                level_value_str(&c).as_deref(),
                Some(format!("Fixed surface {code}").as_str())
            );
        }
    }

    /// A lead in a unit Table 4.4 does not name keeps the unit's code (#774).
    #[test]
    fn an_unnamed_time_unit_keeps_its_code() {
        assert_eq!(forecast_hours(&common(99, 5)), None);
        // Reserved and local-use codes have no hours in `forecast_hours`, at
        // 0 as well (the 0.6.0 promise, #1680); a start of 0 is still 0.
        assert_eq!(forecast_hours(&common(99, 0)), None);
        assert_eq!(forecast_hours(&common(200, 0)), None);
        assert_eq!(offset_hours(&common(99, 0)), Some(0));
        assert_eq!(offset_hours(&common(99, 5)), None);
        assert_eq!(forecast_display(&common(99, 5)), "+5 time-range unit 99");
        assert_eq!(forecast_display(&common(200, 5)), "+5 time-range unit 200");
    }

    /// A surface with no scaled value is named rather than numbered, in both
    /// columns — "Ground or water surface" has no height.
    #[test]
    fn a_valueless_surface_is_named_in_the_value_column() {
        let c = at_surface(surface(1, None, None));
        assert_eq!(
            level_value_str(&c).as_deref(),
            Some("Ground or water surface")
        );
        assert_eq!(level_type_str(&c), "Ground or water surface");
    }

    /// The first message of a committed fixture, through the real reader.
    fn fixture_common(bytes: &[u8]) -> HorizontalProductCommon {
        let reader = crate::Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
        *reader.messages[0]
            .pds
            .common()
            .expect("fixture uses a horizontal product template")
    }

    /// The same surface type reads the same in the Level column whether the
    /// producer wrote a value for it or left it missing (#1396). HRRR writes a
    /// scaled zero for the ground surface; the polar stereographic fixture
    /// leaves it missing. eccodes 2.34.1 agrees on what each message states:
    ///
    /// ```text
    /// $ grib_ls -p typeOfFirstFixedSurface:l,scaledValueOfFirstFixedSurface,level
    /// hrrr_complex_spd_lambert.grib2      1  0        0
    /// polar_stereographic_surface.grib2   1  MISSING  0
    /// ```
    #[test]
    fn both_kinds_of_ground_surface_message_read_the_same_level() {
        let stated = fixture_common(include_bytes!(
            "../tests/fixtures/hrrr_complex_spd_lambert.grib2"
        ));
        let missing = fixture_common(include_bytes!(
            "../tests/fixtures/polar_stereographic_surface.grib2"
        ));
        for c in [&stated, &missing] {
            assert_eq!(c.first_surface.surface_type, 1, "fixture precondition");
        }
        assert_eq!(
            stated.first_surface.value(),
            Some(0.0),
            "fixture precondition"
        );
        assert_eq!(missing.first_surface.value(), None, "fixture precondition");

        assert_eq!(
            level_value_str(&stated).as_deref(),
            Some("Ground or water surface")
        );
        assert_eq!(level_value_str(&stated), level_value_str(&missing));
    }

    /// Every Table 4.5 entry that names a place rather than a coordinate reads
    /// its name, whatever the message writes beside it: missing, zero, or a
    /// stray non-zero number (the constituent fixtures write 2 on the ground
    /// surface). Mean sea level is the other type the corpus states both ways:
    /// ECMWF leaves it missing and NCEP's Eta writes 0.
    #[test]
    fn a_named_surface_reads_the_same_whether_or_not_it_states_a_value() {
        for code in [1u8, 101] {
            assert!(crate::tables::is_named_surface(code), "type {code}");
        }
        for code in (0..=u8::MAX).filter(|&c| crate::tables::is_named_surface(c)) {
            // Only codes the table names, so the column never reads a
            // `Fixed surface <n>` placeholder in place of a value.
            assert!(lookup_fixed_surface(code).is_some(), "type {code}");
            let name = surface_name(code);
            for (factor, value) in [(None, None), (Some(0), Some(0)), (Some(0), Some(2))] {
                let c = at_surface(surface(code, factor, value));
                assert_eq!(
                    level_value_str(&c).as_deref(),
                    Some(name.as_str()),
                    "type {code} with value {value:?}"
                );
            }
        }
    }

    /// A level with a coordinate keeps its number, including the ones WMO gives
    /// no unit: a hybrid or soil level is counted, and the count is the level.
    #[test]
    fn a_coordinate_level_without_a_unit_still_prints_its_number() {
        for code in [104u8, 105, 111, 113, 114, 115, 118, 119, 150, 151, 152, 168] {
            assert!(!crate::tables::is_named_surface(code), "type {code}");
            let c = at_surface(surface(code, Some(0), Some(3)));
            assert_eq!(level_value_str(&c).as_deref(), Some("3"), "type {code}");
        }
        // A local-use code is not classified, so a stated value still prints.
        let c = at_surface(surface(242, Some(0), Some(0)));
        assert_eq!(level_value_str(&c).as_deref(), Some("0"));
    }

    #[test]
    fn a_scaled_surface_value_is_decoded_before_it_is_printed() {
        // 2 m above ground, and a 850 hPa isobaric surface stated in pascals.
        let c = at_surface(surface(103, Some(0), Some(2)));
        assert_eq!(level_value_str(&c).as_deref(), Some("2"));
        assert_eq!(level_type_str(&c), "Specified height above ground (m)");
        let c = at_surface(surface(100, Some(-2), Some(850)));
        assert_eq!(level_value_str(&c).as_deref(), Some("85000"));
        assert_eq!(level_type_str(&c), "Isobaric surface (Pa)");
    }

    /// A product with no time-range loop reads its lead in the message list,
    /// and a template with no forecast time reads nothing (#775). The
    /// statistics are `crates/fieldglass/tests/statistical_windows.rs`'s,
    /// through the session every host reads them from.
    #[test]
    fn a_product_without_a_time_range_loop_reads_its_lead() {
        let reader = crate::Grib2Reader::from_bytes(
            include_bytes!("../tests/fixtures/eta_lambert_msg0.grib2").to_vec(),
        )
        .expect("fixture parses");
        let pds = &reader.messages[0].pds;
        assert!(pds.stats().is_none(), "fixture precondition");
        assert_eq!(product_forecast_display(pds).as_deref(), Some("+24h"));

        let unread = ProductDefinitionSection {
            section_length: 9,
            num_coordinate_values: 0,
            template_number: 99,
            template: crate::pds::ProductTemplate::Unsupported(99, Box::new([])),
        };
        assert_eq!(product_forecast_display(&unread), None);
    }

    /// A statistic's forecast time alone is its window's start: what
    /// [`forecast_display`] reads, and [`product_forecast_display`] does not.
    #[test]
    fn a_statistic_reads_its_window_where_its_start_alone_reads_a_lead() {
        let reader = crate::Grib2Reader::from_bytes(
            include_bytes!("../tests/fixtures/percentile_min_4_10.grib2").to_vec(),
        )
        .expect("fixture parses");
        let pds = &reader.messages[0].pds;
        let common = pds.common().expect("4.10 has the common block");
        assert_eq!(forecast_display(common), "+6h");
        assert_eq!(forecast_hours(common), Some(6));
        assert_eq!(offset_hours(common), Some(6));
        assert_eq!(product_forecast_display(pds).as_deref(), Some("6–12h min"));
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
                // Mean sea level with a scaled value of zero. It names a place
                // rather than a coordinate, so the zero is not printed (#1396);
                // `a_named_surface_reads_the_same_whether_or_not_it_states_a_value`
                // covers the rule.
                "Mean sea level",
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
            assert_eq!(level_value_str(common).as_deref(), Some(level));
            assert_eq!(level_type_str(common), level_type);
            assert_eq!(forecast_hours(common), hours);
            assert_eq!(offset_hours(common), hours);
            assert_eq!(forecast_display(common), display);
        }
    }
}
