//! Vector arrows, and the thing that makes them hard: rotation (#241).
//!
//! An arrow points the way the flow does *on the map*: its direction is a
//! short geographic step along the flow's bearing, projected, so the target
//! does the rotating. Its length is set on screen, one length per speed
//! (#1159), so a full-speed arrow is the same length anywhere on any map and
//! the key beside them is that length. What is checked here is that the
//! rotation comes out right on targets that turn the map by different amounts,
//! that each arrow's direction is its projected step's, that equal speeds draw
//! equal lengths, that grid-relative components are turned through the grid's
//! own north rather than drawn as if they were east and north, and that the
//! spacing and missing-value rules hold.
//!
//! The oracle is a field whose direction is known everywhere: solid-body
//! rotation about the Earth's axis, `u = cos(lat)`, `v = 0` — due east at every
//! point, fastest at the equator.

use fieldglass::render::{VectorOptions, vector_polylines};
use fieldglass::{RenderOptions, Scan, Source};
use fieldglass_core::{GridGeometry, LatLonParams};

/// One arrow's shaft in pixel space: `((x0, y0), (x1, y1))`.
type Shaft = ((f64, f64), (f64, f64));

const NI: u32 = 72;
const NJ: u32 = 37;

fn global() -> GridGeometry {
    GridGeometry::LatLon(LatLonParams {
        ni: NI,
        nj: NJ,
        lat_first: 90.0,
        lon_first: 0.0,
        lat_last: -90.0,
        lon_last: 355.0,
    })
}

/// Arrows for a `u` and a `v` on the one grid `source` places, which is what
/// every case here but the alignment ones is about.
fn one_grid(
    source: &Source<'_>,
    u: &[Option<f64>],
    v: &[Option<f64>],
    options: &RenderOptions,
    vectors: &VectorOptions,
) -> Result<fieldglass::render::VectorArrows, fieldglass::Error> {
    vector_polylines(source, u, source, v, options, vectors)
}

fn source<'a>(geometry: &'a GridGeometry, family: &'a str) -> Source<'a> {
    Source {
        geometry: Ok(geometry),
        ni: NI,
        nj: NJ,
        scan: Scan::north_down(),
        family,
        points_per_row: None,
    }
}

/// Solid-body rotation about the axis: due east everywhere, `cos(lat)` fast.
fn solid_rotation() -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let mut u = Vec::new();
    let mut v = Vec::new();
    for j in 0..NJ {
        let lat = 90.0 - 5.0 * f64::from(j);
        for _ in 0..NI {
            u.push(Some(lat.to_radians().cos()));
            v.push(Some(0.0));
        }
    }
    (u, v)
}

fn options(projection: &str) -> RenderOptions {
    let mut o = RenderOptions::new(projection, "nearest");
    o.width = Some(400);
    o.height = Some(300);
    o
}

/// The shaft of every arrow that came back whole, as `((x0, y0), (x1, y1))`.
///
/// One arrow is one run of five vertices, the first two being the shaft. A run
/// that was split for visibility or at the antimeridian is shorter, and is
/// skipped here rather than read as if its vertices were still an arrow.
fn shafts(arrows: &fieldglass::render::VectorArrows) -> Vec<Shaft> {
    let p = &arrows.runs;
    let mut out = Vec::new();
    let mut at = 0usize;
    for &len in &p.seg_lengths {
        let n = len as usize;
        if n as u32 == fieldglass::render::ARROW_VERTICES {
            out.push((
                (p.xy[at * 2], p.xy[at * 2 + 1]),
                (p.xy[at * 2 + 2], p.xy[at * 2 + 3]),
            ));
        }
        at += n;
    }
    out
}

#[test]
fn an_eastward_field_points_east_on_an_equirectangular_map() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let arrows = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options("equirectangular"),
        &VectorOptions::new(),
    )
    .expect("arrows project");
    let shafts = shafts(&arrows);
    assert!(shafts.len() > 20, "only {} arrows", shafts.len());
    for ((x0, y0), (x1, y1)) in shafts {
        assert!(x1 > x0, "an eastward arrow points right: {x0} → {x1}");
        // Due east is along the parallel, which this map draws horizontal,
        // and the shaft is straight on screen. (An arrow on the map's left
        // edge reads its direction from the steps ahead alone, the one behind
        // being over the seam, which cancels a great circle's lean off the
        // parallel to second order, not exactly.)
        assert!(
            (y1 - y0).abs() < 1e-6 * (x1 - x0).abs(),
            "and runs along its parallel: {y0} → {y1} over {x0} → {x1}"
        );
    }
}

/// The same field on a globe seen from above the north pole. East is no longer
/// "right": it is a circle around the pole, so every arrow must run at a right
/// angle to its own radius, and all of them the same way round.
#[test]
fn the_same_field_runs_around_the_pole_on_an_orthographic_globe() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let mut options = options("orthographic");
    options.center_lat = Some(90.0);
    options.center_lon = Some(0.0);
    let arrows = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows project");
    let shafts = shafts(&arrows);
    assert!(shafts.len() > 10, "only {} arrows", shafts.len());

    // Where the pole itself lands, through the same projection: the center the
    // arrows must run around. Asked of the pipeline rather than assumed from the
    // raster size, so the test does not encode its own idea of the layout.
    let pole = fieldglass::render::overlay_polylines(
        &source(&geometry, "latlon"),
        &options,
        &[90.0, 0.0, 90.0, 0.0],
        &[2],
    )
    .expect("the pole projects");
    let center = (pole.xy[0], pole.xy[1]);
    let mut turns = Vec::new();
    for ((x0, y0), (x1, y1)) in shafts {
        let radius = (x0 - center.0, y0 - center.1);
        let along = (x1 - x0, y1 - y0);
        let r = radius.0.hypot(radius.1);
        let a = along.0.hypot(along.1);
        if r < 20.0 || a < 1.0 {
            continue; // too near the center for the angle to mean much
        }
        // Perpendicular to the radius, within a few degrees: the arrow is the
        // small circle's tangent, not a ray from the pole.
        let cosine = (radius.0 * along.0 + radius.1 * along.1) / (r * a);
        assert!(
            cosine.abs() < 0.2,
            "an eastward arrow is tangential on a polar globe, got cos {cosine}"
        );
        // Which way round: the sign of the cross product, the same for all.
        turns.push((radius.0 * along.1 - radius.1 * along.0).signum());
    }
    assert!(turns.len() > 10, "only {} usable arrows", turns.len());
    assert!(
        turns.windows(2).all(|w| w[0] == w[1]),
        "every arrow turns the same way around the pole"
    );
}

/// A grid-relative pair is rotated through the grid's own north. On a lat/lon
/// grid the two conventions agree, which is the case that proves the rotation
/// is applied rather than merely present: `v = 1` grid-relative is still due
/// north, because the grid's north *is* north here.
#[test]
fn grid_relative_components_agree_with_earth_relative_on_a_latlon_grid() {
    let geometry = global();
    let cells = (NI * NJ) as usize;
    let (u, v) = (vec![Some(0.0); cells], vec![Some(5.0); cells]);
    let options = options("equirectangular");
    let earth = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows");
    let mut grid_relative = VectorOptions::new();
    grid_relative.grid_relative = true;
    let grid = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &grid_relative,
    )
    .expect("arrows");

    let (earth, grid) = (shafts(&earth), shafts(&grid));
    assert_eq!(earth.len(), grid.len());
    for (a, b) in earth.iter().zip(&grid) {
        assert!(
            (a.1.0 - b.1.0).abs() < 0.5 && (a.1.1 - b.1.1).abs() < 0.5,
            "{a:?} vs {b:?}"
        );
    }
    // And they really do point north: up the raster, which is north-up. The
    // pole row draws nothing, having no north to point along (#1045), so
    // every arrow does.
    assert!(earth.len() > 20, "only {} arrows", earth.len());
    for ((_, y0), (_, y1)) in earth.iter().map(|(a, b)| (*a, *b)) {
        assert!(y0 >= 1.0, "no arrow starts on the pole row: y {y0}");
        assert!(y1 < y0, "north is up: {y0} → {y1}");
    }
}

/// The case the option exists for: a projected grid, where the grid's north and
/// true north differ by the convergence angle.
///
/// HRRR and NAM are Lambert and store their winds this way. A grid-relative
/// `(0, 1)` runs along the grid's own columns — straight up the raster in the
/// source projection, everywhere — while the same numbers read as east/north
/// point at true north and fan out across the domain. So the two must differ,
/// and differ by more the further a column sits from the orientation meridian.
#[test]
fn grid_relative_and_earth_relative_part_company_on_a_lambert_grid() {
    let geometry = GridGeometry::Lambert(fieldglass_core::LambertParams {
        earth_radius_m: fieldglass_core::DEFAULT_EARTH_RADIUS_M,
        ni: NI,
        nj: NJ,
        lat_first: 25.0,
        lon_first: -120.0,
        lad: 38.5,
        lov: -97.5,
        dx_metres: 40_000.0,
        dy_metres: 40_000.0,
        latin1: 38.5,
        latin2: 38.5,
    });
    let cells = (NI * NJ) as usize;
    let (u, v) = (vec![Some(0.0); cells], vec![Some(10.0); cells]);
    // The source projection paints grid point (i, j) at pixel (i, j), so "along
    // the grid's columns" is exactly "straight up the raster" there.
    let options = RenderOptions::new("source", "nearest");
    let mut grid_relative = VectorOptions::new();
    grid_relative.grid_relative = true;
    grid_relative.spacing = Some(6);
    let mut earth_relative = grid_relative.clone();
    earth_relative.grid_relative = false;

    let source = source(&geometry, "lambert");
    let grid = shafts(&one_grid(&source, &u, &v, &options, &grid_relative).expect("arrows"));
    let earth = shafts(&one_grid(&source, &u, &v, &options, &earth_relative).expect("arrows"));
    assert!(grid.len() > 20, "only {} arrows", grid.len());

    // Paired by the cell they start from, not by position: the two conventions
    // tilt differently, so they lose different arrows off the top edge.
    let cell = |p: &Shaft| ((p.0.0 * 8.0) as i64, (p.0.1 * 8.0) as i64);
    let by_cell: std::collections::HashMap<(i64, i64), Shaft> =
        earth.iter().map(|a| (cell(a), *a)).collect();
    let mut largest_gap: f64 = 0.0;
    let mut compared = 0;
    for arrow in &grid {
        let ((gx0, gy0), (gx1, gy1)) = *arrow;
        // Grid-relative: straight up the raster, every arrow, to a pixel.
        assert!(
            (gx1 - gx0).abs() < 0.01,
            "a grid-relative arrow runs along its column: {gx0} → {gx1}"
        );
        assert!(gy1 < gy0, "and towards row 0");
        let Some(((_, _), (ex1, ey1))) = by_cell.get(&cell(arrow)).copied() else {
            continue; // clipped in the other convention
        };
        // Earth-relative: true north, which is off-column away from LoV.
        largest_gap = largest_gap.max(((ex1 - gx0) / (gy0 - ey1)).atan().to_degrees().abs());
        compared += 1;
    }
    assert!(compared > 20, "only {compared} arrows in both");
    assert!(
        largest_gap > 5.0,
        "the two conventions should diverge by the convergence angle across a \
         continental domain; the largest was {largest_gap:.1}°"
    );
}

#[test]
fn spacing_thins_the_arrows_and_missing_cells_draw_none() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let options = options("equirectangular");
    let count = |spacing: Option<u32>| {
        let mut vectors = VectorOptions::new();
        vectors.spacing = spacing;
        shafts(&one_grid(&source(&geometry, "latlon"), &u, &v, &options, &vectors).expect("arrows"))
            .len()
    };
    let dense = count(Some(2));
    let sparse = count(Some(8));
    assert!(dense > sparse * 3, "{dense} vs {sparse}");
    assert_eq!(count(Some(0)), count(None), "zero spacing means automatic");

    // A cell missing in either component draws nothing, and an all-missing
    // field draws nothing at all rather than failing.
    let gone = vec![None; (NI * NJ) as usize];
    let mut vectors = VectorOptions::new();
    vectors.spacing = Some(2);
    let half = one_grid(&source(&geometry, "latlon"), &u, &gone, &options, &vectors)
        .expect("a half-missing field is not an error");
    assert_eq!(shafts(&half).len(), 0);
    let still = vec![Some(0.0); (NI * NJ) as usize];
    let calm = one_grid(
        &source(&geometry, "latlon"),
        &still,
        &still,
        &options,
        &vectors,
    )
    .expect("a calm field is not an error");
    assert_eq!(shafts(&calm).len(), 0, "a field at rest draws no arrows");
}

/// The scale a legend puts beside its reference arrow: the fastest cell drawn,
/// or the caller's own reference when it pinned one so two renders compare.
#[test]
fn the_reference_speed_is_the_scale_the_arrows_were_drawn_to() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let options = options("equirectangular");
    let arrows = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows");
    // `u = cos(lat)`, so the fastest sampled cell is on the equator.
    let speed = arrows.reference_speed.expect("arrows were drawn");
    assert!((speed - 1.0).abs() < 0.01, "got {speed}");

    // Pinned: the arrows scale to it instead, so a slower field draws shorter
    // arrows rather than filling the plot again.
    let mut pinned = VectorOptions::new();
    pinned.reference_speed = Some(4.0);
    let held = one_grid(&source(&geometry, "latlon"), &u, &v, &options, &pinned).expect("arrows");
    assert_eq!(held.reference_speed, Some(4.0));
    // The median shaft, not the first: a shaft that crosses the antimeridian
    // comes back as a run spanning the raster, and one sample could be it.
    let length = |a: &fieldglass::render::VectorArrows| {
        let mut lengths: Vec<f64> = shafts(a)
            .into_iter()
            .map(|((x0, y0), (x1, y1))| (x1 - x0).hypot(y1 - y0))
            .collect();
        lengths.sort_by(f64::total_cmp);
        lengths[lengths.len() / 2]
    };
    assert!(
        length(&held) < length(&arrows) / 3.0,
        "a quarter-speed scale draws quarter-length arrows: {} vs {}",
        length(&held),
        length(&arrows)
    );

    // Nothing drawn, nothing to scale.
    let calm = vec![Some(0.0); (NI * NJ) as usize];
    let none = one_grid(
        &source(&geometry, "latlon"),
        &calm,
        &calm,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows");
    assert_eq!(none.reference_speed, None);
}

/// Every target the overlay projects onto, and the source view, each the
/// right way up and flipped, plus the views that put the arrows near a
/// globe's edge or a polar disc's other hemisphere: an off-center globe and a
/// south polar view.
fn every_view() -> Vec<(String, RenderOptions)> {
    let mut views = Vec::new();
    for projection in [
        "source",
        "equirectangular",
        "web_mercator",
        "orthographic",
        "polar_stereographic",
        "mollweide",
        "robinson",
        "equal_earth",
    ] {
        for flip_y in [false, true] {
            let mut o = options(projection);
            o.flip_y = flip_y;
            views.push((format!("{projection}, flip_y {flip_y}"), o));
        }
    }
    let mut tilted = options("orthographic");
    tilted.center_lat = Some(40.0);
    tilted.center_lon = Some(60.0);
    views.push(("orthographic at (40, 60)".to_string(), tilted));
    let mut south = options("polar_stereographic");
    south.center_lat = Some(-90.0);
    views.push(("south polar stereographic".to_string(), south));
    views
}

/// Every whole arrow's five vertices in pixels: tail, tip, barb, tip, barb.
fn whole_arrows(arrows: &fieldglass::render::VectorArrows) -> Vec<[(f64, f64); 5]> {
    let p = &arrows.runs;
    let mut out = Vec::new();
    let mut at = 0usize;
    for &len in &p.seg_lengths {
        if len == fieldglass::render::ARROW_VERTICES {
            let v = |k: usize| (p.xy[(at + k) * 2], p.xy[(at + k) * 2 + 1]);
            out.push([v(0), v(1), v(2), v(3), v(4)]);
        }
        at += len as usize;
    }
    out
}

/// A flow whose direction turns with longitude and whose speed is the same
/// everywhere: `(u, v) = (cos λ, sin λ)`, so every arrow is full length and
/// the arrows point every way between them.
fn turning() -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let (mut u, mut v) = (Vec::new(), Vec::new());
    for _ in 0..NJ {
        for i in 0..NI {
            let lon = (5.0 * f64::from(i)).to_radians();
            u.push(Some(lon.cos()));
            v.push(Some(lon.sin()));
        }
    }
    (u, v)
}

/// The point `metres` from `(lat, lon)` along `bearing` (radians clockwise
/// from north), on the sphere: the test's own formula, not the library's.
fn step_along(lat: f64, lon: f64, bearing: f64, metres: f64) -> (f64, f64) {
    let d = metres / fieldglass_core::DEFAULT_EARTH_RADIUS_M;
    let (p1, l1) = (lat.to_radians(), lon.to_radians());
    let p2 = (p1.sin() * d.cos() + p1.cos() * d.sin() * bearing.cos()).asin();
    let l2 = l1 + (bearing.sin() * d.sin() * p1.cos()).atan2(d.cos() - p1.sin() * p2.sin());
    (p2.to_degrees(), l2.to_degrees())
}

/// The pixels a cell may be at, and which way its flow runs on screen there.
type Flow = (Vec<(f64, f64)>, (f64, f64));

/// Where each sampled cell lands and which way its flow runs there, from
/// `p`, the projection of one three-vertex ring per cell: the point a short
/// step behind it, the cell, and the point a step ahead. A whole ring gives
/// the chord from behind to ahead; a ring cut at the seam or a disc's edge
/// leaves a two-vertex run on one side of the cell, which gives that side's
/// step, and either of its ends may be the cell.
fn flows(p: &fieldglass::ProjectedPolylines) -> Vec<Flow> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for &len in &p.seg_lengths {
        let n = len as usize;
        let v = |k: usize| (p.xy[(at + k) * 2], p.xy[(at + k) * 2 + 1]);
        let (first, last) = (v(0), v(n - 1));
        let along = (last.0 - first.0, last.1 - first.1);
        match n {
            3 => out.push((vec![v(1)], along)),
            2 => out.push((vec![first, last], along)),
            _ => {}
        }
        at += n;
    }
    out
}

/// Each arrow points the way a short step along its flow goes on the map.
///
/// The oracle is the overlay pipeline itself, asked for a 20 m chord through
/// each sampled cell along the flow's bearing. Where the chord is cut, at the
/// seam or off a globe's disc, the half left on the map stands in. Each arrow
/// is paired with its cell by its tail, which is the cell's own pixel. The
/// shaft must run the chord's way to within a hundredth of a degree.
#[test]
fn every_arrow_points_along_its_projected_flow_on_every_target() {
    let geometry = global();
    let (u, v) = turning();
    let step = (NI / 24) as usize;
    let mut rings = Vec::new();
    for j in (step..NJ as usize - 1).step_by(step) {
        for i in (0..NI as usize).step_by(step) {
            let (lat, lon) = (90.0 - 5.0 * j as f64, 5.0 * i as f64);
            // `(cos λ, sin λ)` as (u, v): a bearing of `atan2(u, v)`.
            let bearing = lon.to_radians().cos().atan2(lon.to_radians().sin());
            let to = step_along(lat, lon, bearing, 10.0);
            let from = step_along(lat, lon, bearing + std::f64::consts::PI, 10.0);
            rings.extend_from_slice(&[from.0, from.1, lat, lon, to.0, to.1]);
        }
    }
    let lengths = vec![3u32; rings.len() / 6];
    for (what, o) in every_view() {
        let arrows = one_grid(
            &source(&geometry, "latlon"),
            &u,
            &v,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{what}: {e}"));
        let drawn = whole_arrows(&arrows);
        assert!(drawn.len() > 20, "{what}: only {} arrows", drawn.len());
        let flows = flows(
            &fieldglass::render::overlay_polylines(
                &source(&geometry, "latlon"),
                &o,
                &rings,
                &lengths,
            )
            .unwrap_or_else(|e| panic!("{what}: {e}")),
        );
        let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1) < 1e-6;
        let mut checked = 0;
        for [tail, tip, ..] in &drawn {
            let Some((_, along)) = flows
                .iter()
                .find(|(cells, _)| cells.iter().any(|c| near(*c, *tail)))
            else {
                continue;
            };
            let shaft = (tip.0 - tail.0, tip.1 - tail.1);
            let turn = (shaft.0 * along.1 - shaft.1 * along.0)
                .atan2(shaft.0 * along.0 + shaft.1 * along.1)
                .to_degrees();
            assert!(
                turn.abs() < 0.01,
                "{what}: the arrow at {tail:?} is {turn}° off its projected flow"
            );
            checked += 1;
        }
        assert!(
            checked * 10 >= drawn.len() * 9,
            "{what}: only {checked} of {} arrows checked",
            drawn.len()
        );
    }
}

/// A fine grid next to the pole, on the world maps whose forward projection
/// solves for an auxiliary angle (Mollweide's θ by Newton's method, Robinson's
/// table, Equal Earth's polynomial). Along a row there, neighboring cells are
/// metres apart, and a direction step sized from that row spacing is short
/// enough for the solver's own error near the pole to tilt it. The direction
/// must still be the projected chord's, within a hundredth of a degree.
#[test]
fn arrows_on_a_fine_grid_by_the_pole_point_along_their_projected_flow() {
    // 0.01° cells from lat 89.98 down to 89.62.
    let geometry = GridGeometry::LatLon(LatLonParams {
        ni: NI,
        nj: NJ,
        lat_first: 89.98,
        lon_first: 0.0,
        lat_last: 89.62,
        lon_last: 0.71,
    });
    let cells = (NI * NJ) as usize;
    let (u, v) = (vec![Some(3.0); cells], vec![Some(4.0); cells]);
    let bearing = 3.0f64.atan2(4.0);
    let step = (NI / 24) as usize;
    let mut rings = Vec::new();
    for j in (0..NJ as usize).step_by(step) {
        for i in (0..NI as usize).step_by(step) {
            let (lat, lon) = (89.98 - 0.01 * j as f64, 0.01 * i as f64);
            let to = step_along(lat, lon, bearing, 20.0);
            let from = step_along(lat, lon, bearing + std::f64::consts::PI, 20.0);
            rings.extend_from_slice(&[from.0, from.1, lat, lon, to.0, to.1]);
        }
    }
    let lengths = vec![3u32; rings.len() / 6];
    for projection in ["mollweide", "robinson", "equal_earth"] {
        let mut o = options(projection);
        o.width = Some(4000);
        o.height = Some(2000);
        let arrows = one_grid(
            &source(&geometry, "latlon"),
            &u,
            &v,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{projection}: {e}"));
        let drawn = whole_arrows(&arrows);
        assert!(
            drawn.len() > 100,
            "{projection}: only {} arrows",
            drawn.len()
        );
        let flows = flows(
            &fieldglass::render::overlay_polylines(
                &source(&geometry, "latlon"),
                &o,
                &rings,
                &lengths,
            )
            .unwrap_or_else(|e| panic!("{projection}: {e}")),
        );
        let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).hypot(a.1 - b.1) < 1e-9;
        let mut checked = 0;
        for [tail, tip, ..] in &drawn {
            let Some((_, along)) = flows
                .iter()
                .find(|(cells, _)| cells.iter().any(|c| near(*c, *tail)))
            else {
                continue;
            };
            let shaft = (tip.0 - tail.0, tip.1 - tail.1);
            let turn = (shaft.0 * along.1 - shaft.1 * along.0)
                .atan2(shaft.0 * along.0 + shaft.1 * along.1)
                .to_degrees();
            assert!(
                turn.abs() < 0.01,
                "{projection}: the arrow at {tail:?} is {turn}° off its projected flow"
            );
            checked += 1;
        }
        assert!(
            checked * 10 >= drawn.len() * 9,
            "{projection}: only {checked} of {} arrows checked",
            drawn.len()
        );
    }
}

/// One length per speed, everywhere: with every cell at the same speed, every
/// arrow on every view is drawn whole, its shaft is exactly the key's length,
/// and its head is the same shape on screen: two barbs `0.32` of the shaft
/// long, at 155° either side of it.
#[test]
fn equal_speeds_draw_equal_arrows_on_every_target() {
    let geometry = global();
    let (u, v) = turning();
    for (what, o) in every_view() {
        let arrows = one_grid(
            &source(&geometry, "latlon"),
            &u,
            &v,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{what}: {e}"));
        let key = arrows
            .reference_length
            .unwrap_or_else(|| panic!("{what}: a key length"));
        assert!(
            arrows
                .runs
                .seg_lengths
                .iter()
                .all(|&n| n == fieldglass::render::ARROW_VERTICES),
            "{what}: every arrow is drawn whole, none split at a seam or a disc's edge"
        );
        let drawn = whole_arrows(&arrows);
        assert!(drawn.len() > 20, "{what}: only {} arrows", drawn.len());
        for [tail, tip, left, again, right] in drawn {
            let shaft = (tip.0 - tail.0, tip.1 - tail.1);
            let length = shaft.0.hypot(shaft.1);
            assert!(
                (length - key).abs() < 1e-9 * key,
                "{what}: an arrow at {tail:?} is {length} px, the key {key} px"
            );
            assert_eq!(tip, again, "{what}: the stroke returns to the tip");
            for (barb, side) in [(left, -1.0), (right, 1.0)] {
                let b = (barb.0 - tip.0, barb.1 - tip.1);
                assert!(
                    (b.0.hypot(b.1) - 0.32 * length).abs() < 1e-9 * key,
                    "{what}: a barb is {} px of a {length} px shaft",
                    b.0.hypot(b.1)
                );
                let angle = (shaft.0 * b.1 - shaft.1 * b.0)
                    .atan2(shaft.0 * b.0 + shaft.1 * b.1)
                    .to_degrees();
                assert!(
                    (angle - side * 155.0).abs() < 1e-6,
                    "{what}: a barb leaves the tip at {angle}°"
                );
            }
        }
    }
}

/// The case #1046 found: Equirectangular stretches a parallel by `1 / cos φ`,
/// and an arrow drawn as a geographic segment was stretched with it, twice as
/// long at lat 60 as on the equator. On screen, an eastward arrow is as long
/// at lat 60 as at lat 0.
#[test]
fn equal_speeds_are_equal_lengths_at_every_latitude_on_equirectangular() {
    let geometry = global();
    let cells = (NI * NJ) as usize;
    let (u, v) = (vec![Some(10.0); cells], vec![Some(0.0); cells]);
    let o = options("equirectangular");
    let arrows = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &o,
        &VectorOptions::new(),
    )
    .expect("arrows");
    let row = |lat: f64| {
        let at = fieldglass::render::overlay_polylines(
            &source(&geometry, "latlon"),
            &o,
            &[lat, 0.0, lat, 0.0],
            &[2],
        )
        .expect("a row projects");
        at.xy[1]
    };
    let lengths_on = |lat: f64| -> Vec<f64> {
        let y = row(lat);
        whole_arrows(&arrows)
            .into_iter()
            .filter(|a| (a[0].1 - y).abs() < 1e-6)
            .map(|[tail, tip, ..]| (tip.0 - tail.0).hypot(tip.1 - tail.1))
            .collect()
    };
    let (equator, sixty) = (lengths_on(0.0), lengths_on(60.0));
    assert_eq!(equator.len(), 24, "every column of the equator draws");
    assert_eq!(sixty.len(), 24, "and of lat 60");
    for (a, b) in equator.iter().zip(&sixty) {
        assert!(
            (a - b).abs() < 1e-9,
            "{a} px on the equator, {b} px at lat 60"
        );
    }
}

/// The key is the longest arrow on the map, within a pixel, whatever the map
/// does to distance (#1046's acceptance): `u = cos(lat)` is fastest on the
/// equator, and no arrow anywhere is longer than the key. Where the equator is
/// on the map, the arrows there are the key's length.
#[test]
fn the_key_is_the_longest_arrow_on_every_target() {
    let geometry = global();
    let (u, v) = solid_rotation();
    for (what, o) in every_view() {
        let arrows = one_grid(
            &source(&geometry, "latlon"),
            &u,
            &v,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{what}: {e}"));
        let key = arrows.reference_length.expect("arrows were drawn");
        let longest = whole_arrows(&arrows)
            .into_iter()
            .map(|[tail, tip, ..]| (tip.0 - tail.0).hypot(tip.1 - tail.1))
            .fold(0.0, f64::max);
        assert!(
            longest <= key * (1.0 + 1e-12),
            "{what}: an arrow is {longest} px, longer than the key's {key} px"
        );
        // The equator is a polar view's rim and is not drawn on it.
        if !what.contains("polar") {
            assert!(
                key - longest < 1.0,
                "{what}: the longest arrow is {longest} px, the key {key} px"
            );
        }
    }
}

/// The #1046 fixture, as the panel's test pairs it: `gfs_c255_latlon.grib2`
/// against itself, so every arrow points north-east at √2 × the cell's value.
/// On Equirectangular, Orthographic, both polar views and the source view, the
/// key is the longest arrow drawn, within a pixel.
#[test]
fn the_key_is_the_longest_arrow_on_the_gfs_fixture() {
    /// Embedded rather than read, so the test runs under wasm32 too.
    const GFS: &[u8] =
        include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");
    let session = fieldglass::Session::open(GFS.to_vec()).expect("the fixture opens");
    let field = session
        .decode(0, &fieldglass::DecodeOptions::default())
        .expect("the fixture decodes");
    let values: Vec<Option<f64>> = (0..field.values.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let source = field.source();
    let mut views = Vec::new();
    for projection in [
        "source",
        "equirectangular",
        "orthographic",
        "polar_stereographic",
    ] {
        views.push((
            projection.to_string(),
            RenderOptions::new(projection, "nearest"),
        ));
    }
    let mut south = RenderOptions::new("polar_stereographic", "nearest");
    south.center_lat = Some(-90.0);
    views.push(("south polar stereographic".to_string(), south));
    for (what, o) in views {
        let arrows = vector_polylines(
            &source,
            &values,
            &source,
            &values,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{what}: {e}"));
        let key = arrows.reference_length.expect("arrows were drawn");
        let longest = whole_arrows(&arrows)
            .into_iter()
            .map(|[tail, tip, ..]| (tip.0 - tail.0).hypot(tip.1 - tail.1))
            .fold(0.0, f64::max);
        assert!(
            (key - longest).abs() <= 1.0 && longest <= key * (1.0 + 1e-12),
            "{what}: the longest arrow is {longest} px, the key {key} px"
        );
    }
}

/// At the panel's deepest zoom, a hundredth of a degree, a full-length arrow
/// was the spacing between arrows on screen: on the #1046 fixture at the
/// automatic spacing (15°), some 1,500 map widths, drawn as a headless line
/// across the map, with a key that could not fit (#1168). It is now held to a
/// quarter of the map's shorter side, so the arrow from the cell in view is on
/// the map whole, head and all, and is no longer than the key. Equirectangular,
/// where the zoom happens, and Web Mercator, which zooms the same way.
#[test]
fn a_deep_zoom_holds_a_full_length_arrow_to_a_quarter_of_the_map() {
    /// Embedded rather than read, so the test runs under wasm32 too.
    const GFS: &[u8] =
        include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");
    let session = fieldglass::Session::open(GFS.to_vec()).expect("the fixture opens");
    let field = session
        .decode(0, &fieldglass::DecodeOptions::default())
        .expect("the fixture decodes");
    let values: Vec<Option<f64>> = (0..field.values.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let source = field.source();
    for projection in ["equirectangular", "web_mercator"] {
        // 0.01° across, centered on the sampled cell at (0°, 180°).
        let mut o = RenderOptions::new(projection, "nearest");
        o.width = Some(720);
        o.height = Some(365);
        o.bounds_lat_min = Some(-0.0025);
        o.bounds_lat_max = Some(0.0025);
        o.bounds_lon_min = Some(179.995);
        o.bounds_lon_max = Some(180.005);
        let arrows = vector_polylines(
            &source,
            &values,
            &source,
            &values,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{projection}: {e}"));
        let key = arrows.reference_length.expect("the cell in view draws");
        assert_eq!(
            key,
            0.25 * 365.0,
            "{projection}: a full-length arrow is a quarter of the shorter side"
        );
        let on_map =
            |(x, y): (f64, f64)| (-0.5..=719.5).contains(&x) && (-0.5..=364.5).contains(&y);
        let whole = whole_arrows(&arrows);
        assert!(
            whole.iter().any(|arrow| arrow.iter().all(|&p| on_map(p))),
            "{projection}: an arrow is on the map, head and all: {whole:?}"
        );
        for [tail, tip, ..] in whole {
            let length = (tip.0 - tail.0).hypot(tip.1 - tail.1);
            assert!(
                length <= key * (1.0 + 1e-12),
                "{projection}: an arrow is {length} px, the key {key} px"
            );
        }
    }
}

/// The shorter side of the raster `options` draw `source` on: the one a
/// render, the overlays and the arrows share. Only the box targets read the
/// caller's width and height; the source view is the grid's own shape and the
/// other targets size their own.
fn shorter_side(source: &Source<'_>, values: &[Option<f64>], options: &RenderOptions) -> f64 {
    let raster = fieldglass::render::project(source, values, options).expect("the field renders");
    f64::from(raster.width.min(raster.height))
}

/// The cap leaves a whole-map view of the automatic spacing alone: there a
/// full-length arrow is the spacing between arrows, well short of a quarter
/// of the raster it is drawn on, on every target the panel offers.
#[test]
fn the_cap_leaves_the_whole_map_alone() {
    let geometry = global();
    let (u, v) = solid_rotation();
    for (what, o) in every_view() {
        let src = source(&geometry, "latlon");
        let arrows = one_grid(&src, &u, &v, &o, &VectorOptions::new())
            .unwrap_or_else(|e| panic!("{what}: {e}"));
        let key = arrows.reference_length.expect("arrows were drawn");
        let shorter = shorter_side(&src, &u, &o);
        assert!(
            key < 0.25 * shorter * 0.9,
            "{what}: a full-length arrow is {key} px, near the cap of a quarter of {shorter} px"
        );
    }
}

/// What `MAX_FULL_ARROW_SHARE`'s documentation says of the #1046 fixture: at
/// the automatic spacing, on every target at its own size, a full-length arrow
/// is about a twelfth of the raster's shorter side (0.06 to 0.09 of it), so
/// the quarter cap does not touch a whole-map view.
#[test]
fn a_whole_map_arrow_on_the_gfs_fixture_is_about_a_twelfth_of_the_map() {
    /// Embedded rather than read, so the test runs under wasm32 too.
    const GFS: &[u8] =
        include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");
    let session = fieldglass::Session::open(GFS.to_vec()).expect("the fixture opens");
    let field = session
        .decode(0, &fieldglass::DecodeOptions::default())
        .expect("the fixture decodes");
    let values: Vec<Option<f64>> = (0..field.values.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let source = field.source();
    for projection in [
        "source",
        "equirectangular",
        "web_mercator",
        "orthographic",
        "polar_stereographic",
        "mollweide",
        "robinson",
        "equal_earth",
    ] {
        let o = RenderOptions::new(projection, "nearest");
        let arrows = vector_polylines(
            &source,
            &values,
            &source,
            &values,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{projection}: {e}"));
        let key = arrows.reference_length.expect("arrows were drawn");
        let share = key / shorter_side(&source, &values, &o);
        assert!(
            (0.06..=0.09).contains(&share),
            "{projection}: a full-length arrow is {share} of the shorter side"
        );
    }
}

/// An arrow is drawn on screen from its cell, so one at the map's seam or a
/// globe's edge is drawn whole: it runs off the edge of the map, or past the
/// limb, rather than being cut in two or dropped. On a whole-world map every
/// sampled cell draws; on a globe every cell whose tail is on the near side.
#[test]
fn arrows_at_the_seam_and_the_limb_are_drawn_whole() {
    let geometry = global();
    let cells = (NI * NJ) as usize;
    // Due east, so the arrows on lon 180 point across the seam.
    let (u, v) = (vec![Some(1.0); cells], vec![Some(0.0); cells]);
    // Auto spacing samples every 3rd cell: 24 columns, and 11 rows once the
    // two pole rows are left out.
    let sampled: Vec<(f64, f64)> = (3..NJ - 1)
        .step_by(3)
        .flat_map(|j| {
            (0..NI)
                .step_by(3)
                .map(move |i| (90.0 - 5.0 * f64::from(j), 5.0 * f64::from(i)))
        })
        .collect();
    assert_eq!(sampled.len(), 24 * 11);
    let latlon: Vec<f64> = sampled
        .iter()
        .flat_map(|&(lat, lon)| [lat, lon, lat, lon])
        .collect();
    let mut globe = options("orthographic");
    globe.center_lat = Some(20.0);
    globe.center_lon = Some(100.0);
    for (what, o) in [
        ("equirectangular", options("equirectangular")),
        ("mollweide", options("mollweide")),
        ("robinson", options("robinson")),
        ("source", options("source")),
        ("orthographic", globe),
    ] {
        let arrows = one_grid(
            &source(&geometry, "latlon"),
            &u,
            &v,
            &o,
            &VectorOptions::new(),
        )
        .unwrap_or_else(|e| panic!("{what}: {e}"));
        // How many of the sampled cells land on this map at all.
        let placed = fieldglass::render::overlay_polylines(
            &source(&geometry, "latlon"),
            &o,
            &latlon,
            &vec![2; sampled.len()],
        )
        .expect("the cells project")
        .seg_lengths
        .len();
        assert!(
            arrows
                .runs
                .seg_lengths
                .iter()
                .all(|&n| n == fieldglass::render::ARROW_VERTICES),
            "{what}: no arrow is split"
        );
        assert_eq!(
            arrows.runs.seg_lengths.len(),
            placed,
            "{what}: every cell on the map draws its arrow"
        );
        if what != "orthographic" {
            assert_eq!(
                placed,
                sampled.len(),
                "{what}: the whole world is on the map"
            );
        }
    }
}

/// A pinned scale draws every arrow at a share of full length, and the key is
/// still a full-length arrow, the same length as with the field's own scale:
/// the full length is set by the spacing, not by the speeds.
#[test]
fn a_pinned_reference_speed_keeps_the_key_at_full_length() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let options = options("equirectangular");
    let auto = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows");
    let mut pinned = VectorOptions::new();
    pinned.reference_speed = Some(4.0);
    let held = one_grid(&source(&geometry, "latlon"), &u, &v, &options, &pinned).expect("arrows");
    let (a, h) = (
        auto.reference_length.expect("drawn"),
        held.reference_length.expect("drawn"),
    );
    assert_eq!(a, h, "the same spacing gives the same full length");
    // The fastest cell is speed 1 against a scale of 4: a quarter of the key.
    let longest = shafts(&held)
        .into_iter()
        .map(|((x0, y0), (x1, y1))| (x1 - x0).hypot(y1 - y0))
        .fold(0.0, f64::max);
    assert!(
        (longest - h / 4.0).abs() < 1e-9 * h,
        "the fastest arrow is a quarter of the key: {longest} px of {h}"
    );
}

/// A sparser spacing puts the sampled cells further apart, and a full-length
/// arrow is that far apart on screen, so the key lengthens with it. Not
/// exactly in proportion: the last sampled row and column sit at a different
/// distance from the grid's edge at each spacing.
#[test]
fn spacing_moves_the_reference_length_with_the_arrows() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let options = options("equirectangular");
    let at = |spacing| {
        let mut vectors = VectorOptions::new();
        vectors.spacing = Some(spacing);
        one_grid(&source(&geometry, "latlon"), &u, &v, &options, &vectors)
            .expect("arrows")
            .reference_length
            .expect("drawn")
    };
    let (dense, sparse) = (at(2), at(6));
    assert!(
        (2.5..3.5).contains(&(sparse / dense)),
        "three times the spacing, about three times the length: {dense} px, {sparse} px"
    );
}

/// A spacing wider than the grid samples one cell, which has no sampled
/// neighbor to be spaced from. It still draws, at the length to the grid's
/// last column and row, as it did when arrows were measured on the Earth, and
/// that is longer than a quarter of the map, so the arrow is held to that
/// (#1168).
#[test]
fn a_spacing_wider_than_the_grid_still_draws_one_arrow() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let mut vectors = VectorOptions::new();
    vectors.spacing = Some(1000);
    let arrows = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options("equirectangular"),
        &vectors,
    )
    .expect("arrows");
    // On the global grid the one sampled cell is (0, 0), the north pole,
    // which has no direction to draw (#1045). A band stopping short of the
    // poles has a cell to draw from.
    assert!(arrows.runs.seg_lengths.is_empty());
    let band = GridGeometry::LatLon(LatLonParams {
        ni: NI,
        nj: NJ,
        lat_first: 60.0,
        lon_first: 0.0,
        lat_last: -60.0,
        lon_last: 355.0,
    });
    let arrows = one_grid(
        &source(&band, "latlon"),
        &u,
        &v,
        &options("equirectangular"),
        &vectors,
    )
    .expect("arrows");
    assert_eq!(
        arrows.runs.seg_lengths,
        vec![fieldglass::render::ARROW_VERTICES]
    );
    let key = arrows.reference_length.expect("a key length");
    assert_eq!(key, 0.25 * 300.0, "held to a quarter of the 400 × 300 map");
}

/// Zoomed into a window that leaves out the fastest cell, no arrow on the map
/// is full length, so the key is longer than any of them: the length a
/// full-speed arrow is drawn at in that window.
#[test]
fn a_window_without_the_fastest_cell_still_has_a_key() {
    let geometry = global();
    let (u, v) = solid_rotation();
    let mut window = options("equirectangular");
    window.bounds_lat_min = Some(40.0);
    window.bounds_lat_max = Some(70.0);
    window.bounds_lon_min = Some(0.0);
    window.bounds_lon_max = Some(60.0);
    let arrows = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &window,
        &VectorOptions::new(),
    )
    .expect("arrows");
    let key = arrows.reference_length.expect("arrows are on the window");
    let longest = shafts(&arrows)
        .into_iter()
        .map(|((x0, y0), (x1, y1))| (x1 - x0).hypot(y1 - y0))
        .fold(0.0, f64::max);
    assert!(
        longest > 0.0 && key > longest,
        "key {key} px, longest arrow {longest} px"
    );
}

/// A window the field does not reach clips every arrow away. Then nothing was
/// drawn, so there is no scale to report, even though the field has wind (#871).
#[test]
fn arrows_clipped_off_the_window_report_no_reference_speed() {
    let geometry = global();
    let (u, v) = solid_rotation();
    // A narrow equatorial band of the globe, viewed only over the far south.
    let band = GridGeometry::LatLon(LatLonParams {
        ni: NI,
        nj: NJ,
        lat_first: 10.0,
        lon_first: 0.0,
        lat_last: -10.0,
        lon_last: 20.0,
    });
    let mut window = options("equirectangular");
    window.bounds_lat_min = Some(-80.0);
    window.bounds_lat_max = Some(-60.0);
    window.bounds_lon_min = Some(100.0);
    window.bounds_lon_max = Some(140.0);
    let arrows = one_grid(
        &source(&band, "latlon"),
        &u,
        &v,
        &window,
        &VectorOptions::new(),
    )
    .expect("arrows");
    assert!(
        arrows.runs.seg_lengths.is_empty(),
        "every arrow is off the window"
    );
    assert_eq!(arrows.reference_speed, None);
    assert_eq!(arrows.reference_length, None, "and no length for a key");

    // The same field over the whole globe does draw, so the window is the cause.
    let drawn = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options("equirectangular"),
        &VectorOptions::new(),
    )
    .expect("arrows");
    assert!(drawn.reference_speed.is_some());
}

#[test]
fn mismatched_components_are_refused() {
    let geometry = global();
    let (u, _) = solid_rotation();
    let err = one_grid(
        &source(&geometry, "latlon"),
        &u,
        &[Some(1.0), Some(2.0)],
        &options("equirectangular"),
        &VectorOptions::new(),
    )
    .expect_err("two sizes are not one field");
    assert_eq!(err.code(), "invalid_option");
}

/// `u` and `v` are paired cell by cell, so a `v` whose cells sit somewhere else
/// gives an arrow that exists at no point. Two same-shape grids that differ in
/// origin, or in the direction their rows run, are refused with the words a
/// refused combine uses, because it is the same question (#793).
#[test]
fn a_v_on_another_grid_is_refused_as_combine_refuses_it() {
    let here = global();
    let (u, v) = solid_rotation();
    let options = options("equirectangular");
    let vectors = VectorOptions::new();

    // Same shape, same spacing, moved 5° east.
    let moved = GridGeometry::LatLon(LatLonParams {
        ni: NI,
        nj: NJ,
        lat_first: 90.0,
        lon_first: 5.0,
        lat_last: -90.0,
        lon_last: 360.0,
    });
    let err = vector_polylines(
        &source(&here, "latlon"),
        &u,
        &source(&moved, "latlon"),
        &v,
        &options,
        &vectors,
    )
    .expect_err("a v on a moved grid");
    assert_eq!(err.code(), "unsupported");
    assert!(
        err.message()
            .contains("on different grids and cannot be combined: their grid differs"),
        "{}",
        err.message()
    );
    // The words are `aligned`'s, not a second spelling of them.
    let gate = fieldglass::aligned(&source(&here, "latlon"), &source(&moved, "latlon"))
        .expect_err("the gate refuses the same pair");
    assert_eq!(err.message(), gate.message());

    // Same grid, rows stored the other way up.
    let mut south_up = source(&here, "latlon");
    south_up.scan = Scan::new(false, true, false);
    let err = vector_polylines(
        &source(&here, "latlon"),
        &u,
        &south_up,
        &v,
        &options,
        &vectors,
    )
    .expect_err("a v scanned the other way up");
    assert_eq!(err.code(), "unsupported");
    assert!(
        err.message().contains("their scan order differs"),
        "{}",
        err.message()
    );

    // The same grid built twice is one grid: a second source is not refused
    // for being a second source.
    let again = global();
    let arrows = vector_polylines(
        &source(&here, "latlon"),
        &u,
        &source(&again, "latlon"),
        &v,
        &options,
        &vectors,
    )
    .expect("two equal geometries align");
    assert!(!shafts(&arrows).is_empty());
}
