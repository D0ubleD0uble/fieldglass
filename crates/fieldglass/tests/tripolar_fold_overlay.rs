//! Overlays on a tripolar grid's source view do not draw chords along the fold
//! (#1430).
//!
//! The RTOFS tripolar mesh folds along its last row, which lies on the
//! meridians 106°W and 74°E from 47°N to the pole: the last row is the row below
//! it in reverse, so column `i` of one meets column `ni - 1 - i` of the other.
//! A coastline that crosses either meridian there has its two vertices on
//! opposite halves of those rows, and on the source projection, which lays the
//! grid straight into pixels, they land by the same edge more than a thousand
//! pixels apart. Joined, they made a straight line along the edge of the
//! picture, across the no-data region by the fold. Nothing caught it: the jump
//! is less than half the raster width, which is the only seam the source
//! projection tested for.
//!
//! The fixture is the full width of the file's last 28 rows, so every 1:110m
//! coastline segment that crosses the fold is in it with both ends, and the
//! test goes through `render::overlay_polylines`, the call both hosts make,
//! with the layers the extension bundles. It rasterises what comes back and
//! counts the pixels on the rows next to the fold. Before the fix the
//! coastline alone lit 574 of them and the graticule 361; now each layer lights
//! only the short pieces that really do reach the fold.

#![cfg(all(feature = "netcdf", feature = "render"))]

use std::collections::BTreeSet;

use fieldglass::{DecodeOptions, PlacedSlice, RenderOptions, Session};

/// RTOFS rows 3270..3298, every column: both halves of the fold.
const FOLD: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_fold.nc");
/// The 200 × 260 Arctic window around one of the bipolar patch's poles. Its
/// last row is on the fold too, but it holds only one half of it.
const ARCTIC: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc");

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

/// Rows next to the fold that are counted.
const EDGE_ROWS: u32 = 3;

/// Lit pixels a layer may leave on those rows: the pieces of line that really
/// do reach the fold from either side. The coastline, the busiest layer, now
/// leaves 59 on `FOLD`; a single chord across the fold leaves several hundred.
const MAX_EDGE_PIXELS: usize = 100;

/// Flat `[lat, lon, …]` and per-line vertex counts, as `projectOverlay` takes
/// them, from a bundled asset's `[lon, lat, …]` lines.
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

/// The extension's default graticule (`buildGraticule(30)`): meridians every
/// 30° from pole to pole, parallels at 0, ±30 and ±60, a vertex every 2°.
fn graticule() -> Lines {
    let (mut latlon, mut rings) = (Vec::new(), Vec::new());
    for k in 0..12 {
        let lon = -180.0 + 30.0 * f64::from(k);
        for s in 0..=90 {
            latlon.extend_from_slice(&[-90.0 + 2.0 * f64::from(s), lon]);
        }
        rings.push(91);
    }
    for lat in [0.0, 30.0, -30.0, 60.0, -60.0] {
        for s in 0..=180 {
            latlon.extend_from_slice(&[lat, -180.0 + 2.0 * f64::from(s)]);
        }
        rings.push(181);
    }
    (latlon, rings)
}

/// A file's `ice_coverage`, decoded on its own axes, and where its cells are.
fn ice_coverage(bytes: &[u8]) -> (fieldglass::api::Field, PlacedSlice) {
    let session = Session::open(bytes.to_vec()).expect("the fixture opens");
    let v = session
        .variables()
        .iter()
        .find(|v| v.name == "ice_coverage")
        .expect("ice_coverage is listed")
        .clone();
    let (y, x) = (
        v.detected_y_dim.expect("a y axis"),
        v.detected_x_dim.expect("an x axis"),
    );
    let field = session
        .decode_slice(
            v.index,
            y,
            x,
            &vec![0; v.dims.len()],
            &DecodeOptions::default(),
        )
        .expect("the slice decodes");
    let placed = session
        .place_slice(v.index, y, x)
        .expect("the slice is placed");
    (field, placed)
}

/// Every pixel the runs stroke on rows `rows`, by stepping each segment one
/// pixel at a time along its longer axis.
fn lit_pixels(xy: &[f64], seg_lengths: &[u32], rows: std::ops::Range<i64>) -> BTreeSet<(i64, i64)> {
    let mut lit = BTreeSet::new();
    let mut start = 0usize;
    for &n in seg_lengths {
        let run = &xy[2 * start..2 * (start + n as usize)];
        for w in run.as_chunks::<2>().0.windows(2) {
            let ([x0, y0], [x1, y1]) = (w[0], w[1]);
            let steps = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as usize;
            for t in 0..=steps {
                let f = t as f64 / steps as f64;
                // Pixel `k` spans `k..k + 1` (#1464).
                let px = (x0 + f * (x1 - x0)).floor() as i64;
                let py = (y0 + f * (y1 - y0)).floor() as i64;
                if rows.contains(&py) {
                    lit.insert((px, py));
                }
            }
        }
        start += n as usize;
    }
    lit
}

/// Flat `[lat, lon, …]` and per-line vertex counts.
type Lines = (Vec<f64>, Vec<u32>);

/// Count each layer's lit pixels on the rows next to the fold, on the raster
/// the source projection draws `bytes` onto.
fn edge_pixels_per_layer(bytes: &[u8]) -> Vec<(&'static str, usize)> {
    let (field, placed) = ice_coverage(bytes);
    let source = field.source();
    let (ni, nj) = (source.ni, source.nj);
    let options = RenderOptions::new("source", "nearest");

    // Which edge the fold is on: the render may flip rows to face north up, so
    // read it off the same call rather than restating that rule. The probe is
    // a short line from a first-row cell's centre one row in, since the first
    // row is the opposite edge and, unlike the last, holds no repeated cells.
    let forward = |i: u32, j: u32| placed.geometry().forward(i, j).expect("a placed cell");
    let (a, b) = (forward(ni / 8, 0), forward(ni / 8, 1));
    let probe =
        fieldglass::render::overlay_polylines(&source, &options, &[a.0, a.1, b.0, b.1], &[2])
            .expect("the probe line projects");
    let first_row_y = probe.xy[1].floor() as i64;
    let rows = if first_row_y == 0 {
        i64::from(nj - EDGE_ROWS)..i64::from(nj)
    } else {
        assert_eq!(
            first_row_y,
            i64::from(nj) - 1,
            "the first row is an edge row"
        );
        0..i64::from(EDGE_ROWS)
    };

    let mut layers: Vec<(&'static str, Lines)> = LAYERS
        .iter()
        .map(|&(name, json)| (name, vector_layer(json)))
        .collect();
    layers.push(("graticule", graticule()));
    layers
        .into_iter()
        .map(|(name, (latlon, rings))| {
            let out = fieldglass::render::overlay_polylines(&source, &options, &latlon, &rings)
                .expect("the layer projects");
            (
                name,
                lit_pixels(&out.xy, &out.seg_lengths, rows.clone()).len(),
            )
        })
        .collect()
}

#[test]
fn the_fixture_holds_both_halves_of_the_fold() {
    // Without this the test below could pass on a window that has no fold to
    // jump across. The last row is the row below it in reverse: cell
    // `(i, nj - 1)` is cell `(ni - 1 - i, nj - 2)`. Compared as positions, to
    // the round trip through the index's unit vectors, since the file writes
    // the same place as longitudes 720° apart.
    let (field, placed) = ice_coverage(FOLD);
    let (ni, nj) = (field.source().ni, field.source().nj);
    for i in [0, 500, 1600, 2000, 2200, 2700, 4000] {
        let (lat_a, lon_a) = placed.geometry().forward(i, nj - 1).expect("placed");
        let (lat_b, lon_b) = placed
            .geometry()
            .forward(ni - 1 - i, nj - 2)
            .expect("placed");
        let dlon = (lon_a - lon_b).rem_euclid(360.0);
        assert!(
            (lat_a - lat_b).abs() < 1e-9 && dlon.min(360.0 - dlon) < 1e-9,
            "({i}, {}) at ({lat_a}, {lon_a}) is not ({}, {}) at ({lat_b}, {lon_b})",
            nj - 1,
            ni - 1 - i,
            nj - 2
        );
    }
}

#[test]
fn no_overlay_layer_draws_a_chord_along_the_fold() {
    for (file, bytes) in [
        ("rtofs_tripolar_fold.nc", FOLD),
        ("rtofs_tripolar_arctic.nc", ARCTIC),
    ] {
        for (layer, lit) in edge_pixels_per_layer(bytes) {
            assert!(
                lit <= MAX_EDGE_PIXELS,
                "{file}: the {layer} layer lights {lit} pixels on the {EDGE_ROWS} rows by \
                 the fold (at most {MAX_EDGE_PIXELS}): a segment is joined across it"
            );
        }
    }
}
