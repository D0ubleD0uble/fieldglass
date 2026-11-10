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
/// use the step. So does a GRIB1 period or step from a start of 0 in a unit
/// ON388 Table 4 reserves (#1632). [`fieldglass_core::lead_time`] records
/// why.
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
    let lead = common.forecast_time;
    let label = unit_label(unit);
    match lead.checked_abs() {
        // The magnitude carries the text; `signed` picks the sign.
        Some(magnitude) => signed(lead < 0, value_text(unit, magnitude)),
        // Only a struct literal holds `i64::MIN`, which has no magnitude in
        // an `i64`. Keep its number as it is, with the one true minus.
        None => {
            let seconds = time_range_unit_seconds(unit).and_then(|per| lead.checked_mul(per));
            let text = lead_text(seconds, lead, &label);
            signed(true, text.trim_start_matches('-').to_owned())
        }
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
///   missing, a length or increment in a unit Code Table 4.4 reserves or
///   leaves to local use (#1632), a window whose ends have no common unit
///   (a start of 12 hours and a length of a month), and a statistic over
///   runs that are not a step apart or span no time, or that cannot hold
///   two runs (an increment at least as long as the length, both in
///   fixed-length units: `every 48h over 24h`, #1627). For example `+0h,
///   statistics: average over 1 month, increment 6h of type 2 (same run);
///   accum over 6h, increment 0h of type 2 (same run)`. It lists the first
///   two time ranges and counts the rest (`and 253 more`), since a loop can
///   state 255.
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
        Some(stats) => statistical_window(
            common.forecast_time_unit,
            common.forecast_time,
            &stats.specs,
        )
        .map_or_else(
            || raw_statistic(common, stats),
            |window| window_text(&window),
        ),
    })
}

/// What a §4 time-range loop covers, read by WMO FM 92 Code Tables 4.10 and
/// 4.11: one window within a run, a series of runs at one forecast time, or
/// a series of runs each over a window. [`statistical_window`] reads it.
///
/// It is the one reading of the loop: [`product_forecast_display`] writes it
/// as text, and the `fieldglass` crate's `verify_message` compares a
/// sidecar's forecast field with it (#1624). A loop it does not describe is
/// shown raw there, and `verify_message` compares it as it did before.
///
/// The end of the overall time interval a message states (Template 4.8
/// octets 35-41, and the same octets of the other templates with a loop) is
/// not read. Template 4.8 Note 2 puts the start at the reference time plus
/// the forecast time, and the loop gives the length; the end restates the
/// two, and where it disagrees with them the window is theirs, as eccodes'
/// `stepRange` and wgrib2's forecast field read it. ECMWF's open-data
/// `mx2t3`, a maximum over hours 0 to 3, states its end as the reference
/// time itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StatisticalWindow {
    /// One time range of Code Table 4.11 type 2: successive fields share a
    /// reference time and step forward in forecast time, so the statistic
    /// covers the window from the forecast time, where §4 Note 2 puts the
    /// start, to the forecast time plus the length. The time increment is
    /// how often it was sampled inside the window, so it moves neither end.
    WithinRun(TimeWindow),
    /// One time range of type 1: successive fields share a forecast time and
    /// come from runs one increment apart, as a monthly mean of analyses
    /// does.
    OverRuns(RunSeries),
    /// Two time ranges, the outer of type 1 over an inner of type 2: a
    /// statistic over runs of a statistic over each run's window, as CFSR's
    /// monthly means of 6-hour accumulations are. Template 4.8 Note 4 puts
    /// the outer range first. The window is the inner range's, from the
    /// forecast time, and covers some time.
    RunsOverWindows(RunSeries, TimeWindow),
}

/// The window of a statistic within one run: from the forecast time to the
/// forecast time plus the length, and what was done over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct TimeWindow {
    process: u8,
    ends: WindowEnds,
}

/// Both ends of a [`TimeWindow`], in the one unit they share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowEnds {
    /// In seconds from the reference time.
    Seconds { start: i64, end: i64 },
    /// In a Code Table 4.4 unit with no fixed length, a month or longer: the
    /// unit of the length, which the start is in too or is 0.
    Calendar { start: i64, end: i64, unit: u8 },
}

impl TimeWindow {
    /// The statistical process (Code Table 4.10).
    #[must_use]
    pub fn process(&self) -> u8 {
        self.process
    }

    /// The start and the end in seconds from the reference time. `None` for
    /// a window whose length is in a unit with no fixed length (a month, a
    /// year), which has no exact length in seconds.
    #[must_use]
    pub fn seconds(&self) -> Option<(i64, i64)> {
        match self.ends {
            WindowEnds::Seconds { start, end } => Some((start, end)),
            WindowEnds::Calendar { .. } => None,
        }
    }
}

/// A series of runs one increment apart, each at the same forecast time
/// (Code Table 4.11 type 1), over the length of the time range, and what
/// was done over them. It always holds at least two runs where its length
/// and increment both have a fixed length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RunSeries {
    process: u8,
    lead_unit: u8,
    lead: i64,
    increment_unit: u8,
    increment: u32,
    length_unit: u8,
    length: u32,
}

impl RunSeries {
    /// The statistical process (Code Table 4.10).
    #[must_use]
    pub fn process(&self) -> u8 {
        self.process
    }

    /// The forecast time every run is at, in seconds from its reference
    /// time: where the overall time interval starts from the first run's.
    /// `None` for a nonzero one in a unit with no fixed length.
    #[must_use]
    pub fn start_seconds(&self) -> Option<i64> {
        seconds_of(self.lead_unit, self.lead)
    }

    /// The forecast time plus the length, in seconds from the first run's
    /// reference time: the end of the overall time interval. `None` where
    /// either has no fixed length.
    #[must_use]
    pub fn end_seconds(&self) -> Option<i64> {
        self.start_seconds()?
            .checked_add(seconds_of(self.length_unit, i64::from(self.length))?)
    }

    /// How far apart the runs are and how long they go on for, as text:
    /// `("6h", "1 month")`.
    fn texts(&self) -> (String, String) {
        (
            value_text(self.increment_unit, i64::from(self.increment)),
            value_text(self.length_unit, i64::from(self.length)),
        )
    }
}

/// What a §4 time-range loop covers ([`StatisticalWindow`]), from the
/// forecast time (§4 octet 18 and octets 19-22) and the loop's time ranges,
/// outermost first. `None` for a loop one window does not describe, which
/// [`product_forecast_display`] shows raw: see there for which loops those
/// are.
#[must_use]
pub fn statistical_window(
    forecast_time_unit: u8,
    forecast_time: i64,
    specs: &[TimeRangeSpec],
) -> Option<StatisticalWindow> {
    match specs {
        [spec] => match spec.time_increment_type {
            2 => time_window(forecast_time_unit, forecast_time, spec)
                .map(StatisticalWindow::WithinRun),
            // The lead is written by its magnitude, which `i64::MIN` (only a
            // struct literal holds it) does not have.
            1 => {
                forecast_time.checked_abs()?;
                run_series(forecast_time_unit, forecast_time, spec).map(StatisticalWindow::OverRuns)
            }
            _ => None,
        },
        [outer, inner] => runs_over_windows(forecast_time_unit, forecast_time, outer, inner),
        _ => None,
    }
}

/// An outer time range of type 1 over an inner one of type 2. `None` for
/// any other pair of types, for an outer range [`run_series`] does not read
/// as a series, and for an inner window [`time_window`] cannot place or that
/// covers no time, as GRIB1 shows a code over several products raw when its
/// period covers none.
fn runs_over_windows(
    unit: u8,
    forecast_time: i64,
    outer: &TimeRangeSpec,
    inner: &TimeRangeSpec,
) -> Option<StatisticalWindow> {
    if (outer.time_increment_type, inner.time_increment_type) != (1, 2) || inner.stat_length == 0 {
        return None;
    }
    let runs = run_series(unit, forecast_time, outer)?;
    let window = time_window(unit, forecast_time, inner)?;
    Some(StatisticalWindow::RunsOverWindows(runs, window))
}

/// The forecast column's text for a window: `0–6h accum`, `average of
/// analyses every 6h over 720h`, `average of 0–6h accums issued every 6h
/// over 1 month`.
///
/// Runs over windows are named in GRIB1's words for NCEP's codes 128 to 131
/// and 137 to 140 (`averages`, `accums`), and any other inner process by its
/// word and `products`, as GRIB1 names ECMWF's products of code 128: `max
/// of 0–24h max products issued every 24h over 720h`.
fn window_text(window: &StatisticalWindow) -> String {
    match window {
        StatisticalWindow::WithinRun(window) => {
            format!("{} {}", span(window), process_word(window.process))
        }
        StatisticalWindow::OverRuns(runs) => over_runs(runs),
        StatisticalWindow::RunsOverWindows(runs, window) => {
            let (every, over) = runs.texts();
            let products = match window.process {
                0 => "averages".to_string(),
                1 => "accums".to_string(),
                other => format!("{} products", process_word(other)),
            };
            format!(
                "{} of {} {products} issued every {every} over {over}",
                process_word(runs.process),
                span(window)
            )
        }
    }
}

/// Whether a statistic's length or a run increment in Table 4.4 `unit` may
/// be read in words: whether the table names it, and it is not the missing
/// one. A unit the table reserves (8, 9 and 14 to 191) or leaves to local
/// use (192 to 254) says no more about what its number counts than the
/// missing one does, so a length or an increment in it is shown raw, as one
/// in the missing unit is (#1632). A calendar unit is named, and reads in
/// words.
///
/// The forecast time is not held to this. A forecast time of 0 is the
/// reference time in any unit that is not missing, as [`seconds_of`] and
/// [`offset_hours`] read it, so a window from 0 or runs at 0 still read in
/// words. A nonzero one in such a unit is a lead, which keeps its number
/// and the unit's code, `+24 time-range unit 99`; a window from it has no
/// end in a unit it shares, so it is shown raw.
fn named_unit(unit: u8) -> bool {
    unit != MISSING_TIME_UNIT && lookup_time_range_unit(unit).is_some()
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

/// The window from the forecast time to the forecast time plus the length,
/// with both ends in one unit: seconds, or a unit with no fixed length the
/// length is in and the start is in too or is 0. `None` when either unit is
/// missing, when the length's unit is reserved or local ([`named_unit`]),
/// when the two ends have no unit in common, as a nonzero start in a
/// reserved or local unit has with any length, and for a start whose
/// magnitude an `i64` does not hold.
fn time_window(unit: u8, forecast_time: i64, spec: &TimeRangeSpec) -> Option<TimeWindow> {
    let (start_unit, start) = (unit, forecast_time);
    let (length_unit, length) = (spec.stat_length_unit, i64::from(spec.stat_length));
    if start_unit == MISSING_TIME_UNIT || !named_unit(length_unit) {
        return None;
    }
    let ends = if let (Some(s), Some(l)) = (
        seconds_of(start_unit, start),
        seconds_of(length_unit, length),
    ) {
        WindowEnds::Seconds {
            start: s,
            end: s.checked_add(l)?,
        }
    } else if start_unit == length_unit || start == 0 {
        // No length in seconds: the length has a calendar unit, or the start
        // is a nonzero value in a calendar, reserved or local unit. The ends
        // still share a unit when both are stated in it, which the length's
        // being named keeps to a calendar one, or when the start is 0.
        WindowEnds::Calendar {
            start,
            end: start.checked_add(length)?,
            unit: length_unit,
        }
    } else {
        return None;
    };
    // A window before the reference time is written by each end's
    // magnitude; the end is past the start, so the start's is the one that
    // can be missing.
    let (WindowEnds::Seconds { start, .. } | WindowEnds::Calendar { start, .. }) = ends;
    start.checked_abs()?;
    Some(TimeWindow {
        process: spec.stat_process,
        ends,
    })
}

/// A window as text: `0–6h`, both ends in one unit, or `0–1 month` in a
/// unit with no fixed length. A window that starts before the reference
/// time reads as GRIB1's codes 6 and 7 do, each end signed and in its own
/// unit, `−6h to −3h`.
fn span(window: &TimeWindow) -> String {
    // `time_window` holds the start's magnitude, and the end, past the
    // start, has one too, so `saturating_abs` is exact here.
    match window.ends {
        WindowEnds::Seconds { start, end } if start < 0 => {
            let end_text = |x: i64| signed(x < 0, duration_text(x.saturating_abs()));
            format!("{} to {}", end_text(start), end_text(end))
        }
        WindowEnds::Seconds { start, end } => span_text(start, end),
        WindowEnds::Calendar { start, end, unit } => {
            let label = unit_label(unit);
            if start < 0 {
                let end_text = |x: i64| signed(x < 0, format!("{} {label}", x.saturating_abs()));
                format!("{} to {}", end_text(start), end_text(end))
            } else {
                format!("{start}–{end} {label}")
            }
        }
    }
}

/// `text` after a true minus sign (U+2212) when `negative`, and a `+`
/// otherwise, as GRIB1 writes the ends of its codes 6 and 7.
fn signed(negative: bool, text: String) -> String {
    format!("{}{text}", if negative { '\u{2212}' } else { '+' })
}

/// A statistic over runs one increment apart, each at the same forecast
/// time (Code Table 4.11 type 1), in GRIB1's words for codes 113 and 123:
/// `average of analyses every 6h over 720h` at a forecast time of 0, and
/// `average of +24h forecasts issued every 24h over 720h` otherwise.
fn over_runs(runs: &RunSeries) -> String {
    let (every, over) = runs.texts();
    let process = process_word(runs.process);
    let lead = runs.lead;
    if lead == 0 {
        format!("{process} of analyses every {every} over {over}")
    } else {
        // `statistical_window` holds the lead's magnitude, so
        // `saturating_abs` is exact here.
        let lead = signed(lead < 0, value_text(runs.lead_unit, lead.saturating_abs()));
        format!("{process} of {lead} forecasts issued every {every} over {over}")
    }
}

/// The runs of a type 1 range. `None` when the increment or the length is
/// zero, since runs no time apart, or over no time, are not a series, when
/// the length or the increment has a unit that is missing, reserved or
/// local ([`named_unit`]), or the forecast time a missing unit, and when
/// both are in units of a fixed length and the increment is at least the
/// length: fewer than two runs, a series the message cannot mean (#1627).
/// An increment equal to the length is such a series. A calendar increment
/// or length has no exact length to compare, so it stays in words. The text
/// follows GRIB1, which shows a statistic whose step is zero raw too
/// (#1554), but the hours do not: see [`offset_hours`].
fn run_series(unit: u8, forecast_time: i64, spec: &TimeRangeSpec) -> Option<RunSeries> {
    // A forecast time in a reserved or local unit is a lead, read as one
    // (see [`named_unit`]); only a missing one leaves the runs unknown.
    if unit == MISSING_TIME_UNIT
        || !named_unit(spec.stat_length_unit)
        || !named_unit(spec.increment_unit)
        || spec.increment == 0
        || spec.stat_length == 0
    {
        return None;
    }
    // Exact, in seconds: a unit with no fixed length gives `None` and skips
    // the comparison.
    let step = seconds_of(spec.increment_unit, i64::from(spec.increment));
    let length = seconds_of(spec.stat_length_unit, i64::from(spec.stat_length));
    if step
        .zip(length)
        .is_some_and(|(step, length)| step >= length)
    {
        return None;
    }
    Some(RunSeries {
        process: spec.stat_process,
        lead_unit: unit,
        lead: forecast_time,
        increment_unit: spec.increment_unit,
        increment: spec.increment,
        length_unit: spec.stat_length_unit,
        length: spec.stat_length,
    })
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
/// or `−6h` before the reference time. It is [`forecast_display`] for every
/// lead but `i64::MIN`, which has no magnitude to sign.
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
        // A negative lead takes the true minus, as the window ends do (#1616).
        assert_eq!(forecast_display(&common(1, -6)), "\u{2212}6h");
        assert_eq!(forecast_display(&common(0, -90)), "\u{2212}90min");
    }

    /// `raw_lead` and `forecast_display` spell a lead the same way, and the one
    /// lead with no magnitude (`i64::MIN`, reachable only from a struct
    /// literal) still gets a single true minus (#1616).
    #[test]
    fn raw_lead_agrees_with_forecast_display_and_min_has_one_sign() {
        for unit in 0..=13u8 {
            for lead in [-100_000, -90, -6, -1, 0, 1, 6, 90, 100_000, i64::MAX] {
                let c = common(unit, lead);
                assert_eq!(
                    raw_lead(&c),
                    forecast_display(&c),
                    "unit {unit} lead {lead}"
                );
            }
            let min = forecast_display(&common(unit, i64::MIN));
            assert!(min.starts_with('\u{2212}'), "unit {unit}: {min}");
            assert!(
                !min.starts_with("\u{2212}-") && !min.contains('+'),
                "unit {unit}: {min}"
            );
        }
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

    fn range(time_increment_type: u8, unit: u8, length: u32, increment: u32) -> TimeRangeSpec {
        TimeRangeSpec {
            stat_process: 1,
            time_increment_type,
            stat_length_unit: unit,
            stat_length: length,
            increment_unit: unit,
            increment,
        }
    }

    /// The structured window carries what a sidecar's forecast field is
    /// compared with (#1624): a window within a run in seconds, unless its
    /// length is in a calendar unit, and a series of runs at its forecast
    /// time, to the forecast time plus the length.
    #[test]
    fn the_window_says_where_it_starts_and_ends() {
        let Some(StatisticalWindow::WithinRun(window)) =
            statistical_window(1, 12, &[range(2, 1, 24, 0)])
        else {
            panic!("a window within a run");
        };
        assert_eq!(window.seconds(), Some((12 * 3_600, 36 * 3_600)));
        assert_eq!(window.process(), 1);
        // A calendar window has no length in seconds.
        let Some(StatisticalWindow::WithinRun(month)) =
            statistical_window(3, 0, &[range(2, 3, 1, 0)])
        else {
            panic!("a calendar window");
        };
        assert_eq!(month.seconds(), None);

        let Some(StatisticalWindow::OverRuns(runs)) =
            statistical_window(1, 24, &[range(1, 1, 720, 24)])
        else {
            panic!("a series of runs");
        };
        assert_eq!(runs.start_seconds(), Some(24 * 3_600));
        assert_eq!(runs.end_seconds(), Some(744 * 3_600));
        assert_eq!(runs.process(), 1);
        // Over a calendar month the runs still have a forecast time.
        let Some(StatisticalWindow::OverRuns(monthly)) = statistical_window(
            1,
            0,
            &[TimeRangeSpec {
                stat_length_unit: 3,
                stat_length: 1,
                ..range(1, 1, 720, 6)
            }],
        ) else {
            panic!("a monthly series of runs");
        };
        assert_eq!(
            (monthly.start_seconds(), monthly.end_seconds()),
            (Some(0), None)
        );

        let Some(StatisticalWindow::RunsOverWindows(outer, inner)) =
            statistical_window(1, 0, &[range(1, 1, 720, 6), range(2, 1, 6, 0)])
        else {
            panic!("runs over windows");
        };
        assert_eq!(outer.end_seconds(), Some(720 * 3_600));
        assert_eq!(inner.seconds(), Some((0, 6 * 3_600)));
    }

    /// `None` is exactly the loops the forecast column shows raw.
    #[test]
    fn a_loop_no_window_describes_has_none() {
        for (what, specs) in [
            ("no time range", vec![]),
            ("type 3", vec![range(3, 1, 6, 0)]),
            ("a missing type", vec![range(255, 1, 6, 0)]),
            ("runs no time apart", vec![range(1, 1, 720, 0)]),
            ("runs that cannot hold two", vec![range(1, 1, 24, 48)]),
            (
                "two windows in one run",
                vec![range(2, 1, 720, 6), range(2, 1, 6, 0)],
            ),
            (
                "an inner window of no length",
                vec![range(1, 1, 720, 6), range(2, 1, 0, 0)],
            ),
            ("three time ranges", vec![range(2, 1, 6, 0); 3]),
        ] {
            assert_eq!(statistical_window(1, 0, &specs), None, "{what}");
        }
        // A missing forecast-time unit, and a lead whose magnitude an `i64`
        // does not hold, over runs and in a window.
        assert_eq!(statistical_window(255, 0, &[range(2, 1, 6, 0)]), None);
        assert_eq!(
            statistical_window(1, i64::MIN, &[range(1, 1, 720, 6)]),
            None
        );
        assert_eq!(
            statistical_window(13, i64::MIN, &[range(2, 13, 6, 0)]),
            None
        );
    }
}
