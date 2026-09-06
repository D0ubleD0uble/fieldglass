//! Run discovery: which object keys should exist by now.
//!
//! A host that wants "the latest HRRR" has to turn a wall-clock instant into a
//! key, and the key is a template over a model's cycle schedule. This crate
//! expands the template and nothing more:
//!
//! * **The catalog is the host's data.** [`SourceSpec`] is one entry of it. No
//!   bucket name, no model name and no URL is written down here; a source that
//!   moves is a data edit, not a release.
//! * **The clock is the host's too.** `now` is a parameter. ADR-0005 keeps I/O
//!   at the host, and a planner that read the clock could not be tested at a
//!   fixed instant.
//! * **A candidate is a guess.** `candidates` says which runs *should* have
//!   posted; only a fetch says which did. That is why it returns a list, newest
//!   first, rather than an answer.
//!
//! The calendar arithmetic is the proleptic Gregorian one, days from the Unix
//! epoch, with no leap seconds — which is exactly what Unix time is defined to
//! count, so there is nothing here to get wrong about them.

use crate::error::FetchPlanError;

/// Seconds in an hour, and in a day.
const SECS_PER_HOUR: i64 = 3_600;
/// Seconds in a day. No leap seconds: Unix time does not count them.
const SECS_PER_DAY: i64 = 86_400;

/// The widest instant the day arithmetic below converts without overflowing an
/// `i64` day count, with room to spare. Roughly ±2.9 million years.
const MAX_UNIX_SECS: i64 = i64::MAX / 1_000;

/// One entry of the host's source catalog: how a model names its objects and
/// when it posts them.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SourceSpec {
    /// The object-key template, e.g.
    /// `hrrr.{yyyy}{mm}{dd}/conus/hrrr.t{HH}z.wrfsfcf{ff}.grib2`.
    ///
    /// Placeholders are `{yyyy}` `{yy}` `{mm}` `{dd}` `{doy}` for the cycle's
    /// date, `{HH}` `{H}` for its hour, and `{fff}` `{ff}` `{f}` for the
    /// forecast step in hours. Anything else is refused by
    /// [`validate`](Self::validate) rather than passed through: a key with an
    /// unexpanded brace in it fetches nothing, and the 404 that follows is a
    /// much worse error message.
    pub key_pattern: String,
    /// The hours of the day this model runs, strictly ascending, each `0..=23`.
    pub cycle_hours: Vec<u8>,
    /// How long after a cycle's nominal time its objects typically appear.
    /// A cycle is not offered as a candidate until `now` is past it.
    pub latency_minutes: u32,
    /// How many cycles back to offer, newest first. A host walks them until a
    /// fetch succeeds, which is what covers a late or skipped run.
    pub max_candidates: u32,
}

impl SourceSpec {
    /// Check the entry before it is used.
    ///
    /// Called by [`candidates`] as well, so a host that skipped it still cannot
    /// get a malformed key out.
    pub fn validate(&self) -> Result<(), FetchPlanError> {
        if self.cycle_hours.is_empty() {
            return Err(FetchPlanError::NoCycleHours);
        }
        if let Some(&hour) = self.cycle_hours.iter().find(|&&h| h > 23) {
            return Err(FetchPlanError::CycleHourOutOfRange { hour });
        }
        // Strict, so this catches a duplicate as well as an unsorted list. A
        // duplicate would emit the same run twice and make the candidate list
        // silently shorter than it looks.
        if self.cycle_hours.windows(2).any(|w| w[0] >= w[1]) {
            return Err(FetchPlanError::UnsortedCycleHours {
                hours: self.cycle_hours.clone(),
            });
        }
        // Expanded against a fixed instant purely to reach every placeholder in
        // the pattern; the result is discarded.
        expand(&self.key_pattern, &CycleFields::probe()).map(|_| ())
    }
}

/// A run this source should have posted, and the key it should be under.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Candidate {
    /// The expanded object key.
    pub key: String,
    /// The cycle's nominal time, as Unix seconds.
    pub cycle_unix_secs: i64,
    /// The forecast step, in hours, that this key was expanded for.
    pub step_hours: u32,
}

/// The runs of `source` that should have posted by `now`, newest first.
///
/// **Monotone in `now`.** Advancing the clock never removes a run from the
/// front of the list or reorders what is there: the newest candidate's cycle
/// time is non-decreasing in `now`, because a cycle becomes eligible when
/// `now` passes its nominal time plus the latency and nothing makes it
/// ineligible again. A host can therefore poll on a timer and compare against
/// what it fetched last time without the list shifting under it.
///
/// Returns at most [`SourceSpec::max_candidates`] entries, and in practice
/// exactly that many: the calendar is proleptic and a schedule has no first
/// run, so there is always an earlier cycle. The list is empty only when the
/// caller asked for none. A host therefore stops when a fetch succeeds, not
/// when the list runs out.
pub fn candidates(
    source: &SourceSpec,
    now_unix_secs: i64,
    step_hours: u32,
) -> Result<Vec<Candidate>, FetchPlanError> {
    source.validate()?;
    // `abs()` and not `unsigned_abs()` would be the natural spelling and is a
    // panic: `i64::MIN` has no positive counterpart, so it overflows in debug
    // and wraps to itself in release. A host passing a clock it read from
    // somewhere untrusted must get the refusal, not the panic.
    if !(-MAX_UNIX_SECS..=MAX_UNIX_SECS).contains(&now_unix_secs) {
        return Err(FetchPlanError::TimeOutOfRange {
            unix_secs: now_unix_secs,
        });
    }

    // A cycle is due once `now` is past its nominal time plus the latency, so
    // the search runs against the clock shifted back by the latency.
    let due_by = now_unix_secs - i64::from(source.latency_minutes) * 60;

    let mut out = Vec::new();
    // Walk days backwards from the day `due_by` falls in, taking each day's
    // cycle hours from latest to earliest. Bounded by `max_candidates`, and by
    // a day budget so a source with one cycle a day and a large
    // `max_candidates` still terminates in one pass rather than scanning to the
    // epoch.
    let mut day = div_floor(due_by, SECS_PER_DAY);
    let per_day = source.cycle_hours.len() as u32;
    let days_needed = source.max_candidates.div_ceil(per_day.max(1)) + 1;

    for _ in 0..days_needed {
        for &hour in source.cycle_hours.iter().rev() {
            let cycle = day * SECS_PER_DAY + i64::from(hour) * SECS_PER_HOUR;
            if cycle > due_by {
                continue;
            }
            if out.len() as u32 >= source.max_candidates {
                return Ok(out);
            }
            out.push(Candidate {
                key: expand(
                    &source.key_pattern,
                    &CycleFields::at(cycle, hour, step_hours)?,
                )?,
                cycle_unix_secs: cycle,
                step_hours,
            });
        }
        day -= 1;
    }
    Ok(out)
}

/// The values a key pattern can be expanded with.
#[derive(Debug)]
struct CycleFields {
    year: i64,
    month: u32,
    day: u32,
    day_of_year: u32,
    hour: u8,
    step: u32,
}

impl CycleFields {
    /// The fields of one cycle.
    fn at(cycle_unix_secs: i64, hour: u8, step: u32) -> Result<Self, FetchPlanError> {
        let days = div_floor(cycle_unix_secs, SECS_PER_DAY);
        let (year, month, day) = civil_from_days(days);
        let day_of_year = (days - days_from_civil(year, 1, 1) + 1) as u32;
        Ok(Self {
            year,
            month,
            day,
            day_of_year,
            hour,
            step,
        })
    }

    /// A fixed set of fields for [`SourceSpec::validate`], which needs to reach
    /// every placeholder in the pattern but does not care what they expand to.
    fn probe() -> Self {
        Self {
            year: 2000,
            month: 1,
            day: 1,
            day_of_year: 1,
            hour: 0,
            step: 0,
        }
    }

    /// The value of one placeholder, or `None` when the name is unknown.
    fn placeholder(&self, name: &str) -> Option<String> {
        Some(match name {
            "yyyy" => format!("{:04}", self.year),
            "yy" => format!("{:02}", self.year.rem_euclid(100)),
            "mm" => format!("{:02}", self.month),
            "dd" => format!("{:02}", self.day),
            "doy" => format!("{:03}", self.day_of_year),
            "HH" => format!("{:02}", self.hour),
            "H" => self.hour.to_string(),
            "fff" => format!("{:03}", self.step),
            "ff" => format!("{:02}", self.step),
            "f" => self.step.to_string(),
            _ => return None,
        })
    }
}

/// Substitute `{name}` placeholders in a key pattern.
///
/// An unknown name or an unterminated brace is an error, not a pass-through.
fn expand(pattern: &str, fields: &CycleFields) -> Result<String, FetchPlanError> {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    let mut consumed = 0usize;

    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or(FetchPlanError::UnterminatedPlaceholder {
                at: consumed + open,
            })?;
        let name = &after[..close];
        out.push_str(&fields.placeholder(name).ok_or_else(|| {
            FetchPlanError::UnknownPlaceholder {
                placeholder: name.to_string(),
            }
        })?);
        consumed += open + 1 + close + 1;
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Floor division, which is what a negative day count needs.
///
/// `-1 / 86400` truncates towards zero and gives day 0 — the day *after* the
/// one that instant is in. Every pre-1970 date would land one day late.
fn div_floor(a: i64, b: i64) -> i64 {
    let q = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

/// Civil date from a day count since 1970-01-01, proleptic Gregorian.
///
/// Howard Hinnant's `civil_from_days`, which is exact over the whole `i64`
/// range this crate admits and needs no table. Shifting the era to start in
/// March is what makes the leap day the last day of the year, so the
/// day-of-year arithmetic has no February special case in it.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = div_floor(z, 146_097);
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Day count since 1970-01-01 for a civil date, proleptic Gregorian.
///
/// The inverse of [`civil_from_days`], used only to find the first of January
/// so a day-of-year can be measured against it.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = div_floor(y, 400);
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-09-04T00:00:00Z, the reference time on the committed sidecars.
    const SEP_4: i64 = 1_788_480_000;

    fn hrrr() -> SourceSpec {
        SourceSpec {
            key_pattern: "hrrr.{yyyy}{mm}{dd}/conus/hrrr.t{HH}z.wrfsfcf{ff}.grib2".to_string(),
            cycle_hours: (0..24).collect(),
            latency_minutes: 55,
            max_candidates: 3,
        }
    }

    /// The epoch and a handful of dates either side of it, including a leap
    /// day and a century boundary, so the calendar is checked rather than
    /// assumed. These are the values a wrong `div_floor` or a March-shift error
    /// moves by exactly one day.
    #[test]
    fn the_calendar_round_trips_across_the_awkward_dates() {
        for (days, ymd) in [
            (0_i64, (1970, 1, 1)),
            (-1, (1969, 12, 31)),
            (-719_162, (1, 1, 1)),
            (11_016, (2000, 2, 29)),
            (11_017, (2000, 3, 1)),
            (-25_567, (1900, 1, 1)),
            (20_700, (2026, 9, 4)),
        ] {
            assert_eq!(civil_from_days(days), ymd, "day {days}");
            assert_eq!(days_from_civil(ymd.0, ymd.1, ymd.2), days, "{ymd:?}");
        }
    }

    /// A pre-epoch instant must not land a day late, which is what truncating
    /// division does to a negative day count.
    #[test]
    fn a_pre_epoch_instant_lands_on_the_right_day() {
        // One second before the epoch is still 1969-12-31.
        assert_eq!(civil_from_days(div_floor(-1, SECS_PER_DAY)), (1969, 12, 31));
        // Truncating division would have said 1970-01-01.
        assert_ne!(civil_from_days(-1 / SECS_PER_DAY), (1969, 12, 31));
    }

    /// Every placeholder expands, and the day-of-year is the one `date +%j`
    /// gives: 2026-09-04 is day 247 of a non-leap year.
    #[test]
    fn every_placeholder_expands() {
        let fields = CycleFields::at(SEP_4 + 6 * SECS_PER_HOUR, 6, 12).unwrap();
        let key = expand(
            "{yyyy}-{yy}-{mm}-{dd}-{doy}-{HH}-{H}-{fff}-{ff}-{f}",
            &fields,
        )
        .unwrap();
        assert_eq!(key, "2026-26-09-04-247-06-6-012-12-12");
    }

    /// An unexpanded brace would fetch nothing, so it is refused at validation
    /// rather than passed through into a key.
    #[test]
    fn a_bad_pattern_is_refused_by_name() {
        let mut spec = hrrr();
        spec.key_pattern = "a/{cycle}/b".into();
        assert_eq!(
            spec.validate(),
            Err(FetchPlanError::UnknownPlaceholder {
                placeholder: "cycle".to_string()
            })
        );

        spec.key_pattern = "a/{yyyy".into();
        assert_eq!(
            spec.validate(),
            Err(FetchPlanError::UnterminatedPlaceholder { at: 2 })
        );
    }

    /// A catalog entry that describes no runs, or describes them twice, is
    /// refused: both would make the candidate list quietly wrong rather than
    /// empty.
    #[test]
    fn a_malformed_schedule_is_refused() {
        let mut spec = hrrr();
        spec.cycle_hours = vec![];
        assert_eq!(spec.validate(), Err(FetchPlanError::NoCycleHours));

        spec.cycle_hours = vec![0, 24];
        assert_eq!(
            spec.validate(),
            Err(FetchPlanError::CycleHourOutOfRange { hour: 24 })
        );

        spec.cycle_hours = vec![0, 6, 6, 12];
        assert_eq!(
            spec.validate(),
            Err(FetchPlanError::UnsortedCycleHours {
                hours: vec![0, 6, 6, 12]
            })
        );

        spec.cycle_hours = vec![12, 6];
        assert!(matches!(
            spec.validate(),
            Err(FetchPlanError::UnsortedCycleHours { .. })
        ));
    }

    /// The latency is what keeps a cycle out of the list until it has had time
    /// to post. At 00:54Z the 00Z HRRR run is not offered; at 00:55Z it is.
    #[test]
    fn a_cycle_is_not_offered_until_its_latency_has_passed() {
        let spec = hrrr();
        let just_before = candidates(&spec, SEP_4 + 54 * 60, 0).unwrap();
        assert_eq!(just_before[0].cycle_unix_secs, SEP_4 - SECS_PER_HOUR);

        let just_after = candidates(&spec, SEP_4 + 55 * 60, 0).unwrap();
        assert_eq!(just_after[0].cycle_unix_secs, SEP_4);
        assert_eq!(
            just_after[0].key,
            "hrrr.20260904/conus/hrrr.t00z.wrfsfcf00.grib2"
        );
    }

    /// Newest first, strictly descending, and no more than asked for.
    #[test]
    fn candidates_are_newest_first_and_bounded() {
        let spec = SourceSpec {
            key_pattern: "gfs.{yyyy}{mm}{dd}/{HH}/atmos/gfs.t{HH}z.pgrb2.0p25.f{fff}".to_string(),
            cycle_hours: vec![0, 6, 12, 18],
            latency_minutes: 0,
            max_candidates: 5,
        };
        let got = candidates(&spec, SEP_4 + SECS_PER_HOUR, 3).unwrap();
        assert_eq!(got.len(), 5);
        assert!(
            got.windows(2)
                .all(|w| w[0].cycle_unix_secs > w[1].cycle_unix_secs),
            "{got:?}"
        );
        assert_eq!(got[0].key, "gfs.20260904/00/atmos/gfs.t00z.pgrb2.0p25.f003");
        // …and the walk crosses midnight into the previous day's 18Z run.
        assert_eq!(got[1].key, "gfs.20260903/18/atmos/gfs.t18z.pgrb2.0p25.f003");
    }

    /// The monotonicity the acceptance asks for, checked over a day of minutes
    /// rather than asserted: as `now` advances the newest cycle offered never
    /// goes backwards, and the list stays sorted at every instant.
    #[test]
    fn the_newest_candidate_never_moves_backwards_as_now_advances() {
        let spec = SourceSpec {
            key_pattern: "m/{yyyy}{mm}{dd}/{HH}".to_string(),
            cycle_hours: vec![0, 6, 12, 18],
            latency_minutes: 37,
            max_candidates: 4,
        };
        let mut newest = i64::MIN;
        let mut seen_advance = false;
        for minute in 0..(48 * 60) {
            let got = candidates(&spec, SEP_4 + minute * 60, 0).unwrap();
            assert!(
                got.windows(2)
                    .all(|w| w[0].cycle_unix_secs > w[1].cycle_unix_secs)
            );
            let head = got[0].cycle_unix_secs;
            assert!(head >= newest, "went backwards at minute {minute}");
            seen_advance |= head > newest;
            newest = head;
        }
        assert!(seen_advance, "the clock never advanced a cycle at all");
    }

    /// The calendar is proleptic and a schedule has no first run, so there is
    /// always an earlier cycle: the list is empty only when the caller asked
    /// for none. Written down because the tempting assumption — that a large
    /// latency eventually exhausts the schedule — is false, and a host relying
    /// on an empty list to mean "nothing has posted yet" would wait forever.
    #[test]
    fn the_list_is_empty_only_when_none_were_asked_for() {
        let mut spec = SourceSpec {
            key_pattern: "m/{yyyy}{mm}{dd}{HH}".to_string(),
            cycle_hours: vec![12],
            latency_minutes: u32::MAX / 60,
            max_candidates: 3,
        };
        // A latency of years just walks the schedule back years; it does not
        // run out of cycles.
        assert_eq!(candidates(&spec, 0, 0).unwrap().len(), 3);

        spec.max_candidates = 0;
        assert!(candidates(&spec, 0, 0).unwrap().is_empty());
    }

    /// The day budget has to cover a source with one cycle a day; a walk sized
    /// only by `max_candidates / per_day` would come up short.
    #[test]
    fn a_once_daily_source_still_fills_its_quota() {
        let spec = SourceSpec {
            key_pattern: "d/{yyyy}{mm}{dd}".to_string(),
            cycle_hours: vec![0],
            latency_minutes: 0,
            max_candidates: 4,
        };
        let got = candidates(&spec, SEP_4 + SECS_PER_HOUR, 0).unwrap();
        assert_eq!(got.len(), 4);
        assert_eq!(got[0].key, "d/20260904");
        assert_eq!(got[3].key, "d/20260901");
    }

    /// An absurd instant is refused rather than wrapped into a plausible date.
    ///
    /// Both ends, and `i64::MIN` specifically: the obvious range check is
    /// `now.abs() > MAX`, and `i64::MIN.abs()` overflows — a panic in debug, a
    /// wrap back to `i64::MIN` in release, which then compares as *less* than
    /// the bound and sails through into the day arithmetic.
    #[test]
    fn an_out_of_range_clock_is_refused_at_both_ends() {
        for absurd in [i64::MAX, i64::MIN, MAX_UNIX_SECS + 1, -MAX_UNIX_SECS - 1] {
            assert_eq!(
                candidates(&hrrr(), absurd, 0),
                Err(FetchPlanError::TimeOutOfRange { unix_secs: absurd }),
                "{absurd}"
            );
        }
        // …and the boundary itself is still accepted, so the check is a bound
        // and not an off-by-one that quietly narrows the range.
        assert!(candidates(&hrrr(), MAX_UNIX_SECS, 0).is_ok());
    }
}
