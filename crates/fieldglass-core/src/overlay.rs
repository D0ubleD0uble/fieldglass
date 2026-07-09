//! Project geographic polylines (coastlines, graticule, user shapes) onto a
//! warped raster for the 2-D render panel's overlay layer (#72).
//!
//! The caller supplies polylines as flat `(lat, lon)` vertices plus the
//! vertex count of each input ring, and a [`ForwardMap`] describing the
//! same raster the warp painted. We run every vertex through the target's
//! forward map ([`ForwardMap::lonlat_to_pixel`]) and emit the *visible*
//! pixel-space runs, ready for the webview to stroke on its overlay canvas.
//!
//! A single input ring becomes zero or more output runs: we break a run
//! whenever a vertex leaves the projection's visible domain (the back of an
//! orthographic globe, past a polar disc's equator rim) and — only for targets
//! that wrap longitude (`wraps_antimeridian`: the lat/lon-box targets and the
//! source projection) — whenever consecutive vertices jump more than half the
//! raster width apart, the antimeridian / grid seam where a polyline wraps from
//! one edge to the other and must not be drawn as a streak across the map. The
//! azimuthal targets have no seam, so they pass `wraps_antimeridian = false`.
//! Runs whose bounding box never reaches the viewport, or that are left with
//! fewer than two vertices, are dropped (nothing visible to stroke).
//!
//! This module is deliberately projection-agnostic: it knows nothing about
//! "coastline" vs "graticule" vs a user-drawn shape — they are all just
//! `(lat, lon)` rings. The render contract (`RenderOptions`/`RenderedGrid`)
//! is untouched; this is an additive, geometry-only path.

use crate::projection::GridIndex;
use crate::warp::ForwardMap;

/// Overlay forward map for the **source projection** — the unwarped grid shown
/// as-is. `paint_source` lays source grid point `(i, j)` straight into output
/// pixel `(i, j)`, so the warp's own inverse map (geographic `(lat, lon)` →
/// fractional grid index) *is* the forward pixel map here. Wrapping it as a
/// [`ForwardMap`] lets the overlay layer (#72) project coastlines /
/// graticule onto the source projection too — reusing [`project_polylines`]'s
/// visibility + antimeridian-seam splitting unchanged, and working for every
/// grid type (latlon, gaussian, lambert, polar stereographic) since each
/// already supplies an inverse map.
///
/// Holds a borrowed inverse closure — a shared reference, hence `Copy`; the
/// borrow only needs to outlive the synchronous `project_polylines` call.
#[derive(Clone, Copy)]
pub struct SourceOverlayTarget<'a> {
    inverse: &'a dyn Fn(f64, f64) -> Option<GridIndex>,
}

impl<'a> SourceOverlayTarget<'a> {
    /// Wrap the warp's inverse map (`lat, lon →` fractional grid index) as the
    /// source projection's overlay forward map.
    pub fn new(inverse: &'a dyn Fn(f64, f64) -> Option<GridIndex>) -> Self {
        Self { inverse }
    }
}

impl ForwardMap for SourceOverlayTarget<'_> {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        (self.inverse)(lat, lon).map(|GridIndex { i, j }| (i, j))
    }
}

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
/// `wraps_antimeridian` controls the seam split: pass `true` for the lat/lon-box
/// targets (equirectangular, Web Mercator) and the source projection, where a
/// polyline can jump edge-to-edge across the antimeridian / grid seam and must
/// be broken there; pass `false` for the azimuthal targets (orthographic, polar
/// stereographic), which have no seam — their only run break is leaving the
/// visible disc (`None`), and a straight chord between two visible points always
/// stays on the convex hemisphere. This stops a sparse polyline (e.g. a
/// user-drawn shape) from being wrongly split when two distant points happen to
/// land more than half a raster apart on a disc.
///
/// `latlon` is flat `[lat, lon, lat, lon, …]`; `ring_lengths[k]` is the
/// vertex count of ring `k`. See the module docs for the run-splitting rules.
pub fn project_polylines<P: ForwardMap>(
    prepared: &P,
    width: u32,
    height: u32,
    flip_y: bool,
    wraps_antimeridian: bool,
    latlon: &[f64],
    ring_lengths: &[u32],
) -> ProjectedPolylines {
    let mut out = ProjectedPolylines::default();
    if width == 0 || height == 0 {
        return out;
    }
    let (w, h) = (width as f64, height as f64);
    // Consecutive vertices farther apart than this in x are treated as an
    // antimeridian seam (only when `wraps_antimeridian`) rather than a real
    // edge: split there.
    let wrap_threshold = w / 2.0;
    let y_top = h - 1.0;
    // A run is kept only if its bounding box overlaps the viewport expanded by
    // one raster on each side. Testing the *box* (not just individual vertices)
    // keeps a run whose endpoints both fall outside the margin but whose chord
    // crosses the screen — a long coastline segment on a heavily-zoomed window —
    // while still culling runs that project entirely off to one side, so we
    // don't marshal and stroke thousands of invisible points.
    let keeps = |run: &[f64]| -> bool {
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut k = 0;
        while k + 1 < run.len() {
            let (x, y) = (run[k], run[k + 1]);
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            k += 2;
        }
        min_x <= 2.0 * w && max_x >= -w && min_y <= 2.0 * h && max_y >= -h
    };

    let mut cursor = 0usize; // Index into `latlon`, in floats (2 per vertex).
    let mut run: Vec<f64> = Vec::new();

    let flush = |run: &mut Vec<f64>, out: &mut ProjectedPolylines| {
        // Two floats per vertex; a run needs ≥ 2 vertices to stroke and its
        // bounding box must reach the viewport to be worth keeping.
        if run.len() >= 4 && keeps(run) {
            out.seg_lengths.push((run.len() / 2) as u32);
            out.xy.append(run);
        } else {
            run.clear();
        }
    };

    for &len in ring_lengths {
        run.clear();
        let mut prev_x: Option<f64> = None;
        for _ in 0..len {
            // Stop the ring if `ring_lengths` overruns `latlon` (or a final
            // `lat` has no paired `lon`) rather than fabricating a vertex.
            let (Some(&lat), Some(&lon)) = (latlon.get(cursor), latlon.get(cursor + 1)) else {
                break;
            };
            cursor += 2;

            match prepared.lonlat_to_pixel(lat, lon) {
                None => {
                    // Left the visible domain — end the current run.
                    flush(&mut run, &mut out);
                    prev_x = None;
                }
                Some((x, mut y)) => {
                    if flip_y {
                        y = y_top - y;
                    }
                    if wraps_antimeridian
                        && let Some(px) = prev_x
                        && (x - px).abs() > wrap_threshold
                    {
                        // Antimeridian seam — break before this vertex.
                        flush(&mut run, &mut out);
                    }
                    run.push(x);
                    run.push(y);
                    prev_x = Some(x);
                }
            }
        }
        flush(&mut run, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warp::{Mollweide, Orthographic, TargetProjection, TargetRaster};

    /// A full-globe equirectangular raster for box-target tests.
    fn global_equirect(width: u32, height: u32) -> impl ForwardMap {
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
        let out = project_polylines(&prep, 361, 181, false, true, &latlon, &ring_lengths);
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
        let out = project_polylines(&prep, 361, 181, false, true, &latlon, &[4]);
        // The wrap breaks the 4-vertex ring into two 2-vertex runs.
        assert_eq!(out.seg_lengths, vec![2, 2]);
    }

    #[test]
    fn mollweide_splits_at_the_seam_but_keeps_wide_visible_runs() {
        // Mollweide wraps longitude about its centre, so a polyline hopping the
        // ±180° seam jumps rim-to-rim and must break there (wraps_antimeridian =
        // true), exactly like the lat/lon-box targets.
        let prep = Mollweide::new(360, 180, 0.0).prepare();
        let seam = [0.0, 160.0, 0.0, 170.0, 0.0, -170.0, 0.0, -160.0];
        let out = project_polylines(&prep, 360, 180, false, true, &seam, &[4]);
        assert_eq!(out.seg_lengths, vec![2, 2], "seam crossing must split");
        // A genuine 160°-wide equatorial segment (no seam crossing) spans less
        // than half the width, so it must stay one run rather than false-split.
        let wide = [0.0, -80.0, 0.0, 80.0];
        let out = project_polylines(&prep, 360, 180, false, true, &wide, &[2]);
        assert_eq!(out.seg_lengths, vec![2], "wide visible run must not split");
    }

    #[test]
    fn flip_y_mirrors_the_output_row() {
        let prep = global_equirect(361, 181);
        let latlon = [45.0, 0.0];
        let unflipped = project_polylines(&prep, 361, 181, false, true, &latlon, &[1]);
        // A single-vertex run is dropped, so compare a 2-vertex run instead.
        let latlon2 = [45.0, 0.0, 45.0, 1.0];
        let plain = project_polylines(&prep, 361, 181, false, true, &latlon2, &[2]);
        let flipped = project_polylines(&prep, 361, 181, true, true, &latlon2, &[2]);
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
        // Azimuthal target: no antimeridian seam, so `wraps_antimeridian` is
        // false — runs break only on leaving the disc (`None`).
        let out = project_polylines(&prep, 101, 101, false, false, &latlon, &[5]);
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
        let culled = project_polylines(&prep, 80, 40, false, true, &asia, &[3]);
        assert!(
            culled.xy.is_empty() && culled.seg_lengths.is_empty(),
            "off-window polyline should be culled"
        );
        // A polyline inside the window is still kept.
        let here = [40.0, -110.0, 41.0, -109.0, 42.0, -108.0];
        let kept = project_polylines(&prep, 80, 40, false, true, &here, &[3]);
        assert_eq!(kept.seg_lengths, vec![3]);
    }

    #[test]
    fn drops_runs_shorter_than_two_vertices() {
        let prep = global_equirect(361, 181);
        let out = project_polylines(&prep, 361, 181, false, true, &[0.0, 0.0], &[1]);
        assert!(out.xy.is_empty() && out.seg_lengths.is_empty());
    }

    #[test]
    fn source_target_projects_through_the_inverse_map() {
        use crate::projection::{LatLonParams, latlon_inverse};
        // A global 1° lat/lon grid: 361×181, north-up. Its inverse lands a
        // point at px = lon + 180, py = 90 - lat — exactly the source raster's
        // grid-index → pixel identity.
        let p = LatLonParams {
            ni: 361,
            nj: 181,
            lat_first: 90.0,
            lon_first: -180.0,
            lat_last: -90.0,
            lon_last: 180.0,
        };
        let inverse = move |lat: f64, lon: f64| latlon_inverse(&p, lat, lon);
        let target = SourceOverlayTarget::new(&inverse);
        let out = project_polylines(
            &target,
            361,
            181,
            false,
            true,
            &[45.0, 0.0, 45.0, 10.0],
            &[2],
        );
        assert_eq!(out.seg_lengths, vec![2]);
        assert!((out.xy[0] - 180.0).abs() < 1e-6, "lon 0 → px {}", out.xy[0]);
        assert!((out.xy[1] - 45.0).abs() < 1e-6, "lat 45 → py {}", out.xy[1]);
        assert!(
            (out.xy[2] - 190.0).abs() < 1e-6,
            "lon 10 → px {}",
            out.xy[2]
        );
        // A vertex off the grid (past the pole) inverts to None and breaks the
        // run; the lone remaining vertex is dropped.
        let off = project_polylines(
            &target,
            361,
            181,
            false,
            true,
            &[45.0, 0.0, 95.0, 0.0],
            &[2],
        );
        assert!(off.xy.is_empty(), "vertex past the pole leaves the grid");
    }

    #[test]
    fn degenerate_raster_returns_empty() {
        let prep = global_equirect(0, 181);
        let out = project_polylines(&prep, 0, 181, false, true, &[0.0, 0.0, 1.0, 1.0], &[2]);
        assert_eq!(out, ProjectedPolylines::default());
    }

    #[test]
    fn regional_window_west_edge_stays_continuous() {
        // A narrow window lon -120..-100 (80 px wide). A vertex 1° WEST of the
        // edge must map to a small *negative* pixel (continuous past the edge),
        // not wrap to the far right — so the segment clipping in from the west
        // is kept as one run, not split at the edge.
        let prep = TargetRaster {
            width: 80,
            height: 40,
            lat_max: 50.0,
            lat_min: 30.0,
            lon_min: -120.0,
            lon_max: -100.0,
        }
        .prepare();
        let crossing = [40.0, -121.0, 40.0, -119.0];
        let out = project_polylines(&prep, 80, 40, false, true, &crossing, &[2]);
        assert_eq!(out.seg_lengths, vec![2], "edge-crossing run must not split");
        assert!(
            out.xy[0] < 0.0,
            "west-of-edge vertex → negative px, got {}",
            out.xy[0]
        );
        assert!(
            out.xy[2] > 0.0,
            "in-window vertex → positive px, got {}",
            out.xy[2]
        );
    }

    /// A direct geographic→pixel identity (`lon → x`, `lat → y`) so a test can
    /// place run vertices at exact pixel coordinates.
    #[derive(Clone, Copy)]
    struct IdentityPixels;
    impl ForwardMap for IdentityPixels {
        fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
            Some((lon, lat))
        }
    }

    #[test]
    fn keeps_a_chord_that_crosses_the_viewport_with_both_endpoints_off_margin() {
        // 100×100 raster; keep-margin is x,y ∈ [-100, 200]. Both endpoints fall
        // outside the margin (x = -150 and x = 250) but the chord crosses the
        // viewport, so the run must be kept (not culled vertex-by-vertex). Use a
        // non-wrapping target so the > w/2 jump isn't treated as a seam.
        let crossing = [50.0, -150.0, 50.0, 250.0];
        let kept = project_polylines(&IdentityPixels, 100, 100, false, false, &crossing, &[2]);
        assert_eq!(kept.seg_lengths, vec![2], "viewport-crossing chord kept");
        // A run entirely off to one side is still culled.
        let off = [50.0, 250.0, 50.0, 300.0];
        let culled = project_polylines(&IdentityPixels, 100, 100, false, false, &off, &[2]);
        assert!(culled.xy.is_empty(), "run off to one side is culled");
    }

    #[test]
    fn azimuthal_target_does_not_split_far_apart_visible_points() {
        // Two visible front-hemisphere points project near opposite edges of the
        // disc (> half the raster apart). With `wraps_antimeridian = false` the
        // azimuthal target keeps them as one chord instead of mistaking the gap
        // for an antimeridian wrap (which would split + drop both as singletons).
        let prep = Orthographic::new(100, 100, 0.0, 0.0).prepare();
        let wide = [0.0, -80.0, 0.0, 80.0];
        let out = project_polylines(&prep, 100, 100, false, false, &wide, &[2]);
        assert_eq!(out.seg_lengths, vec![2]);
    }

    #[test]
    fn truncated_input_does_not_fabricate_a_vertex() {
        // `ring_lengths` claims 2 vertices but `latlon` holds only 1.5 pairs.
        // The dangling `lat` (30.0) has no paired `lon`, so it is dropped rather
        // than fabricating a (30, 0) vertex on the prime meridian.
        let prep = global_equirect(361, 181);
        let out = project_polylines(&prep, 361, 181, false, true, &[10.0, 20.0, 30.0], &[2]);
        assert!(
            out.xy.is_empty(),
            "truncated tail must not fabricate a vertex"
        );
    }
}
