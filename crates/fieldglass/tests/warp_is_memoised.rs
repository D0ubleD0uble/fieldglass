//! A repaint that changes only the colours, or draws another field on the same
//! grid, does not warp again (#1233).
//!
//! `render::project` runs the warp's inverse map for every output pixel, which
//! on a curvilinear grid is a spatial-index search a pixel: about five seconds
//! for the full RTOFS tripolar mesh on Equirectangular. A host that keeps state
//! between draws holds a `WarpMemo`, and these tests hold it to two rules:
//!
//! * **It warps once.** A colour map, reversal, scale or colour range change,
//!   and a change to another field on the same grid, make no new plan. Counted
//!   with `WarpMemo::plans_made`, not timed: a wall-clock bound on a shared
//!   runner is a machine-speed assertion, and it cannot tell a memo from a fast
//!   rebuild.
//! * **It changes nothing.** Every answer is `render::project`'s for the same
//!   field and options, to the bit, whether it came from a kept plan or a new
//!   one. And whatever the warp depends on (the grid, the target and its
//!   parameters, the window, the output size, the resampling) makes a new plan.

#![cfg(all(feature = "netcdf", feature = "grib2", feature = "render"))]

use std::sync::Arc;

use fieldglass::api::Field;
use fieldglass::render::{Projected, project};
use fieldglass::{DecodeOptions, RenderOptions, Session, Source, WarpMemo};
use fieldglass_core::GridGeometry;

/// Three RTOFS ice fields on one 260 × 200 window of the tripolar mesh,
/// placed by their 2-D coordinates: a lookup grid, the case the memo is for.
const RTOFS: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc");
/// A Lambert conformal message: a grid whose inverse map is a formula, and
/// which `Session` hands over as a fresh geometry on every decode.
const LAMBERT: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2");

/// Every slice of the RTOFS fixture that has horizontal axes, decoded.
fn rtofs_fields() -> Vec<Field> {
    let session = Session::open(RTOFS.to_vec()).expect("the fixture opens");
    session
        .variables()
        .iter()
        .filter_map(|v| {
            let (y, x) = (v.detected_y_dim?, v.detected_x_dim?);
            Some(
                session
                    .decode_slice(
                        v.index,
                        y,
                        x,
                        &vec![0; v.dims.len()],
                        &DecodeOptions::default(),
                    )
                    .expect("the slice decodes"),
            )
        })
        .collect()
}

/// A field's values as the warp reads them.
fn cells(field: &Field) -> Vec<Option<f64>> {
    (0..field.mask.len())
        .map(|k| (field.mask[k] != 0).then(|| field.values.get(k)).flatten())
        .collect()
}

/// `field`'s source, placed on `geometry` rather than on its own copy.
fn on<'a>(field: &'a Field, geometry: &'a GridGeometry) -> Source<'a> {
    let mut source = field.source();
    source.geometry = Ok(geometry);
    source
}

fn options(projection: &str) -> RenderOptions {
    RenderOptions::new(projection, "nearest")
}

/// `memo.project` answers what `render::project` answers, to the bit.
fn assert_projects_as_fresh(
    memo: &WarpMemo,
    geometry: &Arc<GridGeometry>,
    field: &Field,
    o: &RenderOptions,
) -> Projected {
    let values = cells(field);
    let memoised = memo
        .project(geometry, &on(field, geometry), &values, o)
        .expect("the memo projects");
    let fresh = project(&field.source(), &values, o).expect("a fresh warp");
    assert_eq!(
        (memoised.width, memoised.height, &memoised.bounds),
        (fresh.width, fresh.height, &fresh.bounds),
        "{} under {o:?}",
        field.georef.label
    );
    // Compared whole but not printed: a raster of either is too long to read.
    assert!(memoised.mask == fresh.mask, "the mask under {o:?}");
    let bits = |p: &Projected| p.values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
    assert!(
        bits(&memoised) == bits(&fresh),
        "the warped values under {o:?}"
    );
    assert_eq!(memoised.summary, fresh.summary);
    memoised
}

/// The colour controls the panel offers, each on its own and together: none
/// of them is read before the paint.
fn colour_changes(projection: &str) -> Vec<RenderOptions> {
    let mut out = Vec::new();
    let mut o = options(projection);
    o.colormap = Some("turbo".into());
    out.push(o.clone());
    o.reverse_colormap = Some(true);
    out.push(o.clone());
    o.scale_mode = Some("log10".into());
    o.range_min = Some(0.01);
    out.push(o.clone());
    o.scale_mode = None;
    o.range_min = Some(0.1);
    o.range_max = Some(0.6);
    out.push(o.clone());
    o.range_min = None;
    o.range_max = None;
    o.range_center = Some(0.5);
    out.push(o);
    out
}

#[test]
fn a_colour_change_on_a_curvilinear_grid_warps_once() {
    let fields = rtofs_fields();
    let field = &fields[0];
    assert_eq!(field.georef.label, "curvilinear", "a lookup grid");
    let geometry = Arc::new(field.georef.geometry.clone());
    let memo = WarpMemo::new();

    let first = assert_projects_as_fresh(&memo, &geometry, field, &options("equirectangular"));
    assert!(first.mask.contains(&1), "the warp paints the field");
    assert_eq!(memo.plans_made(), 1, "the first draw plans");
    for o in colour_changes("equirectangular") {
        assert_projects_as_fresh(&memo, &geometry, field, &o);
    }
    assert_eq!(memo.plans_made(), 1, "no colour change warps again");
}

#[test]
fn another_field_on_the_same_grid_warps_once() {
    let fields = rtofs_fields();
    assert!(fields.len() >= 3, "three ice fields on one mesh");
    let geometry = Arc::new(fields[0].georef.geometry.clone());
    let memo = WarpMemo::new();
    for field in &fields {
        // Each field's own geometry is equal to the shared one, as every field
        // of an RTOFS file is on one mesh; the memo serves them all from it.
        assert_eq!(field.georef.geometry, *geometry);
        assert_projects_as_fresh(&memo, &geometry, field, &options("mollweide"));
    }
    assert_eq!(
        memo.plans_made(),
        1,
        "a change of field on one grid samples the same plan"
    );
}

#[test]
fn a_fresh_geometry_equal_to_the_kept_one_reuses_its_plan() {
    // `Session` hands a GRIB message's geometry over by value, so a host that
    // wraps it on each draw shares no pointer with the last: the memo compares
    // the grids themselves.
    let session = Session::open(LAMBERT.to_vec()).expect("the fixture opens");
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("message 0 decodes");
    let memo = WarpMemo::new();
    let o = options("equirectangular");
    assert_projects_as_fresh(&memo, &Arc::new(field.georef.geometry.clone()), &field, &o);
    for c in colour_changes("equirectangular") {
        assert_projects_as_fresh(&memo, &Arc::new(field.georef.geometry.clone()), &field, &c);
    }
    assert_eq!(memo.plans_made(), 1);
}

#[test]
fn what_the_warp_depends_on_makes_a_new_plan() {
    let fields = rtofs_fields();
    let field = &fields[0];
    let geometry = Arc::new(field.georef.geometry.clone());
    let memo = WarpMemo::new();
    let mut made = 0;
    let mut expect_new_plan = |o: &RenderOptions, why: &str| {
        assert_projects_as_fresh(&memo, &geometry, field, o);
        made += 1;
        assert_eq!(memo.plans_made(), made, "{why} plans again");
    };

    expect_new_plan(&options("equirectangular"), "the first draw");
    expect_new_plan(&options("web_mercator"), "another target");
    let mut moved = options("mollweide");
    expect_new_plan(&moved, "a world target");
    moved.center_lon = Some(120.0);
    expect_new_plan(&moved, "a new central meridian");
    let mut window = options("equirectangular");
    window.bounds_lat_min = Some(60.0);
    window.bounds_lat_max = Some(80.0);
    window.bounds_lon_min = Some(-100.0);
    window.bounds_lon_max = Some(-40.0);
    expect_new_plan(&window, "a manual window");
    window.width = Some(300);
    window.height = Some(100);
    expect_new_plan(&window, "a named output size");

    // A lookup grid samples nearest whatever is asked, so asking for bilinear
    // is the same warp, and is served from the same plan.
    let mut bilinear = window.clone();
    bilinear.resampling = "bilinear".into();
    assert_projects_as_fresh(&memo, &geometry, field, &bilinear);
    assert_eq!(
        memo.plans_made(),
        made,
        "bilinear on a lookup grid is nearest"
    );
}

#[test]
fn a_refused_window_keeps_the_plan_it_found() {
    // A Web Mercator band entirely poleward of its renderable range is
    // refused while the target is built. The refusal must not cost the plan
    // already held: going back to the earlier window is then free (#1278).
    let fields = rtofs_fields();
    let field = &fields[0];
    let geometry = Arc::new(field.georef.geometry.clone());
    let memo = WarpMemo::new();
    let original = options("web_mercator");
    assert_projects_as_fresh(&memo, &geometry, field, &original);
    assert_eq!(memo.plans_made(), 1);

    let mut polar = options("web_mercator");
    polar.bounds_lat_min = Some(86.0);
    polar.bounds_lat_max = Some(89.0);
    polar.bounds_lon_min = Some(-100.0);
    polar.bounds_lon_max = Some(-40.0);
    let values = cells(field);
    memo.project(&geometry, &on(field, &geometry), &values, &polar)
        .expect_err("a band outside the Web Mercator range is refused");
    assert_eq!(memo.plans_made(), 1, "a refusal plans nothing");

    assert_projects_as_fresh(&memo, &geometry, field, &original);
    assert_eq!(
        memo.plans_made(),
        1,
        "the plan survived the refused request"
    );
}

#[test]
fn bilinear_on_a_grid_that_blends_makes_a_new_plan() {
    let session = Session::open(LAMBERT.to_vec()).expect("the fixture opens");
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("message 0 decodes");
    let geometry = Arc::new(field.georef.geometry.clone());
    let memo = WarpMemo::new();
    let mut o = options("equirectangular");
    assert_projects_as_fresh(&memo, &geometry, &field, &o);
    o.resampling = "bilinear".into();
    assert_projects_as_fresh(&memo, &geometry, &field, &o);
    assert_eq!(
        memo.plans_made(),
        2,
        "a blend reads other cells than nearest"
    );
    o.colormap = Some("plasma".into());
    assert_projects_as_fresh(&memo, &geometry, &field, &o);
    assert_eq!(
        memo.plans_made(),
        2,
        "and a colour change on it warps no more"
    );
}

#[test]
fn another_grid_makes_a_new_plan() {
    let fields = rtofs_fields();
    let rtofs = &fields[0];
    let session = Session::open(LAMBERT.to_vec()).expect("the fixture opens");
    let lambert = session
        .decode(0, &DecodeOptions::default())
        .expect("message 0 decodes");
    let memo = WarpMemo::new();
    let o = options("equirectangular");
    assert_projects_as_fresh(&memo, &Arc::new(rtofs.georef.geometry.clone()), rtofs, &o);
    assert_projects_as_fresh(
        &memo,
        &Arc::new(lambert.georef.geometry.clone()),
        &lambert,
        &o,
    );
    assert_projects_as_fresh(&memo, &Arc::new(rtofs.georef.geometry.clone()), rtofs, &o);
    assert_eq!(memo.plans_made(), 3, "each change of grid plans");
}

#[test]
fn a_source_not_on_the_given_geometry_is_projected_without_the_memo() {
    let fields = rtofs_fields();
    let field = &fields[0];
    let session = Session::open(LAMBERT.to_vec()).expect("the fixture opens");
    let other = Arc::new(
        session
            .decode(0, &DecodeOptions::default())
            .expect("message 0 decodes")
            .georef
            .geometry,
    );
    let memo = WarpMemo::new();
    let values = cells(field);
    let o = options("equirectangular");
    // The source reads its own geometry; the one handed over is another grid.
    let answered = memo
        .project(&other, &field.source(), &values, &o)
        .expect("projects");
    assert_eq!(
        answered,
        project(&field.source(), &values, &o).expect("fresh")
    );
    assert_eq!(memo.plans_made(), 0, "nothing was kept for the wrong grid");
}

#[test]
fn the_source_view_needs_no_plan() {
    let fields = rtofs_fields();
    let field = &fields[0];
    let geometry = Arc::new(field.georef.geometry.clone());
    let memo = WarpMemo::new();
    assert_projects_as_fresh(&memo, &geometry, field, &options("source"));
    assert_eq!(memo.plans_made(), 0);
}

#[test]
fn values_of_the_wrong_length_are_refused_as_project_refuses_them() {
    let fields = rtofs_fields();
    let field = &fields[0];
    let geometry = Arc::new(field.georef.geometry.clone());
    let memo = WarpMemo::new();
    let short = vec![Some(1.0); 4];
    let o = options("equirectangular");
    let memoised = memo
        .project(&geometry, &on(field, &geometry), &short, &o)
        .expect_err("too few values");
    let fresh = project(&field.source(), &short, &o).expect_err("too few values");
    assert_eq!(memoised.message(), fresh.message());
    assert_eq!(memo.plans_made(), 0);
}

#[test]
fn a_plan_past_the_budget_keeps_the_plan_before_it() {
    let fields = rtofs_fields();
    let field = &fields[0];
    let geometry = Arc::new(field.georef.geometry.clone());
    let o = options("equirectangular");
    let fresh = project(&field.source(), &cells(field), &o).expect("a fresh warp");
    let bytes = u64::from(fresh.width) * u64::from(fresh.height) * 4;

    // Room for the default raster and no more.
    let memo = WarpMemo::with_max_plan_bytes(bytes);
    assert_projects_as_fresh(&memo, &geometry, field, &o);
    assert_eq!(memo.plans_made(), 1);

    // A named size twice as wide plans past the budget: it runs unmemoised.
    let mut big = o.clone();
    big.width = Some(fresh.width * 2);
    big.height = Some(fresh.height);
    assert_projects_as_fresh(&memo, &geometry, field, &big);
    assert_eq!(memo.plans_made(), 1, "the oversize plan is not made");

    assert_projects_as_fresh(&memo, &geometry, field, &o);
    assert_eq!(
        memo.plans_made(),
        1,
        "the kept plan still serves the original options"
    );
}

#[test]
fn a_plan_past_the_budget_is_not_kept() {
    let fields = rtofs_fields();
    let field = &fields[0];
    let geometry = Arc::new(field.georef.geometry.clone());
    let o = options("equirectangular");

    // The fixture's raster is floored to 720 pixels on its long edge, and its
    // plan keeps one `u32` a pixel.
    let fresh = project(&field.source(), &cells(field), &o).expect("a fresh warp");
    let bytes = u64::from(fresh.width) * u64::from(fresh.height) * 4;

    let at_budget = WarpMemo::with_max_plan_bytes(bytes);
    assert_projects_as_fresh(&at_budget, &geometry, field, &o);
    assert_projects_as_fresh(
        &at_budget,
        &geometry,
        field,
        &colour_changes("equirectangular")[0],
    );
    assert_eq!(
        at_budget.plans_made(),
        1,
        "a plan of exactly the budget is kept"
    );

    let under = WarpMemo::with_max_plan_bytes(bytes - 1);
    assert_projects_as_fresh(&under, &geometry, field, &o);
    assert_projects_as_fresh(
        &under,
        &geometry,
        field,
        &colour_changes("equirectangular")[0],
    );
    assert_eq!(
        under.plans_made(),
        0,
        "a plan past the budget is never made"
    );

    assert_eq!(WarpMemo::MAX_PLAN_BYTES, 256 << 20);
}

/// `geometry` with the centres at `fill` left as fill values (NaN), the way a
/// swath granule or a masked ocean coordinate arrives.
fn with_fill_value_centres(geometry: &GridGeometry, fill: &[usize]) -> GridGeometry {
    let GridGeometry::Lookup(index) = geometry else {
        panic!("a lookup grid");
    };
    let (ni, nj) = index.dims();
    let (mut lats, mut lons) = (Vec::new(), Vec::new());
    for j in 0..nj {
        for i in 0..ni {
            let (lat, lon) = index.centre(i, j).unwrap_or((f64::NAN, f64::NAN));
            lats.push(lat);
            lons.push(lon);
        }
    }
    for &k in fill {
        lats[k] = f64::NAN;
        lons[k] = f64::NAN;
    }
    GridGeometry::Lookup(
        fieldglass_core::SpatialIndex::new(ni, nj, &lats, &lons).expect("finite centres remain"),
    )
}

#[test]
fn a_curvilinear_grid_with_fill_value_centres_warps_once_from_fresh_copies() {
    // `NaN != NaN`, so such a grid does not equal a copy of itself. A host that
    // hands a fresh geometry on every draw (the Zarr handle before it kept its
    // slices) used to plan again, about five seconds on RTOFS, on every colour
    // change (#1273). The grids compare by fingerprint instead.
    let fields = rtofs_fields();
    let field = &fields[0];
    let grid = with_fill_value_centres(&field.georef.geometry, &[0, 7, 1234]);
    assert_ne!(grid, grid.clone(), "the grid is not equal to its own copy");

    let memo = WarpMemo::new();
    let draw = |o: &RenderOptions| {
        let geometry = Arc::new(grid.clone());
        let values = cells(field);
        let memoised = memo
            .project(&geometry, &on(field, &geometry), &values, o)
            .expect("the memo projects");
        let fresh = project(&on(field, &geometry), &values, o).expect("a fresh warp");
        assert_eq!(memoised, fresh, "under {o:?}");
    };
    draw(&options("equirectangular"));
    assert_eq!(memo.plans_made(), 1);
    for o in colour_changes("equirectangular") {
        draw(&o);
    }
    assert_eq!(
        memo.plans_made(),
        1,
        "no colour change plans again on a copy of the same grid"
    );

    // A different grid is still told apart: one more centre left as a fill.
    let other = Arc::new(with_fill_value_centres(&grid, &[2000]));
    memo.project(
        &other,
        &on(field, &other),
        &cells(field),
        &options("equirectangular"),
    )
    .expect("the memo projects");
    assert_eq!(memo.plans_made(), 2, "another grid plans");
}

#[test]
fn a_new_orthographic_centre_latitude_or_polar_hemisphere_makes_a_new_plan() {
    // The key is the resolved target, so a change of only `lat0` or only
    // `south_pole` must not be served from the plan before it (#1233 review).
    let fields = rtofs_fields();
    let field = &fields[0];
    let geometry = Arc::new(field.georef.geometry.clone());
    let memo = WarpMemo::new();
    let mut made = 0;
    let mut expect_new_plan = |o: &RenderOptions, why: &str| {
        assert_projects_as_fresh(&memo, &geometry, field, o);
        made += 1;
        assert_eq!(memo.plans_made(), made, "{why} plans again");
    };

    let mut ortho = options("orthographic");
    ortho.center_lat = Some(40.0);
    ortho.center_lon = Some(-30.0);
    expect_new_plan(&ortho, "the first orthographic draw");
    ortho.center_lat = Some(60.0);
    expect_new_plan(&ortho, "a new centre latitude alone");

    let mut polar = options("polar_stereographic");
    polar.projection_preset = Some("north".into());
    expect_new_plan(&polar, "a polar target");
    polar.projection_preset = Some("south".into());
    expect_new_plan(&polar, "the other pole alone");

    // And a colour change on the last one is free.
    polar.colormap = Some("turbo".into());
    assert_projects_as_fresh(&memo, &geometry, field, &polar);
    assert_eq!(memo.plans_made(), made, "a colour change plans nothing");
}
