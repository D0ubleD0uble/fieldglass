//! Gaussian latitude/longitude grids — GRIB1 `grid_type` 4, GRIB2 template 3.40.
//!
//! Rows sit on the Gauss–Legendre quadrature nodes rather than on an even
//! latitude step, so the inverse map searches the node list (cached per
//! `n_parallels`) instead of dividing. Reduced grids — fewer points on rows
//! near the poles — are expanded onto a regular raster before rendering, which
//! is what [`reduced_raster_width`] and [`expand_reduced_to_regular`] are for.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::f64::consts::PI;

use super::latlon::{eastward_lon_span, eastward_rel_lon};
use super::{GridIndex, RAD2DEG};

/// A Gaussian latitude/longitude grid: longitude is evenly spaced, but the
/// rows sit on the `2 · n_parallels` Gauss–Legendre quadrature nodes, which
/// crowd toward the equator. GRIB1 `grid_type` 4, GRIB2 template 3.40.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GaussianParams {
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the first scanned point, degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point, degrees.
    pub lon_first: f64,
    /// Latitude of the last scanned point, degrees.
    pub lat_last: f64,
    /// Longitude of the last scanned point, degrees.
    pub lon_last: f64,
    /// "N" — number of parallels between a pole and the equator. The
    /// full grid has `2N` Gaussian latitudes.
    pub n_parallels: u32,
}

thread_local! {
    /// Cached Gauss–Legendre nodes per `N` value — computing is O(N²) and
    /// the same fixture re-renders many times during a session.
    /// `BTreeMap` to keep the cache deterministic across iterations.
    static GAUSS_CACHE: RefCell<BTreeMap<u32, Vec<f64>>> = const { RefCell::new(BTreeMap::new()) };
}

/// Whether a reduced Gaussian grid's `PL` list describes an **octahedral**
/// grid rather than a classic one.
///
/// The two are named differently by every tool that prints them — ECMWF's
/// `O1280` against the older `N320` — and the difference is visible only in the
/// row widths. A classic grid's widths come from a tabulated algorithm; an
/// octahedral grid's rise by exactly four per row from the pole to the equator
/// and fall by four again, which is what this recognises.
///
/// This is eccodes' own rule, transcribed from `OctahedralGaussian.cc`
/// (`is_pl_octahedral`), rather than the arithmetic shortcut of comparing the
/// equatorial row against `4N + 16`: the shortcut reads one row, and would
/// accept a grid that is octahedral only at its equator. Each step must be
/// `0`, `+4` or `-4`, and the steps must be ordered — rising, then at most one
/// plateau, then falling.
///
/// Deliberately matches eccodes on the degenerate input too: a list of fewer
/// than two rows has no step to disagree about and answers `true`. That is not
/// a claim that a one-row grid is octahedral — it is so this never disagrees
/// with the oracle it is checked against. What counts as a grid is the
/// caller's question; this function is not told how many rows were declared.
pub fn is_octahedral_pl(points_per_row: &[u32]) -> bool {
    let mut previous: Option<i64> = None;
    for index in 1..points_per_row.len() {
        let step = i64::from(points_per_row[index]) - i64::from(points_per_row[index - 1]);
        let first = index == 1;
        let ordered = match step {
            // A plateau is allowed at the equator, and only after a rise.
            0 => first || previous == Some(4),
            4 => first || previous == Some(4),
            -4 => first || previous == Some(-4) || previous == Some(0),
            _ => false,
        };
        if !ordered {
            return false;
        }
        previous = Some(step);
    }
    true
}

/// Width of the regular raster a reduced grid's rows expand into: its widest
/// row (`0` for an empty list).
///
/// A reduced grid's rows differ in width, so the only rectangle that holds all
/// of them without dropping a point is `max(PL) × PL.len()`. This is the `ni`
/// GRIB1 and GRIB2 both report from `dimensions()`, and the width
/// [`expand_reduced_to_regular`] widens each row to.
pub fn reduced_raster_width(points_per_row: &[u32]) -> u32 {
    points_per_row.iter().copied().max().unwrap_or(0)
}

/// Longitude of the last column of the raster [`expand_reduced_to_regular`]
/// builds, given the grid's declared first longitude and the raster width.
///
/// **The message's own last-point longitude cannot be used here.** Under §3's
/// interpretation code 1 — "numbers define number of points corresponding to
/// full coordinate circles", which is what every reduced grid in the wild
/// carries — each row spans the whole circle in `PL[j]` steps, and the code
/// itself warns that "extreme coordinate values given in grid definition may
/// not be reached in all rows". ECMWF writes `lo2` from the *reference regular*
/// grid, `4N` columns wide. For a classic `N32` that is also the widest row
/// (128) and the two agree; for an octahedral `O32` the widest row is 144, and
/// trusting the declared 357.1875° would place the raster's 144 columns on a
/// 128-column grid — every column east of Greenwich drawn up to an eighth of a
/// cell west of where its data actually is.
///
/// The expanded raster puts column `k` at `lon_first + k·360/width` by
/// construction, so its last column is `lon_first + (width - 1)·360/width`.
/// Returns `lon_first` for a degenerate zero width.
pub fn reduced_raster_lon_last(lon_first: f64, width: u32) -> f64 {
    if width == 0 {
        return lon_first;
    }
    lon_first + f64::from(width - 1) * 360.0 / f64::from(width)
}

/// Widen a reduced (quasi-regular) grid's row-packed `values` into a regular
/// `max(PL) × PL.len()` raster, so the regular-grid render and reproject paths
/// apply unchanged. `values` is the field in storage order — `PL[j]` points for
/// row `j`, concatenated — and the result is row-major, with the raster width
/// taken from `points_per_row` by [`reduced_raster_width`]. The width is not a
/// parameter: it is `max(PL)` or the raster is misregistered, so there is
/// nothing for a caller to pass but the same derivation.
///
/// Each reduced row holds `PL[j]` points equispaced around the **full longitude
/// circle** (`Δλ = 360°/PL[j]`), which is how every standard reduced grid
/// (ECMWF `reduced_gg` / `reduced_ll`) is laid out. So output column `k` maps to
/// the nearest source column *by longitude*, wrapping at the antimeridian —
/// `round(k·PL[j] / width) mod PL[j]` — not by proportional index, which would
/// stretch a narrow polar row across the whole width and misregister it east to
/// west. Masked (`None`) points are carried through. The widest row(s) map
/// one-to-one.
///
/// The caller is responsible for refusing a `width · PL.len()` beyond whatever
/// point cap it enforces; both readers do so before reaching here.
pub fn expand_reduced_to_regular(
    values: &[Option<f64>],
    points_per_row: &[u32],
) -> Vec<Option<f64>> {
    let width = reduced_raster_width(points_per_row) as usize;
    let mut out = Vec::with_capacity(width.saturating_mul(points_per_row.len()));
    let mut offset = 0usize;
    for &count in points_per_row {
        let count = count as usize;
        let row = &values[offset.min(values.len())..(offset + count).min(values.len())];
        if row.is_empty() {
            out.resize(out.len() + width, None);
        } else {
            let len = row.len();
            for k in 0..width {
                // Nearest source column by longitude, with antimeridian wrap:
                // (k·len + width/2) / width rounds k·len/width to nearest.
                let src = (k * len + width / 2) / width % len;
                out.push(row[src]);
            }
        }
        offset += count;
    }
    out
}

/// Return the `2N` Gauss–Legendre quadrature nodes in degrees of
/// latitude, ordered north-to-south (matching the GRIB convention).
/// Roots are computed iteratively per Numerical Recipes §4.6.
pub fn gaussian_latitudes(n_parallels: u32) -> Vec<f64> {
    if let Some(cached) = GAUSS_CACHE.with(|c| c.borrow().get(&n_parallels).cloned()) {
        return cached;
    }

    let n = 2 * n_parallels as usize;
    let mut xs: Vec<f64> = vec![0.0; n];

    // Newton-Raphson on the Legendre polynomial. The roots are symmetric;
    // compute the southern half and mirror.
    let half = n.div_ceil(2);
    for i in 0..half {
        let mut x = (PI * (i as f64 + 0.75) / (n as f64 + 0.5)).cos();
        for _iter in 0..30 {
            let mut p1 = 1.0f64;
            let mut p2 = 0.0f64;
            for k in 1..=n {
                let p3 = p2;
                p2 = p1;
                let kf = k as f64;
                p1 = ((2.0 * kf - 1.0) * x * p2 - (kf - 1.0) * p3) / kf;
            }
            let pp = n as f64 * (x * p1 - p2) / (x * x - 1.0);
            let dx = p1 / pp;
            x -= dx;
            if dx.abs() < 1e-14 {
                break;
            }
        }
        xs[i] = x;
        xs[n - 1 - i] = -x;
    }

    let mut lats_deg: Vec<f64> = xs.iter().map(|s| s.asin() * RAD2DEG).collect();
    // `total_cmp` rather than `partial_cmp().expect(...)`: the Newton-Raphson
    // roots are finite by construction, but a non-panicking total order means a
    // stray NaN sorts to one end instead of crashing the whole render.
    lats_deg.sort_by(|a, b| b.total_cmp(a));
    GAUSS_CACHE.with(|c| {
        c.borrow_mut().insert(n_parallels, lats_deg.clone());
    });
    lats_deg
}

/// Inverse map for a Gaussian source grid. **Builds a transient
/// [`GaussianProjector`] per call** — for warp loops use
/// [`GaussianProjector::new`] once outside the loop and call
/// [`GaussianProjector::inverse`] inside it.
pub(crate) fn gaussian_inverse(p: &GaussianParams, lat: f64, lon: f64) -> Option<GridIndex> {
    GaussianProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a Gaussian source grid. Holds the cached
/// row latitudes ordered to match the grid's `lat_first` → `lat_last`
/// scan direction, so `inverse` does one bracket search per call without
/// touching the global Gauss–Legendre cache or re-reversing the vec.
///
/// Build once outside the warp loop; call `inverse` per output pixel.
#[derive(Debug)]
pub struct GaussianProjector {
    /// The grid this projector was built for.
    pub params: GaussianParams,
    row_lats: Vec<f64>,
    north_to_south: bool,
}

impl GaussianProjector {
    /// Precompute the node list for `params`. Build once outside a warp loop.
    pub fn new(params: GaussianParams) -> Self {
        let north_to_south = params.lat_first > params.lat_last;
        let mut row_lats = gaussian_latitudes(params.n_parallels);
        if !north_to_south {
            row_lats.reverse();
        }
        Self {
            params,
            row_lats,
            north_to_south,
        }
    }

    /// Fractional grid index for a geographic point, or `None` outside the
    /// grid's extent.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        let p = &self.params;
        if p.ni < 2 || p.nj < 2 {
            // Degenerate dimensions — the longitude interpolation step
            // would divide by zero, and the latitude bracket has no
            // useful row span. Real Gaussian grids always have N ≥ 1
            // parallels (and thus nj ≥ 2 rows); guard anyway.
            return None;
        }
        let min_lat = p.lat_first.min(p.lat_last);
        let max_lat = p.lat_first.max(p.lat_last);
        if !(min_lat..=max_lat).contains(&lat) {
            return None;
        }
        let (rel_lon, east_span) = eastward_rel_lon(p.lon_first, p.lon_last, p.ni, lon)?;
        let ew = east_span / (p.ni as f64 - 1.0);
        let i = rel_lon / ew;

        const BOUND_EPS: f64 = 1e-3;
        let last_row = self.row_lats.len() - 1;
        if self.north_to_south {
            if lat >= self.row_lats[0] - BOUND_EPS {
                return Some(GridIndex { i, j: 0.0 });
            }
            if lat <= self.row_lats[last_row] + BOUND_EPS {
                return Some(GridIndex {
                    i,
                    j: last_row as f64,
                });
            }
        } else {
            if lat <= self.row_lats[0] + BOUND_EPS {
                return Some(GridIndex { i, j: 0.0 });
            }
            if lat >= self.row_lats[last_row] - BOUND_EPS {
                return Some(GridIndex {
                    i,
                    j: last_row as f64,
                });
            }
        }
        for row in 0..last_row {
            let hi = self.row_lats[row];
            let lo = self.row_lats[row + 1];
            let inside = if self.north_to_south {
                lat <= hi && lat >= lo
            } else {
                lat >= hi && lat <= lo
            };
            if inside {
                let span = hi - lo;
                if span == 0.0 {
                    return Some(GridIndex { i, j: row as f64 });
                }
                let frac = (hi - lat) / span;
                return Some(GridIndex {
                    i,
                    j: row as f64 + frac,
                });
            }
        }
        None
    }
}

// Forward geolocation: grid index → (lat, lon).

impl GaussianProjector {
    /// `(lat, lon)` of grid point `(i, j)` — the inverse of [`Self::inverse`].
    ///
    /// The row latitude is read straight from the cached Gauss–Legendre roots
    /// (already ordered to match the grid's scan direction), *not* interpolated:
    /// a Gaussian grid's rows are unevenly spaced by construction, so a linear
    /// formula would misplace every row but the first and last. Columns are
    /// evenly spaced in longitude, as on a regular lat/lon grid.
    pub fn grid_point_lonlat(&self, i: u32, j: u32) -> Option<(f64, f64)> {
        let p = &self.params;
        if p.ni < 2 || p.nj < 2 {
            return None;
        }
        let lat = *self.row_lats.get(j as usize)?;
        let ew = eastward_lon_span(p.lon_first, p.lon_last) / (p.ni as f64 - 1.0);
        Some((lat, p.lon_first + i as f64 * ew))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::near;

    #[test]
    fn gaussian_n32_node_count_and_symmetry() {
        let lats = gaussian_latitudes(32);
        assert_eq!(lats.len(), 64);
        assert!(near(lats[0], 87.8638, 1e-3));
        assert!(near(lats[63], -87.8638, 1e-3));
        for k in 0..32 {
            assert!(near(lats[k] + lats[63 - k], 0.0, 1e-9), "row {k} symmetry");
        }
    }

    #[test]
    fn gaussian_n48_first_node_pins() {
        let lats = gaussian_latitudes(48);
        assert_eq!(lats.len(), 96);
        assert!(near(lats[0], 88.5722, 1e-3));
    }

    #[test]
    fn gaussian_inverse_equator_lands_mid_grid() {
        let p = GaussianParams {
            ni: 128,
            nj: 64,
            lat_first: 87.8638,
            lon_first: 0.0,
            lat_last: -87.8638,
            lon_last: 357.188,
            n_parallels: 32,
        };
        let idx = gaussian_inverse(&p, 0.0, 180.0).expect("equator");
        assert!(idx.j >= 31.0 && idx.j <= 32.0, "j = {}", idx.j);
    }

    #[test]
    fn gaussian_inverse_handles_antimeridian_origin_grid() {
        // Column longitudes run 180 → 270 → 0 → 90 (`lon_last` numerically
        // below `lon_first`); the eastward span must unwrap like the lat/lon
        // inverse instead of collapsing to a reversed sliver.
        let p = GaussianParams {
            ni: 4,
            nj: 64,
            lat_first: 87.8638,
            lon_first: 180.0,
            lat_last: -87.8638,
            lon_last: 90.0,
            n_parallels: 32,
        };
        let projector = GaussianProjector::new(p);
        assert!(near(
            projector.inverse(0.0, 270.0).expect("col 1").i,
            1.0,
            1e-9
        ));
        assert!(near(
            projector.inverse(0.0, 0.0).expect("col 2 at 360°").i,
            2.0,
            1e-9
        ));
        // This grid is global (270° span + 90° step = 360°): the seam gap
        // maps past `ni - 1` for the periodic sampler to wrap.
        assert!(near(
            projector.inverse(0.0, 135.0).expect("seam gap").i,
            3.5,
            1e-9
        ));
    }

    #[test]
    fn gaussian_inverse_returns_none_outside_lat_range() {
        let p = GaussianParams {
            ni: 128,
            nj: 64,
            lat_first: 87.8638,
            lon_first: 0.0,
            lat_last: -87.8638,
            lon_last: 357.188,
            n_parallels: 32,
        };
        // Lat outside the [-87.86, 87.86] band.
        assert!(gaussian_inverse(&p, 95.0, 0.0).is_none());
        // Lon outside the [0, 357.188] band even after wrap normalisation —
        // pass a far-away value and force a tiny longitude range.
        let narrow = GaussianParams {
            lon_first: 100.0,
            lon_last: 110.0,
            ..p
        };
        assert!(gaussian_inverse(&narrow, 0.0, 200.0).is_none());
    }

    #[test]
    fn gaussian_inverse_handles_south_to_north_ordering() {
        // Some producers list rows south-to-north (`lat_first < lat_last`).
        // Verify the inverse map still locates rows correctly.
        let p = GaussianParams {
            ni: 128,
            nj: 64,
            lat_first: -87.8638,
            lon_first: 0.0,
            lat_last: 87.8638,
            lon_last: 357.188,
            n_parallels: 32,
        };
        let idx = gaussian_inverse(&p, -87.8638, 0.0).expect("southernmost");
        assert!(near(idx.j, 0.0, 1e-3), "south-to-north start at j=0");
        let idx = gaussian_inverse(&p, 87.8638, 0.0).expect("northernmost");
        assert!(near(idx.j, 63.0, 1e-3), "north end at j=last");
        // An equator-ish lat lands mid-grid.
        let mid = gaussian_inverse(&p, 0.0, 180.0).expect("mid");
        assert!(mid.j >= 31.0 && mid.j <= 32.0);
    }

    #[test]
    fn gaussian_latitudes_cache_hits_on_second_call() {
        // Force a fresh N value so we hit the build path then the cache.
        let _ = gaussian_latitudes(96);
        let cached = gaussian_latitudes(96);
        assert_eq!(cached.len(), 192);
    }

    #[test]
    fn gaussian_inverse_boundary_clamps() {
        let p = GaussianParams {
            ni: 128,
            nj: 64,
            lat_first: 87.8638,
            lon_first: 0.0,
            lat_last: -87.8638,
            lon_last: 357.188,
            n_parallels: 32,
        };
        let idx = gaussian_inverse(&p, 87.8638, 0.0).expect("northern boundary");
        assert!(near(idx.j, 0.0, 1e-3));
    }

    #[test]
    fn gaussian_inverse_rejects_nonfinite() {
        let p = GaussianParams {
            ni: 128,
            nj: 64,
            lat_first: 87.8638,
            lon_first: 0.0,
            lat_last: -87.8638,
            lon_last: 357.188,
            n_parallels: 32,
        };
        assert!(gaussian_inverse(&p, f64::NAN, 0.0).is_none());
        assert!(gaussian_inverse(&p, 0.0, f64::INFINITY).is_none());
    }

    // Reduced (thinned) grids: the octahedral rule and the expansion onto a
    // regular raster.

    /// The octahedral rule, against the shapes it must accept and reject.
    ///
    /// Transcribed from eccodes' `is_pl_octahedral`, so the cases that matter
    /// are the ordering ones: a rise after a fall, or a second plateau, is a
    /// grid whose widths happen to move in fours without being octahedral, and
    /// the arithmetic shortcut of checking one row against `4N + 16` would take
    /// all of them.
    #[test]
    fn octahedral_pl_recognises_the_shape_not_the_arithmetic() {
        // The real thing: rise by four to the equator, one plateau, fall by
        // four. This is `O32`, the shape of the committed fixture.
        let northern: Vec<u32> = (0..32).map(|row| 20 + 4 * row).collect();
        let octahedral: Vec<u32> = northern
            .iter()
            .copied()
            .chain(northern.iter().rev().copied())
            .collect();
        assert_eq!(octahedral.len(), 64);
        assert!(is_octahedral_pl(&octahedral));

        // A classic reduced grid's widths do not move in fours.
        assert!(!is_octahedral_pl(&[18, 25, 36, 40, 45, 54]));
        // Steps of the right size in the wrong order: falls, then rises again.
        assert!(!is_octahedral_pl(&[20, 24, 20, 24]));
        // A plateau before any rise.
        assert!(!is_octahedral_pl(&[20, 20, 24, 28]));
        // Two plateaux — only the equator may repeat.
        assert!(!is_octahedral_pl(&[20, 24, 24, 24, 20]));
        // A step of the wrong size, everything else right.
        assert!(!is_octahedral_pl(&[20, 24, 30, 34]));
        // Monotone rise with no fall is still octahedral by this rule: eccodes
        // asks about the steps, not about symmetry, and a caller that needs a
        // whole globe checks the row count against `Nj` itself.
        assert!(is_octahedral_pl(&[20, 24, 28, 32]));

        // Degenerate inputs answer as eccodes does — no step, no disagreement.
        assert!(is_octahedral_pl(&[]));
        assert!(is_octahedral_pl(&[20]));
    }

    /// A width near `u32::MAX` cannot make the step arithmetic wrap.
    ///
    /// The list is read from an untrusted file, so the differences are taken in
    /// `i64`: in `u32` the subtraction below would underflow and, in release,
    /// wrap to a number that is not `4` and not `-4` — the right answer by
    /// accident, from arithmetic that is wrong.
    #[test]
    fn octahedral_pl_does_not_wrap_on_hostile_widths() {
        assert!(!is_octahedral_pl(&[u32::MAX, 0]));
        assert!(!is_octahedral_pl(&[0, u32::MAX]));
        assert!(is_octahedral_pl(&[u32::MAX - 4, u32::MAX]));
    }

    fn vals(xs: &[f64]) -> Vec<Option<f64>> {
        xs.iter().map(|&x| Some(x)).collect()
    }

    #[test]
    fn widest_rows_map_one_to_one() {
        // A full-width row is copied through unchanged.
        let out = expand_reduced_to_regular(&vals(&[10.0, 20.0, 30.0, 40.0]), &[4]);
        assert_eq!(out, vals(&[10.0, 20.0, 30.0, 40.0]));
    }

    #[test]
    fn narrow_row_maps_by_longitude_and_wraps_at_antimeridian() {
        // Row of 4 points (a,b,c,d at 0°, 90°, 180°, 270°) widened to the
        // raster's 8 columns (0°, 45°, …, 315°) — 8 because a second row is
        // that wide, which is the only way a raster gets wider than a row.
        // Each output column takes its nearest-longitude source point, and the
        // last column (315°) wraps to a (at 360°≡0°) — not to d, which a
        // proportional-index stretch would wrongly pick.
        let mut raw = vals(&[1.0, 2.0, 3.0, 4.0]);
        raw.extend(vals(&[0.0; 8]));
        let out = expand_reduced_to_regular(&raw, &[4, 8]);
        assert_eq!(out.len(), 16, "two rows of the raster's 8 columns");
        assert_eq!(
            &out[0..8],
            &vals(&[1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 1.0])[..]
        );
    }

    #[test]
    fn two_point_row_wraps() {
        // [a,b] at 0°/180° → 4 columns at 0°/90°/180°/270°: a, b, b, a (the 90°
        // and 270° ties round up / wrap).
        let mut raw = vals(&[1.0, 2.0]);
        raw.extend(vals(&[0.0; 4]));
        let out = expand_reduced_to_regular(&raw, &[2, 4]);
        assert_eq!(out.len(), 8, "two rows of the raster's 4 columns");
        assert_eq!(&out[0..4], &vals(&[1.0, 2.0, 2.0, 1.0])[..]);
    }

    #[test]
    fn single_point_row_fills_width() {
        // A one-point polar row spreads across the whole width.
        let mut raw = vals(&[7.0]);
        raw.extend(vals(&[0.0; 3]));
        let out = expand_reduced_to_regular(&raw, &[1, 3]);
        assert_eq!(out.len(), 6, "two rows of the raster's 3 columns");
        assert_eq!(&out[0..3], &vals(&[7.0, 7.0, 7.0])[..]);
    }

    #[test]
    fn masked_points_are_preserved() {
        let row = vec![Some(1.0), None, Some(3.0)];
        let out = expand_reduced_to_regular(&row, &[3]);
        assert_eq!(out, vec![Some(1.0), None, Some(3.0)]);
    }

    #[test]
    fn multiple_rows_are_widened_independently() {
        // Row 0: 2 points widened to 4; row 1: already 4 wide.
        let raw = vals(&[1.0, 2.0, 10.0, 20.0, 30.0, 40.0]);
        let out = expand_reduced_to_regular(&raw, &[2, 4]);
        assert_eq!(out.len(), 8);
        assert_eq!(&out[0..4], &vals(&[1.0, 2.0, 2.0, 1.0])[..]);
        assert_eq!(&out[4..8], &vals(&[10.0, 20.0, 30.0, 40.0])[..]);
    }

    #[test]
    fn a_short_values_slice_pads_the_missing_rows() {
        // A truncated field must not index out of bounds: the rows it does not
        // reach come back masked rather than panicking.
        let out = expand_reduced_to_regular(&vals(&[1.0, 2.0]), &[2, 4]);
        assert_eq!(out.len(), 8);
        assert_eq!(&out[4..8], &[None, None, None, None]);
    }

    #[test]
    fn raster_width_is_the_widest_row() {
        assert_eq!(reduced_raster_width(&[20, 27, 128, 27, 20]), 128);
        assert_eq!(reduced_raster_width(&[]), 0, "no rows, no raster");
    }

    #[test]
    fn raster_lon_last_is_derived_from_the_width_not_the_file() {
        // Classic N32: the widest row is 128, and 127·360/128 = 357.1875 is
        // exactly the `lo2` ECMWF writes into the section, so nothing moves.
        assert!((reduced_raster_lon_last(0.0, 128) - 357.1875).abs() < 1e-9);
        // Octahedral O32: the widest row is 144, but the same file still
        // declares 357.1875 (the 4N reference grid). The raster's last column
        // is at 357.5, and using the declared value would shift it west.
        assert!((reduced_raster_lon_last(0.0, 144) - 357.5).abs() < 1e-9);
        // A one-column raster's only column is its first.
        assert_eq!(reduced_raster_lon_last(0.0, 1), 0.0);
        // Degenerate: no columns, no offset to apply.
        assert_eq!(reduced_raster_lon_last(-180.0, 0), -180.0);
    }
}
