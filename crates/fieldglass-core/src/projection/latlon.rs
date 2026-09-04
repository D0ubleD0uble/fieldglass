//! Regular latitude/longitude grids — GRIB1 `grid_type` 0, GRIB2 template 3.0.
//!
//! The simplest family: both axes are evenly spaced in degrees, so the inverse
//! map is a pair of divisions once the requested longitude has been folded into
//! the grid's eastward span. `eastward_rel_lon` does that folding and the
//! Mercator and Gaussian families reuse it, since they share the longitude axis
//! and differ only in how the row ordinate is computed.

use super::{GridIndex, axis_position};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LatLonParams {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
}

/// Eastward longitude span of a west-to-east lat/lon grid, in degrees.
///
/// A grid that crosses the antimeridian reports `lon_last` numerically *below*
/// `lon_first` (e.g. ECMWF open data runs 180° → 359.75° → 0° → 179.75°, so
/// `lon_first = 180`, `lon_last = 179.75`). Taking `min`/`max` of the two
/// corners then collapses the span to a single grid step and reverses the
/// east-west increment — the field renders mirrored and only a sliver near the
/// seam survives. Unwrapping by +360° recovers the true span. All operational
/// lat/lon grids scan west-to-east; a descending-longitude grid would be
/// misread as a wrap, so the render seam keeps the rare east-to-west scan out
/// of reprojection. A grid spanning exactly the globe (e.g. -180°..180°) keeps
/// its full 360° span (`span >= 0`).
pub fn eastward_lon_span(lon_first: f64, lon_last: f64) -> f64 {
    let span = lon_last - lon_first;
    if span < 0.0 { span + 360.0 } else { span }
}

/// Whether a west-to-east grid covers the full globe: one more column step
/// past the last column lands back on the first (`span + step ≈ 360°`). A
/// global grid is periodic in longitude — the seam gap between the last
/// column and the first belongs to the grid and wrap-interpolates (see
/// `SourceGrid::periodic_i` in the warp). The tolerance is relative to the
/// step so coarse and fine grids alike qualify only when truly periodic; a
/// grid spanning exactly 360° (duplicated seam column) has no gap and isn't
/// flagged.
pub fn lon_grid_is_global(east_span: f64, ni: u32) -> bool {
    if ni < 2 || !east_span.is_finite() || east_span <= 0.0 {
        return false;
    }
    let ew = east_span / (ni as f64 - 1.0);
    (east_span + ew - 360.0).abs() <= ew * 1e-3
}

/// Eastward offset of `lon` from `lon_first` on a west-to-east grid covering
/// `[lon_first, lon_first + east_span]`, plus the span itself — or `None` when
/// the longitude is off-grid or the corners are malformed (non-finite, or no
/// east-west extent). On a global grid (see [`lon_grid_is_global`]) the seam
/// gap past the last column is on-grid too: the offset lands in
/// `(east_span, 360)` — a fractional column between `ni - 1` and `ni` — which
/// a periodic-aware sampler wraps back to column 0. Shared by the lat/lon,
/// Mercator, and Gaussian inverse maps so the antimeridian unwrap (see
/// [`eastward_lon_span`]) can't drift between them.
pub(super) fn eastward_rel_lon(
    lon_first: f64,
    lon_last: f64,
    ni: u32,
    lon: f64,
) -> Option<(f64, f64)> {
    let east_span = eastward_lon_span(lon_first, lon_last);
    if !east_span.is_finite() || east_span == 0.0 {
        // A non-finite corner (a NaN NetCDF coordinate, say) must be rejected
        // here: NaN survives `rem_euclid` and both comparisons below, and
        // would escape as a NaN grid index that the warp samples as column 0.
        return None;
    }
    let rel = lon - lon_first;
    if (0.0..=east_span).contains(&rel) {
        // Fast path: already in range — the common case in a warp loop, where
        // this runs once per output pixel and `rem_euclid` is an fmod.
        return Some((rel, east_span));
    }
    let rel = rel.rem_euclid(360.0);
    if rel > east_span && !lon_grid_is_global(east_span, ni) {
        return None;
    }
    Some((rel, east_span))
}

pub fn latlon_inverse(p: &LatLonParams, lat: f64, lon: f64) -> Option<GridIndex> {
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    if p.ni < 2 || p.nj < 2 {
        // A 1×N or N×1 grid is degenerate for linear interpolation; no
        // sane caller asks for one but the math would divide by zero.
        return None;
    }
    let (rel_lon, east_span) = eastward_rel_lon(p.lon_first, p.lon_last, p.ni, lon)?;
    let min_lat = p.lat_first.min(p.lat_last);
    let max_lat = p.lat_first.max(p.lat_last);
    if !(min_lat..=max_lat).contains(&lat) {
        return None;
    }
    let ew = east_span / (p.ni as f64 - 1.0);
    let ns = (p.lat_last - p.lat_first) / (p.nj as f64 - 1.0);
    Some(GridIndex {
        i: rel_lon / ew,
        j: (lat - p.lat_first) / ns,
    })
}

// Forward geolocation: grid index → (lat, lon).

/// `(lat, lon)` of grid point `(i, j)` on a regular lat/lon grid — the inverse
/// of [`latlon_inverse`]. Rows are evenly spaced in latitude, columns in
/// longitude.
///
/// The longitude walks the *eastward* span, so a grid that crosses the
/// antimeridian (`lon_last` numerically below `lon_first`) marches east like
/// the inverse reads it, rather than doubling back west. It is returned in the
/// grid's own frame and may exceed 360°; see [`super::normalise_lon`].
pub fn latlon_point(p: &LatLonParams, i: u32, j: u32) -> Option<(f64, f64)> {
    if p.ni < 2 || p.nj < 2 {
        return None;
    }
    let east_span = eastward_lon_span(p.lon_first, p.lon_last);
    Some((
        axis_position(p.lat_first, p.lat_last, p.nj, j),
        p.lon_first + i as f64 * (east_span / (p.ni as f64 - 1.0)),
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::near;

    fn latlon_params() -> LatLonParams {
        LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 10.0,
            lon_first: 100.0,
            lat_last: 50.0,
            lon_last: 140.0,
        }
    }

    #[test]
    fn latlon_inverse_corners_round_trip() {
        let p = latlon_params();
        let tl = latlon_inverse(&p, p.lat_first, p.lon_first).expect("tl");
        assert!(near(tl.i, 0.0, 1e-9));
        assert!(near(tl.j, 0.0, 1e-9));
        let br = latlon_inverse(&p, p.lat_last, p.lon_last).expect("br");
        assert!(near(br.i, p.ni as f64 - 1.0, 1e-9));
        assert!(near(br.j, p.nj as f64 - 1.0, 1e-9));
    }

    #[test]
    fn latlon_inverse_centre_interpolates() {
        let mid = latlon_inverse(&latlon_params(), 30.0, 120.0).expect("mid");
        assert!(near(mid.i, 2.0, 1e-9));
        assert!(near(mid.j, 2.0, 1e-9));
    }

    #[test]
    fn latlon_inverse_outside_returns_none() {
        let p = latlon_params();
        assert!(latlon_inverse(&p, 60.0, 120.0).is_none());
        assert!(latlon_inverse(&p, 30.0, 200.0).is_none());
    }

    #[test]
    fn latlon_inverse_handles_lon_wrap() {
        let p = LatLonParams {
            lon_first: 0.0,
            lon_last: 358.0,
            ..latlon_params()
        };
        let idx = latlon_inverse(&p, 30.0, -2.0).expect("wrap -2° to 358°");
        assert!(near(idx.i, p.ni as f64 - 1.0, 1e-9));
    }

    #[test]
    fn latlon_inverse_handles_antimeridian_origin_grid() {
        // ECMWF open data starts at the antimeridian and wraps: here lon runs
        // 180 → 270 → 0 → 90 across four columns (90° step), so `lon_last` (90)
        // comes back numerically below `lon_first` (180). The grid still covers
        // a 270° eastward arc; taking min/max of the corners used to collapse it
        // to a single step and render a mirrored sliver.
        let p = LatLonParams {
            ni: 4,
            lon_first: 180.0,
            lon_last: 90.0,
            ..latlon_params() // nj = 5, lat 10..50
        };
        // Each column resolves to its true longitude, including the one past
        // the 360° wrap.
        assert!(near(
            latlon_inverse(&p, 30.0, 180.0).expect("col 0").i,
            0.0,
            1e-9
        ));
        assert!(near(
            latlon_inverse(&p, 30.0, 270.0).expect("col 1").i,
            1.0,
            1e-9
        ));
        assert!(near(
            latlon_inverse(&p, 30.0, 0.0).expect("col 2 at 360°").i,
            2.0,
            1e-9
        ));
        assert!(near(
            latlon_inverse(&p, 30.0, 90.0).expect("col 3").i,
            3.0,
            1e-9
        ));
        // This grid is global (270° span + 90° step = 360°), so a longitude
        // in the seam gap between the last column and the wrap of the first
        // is on-grid: it maps past `ni - 1`, and the periodic sampler wraps
        // it back to column 0.
        assert!(near(
            latlon_inverse(&p, 30.0, 135.0).expect("seam gap").i,
            3.5,
            1e-9
        ));
    }

    #[test]
    fn latlon_inverse_rejects_seam_gap_of_regional_grid() {
        // A regional grid (40° span + 10° step ≠ 360°) is not periodic; a
        // longitude past its eastern edge stays off-grid.
        let p = latlon_params(); // lon 100..140, ni = 5
        assert!(latlon_inverse(&p, 30.0, 150.0).is_none());
    }

    #[test]
    fn lon_grid_is_global_detects_periodic_spans() {
        // GFS-style 0.25° global grid: 0..359.75 over 1440 columns.
        assert!(lon_grid_is_global(359.75, 1440));
        // Coarse global grid: 270° over 4 columns (90° step).
        assert!(lon_grid_is_global(270.0, 4));
        // Regional grid.
        assert!(!lon_grid_is_global(40.0, 5));
        // Exactly 360° means a duplicated seam column — no gap to wrap.
        assert!(!lon_grid_is_global(360.0, 1441));
        // Malformed spans.
        assert!(!lon_grid_is_global(f64::NAN, 1440));
        assert!(!lon_grid_is_global(0.0, 1440));
    }

    #[test]
    fn latlon_inverse_rejects_non_finite_corner() {
        // A NaN corner (a corrupt NetCDF coordinate, say) must reject, not
        // escape as a NaN grid index that the warp would sample as column 0.
        let p = LatLonParams {
            lon_first: f64::NAN,
            ..latlon_params()
        };
        assert!(latlon_inverse(&p, 30.0, 120.0).is_none());
        let p = LatLonParams {
            lon_last: f64::NAN,
            ..latlon_params()
        };
        assert!(latlon_inverse(&p, 30.0, 120.0).is_none());
    }

    #[test]
    fn latlon_inverse_rejects_nonfinite_and_degenerate_dims() {
        let p = latlon_params();
        assert!(latlon_inverse(&p, f64::NAN, 120.0).is_none());
        assert!(latlon_inverse(&p, 30.0, f64::INFINITY).is_none());
        let degenerate = LatLonParams { nj: 1, ..p };
        assert!(latlon_inverse(&degenerate, 30.0, 120.0).is_none());
    }
}
