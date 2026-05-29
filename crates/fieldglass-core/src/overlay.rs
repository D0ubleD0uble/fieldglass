//! Project geographic polylines (coastlines, graticule, user shapes) onto a
//! warped raster for the 2-D render panel's overlay layer (#72).
//!
//! The caller supplies polylines as flat `(lat, lon)` vertices plus the
//! vertex count of each input ring, and a [`PreparedTarget`] describing the
//! same raster the warp painted. We run every vertex through the target's
//! forward map ([`PreparedTarget::lonlat_to_pixel`]) and emit the *visible*
//! pixel-space runs, ready for the webview to stroke on its overlay canvas.
//!
//! A single input ring becomes zero or more output runs: we break a run
//! whenever a vertex leaves the projection's visible domain (the back of an
//! orthographic globe, past a polar disc's equator rim) or whenever
//! consecutive vertices jump more than half the raster width apart — the
//! antimeridian seam on the lat/lon-box targets, where a polyline wraps from
//! one edge to the other and must not be drawn as a streak across the map.
//! Runs left with fewer than two vertices are dropped (nothing to stroke).
//!
//! This module is deliberately projection-agnostic: it knows nothing about
//! "coastline" vs "graticule" vs a user-drawn shape — they are all just
//! `(lat, lon)` rings. The render contract (`RenderOptions`/`RenderedGrid`)
//! is untouched; this is an additive, geometry-only path.

use crate::warp::PreparedTarget;

/// Projected overlay geometry in output pixel space. `xy` is flat
/// `[x0, y0, x1, y1, …]`; `seg_lengths` gives the vertex count of each
/// visible run, so `seg_lengths.iter().sum::<u32>() * 2 == xy.len()`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectedPolylines {
    pub xy: Vec<f64>,
    pub seg_lengths: Vec<u32>,
}

/// Project `(lat, lon)` rings onto the raster described by `prepared`, of
/// size `width × height`. When `flip_y` is set the output Y is flipped to
/// match a vertically-flipped render (the `paint_grid_rgba` flip).
///
/// `latlon` is flat `[lat, lon, lat, lon, …]`; `ring_lengths[k]` is the
/// vertex count of ring `k`. See the module docs for the run-splitting rules.
pub fn project_polylines<P: PreparedTarget>(
    prepared: &P,
    width: u32,
    height: u32,
    flip_y: bool,
    latlon: &[f64],
    ring_lengths: &[u32],
) -> ProjectedPolylines {
    let mut out = ProjectedPolylines::default();
    if width == 0 || height == 0 {
        return out;
    }
    let (w, h) = (width as f64, height as f64);
    // Consecutive vertices farther apart than this in x are treated as an
    // antimeridian wrap (box targets) rather than a real edge: split there.
    // The azimuthal targets never wrap, and their dense vertices stay well
    // under this bound, so a single threshold serves both.
    let wrap_threshold = w / 2.0;
    let y_top = h - 1.0;
    // A run is kept only if at least one vertex lands within one raster of the
    // viewport. This culls polylines that project far off-canvas (e.g. the
    // whole far side of the world behind a small regional window) so we don't
    // marshal and stroke thousands of invisible points; the one-raster margin
    // keeps segments that merely clip the edge between two sampled vertices.
    let on_screen = |x: f64, y: f64| x >= -w && x <= 2.0 * w && y >= -h && y <= 2.0 * h;

    let mut cursor = 0usize; // Index into `latlon`, in floats (2 per vertex).
    let mut run: Vec<f64> = Vec::new();

    let flush = |run: &mut Vec<f64>, visible: &mut bool, out: &mut ProjectedPolylines| {
        // Two floats per vertex; a run needs ≥ 2 vertices to stroke and at
        // least one on/near the viewport to be worth keeping.
        if run.len() >= 4 && *visible {
            out.seg_lengths.push((run.len() / 2) as u32);
            out.xy.append(run);
        } else {
            run.clear();
        }
        *visible = false;
    };

    for &len in ring_lengths {
        run.clear();
        let mut prev_x: Option<f64> = None;
        let mut visible = false;
        for _ in 0..len {
            // Guard against a ring_lengths total that overruns latlon.
            let Some(&lat) = latlon.get(cursor) else {
                break;
            };
            let lon = latlon.get(cursor + 1).copied().unwrap_or(0.0);
            cursor += 2;

            match prepared.lonlat_to_pixel(lat, lon) {
                None => {
                    // Left the visible domain — end the current run. (Both
                    // azimuthal domains are a convex hemisphere, so a straight
                    // chord between two *visible* vertices always stays on the
                    // disc — no extra mid-segment check is needed.)
                    flush(&mut run, &mut visible, &mut out);
                    prev_x = None;
                }
                Some((x, mut y)) => {
                    if flip_y {
                        y = y_top - y;
                    }
                    if let Some(px) = prev_x
                        && (x - px).abs() > wrap_threshold
                    {
                        // Antimeridian wrap — break before this vertex.
                        flush(&mut run, &mut visible, &mut out);
                    }
                    run.push(x);
                    run.push(y);
                    visible |= on_screen(x, y);
                    prev_x = Some(x);
                }
            }
        }
        flush(&mut run, &mut visible, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warp::{Orthographic, TargetProjection, TargetRaster};

    /// A full-globe equirectangular raster for box-target tests.
    fn global_equirect(width: u32, height: u32) -> impl PreparedTarget {
        TargetRaster {
            width,
            height,
            lat_max: 90.0,
            lat_min: -90.0,
            lon_min: -180.0,
            lon_max: 180.0,
        }
        .prepare()
    }

    #[test]
    fn projects_an_interior_polyline_to_one_run() {
        let prep = global_equirect(361, 181);
        // Three points well inside the map.
        let latlon = [10.0, -20.0, 12.0, -18.0, 14.0, -16.0];
        let ring_lengths = [3u32];
        let out = project_polylines(&prep, 361, 181, false, &latlon, &ring_lengths);
        assert_eq!(out.seg_lengths, vec![3]);
        assert_eq!(out.xy.len(), 6);
        // First vertex must match the target's own forward map.
        let (x0, y0) = prep.lonlat_to_pixel(10.0, -20.0).unwrap();
        assert!((out.xy[0] - x0).abs() < 1e-9 && (out.xy[1] - y0).abs() < 1e-9);
    }

    #[test]
    fn splits_a_run_at_the_antimeridian_seam() {
        let prep = global_equirect(361, 181);
        // A segment hopping the antimeridian: lon 170 → -170 wraps edge-to-edge.
        let latlon = [0.0, 160.0, 0.0, 170.0, 0.0, -170.0, 0.0, -160.0];
        let out = project_polylines(&prep, 361, 181, false, &latlon, &[4]);
        // The wrap breaks the 4-vertex ring into two 2-vertex runs.
        assert_eq!(out.seg_lengths, vec![2, 2]);
    }

    #[test]
    fn flip_y_mirrors_the_output_row() {
        let prep = global_equirect(361, 181);
        let latlon = [45.0, 0.0];
        let unflipped = project_polylines(&prep, 361, 181, false, &latlon, &[1]);
        // A single-vertex run is dropped, so compare a 2-vertex run instead.
        let latlon2 = [45.0, 0.0, 45.0, 1.0];
        let plain = project_polylines(&prep, 361, 181, false, &latlon2, &[2]);
        let flipped = project_polylines(&prep, 361, 181, true, &latlon2, &[2]);
        assert!(unflipped.xy.is_empty(), "single-vertex run is dropped");
        // y_flipped = (height-1) - y_plain.
        assert!((flipped.xy[1] - (180.0 - plain.xy[1])).abs() < 1e-9);
        assert!(
            (flipped.xy[0] - plain.xy[0]).abs() < 1e-9,
            "x unaffected by flip"
        );
    }

    #[test]
    fn breaks_runs_where_the_polyline_leaves_the_visible_disc() {
        // Orthographic centred on (0, 0): a ring crossing to the antipode side.
        let t = Orthographic::new(101, 101, 0.0, 0.0);
        let prep = t.prepare();
        // Front (lon 0), back (lon 180, off-disc), front again.
        let latlon = [0.0, -10.0, 0.0, 0.0, 0.0, 180.0, 0.0, 10.0, 0.0, 20.0];
        let out = project_polylines(&prep, 101, 101, false, &latlon, &[5]);
        // The off-disc vertex splits the ring; the back side is dropped.
        // Front run #1 = 2 verts (lon -10, 0); front run #2 = 2 verts (lon 10, 20).
        assert_eq!(out.seg_lengths, vec![2, 2]);
    }

    #[test]
    fn culls_runs_that_project_far_outside_a_regional_window() {
        // A narrow North-America window; a polyline over Asia projects many
        // raster-widths off to the side and must be culled, not marshalled.
        let prep = TargetRaster {
            width: 80,
            height: 40,
            lat_max: 50.0,
            lat_min: 30.0,
            lon_min: -120.0,
            lon_max: -100.0,
        }
        .prepare();
        let asia = [35.0, 100.0, 36.0, 101.0, 37.0, 102.0];
        let culled = project_polylines(&prep, 80, 40, false, &asia, &[3]);
        assert!(
            culled.xy.is_empty() && culled.seg_lengths.is_empty(),
            "off-window polyline should be culled"
        );
        // A polyline inside the window is still kept.
        let here = [40.0, -110.0, 41.0, -109.0, 42.0, -108.0];
        let kept = project_polylines(&prep, 80, 40, false, &here, &[3]);
        assert_eq!(kept.seg_lengths, vec![3]);
    }

    #[test]
    fn drops_runs_shorter_than_two_vertices() {
        let prep = global_equirect(361, 181);
        let out = project_polylines(&prep, 361, 181, false, &[0.0, 0.0], &[1]);
        assert!(out.xy.is_empty() && out.seg_lengths.is_empty());
    }

    #[test]
    fn degenerate_raster_returns_empty() {
        let prep = global_equirect(0, 181);
        let out = project_polylines(&prep, 0, 181, false, &[0.0, 0.0, 1.0, 1.0], &[2]);
        assert_eq!(out, ProjectedPolylines::default());
    }
}
