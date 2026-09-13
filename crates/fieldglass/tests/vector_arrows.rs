//! Vector arrows, and the thing that makes them hard: rotation (#241).
//!
//! An arrow is built as a geographic segment — from the cell, along the flow's
//! bearing — and projected like a coastline, so the target does the rotating.
//! What is checked here is that the rotation comes out right on targets that
//! turn the map by different amounts, that grid-relative components are turned
//! through the grid's own north rather than drawn as if they were east and
//! north, and that the spacing and missing-value rules hold.
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
    let arrows = vector_polylines(
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
        // A great circle leaving due east bends poleward, so the shaft is not
        // exactly horizontal — but it is within a few degrees of it.
        assert!(
            (y1 - y0).abs() < 0.1 * (x1 - x0).abs(),
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
    let arrows = vector_polylines(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows project");
    let shafts = shafts(&arrows);
    assert!(shafts.len() > 10, "only {} arrows", shafts.len());

    // Where the pole itself lands, through the same projection: the centre the
    // arrows must run around. Asked of the pipeline rather than assumed from the
    // raster size, so the test does not encode its own idea of the layout.
    let pole = fieldglass::render::overlay_polylines(
        &source(&geometry, "latlon"),
        &options,
        &[90.0, 0.0, 90.0, 0.0],
        &[2],
    )
    .expect("the pole projects");
    let centre = (pole.xy[0], pole.xy[1]);
    let mut turns = Vec::new();
    for ((x0, y0), (x1, y1)) in shafts {
        let radius = (x0 - centre.0, y0 - centre.1);
        let along = (x1 - x0, y1 - y0);
        let r = radius.0.hypot(radius.1);
        let a = along.0.hypot(along.1);
        if r < 20.0 || a < 1.0 {
            continue; // too near the centre for the angle to mean much
        }
        // Perpendicular to the radius, within a few degrees: the arrow is a
        // chord of a small circle, not a ray from the pole.
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
    let earth = vector_polylines(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows");
    let mut grid_relative = VectorOptions::new();
    grid_relative.grid_relative = true;
    let grid = vector_polylines(
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
    // arrow *at* the pole is the exception and is left in deliberately — north
    // from the north pole crosses the pole and comes back down the far side,
    // which is what a great circle does and what the projection draws.
    let mut crossed_the_pole = 0;
    for ((_, y0), (_, y1)) in earth.iter().map(|(a, b)| (*a, *b)) {
        if y0 < 1.0 {
            crossed_the_pole += 1;
            continue;
        }
        assert!(y1 < y0, "north is up: {y0} → {y1}");
    }
    assert!(crossed_the_pole > 0, "the pole row is in this grid");
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
    let grid =
        shafts(&vector_polylines(&source, &u, &v, &options, &grid_relative).expect("arrows"));
    let earth =
        shafts(&vector_polylines(&source, &u, &v, &options, &earth_relative).expect("arrows"));
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
        shafts(
            &vector_polylines(&source(&geometry, "latlon"), &u, &v, &options, &vectors)
                .expect("arrows"),
        )
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
    let half = vector_polylines(&source(&geometry, "latlon"), &u, &gone, &options, &vectors)
        .expect("a half-missing field is not an error");
    assert_eq!(shafts(&half).len(), 0);
    let still = vec![Some(0.0); (NI * NJ) as usize];
    let calm = vector_polylines(
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
    let arrows = vector_polylines(
        &source(&geometry, "latlon"),
        &u,
        &v,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows");
    // `u = cos(lat)`, so the fastest sampled cell is on the equator.
    assert!(
        (arrows.reference_speed - 1.0).abs() < 0.01,
        "got {}",
        arrows.reference_speed
    );

    // Pinned: the arrows scale to it instead, so a slower field draws shorter
    // arrows rather than filling the plot again.
    let mut pinned = VectorOptions::new();
    pinned.reference_speed = Some(4.0);
    let held =
        vector_polylines(&source(&geometry, "latlon"), &u, &v, &options, &pinned).expect("arrows");
    assert_eq!(held.reference_speed, 4.0);
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
    let none = vector_polylines(
        &source(&geometry, "latlon"),
        &calm,
        &calm,
        &options,
        &VectorOptions::new(),
    )
    .expect("arrows");
    assert_eq!(none.reference_speed, 0.0);
}

#[test]
fn mismatched_components_are_refused() {
    let geometry = global();
    let (u, _) = solid_rotation();
    let err = vector_polylines(
        &source(&geometry, "latlon"),
        &u,
        &[Some(1.0), Some(2.0)],
        &options("equirectangular"),
        &VectorOptions::new(),
    )
    .expect_err("two sizes are not one field");
    assert_eq!(err.code(), "invalid_option");
}
