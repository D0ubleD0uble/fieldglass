//! The colour-range rule through this host's real render path (#1155).
//!
//! `fieldglass::colour_range` states the rule and tests it on numbers; this
//! checks that the handle the extension calls follows it, over the committed
//! GFS fixture #880 measured (message 0, stats 53502.3..99919.6). The browser
//! host's half is `crates/fieldglass-wasm/tests/node/colour_range.mjs`, over
//! the same fixture.

use super::*;
use crate::netcdf_slice_tests::opts;
use fieldglass::{RangeEnd, RangeRefusalReason, RangeSource};

const GFS: &[u8] = include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");

fn gfs() -> Grib2Handle {
    Grib2Handle::from_bytes(GFS.to_vec().into()).expect("GFS opens")
}

fn with_range(min: Option<f64>, max: Option<f64>) -> RenderOptions {
    RenderOptions {
        range_min: min,
        range_max: max,
        ..opts("source")
    }
}

fn log10(min: Option<f64>, max: Option<f64>) -> RenderOptions {
    RenderOptions {
        scale_mode: Some("log10".to_string()),
        ..with_range(min, max)
    }
}

/// The refusal a render failed with: its prose and its fields.
fn refused(r: Result<RenderedGrid, RenderError>) -> (String, fieldglass::RangeRefusal) {
    let Err(e) = r else {
        panic!("the range was not refused");
    };
    let api = e.api().expect("an engine refusal, not a host one");
    assert_eq!(api.code(), "invalid_option");
    let refusal = api.range_refusal().expect("a colour-range refusal").clone();
    (e.reason(), refusal)
}

/// The field's own present-cell range, as the decoded values hold it.
fn field_range(h: &Grib2Handle) -> (f64, f64) {
    let (raw, _) = h.resolved(0).expect("message 0");
    fieldglass::min_max_ignoring_mask(raw.iter().copied()).expect("present cells")
}

fn source_of(r: &RenderedGrid) -> RangeSource {
    serde_json::from_value(r.range["rangeSource"].clone()).expect("a range source")
}

/// Rule 2: one manual end is used and the other comes from the data; past the
/// data's other end, the refusal says where that end came from.
#[test]
fn one_manual_end_takes_the_other_from_the_data() {
    let h = gfs();
    let (lo, hi) = field_range(&h);
    let r = h
        .render_grid(0, with_range(Some(76711.0), None))
        .expect("renders");
    assert_eq!(r.used(), (76711.0, hi));
    assert_eq!(source_of(&r), RangeSource::ManualMin);
    let r = h
        .render_grid(0, with_range(None, Some(60000.0)))
        .expect("renders");
    assert_eq!(r.used(), (lo, 60000.0));
    assert_eq!(source_of(&r), RangeSource::ManualMax);

    let (message, refusal) = refused(h.render_grid(0, with_range(Some(100000.0), None)));
    assert_eq!(
        message,
        format!(
            "minimum 100000 is above the field's maximum {hi} (taken from the data); set a \
             maximum too"
        )
    );
    assert_eq!(
        (refusal.reason, refusal.end, refusal.given),
        (RangeRefusalReason::Inverted, RangeEnd::Min, Some(100000.0))
    );
    assert_eq!((refusal.field_min, refusal.field_max), (Some(lo), Some(hi)));
}

/// Rule 3: an inverted or zero-width manual range is refused, naming both; a
/// constant field's automatic `min == max` still paints.
#[test]
fn an_inverted_or_zero_width_manual_range_is_refused() {
    let h = gfs();
    for (lo, hi) in [(80000.0, 60000.0), (70000.0, 70000.0)] {
        let (message, refusal) = refused(h.render_grid(0, with_range(Some(lo), Some(hi))));
        assert_eq!(
            (refusal.reason, refusal.end),
            (RangeRefusalReason::Inverted, RangeEnd::Both)
        );
        assert!(message.contains(&format!("minimum {lo}")), "{message}");
        assert!(message.contains(&format!("maximum {hi}")), "{message}");
    }

    let (raw, placed) = h.resolved(0).expect("message 0");
    let constant: Vec<Option<f64>> = raw.iter().map(|v| v.map(|_| 5.0)).collect();
    let r = render_from_source(&placed.source(), &constant, &opts("source")).expect("paints");
    assert_eq!(r.used(), (5.0, 5.0));
    assert!(r.rgba.as_chunks::<4>().0.iter().any(|px| px[3] == 255));
}

/// Rule 4: a non-finite manual end is refused on both scales, and log10
/// needs a positive minimum, given or taken from the data.
#[test]
fn a_non_finite_end_or_a_log10_floor_at_zero_is_refused() {
    let h = gfs();
    for scale in [with_range, log10] {
        for (min, max, end) in [
            (Some(f64::NAN), None, RangeEnd::Min),
            (None, Some(f64::NAN), RangeEnd::Max),
            (Some(f64::NEG_INFINITY), Some(1.0), RangeEnd::Min),
            (Some(1.0), Some(f64::INFINITY), RangeEnd::Max),
        ] {
            let (_, refusal) = refused(h.render_grid(0, scale(min, max)));
            assert_eq!(
                (refusal.reason, refusal.end),
                (RangeRefusalReason::NotFinite, end)
            );
            assert_eq!(
                refusal.given, None,
                "JSON has no spelling for {min:?} {max:?}"
            );
        }
    }
    let (message, refusal) = refused(h.render_grid(0, log10(Some(0.0), None)));
    assert_eq!(
        (refusal.reason, refusal.end, refusal.given),
        (
            RangeRefusalReason::Log10NotPositive,
            RangeEnd::Min,
            Some(0.0)
        )
    );
    assert!(message.contains("set a minimum above 0"), "{message}");
    // GFS is positive throughout, so its own floor takes a logarithm.
    assert!(h.render_grid(0, log10(None, None)).is_ok());
}

/// Rule 6: the automatic range is the whole field's, taken before the warp,
/// so neither a window, the output size nor the resampling moves it (#880's
/// table), and the automatic contour levels span the same range.
#[test]
fn the_range_does_not_follow_the_warp() {
    let h = gfs();
    let whole = field_range(&h);
    let window = |o: RenderOptions| RenderOptions {
        bounds_lat_min: Some(30.0),
        bounds_lat_max: Some(60.0),
        bounds_lon_min: Some(-10.0),
        bounds_lon_max: Some(30.0),
        ..o
    };
    let sized = |o: RenderOptions| RenderOptions {
        width: Some(64),
        height: Some(32),
        ..o
    };
    let bilinear = || RenderOptions {
        resampling: "bilinear".to_string(),
        ..opts("equirectangular")
    };
    for (label, o) in [
        ("default", opts("equirectangular")),
        ("bilinear", bilinear()),
        ("64×32", sized(opts("equirectangular"))),
        ("64×32 bilinear", sized(bilinear())),
        ("window", window(opts("equirectangular"))),
    ] {
        assert_eq!(
            h.render_grid(0, o).expect("renders").used(),
            whole,
            "{label}"
        );
    }

    // Contours under the window, automatic and with the whole field's range
    // stated, are the same runs: their levels span the whole field.
    let windowed = || window(opts("equirectangular"));
    let auto = h.project_contours(0, windowed(), None).expect("contours");
    let stated = h
        .project_contours(
            0,
            RenderOptions {
                range_min: Some(whole.0),
                range_max: Some(whole.1),
                ..windowed()
            },
            None,
        )
        .expect("contours");
    assert!(!auto.seg_lengths.is_empty());
    assert_eq!(auto.seg_lengths.as_ref(), stated.seg_lengths.as_ref());
    assert_eq!(auto.xy.as_ref(), stated.xy.as_ref());
}

/// The range result: clamping, the counts the painter took, and the totals,
/// checked against the cells themselves in the source projection, where a
/// pixel is a cell.
#[test]
fn the_range_result_counts_the_cells_it_clamps() {
    let h = gfs();
    let (raw, _) = h.resolved(0).expect("message 0");
    let (lo, hi) = (60000.0, 90000.0);
    let r = h
        .render_grid(0, with_range(Some(lo), Some(hi)))
        .expect("renders");
    let present: Vec<f64> = raw
        .iter()
        .flatten()
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    let below = present.iter().filter(|&&v| v < lo).count() as u64;
    let above = present.iter().filter(|&&v| v > hi).count() as u64;
    assert!(
        below > 0 && above > 0,
        "the range clamps cells at both ends"
    );
    let range = &r.range;
    assert_eq!(range["below"], below);
    assert_eq!(range["above"], above);
    assert_eq!(range["present"], present.len() as u64);
    assert_eq!(range["total"], raw.len() as u64);
    assert_eq!(range["outOfRange"], "clamp");
    assert_eq!(range["rangeSource"], "manual");
    assert_eq!(range["center"], serde_json::Value::Null);
    assert_eq!(
        (range["usedMin"].as_f64(), range["usedMax"].as_f64()),
        (Some(lo), Some(hi))
    );

    // Clamped, not dropped: every present cell is painted.
    let opaque = r
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] == 255)
        .count();
    assert_eq!(opaque, present.len());
}

/// A center makes the range symmetric about it, and is refused beside two
/// manual ends.
#[test]
fn a_center_centres_the_range() {
    let h = gfs();
    let (lo, hi) = field_range(&h);
    let c = 80000.0;
    let r = h
        .render_grid(
            0,
            RenderOptions {
                range_center: Some(c),
                ..opts("source")
            },
        )
        .expect("renders");
    let reach = (c - lo).max(hi - c);
    assert_eq!(r.used(), (c - reach, c + reach));
    assert_eq!(r.range["center"], c);
    let (_, refusal) = refused(h.render_grid(
        0,
        RenderOptions {
            range_center: Some(c),
            ..with_range(Some(70000.0), Some(90000.0))
        },
    ));
    assert_eq!(
        (refusal.reason, refusal.end),
        (RangeRefusalReason::CenterWithBothEnds, RangeEnd::Center)
    );
}
