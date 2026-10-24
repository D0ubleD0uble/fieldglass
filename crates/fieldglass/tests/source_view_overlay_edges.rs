//! On the source view, overlays run to the edges of the drawn cells and are
//! drawn in the middle of each cell, where its value is (#1464).
//!
//! The source view paints grid point `(i, j)` as the pixel from `(i, j)` to
//! `(i + 1, j + 1)`, and a host strokes an overlay coordinate `x` at `x`
//! pixels from the picture's left edge. Two things were wrong there. Overlay
//! coordinates were the grid index itself, so every coastline, border, lake,
//! river, graticule line, contour and arrow was drawn half a cell up and left
//! of the field under it. And the inverse stopped at the outer grid points, so
//! nothing reached past them: on `regular_latlon_surface.grib2` (16 × 31)
//! every layer spanned `x` 0 to 15 of a 16-pixel picture, a whole cell short on
//! the right. A line that left the grid also stopped at its last vertex
//! inside, up to a segment short of where it crossed the edge.
//!
//! The fixtures are read through `Session` and projected with
//! `render::overlay_polylines` and `WarpMemo::overlay_polylines`, the calls the
//! hosts make, with the layers the extension bundles: a formula grid
//! (`regular_latlon_surface.grib2`), a projected one (HRRR, Lambert conformal)
//! and a lookup grid (a MiRS swath, CF 2-D coordinates).

#![cfg(all(feature = "grib2", feature = "netcdf", feature = "render"))]

use std::sync::Arc;

use fieldglass::render::{VectorOptions, overlay_polylines, vector_polylines};
use fieldglass::{DecodeOptions, Field, ProjectedPolylines, RenderOptions, Session, WarpMemo};

/// 16 × 31 points, 2° apart from 0 to 30°E.
const LATLON: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");
/// HRRR's 1799 × 1059 Lambert conformal grid over the United States.
const HRRR: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2");
/// A MiRS swath: 96 fields of view by 100 scanlines.
const MIRS: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/mirs_swath_n21.nc");

/// The Natural Earth layers the extension draws, embedded rather than read
/// from disk so the test runs where only the working directory is visible.
const LAYERS: [(&str, &str); 4] = [
    (
        "coastline",
        include_str!("../../../extension/media/coastline-110m.json"),
    ),
    (
        "borders",
        include_str!("../../../extension/media/borders-110m.json"),
    ),
    (
        "lakes",
        include_str!("../../../extension/media/lakes-110m.json"),
    ),
    (
        "rivers",
        include_str!("../../../extension/media/rivers-110m.json"),
    ),
];

/// Flat `[lat, lon, …]` and per-line vertex counts.
type Lines = (Vec<f64>, Vec<u32>);

/// A bundled asset's `[lon, lat, …]` lines, as `projectOverlay` takes them.
fn vector_layer(json: &str) -> Lines {
    let asset: serde_json::Value = serde_json::from_str(json).expect("the asset is JSON");
    let (mut latlon, mut rings) = (Vec::new(), Vec::new());
    for line in asset["lines"].as_array().expect("a lines array") {
        let coords: Vec<f64> = line
            .as_array()
            .expect("a line is an array")
            .iter()
            .map(|c| c.as_f64().expect("a coordinate is a number"))
            .collect();
        rings.push(u32::try_from(coords.len() / 2).expect("a line fits u32"));
        for &[lon, lat] in coords.as_chunks::<2>().0 {
            latlon.extend_from_slice(&[lat, lon]);
        }
    }
    (latlon, rings)
}

/// A graticule every 5°: meridians from pole to pole and parallels all the
/// way round, a vertex every degree. Every line runs past any regional grid
/// on both sides, so on one each piece drawn must end on the picture's edge.
fn graticule() -> Lines {
    let (mut latlon, mut rings) = (Vec::new(), Vec::new());
    for k in 0..72 {
        let lon = -180.0 + 5.0 * f64::from(k);
        for s in 0..=178 {
            latlon.extend_from_slice(&[-89.0 + f64::from(s), lon]);
        }
        rings.push(179);
    }
    for k in 1..36 {
        let lat = -90.0 + 5.0 * f64::from(k);
        for s in 0..=360 {
            latlon.extend_from_slice(&[lat, -180.0 + f64::from(s)]);
        }
        rings.push(361);
    }
    (latlon, rings)
}

/// Every layer the extension draws, by name.
fn layers() -> Vec<(&'static str, Lines)> {
    let mut layers: Vec<_> = LAYERS
        .iter()
        .map(|&(name, json)| (name, vector_layer(json)))
        .collect();
    layers.push(("graticule", graticule()));
    layers
}

/// A GRIB file's first message, decoded.
fn message(bytes: &[u8]) -> Field {
    let session = Session::open(bytes.to_vec()).expect("the fixture opens");
    session
        .decode(0, &DecodeOptions::default())
        .expect("the fixture decodes")
}

/// A NetCDF variable's first slice on its own axes.
fn variable(bytes: &[u8], name: &str) -> Field {
    let session = Session::open(bytes.to_vec()).expect("the fixture opens");
    let v = session
        .variables()
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .clone();
    let (y, x) = (
        v.detected_y_dim.expect("a y axis"),
        v.detected_x_dim.expect("an x axis"),
    );
    session
        .decode_slice(
            v.index,
            y,
            x,
            &vec![0; v.dims.len()],
            &DecodeOptions::default(),
        )
        .expect("the slice decodes")
}

/// A layer on a field's source view, through both host calls, which must
/// agree.
fn projected(field: &Field, flip_y: bool, (latlon, rings): &Lines) -> ProjectedPolylines {
    let source = field.source();
    let mut options = RenderOptions::new("source", "nearest");
    options.flip_y = flip_y;
    let free = overlay_polylines(&source, &options, latlon, rings).expect("the layer projects");
    let geometry = Arc::new((*source.geometry.as_ref().expect("placed")).clone());
    let kept = WarpMemo::new()
        .overlay_polylines(&geometry, &source, &options, latlon, rings)
        .expect("the layer projects");
    assert_eq!(kept, free, "WarpMemo and render disagree");
    free
}

/// How far a pixel coordinate may sit from where it should and still be there.
const TOL: f64 = 1e-6;

/// The runs of a projection, as vertex lists.
fn runs(out: &ProjectedPolylines) -> Vec<&[[f64; 2]]> {
    let vertices = out.xy.as_chunks::<2>().0;
    let mut start = 0usize;
    out.seg_lengths
        .iter()
        .map(|&n| {
            let run = &vertices[start..start + n as usize];
            start += n as usize;
            run
        })
        .collect()
}

/// Every vertex is on the picture, `0..=width` by `0..=height`.
fn assert_on_picture(what: &str, out: &ProjectedPolylines, (w, h): (f64, f64)) {
    for &[x, y] in out.xy.as_chunks::<2>().0 {
        assert!(
            (-TOL..=w + TOL).contains(&x) && (-TOL..=h + TOL).contains(&y),
            "{what}: ({x}, {y}) is off the {w} × {h} picture"
        );
    }
}

/// Whether a point is within `tol` pixels of the picture's border.
fn on_border([x, y]: [f64; 2], (w, h): (f64, f64), tol: f64) -> bool {
    x.abs() < tol || (x - w).abs() < tol || y.abs() < tol || (y - h).abs() < tol
}

/// How close to the border a lookup grid's line ends. Its position within a
/// cell comes from that cell's own shape, so it steps a little where the
/// nearest cell changes, and the edge is found at such a step.
const LOOKUP_TOL: f64 = 0.01;

/// Where each line's own first and last vertex is drawn, for those on the
/// picture: the first vertex of its first segment and the last of its last,
/// each projected on its own.
fn line_ends(field: &Field, flip_y: bool, (latlon, rings): &Lines) -> Vec<[f64; 2]> {
    let mut ends = Vec::new();
    let mut start = 0usize;
    for &n in rings {
        let n = n as usize;
        let vertex = |k: usize| [latlon[2 * (start + k)], latlon[2 * (start + k) + 1]];
        for (a, b, keep_first) in [(0, 1, true), (n - 2, n - 1, false)] {
            let segment = [vertex(a), vertex(b)].concat();
            let out = projected(field, flip_y, &(segment, vec![2]));
            let pieces = runs(&out);
            let end = if keep_first {
                pieces.first().map(|run| run[0])
            } else {
                pieces.last().map(|run| run[run.len() - 1])
            };
            ends.extend(end);
        }
        start += n;
    }
    ends
}

/// The smallest and largest `x` drawn.
fn x_extent(out: &ProjectedPolylines) -> (f64, f64) {
    out.xy
        .as_chunks::<2>()
        .0
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &[x, _]| {
            (lo.min(x), hi.max(x))
        })
}

#[test]
fn coastlines_on_the_coarse_latlon_source_view_reach_both_side_edges() {
    // The case from the owner's pass: the North African and Turkish coasts
    // ended about a cell before the right edge. On the old code the coastline
    // spanned x 0.05 to 14.85 of 16, and so did every other layer: none passed
    // 15, the last column's grid point.
    let field = message(LATLON);
    let source = field.source();
    let size = (f64::from(source.ni), f64::from(source.nj));
    assert_eq!((source.ni, source.nj), (16, 31), "the fixture's grid");
    for flip_y in [false, true] {
        for (layer, lines) in layers() {
            let what = format!("{layer} (flip_y {flip_y})");
            let out = projected(&field, flip_y, &lines);
            assert!(!out.seg_lengths.is_empty(), "{what}: the layer draws");
            assert_on_picture(&what, &out, size);
            if matches!(layer, "coastline" | "borders" | "graticule") {
                // These cross both side edges on this grid.
                let (lo, hi) = x_extent(&out);
                assert!(
                    lo.abs() < TOL && (hi - size.0).abs() < TOL,
                    "{what}: x runs {lo} to {hi}, not the picture's 0 to {}",
                    size.0
                );
            }
        }
    }
}

#[test]
fn every_graticule_line_runs_to_the_pictures_edge() {
    // Each graticule line runs past every side of these grids, so each piece
    // drawn starts and ends on the picture's border. On the old code a piece
    // stopped at the outer grid points, half a cell in, and short of them by
    // up to a segment where it left the grid between two vertices. The same
    // holds for a formula grid, a projected one and a lookup grid.
    for (what, field, tol) in [
        ("regular_latlon_surface.grib2", message(LATLON), TOL),
        ("hrrr_complex_spd_lambert.grib2", message(HRRR), TOL),
        ("mirs_swath_n21.nc", variable(MIRS, "TPW"), LOOKUP_TOL),
    ] {
        let source = field.source();
        let size = (f64::from(source.ni), f64::from(source.nj));
        for flip_y in [false, true] {
            let what = format!("{what} (flip_y {flip_y})");
            let lines = graticule();
            let out = projected(&field, flip_y, &lines);
            assert!(!out.seg_lengths.is_empty(), "{what}: the graticule draws");
            assert_on_picture(&what, &out, size);
            // The swath passes over a pole, where a meridian's own end, a
            // degree short of it, is on the picture. Those ends are kept.
            let own_ends = line_ends(&field, flip_y, &lines);
            let ends_well = |p: [f64; 2]| {
                on_border(p, size, tol)
                    || own_ends
                        .iter()
                        .any(|e| (e[0] - p[0]).abs() < TOL && (e[1] - p[1]).abs() < TOL)
            };
            for run in runs(&out) {
                let (first, last) = (run[0], run[run.len() - 1]);
                assert!(
                    ends_well(first) && ends_well(last),
                    "{what}: a graticule piece runs from {first:?} to {last:?}, \
                     inside the {} × {} picture",
                    size.0,
                    size.1
                );
            }
        }
    }
}

#[test]
fn every_layer_stays_on_the_picture_of_a_projected_and_a_lookup_grid() {
    for (what, field) in [
        ("hrrr_complex_spd_lambert.grib2", message(HRRR)),
        ("mirs_swath_n21.nc", variable(MIRS, "TPW")),
    ] {
        let source = field.source();
        let size = (f64::from(source.ni), f64::from(source.nj));
        for (layer, lines) in layers() {
            let out = projected(&field, false, &lines);
            assert_on_picture(&format!("{what} {layer}"), &out, size);
        }
    }
}

#[test]
fn a_grid_point_is_drawn_in_the_middle_of_its_cell() {
    // A line through two grid points of the formula grid lands on the middle
    // of their pixels, as the arrow tails do; on the old code both were on the
    // pixels' top-left corners, half a cell from the value under them.
    let field = message(LATLON);
    let source = field.source();
    let geometry = source.geometry.as_ref().expect("placed");
    let (ni, nj) = (source.ni, source.nj);
    let flip = source.scan.flips_source_rows(false);
    let middle = |i: u32, j: u32| {
        let row = if flip { nj - 1 - j } else { j };
        (f64::from(i) + 0.5, f64::from(row) + 0.5)
    };
    for (a, b) in [
        ((3, 4), (3, 9)),
        ((0, 0), (5, 0)),
        ((ni - 1, nj - 1), (ni - 1, nj - 6)),
    ] {
        let (pa, pb) = (
            geometry.forward(a.0, a.1).expect("placed"),
            geometry.forward(b.0, b.1).expect("placed"),
        );
        let out = projected(&field, false, &(vec![pa.0, pa.1, pb.0, pb.1], vec![2]));
        let drawn = out.xy.as_chunks::<2>().0;
        for (got, want) in [(drawn[0], middle(a.0, a.1)), (drawn[1], middle(b.0, b.1))] {
            assert!(
                (got[0] - want.0).abs() < TOL && (got[1] - want.1).abs() < TOL,
                "drawn at {got:?}, its cell's middle is {want:?}"
            );
        }
    }

    // Arrows, one per cell at a spacing of one, start in their cells' middles.
    let u: Vec<Option<f64>> = vec![Some(1.0); (ni * nj) as usize];
    let options = RenderOptions::new("source", "nearest");
    let mut vectors = VectorOptions::new();
    vectors.spacing = Some(1);
    let arrows = vector_polylines(&source, &u, &source, &u, &options, &vectors)
        .expect("the arrows are drawn");
    let tails: Vec<[f64; 2]> = arrows
        .runs
        .xy
        .as_chunks::<10>()
        .0
        .iter()
        .map(|run| [run[0], run[1]])
        .collect();
    assert_eq!(tails.len(), (ni * nj) as usize, "an arrow per cell");
    for (k, tail) in tails.iter().enumerate() {
        let (i, j) = (k as u32 % ni, k as u32 / ni);
        let want = middle(i, j);
        assert!(
            (tail[0] - want.0).abs() < TOL && (tail[1] - want.1).abs() < TOL,
            "the arrow for ({i}, {j}) starts at {tail:?}, not its cell's middle {want:?}"
        );
    }
}
