//! The colour range: one rule, for every host (#1155).
//!
//! Before this, the Node addon and the browser module chose a range by
//! different rules (#880 measured both). The addon dropped a single manual end
//! and auto-ranged over the *warped* raster, so a window or the output size
//! moved the colours; the browser refused an empty field and let an inverted
//! or non-finite range paint one colour. Both now call [`colour_range`], and so
//! do the contour levels, so the isolines and the colours they sit on agree.
//!
//! The rules, as decided on #880 and checked against matplotlib, xarray, QGIS
//! and ncview there:
//!
//! 1. A field with no present cell and an automatic range has no range: the
//!    raster is transparent and the range is `null`, not an error.
//! 2. One manual end is used as given and the other comes from the data. If
//!    that inverts the range, the refusal says where the other end came from.
//! 3. A manual maximum not above a manual minimum is refused, naming both. A
//!    constant field's automatic `min == max` still paints.
//! 4. A non-finite manual end is refused, on either scale; a log10 scale needs
//!    a finite, positive minimum.
//! 5. A refusal is [`Error::InvalidOption`] with a [`RangeRefusal`] beside the
//!    prose: which end, the value given, the field's own range, and a stable
//!    code.
//! 6. The automatic range is the present-cell minimum and maximum of the whole
//!    decoded 2-D field, taken before any warp. A window is a view of the
//!    field, not a new one.
//!
//! A `center` makes the range symmetric about it, as xarray's `center` does:
//! with no manual end the wider of the data's two distances from it is used on
//! both sides, and with one manual end the other is that end mirrored about it.
//! Under a log10 scale the symmetry is in `log10`, which is where the ramp is
//! linear. A center with both ends given is refused rather than ignored, since
//! the two ends already fix the range.

use crate::Error;
use crate::api::{
    ColourRange, OutOfRange, RangeEnd, RangeRefusal, RangeRefusalReason, RangeSource, Stats,
};

/// How many levels contours aim for when the caller names neither levels nor
/// an interval. Both hosts space them over the same [`colour_range`], so the
/// count is one number too (#880 item 8: the browser's was 10 and the addon's
/// 8).
pub const AUTO_CONTOUR_LEVELS: usize = 8;

/// The present-cell range a **display** field is coloured over: the full
/// field's, from stats the caller supplies, checked (#1082).
///
/// A field decoded at a reduced resolution is a wavelet low-pass, which trims
/// some of the field's extremes and overshoots others, so its own range is not
/// the file's and would differ between viewers at different reductions. Rule 6
/// says the automatic range is the whole decoded field's, so a display field's
/// is taken from that field's [`Stats`] (`full`), held from an earlier full
/// decode, and handed to [`colour_range`] as its `extremes`. Both hosts call
/// this: `Session::render_display` and `palette_display`, and the Node addon's
/// display render.
///
/// When `min` and `max` are both given the rule reads no data, so `full` may be
/// `None` and is not checked; a good one is still passed on, for a refusal's
/// `field_min` and `field_max`. `display_has_values` is whether the display field has a
/// present cell.
///
/// # Errors
///
/// [`Error::InvalidOption`], only when an end is left to the data: when `full` is
/// `None`; when `full` states no present cell and the display field has some
/// (it would paint transparent and say nothing); and when `full` is not a
/// range: one end missing, an end not finite, or the minimum above the
/// maximum.
pub fn full_field_extremes(
    full: Option<&Stats>,
    display_has_values: bool,
    min: Option<f64>,
    max: Option<f64>,
) -> Result<Option<(f64, f64)>, Error> {
    let range = |stats: &Stats| match (stats.min, stats.max) {
        (Some(lo), Some(hi)) if lo.is_finite() && hi.is_finite() && lo <= hi => Some((lo, hi)),
        _ => None,
    };
    // Both ends manual: the rule reads no data, so stats it would ignore are
    // not refused. Good ones are still passed on, for a refusal's
    // `field_min` / `field_max`.
    if min.is_some() && max.is_some() {
        return Ok(full.and_then(range));
    }
    let Some(stats) = full else {
        return Err(Error::invalid_option(
            "a display field's automatic colour range is the full field's, and no full-field \
             stats were given; pass the stats of the same message decoded at reduction 0, or \
             set both a minimum and a maximum"
                .to_string(),
        ));
    };
    if let Some(extremes) = range(stats) {
        return Ok(Some(extremes));
    }
    match (stats.min, stats.max) {
        (None, None) => {
            if display_has_values {
                return Err(Error::invalid_option(
                    "the full-field stats state no present cell, and this display field has \
                     some; pass the stats of the same message decoded at reduction 0"
                        .to_string(),
                ));
            }
            Ok(None)
        }
        (lo, hi) => Err(Error::invalid_option(format!(
            "the full-field stats are not a range (minimum {lo:?}, maximum {hi:?}); pass the \
             stats of the same message decoded at reduction 0"
        ))),
    }
}

/// The colour range a render uses, by the one rule every host follows.
///
/// `extremes` is the field's present-cell `(min, max)`, over finite values,
/// before any warp — `Field::stats`, or `min_max_ignoring_mask` over the
/// values a host decoded; `None` when no cell is present. `min`, `max` and
/// `center` are the caller's, each `None` when not given.
///
/// The returned range has its counts at zero: the painter fills them, in the
/// pass that colours the cells (`ColourRange::counted`, behind `render`).
///
/// # Errors
///
/// [`Error::InvalidOption`] carrying a [`RangeRefusal`], for the cases in the
/// module docs.
pub fn colour_range(
    extremes: Option<(f64, f64)>,
    min: Option<f64>,
    max: Option<f64>,
    center: Option<f64>,
    log10: bool,
) -> Result<ColourRange, Error> {
    let refuse = |reason, end, given: Option<f64>, detail: String| Error::InvalidOption {
        detail,
        range_refusal: Some(RangeRefusal {
            reason,
            end,
            given: given.filter(|v| v.is_finite()),
            field_min: extremes.map(|(lo, _)| lo),
            field_max: extremes.map(|(_, hi)| hi),
        }),
    };

    for (end, name, value) in [
        (RangeEnd::Min, "minimum", min),
        (RangeEnd::Max, "maximum", max),
        (RangeEnd::Center, "center", center),
    ] {
        if let Some(v) = value
            && !v.is_finite()
        {
            // NaN is what a field holding text it cannot parse arrives as, and
            // "NaN is not a finite number" would read as jargon to its typist.
            let detail = if v.is_nan() {
                format!("the {name} is not a number; set a number or clear it")
            } else {
                format!("the {name} {v} is not a finite number; set a finite {name} or clear it")
            };
            return Err(refuse(RangeRefusalReason::NotFinite, end, Some(v), detail));
        }
    }
    if log10 {
        for (end, name, value) in [
            (RangeEnd::Min, "minimum", min),
            (RangeEnd::Center, "center", center),
        ] {
            if let Some(v) = value
                && v <= 0.0
            {
                return Err(refuse(
                    RangeRefusalReason::Log10NotPositive,
                    end,
                    Some(v),
                    format!(
                        "a log10 scale needs a finite, positive {name}; the {name} given is \
                         {v}; set a {name} above 0"
                    ),
                ));
            }
        }
    }

    // The ramp is linear in this space, so a center's symmetry is taken here.
    let to = |v: f64| if log10 { v.log10() } else { v };
    let from = |t: f64| if log10 { 10f64.powf(t) } else { t };
    // The data's minimum as the range's low end has to have a logarithm too.
    // Nothing was given for that end, so `given` is `None`; the value is the
    // refusal's `field_min`, and the message says where it came from.
    let log10_floor = |lo: f64| -> Result<(), Error> {
        if log10 && lo <= 0.0 {
            return Err(refuse(
                RangeRefusalReason::Log10NotPositive,
                RangeEnd::Min,
                None,
                // Rounding keeps the sign of a value that is not zero, so
                // the figures the colour bar would use never read as
                // positive.
                format!(
                    "a log10 scale needs a finite, positive minimum; the range starts at \
                     {} (taken from the data); set a minimum above 0",
                    to_precision(lo, colour_bar_figures(lo))
                ),
            ));
        }
        Ok(())
    };

    // Every value compared from here on is finite: the caller's were refused
    // above if not, and `extremes` is over finite cells. So `hi <= lo` is the
    // whole of "not above", with no `NaN` slipping past it.
    let (source, used) = match (min, max) {
        (Some(lo), Some(hi)) => {
            if let Some(c) = center {
                return Err(refuse(
                    RangeRefusalReason::CenterWithBothEnds,
                    RangeEnd::Center,
                    Some(c),
                    format!(
                        "a center needs at most one manual end; minimum {lo} and maximum {hi} \
                         already fix the range, so clear one of them or the center"
                    ),
                ));
            }
            if hi <= lo {
                return Err(refuse(
                    RangeRefusalReason::Inverted,
                    RangeEnd::Both,
                    None,
                    format!(
                        "minimum {lo} is not below maximum {hi}; set a minimum below the maximum"
                    ),
                ));
            }
            (RangeSource::Manual, Some((lo, hi)))
        }
        (Some(lo), None) => {
            let hi = match (center, extremes) {
                (Some(c), _) => {
                    if lo >= c {
                        return Err(refuse(
                            RangeRefusalReason::CenterOutside,
                            RangeEnd::Center,
                            Some(c),
                            format!(
                                "center {c} is not above minimum {lo}; set a center above the \
                                 minimum"
                            ),
                        ));
                    }
                    Some(from(2.0 * to(c) - to(lo)))
                }
                (None, Some((_, data_hi))) => {
                    if data_hi <= lo {
                        let relation = if lo > data_hi {
                            "is above"
                        } else {
                            "is not below"
                        };
                        return Err(refuse(
                            RangeRefusalReason::Inverted,
                            RangeEnd::Min,
                            Some(lo),
                            format!(
                                "minimum {lo} {relation} the field's maximum {} (taken from the \
                                 data); set a maximum too",
                                data_end(data_hi, lo)
                            ),
                        ));
                    }
                    Some(data_hi)
                }
                (None, None) => None,
            };
            (RangeSource::ManualMin, hi.map(|hi| (lo, hi)))
        }
        (None, Some(hi)) => {
            let lo = match (center, extremes) {
                (Some(c), _) => {
                    if hi <= c {
                        return Err(refuse(
                            RangeRefusalReason::CenterOutside,
                            RangeEnd::Center,
                            Some(c),
                            format!(
                                "center {c} is not below maximum {hi}; set a center below the \
                                 maximum"
                            ),
                        ));
                    }
                    Some(from(2.0 * to(c) - to(hi)))
                }
                (None, Some((data_lo, _))) => {
                    if hi <= data_lo {
                        let relation = if hi < data_lo {
                            "is below"
                        } else {
                            "is not above"
                        };
                        return Err(refuse(
                            RangeRefusalReason::Inverted,
                            RangeEnd::Max,
                            Some(hi),
                            format!(
                                "maximum {hi} {relation} the field's minimum {} (taken from the \
                                 data); set a minimum too",
                                data_end(data_lo, hi)
                            ),
                        ));
                    }
                    log10_floor(data_lo)?;
                    Some(data_lo)
                }
                (None, None) => None,
            };
            (RangeSource::ManualMax, lo.map(|lo| (lo, hi)))
        }
        (None, None) => {
            let used = match extremes {
                None => None,
                Some((lo, hi)) => {
                    log10_floor(lo)?;
                    match center {
                        None => Some((lo, hi)),
                        Some(c) => {
                            let tc = to(c);
                            let reach = (to(lo) - tc).abs().max((to(hi) - tc).abs());
                            Some((from(tc - reach), from(tc + reach)))
                        }
                    }
                }
            };
            (RangeSource::Auto, used)
        }
    };
    // Mirroring about a center can carry an end past what an `f64` holds
    // (`2c − min` for a center near `f64::MAX`), and an infinite end paints
    // every cell one colour. Only a center moves an end, so it is the one
    // named.
    if let (Some((lo, hi)), Some(c)) = (used, center)
        && !(lo.is_finite() && hi.is_finite())
    {
        return Err(refuse(
            RangeRefusalReason::NotFinite,
            RangeEnd::Center,
            Some(c),
            format!(
                "centring on {c} puts an end of the range past the largest number there is; \
                 set a center nearer the data"
            ),
        ));
    }
    // Under log10 the mirror is `c² / end`, which can underflow to 0: a low
    // end with no logarithm, and a domain starting at `-inf`.
    if let (Some((lo, _)), Some(c)) = (used, center)
        && log10
        && lo <= 0.0
    {
        return Err(refuse(
            RangeRefusalReason::NotFinite,
            RangeEnd::Center,
            Some(c),
            format!(
                "centring on {c} on a log10 scale puts the low end of the range below the \
                 smallest number there is; set a center nearer the data"
            ),
        ));
    }

    Ok(ColourRange {
        used_min: used.map(|(lo, _)| lo),
        used_max: used.map(|(_, hi)| hi),
        range_source: source,
        center,
        out_of_range: OutOfRange::Clamp,
        below: 0,
        above: 0,
        present: 0,
        total: 0,
    })
}

/// Whether a scale name asks for log10: `None` and `"linear"` do not, and any
/// other name is refused.
///
/// The one scale parser (#880 item 5). Here rather than in `render` because
/// contours take a scale too (it decides what range they may be spaced over)
/// and are behind `analysis` alone.
pub(crate) fn scale_is_log10(name: Option<&str>) -> Result<bool, Error> {
    match name {
        None | Some("linear") => Ok(false),
        Some("log10") => Ok(true),
        Some(other) => Err(Error::invalid_option(format!(
            "unknown scale mode {other:?} (expected \"linear\" or \"log10\")"
        ))),
    }
}

/// The fewest significant figures the extension's colour bar writes an end
/// with (`formatReading(v, 4)` in `extension/src/render-panel.ts`).
const COLOUR_BAR_FIGURES: usize = 4;

/// The significant figures the colour bar writes `v` with: at least
/// [`COLOUR_BAR_FIGURES`], and the whole part in full below 2^53, so
/// `1963536` reads as itself and not `1.964e+6`, `1700003600` seconds as
/// itself and not `1.700e+9` (#1432). A fraction of a billion or more takes
/// one decimal as well, so `1700000000.5` seconds reads as itself, not
/// `1.700e+9`, and apart from `1700000001` (#1450). The same rule as `readingFigures`
/// in `extension/src/render-panel.ts`, which counts the digits of
/// `Math.round(|v|)`; for a value that is not negative, Rust's `round` and
/// JavaScript's agree.
fn colour_bar_figures(v: f64) -> usize {
    /// 2^53: every whole number below it is an `f64` exactly, and every
    /// `f64` from 2^52 up is a whole number, so a fraction is below it.
    const EXACT_WHOLE: f64 = 9_007_199_254_740_992.0;
    let size = v.abs();
    // Not a number, too: it compares as nothing.
    if size.is_nan() || size >= EXACT_WHOLE {
        return COLOUR_BAR_FIGURES;
    }
    let figures = COLOUR_BAR_FIGURES.max(format!("{:.0}", size.round()).len());
    // At most 16 + 1 figures, inside what `to_precision` writes.
    if size >= 1e9 && size.fract() != 0.0 {
        figures + 1
    } else {
        figures
    }
}

/// An end of the range taken from the data, as a refusal names it (#1053).
///
/// The field's own value is float noise past a few figures (`311.0986328125`
/// for a bar that reads `311.1`), so it is written as the colour bar writes
/// it. Where those figures would put it level with or past `given`, the value
/// typed, it takes more until it reads on its own side of it; one equal to
/// `given` is written as `given` is.
fn data_end(data: f64, given: f64) -> String {
    // `given`, not `data`: the two compare equal across a signed zero, and a
    // data end of -0 beside a typed 0 is written `0`, as the 0 was.
    if data == given {
        return format!("{given}");
    }
    let side = data.partial_cmp(&given);
    // Seventeen figures always read back as the same `f64`, so the loop
    // returns by its last pass.
    for digits in colour_bar_figures(data)..=17 {
        let text = to_precision(data, digits);
        if text.parse::<f64>().ok().and_then(|v| v.partial_cmp(&given)) == side {
            return text;
        }
    }
    format!("{data}")
}

/// `v` to `digits` significant figures (1 to 21), as JavaScript's
/// `Number.prototype.toPrecision` writes it (ECMA-262 §21.1.3.5): exponent
/// form when the exponent is below −6 or not below `digits`, and a tie
/// rounded away from zero.
///
/// A value that is not finite is written as Rust writes it (`inf`, `-inf`,
/// `NaN`). `colour_range` documents its `extremes` as finite but cannot hold a
/// caller to that, and an infinite data end reached a refusal's text before
/// this rounding existed; it must not become a panic.
fn to_precision(v: f64, digits: usize) -> String {
    debug_assert!((1..=21).contains(&digits));
    if !v.is_finite() {
        return format!("{v}");
    }
    if v == 0.0 {
        // Negative zero too, as JavaScript writes it.
        return if digits == 1 {
            "0".to_string()
        } else {
            format!("0.{}", "0".repeat(digits - 1))
        };
    }
    let sign = if v < 0.0 { "-" } else { "" };
    // Every digit of the value, exactly: an `f64` has at most 767 significant
    // decimal digits, so 800 after the point leaves nothing rounded by the
    // formatter. The rounding to `digits` is then this function's, half up.
    let exact = format!("{:.800e}", v.abs());
    let (mantissa, exponent) = exact.split_once('e').unwrap_or((&exact, "0"));
    let mut e: i32 = exponent.parse().unwrap_or(0);
    let all: Vec<u8> = mantissa
        .bytes()
        .filter(u8::is_ascii_digit)
        .map(|b| b - b'0')
        .collect();
    let mut kept = all[..digits].to_vec();
    if all[digits] >= 5 {
        let mut i = digits;
        loop {
            if i == 0 {
                // 9.99… rounded up to 10.0…: one more power of ten.
                kept.insert(0, 1);
                kept.pop();
                e += 1;
                break;
            }
            i -= 1;
            if kept[i] == 9 {
                kept[i] = 0;
            } else {
                kept[i] += 1;
                break;
            }
        }
    }
    let figures: String = kept.iter().map(|d| char::from(b'0' + d)).collect();
    // `digits` is at most 21, so it fits an `i32`.
    let p = digits as i32;
    if e < -6 || e >= p {
        let (lead, rest) = figures.split_at(1);
        let point = if rest.is_empty() { "" } else { "." };
        let exp_sign = if e < 0 { '-' } else { '+' };
        return format!("{sign}{lead}{point}{rest}e{exp_sign}{}", e.unsigned_abs());
    }
    if e >= 0 {
        // `e` is in `0..p` here, so the split is inside `figures`.
        let (int, frac) = figures.split_at(e as usize + 1);
        let point = if frac.is_empty() { "" } else { "." };
        return format!("{sign}{int}{point}{frac}");
    }
    // `e` is in `-6..0`: leading zeros after the point.
    format!("{sign}0.{}{figures}", "0".repeat((-e - 1) as usize))
}

impl ColourRange {
    /// The range as a pair, or `None` when there is none to paint over.
    pub fn used(&self) -> Option<(f64, f64)> {
        self.used_min.zip(self.used_max)
    }

    /// The domain a palette is built over: [`used`](Self::used), or a
    /// placeholder when there is no range.
    ///
    /// With no range every cell is masked and paints transparent, so the
    /// placeholder is never read for a colour; `1..10` is positive, so it is a
    /// valid domain under log10 too.
    pub fn paint_domain(&self) -> (f64, f64) {
        self.used().unwrap_or((1.0, 10.0))
    }

    /// This range with the counts the painter took while colouring a raster of
    /// `total` cells.
    #[cfg(feature = "render")]
    #[must_use]
    pub fn counted(mut self, counts: &fieldglass_core::colormap::PaintCounts, total: u64) -> Self {
        self.below = counts.below;
        self.above = counts.above;
        self.present = counts.present;
        self.total = total;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: Option<(f64, f64)> = Some((53502.3, 99919.6));

    fn refusal(r: Result<ColourRange, Error>) -> (RangeRefusal, String) {
        let e = r.expect_err("refused");
        assert_eq!(e.code(), "invalid_option");
        (
            e.range_refusal().expect("a range refusal").clone(),
            e.message(),
        )
    }

    #[test]
    fn an_empty_field_has_no_range_and_no_error() {
        let r = colour_range(None, None, None, None, false).unwrap();
        assert_eq!((r.used_min, r.used_max), (None, None));
        assert_eq!(r.range_source, RangeSource::Auto);
        assert_eq!(r.paint_domain(), (1.0, 10.0));
        // Under log10 too: there is no minimum to refuse.
        assert!(
            colour_range(None, None, None, None, true)
                .unwrap()
                .used()
                .is_none()
        );
    }

    #[test]
    fn one_manual_end_takes_the_other_from_the_data() {
        let r = colour_range(DATA, Some(76711.0), None, None, false).unwrap();
        assert_eq!(r.used(), Some((76711.0, 99919.6)));
        assert_eq!(r.range_source, RangeSource::ManualMin);
        let r = colour_range(DATA, None, Some(60000.0), None, false).unwrap();
        assert_eq!(r.used(), Some((53502.3, 60000.0)));
        assert_eq!(r.range_source, RangeSource::ManualMax);
    }

    #[test]
    fn one_manual_end_past_the_data_says_where_the_other_came_from() {
        let (r, msg) = refusal(colour_range(DATA, Some(100000.0), None, None, false));
        assert_eq!(
            msg,
            "minimum 100000 is above the field's maximum 99920 (taken from the data); set a \
             maximum too"
        );
        assert_eq!(
            (r.reason, r.end, r.given),
            (RangeRefusalReason::Inverted, RangeEnd::Min, Some(100000.0))
        );
        assert_eq!((r.field_min, r.field_max), (Some(53502.3), Some(99919.6)));
        let (r, _) = refusal(colour_range(DATA, None, Some(53502.3), None, false));
        assert_eq!(
            (r.reason, r.end),
            (RangeRefusalReason::Inverted, RangeEnd::Max)
        );
    }

    #[test]
    fn an_inverted_or_zero_width_manual_range_is_refused_naming_both() {
        for (lo, hi) in [(5.0, 3.0), (4.0, 4.0)] {
            let (r, msg) = refusal(colour_range(DATA, Some(lo), Some(hi), None, false));
            assert_eq!(
                (r.reason, r.end, r.given),
                (RangeRefusalReason::Inverted, RangeEnd::Both, None)
            );
            assert!(
                msg.contains(&format!("minimum {lo}")) && msg.contains(&format!("maximum {hi}")),
                "{msg}"
            );
        }
        // A constant field's automatic range is zero-width and still paints.
        let r = colour_range(Some((7.0, 7.0)), None, None, None, false).unwrap();
        assert_eq!(r.used(), Some((7.0, 7.0)));
    }

    #[test]
    fn a_non_finite_end_is_refused_on_both_scales() {
        for log10 in [false, true] {
            for (min, max, end) in [
                (Some(f64::NAN), None, RangeEnd::Min),
                (None, Some(f64::NAN), RangeEnd::Max),
                (Some(f64::NEG_INFINITY), None, RangeEnd::Min),
                (None, Some(f64::INFINITY), RangeEnd::Max),
            ] {
                let (r, _) = refusal(colour_range(DATA, min, max, None, log10));
                assert_eq!(
                    (r.reason, r.end, r.given),
                    (RangeRefusalReason::NotFinite, end, None)
                );
            }
            let (r, _) = refusal(colour_range(DATA, None, None, Some(f64::NAN), log10));
            assert_eq!(
                (r.reason, r.end),
                (RangeRefusalReason::NotFinite, RangeEnd::Center)
            );
        }
    }

    #[test]
    fn a_nan_end_says_it_is_not_a_number_and_an_infinite_one_keeps_its_value() {
        let (_, msg) = refusal(colour_range(DATA, None, Some(f64::NAN), None, false));
        assert_eq!(msg, "the maximum is not a number; set a number or clear it");
        let (_, msg) = refusal(colour_range(DATA, Some(f64::INFINITY), None, None, false));
        assert_eq!(
            msg,
            "the minimum inf is not a finite number; set a finite minimum or clear it"
        );
    }

    #[test]
    fn log10_needs_a_positive_minimum_given_or_taken() {
        let (r, msg) = refusal(colour_range(DATA, Some(0.0), None, None, true));
        assert_eq!(
            (r.reason, r.end, r.given),
            (
                RangeRefusalReason::Log10NotPositive,
                RangeEnd::Min,
                Some(0.0)
            )
        );
        assert!(msg.contains("set a minimum above 0"), "{msg}");
        let (r, msg) = refusal(colour_range(Some((-3.0, 9.0)), None, None, None, true));
        // Taken from the data, so nothing was given: the value is the field's own.
        assert_eq!(
            (r.end, r.given, r.field_min),
            (RangeEnd::Min, None, Some(-3.0))
        );
        let (r, _) = refusal(colour_range(Some((-3.0, 9.0)), None, Some(5.0), None, true));
        assert_eq!(
            (r.end, r.given, r.field_min),
            (RangeEnd::Min, None, Some(-3.0))
        );
        assert!(msg.contains("-3.000 (taken from the data)"), "{msg}");
        // A positive manual minimum over data that dips below zero paints.
        assert!(colour_range(Some((-3.0, 9.0)), Some(1.0), None, None, true).is_ok());
    }

    #[test]
    fn a_center_makes_the_range_symmetric() {
        let r = colour_range(Some((-2.0, 6.0)), None, None, Some(0.0), false).unwrap();
        assert_eq!(r.used(), Some((-6.0, 6.0)));
        assert_eq!(r.center, Some(0.0));
        let r = colour_range(Some((-2.0, 6.0)), Some(-1.0), None, Some(0.0), false).unwrap();
        assert_eq!(r.used(), Some((-1.0, 1.0)));
        // Symmetric in log10: 1 is a decade below 10, so the top is a decade above.
        let r = colour_range(Some((1.0, 50.0)), None, None, Some(10.0), true).unwrap();
        let (lo, hi) = r.used().unwrap();
        assert!(
            (lo - 1.0).abs() < 1e-12 && (hi - 100.0).abs() < 1e-9,
            "{lo} {hi}"
        );
        let (r, _) = refusal(colour_range(DATA, Some(1.0), Some(2.0), Some(1.5), false));
        assert_eq!(
            (r.reason, r.end),
            (RangeRefusalReason::CenterWithBothEnds, RangeEnd::Center)
        );
        let (r, _) = refusal(colour_range(DATA, Some(5.0), None, Some(4.0), false));
        assert_eq!(
            (r.reason, r.end, r.given),
            (
                RangeRefusalReason::CenterOutside,
                RangeEnd::Center,
                Some(4.0)
            )
        );
        // A center so far out that mirroring overflows is refused rather than
        // handed on as an infinite end.
        let (r, _) = refusal(colour_range(DATA, Some(-1e308), None, Some(1e308), false));
        assert_eq!(
            (r.reason, r.end),
            (RangeRefusalReason::NotFinite, RangeEnd::Center)
        );
        // Under log10 the mirror `c² / max` can underflow to 0, a low end with
        // no logarithm: refused, not handed on as a `-inf` domain.
        let (r, msg) = refusal(colour_range(DATA, None, Some(1e200), Some(1e-200), true));
        assert_eq!(
            (r.reason, r.end, r.given),
            (
                RangeRefusalReason::NotFinite,
                RangeEnd::Center,
                Some(1e-200)
            )
        );
        assert!(msg.contains("smallest number"), "{msg}");
        let (r, _) = refusal(colour_range(DATA, None, None, Some(-1.0), true));
        assert_eq!(
            (r.reason, r.end),
            (RangeRefusalReason::Log10NotPositive, RangeEnd::Center)
        );
    }

    #[test]
    fn the_refusal_survives_the_wire() {
        let e = colour_range(DATA, Some(5.0), Some(3.0), None, false).unwrap_err();
        let json = serde_json::to_value(&e).unwrap();
        assert_eq!(json["code"], "invalid_option");
        assert_eq!(json["rangeRefusal"]["reason"], "inverted");
        assert_eq!(json["rangeRefusal"]["end"], "both");
        assert_eq!(json["rangeRefusal"]["fieldMax"], 99919.6);
        let back: Error = serde_json::from_value(json).unwrap();
        assert_eq!(back, e);
        // An error from before the field existed still reads.
        let old: Error = serde_json::from_str(r#"{"code":"invalid_option","detail":"x"}"#).unwrap();
        assert_eq!(old, Error::invalid_option("x"));
    }

    /// The colour bar writes each end with JavaScript's `toPrecision(4)`, and a
    /// refusal names an end taken from the data the same way (#1053): the
    /// field's maximum read `311.0986328125` beside a bar that said `311.1`.
    #[test]
    fn an_end_taken_from_the_data_is_written_as_the_colour_bar_writes_it() {
        let hi = 311.098_632_812_5;
        let (_, msg) = refusal(colour_range(
            Some((270.0, hi)),
            Some(400.0),
            None,
            None,
            false,
        ));
        assert_eq!(
            msg,
            "minimum 400 is above the field's maximum 311.1 (taken from the data); set a \
             maximum too"
        );
        let (_, msg) = refusal(colour_range(
            Some((270.123_456, hi)),
            None,
            Some(200.0),
            None,
            false,
        ));
        assert_eq!(
            msg,
            "maximum 200 is below the field's minimum 270.1 (taken from the data); set a \
             minimum too"
        );
    }

    /// Below 2^53 an end keeps its whole part, as the colour bar now writes
    /// it (#1432): `1972320` hours, not `1.972e+6`, and a negative end keeps
    /// its sign. So does a time in seconds or milliseconds since 1970, and a
    /// fraction of a billion or more takes one decimal besides (#1450). Four
    /// figures stand where they already hold the whole part, and from 2^53
    /// up the exponent form stays.
    #[test]
    fn an_end_taken_from_the_data_keeps_its_whole_part() {
        for (v, figures) in [
            (311.098_632_812_5, 4),
            (-30.0, 4),
            (1013.25, 4),
            (0.000_123, 4),
            (99919.6, 5),
            (99_999.7, 6),
            (1_972_320.0, 7),
            (-1_963_536.0, 7),
            (999_999_999.4, 9),
            (1e9, 10),
            (1_700_000_000.0, 10),
            (1_700_003_600.0, 10),
            (1.7e12, 13),
            (-1.7e12, 13),
            (9_007_199_254_740_991.0, 16),
            (9_007_199_254_740_992.0, 4),
            (1_700_000_000.5, 11),
            (-1_700_000_000.25, 11),
            (1_066_953_600.123, 11),
            (999_999_999.5, 10),
            (4_503_599_627_370_495.5, 17),
            (1.7e16, 4),
            (f64::INFINITY, 4),
            (f64::NAN, 4),
        ] {
            assert_eq!(colour_bar_figures(v), figures, "{v}");
        }
        let (_, msg) = refusal(colour_range(
            Some((1_963_536.0, 1_972_320.0)),
            Some(2_000_000.0),
            None,
            None,
            false,
        ));
        assert!(
            msg.starts_with("minimum 2000000 is above the field's maximum 1972320 "),
            "{msg}"
        );
        let (_, msg) = refusal(colour_range(
            Some((-1_963_536.0, 5.0)),
            None,
            None,
            None,
            true,
        ));
        assert!(
            msg.contains("the range starts at -1963536 (taken from the data)"),
            "{msg}"
        );
        // Time bounds an hour apart in seconds since 1970.
        let (_, msg) = refusal(colour_range(
            Some((1_700_000_000.0, 1_700_003_600.0)),
            Some(1_800_000_000.0),
            None,
            None,
            false,
        ));
        assert!(
            msg.starts_with("minimum 1800000000 is above the field's maximum 1700003600 "),
            "{msg}"
        );
        // Times half a second apart, with fractions: the bar reads the ends
        // `1700000000.6` and `1700000000.9`, and so does the note (#1450).
        let (_, msg) = refusal(colour_range(
            Some((1_700_000_000.6, 1_700_000_000.9)),
            Some(1_800_000_000.0),
            None,
            None,
            false,
        ));
        assert!(
            msg.starts_with("minimum 1800000000 is above the field's maximum 1700000000.9 "),
            "{msg}"
        );
        let (_, msg) = refusal(colour_range(
            Some((1_700_000_000.6, 1_700_000_000.9)),
            None,
            Some(1_600_000_000.0),
            None,
            false,
        ));
        assert!(
            msg.starts_with("maximum 1600000000 is below the field's minimum 1700000000.6 "),
            "{msg}"
        );
    }

    /// Four figures that would put the data's end on the other side of the
    /// value typed are widened until they do not: `311.1` is not below `311.1`.
    #[test]
    fn an_end_taken_from_the_data_keeps_its_side_of_the_value_given() {
        let hi = 311.098_632_812_5;
        let (_, msg) = refusal(colour_range(
            Some((270.0, hi)),
            Some(311.1),
            None,
            None,
            false,
        ));
        assert!(
            msg.starts_with("minimum 311.1 is above the field's maximum 311.099 "),
            "{msg}"
        );
        // A data end of -0 equals a typed 0, and is written as the 0 was.
        let (_, msg) = refusal(colour_range(
            Some((-5.0, -0.0)),
            Some(0.0),
            None,
            None,
            false,
        ));
        assert!(
            msg.starts_with("minimum 0 is not below the field's maximum 0 "),
            "{msg}"
        );
        // Equal to the value typed: written as that value is.
        let (_, msg) = refusal(colour_range(Some((270.0, hi)), Some(hi), None, None, false));
        assert!(
            msg.starts_with(
                "minimum 311.0986328125 is not below the field's maximum 311.0986328125 "
            ),
            "{msg}"
        );
        let (_, msg) = refusal(colour_range(
            Some((0.123_449, 1.0)),
            None,
            Some(0.1234),
            None,
            false,
        ));
        assert!(
            msg.starts_with("maximum 0.1234 is below the field's minimum 0.12345 "),
            "{msg}"
        );
    }

    /// `to_precision` against what `Number.prototype.toPrecision` returns for
    /// the same `f64` (each value read from Node 22).
    #[test]
    fn to_precision_writes_what_javascript_writes() {
        for (v, digits, js) in [
            (311.098_632_812_5, 4, "311.1"),
            (99919.6, 4, "9.992e+4"),
            (1000.0, 4, "1000"),
            (10000.0, 4, "1.000e+4"),
            (9999.96, 4, "1.000e+4"),
            (0.0, 4, "0.000"),
            (-0.0, 4, "0.000"),
            (-3.0, 4, "-3.000"),
            (0.000_123_456, 4, "0.0001235"),
            (0.000_000_123_456, 4, "1.235e-7"),
            (0.000_001, 4, "0.000001000"),
            (1.0, 1, "1"),
            (25.0, 1, "3e+1"),
            // A tie in binary is a tie in decimal: JavaScript rounds it up,
            // where Rust's own formatting would round it to even.
            (0.125, 2, "0.13"),
            (-0.125, 2, "-0.13"),
            (311.25, 4, "311.3"),
            (f64::MAX, 4, "1.798e+308"),
            (f64::MIN_POSITIVE, 4, "2.225e-308"),
            (5e-324, 4, "4.941e-324"),
            (0.1, 17, "0.10000000000000001"),
            // At the figures `colour_bar_figures` gives (#1432).
            (99919.6, 5, "99920"),
            (99_999.7, 6, "99999.7"),
            (9999.96, 5, "10000"),
            (1_963_536.5, 7, "1963537"),
            (999_999_999.6, 10, "999999999.6"),
            (1_700_000_000.0, 10, "1700000000"),
            (1_700_003_600.0, 10, "1700003600"),
            (1.7e12, 13, "1700000000000"),
            (-1.7e12, 13, "-1700000000000"),
            (9_007_199_254_740_991.0, 16, "9007199254740991"),
            // A fraction past a billion keeps its whole part and one decimal
            // (#1450): times in seconds since 1970, in half-second pairs
            // across a rounding boundary, and in seconds since 1993 (TAI93).
            // A whole value there keeps no decimal.
            (1_700_000_000.25, 11, "1700000000.3"),
            (1_700_000_000.75, 11, "1700000000.8"),
            (1_700_000_000.0, 10, "1700000000"),
            (1_700_000_000.5, 11, "1700000000.5"),
            (1_700_000_001.0, 10, "1700000001"),
            (1_700_000_000.6, 11, "1700000000.6"),
            (1_700_000_001.1, 11, "1700000001.1"),
            (1_700_000_000.9, 11, "1700000000.9"),
            (-1_700_000_000.5, 11, "-1700000000.5"),
            (1_066_953_600.123, 11, "1066953600.1"),
            (9_999_999_999.5, 12, "9999999999.50"),
            (4_503_599_627_370_495.5, 17, "4503599627370495.5"),
        ] {
            assert_eq!(to_precision(v, digits), js, "{v} to {digits}");
        }
    }

    /// `extremes` is documented finite, not checked. An infinite data end
    /// is named as it was before the rounding, not a panic (#1053 review).
    #[test]
    fn a_non_finite_data_end_is_named_not_a_panic() {
        let (_, msg) = refusal(colour_range(
            Some((f64::NEG_INFINITY, 9.0)),
            None,
            None,
            None,
            true,
        ));
        assert!(
            msg.contains("starts at -inf (taken from the data)"),
            "{msg}"
        );
        let (_, msg) = refusal(colour_range(
            Some((f64::NEG_INFINITY, f64::NEG_INFINITY)),
            Some(0.0),
            None,
            None,
            false,
        ));
        assert!(
            msg.starts_with("minimum 0 is above the field's maximum -inf "),
            "{msg}"
        );
        for v in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert_eq!(to_precision(v, 4), format!("{v}"));
        }
    }
}
