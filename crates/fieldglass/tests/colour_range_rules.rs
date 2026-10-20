//! The colour-range rule through `Session`, the path the browser host binds
//! (#1155).
//!
//! Over the committed GFS fixture #880 measured: message 0, a field whose
//! present cells run 53502.3..99919.6. The addon's half is
//! `fieldglass-napi`'s `range_rule_tests`, and the browser bundle's is
//! `crates/fieldglass-wasm/tests/node/colour_range.mjs`; both use this
//! fixture, so the three answer the same questions about the same field.

use fieldglass::{
    CombineOp, ContourOptions, DecodeOptions, Field, PaletteOptions, RangeEnd, RangeRefusal,
    RangeRefusalReason, RangeSource, Session, WarpOptions,
};

const GFS: &[u8] = include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");

fn gfs() -> (Session, Field) {
    let session = Session::open(GFS.to_vec()).expect("opens");
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    (session, field)
}

fn range(min: Option<f64>, max: Option<f64>) -> PaletteOptions {
    let mut o = PaletteOptions::default();
    o.min = min;
    o.max = max;
    o
}

fn log10(min: Option<f64>, max: Option<f64>) -> PaletteOptions {
    let mut o = range(min, max);
    o.scale = Some("log10".to_string());
    o
}

fn stats(field: &Field) -> (f64, f64) {
    (field.stats.min.expect("min"), field.stats.max.expect("max"))
}

fn refused(r: Result<impl std::fmt::Debug, fieldglass::Error>) -> (String, RangeRefusal) {
    let e = r.expect_err("refused");
    assert_eq!(e.code(), "invalid_option");
    let refusal = e.range_refusal().expect("a colour-range refusal").clone();
    (e.message(), refusal)
}

/// Rule 1: a field with no present cell and an automatic range paints
/// transparent, reports no range, and is not refused, by `render` or by
/// `palette`, on either scale.
#[test]
fn an_empty_field_paints_transparent_with_no_range() {
    let (session, field) = gfs();
    // Every cell divides by zero, so the ratio has no present cell.
    let zeros = session
        .combine(&field, &field, CombineOp::Difference)
        .expect("aligned");
    let empty = session
        .combine(&field, &zeros, CombineOp::Ratio)
        .expect("aligned");
    assert_eq!(empty.stats.valid_count, 0);
    for options in [range(None, None), log10(None, None)] {
        let raster = session.render(&empty, &options, false).expect("renders");
        assert!(raster.rgba.as_chunks::<4>().0.iter().all(|px| px[3] == 0));
        assert_eq!((raster.range.used_min, raster.range.used_max), (None, None));
        assert_eq!(raster.range.range_source, RangeSource::Auto);
        assert_eq!(raster.range.present, 0);
        assert_eq!(
            raster.range.total,
            u64::from(field.ni) * u64::from(field.nj)
        );
        session.palette(&empty, &options).expect("a palette");
    }
}

/// Rule 2: one manual end is used, the other comes from the data, and past
/// the data's other end the refusal says where that end came from.
#[test]
fn one_manual_end_takes_the_other_from_the_data() {
    let (session, field) = gfs();
    let (lo, hi) = stats(&field);
    let raster = session
        .render(&field, &range(Some(76711.0), None), false)
        .expect("renders");
    assert_eq!(raster.range.used(), Some((76711.0, hi)));
    assert_eq!(raster.range.range_source, RangeSource::ManualMin);
    let palette = session
        .palette(&field, &range(None, Some(60000.0)))
        .expect("palette");
    assert_eq!((palette.t0, palette.t1), (lo, 60000.0));

    let (message, refusal) = refused(session.render(&field, &range(Some(100000.0), None), false));
    // The data's end as the colour bar writes it, four figures (#1053).
    assert_eq!(hi, 99919.6);
    assert_eq!(
        message,
        "minimum 100000 is above the field's maximum 9.992e+4 (taken from the data); set a \
         maximum too"
    );
    assert_eq!(
        (refusal.reason, refusal.end, refusal.given),
        (RangeRefusalReason::Inverted, RangeEnd::Min, Some(100000.0))
    );
    assert_eq!((refusal.field_min, refusal.field_max), (Some(lo), Some(hi)));
}

/// Rule 3: a manual `max <= min` is refused, naming both, where #880 measured
/// the browser painting every cell the low end's colour.
#[test]
fn an_inverted_or_zero_width_manual_range_is_refused() {
    let (session, field) = gfs();
    for (lo, hi) in [(80000.0, 60000.0), (70000.0, 70000.0)] {
        let (message, refusal) = refused(session.palette(&field, &range(Some(lo), Some(hi))));
        assert_eq!(
            (refusal.reason, refusal.end, refusal.given),
            (RangeRefusalReason::Inverted, RangeEnd::Both, None)
        );
        assert!(message.contains(&format!("minimum {lo}")), "{message}");
        assert!(message.contains(&format!("maximum {hi}")), "{message}");
        refused(session.render(&field, &range(Some(lo), Some(hi)), false));
    }
}

/// Rule 4: a non-finite manual end is refused on both scales, where #880
/// measured `NaN` and `Infinity` reaching the palette; log10 needs a positive
/// minimum.
#[test]
fn a_non_finite_end_or_a_log10_floor_at_zero_is_refused() {
    let (session, field) = gfs();
    for scale in [range, log10] {
        for (min, max, end) in [
            (Some(f64::NAN), None, RangeEnd::Min),
            (None, Some(f64::NAN), RangeEnd::Max),
            (Some(f64::NEG_INFINITY), None, RangeEnd::Min),
            (None, Some(f64::INFINITY), RangeEnd::Max),
        ] {
            let (_, refusal) = refused(session.palette(&field, &scale(min, max)));
            assert_eq!(
                (refusal.reason, refusal.end, refusal.given),
                (RangeRefusalReason::NotFinite, end, None)
            );
        }
    }
    let (message, refusal) = refused(session.palette(&field, &log10(Some(-1.0), None)));
    assert_eq!(
        (refusal.reason, refusal.end),
        (RangeRefusalReason::Log10NotPositive, RangeEnd::Min)
    );
    assert!(message.contains("set a minimum above 0"), "{message}");
}

/// Rule 5: one parser, one message. A misspelt colormap or scale is refused
/// in the same words through the palette options as through the render
/// options, and carries no range refusal.
#[test]
fn both_option_types_word_a_misspelling_the_same_way() {
    let (session, field) = gfs();
    let mut palette = PaletteOptions::default();
    palette.colormap = Some("viridiss".to_string());
    let from_palette = session.palette(&field, &palette).unwrap_err();
    let mut render = fieldglass::RenderOptions::default();
    render.colormap = Some("viridiss".to_string());
    let from_render = fieldglass::ResolvedOptions::parse(&render).unwrap_err();
    assert_eq!(from_palette.message(), from_render.message());
    assert!(from_palette.range_refusal().is_none());

    let mut palette = PaletteOptions::default();
    palette.scale = Some("log2".to_string());
    let mut render = fieldglass::RenderOptions::default();
    render.scale_mode = Some("log2".to_string());
    assert_eq!(
        session.palette(&field, &palette).unwrap_err().message(),
        fieldglass::ResolvedOptions::parse(&render)
            .unwrap_err()
            .message()
    );
}

/// Rule 6: the automatic range is the whole field's, before any warp, so a
/// window does not move it; the automatic contour levels span the same range,
/// at the addon's level count.
#[test]
fn the_range_is_the_whole_fields_and_contours_share_it() {
    let (session, field) = gfs();
    let (lo, hi) = stats(&field);
    let mut window = WarpOptions::new(false);
    window.bounds = Some([30.0, 60.0, -10.0, 30.0]);
    let warped = session.warp(&field, &window).expect("warps");
    let in_window = fieldglass::min_max_ignoring_mask(
        warped
            .values
            .iter()
            .zip(&warped.mask)
            .map(|(&v, &m)| (m == 1).then_some(f64::from(v))),
    )
    .expect("present cells in the window");
    assert!(
        in_window.0 > lo && in_window.1 < hi,
        "the window is narrower"
    );
    let palette = session
        .palette(&field, &range(None, None))
        .expect("palette");
    assert_eq!((palette.t0, palette.t1), (lo, hi));

    let auto = session
        .contours(&field, &[], &ContourOptions::default())
        .expect("contours");
    let levels: Vec<f64> = auto.iter().map(|l| l.value).collect();
    assert_eq!(levels, AUTO_LEVELS, "nice levels over {lo}..{hi}");
}

/// A manual range spaces the automatic contour levels, as it does in the
/// addon (#1155), and a range the render refuses, contours refuse alike.
#[test]
fn a_manual_range_spaces_the_automatic_contours() {
    let (session, field) = gfs();
    let mut options = ContourOptions::default();
    options.min = Some(60000.0);
    options.max = Some(90000.0);
    let levels: Vec<f64> = session
        .contours(&field, &[], &options)
        .expect("contours")
        .iter()
        .map(|l| l.value)
        .collect();
    assert_eq!(levels, MANUAL_LEVELS);

    options.max = Some(50000.0);
    let e = session.contours(&field, &[], &options).unwrap_err();
    let refusal = e.range_refusal().expect("a colour-range refusal");
    assert_eq!(
        (refusal.reason, refusal.end),
        (RangeRefusalReason::Inverted, RangeEnd::Both)
    );
}

/// The automatic levels over a manual 60000..90000; the wasm node test pins
/// the same list.
const MANUAL_LEVELS: [f64; 5] = [65000.0, 70000.0, 75000.0, 80000.0, 85000.0];

/// The automatic contour levels for GFS message 0, `nice_levels` over the
/// field's own range at [`fieldglass::AUTO_CONTOUR_LEVELS`]. The wasm node test
/// pins the same list, so the two runners agree on it.
const AUTO_LEVELS: [f64; 9] = [
    55000.0, 60000.0, 65000.0, 70000.0, 75000.0, 80000.0, 85000.0, 90000.0, 95000.0,
];

/// The range result: clamping and the counts the painter took, against the
/// cells themselves.
#[test]
fn the_range_result_counts_the_cells_it_clamps() {
    let (session, field) = gfs();
    let (lo, hi) = (60000.0, 90000.0);
    let raster = session
        .render(&field, &range(Some(lo), Some(hi)), false)
        .expect("renders");
    let values = field.values.to_f64();
    let present: Vec<f64> = values
        .iter()
        .zip(&field.mask)
        .filter(|&(_, &m)| m == 1)
        .map(|(&v, _)| v)
        .collect();
    let r = &raster.range;
    assert_eq!(r.below, present.iter().filter(|&&v| v < lo).count() as u64);
    assert_eq!(r.above, present.iter().filter(|&&v| v > hi).count() as u64);
    assert!(r.below > 0 && r.above > 0);
    assert_eq!(r.present, present.len() as u64);
    assert_eq!(r.total, values.len() as u64);
    assert_eq!(r.range_source, RangeSource::Manual);
    assert_eq!(r.out_of_range, fieldglass::OutOfRange::Clamp);
    let opaque = raster
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] == 255)
        .count();
    assert_eq!(opaque, present.len(), "clamped, not dropped");
}

/// A center makes the range symmetric about it.
#[test]
fn a_center_centres_the_range() {
    let (session, field) = gfs();
    let (lo, hi) = stats(&field);
    let mut o = range(None, None);
    o.center = Some(80000.0);
    let raster = session.render(&field, &o, false).expect("renders");
    let reach = (80000.0 - lo).max(hi - 80000.0);
    assert_eq!(
        raster.range.used(),
        Some((80000.0 - reach, 80000.0 + reach))
    );
    assert_eq!(raster.range.center, Some(80000.0));
}

/// Rule 7: a palette reports the range it spans, the one `render` paints over
/// for the same options, with the counts taken over the same cells (#1156). A
/// GPU host colours from the palette, so this is its legend's range.
#[test]
fn a_palette_reports_the_range_render_paints_over() {
    let (session, field) = gfs();
    let (lo, hi) = stats(&field);
    let mut centred = range(None, None);
    centred.center = Some(80000.0);
    for (label, o) in [
        ("auto", range(None, None)),
        ("manual", range(Some(60000.0), Some(90000.0))),
        ("min alone", range(Some(70000.0), None)),
        ("max alone", range(None, Some(80000.0))),
        ("log10", log10(Some(60000.0), None)),
        ("centred", centred),
    ] {
        let raster = session.render(&field, &o, false).expect("renders");
        let r = session.palette_range(&field, &o).expect("a range");
        assert_eq!(r, raster.range, "{label}");
        let p = session.palette(&field, &o).expect("a palette");
        let (lo, hi) = r.used().expect("a range to paint over");
        let (t0, t1) = if o.scale.as_deref() == Some("log10") {
            (lo.log10(), hi.log10())
        } else {
            (lo, hi)
        };
        assert_eq!((p.t0, p.t1), (t0, t1), "{label}: the palette spans it");
    }
    assert_eq!(
        session
            .palette_range(&field, &range(None, None))
            .unwrap()
            .used(),
        Some((lo, hi))
    );
    // Refused as `palette` refuses, with the same fields.
    let (_, from_range) = refused(session.palette_range(&field, &range(Some(9.0), Some(3.0))));
    let (_, from_palette) = refused(session.palette(&field, &range(Some(9.0), Some(3.0))));
    assert_eq!(from_range, from_palette);
}

/// The display variant: `palette_display_range` is the range
/// `render_display` paints over, from the full field's stats (#1156).
#[test]
fn a_display_palette_reports_the_range_render_display_paints_over() {
    let session = Session::open(RAP.to_vec()).expect("opens");
    let full = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    let coarse = session
        .decode_display(0, &fieldglass::DisplayDecodeOptions::new(1))
        .expect("a coarse field");
    for o in [range(None, None), range(Some(270.0), Some(300.0))] {
        let raster = session
            .render_display(&coarse, Some(&full.stats), &o, false)
            .expect("renders");
        let r = session
            .palette_display_range(&coarse, Some(&full.stats), &o)
            .expect("a range");
        assert_eq!(r, raster.range);
        assert_eq!(r.total, u64::from(coarse.ni) * u64::from(coarse.nj));
    }
    // The automatic range is the full field's, not the coarse field's own.
    let r = session
        .palette_display_range(&coarse, Some(&full.stats), &range(None, None))
        .unwrap();
    assert_eq!((r.used_min, r.used_max), (full.stats.min, full.stats.max));
    assert!(
        session
            .palette_display_range(&coarse, None, &range(None, None))
            .is_err()
    );
}

const RAP: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/rap_jpeg2000_lambert.grib2");
