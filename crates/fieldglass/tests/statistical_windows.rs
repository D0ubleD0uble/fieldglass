//! A GRIB2 statistic shows its window, not where its window starts (#1566).
//!
//! Every product template with a §4 time-range loop is read here through
//! [`Session::message`], the path every host takes, from a committed message
//! in that template: 4.8, 4.9, 4.10, 4.11, 4.12 and the chemistry and aerosol
//! templates 4.42, 4.43, 4.46 and 4.47. Before #1566 each read its forecast
//! time alone, so ECMWF's 0–6 h runoff read `+0h`.
//!
//! The loops the committed messages do not state (another type of time
//! increment, a missing unit, a month, two time ranges, none) restamp the
//! `mx2t3` message's loop, so they too go through the real reader. One
//! restamp, a monthly mean of 6-hour accumulations over successive runs, is
//! committed as its own fixture so eccodes can read it too (#1622).

use fieldglass::{MessageInfo, Session};

const MX2T3: &[u8] = include_bytes!("fixtures/ecmwf_ifs_0p25_mx2t3.grib2");
const MONTHLY_MEAN_OF_6H_ACCUMS: &[u8] = include_bytes!("fixtures/monthly_mean_of_6h_accums.grib2");

fn message(bytes: Vec<u8>) -> MessageInfo {
    Session::open(bytes)
        .expect("opens")
        .message(0)
        .expect("message 0")
}

/// One committed message per template with a time-range loop, and what its
/// forecast column and hours read. The windows are eccodes 2.34.1's
/// `stepRange` for each (`grib_ls -p productDefinitionTemplateNumber,
/// stepRange,typeOfStatisticalProcessing`), and the process its
/// `typeOfStatisticalProcessing`.
#[test]
fn every_template_with_a_time_range_loop_shows_its_window() {
    macro_rules! fixture {
        ($path:literal) => {
            include_bytes!($path).as_slice()
        };
    }
    // (template, fixture, eccodes stepRange, forecast, hours)
    let cases: [(&str, &[u8], &str, &str, i32); 14] = [
        // 2 m maximum and minimum temperature over the first 3 hours.
        ("4.8", MX2T3, "0-3", "0–3h max", 0),
        (
            "4.8",
            fixture!("fixtures/ecmwf_ifs_0p25_mn2t3.grib2"),
            "0-3",
            "0–3h min",
            0,
        ),
        // Surface thermal radiation accumulated over no time at all, the
        // analysis step of an accumulation: a window of no length.
        (
            "4.8",
            fixture!("fixtures/ecmwf_ifs_0p25_str.grib2"),
            "0",
            "0–0h accum",
            0,
        ),
        // NBM's hourly precipitation, sampled continuously (an increment of 0).
        (
            "4.8",
            fixture!("fixtures/nbm_core_co_apcp.grib2"),
            "0-1",
            "0–1h accum",
            0,
        ),
        // A probability of precipitation over 12 to 36 hours: a window that
        // does not start at the reference time, so the hours are its start.
        (
            "4.9",
            fixture!("fixtures/ecmwf_ifs_0p25_tpg100.grib2"),
            "12-36",
            "12–36h accum",
            12,
        ),
        (
            "4.9",
            fixture!("fixtures/nbm_core_co_apcp_prob.grib2"),
            "0-1",
            "0–1h accum",
            0,
        ),
        (
            "4.10",
            fixture!("../../fieldglass-grib2/tests/fixtures/percentile_min_4_10.grib2"),
            "6-12",
            "6–12h min",
            6,
        ),
        // The issue's own case: runoff over 0 to 6 h, sampled every 450 s (the
        // model's time step), which does not move either end of the window.
        (
            "4.11",
            fixture!("fixtures/ecmwf_ifs_0p25_cf_ro.grib2"),
            "0-6",
            "0–6h accum",
            0,
        ),
        (
            "4.11",
            fixture!("fixtures/ecmwf_ifs_0p25_pf_ro_48.grib2"),
            "0-6",
            "0–6h accum",
            0,
        ),
        (
            "4.12",
            fixture!("../../fieldglass-grib2/tests/fixtures/derived_max_4_12.grib2"),
            "0-6",
            "0–6h max",
            0,
        ),
        (
            "4.42",
            fixture!("../../fieldglass-grib2/tests/fixtures/constituent_4_42.grib2"),
            "0-6",
            "0–6h accum",
            0,
        ),
        (
            "4.43",
            fixture!("../../fieldglass-grib2/tests/fixtures/constituent_4_43.grib2"),
            "0-6",
            "0–6h accum",
            0,
        ),
        (
            "4.46",
            fixture!("../../fieldglass-grib2/tests/fixtures/constituent_4_46.grib2"),
            "0-6",
            "0–6h accum",
            0,
        ),
        (
            "4.47",
            fixture!("../../fieldglass-grib2/tests/fixtures/constituent_4_47.grib2"),
            "0-6",
            "0–6h accum",
            0,
        ),
    ];
    for (template, bytes, step_range, forecast, hours) in cases {
        let info = message(bytes.to_vec());
        let what = format!("template {template}, eccodes stepRange {step_range}");
        assert_eq!(info.forecast.as_deref(), Some(forecast), "{what}");
        assert_eq!(info.offset_hours, Some(hours), "{what}");
    }
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

/// One §4 time range as Template 4.8 octets 47-58 lay it out.
#[derive(Clone, Copy)]
struct Range {
    process: u8,
    increment_type: u8,
    length_unit: u8,
    length: u32,
    increment_unit: u8,
    increment: u32,
}

impl Range {
    fn octets(self) -> [u8; 12] {
        let mut out = [0; 12];
        out[0] = self.process;
        out[1] = self.increment_type;
        out[2] = self.length_unit;
        out[3..7].copy_from_slice(&self.length.to_be_bytes());
        out[7] = self.increment_unit;
        out[8..12].copy_from_slice(&self.increment.to_be_bytes());
        out
    }
}

/// `mx2t3`'s own time range: a maximum over 3 hours, sampled every 450 s.
const MX2T3_RANGE: Range = Range {
    process: 2,
    increment_type: 2,
    length_unit: 1,
    length: 3,
    increment_unit: 13,
    increment: 450,
};

/// The `mx2t3` message with its forecast time (§4 octets 18-22) and its loop
/// (octet 42 and the time ranges from octet 47) replaced, and the §4 and §0
/// lengths moved to fit, read through the session.
fn restamped(unit: u8, forecast_time: u32, ranges: &[Range]) -> MessageInfo {
    message(restamped_bytes(unit, forecast_time, ranges))
}

/// The bytes [`restamped`] reads.
fn restamped_bytes(unit: u8, forecast_time: u32, ranges: &[Range]) -> Vec<u8> {
    let mut bytes = MX2T3.to_vec();
    let s4 = section_start(&bytes, 4);
    assert_eq!(
        (bytes[s4 + 7], bytes[s4 + 8], bytes[s4 + 41]),
        (0, 8, 1),
        "fixture precondition: template 4.8 with one time range"
    );
    bytes[s4 + 17] = unit;
    bytes[s4 + 18..s4 + 22].copy_from_slice(&forecast_time.to_be_bytes());
    bytes[s4 + 41] = u8::try_from(ranges.len()).unwrap();
    let loop_octets: Vec<u8> = ranges.iter().flat_map(|r| r.octets()).collect();
    bytes.splice(s4 + 46..s4 + 58, loop_octets);
    let grown = |old: u64| old + 12 * ranges.len() as u64 - 12;
    let s4_len = u64::from(u32::from_be_bytes(bytes[s4..s4 + 4].try_into().unwrap()));
    let s4_len = u32::try_from(grown(s4_len)).unwrap();
    bytes[s4..s4 + 4].copy_from_slice(&s4_len.to_be_bytes());
    let total = grown(u64::from_be_bytes(bytes[8..16].try_into().unwrap()));
    bytes[8..16].copy_from_slice(&total.to_be_bytes());
    bytes
}

/// The restamp reproduces the committed message when it changes nothing, so
/// the cases below differ from it only where they say.
#[test]
fn restamping_the_loop_as_it_is_changes_nothing() {
    let same = restamped(1, 0, &[MX2T3_RANGE]);
    assert_eq!(same, message(MX2T3.to_vec()));
}

/// Successive fields from runs one increment apart, at one forecast time
/// (Code Table 4.11 type 1): a statistic over runs, in GRIB1's words for
/// codes 123 and 113. The hours are the forecast time, so an average of
/// analyses is 0 as GRIB1's 123 is.
#[test]
fn a_statistic_over_runs_says_how_far_apart_they_are() {
    let monthly = Range {
        process: 0,
        increment_type: 1,
        length_unit: 2,
        length: 30,
        increment_unit: 1,
        increment: 6,
    };
    let analyses = restamped(1, 0, &[monthly]);
    assert_eq!(
        analyses.forecast.as_deref(),
        Some("average of analyses every 6h over 720h")
    );
    assert_eq!(analyses.offset_hours, Some(0));

    let daily = Range {
        increment: 24,
        ..monthly
    };
    let forecasts = restamped(1, 24, &[daily]);
    assert_eq!(
        forecasts.forecast.as_deref(),
        Some("average of +24h forecasts issued every 24h over 720h")
    );
    assert_eq!(forecasts.offset_hours, Some(24));

    // Runs no time apart are not a series of runs: the text is raw, as
    // GRIB1's is for a statistic whose step is zero (#1554). The hours are
    // not GRIB1's: they stay the forecast time (see `lead_time`).
    let no_step = restamped(
        1,
        0,
        &[Range {
            increment: 0,
            ..monthly
        }],
    );
    assert_eq!(
        no_step.forecast.as_deref(),
        Some("+0h, statistics: average over 720h, increment 0h of type 1 (successive runs)")
    );
    assert_eq!(no_step.offset_hours, Some(0));
    // Nor are runs over no time.
    let no_span = restamped(
        1,
        0,
        &[Range {
            length: 0,
            ..monthly
        }],
    );
    assert_eq!(
        no_span.forecast.as_deref(),
        Some("+0h, statistics: average over 0h, increment 6h of type 1 (successive runs)")
    );
    // Runs whose increment is in the missing unit: how far apart they are is
    // not stated, so they are not described as a series.
    let increment_missing = restamped(
        1,
        0,
        &[Range {
            increment_unit: 255,
            ..monthly
        }],
    );
    assert_eq!(
        increment_missing.forecast.as_deref(),
        Some("+0h, statistics: average over 720h, increment 6 Missing of type 1 (successive runs)")
    );
    // Nor are runs at a forecast time in the missing unit: which forecasts
    // they are is not stated.
    let lead_missing = restamped(255, 0, &[monthly]);
    assert_eq!(
        lead_missing.forecast.as_deref(),
        Some("+0 Missing, statistics: average over 720h, increment 6h of type 1 (successive runs)")
    );
    assert_eq!(lead_missing.offset_hours, None);
    // Nor are runs over a length in the missing unit.
    let length_missing = restamped(
        1,
        0,
        &[Range {
            length_unit: 255,
            ..monthly
        }],
    );
    assert_eq!(
        length_missing.forecast.as_deref(),
        Some("+0h, statistics: average over 30 Missing, increment 6h of type 1 (successive runs)")
    );
}

/// A series of runs that cannot hold two runs is not one (#1627): when the
/// increment and the length are both in units of a fixed length and the
/// increment is at least the length, the loop is shown raw, as one with no
/// step already is. The units are compared exactly, in seconds, so an
/// increment in days and a length in hours compare. A calendar increment or
/// length has no exact length, so it stays in words. The hours stay the
/// forecast time.
#[test]
fn a_series_that_cannot_hold_two_runs_is_shown_raw() {
    let series = Range {
        process: 0,
        increment_type: 1,
        length_unit: 1,
        length: 24,
        increment_unit: 1,
        increment: 48,
    };
    let shown = |ranges: &[Range]| restamped(1, 0, ranges).forecast.expect("a forecast");
    // One time range, an increment longer than the length, then equal to it.
    let longer = restamped(1, 0, &[series]);
    assert_eq!(
        longer.forecast.as_deref(),
        Some("+0h, statistics: average over 24h, increment 48h of type 1 (successive runs)")
    );
    assert_eq!(longer.offset_hours, Some(0));
    let equal = restamped(
        1,
        24,
        &[Range {
            increment: 24,
            ..series
        }],
    );
    assert_eq!(
        equal.forecast.as_deref(),
        Some("+24h, statistics: average over 24h, increment 24h of type 1 (successive runs)")
    );
    assert_eq!(equal.offset_hours, Some(24));
    // One hour less than the length holds two runs: still words.
    assert_eq!(
        shown(&[Range {
            increment: 23,
            ..series
        }]),
        "average of analyses every 23h over 24h"
    );
    // The units differ: 1 day against 24 hours is equal, against 25 is not.
    let days = Range {
        increment_unit: 2,
        increment: 1,
        ..series
    };
    assert_eq!(
        shown(&[days]),
        "+0h, statistics: average over 24h, increment 24h of type 1 (successive runs)"
    );
    assert_eq!(
        shown(&[Range { length: 25, ..days }]),
        "average of analyses every 24h over 25h"
    );

    // Two time ranges, an outer type 1 over an inner type 2.
    let inner = Range {
        increment_type: 2,
        ..MONTHLY_INNER
    };
    let raw_pair = "+0h, statistics: average over 24h, increment {}h of type 1 (successive \
                    runs); accum over 6h, increment 0h of type 2 (same run)";
    let outer_longer = restamped(1, 0, &[series, inner]);
    assert_eq!(
        outer_longer.forecast.as_deref(),
        Some(raw_pair.replace("{}", "48").as_str())
    );
    assert_eq!(outer_longer.offset_hours, Some(0));
    assert_eq!(
        shown(&[
            Range {
                increment: 24,
                ..series
            },
            inner
        ]),
        raw_pair.replace("{}", "24")
    );
    assert_eq!(
        shown(&[
            Range {
                increment: 12,
                ..series
            },
            inner
        ]),
        "average of 0\u{2013}6h accums issued every 12h over 24h"
    );

    // A calendar increment or length is not compared: it stays in words.
    let month_every = Range {
        increment_unit: 3,
        increment: 1,
        length: 6,
        ..series
    };
    assert_eq!(
        shown(&[month_every]),
        "average of analyses every 1 month over 6h"
    );
    assert_eq!(
        shown(&[month_every, inner]),
        "average of 0\u{2013}6h accums issued every 1 month over 6h"
    );
    let month_over_month = Range {
        length_unit: 3,
        length: 1,
        ..month_every
    };
    assert_eq!(
        shown(&[month_over_month]),
        "average of analyses every 1 month over 1 month"
    );
    assert_eq!(
        shown(&[Range {
            increment_unit: 4,
            ..month_over_month
        }]),
        "average of analyses every 1 year over 1 month"
    );
    assert_eq!(
        shown(&[Range {
            length_unit: 3,
            length: 1,
            ..series
        }]),
        "average of analyses every 48h over 1 month"
    );
}

/// A loop no single window describes is shown with every time range as the
/// message states it. The hours stay the forecast time, which §4 Note 2 of
/// Template 4.8 makes the start of the overall interval whatever the loop
/// says.
#[test]
fn a_loop_one_window_does_not_describe_is_shown_raw() {
    // Two time ranges in the order no window describes: the accumulations
    // outside, the runs inside (Template 4.8 Note 4 puts the outer range
    // first). Every two-range shape but runs over windows is raw; see
    // `two_time_ranges_other_than_runs_over_windows_are_shown_raw`.
    let two = restamped(1, 0, &[MONTHLY_INNER, MONTHLY_OUTER]);
    assert_eq!(
        two.forecast.as_deref(),
        Some(
            "+0h, statistics: accum over 6h, increment 0h of type 2 (same run); \
             average over 1 month, increment 6h of type 1 (successive runs)"
        )
    );
    assert_eq!(two.offset_hours, Some(0));

    // Successive fields valid at one time (types 3 and 4), a floating
    // subinterval (5), and a type the message leaves missing (255) or Code
    // Table 4.11 reserves (0): none of them is a window from the forecast
    // time.
    for (increment_type, shown) in [
        (0u8, "0"),
        (3, "3 (runs forward, valid time fixed)"),
        (4, "4 (runs back, valid time fixed)"),
        (5, "5 (floating subinterval)"),
        (255, "255 (missing)"),
        (200, "200"),
    ] {
        let info = restamped(
            1,
            6,
            &[Range {
                increment_type,
                ..MX2T3_RANGE
            }],
        );
        assert_eq!(
            info.forecast.as_deref(),
            Some(format!("+6h, statistics: max over 3h, increment 450s of type {shown}").as_str()),
            "type {increment_type}"
        );
        assert_eq!(info.offset_hours, Some(6), "type {increment_type}");
    }

    // No time range at all.
    let none = restamped(1, 6, &[]);
    assert_eq!(
        none.forecast.as_deref(),
        Some("+6h, no statistical time range")
    );
    assert_eq!(none.offset_hours, Some(6));
}

/// A missing time unit says nothing about what its number counts (#1595),
/// so a window with one at either end is shown raw. A forecast time in the
/// missing unit has no hours, as any lead in it has none.
#[test]
fn a_missing_time_unit_is_never_read_as_a_window() {
    let length_missing = restamped(
        1,
        0,
        &[Range {
            length_unit: 255,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(
        length_missing.forecast.as_deref(),
        Some("+0h, statistics: max over 3 Missing, increment 450s of type 2 (same run)")
    );
    assert_eq!(length_missing.offset_hours, Some(0));

    let start_missing = restamped(255, 0, &[MX2T3_RANGE]);
    assert_eq!(
        start_missing.forecast.as_deref(),
        Some("+0 Missing, statistics: max over 3h, increment 450s of type 2 (same run)")
    );
    assert_eq!(start_missing.offset_hours, None);

    // An increment in the missing unit does not move the window, so a window
    // still reads as one.
    let increment_missing = restamped(
        1,
        0,
        &[Range {
            increment_unit: 255,
            increment: 0,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(increment_missing.forecast.as_deref(), Some("0–3h max"));
}

/// A window in a unit with no fixed length reads in that unit when both ends
/// can be stated in it, and raw when they cannot.
#[test]
fn a_calendar_window_keeps_its_unit() {
    let month = Range {
        process: 0,
        length_unit: 3,
        length: 1,
        ..MX2T3_RANGE
    };
    // A start of 0 is the reference time in any unit.
    assert_eq!(
        restamped(1, 0, &[month]).forecast.as_deref(),
        Some("0–1 month average")
    );
    // A start of 0 months is the reference time too, so a 3-hour length
    // reads in hours.
    assert_eq!(
        restamped(3, 0, &[MX2T3_RANGE]).forecast.as_deref(),
        Some("0–3h max")
    );
    // Both ends in months.
    let info = restamped(3, 2, &[month]);
    assert_eq!(info.forecast.as_deref(), Some("2–3 month average"));
    assert_eq!(info.offset_hours, None, "a month has no hours");
    // 12 hours, then a month: no unit states both ends.
    let info = restamped(1, 12, &[month]);
    assert_eq!(
        info.forecast.as_deref(),
        Some("+12h, statistics: average over 1 month, increment 450s of type 2 (same run)")
    );
    assert_eq!(info.offset_hours, Some(12));
    // A window in days reads in hours, as a lead does, and one that is not a
    // whole number of hours in minutes, both ends in one unit.
    let days = Range {
        length_unit: 2,
        length: 1,
        ..MX2T3_RANGE
    };
    assert_eq!(
        restamped(1, 24, &[days]).forecast.as_deref(),
        Some("24–48h max")
    );
    let minutes = Range {
        length_unit: 0,
        length: 15,
        ..MX2T3_RANGE
    };
    assert_eq!(
        restamped(1, 1, &[minutes]).forecast.as_deref(),
        Some("60–75min max")
    );
}

/// A forecast time of `-v` as §4 writes it: sign-magnitude, octet 19's top
/// bit set.
const fn negative(v: u32) -> u32 {
    0x8000_0000 | v
}

/// A plain lead (Template 4.0) before the reference time is signed with the
/// same true minus as the ends of a window, not `+-6h` (#1616).
#[test]
fn a_negative_plain_lead_is_signed_as_a_window_end_is() {
    const PLAIN: &[u8] =
        include_bytes!("../../fieldglass-grib2/tests/fixtures/complex_regular_latlon.grib2");
    let lead = |unit: u8, forecast_time: u32| {
        let mut bytes = PLAIN.to_vec();
        let s4 = section_start(&bytes, 4);
        assert_eq!((bytes[s4 + 7], bytes[s4 + 8]), (0, 0), "template 4.0");
        bytes[s4 + 17] = unit;
        bytes[s4 + 18..s4 + 22].copy_from_slice(&forecast_time.to_be_bytes());
        message(bytes)
    };
    let hours = lead(1, negative(6));
    assert_eq!(hours.forecast.as_deref(), Some("\u{2212}6h"));
    assert_eq!(hours.offset_hours, Some(-6));
    assert_eq!(
        lead(0, negative(90)).forecast.as_deref(),
        Some("\u{2212}90min")
    );
    assert_eq!(
        lead(3, negative(2)).forecast.as_deref(),
        Some("\u{2212}2 month")
    );
    // Negative zero and positive leads keep their `+`.
    assert_eq!(lead(1, negative(0)).forecast.as_deref(), Some("+0h"));
    assert_eq!(lead(1, 6).forecast.as_deref(), Some("+6h"));
}

/// §4 states the forecast time in sign-magnitude, so a window can start
/// before the reference time. It reads as GRIB1's codes 6 and 7 read, each
/// end signed with a true minus (`−6h to −3h average` in GRIB1).
#[test]
fn a_window_before_the_reference_time_reads_as_grib1s() {
    let info = restamped(1, negative(6), &[MX2T3_RANGE]);
    assert_eq!(
        info.forecast.as_deref(),
        Some("\u{2212}6h to \u{2212}3h max")
    );
    assert_eq!(info.offset_hours, Some(-6));
    // One that runs past the reference time, as GRIB1's code 7 does.
    let across = restamped(
        1,
        negative(1),
        &[Range {
            length: 3,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(across.forecast.as_deref(), Some("\u{2212}1h to +2h max"));
    // One that ends at the reference time: its end is `+0h`, not `−0h`.
    let to_zero = restamped(1, negative(3), &[MX2T3_RANGE]);
    assert_eq!(to_zero.forecast.as_deref(), Some("\u{2212}3h to +0h max"));
    // The same in a calendar unit.
    let to_zero_months = restamped(
        3,
        negative(1),
        &[Range {
            length_unit: 3,
            length: 1,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(
        to_zero_months.forecast.as_deref(),
        Some("\u{2212}1 month to +0 month max")
    );
    // Each end in its own unit, as GRIB1 writes them.
    let minutes = restamped(
        0,
        negative(90),
        &[Range {
            length_unit: 1,
            length: 1,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(
        minutes.forecast.as_deref(),
        Some("\u{2212}90min to \u{2212}30min max")
    );
    // Both ends in months.
    let months = restamped(
        3,
        negative(2),
        &[Range {
            length_unit: 3,
            length: 1,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(
        months.forecast.as_deref(),
        Some("\u{2212}2 month to \u{2212}1 month max")
    );
    // A statistic over runs at a negative forecast time.
    let runs = restamped(
        1,
        negative(6),
        &[Range {
            process: 0,
            increment_type: 1,
            length_unit: 2,
            length: 30,
            increment_unit: 1,
            increment: 24,
        }],
    );
    assert_eq!(
        runs.forecast.as_deref(),
        Some("average of \u{2212}6h forecasts issued every 24h over 720h")
    );
    assert_eq!(runs.offset_hours, Some(-6));
}

/// A loop shown raw before the reference time signs its forecast time as a
/// window does, never `+-6h`.
#[test]
fn a_raw_loop_before_the_reference_time_keeps_its_sign() {
    let month = Range {
        process: 0,
        length_unit: 3,
        length: 1,
        ..MX2T3_RANGE
    };
    // Hours before a calendar length: no shared unit, so raw.
    assert_eq!(
        restamped(1, negative(6), &[month]).forecast.as_deref(),
        Some("\u{2212}6h, statistics: average over 1 month, increment 450s of type 2 (same run)")
    );
    // Two ranges.
    assert_eq!(
        restamped(1, negative(6), &[MX2T3_RANGE, MX2T3_RANGE])
            .forecast
            .as_deref(),
        Some(
            "\u{2212}6h, statistics: max over 3h, increment 450s of type 2 (same run); \
             max over 3h, increment 450s of type 2 (same run)"
        )
    );
    // A calendar start before an hour length.
    let hours = Range {
        length: 6,
        ..MX2T3_RANGE
    };
    assert_eq!(
        restamped(3, negative(1), &[hours]).forecast.as_deref(),
        Some("\u{2212}1 month, statistics: max over 6h, increment 450s of type 2 (same run)")
    );
}

/// A loop can state 255 time ranges. The raw form lists two and counts the
/// rest, so one message cannot fill a message list's line with thousands of
/// characters.
#[test]
fn a_long_loop_is_listed_in_part() {
    let all = vec![MX2T3_RANGE; 255];
    let info = restamped(1, 0, &all);
    let text = info.forecast.expect("a forecast");
    assert_eq!(
        text,
        "+0h, statistics: max over 3h, increment 450s of type 2 (same run); \
         max over 3h, increment 450s of type 2 (same run); and 253 more"
    );
    assert!(text.chars().count() < 200, "{} characters", text.len());
    // Three ranges count one.
    let three = restamped(1, 0, &[MX2T3_RANGE; 3]);
    assert!(
        three.forecast.as_deref().unwrap().ends_with("; and 1 more"),
        "{:?}",
        three.forecast
    );
}

/// Every Code Table 4.10 process the table names has a word, and a code it
/// does not name keeps its number.
#[test]
fn each_statistical_process_is_named() {
    for (process, word) in [
        (0u8, "average"),
        (1, "accum"),
        (2, "max"),
        (3, "min"),
        (4, "diff"),
        (5, "root mean square"),
        (6, "std dev"),
        (7, "covariance"),
        (8, "diff (start minus end)"),
        (9, "ratio"),
        (10, "standardized anomaly"),
        (11, "sum"),
        (12, "return period"),
        (13, "median"),
        (100, "severity"),
        (101, "mode"),
        (102, "index processing"),
        (14, "statistical process 14"),
        (200, "statistical process 200"),
        (255, "statistical process missing"),
    ] {
        let info = restamped(
            1,
            0,
            &[Range {
                process,
                ..MX2T3_RANGE
            }],
        );
        assert_eq!(
            info.forecast.as_deref(),
            Some(format!("0–3h {word}").as_str()),
            "process {process}"
        );
    }
}

/// GRIB1's accumulation, average and difference from P1 to P2 and GRIB2's of
/// the same window read the same, word for word.
#[test]
fn a_window_reads_the_same_in_both_editions() {
    const GRIB1: &[u8] =
        include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");
    // (GRIB1 Code Table 5 time range, GRIB2 Code Table 4.10 process)
    for (time_range, process) in [(3u8, 0u8), (4, 1), (5, 4)] {
        let mut grib1 = GRIB1.to_vec();
        // PDS octets 18-21: unit (hour), P1, P2, time range. §0 is 8 octets.
        grib1[8 + 18 - 1..8 + 22 - 1].copy_from_slice(&[1, 0, 6, time_range]);
        let grib1 = message(grib1);
        let grib2 = restamped(
            1,
            0,
            &[Range {
                process,
                length: 6,
                ..MX2T3_RANGE
            }],
        );
        assert_eq!(grib1.forecast, grib2.forecast, "time range {time_range}");
        assert_eq!(
            grib1.offset_hours, grib2.offset_hours,
            "time range {time_range}"
        );
    }

    // The same accumulation in the missing time unit (255) reads as a
    // window in neither edition: neither says what its numbers count, so
    // each shows its own raw form and has no hours (#1567). GRIB1 showed
    // `0–6 Missing accum` before.
    let mut grib1 = GRIB1.to_vec();
    grib1[8 + 18 - 1..8 + 22 - 1].copy_from_slice(&[255, 0, 6, 4]);
    let grib1 = message(grib1);
    let grib2 = restamped(
        255,
        0,
        &[Range {
            process: 1,
            length_unit: 255,
            length: 6,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(
        grib1.forecast.as_deref(),
        Some("time range 4, P1 0 Missing, P2 6 Missing")
    );
    assert_eq!(
        grib2.forecast.as_deref(),
        Some("+0 Missing, statistics: accum over 6 Missing, increment 450s of type 2 (same run)")
    );
    assert_eq!(grib1.offset_hours, None);
    assert_eq!(grib2.offset_hours, None);

    // An average of analyses whose step between them is in the missing unit
    // says nothing about how far apart they are, so neither edition reads it
    // as a series (#1567). GRIB1 showed `average of analyses every 6
    // Missing` before. Both still start at the reference time: the step does
    // not move the start.
    let mut grib1 = GRIB1.to_vec();
    grib1[8 + 18 - 1..8 + 22 - 1].copy_from_slice(&[255, 0, 6, 123]);
    let grib1 = message(grib1);
    let grib2 = restamped(
        1,
        0,
        &[Range {
            process: 0,
            increment_type: 1,
            length_unit: 2,
            length: 30,
            increment_unit: 255,
            increment: 6,
        }],
    );
    assert_eq!(
        grib1.forecast.as_deref(),
        Some("time range 123, P1 0 Missing, P2 6 Missing")
    );
    assert_eq!(
        grib2.forecast.as_deref(),
        Some("+0h, statistics: average over 720h, increment 6 Missing of type 1 (successive runs)")
    );
    assert_eq!(grib1.offset_hours, Some(0));
    assert_eq!(grib2.offset_hours, Some(0));
}

/// A time unit its table reserves or leaves to local use says no more about
/// what its numbers count than the missing one does, so a period, a length
/// or a step between runs stated in one reads raw in both editions, as in
/// the missing unit (#1632). GRIB1's ON388 Table 4 reserves 8, 9 and 15 to
/// 253. GRIB2's Code Table 4.4 reserves 8, 9 and 14 to 191 and leaves 192
/// to 254 to local use. A GRIB2 forecast time in one is a lead, as a plain
/// one is, and a window or runs from a forecast time of 0 in one still read
/// in words, since a forecast time of 0 is the reference time in any unit
/// that is not missing.
///
/// The hours are what they were in both: a start of 0 is 0 hours in any unit
/// that is not missing, and any other value in a unit with no fixed length
/// has none.
#[test]
fn a_reserved_time_unit_reads_as_the_missing_one_does() {
    const GRIB1: &[u8] =
        include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");
    let grib1 = |unit: u8, p1: u8, p2: u8, time_range: u8| {
        let mut bytes = GRIB1.to_vec();
        // PDS octets 18-21: unit, P1, P2, time range. §0 is 8 octets.
        bytes[8 + 18 - 1..8 + 22 - 1].copy_from_slice(&[unit, p1, p2, time_range]);
        message(bytes)
    };
    for unit in [8u8, 9, 20, 253] {
        let raw = |p1: u8, p2: u8, code: u8| {
            format!("time range {code}, P1 {p1} time unit {unit}, P2 {p2} time unit {unit}")
        };
        // An accumulation from 0 to 6: before, `0–6 time unit 20 accum`.
        let m = grib1(unit, 0, 6, 4);
        assert_eq!(
            m.forecast.as_deref(),
            Some(raw(0, 6, 4).as_str()),
            "unit {unit}"
        );
        assert_eq!((m.forecast_hours, m.offset_hours), (None, Some(0)));
        // An average from 6 to 12, which has no hours.
        let m = grib1(unit, 6, 12, 3);
        assert_eq!(
            m.forecast.as_deref(),
            Some(raw(6, 12, 3).as_str()),
            "unit {unit}"
        );
        assert_eq!((m.forecast_hours, m.offset_hours), (None, None));
        // An average of analyses 6 units apart: before, `average of
        // analyses every 6 time unit 20`. It starts at the reference time.
        let m = grib1(unit, 0, 6, 123);
        assert_eq!(
            m.forecast.as_deref(),
            Some(raw(0, 6, 123).as_str()),
            "unit {unit}"
        );
        assert_eq!(m.offset_hours, Some(0));
        // A lead is not a period, and reads as one in the missing unit does.
        let m = grib1(unit, 6, 0, 0);
        assert_eq!(
            m.forecast.as_deref(),
            Some(format!("+6 time unit {unit}").as_str())
        );
        assert_eq!((m.forecast_hours, m.offset_hours), (None, None));
    }

    let monthly = Range {
        process: 0,
        increment_type: 1,
        length_unit: 2,
        length: 30,
        increment_unit: 1,
        increment: 6,
    };
    for unit in [8u8, 9, 14, 99, 191, 192, 200, 254] {
        let label = format!("time-range unit {unit}");
        let at = format!("unit {unit}");
        // A window from a forecast time of 0 in the unit still reads: 0 is
        // the reference time in any unit that is not missing.
        let m = restamped(unit, 0, &[MX2T3_RANGE]);
        assert_eq!(m.forecast.as_deref(), Some("0–3h max"), "{at}");
        assert_eq!((m.forecast_hours, m.offset_hours), (None, Some(0)), "{at}");
        // From a nonzero forecast time in the unit, the window has no end in
        // a unit both share, so it is raw: before, too.
        let m = restamped(unit, 6, &[MX2T3_RANGE]);
        assert_eq!(
            m.forecast.as_deref(),
            Some(
                format!("+6 {label}, statistics: max over 3h, increment 450s of type 2 (same run)")
                    .as_str()
            ),
            "{at}"
        );
        assert_eq!((m.forecast_hours, m.offset_hours), (None, None), "{at}");
        // A window whose length is in the unit: before, `0–6 time-range unit
        // 99 accum`.
        let length = Range {
            process: 1,
            length_unit: unit,
            length: 6,
            ..MX2T3_RANGE
        };
        let m = restamped(1, 0, &[length]);
        assert_eq!(
            m.forecast.as_deref(),
            Some(
                format!(
                    "+0h, statistics: accum over 6 {label}, increment 450s of type 2 (same run)"
                )
                .as_str()
            ),
            "{at}"
        );
        assert_eq!(m.offset_hours, Some(0), "{at}");
        // Both ends in the unit: before, `2–8 time-range unit 99 accum`.
        let m = restamped(unit, 2, &[length]);
        assert_eq!(
            m.forecast.as_deref(),
            Some(
                format!(
                    "+2 {label}, statistics: accum over 6 {label}, increment 450s of type 2 (same run)"
                )
                .as_str()
            ),
            "{at}"
        );
        assert_eq!((m.forecast_hours, m.offset_hours), (None, None), "{at}");
        // Runs whose increment is in the unit: before, `average of analyses
        // every 6 time-range unit 99 over 720h`.
        let m = restamped(
            1,
            0,
            &[Range {
                increment_unit: unit,
                ..monthly
            }],
        );
        assert_eq!(
            m.forecast.as_deref(),
            Some(
                format!(
                    "+0h, statistics: average over 720h, increment 6 {label} of type 1 (successive runs)"
                )
                .as_str()
            ),
            "{at}"
        );
        assert_eq!(m.offset_hours, Some(0), "{at}");
        // Runs over a length in the unit.
        let m = restamped(
            1,
            0,
            &[Range {
                length_unit: unit,
                ..monthly
            }],
        );
        assert_eq!(
            m.forecast.as_deref(),
            Some(
                format!(
                    "+0h, statistics: average over 30 {label}, increment 6h of type 1 (successive runs)"
                )
                .as_str()
            ),
            "{at}"
        );
        // Runs at a forecast time of 0 in the unit are analyses, as at 0
        // in any unit that is not missing.
        let m = restamped(unit, 0, &[monthly]);
        assert_eq!(
            m.forecast.as_deref(),
            Some("average of analyses every 6h over 720h"),
            "{at}"
        );
        assert_eq!(m.offset_hours, Some(0), "{at}");
        // At a nonzero forecast time in the unit the runs are of a lead,
        // which keeps its number and the unit's code, as a plain lead does.
        let m = restamped(
            unit,
            24,
            &[Range {
                increment: 24,
                ..monthly
            }],
        );
        assert_eq!(
            m.forecast.as_deref(),
            Some(format!("average of +24 {label} forecasts issued every 24h over 720h").as_str()),
            "{at}"
        );
        assert_eq!((m.forecast_hours, m.offset_hours), (None, None), "{at}");
        // Runs of windows, the inner window's length in the unit: before,
        // `average of 0–6 time-range unit 99 accums issued every 6h over 1
        // month`.
        let m = restamped(
            1,
            0,
            &[
                MONTHLY_OUTER,
                Range {
                    length_unit: unit,
                    ..MONTHLY_INNER
                },
            ],
        );
        assert_eq!(
            m.forecast.as_deref(),
            Some(
                format!(
                    "+0h, statistics: average over 1 month, increment 6h of type 1 (successive runs); \
                     accum over 6 {label}, increment 0h of type 2 (same run)"
                )
                .as_str()
            ),
            "{at}"
        );
        // An increment in the unit does not move a window, so the window
        // still reads, as with one in the missing unit.
        let m = restamped(
            1,
            0,
            &[Range {
                increment_unit: unit,
                increment: 0,
                ..MX2T3_RANGE
            }],
        );
        assert_eq!(m.forecast.as_deref(), Some("0–3h max"), "{at}");
    }

    // The same accumulation from 0 to 6 in a reserved unit reads raw in both
    // editions, since its length is in that unit, and both start at the
    // reference time.
    let g1 = grib1(20, 0, 6, 4);
    let g2 = restamped(
        20,
        0,
        &[Range {
            process: 1,
            length_unit: 20,
            length: 6,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(
        g1.forecast.as_deref(),
        Some("time range 4, P1 0 time unit 20, P2 6 time unit 20")
    );
    assert_eq!(
        g2.forecast.as_deref(),
        Some(
            "+0 time-range unit 20, statistics: accum over 6 time-range unit 20, \
             increment 450s of type 2 (same run)"
        )
    );
    assert_eq!(g1.offset_hours, g2.offset_hours);
    assert_eq!(g1.offset_hours, Some(0));
    // GRIB2 can state the length in hours from a forecast time of 0 in the
    // reserved unit, which GRIB1, with one unit for P1 and P2, cannot. That
    // window is known, and reads.
    let g2 = restamped(
        20,
        0,
        &[Range {
            process: 1,
            length: 6,
            ..MX2T3_RANGE
        }],
    );
    assert_eq!(g2.forecast.as_deref(), Some("0–6h accum"));
    assert_eq!(g2.offset_hours, Some(0));
    // A calendar unit is named, and still reads in words.
    let month = Range {
        process: 0,
        length_unit: 3,
        length: 1,
        ..MX2T3_RANGE
    };
    assert_eq!(
        restamped(3, 2, &[month]).forecast.as_deref(),
        Some("2–3 month average")
    );
    assert_eq!(
        grib1(3, 2, 3, 3).forecast.as_deref(),
        Some("2–3 month average")
    );
}

/// The outer time range of a monthly mean over successive runs: an average
/// over a month of runs 6 hours apart (Code Table 4.11 type 1).
const MONTHLY_OUTER: Range = Range {
    process: 0,
    increment_type: 1,
    length_unit: 3,
    length: 1,
    increment_unit: 1,
    increment: 6,
};

/// The inner time range under it: each run's accumulation over 6 hours from
/// the forecast time (type 2), continuous, so its increment is 0.
const MONTHLY_INNER: Range = Range {
    process: 1,
    increment_type: 2,
    length_unit: 1,
    length: 6,
    increment_unit: 1,
    increment: 0,
};

/// A statistic over successive runs of a statistic over one run's window,
/// as in CFSR's monthly means of 6-hour accumulations, reads as GRIB1 reads
/// NCEP's code 137, with the length of the runs after it (#1622). The
/// outer range is the first (Template 4.8 Note 4). The hours are the
/// forecast time, where the inner window starts, as GRIB1's 137 is at P1.
///
/// eccodes reads the committed message's inner window as this text does:
/// 2.34.1's `startStep` 0 and `endStep` 6, and 2.48.0's `stepRange` `0-6`.
/// 2.34.1's `stepRange` reads `0-262800`, which no time range in the message
/// states; see the fixture's `NOTICE.md` entry.
#[test]
fn a_monthly_mean_of_6h_accums_reads_as_grib1s_ncep_137() {
    // The committed fixture is the restamp below, with the end of the overall
    // time interval (§4 octets 35-41) moved to a month after the reference
    // time, 2026-10-04 00:00.
    let mut expected = restamped_bytes(1, 0, &[MONTHLY_OUTER, MONTHLY_INNER]);
    let s4 = section_start(&expected, 4);
    expected[s4 + 34..s4 + 41].copy_from_slice(&[0x07, 0xEA, 10, 4, 0, 0, 0]);
    assert_eq!(MONTHLY_MEAN_OF_6H_ACCUMS, expected.as_slice());

    let info = message(MONTHLY_MEAN_OF_6H_ACCUMS.to_vec());
    assert_eq!(
        info.forecast.as_deref(),
        Some("average of 0–6h accums issued every 6h over 1 month")
    );
    assert_eq!(info.offset_hours, Some(0));

    // A window later in each run, and one before the reference time.
    let later = restamped(
        1,
        6,
        &[
            MONTHLY_OUTER,
            Range {
                length: 12,
                ..MONTHLY_INNER
            },
        ],
    );
    assert_eq!(
        later.forecast.as_deref(),
        Some("average of 6–18h accums issued every 6h over 1 month")
    );
    assert_eq!(later.offset_hours, Some(6));
    let before = restamped(1, negative(6), &[MONTHLY_OUTER, MONTHLY_INNER]);
    assert_eq!(
        before.forecast.as_deref(),
        Some("average of −6h to +0h accums issued every 6h over 1 month")
    );
    assert_eq!(before.offset_hours, Some(-6));

    // An inner process GRIB1's NCEP codes do not have keeps its word, and
    // the outer process its own.
    let maxima = restamped(
        1,
        0,
        &[
            Range {
                process: 2,
                length_unit: 2,
                length: 30,
                increment: 24,
                ..MONTHLY_OUTER
            },
            Range {
                process: 2,
                length: 24,
                ..MONTHLY_INNER
            },
        ],
    );
    assert_eq!(
        maxima.forecast.as_deref(),
        Some("max of 0–24h max products issued every 24h over 720h")
    );
}

/// The same statistic reads the same in both editions: GRIB1's NCEP codes
/// 128 to 131 and 137 to 140, each an average of forecasts that are each an
/// accumulation or an average from P1 to P2, from runs 24, P2 − P1, 6 or 12
/// hours apart, and GRIB2's runs over windows with those increments. GRIB2
/// states how long the runs go on for, which GRIB1 keeps in octets this
/// crate does not read, so its text adds it.
#[test]
fn runs_over_windows_read_as_ncep_codes_do_in_grib1() {
    const GRIB1: &[u8] =
        include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");
    // (GRIB1 time range, P1, P2, GRIB2 inner process, outer increment in hours)
    for (time_range, p1, p2, process, every) in [
        (128u8, 0u8, 24u8, 1u8, 24u32),
        (129, 0, 12, 1, 12),
        (130, 0, 24, 0, 24),
        (131, 6, 18, 0, 12),
        (137, 0, 6, 1, 6),
        (138, 0, 6, 0, 6),
        (139, 12, 24, 1, 12),
        (140, 0, 12, 0, 12),
    ] {
        let mut grib1 = GRIB1.to_vec();
        // PDS octet 5 is the originating center, 7 for NCEP, and octets
        // 18-21 the unit (hour), P1, P2 and time range. §0 is 8 octets.
        grib1[8 + 5 - 1] = 7;
        grib1[8 + 18 - 1..8 + 22 - 1].copy_from_slice(&[1, p1, p2, time_range]);
        let grib1 = message(grib1);
        let grib2 = restamped(
            1,
            u32::from(p1),
            &[
                Range {
                    increment: every,
                    ..MONTHLY_OUTER
                },
                Range {
                    process,
                    length: u32::from(p2 - p1),
                    ..MONTHLY_INNER
                },
            ],
        );
        let grib1_text = grib1.forecast.expect("a GRIB1 forecast");
        assert_eq!(
            grib2.forecast.as_deref(),
            Some(format!("{grib1_text} over 1 month").as_str()),
            "time range {time_range}"
        );
        assert_eq!(
            grib1.offset_hours, grib2.offset_hours,
            "time range {time_range}"
        );
    }
}

/// Every other pair of time ranges is shown raw: no other pair of Code
/// Table 4.11 types is described, and runs over windows that GRIB1 would
/// show raw are raw here too. The hours stay the forecast time.
#[test]
fn two_time_ranges_other_than_runs_over_windows_are_shown_raw() {
    let raw = |outer: Range, inner: Range| {
        let info = restamped(1, 0, &[outer, inner]);
        assert_eq!(info.offset_hours, Some(0));
        info.forecast.expect("a forecast")
    };
    // One run's monthly mean of its own 6-hour accumulations (type 2 over
    // type 2).
    assert_eq!(
        raw(
            Range {
                increment_type: 2,
                ..MONTHLY_OUTER
            },
            MONTHLY_INNER
        ),
        "+0h, statistics: average over 1 month, increment 6h of type 2 (same run); \
         accum over 6h, increment 0h of type 2 (same run)"
    );
    // Runs over runs, and runs over fields valid at one time (types 3 to 5),
    // reserved (0), missing (255) or local (200).
    for inner_type in [1u8, 3, 4, 5, 0, 255, 200] {
        let text = raw(
            MONTHLY_OUTER,
            Range {
                increment_type: inner_type,
                ..MONTHLY_INNER
            },
        );
        assert!(
            text.starts_with("+0h, statistics: "),
            "{inner_type}: {text}"
        );
    }
    for outer_type in [3u8, 4, 5, 0, 255, 200] {
        let text = raw(
            Range {
                increment_type: outer_type,
                ..MONTHLY_OUTER
            },
            MONTHLY_INNER,
        );
        assert!(
            text.starts_with("+0h, statistics: "),
            "{outer_type}: {text}"
        );
    }
    // Runs no time apart, over no time, or in the missing unit.
    for outer in [
        Range {
            increment: 0,
            ..MONTHLY_OUTER
        },
        Range {
            length: 0,
            ..MONTHLY_OUTER
        },
        Range {
            increment_unit: 255,
            ..MONTHLY_OUTER
        },
        Range {
            length_unit: 255,
            ..MONTHLY_OUTER
        },
    ] {
        let text = raw(outer, MONTHLY_INNER);
        assert!(text.starts_with("+0h, statistics: "), "{text}");
    }
    // Three time ranges, even when the first two are runs over windows.
    let three = restamped(1, 0, &[MONTHLY_OUTER, MONTHLY_INNER, MONTHLY_INNER]);
    assert_eq!(
        three.forecast.as_deref(),
        Some(
            "+0h, statistics: average over 1 month, increment 6h of type 1 (successive runs); \
             accum over 6h, increment 0h of type 2 (same run); and 1 more"
        )
    );
    assert_eq!(three.offset_hours, Some(0));
    // A window over no time, or in the missing unit.
    assert_eq!(
        raw(
            MONTHLY_OUTER,
            Range {
                length: 0,
                ..MONTHLY_INNER
            }
        ),
        "+0h, statistics: average over 1 month, increment 6h of type 1 (successive runs); \
         accum over 0h, increment 0h of type 2 (same run)"
    );
    let text = raw(
        MONTHLY_OUTER,
        Range {
            length_unit: 255,
            ..MONTHLY_INNER
        },
    );
    assert!(text.starts_with("+0h, statistics: "), "{text}");
}
