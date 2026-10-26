//! Arrows on a lookup grid's source view point along the flow and sit on
//! their own cells (#1456).
//!
//! A lookup grid (here NetCDF 2-D coordinate variables) answers `(lat, lon)`
//! with the nearest whole cell. Arrows used to be placed through that answer:
//! the direction was the chord to a point one arrow spacing along the flow,
//! which lands on a whole cell too, so at a spacing of one only a few
//! directions were drawn, a mean 12° and up to 59° off, and arrows whose chord
//! did not leave their cell were not drawn at all. On the fold of a tripolar
//! mesh, some of the last row's cells answered with their mirror cell on the
//! row before, so those arrows were drawn there. The source view paints cell
//! `(i, j)` at pixel `(i, j)`, so an arrow now starts on its own pixel, in its
//! middle (#1464), and takes its direction from the grid's own shape.
//!
//! The grids are the fixtures' own, through `Session::open` and
//! `decode_slice`, with a flow of known bearing laid over them, and the arrows
//! come from `render::vector_polylines` and `WarpMemo::vector_polylines` (the
//! call the napi addon makes). The oracle does not share the renderer's
//! method: it steps 50 m along the bearing either side of the cell on the
//! sphere and finds where each point falls in the grid by inverting the
//! bilinear map between cell centers.

#![cfg(all(feature = "netcdf", feature = "render"))]

use std::f64::consts::{PI, TAU};
use std::sync::Arc;

use fieldglass::render::{VectorArrows, VectorOptions, vector_polylines};
use fieldglass::{DecodeOptions, Field, GridGeometry, RenderOptions, Session, WarpMemo};

/// A MiRS swath: 96 fields of view by 100 scanlines.
const MIRS: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/mirs_swath_n21.nc");
/// A 260 × 200 window of the RTOFS tripolar mesh.
const ARCTIC: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc");
/// RTOFS rows 3270..3298, every column: both halves of the fold.
const FOLD: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_fold.nc");

/// How far a drawn arrow may point from the oracle's direction, in degrees.
/// The two agree to 0.3° on these grids; the old chord was a mean 12° off at
/// a spacing of one.
const TOLERANCE_DEG: f64 = 1.0;

/// Earth radius for the oracle's steps; the direction does not depend on it.
const RADIUS_M: f64 = 6_371_229.0;

/// A variable's first slice on its own axes.
fn decoded(bytes: &[u8], name: &str) -> Field {
    let session = Session::open(bytes.to_vec()).expect("the fixture opens");
    let v = session
        .variables()
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .clone();
    session
        .decode_slice(
            v.index,
            v.detected_y_dim.expect("a y axis"),
            v.detected_x_dim.expect("an x axis"),
            &vec![0; v.dims.len()],
            &DecodeOptions::default(),
        )
        .expect("the slice decodes")
}

/// The point `distance` metres from `from` along `bearing`, on a sphere.
fn destination(from: (f64, f64), bearing: f64, distance: f64) -> (f64, f64) {
    let a = distance / RADIUS_M;
    let (lat, lon) = (from.0.to_radians(), from.1.to_radians());
    let lat2 = (lat.sin() * a.cos() + lat.cos() * a.sin() * bearing.cos()).asin();
    let lon2 = lon + (bearing.sin() * a.sin() * lat.cos()).atan2(a.cos() - lat.sin() * lat2.sin());
    (lat2.to_degrees(), lon2.to_degrees())
}

/// `p` in east and north metres about `center` (azimuthal equidistant).
fn local(center: (f64, f64), p: (f64, f64)) -> (f64, f64) {
    let (lat1, lat2) = (center.0.to_radians(), p.0.to_radians());
    let d_lon = (p.1 - center.1).to_radians();
    let towards = (d_lon.sin() * lat2.cos())
        .atan2(lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * d_lon.cos());
    let h =
        ((lat2 - lat1) / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    let d = 2.0 * RADIUS_M * h.sqrt().asin();
    (d * towards.sin(), d * towards.cos())
}

/// The bilinear position of grid point `(fi, fj)`, in metres about `center`.
fn bilinear(g: &GridGeometry, center: (f64, f64), (fi, fj): (f64, f64)) -> Option<(f64, f64)> {
    let (ni, nj) = g.dims().expect("a placed grid has dims");
    let i0 = (fi.floor().max(0.0) as u32).min(ni - 2);
    let j0 = (fj.floor().max(0.0) as u32).min(nj - 2);
    let (x, y) = (fi - f64::from(i0), fj - f64::from(j0));
    let at = |i, j| g.forward(i, j).map(|p| local(center, p));
    let (a, b, c, d) = (
        at(i0, j0)?,
        at(i0 + 1, j0)?,
        at(i0, j0 + 1)?,
        at(i0 + 1, j0 + 1)?,
    );
    let mix = |a: f64, b: f64, c: f64, d: f64| {
        a * (1.0 - x) * (1.0 - y) + b * x * (1.0 - y) + c * (1.0 - x) * y + d * x * y
    };
    Some((mix(a.0, b.0, c.0, d.0), mix(a.1, b.1, c.1, d.1)))
}

/// The grid point the bilinear map takes to `target`, by Newton from `start`.
fn invert(
    g: &GridGeometry,
    center: (f64, f64),
    target: (f64, f64),
    start: (f64, f64),
) -> Option<(f64, f64)> {
    let (mut fi, mut fj) = start;
    for _ in 0..8 {
        let h = 1e-6;
        let p = bilinear(g, center, (fi, fj))?;
        let pi = bilinear(g, center, (fi + h, fj))?;
        let pj = bilinear(g, center, (fi, fj + h))?;
        let (a, b) = ((pi.0 - p.0) / h, (pj.0 - p.0) / h);
        let (c, d) = ((pi.1 - p.1) / h, (pj.1 - p.1) / h);
        let det = a * d - b * c;
        let (rx, ry) = (target.0 - p.0, target.1 - p.1);
        fi += (d * rx - b * ry) / det;
        fj += (a * ry - c * rx) / det;
    }
    (fi.is_finite() && fj.is_finite()).then_some((fi, fj))
}

/// Which way `bearing` runs through cell `(i, j)` in grid space, rows as
/// stored: from the point 50 m behind the cell to the point 50 m ahead.
fn oracle(g: &GridGeometry, (i, j): (u32, u32), bearing: f64) -> (f64, f64) {
    let center = g.forward(i, j).expect("a sampled cell has a position");
    let start = (f64::from(i), f64::from(j));
    let grid_point = |b: f64| {
        let target = local(center, destination(center, b, 50.0));
        invert(g, center, target, start).expect("the oracle inverts")
    };
    let (ahead, behind) = (grid_point(bearing), grid_point(bearing + PI));
    (ahead.0 - behind.0, ahead.1 - behind.1)
}

/// The angle between two directions, in degrees.
fn degrees_apart(a: (f64, f64), b: (f64, f64)) -> f64 {
    let d = a.1.atan2(a.0) - b.1.atan2(b.0);
    ((d + PI).rem_euclid(TAU) - PI).abs().to_degrees()
}

/// Arrows for a flow of one bearing and unit speed everywhere, from both
/// entry points, which must agree.
fn arrows(field: &Field, bearing: f64, spacing: Option<u32>, flip_y: bool) -> VectorArrows {
    arrows_at_speed(field, bearing, |_| 1.0, spacing, flip_y)
}

/// [`arrows`], with each cell's speed from `speed(i, j)`.
fn arrows_at_speed(
    field: &Field,
    bearing: f64,
    speed: impl Fn((u32, u32)) -> f64,
    spacing: Option<u32>,
    flip_y: bool,
) -> VectorArrows {
    let source = field.source();
    let cells = sampled(source.ni, source.nj, 1);
    let u: Vec<_> = cells
        .iter()
        .map(|&c| Some(speed(c) * bearing.sin()))
        .collect();
    let v: Vec<_> = cells
        .iter()
        .map(|&c| Some(speed(c) * bearing.cos()))
        .collect();
    let mut options = RenderOptions::new("source", "nearest");
    options.flip_y = flip_y;
    let mut vectors = VectorOptions::new();
    vectors.spacing = spacing;
    let free = vector_polylines(&source, &u, &source, &v, &options, &vectors)
        .expect("the arrows are drawn");
    let geometry = Arc::new((*source.geometry.as_ref().expect("placed")).clone());
    let kept = WarpMemo::new()
        .vector_polylines(&geometry, (&source, &u), (&source, &v), &options, &vectors)
        .expect("the arrows are drawn");
    assert_eq!(kept, free, "WarpMemo and render disagree");
    free
}

/// The cells an arrow is sampled at, in the order they are drawn.
fn sampled(ni: u32, nj: u32, step: u32) -> Vec<(u32, u32)> {
    (0..nj)
        .step_by(step as usize)
        .flat_map(|j| (0..ni).step_by(step as usize).map(move |i| (i, j)))
        .collect()
}

#[test]
fn source_view_arrows_on_a_lookup_grid_point_along_the_flow() {
    for (file, bytes, var) in [
        ("mirs_swath_n21.nc", MIRS, "TPW"),
        ("rtofs_tripolar_arctic.nc", ARCTIC, "ice_coverage"),
    ] {
        let field = decoded(bytes, var);
        let source = field.source();
        let g = source.geometry.as_ref().expect("placed");
        let (ni, nj) = (source.ni, source.nj);
        // One, and the default (`None`): four on the swath, ten on the mesh.
        for (spacing, step) in [(Some(1), 1), (None, (ni.max(nj) / 24).max(1))] {
            let cells = sampled(ni, nj, step);
            for degrees in [20.0f64, 135.0, 250.0] {
                let bearing = degrees.to_radians();
                let want: Vec<(f64, f64)> = cells.iter().map(|&c| oracle(g, c, bearing)).collect();
                for requested_flip in [false, true] {
                    let flip = source.scan.flips_source_rows(requested_flip);
                    let what = format!(
                        "{file} spacing {spacing:?} bearing {degrees}° (flip_y {requested_flip})"
                    );
                    let out = arrows(&field, bearing, spacing, requested_flip);
                    assert_eq!(
                        out.runs.seg_lengths.len(),
                        cells.len(),
                        "{what}: an arrow for every sampled cell"
                    );
                    let mut worst = (0.0f64, 0);
                    for (k, (&(i, j), run)) in cells
                        .iter()
                        .zip(out.runs.xy.as_chunks::<10>().0)
                        .enumerate()
                    {
                        // The middle of the cell's own pixel (#1464).
                        let row = if flip { nj - 1 - j } else { j };
                        assert_eq!(
                            (run[0], run[1]),
                            (f64::from(i) + 0.5, f64::from(row) + 0.5),
                            "{what}: the arrow for cell ({i}, {j}) starts on its own pixel"
                        );
                        let drawn = (run[2] - run[0], run[3] - run[1]);
                        let (dx, dy) = want[k];
                        let off = degrees_apart(drawn, (dx, if flip { -dy } else { dy }));
                        if off > worst.0 {
                            worst = (off, k);
                        }
                    }
                    assert!(
                        worst.0 < TOLERANCE_DEG,
                        "{what}: the arrow at cell {:?} points {:.2}° from the flow",
                        cells[worst.1],
                        worst.0
                    );
                }
            }
        }
    }
}

/// Whether the fold file's cell `(i, j)` has no direction on the grid: column
/// 0 runs into one of the bipolar patch's poles, where every row meets, and a
/// cell whose neighbors down the column sit on it is not drawn.
fn collapsed(g: &GridGeometry, (i, j): (u32, u32)) -> bool {
    let (_, nj) = g.dims().expect("a placed grid has dims");
    let here = g.forward(i, j);
    [j.checked_sub(1), (j + 1 < nj).then_some(j + 1)]
        .into_iter()
        .flatten()
        .all(|k| g.forward(i, k) == here)
}

#[test]
fn arrows_on_a_tripolar_folds_last_row_sit_on_their_own_cells() {
    let field = decoded(FOLD, "ice_coverage");
    let source = field.source();
    let g = source.geometry.as_ref().expect("placed");
    let (ni, nj) = (source.ni, source.nj);
    // A spacing that samples rows 0 and `nj - 1`, the fold's duplicated row,
    // whose cells the nearest-cell answer can mirror onto row `nj - 2`.
    let step = nj - 1;
    let want: Vec<(u32, u32)> = sampled(ni, nj, step)
        .into_iter()
        .filter(|&c| !collapsed(g, c))
        .collect();
    assert!(
        want.iter().filter(|c| c.1 == nj - 1).count() > 100,
        "the fold row is sampled"
    );
    let flip = source.scan.flips_source_rows(false);
    let out = arrows(&field, 0.7, Some(step), false);
    let tails: Vec<(f64, f64)> = out
        .runs
        .xy
        .as_chunks::<10>()
        .0
        .iter()
        .map(|r| (r[0], r[1]))
        .collect();
    let pixels: Vec<(f64, f64)> = want
        .iter()
        .map(|&(i, j)| {
            // The middle of the cell's own pixel (#1464).
            let row = if flip { nj - 1 - j } else { j };
            (f64::from(i) + 0.5, f64::from(row) + 0.5)
        })
        .collect();
    let misplaced: Vec<_> = tails
        .iter()
        .zip(&pixels)
        .filter(|(got, want)| got != want)
        .take(3)
        .collect();
    assert!(
        misplaced.is_empty() && tails.len() == pixels.len(),
        "{} arrows for {} cells; first tails off their cells (drawn, own): {misplaced:?}",
        tails.len(),
        pixels.len()
    );
}

#[test]
fn a_cell_the_grid_gives_no_direction_does_not_set_the_scale() {
    // The scale is a drawn arrow's (#1045): a cell left undrawn for want of a
    // direction must not be the fastest one the key reports.
    let field = decoded(FOLD, "ice_coverage");
    let source = field.source();
    let g = source.geometry.as_ref().expect("placed");
    let step = source.nj - 1;
    let undrawn: Vec<(u32, u32)> = sampled(source.ni, source.nj, step)
        .into_iter()
        .filter(|&c| collapsed(g, c))
        .collect();
    assert!(!undrawn.is_empty(), "the spacing samples a pole cell");
    let out = arrows_at_speed(
        &field,
        0.7,
        |c| if collapsed(g, c) { 10.0 } else { 1.0 },
        Some(step),
        false,
    );
    assert_eq!(
        out.reference_speed,
        Some(1.0),
        "the key's speed is a drawn arrow's, not that of the undrawn cells {undrawn:?}"
    );
}
