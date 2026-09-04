//! Rotated latitude/longitude grids — GRIB1 `grid_type` 10, GRIB2 template 3.1.
//!
//! A regular lat/lon grid on a rotated sphere whose south pole sits at a
//! declared geographic position. [`unrotate_latlon`] and [`rotate_latlon`] move
//! between the two frames; everything after that is the regular lat/lon inverse
//! in rotated coordinates.
//!
//! Tests for this family live with the forward-geolocation round trips in
//! [`super`], which is where the rotated grid's fixture is exercised.

use super::latlon::{LatLonParams, eastward_lon_span, latlon_inverse};
use super::{DEG2RAD, GridIndex, RAD2DEG, axis_position, enclosing_lon_arc, snap_to_range};

/// A regular lat/lon grid laid out on a *rotated* sphere: the geographic south
/// pole is moved to `(south_pole_lat, south_pole_lon)` and the sphere spun by
/// `angle_of_rotation` about the new polar axis. COSMO, DWD ICON-EU, and
/// Environment Canada HRDPS/RDPS publish their limited-area grids this way.
///
/// The grid is evenly spaced in the *rotated* coordinates (`lat_first..lat_last`
/// by `lon_first..lon_last`), so the corner fields are rotated-frame degrees,
/// not geographic. Locating a geographic point means rotating it into that
/// frame first, then indexing exactly like [`latlon_inverse`].
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RotatedLatLonParams {
    pub ni: u32,
    pub nj: u32,
    /// First/last grid-point coordinates **in the rotated frame** (degrees).
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
    /// Geographic latitude of the projection's southern pole (degrees).
    pub south_pole_lat: f64,
    /// Geographic longitude of the projection's southern pole (degrees).
    pub south_pole_lon: f64,
    /// Rotation about the new polar axis (degrees).
    pub angle_of_rotation: f64,
}

/// Rotation-matrix terms shared by [`unrotate_latlon`] and [`rotate_latlon`].
/// The unrotate map is `geo = M · rotated` with `M` orthonormal; the inverse
/// rotate map is `rotated = Mᵀ · geo`.
fn rotation_terms(south_pole_lat: f64, south_pole_lon: f64) -> (f64, f64, f64, f64) {
    let t = -(90.0 + south_pole_lat);
    let o = -south_pole_lon;
    let (sin_t, cos_t) = (t * DEG2RAD).sin_cos();
    let (sin_o, cos_o) = (o * DEG2RAD).sin_cos();
    (sin_t, cos_t, sin_o, cos_o)
}

/// Convert a point from the rotated frame to geographic coordinates. Matches
/// eccodes' `unrotate` (`grib_geography.cc`) — the routine that produces a
/// §3.1 grid's geographic point coordinates — so a Fieldglass warp resolves to
/// the same lat/lon eccodes' iterator reports.
pub fn unrotate_latlon(
    rlat: f64,
    rlon: f64,
    angle_of_rotation: f64,
    south_pole_lat: f64,
    south_pole_lon: f64,
) -> (f64, f64) {
    let (sin_lat, cos_lat) = (rlat * DEG2RAD).sin_cos();
    let (sin_lon, cos_lon) = (rlon * DEG2RAD).sin_cos();
    let xd = cos_lon * cos_lat;
    let yd = sin_lon * cos_lat;
    let zd = sin_lat;

    let (sin_t, cos_t, sin_o, cos_o) = rotation_terms(south_pole_lat, south_pole_lon);
    let x = cos_t * cos_o * xd + sin_o * yd + sin_t * cos_o * zd;
    let y = -cos_t * sin_o * xd + cos_o * yd - sin_t * sin_o * zd;
    let z = (-sin_t * xd + cos_t * zd).clamp(-1.0, 1.0);

    let lat = z.asin() * RAD2DEG;
    // eccodes subtracts the rotation angle from the geographic longitude last.
    let lon = y.atan2(x) * RAD2DEG - angle_of_rotation;
    (lat, lon)
}

/// Inverse of [`unrotate_latlon`]: geographic `(lat, lon)` → rotated-frame
/// `(rlat, rlon)`. `M` is orthonormal so the inverse is its transpose `Mᵀ`;
/// the `angle_of_rotation` term is undone by adding it back to the longitude
/// before rotating. This is the direction a warp needs — output geographic
/// point to source-grid coordinates.
pub fn rotate_latlon(
    lat: f64,
    lon: f64,
    angle_of_rotation: f64,
    south_pole_lat: f64,
    south_pole_lon: f64,
) -> (f64, f64) {
    let (sin_lat, cos_lat) = (lat * DEG2RAD).sin_cos();
    let (sin_lon, cos_lon) = ((lon + angle_of_rotation) * DEG2RAD).sin_cos();
    let x = cos_lon * cos_lat;
    let y = sin_lon * cos_lat;
    let z = sin_lat;

    let (sin_t, cos_t, sin_o, cos_o) = rotation_terms(south_pole_lat, south_pole_lon);
    // Transpose of the unrotate matrix.
    let xd = cos_t * cos_o * x - cos_t * sin_o * y - sin_t * z;
    let yd = sin_o * x + cos_o * y;
    let zd = (sin_t * cos_o * x - sin_t * sin_o * y + cos_t * z).clamp(-1.0, 1.0);

    let rlat = zd.asin() * RAD2DEG;
    let rlon = yd.atan2(xd) * RAD2DEG;
    (rlat, rlon)
}

/// Precomputed inverse map for a rotated lat/lon grid. Caches the rotated-frame
/// corner geometry as a plain [`LatLonParams`] so `inverse` rotates the query
/// into the rotated frame and then reuses [`latlon_inverse`]. Build once
/// outside the warp loop; call [`Self::inverse`] per output pixel.
pub struct RotatedLatLonProjector {
    params: RotatedLatLonParams,
    rotated_grid: LatLonParams,
}

impl RotatedLatLonProjector {
    pub fn new(params: RotatedLatLonParams) -> Self {
        let rotated_grid = LatLonParams {
            ni: params.ni,
            nj: params.nj,
            lat_first: params.lat_first,
            lon_first: params.lon_first,
            lat_last: params.lat_last,
            lon_last: params.lon_last,
        };
        Self {
            params,
            rotated_grid,
        }
    }

    /// Project geographic `(lat, lon)` back to the source-grid fractional
    /// index, or `None` when the point falls outside the grid coverage.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        let (rlat, rlon) = rotate_latlon(
            lat,
            lon,
            self.params.angle_of_rotation,
            self.params.south_pole_lat,
            self.params.south_pole_lon,
        );
        // The rotation arithmetic carries ~1e-14° of round-off, enough to push a
        // point sitting exactly on a grid edge a hair outside `latlon_inverse`'s
        // strict inclusive bounds and spuriously reject it. Snap coordinates
        // within EDGE_EPS of an edge back onto it. EDGE_EPS (1e-9° ≈ 0.1 mm) is
        // far above the round-off and far below any real grid spacing (≥0.01°),
        // so it never reclassifies a genuinely off-grid point.
        const EDGE_EPS: f64 = 1e-9;
        let rlat = snap_to_range(rlat, self.params.lat_first, self.params.lat_last, EDGE_EPS);
        let rlon = snap_to_range(rlon, self.params.lon_first, self.params.lon_last, EDGE_EPS);
        latlon_inverse(&self.rotated_grid, rlat, rlon)
    }

    /// Geographic lat/lon bounding box of the grid, as
    /// `(lat_min, lat_max, lon_min, lon_max)`. A rotated grid's edges are
    /// straight in the rotated frame but curve in geographic coordinates, with
    /// extrema generally in the interior of an edge — so walk a dense sample of
    /// the perimeter and unrotate each point, mirroring the planar
    /// [`super::PlanarGridProjector::lonlat_bbox`].
    pub fn lonlat_bbox(&self) -> (f64, f64, f64, f64) {
        // 512 samples/edge pins the closest-to-pole latitude tightly while
        // staying a trivial ~2k unrotations regardless of grid size.
        const PER_EDGE: u32 = 512;
        let p = &self.params;
        let mut lat_min = f64::INFINITY;
        let mut lat_max = f64::NEG_INFINITY;
        let mut lons: Vec<f64> = Vec::with_capacity(4 * (PER_EDGE as usize + 1));
        let mut visit = |rlat: f64, rlon: f64| {
            let (lat, lon) = unrotate_latlon(
                rlat,
                rlon,
                p.angle_of_rotation,
                p.south_pole_lat,
                p.south_pole_lon,
            );
            lat_min = lat_min.min(lat);
            lat_max = lat_max.max(lat);
            lons.push(lon.rem_euclid(360.0));
        };
        // Walk the row edges along the grid's true (eastward) span. A rotated
        // grid whose columns cross the rotated antimeridian — ECCC's HRDPS
        // continental grid runs 345° → 42° — reports `lon_last` numerically
        // below `lon_first`, and `lon_last - lon_first` would sweep the ~300°
        // complement arc of rotated longitudes that aren't in the grid,
        // inflating the box to nearly the whole globe. Mirrors the unwrap in
        // `latlon_inverse` (which this projector delegates to).
        let east_span = eastward_lon_span(p.lon_first, p.lon_last);
        for k in 0..=PER_EDGE {
            let t = k as f64 / PER_EDGE as f64;
            let rlat = p.lat_first + t * (p.lat_last - p.lat_first);
            let rlon = p.lon_first + t * east_span;
            visit(rlat, p.lon_first); // left edge
            visit(rlat, p.lon_last); // right edge
            visit(p.lat_first, rlon); // first-row edge
            visit(p.lat_last, rlon); // last-row edge
        }
        let (lon_min, lon_max) = enclosing_lon_arc(&mut lons);
        (lat_min, lat_max, lon_min, lon_max)
    }
}

// Forward geolocation: grid index → (lat, lon).

/// `(lat, lon)` — **geographic**, not rotated — of grid point `(i, j)` on a
/// rotated lat/lon grid. The grid is evenly spaced in the *rotated* frame, so
/// the point is placed there first and then unrotated onto the sphere with
/// [`unrotate_latlon`] (the same routine, matching eccodes, that the bbox walk
/// uses).
pub fn rotated_latlon_point(p: &RotatedLatLonParams, i: u32, j: u32) -> Option<(f64, f64)> {
    if p.ni < 2 || p.nj < 2 {
        return None;
    }
    let east_span = eastward_lon_span(p.lon_first, p.lon_last);
    let rlat = axis_position(p.lat_first, p.lat_last, p.nj, j);
    let rlon = p.lon_first + i as f64 * (east_span / (p.ni as f64 - 1.0));
    Some(unrotate_latlon(
        rlat,
        rlon,
        p.angle_of_rotation,
        p.south_pole_lat,
        p.south_pole_lon,
    ))
}
