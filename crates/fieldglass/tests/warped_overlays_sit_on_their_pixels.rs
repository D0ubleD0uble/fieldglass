//! On a reprojected map, overlays, contours and arrows are drawn in the middle
//! of the pixel that shows their point's value (#1470).
//!
//! A warp samples output pixel `(px, py)` at the point its target gives for
//! those whole numbers and paints that value over the pixel from `(px, py)` to
//! `(px + 1, py + 1)`. A host strokes an overlay coordinate `x` at `x` pixels
//! from the raster's left edge. Overlays were placed at the whole coordinate,
//! so every coastline, graticule line, contour and arrow sat on the top-left
//! corner of the pixel holding its point, half a pixel up and left of the
//! picture. The picture is left as it is (the owner's decision on #1470, so a
//! grid drawn at its own size still has one pixel per grid point) and every
//! placed point moves half a pixel on, the rule the source view already uses
//! (#1464).
//!
//! Each case reads `regular_latlon_surface.grib2` (16 × 31 points, 2° apart)
//! through `Session`, paints one hot cell with the real render, and draws
//! through `render::overlay_polylines`, `render::contour_polylines` and
//! `render::vector_polylines` and their `WarpMemo` twins, the calls the hosts
//! make.

#![cfg(all(feature = "grib2", feature = "render", feature = "analysis"))]

use std::sync::Arc;

use fieldglass::render::{
    VectorOptions, contour_polylines, overlay_polylines, probe_pixel, project, vector_polylines,
};
use fieldglass::{DecodeOptions, Field, GridGeometry, ProjectedPolylines, RenderOptions, Session};
use fieldglass::{Source, WarpMemo};

/// 16 × 31 points, 2° apart from 0 to 30°E.
const LATLON: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");

/// The cell made hot: well inside the grid, so its neighbors all exist.
const HOT: (u32, u32) = (7, 12);

const TOL: f64 = 1e-6;

fn field() -> Field {
    let session = Session::open(LATLON.to_vec()).expect("the fixture opens");
    session
        .decode(0, &DecodeOptions::default())
        .expect("the fixture decodes")
}

/// Zero everywhere but [`HOT`], which is one.
fn one_hot(source: &Source<'_>) -> Vec<Option<f64>> {
    let mut values = vec![Some(0.0); (source.ni * source.nj) as usize];
    values[(HOT.1 * source.ni + HOT.0) as usize] = Some(1.0);
    values
}

fn geometry(source: &Source<'_>) -> Arc<GridGeometry> {
    Arc::new((*source.geometry.as_ref().expect("placed")).clone())
}

/// Grid point `(i, j)` as `(lat, lon)`.
fn point(source: &Source<'_>, (i, j): (u32, u32)) -> (f64, f64) {
    source
        .geometry
        .as_ref()
        .expect("placed")
        .forward(i, j)
        .expect("the grid places its points")
}

/// The pixels the render paints with the hot value, as `(px, py)`.
fn hot_pixels(
    source: &Source<'_>,
    values: &[Option<f64>],
    options: &RenderOptions,
) -> Vec<(u32, u32)> {
    let picture = project(source, values, options).expect("renders");
    let width = picture.width as usize;
    (0..picture.values.len())
        .filter(|&k| picture.mask[k] == 1 && picture.values[k] == 1.0)
        .map(|k| ((k % width) as u32, (k / width) as u32))
        .collect()
}

/// Where the overlay draws `(lat, lon)`: the first vertex of a short line from
/// it, through both host calls, which must agree.
fn drawn(
    source: &Source<'_>,
    options: &RenderOptions,
    from: (f64, f64),
    to: (f64, f64),
) -> (f64, f64) {
    let line = [from.0, from.1, to.0, to.1];
    let free = overlay_polylines(source, options, &line, &[2]).expect("projects");
    let kept = WarpMemo::new()
        .overlay_polylines(&geometry(source), source, options, &line, &[2])
        .expect("projects");
    assert_eq!(kept, free, "WarpMemo and render disagree");
    assert_eq!(free.seg_lengths, vec![2], "{:?}", free.xy);
    (free.xy[0], free.xy[1])
}

/// The middle of the box the given pixels cover, in overlay coordinates: a
/// pixel `(px, py)` covers `px..px + 1` and `py..py + 1`.
fn middle_of(pixels: &[(u32, u32)]) -> (f64, f64) {
    let lo_x = pixels.iter().map(|p| p.0).min().expect("some pixels");
    let hi_x = pixels.iter().map(|p| p.0).max().expect("some pixels");
    let lo_y = pixels.iter().map(|p| p.1).min().expect("some pixels");
    let hi_y = pixels.iter().map(|p| p.1).max().expect("some pixels");
    (
        f64::from(lo_x + hi_x + 1) / 2.0,
        f64::from(lo_y + hi_y + 1) / 2.0,
    )
}

/// The lat/lon box at `scale` raster pixels per grid step: `scale == 1` is
/// the grid at its own size, where pixel `(i, j)` samples grid point `(i, j)`.
fn box_options(source: &Source<'_>, scale: u32) -> RenderOptions {
    let mut options = RenderOptions::new("equirectangular", "nearest");
    options.width = Some(scale * (source.ni - 1) + 1);
    options.height = Some(scale * (source.nj - 1) + 1);
    options
}

#[test]
fn at_the_grid_s_own_size_a_point_is_drawn_in_its_pixel_s_middle() {
    let field = field();
    let source = field.source();
    let values = one_hot(&source);
    let options = box_options(&source, 1);
    let hot = hot_pixels(&source, &values, &options);
    assert_eq!(hot.len(), 1, "one pixel per grid point: {hot:?}");
    let (px, py) = hot[0];

    let at = point(&source, HOT);
    let next = point(&source, (HOT.0 + 1, HOT.1));
    let got = drawn(&source, &options, at, next);
    let want = (f64::from(px) + 0.5, f64::from(py) + 0.5);
    assert!(
        (got.0 - want.0).abs() < TOL && (got.1 - want.1).abs() < TOL,
        "the hot point is drawn at {got:?}, its pixel's middle is {want:?}"
    );

    // The probe still reads the cell under the pointer: the pixel the point
    // is drawn in is the hot cell's.
    let probe = probe_pixel(&source, &values, &options, px, py)
        .expect("probes")
        .expect("on the raster");
    assert_eq!(probe.value, Some(1.0));
    assert_eq!(
        (probe.grid_i, probe.grid_j),
        (Some(HOT.0 as i32), Some(HOT.1 as i32))
    );
    let (lat, lon) = at;
    assert!(
        (probe.lat.expect("placed") - lat).abs() < TOL
            && (probe.lon.expect("placed") - lon).abs() < TOL,
        "the probe reads the point the pixel samples, unmoved: {probe:?}"
    );

    // Painted bottom-up, the hot pixel is on row `h - 1 - py`, and the point
    // is drawn in that row's middle; the probe reads it there.
    let mut flipped = options.clone();
    flipped.flip_y = true;
    let row = source.nj - 1 - py;
    let got = drawn(&source, &flipped, at, next);
    let want = (f64::from(px) + 0.5, f64::from(row) + 0.5);
    assert!(
        (got.0 - want.0).abs() < TOL && (got.1 - want.1).abs() < TOL,
        "flipped, the hot point is drawn at {got:?}, its pixel's middle is {want:?}"
    );
    let probe = probe_pixel(&source, &values, &flipped, px, row)
        .expect("probes")
        .expect("on the raster");
    assert_eq!(probe.value, Some(1.0), "flipped: {probe:?}");
}

#[test]
fn at_a_resampled_size_a_point_is_drawn_in_the_middle_of_its_pixels() {
    // Three pixels a grid step: nearest paints the hot cell over the 3 × 3
    // pixels within a step and a half of its point, whose middle is the
    // middle of the one that samples the point itself.
    let field = field();
    let source = field.source();
    let values = one_hot(&source);
    let options = box_options(&source, 3);
    let hot = hot_pixels(&source, &values, &options);
    assert_eq!(hot.len(), 9, "a 3 × 3 block: {hot:?}");
    let want = middle_of(&hot);

    let got = drawn(
        &source,
        &options,
        point(&source, HOT),
        point(&source, (HOT.0, HOT.1 + 1)),
    );
    assert!(
        (got.0 - want.0).abs() < TOL && (got.1 - want.1).abs() < TOL,
        "the hot point is drawn at {got:?}, the middle of its pixels is {want:?}"
    );

    // The probe reads the hot cell across the whole block.
    for &(px, py) in &hot {
        let probe = probe_pixel(&source, &values, &options, px, py)
            .expect("probes")
            .expect("on the raster");
        assert_eq!(probe.value, Some(1.0), "({px}, {py})");
    }
}

#[test]
fn a_disc_s_center_is_drawn_in_the_middle_of_the_raster() {
    // A disc target frames its center on the middle of the raster: pixel
    // `(w - 1) / 2` samples it. Drawn half a pixel on, it is at `w / 2`, the
    // raster's own middle, whatever the parity of its size.
    let field = field();
    let source = field.source();
    let values = one_hot(&source);
    let (lat, lon) = point(&source, HOT);

    let mut globe = RenderOptions::new("orthographic", "nearest");
    globe.center_lat = Some(lat);
    globe.center_lon = Some(lon);
    let mut north = RenderOptions::new("polar_stereographic", "nearest");
    north.projection_preset = Some("north".to_owned());
    for (what, options, center, near) in [
        ("orthographic", &globe, (lat, lon), (lat + 2.0, lon)),
        ("polar_stereographic", &north, (90.0, 0.0), (80.0, 0.0)),
    ] {
        let picture = project(&source, &values, options).expect("renders");
        let want = (
            f64::from(picture.width) / 2.0,
            f64::from(picture.height) / 2.0,
        );
        let got = drawn(&source, options, center, near);
        assert!(
            (got.0 - want.0).abs() < TOL && (got.1 - want.1).abs() < TOL,
            "{what}: the center is drawn at {got:?}, the raster's middle is {want:?}"
        );
    }

    // And on the globe the hot cell, painted round its centre, is drawn over.
    let hot = hot_pixels(&source, &values, &globe);
    let middle = middle_of(&hot);
    let got = drawn(&source, &globe, (lat, lon), (lat + 2.0, lon));
    assert!(
        (got.0 - middle.0).abs() <= 0.5 && (got.1 - middle.1).abs() <= 0.5,
        "the hot point is drawn at {got:?}, its pixels' middle is {middle:?}"
    );
}

#[test]
fn contours_ring_the_hot_pixel_s_middle() {
    // At the grid's own size each isoline round the one hot cell is a
    // diamond on its point, so its vertices average to the point: the middle
    // of the pixel that paints it.
    let field = field();
    let source = field.source();
    let values = one_hot(&source);
    let options = box_options(&source, 1);
    let (px, py) = hot_pixels(&source, &values, &options)[0];

    let free = contour_polylines(&source, &values, &options, None).expect("contours");
    let kept = WarpMemo::new()
        .contour_polylines(&geometry(&source), &source, &values, &options, None)
        .expect("contours");
    assert_eq!(kept, free, "WarpMemo and render disagree");
    let vertices = free.xy.as_chunks::<2>().0;
    assert!(vertices.len() >= 8, "at least one ring: {:?}", free.xy);
    let n = vertices.len() as f64;
    let mean = (
        vertices.iter().map(|v| v[0]).sum::<f64>() / n,
        vertices.iter().map(|v| v[1]).sum::<f64>() / n,
    );
    let want = (f64::from(px) + 0.5, f64::from(py) + 0.5);
    assert!(
        (mean.0 - want.0).abs() < TOL && (mean.1 - want.1).abs() < TOL,
        "the isolines center on {mean:?}, the hot pixel's middle is {want:?}"
    );
    // Every vertex lies within one step of the hot point, inside the four
    // pixels next to it.
    for v in vertices {
        assert!(
            (v[0] - want.0).abs() <= 1.0 && (v[1] - want.1).abs() <= 1.0,
            "{v:?} is off the hot cell's neighborhood round {want:?}"
        );
    }
}

#[test]
fn arrows_start_in_the_middle_of_their_cells_pixels() {
    // At the grid's own size, one arrow a cell: each tail is in the middle of
    // a pixel, and that pixel probes the arrow's own cell.
    let field = field();
    let source = field.source();
    let (ni, nj) = (source.ni, source.nj);
    let values = one_hot(&source);
    let options = box_options(&source, 1);
    let u: Vec<Option<f64>> = vec![Some(1.0); (ni * nj) as usize];
    let mut vectors = VectorOptions::new();
    vectors.spacing = Some(1);
    let free = vector_polylines(&source, &u, &source, &u, &options, &vectors).expect("arrows");
    let kept = WarpMemo::new()
        .vector_polylines(
            &geometry(&source),
            (&source, &u),
            (&source, &u),
            &options,
            &vectors,
        )
        .expect("arrows");
    assert_eq!(kept.runs, free.runs, "WarpMemo and render disagree");

    let runs = free.runs.xy.as_chunks::<10>().0;
    assert_eq!(runs.len(), (ni * nj) as usize, "an arrow a cell");
    for (k, run) in runs.iter().enumerate() {
        let (i, j) = (k as u32 % ni, k as u32 / ni);
        let (x, y) = (run[0], run[1]);
        assert!(
            (x - x.floor() - 0.5).abs() < TOL && (y - y.floor() - 0.5).abs() < TOL,
            "the arrow for ({i}, {j}) starts at ({x}, {y}), not a pixel's middle"
        );
        let probe = probe_pixel(&source, &values, &options, x as u32, y as u32)
            .expect("probes")
            .expect("on the raster");
        assert_eq!(
            (probe.grid_i, probe.grid_j),
            (Some(i as i32), Some(j as i32)),
            "the arrow for ({i}, {j}) starts in another cell's pixel"
        );
    }
}

#[test]
fn every_grid_point_on_a_line_is_drawn_in_a_pixel_s_middle() {
    // A line through grid points, corners included: at the grid's own size
    // pixel `(i, j)` samples grid point `(i, j)` (this grid is stored north
    // first, as the box draws it), and each vertex is in that pixel's middle.
    let field = field();
    let source = field.source();
    let options = box_options(&source, 1);
    let at = |i, j| point(&source, (i, j));
    let points = [(0, 0), (3, 5), (15, 30), (8, 2)];
    let mut latlon = Vec::new();
    for (i, j) in points {
        let (lat, lon) = at(i, j);
        latlon.extend_from_slice(&[lat, lon]);
    }
    let out: ProjectedPolylines =
        overlay_polylines(&source, &options, &latlon, &[4]).expect("projects");
    assert_eq!(out.seg_lengths, vec![4], "{:?}", out.xy);
    for (v, (i, j)) in out.xy.as_chunks::<2>().0.iter().zip(points) {
        let want = (f64::from(i) + 0.5, f64::from(j) + 0.5);
        assert!(
            (v[0] - want.0).abs() < TOL && (v[1] - want.1).abs() < TOL,
            "grid point ({i}, {j}) is drawn at {v:?}, its pixel's middle is {want:?}"
        );
    }
}
