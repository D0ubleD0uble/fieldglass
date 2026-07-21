//! Marching-squares isoline extraction over a decoded field.
//!
//! Contours are the standard way to read a pressure or height field. The
//! extraction runs in **grid space** — the output is line segments in
//! fractional grid coordinates — so the caller can push those vertices through
//! the same forward geolocation + [`project_polylines`](crate::project_polylines)
//! path the coastline overlay uses, and contours then land correctly on every
//! target projection with no per-projection code here.
//!
//! A cell with any missing or non-finite corner contributes no segments, so a
//! contour breaks cleanly around a data hole rather than drawing a spurious line
//! across it.

/// A contour segment in fractional grid coordinates: two endpoints `(i, j)`
/// where `i ∈ [0, ni-1]` and `j ∈ [0, nj-1]`. A vertex sits on a cell edge
/// between two grid points, so its indices are generally fractional.
pub type GridSegment = [(f64, f64); 2];

/// One contour level and the line segments tracing it, in fractional grid
/// coordinates.
#[derive(Debug, Clone, PartialEq)]
pub struct ContourLevel {
    /// The field value this isoline traces.
    pub level: f64,
    /// Unconnected line segments (each a pair of grid-space endpoints). Left
    /// unlinked because the renderer strokes each independently and labels are
    /// out of scope; a masked region simply omits the cells it touches.
    pub segments: Vec<GridSegment>,
}

/// The crossing point of `level` on the edge between corner `a` (value `va`, at
/// grid position `pa`) and corner `b` (value `vb`, at `pb`). Linear along the
/// edge; a degenerate edge (`va == vb`, only reachable when both corners sit
/// exactly on the level) crosses at the midpoint.
fn interp(pa: (f64, f64), va: f64, pb: (f64, f64), vb: f64, level: f64) -> (f64, f64) {
    let denom = vb - va;
    let t = if denom == 0.0 {
        0.5
    } else {
        ((level - va) / denom).clamp(0.0, 1.0)
    };
    (pa.0 + (pb.0 - pa.0) * t, pa.1 + (pb.1 - pa.1) * t)
}

/// Extract isolines at each of `levels` from a row-major `ni × nj` field via
/// marching squares. Returns one [`ContourLevel`] per input level (in the same
/// order); a level with no crossings comes back with an empty `segments`.
///
/// A cell contributes segments only when all four of its corners are present
/// and finite, so contours break around masked or non-finite data.
pub fn contour_segments(
    values: &[Option<f64>],
    ni: usize,
    nj: usize,
    levels: &[f64],
) -> Vec<ContourLevel> {
    let mut out: Vec<ContourLevel> = levels
        .iter()
        .map(|&level| ContourLevel {
            level,
            segments: Vec::new(),
        })
        .collect();
    // A grid smaller than 2×2 has no cells to march.
    if ni < 2 || nj < 2 || values.len() < ni * nj {
        return out;
    }

    let finite_at =
        |i: usize, j: usize| -> Option<f64> { values[j * ni + i].filter(|v| v.is_finite()) };

    // Sort the levels once so each cell can binary-search only the band its four
    // corner values span — every other level gives case 0 or 15 (no crossing),
    // so a dense level set (`levels_by_interval` allows thousands) no longer
    // costs O(cells × levels). `total_cmp` is a total order, so NaN sorts to the
    // end, outside every finite `[cmin, cmax]` band; such a level yields no
    // segments, exactly as before (its `>=` comparisons made every case 0).
    // `order` maps a sorted position back to the caller's level index, so `out`
    // stays in input order.
    let mut order: Vec<usize> = (0..levels.len()).collect();
    order.sort_by(|&a, &b| levels[a].total_cmp(&levels[b]));
    let sorted_levels: Vec<f64> = order.iter().map(|&k| levels[k]).collect();

    for j in 0..nj - 1 {
        for i in 0..ni - 1 {
            // Corners: bl (i,j), br (i+1,j), tr (i+1,j+1), tl (i,j+1).
            let (Some(v_bl), Some(v_br), Some(v_tr), Some(v_tl)) = (
                finite_at(i, j),
                finite_at(i + 1, j),
                finite_at(i + 1, j + 1),
                finite_at(i, j + 1),
            ) else {
                // Any missing/non-finite corner → skip the whole cell.
                continue;
            };
            let bl = (i as f64, j as f64);
            let br = ((i + 1) as f64, j as f64);
            let tr = ((i + 1) as f64, (j + 1) as f64);
            let tl = (i as f64, (j + 1) as f64);

            // Only levels within the cell's corner range can cross it. Binary-
            // search that band in the sorted levels: `[lo, hi)` is every level
            // in `[cmin, cmax]`. (The `case == 0 || case == 15` guard below
            // still handles the band's own endpoints — `level == cmin` is a flat
            // case-15 skip — so the segments are identical to scanning all
            // levels, just far fewer comparisons.)
            let cmin = v_bl.min(v_br).min(v_tr).min(v_tl);
            let cmax = v_bl.max(v_br).max(v_tr).max(v_tl);
            let lo = sorted_levels.partition_point(|&l| l < cmin);
            let hi = sorted_levels.partition_point(|&l| l <= cmax);
            for si in lo..hi {
                let level = sorted_levels[si];
                // Corner-above bits: bl=1, br=2, tr=4, tl=8.
                let case = (v_bl >= level) as u8
                    | (((v_br >= level) as u8) << 1)
                    | (((v_tr >= level) as u8) << 2)
                    | (((v_tl >= level) as u8) << 3);
                if case == 0 || case == 15 {
                    continue;
                }
                // Edge crossings, computed lazily by the arms below.
                let bottom = || interp(bl, v_bl, br, v_br, level); // bl–br
                let right = || interp(br, v_br, tr, v_tr, level); // br–tr
                let top = || interp(tl, v_tl, tr, v_tr, level); // tl–tr
                let left = || interp(bl, v_bl, tl, v_tl, level); // bl–tl

                let segments = &mut out[order[si]].segments;
                match case {
                    1 | 14 => segments.push([left(), bottom()]),
                    2 | 13 => segments.push([bottom(), right()]),
                    4 | 11 => segments.push([right(), top()]),
                    7 | 8 => segments.push([left(), top()]),
                    3 | 12 => segments.push([left(), right()]),
                    6 | 9 => segments.push([bottom(), top()]),
                    // Saddles (opposite corners on the same side). The pairing
                    // is ambiguous; we pick one consistent resolution.
                    5 => {
                        segments.push([left(), bottom()]);
                        segments.push([right(), top()]);
                    }
                    10 => {
                        segments.push([bottom(), right()]);
                        segments.push([top(), left()]);
                    }
                    _ => unreachable!("marching-squares case {case} is 0..=15"),
                }
            }
        }
    }
    out
}

/// Roughly `target_count` "nice" contour levels spanning `(min, max)` — round
/// values (…, 1, 2, 5, 10, 20, …) a reader recognises, à la Panoply's automatic
/// levels. Levels sitting exactly on `min` or `max` are dropped, since a contour
/// at the field's extreme traces its boundary rather than an interior line.
///
/// Returns empty for a non-finite or empty range (`max <= min`) or a zero count.
pub fn nice_levels(min: f64, max: f64, target_count: usize) -> Vec<f64> {
    if !min.is_finite() || !max.is_finite() || max <= min || target_count == 0 {
        return Vec::new();
    }
    let spacing = nice_num((max - min) / target_count as f64, true);
    if spacing <= 0.0 || !spacing.is_finite() {
        return Vec::new();
    }
    let start = (min / spacing).floor() * spacing;
    let mut levels = Vec::new();
    // Walk by index rather than accumulating `+= spacing` so rounding error
    // can't drift the later levels.
    let mut k = 0i64;
    loop {
        let v = start + k as f64 * spacing;
        k += 1;
        if v <= min {
            continue;
        }
        if v >= max {
            break;
        }
        levels.push(v);
        // Guard against a pathological spacing producing an unbounded loop.
        if levels.len() > 10 * target_count + 10 {
            break;
        }
    }
    levels
}

/// The "nice" number nearest `x`: a 1/2/5 × 10ⁿ value. `round` snaps to the
/// nearest such number; otherwise it takes the ceiling (a nice number ≥ `x`).
fn nice_num(x: f64, round: bool) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let exp = x.log10().floor();
    let frac = x / 10f64.powf(exp);
    let nice = if round {
        if frac < 1.5 {
            1.0
        } else if frac < 3.0 {
            2.0
        } else if frac < 7.0 {
            5.0
        } else {
            10.0
        }
    } else if frac <= 1.0 {
        1.0
    } else if frac <= 2.0 {
        2.0
    } else if frac <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * 10f64.powf(exp)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field increasing left→right: `value = i`. A contour at a level between
    /// columns is a single vertical line — one segment per row-cell, each
    /// crossing the horizontal edges at the same `i`.
    fn ramp(ni: usize, nj: usize) -> Vec<Option<f64>> {
        (0..ni * nj).map(|k| Some((k % ni) as f64)).collect()
    }

    #[test]
    fn a_level_outside_the_range_has_no_segments() {
        let f = ramp(5, 4);
        let out = contour_segments(&f, 5, 4, &[-1.0, 99.0]);
        assert_eq!(out.len(), 2);
        assert!(out[0].segments.is_empty(), "below the field minimum");
        assert!(out[1].segments.is_empty(), "above the field maximum");
    }

    #[test]
    fn unsorted_and_out_of_range_levels_map_back_to_input_order() {
        // The per-cell band pruning sorts the levels internally; the output must
        // still be one entry per input level, in input order, with the right
        // segments — so unsorted and out-of-range levels are the regression to
        // guard (#336). value = i over a 5×4 grid, so a level L ∈ (0, 4) crosses
        // every row cell at i = L (3 vertical segments), and a level outside
        // [0, 4] has no crossings.
        let f = ramp(5, 4);
        let levels = [3.5, -10.0, 1.5, 2.5, 99.0];
        let out = contour_segments(&f, 5, 4, &levels);
        assert_eq!(out.len(), levels.len());
        for (k, &lvl) in levels.iter().enumerate() {
            assert_eq!(out[k].level, lvl, "entry {k} keeps its input level {lvl}");
            if lvl > 0.0 && lvl < 4.0 {
                assert_eq!(out[k].segments.len(), 3, "level {lvl}: one segment per row");
                for seg in &out[k].segments {
                    for (x, _y) in seg {
                        assert!(
                            (x - lvl).abs() < 1e-9,
                            "level {lvl} crosses at i={lvl}, got {x}"
                        );
                    }
                }
            } else {
                assert!(
                    out[k].segments.is_empty(),
                    "out-of-range level {lvl} has no segments"
                );
            }
        }
    }

    #[test]
    fn a_ramp_contour_is_a_straight_vertical_line() {
        // value = i over a 5×4 grid; the level 2.5 crosses at i = 2.5 in every
        // row cell, giving nj-1 = 3 vertical segments all at i ≈ 2.5.
        let f = ramp(5, 4);
        let out = contour_segments(&f, 5, 4, &[2.5]);
        assert_eq!(out[0].segments.len(), 3, "one segment per row of cells");
        for seg in &out[0].segments {
            for (x, _y) in seg {
                assert!((x - 2.5).abs() < 1e-9, "crossing at i = 2.5, got {x}");
            }
        }
    }

    #[test]
    fn a_missing_corner_removes_that_cell_from_the_contour() {
        // Same ramp, but knock a hole near the crossing column: every cell that
        // touches the hole must drop out, so fewer than 3 segments remain.
        let mut f = ramp(5, 4);
        f[5 + 2] = None; // row 1, col 2 — a corner of two crossing cells
        let out = contour_segments(&f, 5, 4, &[2.5]);
        assert!(
            out[0].segments.len() < 3,
            "the hole must break the contour, got {} segments",
            out[0].segments.len()
        );
        // Every emitted vertex is still on the i = 2.5 crossing.
        for seg in &out[0].segments {
            for (x, _) in seg {
                assert!((x - 2.5).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn a_non_finite_corner_is_treated_like_missing() {
        let mut f = ramp(3, 3);
        f[4] = Some(f64::NAN); // centre
        // The centre touches all four cells, so the whole field drops out.
        let out = contour_segments(&f, 3, 3, &[1.5]);
        assert!(out[0].segments.is_empty(), "NaN centre voids every cell");
    }

    #[test]
    fn a_saddle_cell_emits_two_segments() {
        // A single cell with opposite-corner highs: bl=tr=1, br=tl=0. Level 0.5
        // is the classic ambiguous saddle → two segments.
        let f = vec![Some(1.0), Some(0.0), Some(0.0), Some(1.0)];
        let out = contour_segments(&f, 2, 2, &[0.5]);
        assert_eq!(out[0].segments.len(), 2, "a saddle cell traces two lines");
    }

    #[test]
    fn a_plateau_at_the_level_still_closes_a_ring_without_nan() {
        // A central bump: the level between the plateau and its surround should
        // trace a closed-ish ring (>= 4 segments) with only finite vertices.
        let f = vec![
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(1.0),
            Some(1.0),
            Some(1.0),
            Some(0.0),
            Some(0.0),
            Some(1.0),
            Some(2.0),
            Some(1.0),
            Some(0.0),
            Some(0.0),
            Some(1.0),
            Some(1.0),
            Some(1.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
            Some(0.0),
        ];
        let out = contour_segments(&f, 5, 5, &[0.5]);
        assert!(out[0].segments.len() >= 4, "the bump is enclosed");
        for seg in &out[0].segments {
            for (x, y) in seg {
                assert!(x.is_finite() && y.is_finite());
            }
        }
    }

    #[test]
    fn empty_or_tiny_grids_yield_no_segments() {
        assert!(contour_segments(&[], 0, 0, &[0.0])[0].segments.is_empty());
        let one = vec![Some(1.0)];
        assert!(contour_segments(&one, 1, 1, &[0.5])[0].segments.is_empty());
    }

    #[test]
    fn nice_levels_are_round_and_inside_the_range() {
        let levels = nice_levels(0.0, 100.0, 5);
        assert!(!levels.is_empty());
        // Endpoints are excluded; every level lies strictly inside.
        for &l in &levels {
            assert!(l > 0.0 && l < 100.0, "{l} out of (0, 100)");
        }
        // "Nice" spacing: consecutive levels are evenly spaced on a round step.
        let step = levels[1] - levels[0];
        assert!(
            (step - 20.0).abs() < 1e-9,
            "expected a step of 20, got {step}"
        );
        for w in levels.windows(2) {
            assert!((w[1] - w[0] - step).abs() < 1e-9, "even spacing");
        }
    }

    #[test]
    fn nice_levels_handles_small_and_negative_ranges() {
        let small = nice_levels(0.001, 0.005, 4);
        assert!(small.iter().all(|&l| l > 0.001 && l < 0.005));
        let neg = nice_levels(-3.0, 3.0, 6);
        assert!(neg.contains(&0.0), "a nice ramp across zero includes 0");
        assert!(neg.iter().all(|&l| l > -3.0 && l < 3.0));
    }

    #[test]
    fn nice_levels_rejects_degenerate_ranges() {
        assert!(nice_levels(5.0, 5.0, 8).is_empty(), "empty range");
        assert!(nice_levels(10.0, 0.0, 8).is_empty(), "inverted range");
        assert!(nice_levels(0.0, 1.0, 0).is_empty(), "zero count");
        assert!(nice_levels(f64::NAN, 1.0, 8).is_empty(), "non-finite");
    }
}
