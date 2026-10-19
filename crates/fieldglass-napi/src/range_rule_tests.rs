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

fn gfs() -> Grib2File {
    Grib2File::from_vec(GFS.to_vec()).expect("GFS opens")
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
fn field_range(h: &Grib2File) -> (f64, f64) {
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

/// The automatic contour levels for GFS message 0 over its own range, the list
/// `Session::contours` pins in `crates/fieldglass/tests/colour_range_rules.rs`
/// and the browser module's in `colour_range.mjs`. Pinned here too, so the
/// three hosts are held to one list rather than each to "the levels moved"
/// (#1156).
const AUTO_LEVELS: [f64; 9] = [
    55000.0, 60000.0, 65000.0, 70000.0, 75000.0, 80000.0, 85000.0, 90000.0, 95000.0,
];

/// The automatic levels over a manual 60000..90000, as those tests pin them.
const MANUAL_LEVELS: [f64; 5] = [65000.0, 70000.0, 75000.0, 80000.0, 85000.0];

/// The runs `projectContours` draws at exactly `levels`, one level at a time.
///
/// This host hands back projected runs, not the levels they trace, so a level
/// list is read through the handle by drawing each level alone: a range just
/// around it, with an interval that puts one level inside that range and none
/// at its ends, then the runs of each in order. Equal runs mean the same
/// levels, since a level's isolines depend on that level and the field alone.
fn runs_at(h: &Grib2File, levels: &[f64]) -> (Vec<f64>, Vec<u32>) {
    let (mut xy, mut lengths) = (Vec::new(), Vec::new());
    for &level in levels {
        let one = h
            .project_contours(
                0,
                with_range(Some(level - 1.0), Some(level + 1.0)),
                Some(5000.0),
            )
            .expect("contours");
        assert!(!one.seg_lengths.is_empty(), "level {level} has isolines");
        xy.extend_from_slice(&one.xy);
        lengths.extend_from_slice(&one.seg_lengths);
    }
    (xy, lengths)
}

/// A manual range spaces the automatic contour levels here as `Session`'s
/// contours do (#1155): the same level lists, automatic and over a manual
/// range (#1156).
#[test]
fn a_manual_range_spaces_the_automatic_contours() {
    let h = gfs();
    for (label, options, levels) in [
        ("auto", opts("source"), &AUTO_LEVELS[..]),
        (
            "manual",
            with_range(Some(60000.0), Some(90000.0)),
            &MANUAL_LEVELS[..],
        ),
    ] {
        let drawn = h.project_contours(0, options, None).expect("contours");
        let (xy, lengths) = runs_at(&h, levels);
        assert_eq!(drawn.seg_lengths.as_ref(), &lengths[..], "{label}");
        assert_eq!(drawn.xy.as_ref(), &xy[..], "{label}");
    }
}

/// A range the render refuses, every contour call refuses in the render's
/// fields: the API's `code` and `RangeRefusal`, not prose alone (#1156). One
/// call per handle and per combined variant, each of which used to drop them.
#[test]
fn contours_refuse_a_range_in_the_renders_fields() {
    let inverted = || with_range(Some(90000.0), Some(60000.0));
    let check = |what: &str, r: Result<ProjectedOverlay, RenderError>| {
        let Err(e) = r else {
            panic!("{what}: the range was not refused");
        };
        let api = e.api().unwrap_or_else(|| panic!("{what}: {}", e.reason()));
        assert_eq!(api.code(), "invalid_option", "{what}");
        let refusal = api.range_refusal().expect("a colour-range refusal");
        assert_eq!(
            (refusal.reason, refusal.end),
            (RangeRefusalReason::Inverted, RangeEnd::Both),
            "{what}"
        );
    };

    let g2 = gfs();
    // The render refuses it in the same fields.
    let (_, from_render) = refused(g2.render_grid(0, inverted()));
    let Err(e) = g2.project_contours(0, inverted(), None) else {
        panic!("the contours drew");
    };
    assert_eq!(
        e.api().and_then(fieldglass::Error::range_refusal),
        Some(&from_render)
    );
    check("grib2", g2.project_contours(0, inverted(), None));
    check(
        "grib2 combined",
        g2.project_contours_combined(0, 0, "a_plus_b".to_string(), inverted(), None),
    );

    let g1 = Grib1File::from_vec(
        include_bytes!("../../fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1").to_vec(),
    )
    .expect("GRIB1 opens");
    let refuse_g1 = || with_range(Some(1e9), Some(-1e9));
    check("grib1", g1.project_contours(0, refuse_g1(), None));
    check(
        "grib1 combined",
        g1.project_contours_combined(0, 0, "a_plus_b".to_string(), refuse_g1(), None),
    );

    let nc = NetcdfFile::from_vec(
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc").to_vec(),
    )
    .expect("NetCDF opens");
    let var = nc
        .variables()
        .into_iter()
        .find(|v| v.name == "sst")
        .expect("sst");
    let (v, y, x) = (
        var.variable_index as u32,
        var.detected_y_dim.expect("a y axis") as u32,
        var.detected_x_dim.expect("an x axis") as u32,
    );
    let at = vec![0; var.dims.len()];
    check(
        "netcdf",
        nc.project_contours(v, y, x, at.clone(), refuse_g1(), None),
    );
    check(
        "netcdf combined",
        nc.project_contours_slice_combined(
            v,
            y,
            x,
            at.clone(),
            v,
            at,
            "a_plus_b".to_string(),
            refuse_g1(),
            None,
        ),
    );

    let zarr =
        ZarrStore::from_directory("../fieldglass-zarr/tests/fixtures/stores/cf_v3".to_string())
            .expect("the store opens");
    let t = zarr
        .variables()
        .into_iter()
        .find(|v| v.name == "t")
        .expect("t");
    let (v, y, x) = (
        t.variable_index as u32,
        t.detected_y_dim.expect("a y axis") as u32,
        t.detected_x_dim.expect("an x axis") as u32,
    );
    let at = vec![0; t.dims.len()];
    check(
        "zarr",
        zarr.project_contours(v, y, x, at.clone(), refuse_g1(), None),
    );
    check(
        "zarr combined",
        zarr.project_contours_slice_combined(
            v,
            y,
            x,
            at.clone(),
            v,
            at,
            "a_plus_b".to_string(),
            refuse_g1(),
            None,
        ),
    );
}
