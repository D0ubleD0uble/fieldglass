//! A reduced-resolution decode through the GRIB2 handle the extension calls
//! (#1082).
//!
//! `decodeDisplay` hands back a `DisplayGrid` on the derived grid, and
//! `renderDisplay` paints it through the same projection pipeline as
//! `renderGrid`: the source view as stored, and a reprojection warped from the
//! coarse grid's own geometry. The colour range is the full field's, from the
//! stats the caller passes. The browser host's half is
//! `crates/fieldglass-wasm/tests/node/reduced.mjs`, over the same fixture.

use super::*;
use crate::netcdf_slice_tests::opts;

const RAP: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/rap_jpeg2000_lambert.grib2");
const SIMPLE: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");

fn rap() -> Grib2File {
    Grib2File::from_vec(RAP.to_vec()).expect("RAP opens")
}

fn reduce(r: u8) -> Option<serde_json::Value> {
    Some(serde_json::json!({ "reduce": r }))
}

/// The full field's stats, in the wire form a caller passes back.
fn full_stats() -> serde_json::Value {
    let session = fieldglass::Session::open(RAP.to_vec()).expect("opens");
    let field = session
        .decode(0, &fieldglass::DecodeOptions::new(fieldglass::Dtype::Auto))
        .expect("decodes");
    serde_json::to_value(&field.stats).expect("stats serialise")
}

fn opaque(r: &RenderedGrid) -> usize {
    r.rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] != 0)
        .count()
}

/// The acceptance case through this host: reduction 1 is half the size on each
/// axis, on the grid `subsampled(1)` derives from the message's own.
#[test]
fn a_rap_message_decodes_at_reduction_one_onto_the_derived_grid() {
    let h = rap();
    let display = h
        .decode_display(0, reduce(1))
        .expect("decodes at reduction 1");
    let (_, placed) = h.resolved(0).expect("the full field");
    assert_eq!(display.reduction(), 1);
    assert_eq!(
        (display.ni(), display.nj()),
        (placed.ni.div_ceil(2), placed.nj.div_ceil(2))
    );
    assert_eq!(display.values.len(), (display.ni() * display.nj()) as usize);
    let derived = placed
        .geometry
        .subsampled(1)
        .expect("a Lambert coarse grid");
    assert_eq!(*display.placed.geometry, derived);
    assert_eq!(display.georef.geometry, derived);
    assert_eq!(display.placed.family, placed.family);
    assert_eq!(display.placed.scan, placed.scan);

    // `grid()` is the API's `Georef`, as it crosses.
    let grid: fieldglass::Georef =
        serde_json::from_value(display.grid().expect("grid")).expect("a Georef");
    assert_eq!(grid.geometry, derived);
    assert_eq!((grid.ni, grid.nj), (Some(display.ni()), Some(display.nj())));

    // No options, and reduction zero, are the message's own field.
    for options in [None, Some(serde_json::Value::Null), reduce(0)] {
        let same = h.decode_display(0, options).expect("reduction zero");
        assert_eq!(same.reduction(), 0);
        assert_eq!(*same.placed.geometry, *placed.geometry);
        let (raw, _) = h.resolved(0).expect("full");
        assert_eq!(same.values, *raw);
    }
}

/// `renderDisplay` paints the coarse grid as stored, and warps it onto a
/// window where the full field lands: the same window at the same size covers
/// the same pixels. On the message's own grid it would cover a quarter.
#[test]
fn a_display_grid_renders_and_warps() {
    let h = rap();
    let display = h.decode_display(0, reduce(1)).expect("decodes");
    let stats = full_stats();

    let source = h
        .render_display(&display, Some(stats.clone()), &opts("source"))
        .expect("the source view paints");
    assert_eq!(
        (source.width, source.height),
        (display.ni() as i32, display.nj() as i32)
    );
    assert_eq!(opaque(&source), display.values.iter().flatten().count());

    let mut warp = opts("equirectangular");
    let full = h
        .render_grid(0, opts("equirectangular"))
        .expect("the full warp");
    warp.bounds_lat_min = full.used_lat_min;
    warp.bounds_lat_max = full.used_lat_max;
    warp.bounds_lon_min = full.used_lon_min;
    warp.bounds_lon_max = full.used_lon_max;
    warp.width = Some(240);
    warp.height = Some(160);
    let full = h
        .render_grid(0, opts_like(&warp))
        .expect("full, in the window");
    let coarse = h
        .render_display(&display, Some(stats), &warp)
        .expect("a display grid warps");
    assert_eq!((coarse.width, coarse.height), (240, 160));
    assert_eq!(
        (coarse.used_lat_min, coarse.used_lon_max),
        (full.used_lat_min, full.used_lon_max)
    );
    let (a, b) = (opaque(&full), opaque(&coarse));
    assert!(
        a.abs_diff(b) * 50 < a,
        "{b} painted pixels against the full field's {a}"
    );
    assert!(
        coarse.projection_summary.contains("equirectangular"),
        "{}",
        coarse.projection_summary
    );
}

/// A copy of the options, since `RenderOptions` is a napi object with no
/// `Clone`.
fn opts_like(o: &RenderOptions) -> RenderOptions {
    RenderOptions {
        projection: o.projection.clone(),
        bounds_lat_min: o.bounds_lat_min,
        bounds_lat_max: o.bounds_lat_max,
        bounds_lon_min: o.bounds_lon_min,
        bounds_lon_max: o.bounds_lon_max,
        width: o.width,
        height: o.height,
        ..opts(&o.projection)
    }
}

/// The automatic range is the full field's, from the stats passed in, never
/// the coarse values' own (which a low-pass moves).
#[test]
fn the_automatic_range_is_the_full_fields() {
    let h = rap();
    let display = h.decode_display(0, reduce(1)).expect("decodes");
    let stats = full_stats();
    let r = h
        .render_display(&display, Some(stats.clone()), &opts("source"))
        .expect("renders");
    assert_eq!(r.used_min, stats["min"].as_f64());
    assert_eq!(r.used_max, stats["max"].as_f64());
    let own = fieldglass::min_max_ignoring_mask(display.values.iter().copied()).expect("present");
    assert_ne!(
        own,
        (r.used_min.unwrap(), r.used_max.unwrap()),
        "the fixture tells the rules apart"
    );
}

/// What is refused, each with the engine's code.
#[test]
fn refusals_carry_their_codes() {
    let h = rap();
    let display = h.decode_display(0, reduce(1)).expect("decodes");
    let code = |r: Result<RenderedGrid, RenderError>| -> String {
        r.expect_err("refused")
            .api()
            .expect("an engine refusal")
            .code()
            .to_string()
    };
    // No stats, and an end left to the data.
    assert_eq!(
        code(h.render_display(&display, None, &opts("source"))),
        "invalid_option"
    );
    // Stats that do not read.
    assert_eq!(
        code(h.render_display(
            &display,
            Some(serde_json::json!({ "min": "cold" })),
            &opts("source")
        )),
        "invalid_option"
    );
    // Both ends manual: no stats needed.
    let manual = RenderOptions {
        range_min: Some(270.0),
        range_max: Some(320.0),
        ..opts("source")
    };
    h.render_display(&display, None, &manual)
        .expect("a manual range needs no stats");

    // A message with no pyramid, and options that do not read.
    let simple = Grib2File::from_vec(SIMPLE.to_vec()).expect("opens");
    let decode_code = |r: Result<DisplayGrid, RenderError>| -> String {
        r.expect_err("refused")
            .api()
            .expect("an engine refusal")
            .code()
            .to_string()
    };
    assert_eq!(
        decode_code(simple.decode_display(0, reduce(1))),
        "unsupported"
    );
    assert_eq!(
        decode_code(h.decode_display(0, Some(serde_json::json!({ "reduce": 300 })))),
        "invalid_option"
    );
    assert_eq!(
        decode_code(h.decode_display(3, reduce(1))),
        "no_such_message"
    );
}

/// At reduction 0 a band-limited spectral map keeps its "shown at T359 of
/// T383" label through `renderDisplay`, as it has through `renderGrid` (#637).
#[test]
fn a_band_limited_spectral_map_keeps_its_label_at_reduction_zero() {
    const T383: &[u8] =
        include_bytes!("../../fieldglass-grib2/tests/fixtures/spectral_simple_t383.grib2");
    let h = Grib2File::from_vec(T383.to_vec()).expect("opens");
    let want = h
        .render_grid(0, opts("source"))
        .expect("renders")
        .truncation;
    assert!(want.is_some(), "the fixture is band-limited");
    let display = h.decode_display(0, reduce(0)).expect("reduction zero");
    let manual = RenderOptions {
        range_min: Some(-1.0e6),
        range_max: Some(1.0e6),
        ..opts("source")
    };
    let got = h
        .render_display(&display, None, &manual)
        .expect("renders")
        .truncation;
    assert_eq!(got, want);
}
