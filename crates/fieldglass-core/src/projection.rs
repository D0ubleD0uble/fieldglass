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
/// Default spherical Earth radius: WMO GRIB2 code table 3.2 shape 6, the value
/// most operational GRIB2 producers declare. A message that declares its own
/// earth shape should pass that instead — the projections are sensitive to it.
/// Being off by one part in 1700 (GRIB1's 6 367 470 m) misplaces the far corner
/// of a continental grid by several kilometres.
pub const DEFAULT_EARTH_RADIUS_M: f64 = 6_371_229.0;

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
fn eastward_rel_lon(lon_first: f64, lon_last: f64, ni: u32, lon: f64) -> Option<(f64, f64)> {
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

// ---------------------------------------------------------------------------
// Lambert Conformal Conic (GRIB1 grid_type 3, GRIB2 template 3.30)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LambertParams {
    /// Radius of the spherical Earth the grid is projected on, in metres. The
    /// message declares it (GRIB1's earth-shape flag, GRIB2's
    /// `shapeOfTheEarth`); [`DEFAULT_EARTH_RADIUS_M`] is the fallback.
    pub earth_radius_m: f64,
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

impl LambertConstants {
    /// Whether these constants describe a usable cone. Degenerate standard
    /// parallels make the cone constant `n` zero or non-finite — both tangent
    /// parallels on the equator (`latin1 == latin2 == 0`, so `n = sin 0 = 0`)
    /// or a parallel at a pole (`cos → 0`, so `F = cos·tanⁿ / n` blows up). The
    /// `/ n` in `f_const` would then divide by zero, yielding `inf`/`NaN` that
    /// silently render blank; callers should reject the grid instead.
    fn well_defined(&self) -> bool {
        self.n.is_finite() && self.n != 0.0 && self.f_const.is_finite() && self.rho0.is_finite()
    }
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
    // `f_const`/`rho0` are non-finite for degenerate parallels (n == 0, or a
    // pole-tangent cone). Rather than clamp here — which would invent a cone
    // the grid never described — we let the values stay non-finite and gate on
    // `LambertConstants::well_defined` at the projection boundary.
    let f_const = lat1.cos() * tan1.powf(n) / n;
    let rho0 = p.earth_radius_m * f_const / (PI / 4.0 + lad / 2.0).tan().powf(n);
    LambertConstants {
        n,
        f_const,
        rho0,
        earth_r: p.earth_radius_m,
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
        if !self.constants.well_defined() {
            // Degenerate standard parallels (see `LambertConstants::well_defined`).
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
        let (i_max, j_max) = (self.params.ni as f64 - 1.0, self.params.nj as f64 - 1.0);
        // The projection arithmetic carries ~1e-13 of a cell in round-off, enough
        // to push a point sitting exactly *on* a grid edge a hair outside the
        // bounds below and spuriously reject it — dropping the outermost row or
        // column to background. Snap an index within EDGE_EPS of an edge back
        // onto it, exactly as the rotated lat/lon inverse already does for the
        // same reason. EDGE_EPS is a nanometre of a grid cell: far above the
        // round-off, far below any real offset.
        const EDGE_EPS: f64 = 1e-9;
        let i = snap_to_range(
            (x - self.origin.0) / self.params.dx_metres,
            0.0,
            i_max,
            EDGE_EPS,
        );
        let j = snap_to_range(
            (y - self.origin.1) / self.params.dy_metres,
            0.0,
            j_max,
            EDGE_EPS,
        );
        if i < 0.0 || i > i_max || j < 0.0 || j > j_max {
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

    /// Whether the cone constants are usable. `false` for degenerate standard
    /// parallels (see [`LambertConstants::well_defined`]); such a projector's
    /// [`inverse`](Self::inverse) always returns `None`, so callers can surface
    /// "not reprojectable" instead of rendering blank.
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined()
    }
}

// ---------------------------------------------------------------------------
// Polar Stereographic (GRIB1 grid_type 5, GRIB2 template 3.20)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarStereoParams {
    /// Radius of the spherical Earth the grid is projected on, in metres. See
    /// [`LambertParams::earth_radius_m`].
    pub earth_radius_m: f64,
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

fn polar_stereo_constants(lad: f64, south_pole: bool, earth_radius_m: f64) -> PolarStereoConstants {
    // The pole scale factor depends on the magnitude of the latitude of true
    // scale; the hemisphere is handled separately by `sign`.
    let k0 = (1.0 + (lad.abs() * DEG2RAD).sin()) / 2.0;
    PolarStereoConstants {
        two_r_k0: 2.0 * earth_radius_m * k0,
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
        &polar_stereo_constants(p.lad, p.south_pole, p.earth_radius_m),
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
    polar_stereo_inverse_xy_with(
        &polar_stereo_constants(p.lad, p.south_pole, p.earth_radius_m),
        p.lov,
        x,
        y,
    )
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
        let constants =
            polar_stereo_constants(params.lad, params.south_pole, params.earth_radius_m);
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
        let (i_max, j_max) = (self.params.ni as f64 - 1.0, self.params.nj as f64 - 1.0);
        // The projection arithmetic carries ~1e-13 of a cell in round-off, enough
        // to push a point sitting exactly *on* a grid edge a hair outside the
        // bounds below and spuriously reject it — dropping the outermost row or
        // column to background. Snap an index within EDGE_EPS of an edge back
        // onto it, exactly as the rotated lat/lon inverse already does for the
        // same reason. EDGE_EPS is a nanometre of a grid cell: far above the
        // round-off, far below any real offset.
        const EDGE_EPS: f64 = 1e-9;
        let i = snap_to_range(
            (x - self.origin.0) / self.params.dx_metres,
            0.0,
            i_max,
            EDGE_EPS,
        );
        let j = snap_to_range(
            (y - self.origin.1) / self.params.dy_metres,
            0.0,
            j_max,
            EDGE_EPS,
        );
        if i < 0.0 || i > i_max || j < 0.0 || j > j_max {
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

    /// `(lat, lon)` of grid point `(i, j)`: step out from the origin in
    /// projected metres and invert. The forward geolocation every planar grid
    /// (Lambert, polar stereographic) shares — the same `origin + i·d` walk
    /// [`Self::lonlat_bbox`] already does along the perimeter, opened up to the
    /// grid interior.
    fn grid_point_lonlat(&self, i: u32, j: u32) -> (f64, f64) {
        let (ox, oy) = self.grid_origin();
        let (dx, dy) = self.grid_spacing();
        self.inverse_lonlat(ox + i as f64 * dx, oy + j as f64 * dy)
    }

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

        // The longitude extent is the minimum enclosing arc of the perimeter
        // samples; see [`enclosing_lon_arc`].
        let (lon_min, lon_max) = enclosing_lon_arc(&mut lons);
        (lat_min, lat_max, lon_min, lon_max)
    }
}

/// Tightest longitude span (degrees) enclosing a set of perimeter-sample
/// longitudes, each already wrapped into `[0, 360)`. Returns
/// `(lon_min, lon_max)` recentred so the midpoint lies in `[-180, 180]`.
///
/// The span is the complement of the largest empty gap between adjacent
/// (sorted, wrapped) samples, so it stays tight and continuous for a grid
/// straddling the ±180° antimeridian and — unlike a single-reference unwrap —
/// for azimuthal extents wider than 180°. `lon_min < -180` (or `lon_max > 180`)
/// intentionally describes a dateline-spanning window; the warp consumes it
/// through periodic trig.
///
/// A sample set that *surrounds* a projection pole has no empty gap, so this
/// arc degenerates toward 360°; callers that can enclose a pole must detect
/// that case separately (e.g. [`PolarStereoProjector::pole_inside_grid`]).
///
/// `total_cmp`: callers feed finite longitudes, but a total order degrades
/// gracefully instead of panicking if a stray NaN ever slips through.
fn enclosing_lon_arc(lons: &mut [f64]) -> (f64, f64) {
    lons.sort_by(|a, b| a.total_cmp(b));
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
    // The arc runs from the sample after the gap to the one before it, adding a
    // turn when the arc crosses 360° (interior gap).
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
    (lon_min + shift, lon_max + shift)
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

// ---------------------------------------------------------------------------
// Rotated latitude/longitude (GRIB1 grid_type 10, GRIB2 template 3.1)
// ---------------------------------------------------------------------------

/// A regular lat/lon grid laid out on a *rotated* sphere: the geographic south
/// pole is moved to `(south_pole_lat, south_pole_lon)` and the sphere spun by
/// `angle_of_rotation` about the new polar axis. COSMO, DWD ICON-EU, and
/// Environment Canada HRDPS/RDPS publish their limited-area grids this way.
///
/// The grid is evenly spaced in the *rotated* coordinates (`lat_first..lat_last`
/// by `lon_first..lon_last`), so the corner fields are rotated-frame degrees,
/// not geographic. Locating a geographic point means rotating it into that
/// frame first, then indexing exactly like [`latlon_inverse`].
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Clamp `v` onto `[min(a,b), max(a,b)]` only when it sits within `eps` just
/// outside the range; otherwise return it unchanged. Used to absorb rotation
/// round-off at a grid edge without masking a genuinely out-of-range value.
fn snap_to_range(v: f64, a: f64, b: f64, eps: f64) -> f64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if v < lo && v >= lo - eps {
        lo
    } else if v > hi && v <= hi + eps {
        hi
    } else {
        v
    }
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
    /// [`PlanarGridProjector::lonlat_bbox`].
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

// ---------------------------------------------------------------------------
// Geostationary / space-view perspective (GRIB2 template 3.90; CF
// `grid_mapping_name = "geostationary"`)
// ---------------------------------------------------------------------------

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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeostationaryParams {
    pub ni: u32,
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

/// Inverse warp: `(lat, lon)` → fractional source grid index. **Recomputes
/// constants per call** — for warp loops use [`GeostationaryProjector`].
pub fn geostationary_inverse(p: &GeostationaryParams, lat: f64, lon: f64) -> Option<GridIndex> {
    GeostationaryProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a geostationary grid. Owns the ellipsoid /
/// sub-satellite constants, invariant across every output pixel of a warp.
pub struct GeostationaryProjector {
    pub params: GeostationaryParams,
    constants: GeostationaryConstants,
}

impl GeostationaryProjector {
    pub fn new(params: GeostationaryParams) -> Self {
        let constants = geostationary_constants(&params);
        Self { params, constants }
    }

    /// Geodetic `(lat, lon)` in degrees → scan angles `(x, y)` in radians, or
    /// `None` off the visible disk.
    pub fn scan_angles(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        geostationary_scan_angles(&self.params, &self.constants, lat, lon)
    }

    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        let p = &self.params;
        if p.ni < 2 || p.nj < 2 || p.dx_rad == 0.0 || p.dy_rad == 0.0 {
            return None;
        }
        let (x, y) = self.scan_angles(lat, lon)?;
        let i = (x - p.x0) / p.dx_rad;
        let j = (y - p.y0) / p.dy_rad;
        if i < 0.0 || i > p.ni as f64 - 1.0 || j < 0.0 || j > p.nj as f64 - 1.0 {
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
    /// ([`enclosing_lon_arc`]) — the same logic the planar projectors use — so
    /// a sector straddling the ±180° antimeridian still frames tightly. Like
    /// [`PlanarGridProjector::lonlat_bbox`], the boundary walk suffices: the
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

// ---------------------------------------------------------------------------
// Forward geolocation: grid index → (lat, lon)
// ---------------------------------------------------------------------------
//
// The rest of this module answers "which grid point holds this lat/lon?" — the
// direction a warp needs, because it walks *output* pixels and samples the
// source. Exporting a field asks the opposite question: "where on Earth is grid
// point (i, j)?". That is what the functions below answer, one per grid type.
//
// Each is the algebraic inverse of the `*_inverse` map above it, and is pinned
// against it by a round-trip test — so the two directions cannot drift apart.
// Longitudes come back as the underlying geometry produces them (they may sit
// outside [-180, 180]); [`normalise_lon`] is there for callers that want the
// conventional range.

/// Wrap a longitude into `[-180, 180)`. The forward maps return longitudes in
/// whatever range the grid's own corners imply (a 0..360 grid keeps 0..360);
/// an exporter that wants the conventional range applies this.
pub fn normalise_lon(lon: f64) -> f64 {
    // `rem_euclid` lands in [0, 360), so the shift lands in [-180, 180): the
    // half-open convention, with +180° folding onto -180°.
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

/// Position along an axis of `n` evenly spaced points running `first` → `last`.
///
/// The endpoints are returned *exactly*, not as `first + (n-1)·step`: the
/// declared corner is the grid's own definition of where its edge is, and
/// walking there in floating point lands an ulp away. That ulp is enough for
/// the `*_inverse` maps' inclusive range checks to reject the point as
/// off-grid, so an exporter would lose the last row of every field.
fn axis_position(first: f64, last: f64, n: u32, k: u32) -> f64 {
    if k == 0 {
        first
    } else if k == n - 1 {
        last
    } else {
        first + (last - first) * (k as f64 / (n as f64 - 1.0))
    }
}

/// `(lat, lon)` of grid point `(i, j)` on a regular lat/lon grid — the inverse
/// of [`latlon_inverse`]. Rows are evenly spaced in latitude, columns in
/// longitude.
///
/// The longitude walks the *eastward* span, so a grid that crosses the
/// antimeridian (`lon_last` numerically below `lon_first`) marches east like
/// the inverse reads it, rather than doubling back west. It is returned in the
/// grid's own frame and may exceed 360°; see [`normalise_lon`].
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

    // -----------------------------------------------------------------
    // Lambert Conformal
    // -----------------------------------------------------------------

    fn lambert_params() -> LambertParams {
        LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            dx_metres: 0.0,
            ..p
        };
        assert!(
            lambert_inverse(&zero_dx, 40.0, -100.0).is_none(),
            "dx_metres = 0 must not divide"
        );
    }

    #[test]
    fn lambert_rejects_degenerate_standard_parallels() {
        // Both standard parallels on the equator: cone constant n = sin 0 = 0,
        // so `F = cos·tanⁿ / n` divides by zero. The constants must report
        // themselves ill-defined and the inverse must return None for every
        // query, rather than emitting an index off a non-finite projection.
        let equator = LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            latin1: 0.0,
            latin2: 0.0,
            ..lambert_params()
        };
        let proj = LambertProjector::new(equator);
        assert!(
            !proj.is_well_defined(),
            "equator-tangent cone is degenerate"
        );
        assert!(proj.inverse(40.0, -100.0).is_none());
        assert!(proj.inverse(equator.lat_first, equator.lon_first).is_none());
        // A healthy cone still reports itself usable.
        assert!(LambertProjector::new(lambert_params()).is_well_defined());
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
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

    // -----------------------------------------------------------------
    // Rotated latitude/longitude (GRIB2 template 3.1)
    // -----------------------------------------------------------------

    /// The committed `rotated_latlon_surface.grib2` fixture: 16×31 grid, rotated
    /// corners (60,0)→(0,30), southern pole at geographic (0,0), no rotation
    /// angle. eccodes 2.34.1 `grib_get_data` reports the corner geographic
    /// coordinates used below as the oracle.
    fn rotated_fixture_params() -> RotatedLatLonParams {
        RotatedLatLonParams {
            ni: 16,
            nj: 31,
            lat_first: 60.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 30.0,
            south_pole_lat: 0.0,
            south_pole_lon: 0.0,
            angle_of_rotation: 0.0,
        }
    }

    // -----------------------------------------------------------------------
    // Forward geolocation (grid index → lat/lon)
    // -----------------------------------------------------------------------
    //
    // The load-bearing property for every grid type: the forward map must be
    // the exact inverse of the `*_inverse` map the warp already uses. Those
    // inverses are the ones validated against eccodes, so round-tripping every
    // grid point through forward → inverse pins the new direction against
    // known-good code rather than against a hand-copied constant.

    /// Assert `forward(i, j) → (lat, lon) → inverse → (i, j)` over every point
    /// of a grid, to within a fraction of a grid cell.
    fn assert_round_trips(
        ni: u32,
        nj: u32,
        forward: impl Fn(u32, u32) -> Option<(f64, f64)>,
        inverse: impl Fn(f64, f64) -> Option<GridIndex>,
        tol: f64,
        what: &str,
    ) {
        for j in 0..nj {
            for i in 0..ni {
                let (lat, lon) = forward(i, j).unwrap_or_else(|| panic!("{what}: no ({i},{j})"));
                let idx = inverse(lat, lon)
                    .unwrap_or_else(|| panic!("{what}: ({i},{j}) → ({lat},{lon}) → off-grid"));
                assert!(
                    near(idx.i, i as f64, tol) && near(idx.j, j as f64, tol),
                    "{what}: ({i},{j}) → ({lat},{lon}) → ({}, {})",
                    idx.i,
                    idx.j
                );
            }
        }
    }

    #[test]
    fn latlon_forward_inverts_the_inverse_map() {
        // A 0.25° global grid scanning north-to-south, the common GFS layout.
        let p = LatLonParams {
            ni: 41,
            nj: 21,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 359.0,
        };
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| latlon_point(&p, i, j),
            |lat, lon| latlon_inverse(&p, lat, lon),
            1e-9,
            "latlon",
        );
        // Anchor: the first point is the grid's own first corner, by definition.
        assert_eq!(latlon_point(&p, 0, 0), Some((90.0, 0.0)));
        // A degenerate grid has no step to walk and must not divide by zero.
        let degenerate = LatLonParams { ni: 1, ..p };
        assert!(latlon_point(&degenerate, 0, 0).is_none());
    }

    #[test]
    fn latlon_forward_handles_an_antimeridian_crossing_grid() {
        // ECMWF open data runs 180° → 359.75° → 0° → 179.75°, so `lon_last` is
        // numerically below `lon_first`. The forward map must walk the *eastward*
        // span (as the inverse does), not the negative difference of the corners
        // — otherwise it would march west and mirror the field.
        let p = LatLonParams {
            ni: 5,
            nj: 3,
            lat_first: 20.0,
            lon_first: 180.0,
            lat_last: -20.0,
            lon_last: 100.0, // 280° of eastward span, wrapping the seam.
        };
        let (_, lon0) = latlon_point(&p, 0, 0).expect("first point");
        let (_, lon_last) = latlon_point(&p, 4, 0).expect("last column");
        assert!(near(lon0, 180.0, 1e-9));
        // Eastward span = 100 - 180 + 360 = 280°, so the last column is at
        // 180 + 280 = 460° ≡ 100°. The raw value keeps the grid's own frame.
        assert!(near(lon_last, 460.0, 1e-9), "lon_last {lon_last}");
        assert!(near(normalise_lon(lon_last), 100.0, 1e-9));
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| latlon_point(&p, i, j),
            |lat, lon| latlon_inverse(&p, lat, lon),
            1e-9,
            "latlon seam",
        );
    }

    #[test]
    fn mercator_forward_inverts_the_inverse_map() {
        // Rows are even in the Mercator ordinate, not in latitude: a linear
        // latitude walk would misplace every interior row, and the round-trip
        // through `mercator_inverse` is what catches that.
        let p = MercatorParams {
            ni: 12,
            nj: 9,
            lat_first: -40.0,
            lon_first: -100.0,
            lat_last: 40.0,
            lon_last: -20.0,
        };
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| mercator_point(&p, i, j),
            |lat, lon| mercator_inverse(&p, lat, lon),
            1e-9,
            "mercator",
        );
        // The interior rows must NOT be evenly spaced in latitude — proof the
        // ordinate is what's being stepped.
        let lat = |j| mercator_point(&p, 0, j).expect("on grid").0;
        let (a, b, c) = (lat(0), lat(1), lat(2));
        assert!(
            ((b - a) - (c - b)).abs() > 1e-3,
            "latitude spacing must not be uniform: {a}, {b}, {c}"
        );
        // The end rows are the declared corners exactly — not an ulp away. The
        // ordinate round-trip is not bit-exact, and that drift is enough for
        // `mercator_inverse`'s inclusive latitude bounds to read the last row as
        // off-grid, which would silently drop it from an export.
        assert_eq!(mercator_point(&p, 0, 0).map(|c| c.0), Some(p.lat_first));
        assert_eq!(
            mercator_point(&p, 0, p.nj - 1).map(|c| c.0),
            Some(p.lat_last)
        );
        // Degenerate dimensions have no step to walk, as in the inverse.
        assert!(mercator_point(&MercatorParams { nj: 1, ..p }, 0, 0).is_none());
    }

    #[test]
    fn gaussian_forward_reads_the_true_row_latitudes() {
        // Gaussian rows sit at the Gauss–Legendre roots, unevenly spaced. The
        // forward map must read them from the cached table, not interpolate.
        let p = GaussianParams {
            ni: 16,
            nj: 8,
            lat_first: 78.0,
            lon_first: 0.0,
            lat_last: -78.0,
            lon_last: 337.5,
            n_parallels: 4,
        };
        let proj = GaussianProjector::new(p);
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| proj.grid_point_lonlat(i, j),
            |lat, lon| proj.inverse(lat, lon),
            1e-9,
            "gaussian",
        );
        // Row latitudes are the Gauss–Legendre roots, north-to-south here.
        let roots = gaussian_latitudes(4);
        for j in 0..p.nj {
            let (lat, _) = proj.grid_point_lonlat(0, j).expect("on grid");
            assert!(
                near(lat, roots[j as usize], 1e-12),
                "row {j}: {lat} vs root {}",
                roots[j as usize]
            );
        }
        // And they are *not* evenly spaced — the whole reason for the table.
        let d0 = roots[1] - roots[0];
        let d3 = roots[4] - roots[3];
        assert!((d0 - d3).abs() > 1e-3, "roots should not be uniform");
    }

    #[test]
    fn gaussian_forward_follows_a_south_to_north_scan() {
        // A south-first grid reverses the row order; the forward map must follow
        // the scan direction rather than always running north-to-south.
        let p = GaussianParams {
            ni: 8,
            nj: 8,
            lat_first: -78.0,
            lon_first: 0.0,
            lat_last: 78.0,
            lon_last: 315.0,
            n_parallels: 4,
        };
        let proj = GaussianProjector::new(p);
        let (first, _) = proj.grid_point_lonlat(0, 0).expect("on grid");
        assert!(first < 0.0, "a south-first scan must start south: {first}");
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| proj.grid_point_lonlat(i, j),
            |lat, lon| proj.inverse(lat, lon),
            1e-9,
            "gaussian s→n",
        );
    }

    #[test]
    fn rotated_latlon_forward_returns_geographic_coordinates() {
        let p = rotated_fixture_params();
        let proj = RotatedLatLonProjector::new(p);
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| rotated_latlon_point(&p, i, j),
            |lat, lon| proj.inverse(lat, lon),
            1e-6,
            "rotated latlon",
        );
        // The eccodes oracle already pinned in `unrotate_matches_eccodes_oracle`:
        // the first grid point, rotated (60, 0), is geographic (30, 180). The
        // forward map must report the *geographic* pair, not the rotated one.
        let (lat, lon) = rotated_latlon_point(&p, 0, 0).expect("first point");
        assert!(
            near(lat, 30.0, 1e-9) && near(lon.abs(), 180.0, 1e-9),
            "({lat},{lon})"
        );
    }

    #[test]
    fn planar_forward_walks_the_grid_from_its_origin() {
        // Lambert (CONUS) and polar stereographic (CMC) share the trait default:
        // origin + (i·dx, j·dy) in projected metres, then invert.
        let lambert = LambertProjector::new(LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 21,
            nj: 15,
            lat_first: 38.5,
            lon_first: -126.0,
            lad: 38.5,
            lov: -95.0,
            dx_metres: 13_545.0,
            dy_metres: 13_545.0,
            latin1: 38.5,
            latin2: 38.5,
        });
        assert_round_trips(
            21,
            15,
            |i, j| Some(lambert.grid_point_lonlat(i, j)),
            |lat, lon| lambert.inverse(lat, lon),
            1e-6,
            "lambert",
        );
        // Grid point (0, 0) is the declared first corner, by construction.
        let (lat, lon) = lambert.grid_point_lonlat(0, 0);
        assert!(
            near(lat, 38.5, 1e-6) && near(lon, -126.0, 1e-6),
            "({lat},{lon})"
        );

        let polar = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 21,
            nj: 17,
            lat_first: 27.203,
            lon_first: -135.213,
            lov: 249.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        });
        assert_round_trips(
            21,
            17,
            |i, j| Some(polar.grid_point_lonlat(i, j)),
            |lat, lon| polar.inverse(lat, lon),
            1e-6,
            "polar stereo",
        );
        let (lat, lon) = polar.grid_point_lonlat(0, 0);
        assert!(
            near(lat, 27.203, 1e-6) && near(normalise_lon(lon), -135.213, 1e-6),
            "({lat},{lon})"
        );
    }

    #[test]
    fn planar_inverse_accepts_a_point_sitting_exactly_on_the_grid_edge() {
        // Regression guard. The projection arithmetic carries ~1e-13 of a cell
        // in round-off, so a coordinate lying exactly *on* the first row came
        // back with j = -6.9e-14 and was rejected as off-grid by the strict
        // `j < 0.0` bound — silently dropping the outermost row/column of every
        // Lambert and polar-stereo field to background. (The rotated lat/lon
        // inverse already snapped for this reason; these two never did.)
        let lambert = LambertProjector::new(LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 21,
            nj: 15,
            lat_first: 38.5,
            lon_first: -126.0,
            lad: 38.5,
            lov: -95.0,
            dx_metres: 13_545.0,
            dy_metres: 13_545.0,
            latin1: 38.5,
            latin2: 38.5,
        });
        // Every point of the first row is on the edge; all must be accepted.
        for i in 0..21 {
            let (lat, lon) = lambert.grid_point_lonlat(i, 0);
            let idx = lambert
                .inverse(lat, lon)
                .unwrap_or_else(|| panic!("edge point ({i},0) → ({lat},{lon}) rejected"));
            assert!(near(idx.j, 0.0, 1e-6), "edge row j = {}", idx.j);
        }
        // A point genuinely outside the grid is still rejected — the snap must
        // not widen the grid, only absorb round-off.
        assert!(
            lambert.inverse(38.5, -126.0 - 5.0).is_none(),
            "a point well west of the grid must stay off-grid"
        );
    }

    #[test]
    fn geostationary_forward_locates_the_disk_and_rejects_space() {
        // A GOES-East-like full disk: the corners of the raster are space, so
        // the forward map must decline to invent a coordinate there, while the
        // centre sits under the satellite.
        let p = GeostationaryParams {
            ni: 21,
            nj: 21,
            h_metres: 42_164_160.0,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314_14,
            sub_lon_deg: -75.0,
            sweep_x: true,
            x0: -0.151844,
            dx_rad: 0.0151844,
            y0: 0.151844,
            dy_rad: -0.0151844,
        };
        let proj = GeostationaryProjector::new(p);
        // Centre pixel looks straight down: the sub-satellite point.
        let (lat, lon) = proj
            .grid_point_lonlat(10, 10)
            .expect("centre is on the disk");
        assert!(
            near(lat, 0.0, 1e-6) && near(lon, -75.0, 1e-6),
            "({lat},{lon})"
        );
        // The raster corners are off the limb.
        for (i, j) in [(0u32, 0u32), (20, 0), (0, 20), (20, 20)] {
            assert!(
                proj.grid_point_lonlat(i, j).is_none(),
                "corner ({i},{j}) should miss the Earth"
            );
        }
        // On-disk points round-trip through the inverse.
        for (i, j) in [(10u32, 10u32), (8, 12), (12, 8), (10, 6)] {
            let (lat, lon) = proj.grid_point_lonlat(i, j).expect("on disk");
            let idx = proj
                .inverse(lat, lon)
                .unwrap_or_else(|| panic!("({i},{j}) → ({lat},{lon}) → off-grid"));
            assert!(
                near(idx.i, i as f64, 1e-6) && near(idx.j, j as f64, 1e-6),
                "({i},{j}) → ({}, {})",
                idx.i,
                idx.j
            );
        }
    }

    #[test]
    fn latlon_forward_matches_the_eccodes_point_iterator() {
        // The round-trip tests above pin the forward map against our own
        // inverse; this pins it against an *outside* oracle. Geometry and
        // coordinates are eccodes' `grib_get_data` output for the committed
        // fixture `crates/fieldglass-grib2/tests/fixtures/ccsds_regular_latlon.grib2`
        // (16 × 31, 60°N 0°E → 0°N 30°E), which is what a field export must
        // reproduce point for point. The lat/lon family carries no Earth-radius
        // dependence, so this is an exact check rather than a tolerance.
        let p = LatLonParams {
            ni: 16,
            nj: 31,
            lat_first: 60.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 30.0,
        };
        for (i, j, lat, lon) in [
            (0, 0, 60.0, 0.0),   // first point
            (1, 0, 60.0, 2.0),   // one column east: Δλ = 30/15 = 2°
            (7, 10, 40.0, 14.0), // interior: lat 60 - 10·2, lon 7·2
            (15, 30, 0.0, 30.0), // last point
        ] {
            let got = latlon_point(&p, i, j).expect("on grid");
            assert!(
                near(got.0, lat, 1e-9) && near(got.1, lon, 1e-9),
                "({i},{j}) → {got:?}, eccodes says ({lat}, {lon})"
            );
        }
    }

    #[test]
    fn normalise_lon_wraps_into_the_conventional_range() {
        assert!(near(normalise_lon(460.0), 100.0, 1e-12));
        assert!(near(normalise_lon(-190.0), 170.0, 1e-12));
        assert!(near(normalise_lon(0.0), 0.0, 1e-12));
        // The half-open convention: +180 folds onto -180, and stays there.
        assert!(near(normalise_lon(180.0), -180.0, 1e-12));
        assert!(near(normalise_lon(-180.0), -180.0, 1e-12));
    }

    #[test]
    fn unrotate_matches_eccodes_oracle() {
        let p = rotated_fixture_params();
        // First grid point: rotated (60, 0) → geographic (30, 180).
        let (lat, lon) = unrotate_latlon(
            p.lat_first,
            p.lon_first,
            p.angle_of_rotation,
            p.south_pole_lat,
            p.south_pole_lon,
        );
        assert!(near(lat, 30.0, 1e-6), "first-point lat = {lat}");
        assert!(near(lon, 180.0, 1e-6), "first-point lon = {lon}");
        // Last grid point: rotated (0, 30) → geographic (60, 90).
        let (lat, lon) = unrotate_latlon(
            p.lat_last,
            p.lon_last,
            p.angle_of_rotation,
            p.south_pole_lat,
            p.south_pole_lon,
        );
        assert!(near(lat, 60.0, 1e-6), "last-point lat = {lat}");
        assert!(near(lon, 90.0, 1e-6), "last-point lon = {lon}");
        // An interior first-row point: rotated (60, 2) → geographic
        // (29.980, 178.846) per the oracle (printed to 3 decimals).
        let (lat, lon) = unrotate_latlon(60.0, 2.0, 0.0, 0.0, 0.0);
        assert!(near(lat, 29.980, 2e-3), "interior lat = {lat}");
        assert!(near(lon, 178.846, 2e-3), "interior lon = {lon}");
    }

    #[test]
    fn rotate_is_inverse_of_unrotate() {
        // A non-trivial pole so every matrix term is exercised, plus a rotation
        // angle to cover the longitude shift.
        let (sp_lat, sp_lon, angle) = (-36.0, 18.0, 12.0);
        for &(rlat, rlon) in &[(45.0, 10.0), (-20.0, -75.0), (5.0, 140.0)] {
            let (lat, lon) = unrotate_latlon(rlat, rlon, angle, sp_lat, sp_lon);
            let (back_lat, back_lon) = rotate_latlon(lat, lon, angle, sp_lat, sp_lon);
            assert!(near(back_lat, rlat, 1e-9), "rlat {rlat} -> {back_lat}");
            // Compare longitudes modulo 360 to ignore wrap.
            let dlon = ((back_lon - rlon + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(near(dlon, 0.0, 1e-9), "rlon {rlon} -> {back_lon}");
        }
    }

    #[test]
    fn rotated_inverse_maps_corners_to_grid_extent() {
        let p = rotated_fixture_params();
        let proj = RotatedLatLonProjector::new(p);
        // Geographic first corner (30, 180) → index (0, 0).
        let first = proj.inverse(30.0, 180.0).expect("first corner");
        assert!(near(first.i, 0.0, 1e-6) && near(first.j, 0.0, 1e-6));
        // Geographic last corner (60, 90) → index (ni-1, nj-1).
        let last = proj.inverse(60.0, 90.0).expect("last corner");
        assert!(near(last.i, p.ni as f64 - 1.0, 1e-6), "i = {}", last.i);
        assert!(near(last.j, p.nj as f64 - 1.0, 1e-6), "j = {}", last.j);
    }

    #[test]
    fn rotated_inverse_rejects_off_grid_and_nonfinite() {
        let proj = RotatedLatLonProjector::new(rotated_fixture_params());
        // Geographic (0, 0) rotates to the antipodal side of the grid.
        assert!(proj.inverse(0.0, 0.0).is_none(), "off-grid point");
        assert!(proj.inverse(f64::NAN, 180.0).is_none(), "NaN lat");
        assert!(proj.inverse(30.0, f64::INFINITY).is_none(), "inf lon");
    }

    #[test]
    fn rotated_bbox_covers_corner_latitudes() {
        // The geographic corner latitudes (30 and 60) must lie within the
        // reported box, and the box must not collapse.
        let (lat_min, lat_max, lon_min, lon_max) =
            RotatedLatLonProjector::new(rotated_fixture_params()).lonlat_bbox();
        assert!(
            lat_min <= 30.0 + 1e-6 && lat_max >= 60.0 - 1e-6,
            "lat box too tight"
        );
        assert!(lon_max > lon_min, "degenerate lon span");
    }

    #[test]
    fn rotated_bbox_handles_antimeridian_crossing_rotated_columns() {
        // Real ECCC HRDPS continental grid: its rotated columns run
        // 345.18° → 42.31°, across the rotated antimeridian. Interpolating the
        // row edges over the raw corner difference swept the ~303° complement
        // arc — rotated longitudes that aren't in the grid — inflating the box
        // to nearly the whole globe, so the equirectangular render split the
        // field across the window's left and right edges.
        let p = RotatedLatLonParams {
            ni: 2540,
            nj: 1290,
            lat_first: -12.302501,
            lon_first: 345.17878,
            lat_last: 16.700001,
            lon_last: 42.306283,
            south_pole_lat: -36.08852,
            south_pole_lon: 245.305142,
            angle_of_rotation: 0.0,
        };
        let (lat_min, lat_max, lon_min, lon_max) = RotatedLatLonProjector::new(p).lonlat_bbox();
        // The HRDPS continental domain covers North America — roughly
        // 27°N..71°N, 153°W..41°W, a ~112° longitude window.
        assert!(near(lat_min, 27.28, 0.05), "lat_min = {lat_min}");
        assert!(near(lat_max, 70.61, 0.05), "lat_max = {lat_max}");
        assert!(near(lon_min, -152.73, 0.05), "lon_min = {lon_min}");
        assert!(near(lon_max, -40.71, 0.05), "lon_max = {lon_max}");
    }

    // -----------------------------------------------------------------
    // Geostationary / space view
    // -----------------------------------------------------------------

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
    fn geostationary_free_fn_matches_projector() {
        let p = goes_east_params();
        let a = geostationary_inverse(&p, 10.0, -70.0);
        let b = GeostationaryProjector::new(p).inverse(10.0, -70.0);
        assert_eq!(a.is_some(), b.is_some());
        if let (Some(a), Some(b)) = (a, b) {
            assert!(near(a.i, b.i, 1e-12) && near(a.j, b.j, 1e-12));
        }
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
