//! Mercator grids — GRIB2 template 3.10.
//!
//! Longitude is linear, as in [`super::latlon`]; rows are evenly spaced in the
//! Mercator ordinate `ln(tan(π/4 + φ/2))` rather than in latitude, so the
//! inverse map converts the requested latitude into that ordinate before
//! dividing.

use std::f64::consts::PI;

use super::latlon::{eastward_lon_span, eastward_rel_lon};
use super::{DEG2RAD, GridIndex, RAD2DEG};

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MercatorParams {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
}

/// Mercator latitude function: geodetic latitude (degrees) → the dimensionless
/// Mercator ordinate `ln(tan(π/4 + φ/2))`. Strictly increasing in latitude and
/// divergent at the poles (±∞), which real Mercator grids never reach.
pub(super) fn mercator_ordinate(lat_deg: f64) -> f64 {
    (PI / 4.0 + lat_deg * DEG2RAD / 2.0).tan().ln()
}

/// Inverse map for a Mercator source grid: `(lat, lon)` in degrees →
/// fractional source-grid index, or `None` when the point lies outside the
/// grid coverage.
///
/// Like a regular lat/lon grid, a Mercator grid is evenly spaced in the
/// projection plane: equally spaced in longitude along i, and equally spaced
/// in the Mercator ordinate `ln(tan(π/4 + φ/2))` along j. The four corner
/// coordinates plus `ni`/`nj` pin the mapping completely, so — mirroring
/// [`super::latlon_inverse`] — the grid lengths (`Di`/`Dj` in metres) and the
/// latitude of true scale (`LaD`) aren't needed to locate a point.
pub fn mercator_inverse(p: &MercatorParams, lat: f64, lon: f64) -> Option<GridIndex> {
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    if p.ni < 2 || p.nj < 2 {
        // Degenerate for linear interpolation; the same guard the regular
        // lat/lon inverse uses.
        return None;
    }
    let (rel_lon, east_span) = eastward_rel_lon(p.lon_first, p.lon_last, p.ni, lon)?;
    let min_lat = p.lat_first.min(p.lat_last);
    let max_lat = p.lat_first.max(p.lat_last);
    if !(min_lat..=max_lat).contains(&lat) {
        return None;
    }
    // Rows are evenly spaced in the Mercator ordinate, not in latitude; columns
    // are evenly spaced in longitude.
    let ew = east_span / (p.ni as f64 - 1.0);
    let y_first = mercator_ordinate(p.lat_first);
    let y_last = mercator_ordinate(p.lat_last);
    if !y_first.is_finite() || !y_last.is_finite() {
        // A corner latitude sits at a pole (±90°), where the Mercator ordinate
        // diverges. Real Mercator grids never include the poles; reject a
        // malformed one rather than emit a NaN/∞ index that the warp would
        // smear into garbage pixels. (Mirrors the `is_finite` guards the
        // Lambert / polar-stereo projectors apply to their projected metres.)
        return None;
    }
    let ns = (y_last - y_first) / (p.nj as f64 - 1.0);
    if ns == 0.0 {
        // Both corner latitudes coincide — no north-south extent to
        // interpolate over. (The longitude counterpart is already rejected by
        // `eastward_rel_lon`'s zero-span guard.)
        return None;
    }
    Some(GridIndex {
        i: rel_lon / ew,
        j: (mercator_ordinate(lat) - y_first) / ns,
    })
}

// Forward geolocation: grid index → (lat, lon).

/// Mercator ordinate → geodetic latitude (degrees): the inverse of
/// [`mercator_ordinate`], `φ = 2·atan(eʸ) − π/2`.
fn mercator_latitude(y: f64) -> f64 {
    (2.0 * y.exp().atan() - PI / 2.0) * RAD2DEG
}

/// `(lat, lon)` of grid point `(i, j)` on a Mercator grid — the inverse of
/// [`mercator_inverse`]. Rows are evenly spaced in the *Mercator ordinate*,
/// not in latitude, so the latitude is recovered through the inverse ordinate.
pub fn mercator_point(p: &MercatorParams, i: u32, j: u32) -> Option<(f64, f64)> {
    if p.ni < 2 || p.nj < 2 {
        return None;
    }
    let y_first = mercator_ordinate(p.lat_first);
    let y_last = mercator_ordinate(p.lat_last);
    if !y_first.is_finite() || !y_last.is_finite() {
        // A corner sits at a pole, where the ordinate diverges — the same
        // malformed-grid guard `mercator_inverse` applies.
        return None;
    }
    // Step the ordinate, then invert it back to a latitude. The end rows are
    // the declared corners exactly: the ordinate round-trip (ln ∘ tan, then
    // atan ∘ exp) is not bit-exact, and the drift is enough for the inverse to
    // read the last row as off-grid.
    let lat = if j == 0 {
        p.lat_first
    } else if j == p.nj - 1 {
        p.lat_last
    } else {
        let ns = (y_last - y_first) / (p.nj as f64 - 1.0);
        mercator_latitude(y_first + j as f64 * ns)
    };
    let ew = eastward_lon_span(p.lon_first, p.lon_last) / (p.ni as f64 - 1.0);
    Some((lat, p.lon_first + i as f64 * ew))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::near;

    fn mercator_params() -> MercatorParams {
        // A small Mercator tile spanning the equator northward, 5×5 points.
        MercatorParams {
            ni: 5,
            nj: 5,
            lat_first: 0.0,
            lon_first: 100.0,
            lat_last: 40.0,
            lon_last: 140.0,
        }
    }

    #[test]
    fn mercator_inverse_handles_antimeridian_origin_grid() {
        // Same layout as the lat/lon antimeridian test: lon runs 180 → 270 →
        // 0 → 90 across four columns (90° step), so `lon_last` comes back
        // numerically below `lon_first` but the grid covers a 270° arc.
        let p = MercatorParams {
            ni: 4,
            lon_first: 180.0,
            lon_last: 90.0,
            ..mercator_params()
        };
        assert!(near(
            mercator_inverse(&p, 20.0, 270.0).expect("col 1").i,
            1.0,
            1e-9
        ));
        assert!(near(
            mercator_inverse(&p, 20.0, 0.0).expect("col 2 at 360°").i,
            2.0,
            1e-9
        ));
        // This grid is global (270° span + 90° step = 360°): the seam gap
        // maps past `ni - 1` for the periodic sampler to wrap.
        assert!(near(
            mercator_inverse(&p, 20.0, 135.0).expect("seam gap").i,
            3.5,
            1e-9
        ));
    }

    #[test]
    fn mercator_inverse_rejects_non_finite_corner() {
        let p = MercatorParams {
            lon_first: f64::NAN,
            ..mercator_params()
        };
        assert!(mercator_inverse(&p, 20.0, 120.0).is_none());
    }

    #[test]
    fn mercator_inverse_maps_corners() {
        let p = mercator_params();
        let tl = mercator_inverse(&p, p.lat_first, p.lon_first).expect("first corner");
        assert!(near(tl.i, 0.0, 1e-9));
        assert!(near(tl.j, 0.0, 1e-9));
        let br = mercator_inverse(&p, p.lat_last, p.lon_last).expect("last corner");
        assert!(near(br.i, p.ni as f64 - 1.0, 1e-9));
        assert!(near(br.j, p.nj as f64 - 1.0, 1e-9));
    }

    #[test]
    fn mercator_inverse_longitude_is_linear() {
        // Longitude is linear in i: the midpoint longitude lands at i = 2.
        let mid = mercator_inverse(&mercator_params(), 0.0, 120.0).expect("mid lon");
        assert!(near(mid.i, 2.0, 1e-9), "i = {}", mid.i);
    }

    #[test]
    fn mercator_inverse_rows_are_spaced_in_mercator_y() {
        // Rows are equally spaced in the Mercator ordinate, *not* in latitude:
        // the latitude halfway up the grid in projected space sits above the
        // arithmetic-mean latitude (20°), so querying 20° lands below j = 2.
        let p = mercator_params();
        let at_mean_lat = mercator_inverse(&p, 20.0, 100.0).expect("mean lat");
        assert!(
            at_mean_lat.j < 2.0,
            "20° must map below the projected midpoint, got j = {}",
            at_mean_lat.j
        );
        // The true projected midpoint is the latitude whose ordinate is the
        // mean of the corner ordinates; it must land exactly at j = 2.
        let y_mid = (mercator_ordinate(p.lat_first) + mercator_ordinate(p.lat_last)) / 2.0;
        let lat_mid = (2.0 * y_mid.exp().atan() - PI / 2.0) * RAD2DEG;
        let mid = mercator_inverse(&p, lat_mid, 100.0).expect("projected midpoint");
        assert!(near(mid.j, 2.0, 1e-9), "j = {}", mid.j);
    }

    #[test]
    fn mercator_inverse_outside_returns_none() {
        let p = mercator_params();
        assert!(mercator_inverse(&p, 50.0, 120.0).is_none(), "north of grid");
        assert!(mercator_inverse(&p, 20.0, 200.0).is_none(), "east of grid");
    }

    #[test]
    fn mercator_inverse_handles_lon_wrap() {
        let p = MercatorParams {
            lon_first: 0.0,
            lon_last: 358.0,
            ..mercator_params()
        };
        let idx = mercator_inverse(&p, 0.0, -2.0).expect("wrap -2° to 358°");
        assert!(near(idx.i, p.ni as f64 - 1.0, 1e-9));
    }

    #[test]
    fn mercator_inverse_rejects_nonfinite_and_degenerate() {
        let p = mercator_params();
        assert!(mercator_inverse(&p, f64::NAN, 120.0).is_none());
        assert!(mercator_inverse(&p, 20.0, f64::INFINITY).is_none());
        let degenerate = MercatorParams { nj: 1, ..p };
        assert!(mercator_inverse(&degenerate, 20.0, 120.0).is_none());
        // Zero latitude extent collapses the Mercator-ordinate span.
        let flat = MercatorParams { lat_last: 0.0, ..p };
        assert!(mercator_inverse(&flat, 0.0, 120.0).is_none());
        // A pole corner (±90°) makes the Mercator ordinate diverge; a query
        // inside the (malformed) grid must be rejected, not return a NaN index.
        let polar = MercatorParams {
            lat_first: -90.0,
            lat_last: 85.0,
            ..p
        };
        assert!(
            mercator_inverse(&polar, 0.0, 120.0).is_none(),
            "a pole-corner grid must not yield a NaN index"
        );
    }
}
