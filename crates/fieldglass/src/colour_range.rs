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
    ColourRange, OutOfRange, RangeEnd, RangeRefusal, RangeRefusalReason, RangeSource,
};

/// How many levels contours aim for when the caller names neither levels nor
/// an interval. Both hosts space them over the same [`colour_range`], so the
/// count is one number too (#880 item 8: the browser's was 10 and the addon's
/// 8).
pub const AUTO_CONTOUR_LEVELS: usize = 8;

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
            return Err(refuse(
                RangeRefusalReason::NotFinite,
                end,
                Some(v),
                format!("the {name} {v} is not a finite number; set a finite {name} or clear it"),
            ));
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
                format!(
                    "a log10 scale needs a finite, positive minimum; the range starts at \
                     {lo} (taken from the data); set a minimum above 0"
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
                                "minimum {lo} {relation} the field's maximum {data_hi} (taken \
                                 from the data); set a maximum too"
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
                                "maximum {hi} {relation} the field's minimum {data_lo} (taken \
                                 from the data); set a minimum too"
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
            "minimum 100000 is above the field's maximum 99919.6 (taken from the data); set a \
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
        assert!(msg.contains("-3 (taken from the data)"), "{msg}");
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
}
