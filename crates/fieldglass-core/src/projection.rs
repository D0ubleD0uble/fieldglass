//! Forward and inverse projections for the source grids that the GRIB
//! readers decode (regular lat/lon, Gaussian lat/lon, Lambert Conformal).
//!
//! The render pipeline uses the inverse direction (output `(lat, lon)` →
//! source grid index) when warping into a target projection raster; the
//! forward direction is exposed for tests and target-projection consumers
//! (Web Mercator etc. — tracked under separate issues).
//!
//! Math references:
//!
//! - Lambert Conformal Conic — Snyder, "Map Projections: A Working
//!   Manual" (USGS PP-1395), pp. 104-110. Two-standard-parallel form,
//!   with a tangent-cone branch when `latin1 == latin2`.
//! - Gauss–Legendre quadrature nodes for Gaussian grid latitudes —
//!   Press et al., "Numerical Recipes", §4.6. Newton-Raphson on the
//!   Legendre polynomial seeded with Chebyshev points.
//! - Polar stereographic — Snyder, PP-1395 §21 (sphere, polar aspect),
//!   eqs 21-33/21-34 (forward) and 20-14/20-17 (inverse). The pole scale
//!   factor `k₀ = (1 + sin|LaD|)/2` follows the latitude of true scale
//!   `LaD`: GRIB1 fixes it at ±60° (`k₀ ≈ 0.93301270…`), while GRIB2 §3.20
//!   carries `LaD` explicitly (e.g. true scale at the pole → `k₀ = 1`).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::f64::consts::PI;

/// Earth radius used by Lambert projection math. WMO `shapeOfTheEarth = 6`
/// (spherical, R = 6 371 229 m) is the GRIB default; other shapes resolve
/// to nearby radii and the projection error is negligible at the scales
/// Fieldglass renders.
///
/// TODO: §3 GDS carries the actual `shape_of_earth` (and for oblate
/// spheroids: custom radius / axis lengths). Plumb that through
/// `LambertParams` / `GaussianParams` once we get a fixture whose
/// projection error against eccodes is visible at pixel scale.
const EARTH_RADIUS_M: f64 = 6_371_229.0;

const DEG2RAD: f64 = PI / 180.0;
const RAD2DEG: f64 = 180.0 / PI;

/// Output of any inverse map: a fractional source-grid index, or `None`
/// when the requested `(lat, lon)` lies outside the grid coverage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridIndex {
    pub i: f64,
    pub j: f64,
}

// ---------------------------------------------------------------------------
// Regular lat/lon (GRIB1 grid_type 0, GRIB2 template 3.0)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLonParams {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
}

pub fn latlon_inverse(p: &LatLonParams, lat: f64, lon: f64) -> Option<GridIndex> {
    if !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    let min_lon = p.lon_first.min(p.lon_last);
    let max_lon = p.lon_first.max(p.lon_last);
    // Shift `lon` into the grid's longitude range without spinning a while
    // loop on pathological inputs (was unbounded if `lon` was huge).
    let norm_lon = if (min_lon..=max_lon).contains(&lon) {
        lon
    } else {
        let shifted = min_lon + (lon - min_lon).rem_euclid(360.0);
        if !(min_lon..=max_lon).contains(&shifted) {
            return None;
        }
        shifted
    };
    let min_lat = p.lat_first.min(p.lat_last);
    let max_lat = p.lat_first.max(p.lat_last);
    if !(min_lat..=max_lat).contains(&lat) {
        return None;
    }
    if p.ni < 2 || p.nj < 2 {
        // A 1×N or N×1 grid is degenerate for linear interpolation; no
        // sane caller asks for one but the math would divide by zero.
        return None;
    }
    let ew = (p.lon_last - p.lon_first) / (p.ni as f64 - 1.0);
    let ns = (p.lat_last - p.lat_first) / (p.nj as f64 - 1.0);
    Some(GridIndex {
        i: (norm_lon - p.lon_first) / ew,
        j: (lat - p.lat_first) / ns,
    })
}

// ---------------------------------------------------------------------------
// Mercator (GRIB2 template 3.10)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
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
fn mercator_ordinate(lat_deg: f64) -> f64 {
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
/// [`latlon_inverse`] — the grid lengths (`Di`/`Dj` in metres) and the
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
    let min_lon = p.lon_first.min(p.lon_last);
    let max_lon = p.lon_first.max(p.lon_last);
    let norm_lon = if (min_lon..=max_lon).contains(&lon) {
        lon
    } else {
        let shifted = min_lon + (lon - min_lon).rem_euclid(360.0);
        if !(min_lon..=max_lon).contains(&shifted) {
            return None;
        }
        shifted
    };
    let min_lat = p.lat_first.min(p.lat_last);
    let max_lat = p.lat_first.max(p.lat_last);
    if !(min_lat..=max_lat).contains(&lat) {
        return None;
    }
    // Rows are evenly spaced in the Mercator ordinate, not in latitude; columns
    // are evenly spaced in longitude.
    let ew = (p.lon_last - p.lon_first) / (p.ni as f64 - 1.0);
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
    if ew == 0.0 || ns == 0.0 {
        // A grid whose longitude or latitude extent collapses to a point has no
        // spatial extent to interpolate over.
        return None;
    }
    Some(GridIndex {
        i: (norm_lon - p.lon_first) / ew,
        j: (mercator_ordinate(lat) - y_first) / ns,
    })
}

// ---------------------------------------------------------------------------
// Gaussian latitude/longitude (GRIB1 grid_type 4, GRIB2 template 3.40)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GaussianParams {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
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
    lats_deg.sort_by(|a, b| b.partial_cmp(a).expect("Gaussian nodes are finite"));
    GAUSS_CACHE.with(|c| {
        c.borrow_mut().insert(n_parallels, lats_deg.clone());
    });
    lats_deg
}

/// Inverse map for a Gaussian source grid. **Builds a transient
/// [`GaussianProjector`] per call** — for warp loops use
/// [`GaussianProjector::new`] once outside the loop and call
/// [`GaussianProjector::inverse`] inside it.
pub fn gaussian_inverse(p: &GaussianParams, lat: f64, lon: f64) -> Option<GridIndex> {
    GaussianProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a Gaussian source grid. Holds the cached
/// row latitudes ordered to match the grid's `lat_first` → `lat_last`
/// scan direction, so `inverse` does one bracket search per call without
/// touching the global Gauss–Legendre cache or re-reversing the vec.
///
/// Build once outside the warp loop; call `inverse` per output pixel.
pub struct GaussianProjector {
    pub params: GaussianParams,
    row_lats: Vec<f64>,
    north_to_south: bool,
}

impl GaussianProjector {
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
        let min_lon = p.lon_first.min(p.lon_last);
        let max_lon = p.lon_first.max(p.lon_last);
        let norm_lon = if (min_lon..=max_lon).contains(&lon) {
            lon
        } else {
            let shifted = min_lon + (lon - min_lon).rem_euclid(360.0);
            if !(min_lon..=max_lon).contains(&shifted) {
                return None;
            }
            shifted
        };
        let ew = (p.lon_last - p.lon_first) / (p.ni as f64 - 1.0);
        let i = (norm_lon - p.lon_first) / ew;

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

// ---------------------------------------------------------------------------
// Lambert Conformal Conic (GRIB1 grid_type 3, GRIB2 template 3.30)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LambertParams {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    /// Latitude of true scale (`LaD`), in degrees.
    pub lad: f64,
    /// Orientation longitude (`LoV`), in degrees.
    pub lov: f64,
    /// Grid spacing in metres along x and y at the latitude of true scale.
    pub dx_metres: f64,
    pub dy_metres: f64,
    pub latin1: f64,
    pub latin2: f64,
}

/// `pub` so projector helpers can hand them around, but the fields are
/// private — callers shouldn't construct these directly.
#[derive(Debug, Clone, Copy)]
pub struct LambertConstants {
    n: f64,
    f_const: f64,
    rho0: f64,
    earth_r: f64,
}

fn lambert_constants(p: &LambertParams) -> LambertConstants {
    let lat1 = p.latin1 * DEG2RAD;
    let lat2 = p.latin2 * DEG2RAD;
    let lad = p.lad * DEG2RAD;
    let tan1 = (PI / 4.0 + lat1 / 2.0).tan();
    let tan2 = (PI / 4.0 + lat2 / 2.0).tan();
    let n = if (p.latin1 - p.latin2).abs() < 1e-9 {
        lat1.sin()
    } else {
        (lat1.cos() / lat2.cos()).ln() / (tan2 / tan1).ln()
    };
    let f_const = lat1.cos() * tan1.powf(n) / n;
    let rho0 = EARTH_RADIUS_M * f_const / (PI / 4.0 + lad / 2.0).tan().powf(n);
    LambertConstants {
        n,
        f_const,
        rho0,
        earth_r: EARTH_RADIUS_M,
    }
}

/// Forward Lambert: `(lat, lon)` in degrees → `(x, y)` in metres.
///
/// Lambert Conformal is undefined at the projection poles
/// (`tan(π/4 ± π/4) = ±∞`). Real-world Lambert grids cover continental
/// tiles and never reach the pole on their own pole side, so this is
/// academic — but callers passing a pole latitude here will get `±inf`
/// / `NaN`.
///
/// **Recomputes Lambert constants per call.** For warp loops use
/// [`LambertProjector`] which caches them once.
pub fn lambert_forward(p: &LambertParams, lat: f64, lon: f64) -> (f64, f64) {
    lambert_forward_with(&lambert_constants(p), p.lov, lat, lon)
}

fn lambert_forward_with(k: &LambertConstants, lov: f64, lat: f64, lon: f64) -> (f64, f64) {
    let lat_r = lat * DEG2RAD;
    // Wrap (lon − lov) into [-180, 180] *before* scaling by the cone constant.
    // Unlike the polar projector — whose `d_lon` only ever reaches `sin`/`cos`
    // and is therefore 360°-periodic — Lambert multiplies the difference by the
    // cone constant `n` before the trig, so an unwrapped 360° offset (e.g. a
    // query longitude in [-180, 180] against a `LoV` carried in [0, 360), as
    // NCEP/Eta files store it) shifts the cone angle by `n·360°` and throws the
    // point far outside the grid — which is why `equirectangular` rendered blank
    // for the Eta Lambert grid. The inverse-index path (`LambertProjector::
    // inverse`) routes through this forward map, so fixing it here is enough.
    let d_lon = ((lon - lov + 180.0).rem_euclid(360.0) - 180.0) * DEG2RAD;
    let rho = k.earth_r * k.f_const / (PI / 4.0 + lat_r / 2.0).tan().powf(k.n);
    let x = rho * (k.n * d_lon).sin();
    let y = k.rho0 - rho * (k.n * d_lon).cos();
    (x, y)
}

/// Inverse Lambert: `(x, y)` in metres → `(lat, lon)` in degrees. Same
/// pole + recompute caveats as [`lambert_forward`].
pub fn lambert_inverse_xy(p: &LambertParams, x: f64, y: f64) -> (f64, f64) {
    lambert_inverse_xy_with(&lambert_constants(p), p.lov, x, y)
}

fn lambert_inverse_xy_with(k: &LambertConstants, lov: f64, x: f64, y: f64) -> (f64, f64) {
    let dy = k.rho0 - y;
    let rho = k.n.signum() * (x * x + dy * dy).sqrt();
    let theta = x.atan2(dy);
    let lon = lov + (theta / k.n) * RAD2DEG;
    let lat = (2.0 * ((k.earth_r * k.f_const / rho).powf(1.0 / k.n)).atan() - PI / 2.0) * RAD2DEG;
    (lat, lon)
}

/// Inverse warp: `(lat, lon)` → fractional source grid index. Returns
/// `None` when the requested point's projected coordinates fall outside
/// the grid. **Recomputes Lambert constants per call** — for warp loops
/// prefer [`LambertProjector::inverse`] which caches the constants and
/// the forward-projected grid origin once.
pub fn lambert_inverse(p: &LambertParams, lat: f64, lon: f64) -> Option<GridIndex> {
    LambertProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a Lambert grid. Owns the cone constants
/// (`n`, `F`, `ρ₀`) and the forward-projected grid origin — both
/// invariant across every output pixel of a warp. Build once outside
/// the per-pixel loop; call [`Self::inverse`] inside it.
pub struct LambertProjector {
    pub params: LambertParams,
    constants: LambertConstants,
    origin: (f64, f64),
}

impl LambertProjector {
    pub fn new(params: LambertParams) -> Self {
        let constants = lambert_constants(&params);
        let origin =
            lambert_forward_with(&constants, params.lov, params.lat_first, params.lon_first);
        Self {
            params,
            constants,
            origin,
        }
    }

    /// Project `(lat, lon)` back to the source-grid fractional index.
    /// Returns `None` when the projected coordinates fall outside the
    /// `ni × nj` grid extent.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        if self.params.ni < 2
            || self.params.nj < 2
            || self.params.dx_metres == 0.0
            || self.params.dy_metres == 0.0
        {
            return None;
        }
        let (x, y) = lambert_forward_with(&self.constants, self.params.lov, lat, lon);
        if !x.is_finite() || !y.is_finite() {
            // Forward map hit a pole singularity. See `lambert_forward`.
            return None;
        }
        let i = (x - self.origin.0) / self.params.dx_metres;
        let j = (y - self.origin.1) / self.params.dy_metres;
        if i < 0.0 || i > self.params.ni as f64 - 1.0 || j < 0.0 || j > self.params.nj as f64 - 1.0
        {
            return None;
        }
        Some(GridIndex { i, j })
    }

    /// Forward-project a `(lat, lon)` through the cached constants. Used
    /// by warp setup to derive equirectangular target bounds from the
    /// four source corners.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        lambert_forward_with(&self.constants, self.params.lov, lat, lon)
    }

    /// Inverse-project a projected-metres `(x, y)` back to `(lat, lon)`.
    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        lambert_inverse_xy_with(&self.constants, self.params.lov, x, y)
    }

    /// Read-only access to the precomputed grid origin in projected
    /// metres. Useful for warp setup that wants to enumerate the
    /// non-origin corners.
    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }
}

// ---------------------------------------------------------------------------
// Polar Stereographic (GRIB1 grid_type 5, GRIB2 template 3.20)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarStereoParams {
    pub ni: u32,
    pub nj: u32,
    /// Latitude of the grid origin (first scanned point), degrees.
    pub lat_first: f64,
    /// Longitude of the grid origin (first scanned point), degrees.
    pub lon_first: f64,
    /// Orientation longitude (`LoV`) — meridian parallel to the y-axis,
    /// degrees.
    pub lov: f64,
    /// Latitude of true scale (`LaD`) — the parallel at which `dx_metres` /
    /// `dy_metres` are specified, degrees. GRIB1 fixes this at ±60°; GRIB2
    /// §3.20 carries it explicitly, so grids whose true scale is at the pole
    /// (90°) or another parallel scale correctly.
    pub lad: f64,
    /// Grid spacing in metres along x at the latitude of true scale.
    pub dx_metres: f64,
    /// Grid spacing in metres along y at the latitude of true scale.
    pub dy_metres: f64,
    /// `true` ⇒ south-pole projection; `false` ⇒ north-pole. GRIB1 carries
    /// this in the projection-centre flag; GRIB2 in §3.20 octet 17 bit 2.
    pub south_pole: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct PolarStereoConstants {
    /// `2 · R · k₀` where `k₀ = (1 + sin|LaD|)/2` is the pole scale factor for
    /// a projection whose latitude of true scale is `LaD` (Snyder PP-1395,
    /// eq. 21-15). The product is what every forward/inverse formula consumes.
    two_r_k0: f64,
    sign: f64,
}

fn polar_stereo_constants(lad: f64, south_pole: bool) -> PolarStereoConstants {
    // The pole scale factor depends on the magnitude of the latitude of true
    // scale; the hemisphere is handled separately by `sign`.
    let k0 = (1.0 + (lad.abs() * DEG2RAD).sin()) / 2.0;
    PolarStereoConstants {
        two_r_k0: 2.0 * EARTH_RADIUS_M * k0,
        sign: if south_pole { -1.0 } else { 1.0 },
    }
}

/// Forward polar stereographic: `(lat, lon)` in degrees → `(x, y)` in
/// metres, in a coordinate system centred on the projection pole with the
/// y-axis along `lov`.
///
/// Undefined at the *opposite* pole (`tan` → ∞); GRIB grids never reach it,
/// but pathological callers will see `±inf` / `NaN`.
///
/// **Recomputes constants per call.** For warp loops use [`PolarStereoProjector`].
pub fn polar_stereo_forward(p: &PolarStereoParams, lat: f64, lon: f64) -> (f64, f64) {
    polar_stereo_forward_with(
        &polar_stereo_constants(p.lad, p.south_pole),
        p.lov,
        lat,
        lon,
    )
}

fn polar_stereo_forward_with(k: &PolarStereoConstants, lov: f64, lat: f64, lon: f64) -> (f64, f64) {
    let lat_r = lat * DEG2RAD;
    let d_lon = (lon - lov) * DEG2RAD;
    // Snyder 21-33 (north) / 21-34 (south). For south-polar, `sign = -1`
    // flips the latitude argument so the same `tan(π/4 - φ_s/2)` form
    // works after substituting `φ_s = -lat`.
    let rho = k.two_r_k0 * (PI / 4.0 - k.sign * lat_r / 2.0).tan();
    let x = rho * d_lon.sin();
    let y = -k.sign * rho * d_lon.cos();
    (x, y)
}

/// Inverse polar stereographic: `(x, y)` in metres → `(lat, lon)` in
/// degrees. Returns `(NaN, lov)` when `(x, y) == (0, 0)` (the projection
/// pole), where longitude is undefined.
pub fn polar_stereo_inverse_xy(p: &PolarStereoParams, x: f64, y: f64) -> (f64, f64) {
    polar_stereo_inverse_xy_with(&polar_stereo_constants(p.lad, p.south_pole), p.lov, x, y)
}

fn polar_stereo_inverse_xy_with(k: &PolarStereoConstants, lov: f64, x: f64, y: f64) -> (f64, f64) {
    let rho = (x * x + y * y).sqrt();
    if rho == 0.0 {
        // At the pole every meridian converges; longitude is undefined.
        // Return lov as a convention so warp setup that hits this case
        // doesn't NaN-pollute downstream min/max.
        return (k.sign * 90.0, lov);
    }
    let c = 2.0 * (rho / k.two_r_k0).atan();
    let lat = k.sign * (PI / 2.0 - c) * RAD2DEG;
    // Snyder 20-16: λ = λ₀ + atan2(x, -y) for north-polar; flip the y-sign
    // for south-polar (same `sign` flip used in the forward direction).
    let lon = lov + x.atan2(-k.sign * y) * RAD2DEG;
    (lat, lon)
}

/// Inverse warp: `(lat, lon)` → fractional source grid index. **Recomputes
/// constants and the grid origin per call** — for warp loops use
/// [`PolarStereoProjector`].
pub fn polar_stereo_inverse(p: &PolarStereoParams, lat: f64, lon: f64) -> Option<GridIndex> {
    PolarStereoProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a polar stereographic grid. Owns the
/// pole-scale constant and the forward-projected grid origin — both
/// invariant across every output pixel of a warp.
pub struct PolarStereoProjector {
    pub params: PolarStereoParams,
    constants: PolarStereoConstants,
    origin: (f64, f64),
}

impl PolarStereoProjector {
    pub fn new(params: PolarStereoParams) -> Self {
        let constants = polar_stereo_constants(params.lad, params.south_pole);
        let origin =
            polar_stereo_forward_with(&constants, params.lov, params.lat_first, params.lon_first);
        Self {
            params,
            constants,
            origin,
        }
    }

    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        if self.params.ni < 2
            || self.params.nj < 2
            || self.params.dx_metres == 0.0
            || self.params.dy_metres == 0.0
        {
            return None;
        }
        // Reject points on the wrong hemisphere — forward-projecting them
        // would hit the `tan` singularity at the antipodal pole and yield
        // ±inf, which then maps to a bogus grid index after the
        // origin-relative division.
        if self.params.south_pole {
            if lat > 0.0 {
                return None;
            }
        } else if lat < 0.0 {
            return None;
        }
        let (x, y) = polar_stereo_forward_with(&self.constants, self.params.lov, lat, lon);
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let i = (x - self.origin.0) / self.params.dx_metres;
        let j = (y - self.origin.1) / self.params.dy_metres;
        if i < 0.0 || i > self.params.ni as f64 - 1.0 || j < 0.0 || j > self.params.nj as f64 - 1.0
        {
            return None;
        }
        Some(GridIndex { i, j })
    }

    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        polar_stereo_forward_with(&self.constants, self.params.lov, lat, lon)
    }

    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        polar_stereo_inverse_xy_with(&self.constants, self.params.lov, x, y)
    }

    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }

    /// `true` when the projection pole (origin in projected metres) falls
    /// inside the grid extent. Warp setup uses this to detect the case
    /// where every meridian is represented in the grid and the
    /// equirectangular target should span the full 360° of longitude.
    pub fn pole_inside_grid(&self) -> bool {
        let (ox, oy) = self.origin;
        let max_x = ox + (self.params.ni as f64 - 1.0) * self.params.dx_metres;
        let max_y = oy + (self.params.nj as f64 - 1.0) * self.params.dy_metres;
        let (x_min, x_max) = if ox <= max_x {
            (ox, max_x)
        } else {
            (max_x, ox)
        };
        let (y_min, y_max) = if oy <= max_y {
            (oy, max_y)
        } else {
            (max_y, oy)
        };
        x_min <= 0.0 && 0.0 <= x_max && y_min <= 0.0 && 0.0 <= y_max
    }
}

// ---------------------------------------------------------------------------
// Planar grids (Lambert, polar stereographic): shared corner geometry
// ---------------------------------------------------------------------------

/// A projection whose source grid lies on a plane in metres — a fixed origin
/// at the first scanned point and constant `(dx, dy)` spacing. Lambert
/// conformal and polar stereographic both qualify; lat/lon and Gaussian grids
/// are already geographic and don't.
///
/// Implementors supply four cheap accessors; the trait derives the grid
/// corners from them. This is the one geometry shared by every planar warp
/// setup (target-bbox derivation) and by GRIB `bounds()` reporting, which
/// otherwise reimplement `origin + (n-1)·d` per projection.
pub trait PlanarGridProjector {
    /// Grid origin (first scanned point) in projected metres.
    fn grid_origin(&self) -> (f64, f64);
    /// `(ni, nj)` grid dimensions in points.
    fn grid_dims(&self) -> (u32, u32);
    /// `(dx, dy)` spacing in metres at the latitude of true scale.
    fn grid_spacing(&self) -> (f64, f64);
    /// Inverse-project projected metres back to `(lat, lon)` in degrees.
    fn inverse_lonlat(&self, x: f64, y: f64) -> (f64, f64);

    /// The four grid corners in projected metres, ordered: origin, far-x
    /// edge, far-y edge, opposite corner.
    fn grid_corners_xy(&self) -> [(f64, f64); 4] {
        let (ox, oy) = self.grid_origin();
        let (ni, nj) = self.grid_dims();
        let (dx, dy) = self.grid_spacing();
        let ex = (ni as f64 - 1.0) * dx;
        let ey = (nj as f64 - 1.0) * dy;
        [(ox, oy), (ox + ex, oy), (ox, oy + ey), (ox + ex, oy + ey)]
    }

    /// The four grid corners as `(lat, lon)` in degrees. Longitudes are
    /// returned as the inverse produces them (may fall outside [-180, 180]);
    /// callers that need a normalised value should wrap it themselves.
    fn grid_corners_lonlat(&self) -> [(f64, f64); 4] {
        self.grid_corners_xy()
            .map(|(x, y)| self.inverse_lonlat(x, y))
    }

    /// `(lat, lon)` of the last scanned grid point — the corner diagonally
    /// opposite the origin. Same longitude caveat as [`Self::grid_corners_lonlat`].
    fn last_grid_point_lonlat(&self) -> (f64, f64) {
        self.grid_corners_lonlat()[3]
    }

    /// Axis-aligned lat/lon bounding box of the grid, returned as
    /// `(lat_min, lat_max, lon_min, lon_max)`.
    ///
    /// The box is taken over a dense sample of the grid **perimeter**, not
    /// just the four corners. A planar grid edge is a straight line in
    /// projected metres but a *curve* in lat/lon, and its lat/lon extrema
    /// generally fall in the interior of an edge — the classic case is the
    /// point of an edge closest to the projection pole, which maximises
    /// latitude and sits nowhere near a corner. Sampling only the corners
    /// badly underestimates the extent (e.g. the CMC polar grid: corners cap
    /// at 60°N while the top edge reaches ~80.6°N). Interior grid points can't
    /// exceed the perimeter's lat/lon range for a pole-exterior grid, so the
    /// boundary walk is sufficient.
    ///
    /// The longitude extent is the **minimum enclosing arc** of the perimeter
    /// samples, found as the complement of the largest empty gap between
    /// adjacent (sorted, wrapped) sample longitudes. This yields a tight,
    /// continuous span for a grid straddling the ±180° antimeridian and, unlike
    /// a single-reference unwrap, stays correct for grids whose azimuthal
    /// extent exceeds 180° (e.g. a wide Lambert tile). The result is recentered
    /// so its midpoint lies in [-180, 180]; `lon_min` may still be `< -180` (or
    /// `lon_max > 180`) to describe a dateline-spanning window — intentional,
    /// since the warp consumes it through periodic trig.
    ///
    /// A grid that fully *surrounds* the projection pole has no empty gap, so
    /// this arc degenerates; detect that with
    /// [`PolarStereoProjector::pole_inside_grid`] and override to the full 360°.
    fn lonlat_bbox(&self) -> (f64, f64, f64, f64) {
        // Subdivisions per edge. 512 puts samples ~16 km apart on an 8000 km
        // edge — fine enough to pin the closest-to-pole latitude to ~0.03°
        // while staying a trivial ~2k inverse projections regardless of grid
        // size.
        const PER_EDGE: u32 = 512;

        let (ox, oy) = self.grid_origin();
        let (ni, nj) = self.grid_dims();
        let (dx, dy) = self.grid_spacing();
        let ex = (ni as f64 - 1.0) * dx;
        let ey = (nj as f64 - 1.0) * dy;

        let mut lat_min = f64::INFINITY;
        let mut lat_max = f64::NEG_INFINITY;
        let mut lons: Vec<f64> = Vec::with_capacity(4 * (PER_EDGE as usize + 1));
        let mut visit = |x: f64, y: f64| {
            let (lat, lon) = self.inverse_lonlat(x, y);
            lat_min = lat_min.min(lat);
            lat_max = lat_max.max(lat);
            lons.push(lon.rem_euclid(360.0));
        };
        for k in 0..=PER_EDGE {
            let t = k as f64 / PER_EDGE as f64;
            visit(ox + t * ex, oy); // bottom edge (j = 0)
            visit(ox + t * ex, oy + ey); // top edge (j = nj-1)
            visit(ox, oy + t * ey); // left edge (i = 0)
            visit(ox + ex, oy + t * ey); // right edge (i = ni-1)
        }

        // Largest empty gap between adjacent wrapped longitudes (including the
        // wrap-around gap from the last sample back to the first). The enclosing
        // arc is everything *except* that gap.
        lons.sort_by(|a, b| a.partial_cmp(b).expect("longitudes are finite"));
        let n = lons.len();
        let mut gap_start = 0usize; // index just after the largest gap
        let mut max_gap = lons[0] + 360.0 - lons[n - 1]; // wrap-around gap
        for i in 1..n {
            let gap = lons[i] - lons[i - 1];
            if gap > max_gap {
                max_gap = gap;
                gap_start = i;
            }
        }
        // The arc runs from the sample after the gap to the one before it,
        // adding a turn when the arc crosses 360° (interior gap).
        let lon_min = lons[gap_start];
        let lon_max = if gap_start == 0 {
            lons[n - 1]
        } else {
            lons[gap_start - 1] + 360.0
        };

        // Recenter on [-180, 180] by shifting a whole number of turns so the
        // midpoint is in range — preserves the (possibly antimeridian-spanning)
        // span while keeping the reported bounds human-sensible.
        let mid = (lon_min + lon_max) / 2.0;
        let shift = ((mid + 180.0).rem_euclid(360.0) - 180.0) - mid;
        (lat_min, lat_max, lon_min + shift, lon_max + shift)
    }
}

impl PlanarGridProjector for LambertProjector {
    fn grid_origin(&self) -> (f64, f64) {
        self.origin
    }
    fn grid_dims(&self) -> (u32, u32) {
        (self.params.ni, self.params.nj)
    }
    fn grid_spacing(&self) -> (f64, f64) {
        (self.params.dx_metres, self.params.dy_metres)
    }
    fn inverse_lonlat(&self, x: f64, y: f64) -> (f64, f64) {
        self.inverse_xy(x, y)
    }
}

impl PlanarGridProjector for PolarStereoProjector {
    fn grid_origin(&self) -> (f64, f64) {
        self.origin
    }
    fn grid_dims(&self) -> (u32, u32) {
        (self.params.ni, self.params.nj)
    }
    fn grid_spacing(&self) -> (f64, f64) {
        (self.params.dx_metres, self.params.dy_metres)
    }
    fn inverse_lonlat(&self, x: f64, y: f64) -> (f64, f64) {
        self.inverse_xy(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(actual: f64, expected: f64, tol: f64) -> bool {
        (actual - expected).abs() < tol
    }

    // -----------------------------------------------------------------
    // Regular lat/lon
    // -----------------------------------------------------------------

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

    // -----------------------------------------------------------------
    // Mercator
    // -----------------------------------------------------------------

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

    // -----------------------------------------------------------------
    // Gaussian latitudes
    // -----------------------------------------------------------------

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

    // -----------------------------------------------------------------
    // Lambert Conformal
    // -----------------------------------------------------------------

    fn lambert_params() -> LambertParams {
        LambertParams {
            ni: 93,
            nj: 65,
            lat_first: 12.19,
            lon_first: -133.459,
            lad: 25.0,
            lov: -95.0,
            dx_metres: 81_271.0,
            dy_metres: 81_271.0,
            latin1: 25.0,
            latin2: 25.0,
        }
    }

    #[test]
    fn lambert_forward_inverse_round_trip() {
        let p = lambert_params();
        let (x, y) = lambert_forward(&p, 40.0, -100.0);
        let (lat, lon) = lambert_inverse_xy(&p, x, y);
        assert!(near(lat, 40.0, 1e-6));
        assert!(near(lon, -100.0, 1e-6));
    }

    #[test]
    fn lambert_handles_0_360_lov_convention() {
        // Eta-style grid: LoV + Lo1 carried in [0, 360) (265°E / 226.541°E), as
        // NCEP files store them, rather than the ±180 form `lambert_params`
        // uses. Regression for the cone-angle wrap bug that rendered such grids
        // blank under equirectangular reprojection.
        let p = LambertParams {
            lov: 265.0,
            lon_first: 226.541,
            ..lambert_params()
        };
        // The forward map must be invariant to a 360° shift in the query
        // longitude (the fix wraps `lon − lov` before scaling by the cone
        // constant; without it the two differ by n·360°).
        let f_pm180 = lambert_forward(&p, 40.0, -95.0);
        let f_0_360 = lambert_forward(&p, 40.0, 265.0);
        assert!(
            near(f_pm180.0, f_0_360.0, 1e-6),
            "x invariant to +360 shift"
        );
        assert!(
            near(f_pm180.1, f_0_360.1, 1e-6),
            "y invariant to +360 shift"
        );
        // And a ±180 query longitude (what the equirectangular target feeds in)
        // resolves to an in-grid index instead of falling off the grid.
        let idx = lambert_inverse(&p, 40.0, -95.0).expect("on-grid point on the LoV meridian");
        assert!(idx.i >= 0.0 && idx.i <= (p.ni as f64 - 1.0));
        assert!(idx.j >= 0.0 && idx.j <= (p.nj as f64 - 1.0));
    }

    #[test]
    fn lambert_inverse_maps_first_corner_to_zero() {
        let p = lambert_params();
        let idx = lambert_inverse(&p, p.lat_first, p.lon_first).expect("corner");
        assert!(near(idx.i, 0.0, 1e-6));
        assert!(near(idx.j, 0.0, 1e-6));
    }

    #[test]
    fn lambert_inverse_rejects_off_grid_points() {
        let p = lambert_params();
        assert!(lambert_inverse(&p, 70.0, -100.0).is_none(), "north");
        assert!(lambert_inverse(&p, 0.0, 0.0).is_none(), "southeast");
    }

    #[test]
    fn lambert_inverse_rejects_nonfinite_and_degenerate_dims() {
        let p = lambert_params();
        assert!(lambert_inverse(&p, f64::NAN, -100.0).is_none(), "NaN lat");
        assert!(
            lambert_inverse(&p, 40.0, f64::INFINITY).is_none(),
            "inf lon"
        );
        let degenerate = LambertParams { ni: 1, ..p };
        assert!(
            lambert_inverse(&degenerate, 40.0, -100.0).is_none(),
            "ni < 2"
        );
        let zero_dx = LambertParams {
            dx_metres: 0.0,
            ..p
        };
        assert!(
            lambert_inverse(&zero_dx, 40.0, -100.0).is_none(),
            "dx_metres = 0 must not divide"
        );
    }

    #[test]
    fn latlon_inverse_rejects_nonfinite_and_degenerate_dims() {
        let p = latlon_params();
        assert!(latlon_inverse(&p, f64::NAN, 120.0).is_none());
        assert!(latlon_inverse(&p, 30.0, f64::INFINITY).is_none());
        let degenerate = LatLonParams { nj: 1, ..p };
        assert!(latlon_inverse(&degenerate, 30.0, 120.0).is_none());
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

    // -----------------------------------------------------------------
    // Polar Stereographic
    // -----------------------------------------------------------------

    /// CMC regional model grid (135×95, 60 km at 60°N, lon_first ≈ −110°,
    /// lov = 247°). Matches the `cmc_wind_300_2010052400_p012.grib`
    /// fixture used by the GRIB1 integration tests.
    fn cmc_polar_params() -> PolarStereoParams {
        PolarStereoParams {
            ni: 135,
            nj: 95,
            lat_first: 11.43,
            lon_first: -110.27,
            lov: 247.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        }
    }

    #[test]
    fn polar_stereo_forward_inverse_round_trip_north() {
        let p = cmc_polar_params();
        for (lat, lon) in [(45.0, -90.0), (60.0, 0.0), (80.0, 100.0)] {
            let (x, y) = polar_stereo_forward(&p, lat, lon);
            let (lat_back, lon_back) = polar_stereo_inverse_xy(&p, x, y);
            assert!(near(lat_back, lat, 1e-7), "lat {lat} → {lat_back}");
            // Normalise to [-180, 180] before comparing — atan2 returns
            // (-π, π] and the test inputs are in that range too.
            let lon_back = ((lon_back + 180.0).rem_euclid(360.0)) - 180.0;
            let lon_norm = ((lon + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(near(lon_back, lon_norm, 1e-7), "lon {lon} → {lon_back}");
        }
    }

    #[test]
    fn polar_stereo_forward_inverse_round_trip_south() {
        let p = PolarStereoParams {
            south_pole: true,
            lat_first: -11.43,
            ..cmc_polar_params()
        };
        for (lat, lon) in [(-45.0, -90.0), (-60.0, 0.0), (-80.0, 100.0)] {
            let (x, y) = polar_stereo_forward(&p, lat, lon);
            let (lat_back, lon_back) = polar_stereo_inverse_xy(&p, x, y);
            assert!(near(lat_back, lat, 1e-7), "lat {lat} → {lat_back}");
            let lon_back = ((lon_back + 180.0).rem_euclid(360.0)) - 180.0;
            let lon_norm = ((lon + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(near(lon_back, lon_norm, 1e-7), "lon {lon} → {lon_back}");
        }
    }

    #[test]
    fn polar_stereo_inverse_honours_north_to_south_scan() {
        // A north-polar grid scanning north→south (jScansPositively = 0): row 0
        // is the northernmost row, successive rows step south. The napi builder
        // encodes that as a *negative* dy; the projector's j must then advance
        // southward. (See `signed_polar_increments` in the napi crate.)
        let base = PolarStereoParams {
            ni: 10,
            nj: 10,
            lat_first: 80.0,
            lon_first: 0.0,
            lov: 0.0,
            lad: 60.0,
            dx_metres: 50_000.0,
            dy_metres: -50_000.0, // north→south scan
            south_pole: false,
        };
        let proj = PolarStereoProjector::new(base);
        // The first scanned point is the projection origin → index (0, 0).
        let origin = proj.inverse(80.0, 0.0).expect("origin resolves");
        assert!(
            origin.i.abs() < 1e-6 && origin.j.abs() < 1e-6,
            "origin {origin:?}"
        );
        // A point ~2° south of the first row lies several rows *into* the grid.
        let south = proj.inverse(78.0, 0.0).expect("southward point resolves");
        assert!(
            south.j > 0.0,
            "north→south scan must increase j going south, got j={}",
            south.j
        );

        // Regression guard: the pre-fix code fed the unsigned magnitude
        // (positive dy), which maps the same southward point to negative j and
        // drops it from the grid entirely.
        let unsigned = PolarStereoParams {
            dy_metres: 50_000.0,
            ..base
        };
        assert!(
            PolarStereoProjector::new(unsigned)
                .inverse(78.0, 0.0)
                .is_none(),
            "positive (unsigned) dy mis-maps the southward point to negative j"
        );
    }

    #[test]
    fn lambert_inverse_honours_north_to_south_scan() {
        // A Lambert grid scanning north→south (jScansPositively = 0): row 0 is
        // the northernmost row. The napi builder encodes that as a negative dy,
        // and the projector's j must advance southward — identical mechanism to
        // the polar-stereo case, since both map `j = (y - origin_y) / dy` in the
        // LoV plane.
        let base = LambertParams {
            ni: 50,
            nj: 50,
            lat_first: 50.0,
            lon_first: -100.0,
            lad: 40.0,
            lov: -100.0,
            dx_metres: 20_000.0,
            dy_metres: -20_000.0, // north→south scan
            latin1: 40.0,
            latin2: 40.0,
        };
        let proj = LambertProjector::new(base);
        // First scanned point (on the central meridian) → index (0, 0).
        let origin = proj.inverse(50.0, -100.0).expect("origin resolves");
        assert!(
            origin.i.abs() < 1e-6 && origin.j.abs() < 1e-6,
            "origin {origin:?}"
        );
        // A point 5° south of the first row lies several rows into the grid.
        let south = proj
            .inverse(45.0, -100.0)
            .expect("southward point resolves");
        assert!(
            south.j > 0.0,
            "north→south scan must increase j going south, got j={}",
            south.j
        );

        // Regression guard: the unsigned magnitude (positive dy) drops the
        // southward point to negative j and rejects it.
        let unsigned = LambertParams {
            dy_metres: 20_000.0,
            ..base
        };
        assert!(
            LambertProjector::new(unsigned)
                .inverse(45.0, -100.0)
                .is_none(),
            "positive (unsigned) dy mis-maps the southward point to negative j"
        );
    }

    #[test]
    fn polar_stereo_north_pole_projects_to_origin() {
        let p = cmc_polar_params();
        let (x, y) = polar_stereo_forward(&p, 90.0, 0.0);
        assert!(near(x, 0.0, 1e-6));
        assert!(near(y, 0.0, 1e-6));
    }

    /// GRIB2 §3.20 carries a variable latitude of true scale (`LaD`); the
    /// pole scale factor `k₀ = (1 + sin|LaD|)/2` must follow it. A grid with
    /// true scale at the pole (LaD = 90°, k₀ = 1) projects to a radius
    /// `1/k₀(60°) = 1.07180…×` larger than the same point under the GRIB1
    /// fixed-60° convention (Snyder PP-1395, eq. 21-15).
    #[test]
    fn polar_stereo_lad_drives_pole_scale_factor() {
        let at_60 = cmc_polar_params(); // lad = 60.0
        let at_90 = PolarStereoParams {
            lad: 90.0,
            ..cmc_polar_params()
        };
        let (x60, y60) = polar_stereo_forward(&at_60, 45.0, 247.0);
        let (x90, y90) = polar_stereo_forward(&at_90, 45.0, 247.0);
        let rho60 = (x60 * x60 + y60 * y60).sqrt();
        let rho90 = (x90 * x90 + y90 * y90).sqrt();
        let k0_60 = (1.0 + (60.0_f64 * DEG2RAD).sin()) / 2.0;
        assert!(
            near(rho90 / rho60, 1.0 / k0_60, 1e-9),
            "LaD=90 vs 60 ratio {} ≠ {}",
            rho90 / rho60,
            1.0 / k0_60
        );
        // Sanity: the two are genuinely different (regression guard against a
        // hardcoded constant silently ignoring LaD).
        assert!((rho90 - rho60).abs() > 1.0, "LaD ignored — radii identical");
    }

    #[test]
    fn polar_stereo_south_pole_projects_to_origin() {
        let p = PolarStereoParams {
            south_pole: true,
            ..cmc_polar_params()
        };
        let (x, y) = polar_stereo_forward(&p, -90.0, 0.0);
        assert!(near(x, 0.0, 1e-6));
        assert!(near(y, 0.0, 1e-6));
    }

    #[test]
    fn polar_stereo_inverse_maps_first_corner_to_zero() {
        let p = cmc_polar_params();
        let idx = polar_stereo_inverse(&p, p.lat_first, p.lon_first).expect("corner");
        assert!(near(idx.i, 0.0, 1e-6));
        assert!(near(idx.j, 0.0, 1e-6));
    }

    #[test]
    fn polar_stereo_inverse_rejects_wrong_hemisphere() {
        let p = cmc_polar_params();
        assert!(
            polar_stereo_inverse(&p, -45.0, 0.0).is_none(),
            "north grid + south lat"
        );
        let south = PolarStereoParams {
            south_pole: true,
            lat_first: -11.43,
            ..p
        };
        assert!(
            polar_stereo_inverse(&south, 45.0, 0.0).is_none(),
            "south grid + north lat"
        );
    }

    #[test]
    fn polar_stereo_inverse_rejects_off_grid_points() {
        let p = cmc_polar_params();
        // A point in Antarctica is on the wrong hemisphere for a north-polar
        // grid; a tropical point near the equator is on the right hemisphere
        // but well outside the 135×95 box around the pole.
        assert!(polar_stereo_inverse(&p, 5.0, 0.0).is_none());
    }

    #[test]
    fn polar_stereo_inverse_rejects_nonfinite_and_degenerate_dims() {
        let p = cmc_polar_params();
        assert!(polar_stereo_inverse(&p, f64::NAN, 0.0).is_none());
        assert!(polar_stereo_inverse(&p, 60.0, f64::INFINITY).is_none());
        let degenerate = PolarStereoParams { ni: 1, ..p };
        assert!(polar_stereo_inverse(&degenerate, 60.0, 0.0).is_none());
        let zero_dx = PolarStereoParams {
            dx_metres: 0.0,
            ..p
        };
        assert!(polar_stereo_inverse(&zero_dx, 60.0, 0.0).is_none());
    }

    #[test]
    fn polar_stereo_pole_inside_grid_detection() {
        // CMC is a regional tile NE of the pole, not a hemispheric grid —
        // its projected box doesn't actually contain (0,0).
        let cmc = PolarStereoProjector::new(cmc_polar_params());
        assert!(
            !cmc.pole_inside_grid(),
            "CMC regional tile excludes the pole"
        );

        // A synthetic hemispheric grid whose NW corner sits at d_lon = -135°
        // from `lov`, at a southern-enough latitude that the projected origin
        // lands at roughly (-3e6, +3e6) metres. Scanning east + south at 2 Mm
        // step over 4×4 cells crosses the pole at (0, 0).
        let hemispheric = PolarStereoParams {
            ni: 4,
            nj: 4,
            lat_first: 50.8,
            lon_first: -135.0,
            lov: 0.0,
            lad: 60.0,
            dx_metres: 2_000_000.0,
            dy_metres: -2_000_000.0,
            south_pole: false,
        };
        let projector = PolarStereoProjector::new(hemispheric);
        assert!(
            projector.pole_inside_grid(),
            "hemispheric grid origin {:?} should bracket the pole",
            projector.origin()
        );
    }

    #[test]
    fn polar_stereo_inverse_xy_origin_returns_pole_with_lov() {
        let p = cmc_polar_params();
        let (lat, lon) = polar_stereo_inverse_xy(&p, 0.0, 0.0);
        assert!(near(lat, 90.0, 1e-9));
        assert!(near(lon, p.lov, 1e-9));
    }

    #[test]
    fn lambert_tangent_cone_at_origin() {
        let p = LambertParams {
            latin1: 40.0,
            latin2: 40.0,
            lad: 40.0,
            ..lambert_params()
        };
        let (x, y) = lambert_forward(&p, 40.0, -95.0);
        // At the projection origin (lad, lov), x and y should be ~0 in
        // the bare projection (no false-easting / false-northing).
        assert!(near(x, 0.0, 1.0));
        assert!(near(y, 0.0, 1.0));
    }

    #[test]
    fn lonlat_bbox_unwraps_antimeridian_crossing_grid() {
        // The real CMC fixture (lov=249) has its `+y` corner at +177.2° while
        // the other three are negative — the grid straddles the dateline.
        // Naive min/max would give a ~312°-wide box; unwrapping must yield a
        // tight, continuous span instead.
        let proj = PolarStereoProjector::new(PolarStereoParams {
            ni: 135,
            nj: 95,
            lat_first: 27.203,
            lon_first: -135.213,
            lov: 249.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        });
        let (lat_min, lat_max, lon_min, lon_max) = proj.lonlat_bbox();
        assert!(near(lat_min, 19.945, 1e-2), "lat_min {lat_min}");
        // The top edge bows toward the pole and reaches ~80.6°N — far above
        // the highest corner (60.5°N). Perimeter sampling must catch this.
        assert!(near(lat_max, 80.593, 5e-2), "lat_max {lat_max}");
        // +177.2° unwraps to ≈ -182.8°, giving a continuous ~151° span rather
        // than the spurious 312° box.
        assert!(near(lon_min, -182.805, 1e-2), "lon_min {lon_min}");
        assert!(near(lon_max, -31.933, 1e-2), "lon_max {lon_max}");
        assert!(lon_max - lon_min < 180.0, "span should be tight");
    }

    #[test]
    fn lonlat_bbox_lat_max_comes_from_edge_not_corner() {
        // Regression guard for the four-corner underestimate: the CMC grid's
        // corners top out at 60.5°N, but the boundary reaches ~80.6°N. A
        // corner-only box would report the former.
        let proj = PolarStereoProjector::new(PolarStereoParams {
            ni: 135,
            nj: 95,
            lat_first: 27.203,
            lon_first: -135.213,
            lov: 249.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        });
        let corner_lat_max = proj
            .grid_corners_lonlat()
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let (_, lat_max, ..) = proj.lonlat_bbox();
        assert!(
            near(corner_lat_max, 60.476, 1e-2),
            "corner cap {corner_lat_max}"
        );
        assert!(
            lat_max > corner_lat_max + 15.0,
            "perimeter lat_max ({lat_max}) must clear the corner cap ({corner_lat_max})"
        );
    }

    #[test]
    fn lonlat_bbox_non_crossing_grid_encloses_corners() {
        // CONUS Lambert grid: all corners well clear of the dateline, so the
        // longitude unwrap is a no-op. The box must enclose every corner — and,
        // because edges bow, may extend beyond them in latitude (this grid's
        // boundary reaches ~83°N, above any corner).
        let proj = LambertProjector::new(LambertParams {
            ni: 601,
            nj: 401,
            lat_first: 38.5,
            lon_first: -126.0,
            lad: 38.5,
            lov: -95.0,
            dx_metres: 13_545.0,
            dy_metres: 13_545.0,
            latin1: 38.5,
            latin2: 38.5,
        });
        let corners = proj.grid_corners_lonlat();
        let (lat_min, lat_max, lon_min, lon_max) = proj.lonlat_bbox();
        for (lat, lon) in corners {
            assert!(
                lat_min - 1e-6 <= lat && lat <= lat_max + 1e-6,
                "lat {lat} outside box"
            );
            assert!(
                lon_min - 1e-6 <= lon && lon <= lon_max + 1e-6,
                "lon {lon} outside box"
            );
        }
        // Edge bow lifts lat_max above the top corners.
        let corner_lat_max = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(lat_max > corner_lat_max, "edge should bow above corner lat");
    }

    #[test]
    fn lonlat_bbox_resolves_spans_wider_than_180_degrees() {
        // A synthetic projector whose perimeter sweeps 270° of longitude at a
        // constant latitude — wider than a single-reference unwrap can resolve.
        // The old code mis-bounded this (reporting a near-360° span); the
        // minimum-enclosing-arc must return the true ~270° window.
        struct WideMock;
        impl PlanarGridProjector for WideMock {
            fn grid_origin(&self) -> (f64, f64) {
                (0.0, 0.0)
            }
            fn grid_dims(&self) -> (u32, u32) {
                (271, 1)
            }
            fn grid_spacing(&self) -> (f64, f64) {
                (1.0, 1.0)
            }
            // Treat the plane x-coordinate directly as longitude (0..=270).
            fn inverse_lonlat(&self, x: f64, _y: f64) -> (f64, f64) {
                (12.0, x)
            }
        }

        let (lat_min, lat_max, lon_min, lon_max) = WideMock.lonlat_bbox();
        assert!((lat_min - 12.0).abs() < 1e-9 && (lat_max - 12.0).abs() < 1e-9);
        let span = lon_max - lon_min;
        assert!(
            (span - 270.0).abs() < 1.0,
            "expected a tight ~270° span, got {span} ([{lon_min}, {lon_max}])"
        );
    }
}
