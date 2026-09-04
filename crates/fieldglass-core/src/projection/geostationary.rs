//! Geostationary / space-view perspective grids — GRIB2 template 3.90, CF
//! `grid_mapping_name = "geostationary"`.
//!
//! The grid axes are scan angles seen from the satellite, not distances on a
//! plane, so this family does not implement [`super::PlanarGridProjector`]; it
//! carries its own inverse and its own corner walk. Points whose line of sight
//! misses the Earth have no grid index at all.

use super::{DEFAULT_SNAP_EPS, DEG2RAD, GridIndex, enclosing_lon_arc, snap_to_range};

/// A regular grid in geostationary **scan-angle** space: a satellite parked
/// over `sub_lon_deg` views the Earth ellipsoid, and each grid point maps to a
/// pair of scan angles `(x, y)` in radians. Unlike the spherical projectors,
/// this one is ellipsoidal (`r_eq` ≠ `r_pol`) — GOES uses GRS80 and Meteosat
/// uses WGS84 — so geolocation goes through geodetic ↔ geocentric latitude.
///
/// The grid layout is given in scan-angle space (`x0`/`dx_rad`, `y0`/`dy_rad`)
/// rather than as projected metres, so the same params describe a GRIB2 §3.90
/// grid (scan angles derived from the apparent Earth diameter) and a GOES ABI
/// fixed grid (1-D `x`/`y` radian coordinate variables, a follow-up in #168).
///
/// Inverse is the GOES-R fixed-grid algorithm (GOES-R PUG Vol. 3 / NOAA STAR),
/// which is the analytic inverse of the CGMS LRIT/HRIT forward that GRIB2 §3.90
/// encodes. Off-disk points (no Earth intersection) invert to `None` so the
/// limb renders transparent.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GeostationaryParams {
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// Distance from the Earth's **centre** to the satellite, metres
    /// (`perspective_point_height + r_eq` for CF; `Nr · r_eq` for GRIB2 §3.90).
    pub h_metres: f64,
    /// Ellipsoid semi-major axis (equatorial radius), metres.
    pub r_eq: f64,
    /// Ellipsoid semi-minor axis (polar radius), metres.
    pub r_pol: f64,
    /// Sub-satellite longitude (`longitude_of_projection_origin`), degrees.
    pub sub_lon_deg: f64,
    /// `true` ⇒ sweep angle about the `x` axis (GOES-R); `false` ⇒ about the
    /// `y` axis (Meteosat / EUMETSAT). Swaps the two scan-angle rotations.
    pub sweep_x: bool,
    /// Scan angle (radians) at column `i = 0`.
    pub x0: f64,
    /// Signed scan-angle increment per column (scan direction baked into the
    /// sign, like the planar grids' `dx_metres`).
    pub dx_rad: f64,
    /// Scan angle (radians) at row `j = 0`.
    pub y0: f64,
    /// Signed scan-angle increment per row.
    pub dy_rad: f64,
}

/// Ellipsoid and sub-satellite terms derived once from a
/// [`GeostationaryParams`], since a warp reuses them for every pixel.
///
/// `pub` so projector helpers can hand them around; the fields are private.
#[derive(Debug, Clone, Copy)]
pub struct GeostationaryConstants {
    sub_lon_rad: f64,
    /// `(r_pol/r_eq)²` — folds the geodetic→geocentric latitude conversion.
    ratio2: f64,
    /// `(r_eq/r_pol)²` — appears in the geocentric→geodetic step and the
    /// off-disk visibility test.
    inv_ratio2: f64,
    /// First eccentricity squared, `1 - (r_pol/r_eq)²`.
    e2: f64,
}

fn geostationary_constants(p: &GeostationaryParams) -> GeostationaryConstants {
    let ratio2 = (p.r_pol / p.r_eq) * (p.r_pol / p.r_eq);
    GeostationaryConstants {
        sub_lon_rad: p.sub_lon_deg * DEG2RAD,
        ratio2,
        inv_ratio2: 1.0 / ratio2,
        e2: 1.0 - ratio2,
    }
}

/// Forward geolocation step: geodetic `(lat, lon)` in degrees → scan angles
/// `(x, y)` in radians, or `None` when the point is off the visible disk (the
/// line of sight from the satellite misses the ellipsoid).
fn geostationary_scan_angles(
    p: &GeostationaryParams,
    k: &GeostationaryConstants,
    lat: f64,
    lon: f64,
) -> Option<(f64, f64)> {
    let lat_r = lat * DEG2RAD;
    // Geocentric latitude of the surface point, then its geocentric radius.
    let phi_c = (k.ratio2 * lat_r.tan()).atan();
    let cos_c = phi_c.cos();
    let r_c = p.r_pol / (1.0 - k.e2 * cos_c * cos_c).sqrt();

    let d_lon = lon * DEG2RAD - k.sub_lon_rad;
    let sx = p.h_metres - r_c * cos_c * d_lon.cos();
    let sy = -r_c * cos_c * d_lon.sin();
    let sz = r_c * phi_c.sin();

    // Off-disk when the satellite's line of sight passes outside the Earth:
    // H·(H − sx) < sy² + (r_eq/r_pol)²·sz² (GOES-R PUG visibility test). This
    // also rejects the far hemisphere, where sx > H makes the left side
    // negative.
    if p.h_metres * (p.h_metres - sx) < sy * sy + k.inv_ratio2 * sz * sz {
        return None;
    }

    let norm = (sx * sx + sy * sy + sz * sz).sqrt();
    let (x, y) = if p.sweep_x {
        ((-sy / norm).asin(), (sz / sx).atan())
    } else {
        ((-sy / sx).atan(), (sz / norm).asin())
    };
    Some((x, y))
}

/// Precomputed inverse map for a geostationary grid. Owns the ellipsoid /
/// sub-satellite constants, invariant across every output pixel of a warp.
#[derive(Debug)]
pub struct GeostationaryProjector {
    /// The grid this projector was built for.
    pub params: GeostationaryParams,
    constants: GeostationaryConstants,
}

impl GeostationaryProjector {
    /// Precompute the ellipsoid constants for `params`. Build once outside a
    /// warp loop.
    pub fn new(params: GeostationaryParams) -> Self {
        let constants = geostationary_constants(&params);
        Self { params, constants }
    }

    /// Geodetic `(lat, lon)` in degrees → scan angles `(x, y)` in radians, or
    /// `None` off the visible disk.
    pub fn scan_angles(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        geostationary_scan_angles(&self.params, &self.constants, lat, lon)
    }

    /// Fractional grid index for a geographic point, or `None` when the point
    /// is off the visible disc or outside the grid's extent.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        let p = &self.params;
        if p.ni < 2 || p.nj < 2 || p.dx_rad == 0.0 || p.dy_rad == 0.0 {
            return None;
        }
        let (x, y) = self.scan_angles(lat, lon)?;
        // The same edge snap the planar projectors apply, for the same reason.
        // `scan_angles` is a ray/ellipsoid intersection, so it does not return
        // a window bound exactly: a grid point sitting *on* the first or last
        // row or column can come back a few ULPs outside it and be refused.
        // The symptom is not a clean missing border but a speckled one, because
        // whether a given edge cell survives depends on that point's own
        // arithmetic — and a refused point renders as a transparent pixel.
        let (i_max, j_max) = (p.ni as f64 - 1.0, p.nj as f64 - 1.0);
        // Only the `Cells` form means anything here: this grid's spacing is an
        // angle, so a tolerance in metres would be divided by radians. That is
        // also why the round trip closes to float noise and a cell fraction is
        // the right shape — see `DEFAULT_SNAP_EPS`.
        let (eps_i, eps_j) = DEFAULT_SNAP_EPS.per_axis(p.dx_rad, p.dy_rad);
        let i = snap_to_range((x - p.x0) / p.dx_rad, 0.0, i_max, eps_i);
        let j = snap_to_range((y - p.y0) / p.dy_rad, 0.0, j_max, eps_j);
        if i < 0.0 || i > i_max || j < 0.0 || j > j_max {
            return None;
        }
        Some(GridIndex { i, j })
    }

    /// Forward geolocation: scan angles `(x, y)` in radians → geodetic
    /// `(lat, lon)` in degrees, or `None` when the line of sight misses the
    /// Earth (an off-disk / limb sample). Inverse of [`Self::scan_angles`].
    ///
    /// Intersects the satellite's view ray with the Earth ellipsoid (GOES-R
    /// PUG §5.1.2.8.1). The unit look direction in the sub-satellite frame is
    /// `(cos x cos y, −sin x, cos x sin y)` for an `x`-sweep (GOES) and
    /// `(cos x cos y, −sin x cos y, sin y)` for a `y`-sweep (Meteosat); both
    /// feed the same ray/ellipsoid quadratic.
    pub fn scan_to_lonlat(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let p = &self.params;
        let k = &self.constants;
        let (cx, sx) = (x.cos(), x.sin());
        let (cy, sy) = (y.cos(), y.sin());
        // Unit look direction (satellite → Earth) in the (sx, sy, sz) frame.
        let (dx, dy, dz) = if p.sweep_x {
            (cx * cy, -sx, cx * sy)
        } else {
            (cx * cy, -sx * cy, sy)
        };
        // The surface point P = S − r_s·d, with the satellite at S = (H, 0, 0),
        // lies on the ellipsoid (x²+y²)/r_eq² + z²/r_pol² = 1, giving
        //   a·r_s² + b·r_s + c = 0,   a = dx²+dy²+(r_eq/r_pol)²·dz²,
        //   b = −2H·dx,   c = H² − r_eq².
        // The near root (smaller r_s) is the visible face; a negative
        // discriminant means the ray misses the disk (limb / off-disk).
        let h = p.h_metres;
        let a = dx * dx + dy * dy + k.inv_ratio2 * dz * dz;
        let b = -2.0 * h * dx;
        let c = h * h - p.r_eq * p.r_eq;
        let disc = b * b - 4.0 * a * c;
        if disc < 0.0 || a <= 0.0 {
            return None;
        }
        let r_s = (-b - disc.sqrt()) / (2.0 * a);
        let px = h - r_s * dx;
        let py = -r_s * dy;
        let pz = r_s * dz;
        // Geocentric surface point → geodetic latitude; longitude is the
        // offset from the sub-satellite meridian. At a geographic pole
        // (px² + py² == 0, unreachable on a real disk) the ratio is ±∞ and
        // `atan` returns ±90°, the correct limit — no NaN.
        let lat = (k.inv_ratio2 * pz / (px * px + py * py).sqrt()).atan();
        let lon = k.sub_lon_rad + py.atan2(px);
        Some((lat / DEG2RAD, lon / DEG2RAD))
    }

    /// Axis-aligned lat/lon bounding box of the grid's **on-disk** extent,
    /// `(lat_min, lat_max, lon_min, lon_max)`, or `None` when the whole grid
    /// perimeter is off-disk (a full disk whose edges are all limb) and the
    /// caller should fall back to a generous default box.
    ///
    /// Walks the scan-angle perimeter, forward-projects each sample with
    /// [`Self::scan_to_lonlat`], and skips off-disk (limb) samples. The
    /// longitude span is the minimum enclosing arc of the on-disk samples
    /// (`enclosing_lon_arc`) — the same logic the planar projectors use — so
    /// a sector straddling the ±180° antimeridian still frames tightly. Like
    /// [`super::PlanarGridProjector::lonlat_bbox`], the boundary walk suffices: the
    /// lat/lon extrema of this smooth map fall on the grid perimeter, not its
    /// interior.
    ///
    /// A degenerate grid (fewer than two points on a side, or zero scan-angle
    /// spacing) has no perimeter to walk and also returns `None`, mirroring the
    /// guard in [`Self::inverse`].
    pub fn lonlat_bbox(&self) -> Option<(f64, f64, f64, f64)> {
        // Subdivisions per edge, matching the planar perimeter walk: cheap and
        // fine enough to pin a bowed edge's extremum.
        const PER_EDGE: u32 = 512;
        let p = &self.params;
        if p.ni < 2 || p.nj < 2 || p.dx_rad == 0.0 || p.dy_rad == 0.0 {
            return None;
        }
        let x1 = p.x0 + (p.ni as f64 - 1.0) * p.dx_rad;
        let y1 = p.y0 + (p.nj as f64 - 1.0) * p.dy_rad;

        let mut lat_min = f64::INFINITY;
        let mut lat_max = f64::NEG_INFINITY;
        let mut lons: Vec<f64> = Vec::with_capacity(4 * (PER_EDGE as usize + 1));
        let mut visit = |x: f64, y: f64| {
            if let Some((lat, lon)) = self.scan_to_lonlat(x, y) {
                lat_min = lat_min.min(lat);
                lat_max = lat_max.max(lat);
                lons.push(lon.rem_euclid(360.0));
            }
        };
        for n in 0..=PER_EDGE {
            let t = n as f64 / PER_EDGE as f64;
            visit(p.x0 + t * (x1 - p.x0), p.y0); // y = y0 edge
            visit(p.x0 + t * (x1 - p.x0), y1); // y = y1 edge
            visit(p.x0, p.y0 + t * (y1 - p.y0)); // x = x0 edge
            visit(x1, p.y0 + t * (y1 - p.y0)); // x = x1 edge
        }
        if lons.is_empty() {
            return None;
        }
        let (lon_min, lon_max) = enclosing_lon_arc(&mut lons);
        Some((lat_min, lat_max, lon_min, lon_max))
    }
}

// Forward geolocation: grid index → (lat, lon).

impl GeostationaryProjector {
    /// `(lat, lon)` of grid point `(i, j)`, or `None` when that pixel's line of
    /// sight misses the Earth — the corners of a full-disk image are space, and
    /// an exporter must skip them rather than invent a coordinate.
    pub fn grid_point_lonlat(&self, i: u32, j: u32) -> Option<(f64, f64)> {
        let p = &self.params;
        self.scan_to_lonlat(p.x0 + i as f64 * p.dx_rad, p.y0 + j as f64 * p.dy_rad)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::near;

    /// GOES-East fixed-grid constants (GRS80 ellipsoid; GOES-R PUG). The full
    /// disk spans ~0.151 rad each way; use a coarse 11×11 layout covering a
    /// central sub-sector so the sub-satellite point lands on an exact index
    /// and near-limb points fall off-grid.
    fn goes_east_params() -> GeostationaryParams {
        // Half-width of the scan-angle window, ~5.7° in radians — a central
        // sector well inside the ~8.7° apparent radius of the Earth's limb.
        let half = 0.10;
        GeostationaryParams {
            ni: 11,
            nj: 11,
            h_metres: 42_164_160.0,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314_14,
            sub_lon_deg: -75.0,
            sweep_x: true,
            x0: -half,
            dx_rad: 2.0 * half / 10.0,
            y0: -half,
            dy_rad: 2.0 * half / 10.0,
        }
    }

    #[test]
    fn geostationary_subsatellite_maps_to_grid_centre() {
        let proj = GeostationaryProjector::new(goes_east_params());
        // The sub-satellite point sits at scan angle (0, 0) → grid centre.
        let (x, y) = proj.scan_angles(0.0, -75.0).expect("sub-sat visible");
        assert!(near(x, 0.0, 1e-9), "x = {x}");
        assert!(near(y, 0.0, 1e-9), "y = {y}");
        let idx = proj.inverse(0.0, -75.0).expect("sub-sat on grid");
        assert!(near(idx.i, 5.0, 1e-6), "i = {}", idx.i);
        assert!(near(idx.j, 5.0, 1e-6), "j = {}", idx.j);
    }

    #[test]
    fn geostationary_scan_angle_round_trips_to_index() {
        let p = goes_east_params();
        let proj = GeostationaryProjector::new(p);
        // A point east and north of the sub-satellite point produces positive
        // scan angles whose index recovers via the linear layout.
        let (lat, lon) = (20.0, -60.0);
        let (x, y) = proj.scan_angles(lat, lon).expect("visible");
        assert!(x > 0.0 && y > 0.0, "expected +x,+y got ({x},{y})");
        let idx = proj.inverse(lat, lon).expect("on grid");
        assert!(near(idx.i, (x - p.x0) / p.dx_rad, 1e-9));
        assert!(near(idx.j, (y - p.y0) / p.dy_rad, 1e-9));
    }

    #[test]
    fn geostationary_off_disk_is_none() {
        let proj = GeostationaryProjector::new(goes_east_params());
        // Antipodal-ish longitude is on the far hemisphere — not visible.
        assert!(proj.scan_angles(0.0, 105.0).is_none(), "far side visible?");
        assert!(proj.inverse(0.0, 105.0).is_none(), "far side on grid?");
        // A near-side point beyond the 11×11 window inverts off-grid.
        assert!(proj.inverse(75.0, -75.0).is_none(), "polar point on grid?");
        // Non-finite inputs reject.
        assert!(proj.inverse(f64::NAN, -75.0).is_none());
        assert!(proj.inverse(0.0, f64::INFINITY).is_none());
    }

    #[test]
    fn geostationary_sweep_axis_swaps_angles() {
        let mut p = goes_east_params();
        let (x_goes, y_goes) = GeostationaryProjector::new(p)
            .scan_angles(20.0, -60.0)
            .unwrap();
        p.sweep_x = false;
        let (x_met, y_met) = GeostationaryProjector::new(p)
            .scan_angles(20.0, -60.0)
            .unwrap();
        // The two conventions order the scan rotations differently, so the
        // angles differ; near the centre they stay close but not identical.
        assert!(
            (x_goes - x_met).abs() > 1e-9 || (y_goes - y_met).abs() > 1e-9,
            "sweep axis had no effect"
        );
    }

    #[test]
    fn geostationary_forward_round_trips_scan_angles() {
        // scan_to_lonlat must invert scan_angles for both sweep conventions.
        for sweep_x in [true, false] {
            let mut p = goes_east_params();
            p.sweep_x = sweep_x;
            let proj = GeostationaryProjector::new(p);
            for &(lat, lon) in &[(0.0, -75.0), (20.0, -60.0), (-15.0, -85.0), (5.0, -70.0)] {
                let (x, y) = proj.scan_angles(lat, lon).expect("visible");
                let (lat2, lon2) = proj.scan_to_lonlat(x, y).expect("on disk");
                assert!(
                    near(lat2, lat, 1e-6),
                    "lat {lat} -> {lat2} (sweep_x={sweep_x})"
                );
                assert!(
                    near(lon2, lon, 1e-6),
                    "lon {lon} -> {lon2} (sweep_x={sweep_x})"
                );
            }
        }
    }

    #[test]
    fn geostationary_forward_off_disk_is_none() {
        let proj = GeostationaryProjector::new(goes_east_params());
        // Scan angles beyond the ~0.152 rad apparent radius miss the disk.
        assert!(proj.scan_to_lonlat(0.3, 0.3).is_none(), "corner off-disk");
        assert!(proj.scan_to_lonlat(f64::NAN, 0.0).is_none());
        assert!(proj.scan_to_lonlat(0.0, f64::INFINITY).is_none());
    }

    #[test]
    fn geostationary_bbox_frames_sector_tightly() {
        // A modest off-centre sub-sector (north-west of the sub-satellite
        // point), like a GOES CONUS sector, well inside the apparent disk.
        let mut p = goes_east_params();
        p.ni = 21;
        p.nj = 21;
        p.x0 = -0.06;
        p.dx_rad = 0.06 / 20.0; // x ∈ [-0.06, 0.0]
        p.y0 = 0.02;
        p.dy_rad = 0.06 / 20.0; // y ∈ [0.02, 0.08]
        let proj = GeostationaryProjector::new(p);
        let (lat_min, lat_max, lon_min, lon_max) = proj.lonlat_bbox().expect("on-disk sector");

        // The box must enclose every grid corner's ground point.
        let x1 = p.x0 + (p.ni as f64 - 1.0) * p.dx_rad;
        let y1 = p.y0 + (p.nj as f64 - 1.0) * p.dy_rad;
        for &(x, y) in &[(p.x0, p.y0), (x1, p.y0), (p.x0, y1), (x1, y1)] {
            let (lat, lon) = proj.scan_to_lonlat(x, y).expect("corner on disk");
            assert!(
                lat >= lat_min - 1e-9 && lat <= lat_max + 1e-9,
                "lat {lat} outside box"
            );
            assert!(
                lon >= lon_min - 1e-9 && lon <= lon_max + 1e-9,
                "lon {lon} outside box"
            );
        }

        // It frames the sector, strictly inside the ±90° hemisphere fallback.
        let lon0 = p.sub_lon_deg;
        assert!(
            lat_min > -90.0 && lat_max < 90.0,
            "lat {lat_min}..{lat_max}"
        );
        assert!(
            lon_min > lon0 - 90.0 && lon_max < lon0 + 90.0,
            "lon {lon_min}..{lon_max}"
        );
        // The window sits north of the equator (y > 0) and runs up to the
        // sub-satellite meridian on its east edge (x = 0), so the frame does
        // too: entirely north, entirely at or west of the sub-lon.
        assert!(
            lat_min > 0.0,
            "sector is north of equator, lat_min {lat_min}"
        );
        assert!(
            lon_max <= lon0 + 1e-9,
            "sector is west of sub-lon, lon_max {lon_max}"
        );
        assert!(
            lon_min < lon0,
            "sector should extend west of sub-lon, lon_min {lon_min}"
        );
        // And the span is tight, nothing like the 180° fallback.
        assert!(lat_max - lat_min < 40.0, "lat span {}", lat_max - lat_min);
        assert!(lon_max - lon_min < 40.0, "lon span {}", lon_max - lon_min);
    }

    #[test]
    fn geostationary_bbox_full_disk_falls_back() {
        // A grid whose square perimeter lies entirely outside the apparent
        // disk (half-width 0.16 rad > ~0.152 rad limb) has no on-disk
        // perimeter sample, so no tight box is available.
        let mut p = goes_east_params();
        let half = 0.16;
        p.x0 = -half;
        p.y0 = -half;
        p.dx_rad = 2.0 * half / (p.ni as f64 - 1.0);
        p.dy_rad = 2.0 * half / (p.nj as f64 - 1.0);
        let proj = GeostationaryProjector::new(p);
        assert!(
            proj.lonlat_bbox().is_none(),
            "full-disk perimeter should be off-disk"
        );
    }
}
