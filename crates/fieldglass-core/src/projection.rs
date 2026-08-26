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

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
/// row `j`, concatenated — and the result is row-major `width` columns per row,
/// with `width = max(PL)` ([`reduced_raster_width`]).
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
    width: usize,
) -> Vec<Option<f64>> {
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

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
    ///
    /// A declared Earth radius of zero (or less) is the quieter case, and the
    /// reason the radius is checked here rather than left to the `is_finite`
    /// tests above: it scales `rho0` to zero without making anything infinite,
    /// so every point on the cone inverts to the pole. That reads as a real
    /// coordinate to anything downstream — a rendered map, a CSV row — instead
    /// of as a broken grid.
    fn well_defined(&self) -> bool {
        self.earth_r.is_finite()
            && self.earth_r > 0.0
            && self.n.is_finite()
            && self.n != 0.0
            && self.f_const.is_finite()
            && self.rho0.is_finite()
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
    /// `ni × nj` grid extent. The shared planar body — kept as an inherent
    /// method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
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
    /// parallels (see `LambertConstants::well_defined`); such a projector's
    /// [`inverse`](Self::inverse) always returns `None`, so callers can surface
    /// "not reprojectable" instead of rendering blank.
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined()
    }
}

// ---------------------------------------------------------------------------
// Polar Stereographic (GRIB1 grid_type 5, GRIB2 template 3.20)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
/// Defined everywhere except the *opposite* pole, where `tan` diverges. In
/// `f64` that divergence does not actually reach infinity — `(PI/4 + PI/4).tan()`
/// is about 1.6e16, because `PI/2` is not exactly representable — so a caller
/// at the antipodal pole gets a finite `(x, y)` around 1e23 rather than `±inf`.
/// [`PolarStereoProjector::inverse`] relies on that landing far outside any
/// grid's extent rather than on a finiteness check.
///
/// `rho` is strictly monotonic in latitude across the whole open range, so the
/// projection is injective there: a point in the hemisphere *opposite* the
/// projection pole has a large `rho`, but it is still that point's own `rho`
/// and cannot alias onto another. Grids that reach across the equator — the CMC
/// regional grid is one — depend on this.
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

    /// Project `(lat, lon)` back to the source-grid fractional index, or
    /// `None` when it falls outside the `ni × nj` extent. The shared planar
    /// body — kept as an inherent method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
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

/// Apply the GRIB scanning-mode sign to a planar projection's grid spacings.
///
/// Both GRIB1 and GRIB2 store Dx/Dy as unsigned magnitudes and carry the scan
/// direction in separate flags. The planar projectors map a point to a grid
/// index by `i = (x - origin_x) / dx`, `j = (y - origin_y) / dy` in the
/// LoV-oriented projection plane, so the increment sign *is* the scan
/// direction: `i` runs −x when it scans negatively, and `j` runs −y
/// (north→south) unless it scans positively. Default-scan grids keep positive
/// values.
///
/// Every caller that walks from the first scanned point to another grid point
/// needs this, which is why it lives beside the projectors rather than in one
/// of the format crates: the grid's declared corner is the *first scanned*
/// point, so stepping the wrong way puts the far corner on the wrong side of
/// it entirely.
pub fn signed_grid_increments(
    dx: f64,
    dy: f64,
    i_scans_negatively: bool,
    j_scans_positively: bool,
) -> (f64, f64) {
    let sdx = if i_scans_negatively {
        -dx.abs()
    } else {
        dx.abs()
    };
    let sdy = if j_scans_positively {
        dy.abs()
    } else {
        -dy.abs()
    };
    (sdx, sdy)
}

/// A projection whose source grid lies on a plane in metres — a fixed origin
/// at the first scanned point and constant `(dx, dy)` spacing. Lambert
/// conformal and polar stereographic both qualify; lat/lon and Gaussian grids
/// are already geographic and don't.
///
/// Implementors supply four cheap accessors; the trait derives the grid
/// corners from them. This is the one geometry shared by every planar warp
/// setup (target-bbox derivation) and by GRIB `bounds()` reporting, which
/// otherwise reimplement `origin + (n-1)·d` per projection.
/// How far past a grid edge a computed index may sit and still be snapped
/// What resampling a grid's geometry can support.
///
/// Lives here rather than beside [`Resampling`](crate::warp::Resampling)
/// because [`GridGeometry`] reports it and must stay outside the `render`
/// feature — the format crates take `core` with `default-features = false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GridResampling {
    /// A raster grid: the fractional part of a [`GridIndex`] is a position
    /// inside the cell and `(i + 1, j)` is the neighbour to its east, so
    /// blending between them is meaningful.
    Any,
    /// A lookup grid: the answer is a cell, not a position within one.
    /// Index-adjacent cells need not be spatially adjacent — a tripolar ocean
    /// grid folds — so blending `(i, j)` with `(i + 1, j)` would average two
    /// places that are nowhere near each other.
    NearestOnly,
}

#[cfg(feature = "render")]
impl GridResampling {
    /// What a `method` request actually becomes on this geometry.
    ///
    /// [`crate::warp::warp`] applies this before resampling, so a caller that
    /// wants to *report* what happened — a render summary naming the method —
    /// must ask the same question rather than echo the request back. Reporting
    /// "bilinear" for a lookup grid names a blend that was never performed.
    pub fn applied_to(self, method: crate::warp::Resampling) -> crate::warp::Resampling {
        match self {
            Self::NearestOnly => crate::warp::Resampling::Nearest,
            Self::Any => method,
        }
    }
}

/// The edge tolerance a projection whose round trip closes to float noise
/// needs: a nanometre of a grid cell — far above the round-off a well-behaved
/// round trip leaves, far below any real offset.
///
/// One value rather than two, because it is one rule. It is the default for
/// [`PlanarGridProjector::snap_eps`] and what [`GeostationaryProjector::
/// inverse`] uses; geostationary cannot implement that trait, since its grid
/// coordinates are scan angles in radians rather than projected metres.
///
/// # Which families snap, and which do not
///
/// #490 survived because nothing wrote this down. Keep it current when a
/// family is added:
///
/// | Family | Snap | Unit |
/// | --- | --- | --- |
/// | Lambert, polar stereo, transverse Mercator | [`PlanarGridProjector::snap_eps`] default | cell fractions |
/// | Lambert azimuthal | same hook, overridden | metres (its authalic series carries real error) |
/// | Rotated lat/lon | its own `EDGE_EPS` | **degrees**, applied in rotated space before [`latlon_inverse`] |
/// | Geostationary | this constant | cell fractions |
/// | lat/lon, Mercator, Gaussian | **none** | — |
///
/// The last row is deliberate, not an oversight of the #490 kind. Those three
/// invert by undoing their own forward arithmetic (`(lat - lat_first) / dlat`),
/// so the round trip closes to ~1e-13 cells and cannot cross a bound. A family
/// whose inverse is a *different computation* from its forward — geostationary
/// intersects a ray with an ellipsoid — has no such guarantee and needs the
/// snap. That is the question to ask of anything new, and
/// `tests/grid_round_trip.rs` is where the answer is checked.
pub const DEFAULT_SNAP_EPS: SnapEps = SnapEps::Cells(1e-9);

/// back onto it. See [`PlanarGridProjector::snap_eps`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapEps {
    /// A fraction of a grid cell, applied identically on both axes. What a
    /// projection whose round trip closes to float noise needs.
    Cells(f64),
    /// A distance in projected metres, divided by each axis' spacing. What a
    /// projection whose round trip carries a real, ground-scale error needs:
    /// the tolerance is then a property of the Earth, not of the cell, and a
    /// fixed cell fraction would be far too tight on a coarse grid.
    Metres(f64),
}

impl SnapEps {
    /// The `(i, j)` tolerances, in cell fractions, for a grid of this spacing.
    fn per_axis(self, dx: f64, dy: f64) -> (f64, f64) {
        match self {
            SnapEps::Cells(e) => (e, e),
            SnapEps::Metres(m) => (m / dx.abs(), m / dy.abs()),
        }
    }
}

pub trait PlanarGridProjector {
    /// Grid origin (first scanned point) in projected metres.
    fn grid_origin(&self) -> (f64, f64);
    /// `(ni, nj)` grid dimensions in points.
    fn grid_dims(&self) -> (u32, u32);
    /// `(dx, dy)` spacing in metres at the latitude of true scale.
    fn grid_spacing(&self) -> (f64, f64);
    /// Inverse-project projected metres back to `(lat, lon)` in degrees.
    fn inverse_lonlat(&self, x: f64, y: f64) -> (f64, f64);
    /// Forward-project `(lat, lon)` in degrees to projected metres — the
    /// direction [`Self::inverse_lonlat`] undoes. [`Self::inverse`] walks it:
    /// a grid index is `(forward_xy(lat, lon) - origin) / spacing`.
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64);

    /// Whether this projector can place `(lat, lon)` at all, asked before the
    /// forward map runs. Two kinds of answer live here. A projection whose own
    /// constants are degenerate rejects every point; one with a singularity
    /// within reach rejects the region that would hit it — polar
    /// stereographic's opposite hemisphere, where the forward `tan` runs to
    /// ±inf and the origin-relative division would turn that back into a
    /// plausible-looking index.
    ///
    /// The default accepts everything. [`Self::inverse`] already rejects a
    /// non-finite argument and a non-finite forward result, so a projector
    /// needs this only for a case those two miss.
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        true
    }

    /// How far outside the grid an index may land and still be snapped back
    /// onto the edge.
    ///
    /// The projection arithmetic carries round-off — enough to push a point
    /// sitting exactly *on* a grid edge a hair outside it and have the extent
    /// check reject it, dropping the outermost row or column to background.
    /// The default is a nanometre of a grid cell: far above the round-off a
    /// well-behaved round trip leaves, far below any real offset. It is the
    /// same rule, for the same reason, that the rotated lat/lon inverse
    /// applies. Override it where the round trip is not exact to float noise.
    ///
    /// A new implementor inherits this without being asked, and getting it
    /// wrong is silent: the symptom is a missing outer row in a render, not an
    /// error. Check a new projector against its own
    /// [`grid_corners_lonlat`](Self::grid_corners_lonlat) — all four must come
    /// back through [`inverse`](Self::inverse) inside the grid.
    /// `tests/planar_inverse_golden.rs` does that for the projectors in its
    /// table; add yours to it.
    fn snap_eps(&self) -> SnapEps {
        DEFAULT_SNAP_EPS
    }

    /// `(lat, lon)` → fractional source-grid index, or `None` when the point
    /// falls outside the `ni × nj` extent or the projection cannot place it.
    ///
    /// The body every planar grid shares: reject what cannot be placed, walk
    /// the forward map, divide out the origin and the spacing, and let
    /// [`Self::snap_eps`] rescue a point that round-off pushed just past an
    /// edge. What differs between projections is in the three hooks, not here.
    fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() || !self.accepts(lat, lon) {
            return None;
        }
        let (ni, nj) = self.grid_dims();
        let (dx, dy) = self.grid_spacing();
        // A one-point axis has no cell to interpolate across, and a zero
        // spacing would divide the whole plane onto one index.
        if ni < 2 || nj < 2 || dx == 0.0 || dy == 0.0 {
            return None;
        }
        let (x, y) = self.forward_xy(lat, lon);
        if !x.is_finite() || !y.is_finite() {
            // The forward map hit a singularity — a pole for the conics, the
            // antipode of the tangent point for the azimuthals.
            return None;
        }
        let (ox, oy) = self.grid_origin();
        let (i_max, j_max) = (ni as f64 - 1.0, nj as f64 - 1.0);
        let (eps_i, eps_j) = self.snap_eps().per_axis(dx, dy);
        let i = snap_to_range((x - ox) / dx, 0.0, i_max, eps_i);
        let j = snap_to_range((y - oy) / dy, 0.0, j_max, eps_j);
        if i < 0.0 || i > i_max || j < 0.0 || j > j_max {
            return None;
        }
        Some(GridIndex { i, j })
    }

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
            // Skip perimeter samples that are not points on Earth. Every planar
            // inverse here has somewhere it cannot answer: Lambert's blows up at
            // the projection pole, and Lambert azimuthal equal-area maps the
            // globe onto a disc whose outside is nowhere at all. One `NaN`
            // sample poisons `lon.rem_euclid(360.0)` and the whole longitude
            // bound with it, and a `NaN` bbox reaches the warp as target extent,
            // where it renders as an empty raster rather than as an error.
            //
            // A grid can legitimately be part-way off the disc — an oversized or
            // malformed §3.140 puts 1 369 of 1 600 points off it — and the right
            // answer for those is a box around the part that exists.
            if lat.is_finite() && lon.is_finite() {
                lat_min = lat_min.min(lat);
                lat_max = lat_max.max(lat);
                lons.push(lon.rem_euclid(360.0));
            }
        };
        for k in 0..=PER_EDGE {
            let t = k as f64 / PER_EDGE as f64;
            visit(ox + t * ex, oy); // bottom edge (j = 0)
            visit(ox + t * ex, oy + ey); // top edge (j = nj-1)
            visit(ox, oy + t * ey); // left edge (i = 0)
            visit(ox + ex, oy + t * ey); // right edge (i = ni-1)
        }

        // A perimeter with no projectable point at all — the whole grid is off
        // the map. Report the empty box rather than `±INFINITY`, which would
        // propagate into the target raster's size.
        if lons.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
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
pub(crate) fn enclosing_lon_arc(lons: &mut [f64]) -> (f64, f64) {
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
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
    }
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        // Degenerate standard parallels (see `LambertConstants::well_defined`)
        // leave no usable cone, so no point can be placed on this grid.
        self.constants.well_defined()
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
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
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
// Lambert azimuthal equal-area (GRIB2 template 3.140)
// ---------------------------------------------------------------------------

/// A Lambert azimuthal equal-area grid: the plane is tangent at one point and
/// area is preserved exactly, which is why Europe's statistical grids and the
/// CEMS/EFAS flood archive are published on it (ETRS89-LAEA, EPSG:3035), along
/// with EUMETSAT OSI SAF sea-ice products.
///
/// # Why this one is on the spheroid too
///
/// Same reason as [`TransverseMercatorParams`], further along: ETRS89-LAEA is
/// GRS80, and over the EFAS domain the spheroid's mean radius puts the far
/// corner **13.5 km** from where it belongs — several grid cells at 5 km.
///
/// It is also what eccodes does. Its `lambert_azimuthal_equal_area` iterator
/// branches on `grib_is_earth_oblate` and runs the authalic-latitude algorithm
/// for an oblate shape, so a mean-radius implementation could not agree with
/// the oracle the acceptance criterion names, never mind with the ground.
///
/// The series degenerates on its own when `a == b`: the authalic corrections
/// are a power series in the eccentricity, `qsfn` collapses to `2 sin φ`, and
/// what is left is the spherical formula eccodes' own `init_sphere` uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LambertAzimuthalParams {
    /// Semi-major and semi-minor axes in metres, as the message declares them.
    pub semi_major_m: f64,
    pub semi_minor_m: f64,
    pub ni: u32,
    pub nj: u32,
    /// First grid point, in degrees. §3.140 states it geographically, so the
    /// projector forward-projects it to find the grid origin in the plane.
    pub lat_first: f64,
    pub lon_first: f64,
    /// The tangent point: `standardParallel` is its latitude and
    /// `centralLongitude` its longitude.
    pub standard_parallel: f64,
    pub central_longitude: f64,
    /// Grid spacing in metres, carrying the scanning-mode sign.
    pub dx_metres: f64,
    pub dy_metres: f64,
}

/// Authalic-projection constants for one spheroid and tangent point. All of it
/// depends only on `(a, b, standard_parallel)`, so a warp loop computes it once.
///
/// `pub` so projector helpers can hand them around; the fields are private.
#[derive(Debug, Clone, Copy)]
pub struct LambertAzimuthalConstants {
    /// First eccentricity, and its square. Zero for a sphere, which collapses
    /// every correction below.
    e: f64,
    es: f64,
    /// `q` at the pole — the authalic normalising constant.
    qp: f64,
    /// `√(qp / 2)`, the authalic sphere's radius in units of `a`.
    rq: f64,
    /// Authalic sine and cosine of the tangent latitude.
    sinb1: f64,
    cosb1: f64,
    /// The oblateness stretch that keeps the projection equal-area, and the two
    /// scale factors derived from it.
    dd: f64,
    xmf: f64,
    ymf: f64,
    /// Authalic → geodetic latitude series.
    apa: [f64; 3],
    semi_major_m: f64,
}

impl LambertAzimuthalConstants {
    /// Whether these constants describe a usable projection. A degenerate
    /// spheroid otherwise yields finite, meaningless coordinates — the same
    /// trap [`TransverseMercatorConstants::well_defined`] guards.
    fn well_defined(&self) -> bool {
        self.qp.is_finite()
            && self.qp > 0.0
            && self.rq.is_finite()
            && self.rq > 0.0
            && self.dd.is_finite()
            && self.dd != 0.0
            && self.xmf.is_finite()
            && self.ymf.is_finite()
            && self.semi_major_m.is_finite()
            && self.semi_major_m > 0.0
    }
}

/// `q`, the authalic area function. eccodes' `pj_qsfn`, which is PROJ's.
///
/// The `e >= 1e-7` guard is theirs too, and it is what makes the spherical case
/// fall out rather than dividing by an eccentricity of zero.
fn authalic_q(sin_lat: f64, e: f64, one_es: f64) -> f64 {
    const EPSILON: f64 = 1.0e-7;
    if e < EPSILON {
        return sin_lat + sin_lat;
    }
    let con = e * sin_lat;
    let div1 = 1.0 - con * con;
    let div2 = 1.0 + con;
    if div1 == 0.0 || div2 == 0.0 {
        return f64::INFINITY;
    }
    one_es * (sin_lat / div1 - (0.5 / e) * ((1.0 - con) / div2).ln())
}

/// Authalic → geodetic latitude, via PROJ's `pj_authset` / `pj_authlat` series.
fn authalic_series(es: f64) -> [f64; 3] {
    const P00: f64 = 0.333_333_333_333_333_3;
    const P01: f64 = 0.172_222_222_222_222_22;
    const P02: f64 = 0.102_579_365_079_365_08;
    const P10: f64 = 0.063_888_888_888_888_88;
    const P11: f64 = 0.066_402_116_402_116_4;
    const P20: f64 = 0.016_776_895_943_562_61;
    let es2 = es * es;
    let es3 = es2 * es;
    [
        es * P00 + es2 * P01 + es3 * P02,
        es2 * P10 + es3 * P11,
        es3 * P20,
    ]
}

fn authalic_to_geodetic(beta: f64, apa: &[f64; 3]) -> f64 {
    let t = beta + beta;
    beta + apa[0] * t.sin() + apa[1] * (t + t).sin() + apa[2] * (t + t + t).sin()
}

fn lambert_azimuthal_constants(p: &LambertAzimuthalParams) -> LambertAzimuthalConstants {
    let a = p.semi_major_m;
    let b = p.semi_minor_m;
    // Same guard, and the same reason, as `transverse_mercator_constants`: an
    // impossible spheroid must fail `well_defined` rather than project.
    if !(a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && b <= a) {
        return LambertAzimuthalConstants {
            e: f64::NAN,
            es: f64::NAN,
            qp: f64::NAN,
            rq: f64::NAN,
            sinb1: f64::NAN,
            cosb1: f64::NAN,
            dd: f64::NAN,
            xmf: f64::NAN,
            ymf: f64::NAN,
            apa: [f64::NAN; 3],
            semi_major_m: f64::NAN,
        };
    }
    let f = (a - b) / a;
    let es = 2.0 * f - f * f;
    let one_es = 1.0 - es;
    let e = es.sqrt();

    let qp = authalic_q(1.0, e, one_es);
    let rq = (0.5 * qp).sqrt();
    let lat1 = p.standard_parallel * DEG2RAD;
    let sin_lat1 = lat1.sin();
    // The pole's authalic latitude *is* the pole, so pin it rather than
    // deriving it. `authalic_q(-1)` takes the logarithm of the reciprocal of
    // the argument `qp` was built from, and floating point does not make those
    // exact negatives: the derived `sinb1` lands a few ulps inside -1, leaving
    // `cosb1` at about 1.5e-8 instead of 0. That leaks into the inverse's `ab`
    // term and biases every latitude on a south-polar grid by a constant 19 cm,
    // at every radius. The north pole is exact by luck — `authalic_q(1)` is
    // literally the expression `qp` came from — which is why the bug only shows
    // on one hemisphere.
    const POLAR_EPS: f64 = 1.0e-10;
    let is_polar = (lat1.abs() - std::f64::consts::FRAC_PI_2).abs() < POLAR_EPS;
    let (sinb1, cosb1) = if is_polar {
        (sin_lat1.signum(), 0.0)
    } else {
        let sinb1 = authalic_q(sin_lat1, e, one_es) / qp;
        (sinb1, (1.0 - sinb1 * sinb1).max(0.0).sqrt())
    };
    // The oblateness stretch is undefined at the pole and unnecessary there.
    // PROJ decides this from the *latitude*, as above; eccodes dropped that
    // branch and guards `cosb1 == 0.0` instead, which is not the same test.
    // With `cosb1` coming out at 1.5e-8 rather than 0 at the south pole, the
    // guard misses, `dd` becomes cos(-π/2) — 6e-17, not 0 — divided by it, and
    // the projected plane inflates by eight orders of magnitude. eccodes 2.48
    // asked for a south-polar §3.140's latitudes answers
    // `Invalid value: arcsin argument=7.60531e+06` and declines to build the
    // iterator at all.
    let dd = if is_polar || cosb1 == 0.0 {
        1.0
    } else {
        lat1.cos() / ((1.0 - es * sin_lat1 * sin_lat1).sqrt() * rq * cosb1)
    };
    LambertAzimuthalConstants {
        e,
        es,
        qp,
        rq,
        sinb1,
        cosb1,
        dd,
        xmf: rq * dd,
        ymf: rq / dd,
        apa: authalic_series(es),
        semi_major_m: a,
    }
}

/// How close to the antipode of the tangent point counts as "off the map".
/// The projection maps the whole sphere onto a disc, and its far edge is a
/// single point; `b` in the forward map goes to zero there and the scale to
/// infinity.
const LAEA_EPS: f64 = 1.0e-10;

/// Forward Lambert azimuthal equal-area: `(lat, lon)` in degrees → `(x, y)` in
/// metres from the tangent point.
///
/// Returns non-finite coordinates at the antipode of the tangent point, where
/// the projection is undefined. Callers going through
/// [`LambertAzimuthalProjector::inverse`] get `None` there instead.
///
/// **Recomputes the constants per call.** For warp loops use
/// [`LambertAzimuthalProjector`].
pub fn lambert_azimuthal_forward(p: &LambertAzimuthalParams, lat: f64, lon: f64) -> (f64, f64) {
    lambert_azimuthal_forward_with(&lambert_azimuthal_constants(p), p, lat, lon)
}

fn lambert_azimuthal_forward_with(
    k: &LambertAzimuthalConstants,
    p: &LambertAzimuthalParams,
    lat: f64,
    lon: f64,
) -> (f64, f64) {
    // Wrapped before the trig, the same trap `lambert_forward_with` documents.
    let d_lon = ((lon - p.central_longitude + 180.0).rem_euclid(360.0) - 180.0) * DEG2RAD;
    let one_es = 1.0 - k.es;
    let sinb = authalic_q((lat * DEG2RAD).sin(), k.e, one_es) / k.qp;
    let cosb = (1.0 - sinb * sinb).max(0.0).sqrt();
    let denom = 1.0 + k.sinb1 * sinb + k.cosb1 * cosb * d_lon.cos();
    if denom.abs() < LAEA_EPS {
        return (f64::NAN, f64::NAN);
    }
    let bb = (2.0 / denom).sqrt();
    (
        k.semi_major_m * k.xmf * bb * cosb * d_lon.sin(),
        k.semi_major_m * k.ymf * bb * (k.cosb1 * sinb - k.sinb1 * cosb * d_lon.cos()),
    )
}

/// Inverse Lambert azimuthal equal-area: `(x, y)` in metres → `(lat, lon)` in
/// degrees. Same recompute caveat as [`lambert_azimuthal_forward`].
pub fn lambert_azimuthal_inverse_xy(p: &LambertAzimuthalParams, x: f64, y: f64) -> (f64, f64) {
    lambert_azimuthal_inverse_xy_with(&lambert_azimuthal_constants(p), p, x, y)
}

fn lambert_azimuthal_inverse_xy_with(
    k: &LambertAzimuthalConstants,
    p: &LambertAzimuthalParams,
    x: f64,
    y: f64,
) -> (f64, f64) {
    let mut xy_x = (x / k.semi_major_m) / k.dd;
    let mut xy_y = (y / k.semi_major_m) * k.dd;
    let rho = xy_x.hypot(xy_y);
    // The tangent point itself. eccodes asserts `rho >= 1e-10` here and aborts
    // the process if a grid ever lands on it; answering with the tangent point
    // is both correct and survivable.
    if rho < LAEA_EPS {
        return (p.standard_parallel, p.central_longitude);
    }
    let asin_arg = 0.5 * rho / k.rq;
    if !(-1.0..=1.0).contains(&asin_arg) {
        // Beyond the projection disc — off the map, not a point on Earth.
        return (f64::NAN, f64::NAN);
    }
    let ce = 2.0 * asin_arg.asin();
    let (sin_ce, cos_ce) = ce.sin_cos();
    xy_x *= sin_ce;
    let ab = cos_ce * k.sinb1 + xy_y * sin_ce * k.cosb1 / rho;
    xy_y = rho * k.cosb1 * cos_ce - xy_y * k.sinb1 * sin_ce;
    let lon = p.central_longitude + xy_x.atan2(xy_y) * RAD2DEG;
    let lat = authalic_to_geodetic(ab.clamp(-1.0, 1.0).asin(), &k.apa) * RAD2DEG;
    (lat, lon)
}

/// Inverse warp: `(lat, lon)` → fractional source grid index. **Recomputes the
/// constants per call** — for warp loops prefer [`LambertAzimuthalProjector`].
pub fn lambert_azimuthal_inverse(
    p: &LambertAzimuthalParams,
    lat: f64,
    lon: f64,
) -> Option<GridIndex> {
    LambertAzimuthalProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a Lambert azimuthal equal-area grid. Owns the
/// authalic constants and the forward-projected grid origin, both invariant
/// across a warp's output pixels.
pub struct LambertAzimuthalProjector {
    pub params: LambertAzimuthalParams,
    constants: LambertAzimuthalConstants,
    origin: (f64, f64),
}

impl LambertAzimuthalProjector {
    pub fn new(params: LambertAzimuthalParams) -> Self {
        let constants = lambert_azimuthal_constants(&params);
        let origin =
            lambert_azimuthal_forward_with(&constants, &params, params.lat_first, params.lon_first);
        Self {
            params,
            constants,
            origin,
        }
    }

    /// Project `(lat, lon)` back to the source-grid fractional index, or `None`
    /// when it falls outside the `ni × nj` extent. The shared planar body —
    /// kept as an inherent method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
    }

    /// Forward-project through the cached constants.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        lambert_azimuthal_forward_with(&self.constants, &self.params, lat, lon)
    }

    /// Inverse-project projected metres back to `(lat, lon)`.
    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        lambert_azimuthal_inverse_xy_with(&self.constants, &self.params, x, y)
    }

    /// Read-only access to the precomputed grid origin in projected metres.
    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }

    /// Whether the projection is usable. `false` leaves
    /// [`inverse`](Self::inverse) returning `None` for every point, so callers
    /// can surface "not reprojectable" rather than render blank.
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined() && self.origin.0.is_finite() && self.origin.1.is_finite()
    }
}

impl PlanarGridProjector for LambertAzimuthalProjector {
    fn grid_origin(&self) -> (f64, f64) {
        self.origin
    }
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
    }
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        self.is_well_defined()
    }
    fn snap_eps(&self) -> SnapEps {
        // The only projector here whose round trip does not close to float
        // noise. `authalic_to_geodetic` is a three-term series and so is not
        // the exact inverse of the `authalic_q` that produced the latitude —
        // PROJ and eccodes carry the same asymmetry, at about a millimetre. A
        // fixed nanometre of a cell is below that for any real grid (5e-9 of a
        // 200 km cell), so the first grid point projected back to `-1.2e-9`
        // and was rejected, dropping the whole outer row and column to
        // background.
        //
        // A centimetre is three orders of magnitude under the coarsest sane
        // grid spacing and one above the series' own error.
        SnapEps::Metres(0.01)
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
// Transverse Mercator (GRIB2 template 3.12)
// ---------------------------------------------------------------------------

/// A transverse Mercator grid: the UTM / British National Grid construction,
/// with the projection cylinder tangent along a meridian rather than the
/// equator. The Met Office publishes UKV, its 1.5 km UK model, on one.
///
/// # Why this one is on the spheroid
///
/// Every other projection in this module runs on a sphere, because the grids
/// that use them declare a spherical Earth (`shapeOfTheEarth` 6, R = 6 371 229 m)
/// and the mean-radius fallback is then exact. Transverse Mercator is where that
/// stops being true: a UKV message declares Airy 1830, and projecting it on the
/// spheroid's mean radius places the field about **2.8 km** from where it
/// belongs — a couple of grid cells at 1.5 km, visible as a coastline offset.
///
/// So this uses the Krüger *n*-series, which is exact on the spheroid to well
/// below a millimetre over a grid this size, and which **degenerates to the
/// spherical formulae on its own** when `a == b`: every α, β and δ coefficient
/// is a power series in `n = f / (2 - f)`, and `n` is zero for a sphere. There
/// is no separate spherical path to keep in step, and no accuracy to trade.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransverseMercatorParams {
    /// Semi-major and semi-minor axes in metres, as the message declares them.
    pub semi_major_m: f64,
    pub semi_minor_m: f64,
    pub ni: u32,
    pub nj: u32,
    /// `LaR` / `LoR` — the reference point, in degrees. `lon_ref` is the
    /// central meridian.
    pub lat_ref: f64,
    pub lon_ref: f64,
    /// `m` — scale factor at the central meridian.
    pub scale_factor: f64,
    /// `XR` / `YR` — false easting and northing in metres.
    pub false_easting_m: f64,
    pub false_northing_m: f64,
    /// `X1` / `Y1` — the first scanned grid point, in projection metres. Unlike
    /// Lambert and polar stereographic, §3.12 states the origin in the plane, so
    /// there is no forward projection to do to find it.
    pub x1_metres: f64,
    pub y1_metres: f64,
    /// Grid spacing in metres, carrying the scanning-mode sign.
    pub dx_metres: f64,
    pub dy_metres: f64,
}

/// Krüger series coefficients for one spheroid, plus the meridional arc to the
/// reference latitude. All of it depends only on `(a, b, lat_ref)`, so a warp
/// loop computes it once.
///
/// `pub` so projector helpers can hand them around; the fields are private.
#[derive(Debug, Clone, Copy)]
pub struct TransverseMercatorConstants {
    /// Third flattening, `n = f / (2 - f)`. Zero for a sphere, which zeroes
    /// every series coefficient below.
    n: f64,
    /// Rectifying radius `A`, the radius of the sphere with the same meridian
    /// arc length. Equals `a` when `n == 0`.
    rectifying_radius: f64,
    /// Forward (geodetic → projected) series.
    alpha: [f64; 5],
    /// Inverse (projected → conformal) series.
    beta: [f64; 5],
    /// Conformal → geodetic latitude series.
    delta: [f64; 5],
    /// ξ at the reference latitude on the central meridian — the arc §3.12's
    /// `YR` is measured from.
    xi_ref: f64,
}

impl TransverseMercatorConstants {
    /// Whether these constants describe a usable projection. A degenerate
    /// spheroid otherwise produces a projection that is arithmetically finite
    /// and geographically meaningless — `a = 1, b = -1` clamps to a one-metre
    /// sphere whose every point still lands inside a UK-sized grid — so this is
    /// the gate that turns such a message into "not reprojectable" rather than
    /// into a raster of garbage.
    fn well_defined(&self) -> bool {
        self.n.is_finite()
            && self.n.abs() < 1.0
            && self.rectifying_radius.is_finite()
            && self.rectifying_radius > 0.0
            && self.xi_ref.is_finite()
    }
}

fn transverse_mercator_constants(p: &TransverseMercatorParams) -> TransverseMercatorConstants {
    let a = p.semi_major_m;
    let b = p.semi_minor_m;
    // Reject a spheroid that is not one before deriving anything from it. The
    // arithmetic below is happy to carry on: `a = 1, b = -1` gives `f = 2`,
    // which the guard on `2 - f` turns into `n = 0`, i.e. a perfectly usable
    // one-metre sphere. Poisoning the rectifying radius makes
    // `well_defined` — and so the projector — reject it instead.
    if !(a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && b <= a) {
        return TransverseMercatorConstants {
            n: f64::NAN,
            rectifying_radius: f64::NAN,
            alpha: [f64::NAN; 5],
            beta: [f64::NAN; 5],
            delta: [f64::NAN; 5],
            xi_ref: f64::NAN,
        };
    }
    // A sphere and a spheroid go down the same path: `f` and therefore `n` are
    // zero, every coefficient below vanishes, and `A` collapses to `a`.
    let f = (a - b) / a;
    let n = if (2.0 - f) != 0.0 { f / (2.0 - f) } else { 0.0 };
    let (n2, n3, n4, n5) = (n * n, n * n * n, n.powi(4), n.powi(5));
    let rectifying_radius = a / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n.powi(6) / 256.0);
    // Krüger's series to sixth order. Within a few degrees of the central
    // meridian these are exact to well under a millimetre, which the tests
    // check against PROJ rather than assert from the literature.
    let alpha = [
        n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0,
        13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0,
        61.0 * n3 / 240.0 - 103.0 * n4 / 140.0 + 15061.0 * n5 / 26880.0,
        49561.0 * n4 / 161280.0 - 179.0 * n5 / 168.0,
        34729.0 * n5 / 80640.0,
    ];
    let beta = [
        n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0 - 81.0 * n5 / 512.0,
        n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0 + 46.0 * n5 / 105.0,
        17.0 * n3 / 480.0 - 37.0 * n4 / 840.0 - 209.0 * n5 / 4480.0,
        4397.0 * n4 / 161280.0 - 11.0 * n5 / 504.0,
        4583.0 * n5 / 161280.0,
    ];
    let delta = [
        2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3 + 116.0 * n4 / 45.0 + 26.0 * n5 / 45.0,
        7.0 * n2 / 3.0 - 8.0 * n3 / 5.0 - 227.0 * n4 / 45.0 + 2704.0 * n5 / 315.0,
        56.0 * n3 / 15.0 - 136.0 * n4 / 35.0 - 1262.0 * n5 / 105.0,
        4279.0 * n4 / 630.0 - 332.0 * n5 / 35.0,
        4174.0 * n5 / 315.0,
    ];
    // ξ of the reference latitude, on the central meridian where η' is zero and
    // every cosh term is 1.
    let xi_ref_prime = conformal_xi_eta(n, p.lat_ref * DEG2RAD, 0.0).0;
    let mut xi_ref = xi_ref_prime;
    for (j, a_j) in alpha.iter().enumerate() {
        xi_ref += a_j * (2.0 * (j as f64 + 1.0) * xi_ref_prime).sin();
    }
    TransverseMercatorConstants {
        n,
        rectifying_radius,
        alpha,
        beta,
        delta,
        xi_ref,
    }
}

/// `(ξ', η')` — the Gauss-Schreiber coordinates of a geodetic latitude and a
/// longitude offset from the central meridian, both in radians.
///
/// The isometric-latitude step uses `e = 2√n / (1 + n)`, which is the first
/// eccentricity exactly (`e² = 4n/(1+n)² = f(2-f)`), so no separate `e` needs
/// carrying. For a sphere `n` is zero and `t` reduces to `tan φ`.
fn conformal_xi_eta(n: f64, lat: f64, d_lon: f64) -> (f64, f64) {
    let sin_lat = lat.sin();
    let t = if n > 0.0 {
        let e = 2.0 * n.sqrt() / (1.0 + n);
        (sin_lat.atanh() - e * (e * sin_lat).atanh()).sinh()
    } else {
        lat.tan()
    };
    let xi = t.atan2(d_lon.cos());
    // `sin(dλ) / √(1 + t²)` is bounded by 1 in exact arithmetic, but round-off
    // at |dλ| = 90° can nudge it past — where `atanh` is infinite. Clamp just
    // inside instead of returning a coordinate that renders as a blank column.
    let eta = (d_lon.sin() / t.hypot(1.0)).clamp(-1.0 + f64::EPSILON, 1.0 - f64::EPSILON);
    (xi, eta.atanh())
}

/// Forward transverse Mercator: `(lat, lon)` in degrees → `(x, y)` in metres,
/// including the false easting and northing.
///
/// **Recomputes the series coefficients per call.** For warp loops use
/// [`TransverseMercatorProjector`], which caches them once.
pub fn transverse_mercator_forward(p: &TransverseMercatorParams, lat: f64, lon: f64) -> (f64, f64) {
    transverse_mercator_forward_with(&transverse_mercator_constants(p), p, lat, lon)
}

fn transverse_mercator_forward_with(
    k: &TransverseMercatorConstants,
    p: &TransverseMercatorParams,
    lat: f64,
    lon: f64,
) -> (f64, f64) {
    // Wrap into [-180, 180] before the trig: a query longitude carried in
    // [0, 360) against a central meridian in [-180, 180] otherwise lands on the
    // far side of the cylinder, the same trap `lambert_forward_with` documents.
    let d_lon = ((lon - p.lon_ref + 180.0).rem_euclid(360.0) - 180.0) * DEG2RAD;
    let (xi_p, eta_p) = conformal_xi_eta(k.n, lat * DEG2RAD, d_lon);
    let (mut xi, mut eta) = (xi_p, eta_p);
    for (j, a_j) in k.alpha.iter().enumerate() {
        let two_j = 2.0 * (j as f64 + 1.0);
        xi += a_j * (two_j * xi_p).sin() * (two_j * eta_p).cosh();
        eta += a_j * (two_j * xi_p).cos() * (two_j * eta_p).sinh();
    }
    let scale = p.scale_factor * k.rectifying_radius;
    (
        p.false_easting_m + scale * eta,
        p.false_northing_m + scale * (xi - k.xi_ref),
    )
}

/// Inverse transverse Mercator: `(x, y)` in metres → `(lat, lon)` in degrees.
/// Same recompute caveat as [`transverse_mercator_forward`].
pub fn transverse_mercator_inverse_xy(p: &TransverseMercatorParams, x: f64, y: f64) -> (f64, f64) {
    transverse_mercator_inverse_xy_with(&transverse_mercator_constants(p), p, x, y)
}

fn transverse_mercator_inverse_xy_with(
    k: &TransverseMercatorConstants,
    p: &TransverseMercatorParams,
    x: f64,
    y: f64,
) -> (f64, f64) {
    let scale = p.scale_factor * k.rectifying_radius;
    let xi = (y - p.false_northing_m) / scale + k.xi_ref;
    let eta = (x - p.false_easting_m) / scale;
    let (mut xi_p, mut eta_p) = (xi, eta);
    for (j, b_j) in k.beta.iter().enumerate() {
        let two_j = 2.0 * (j as f64 + 1.0);
        xi_p -= b_j * (two_j * xi).sin() * (two_j * eta).cosh();
        eta_p -= b_j * (two_j * xi).cos() * (two_j * eta).sinh();
    }
    // Conformal latitude, then the footpoint series back to geodetic.
    let chi = (xi_p.sin() / eta_p.cosh()).clamp(-1.0, 1.0).asin();
    let mut lat = chi;
    for (j, d_j) in k.delta.iter().enumerate() {
        lat += d_j * (2.0 * (j as f64 + 1.0) * chi).sin();
    }
    let lon = p.lon_ref + eta_p.sinh().atan2(xi_p.cos()) * RAD2DEG;
    (lat * RAD2DEG, lon)
}

/// Inverse warp: `(lat, lon)` → fractional source grid index. **Recomputes the
/// series per call** — for warp loops prefer [`TransverseMercatorProjector`].
pub fn transverse_mercator_inverse(
    p: &TransverseMercatorParams,
    lat: f64,
    lon: f64,
) -> Option<GridIndex> {
    TransverseMercatorProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a transverse Mercator grid. Owns the Krüger
/// coefficients and the reference arc, both invariant across a warp's output
/// pixels. Build once outside the per-pixel loop.
pub struct TransverseMercatorProjector {
    pub params: TransverseMercatorParams,
    constants: TransverseMercatorConstants,
}

impl TransverseMercatorProjector {
    pub fn new(params: TransverseMercatorParams) -> Self {
        let constants = transverse_mercator_constants(&params);
        Self { params, constants }
    }

    /// Project `(lat, lon)` back to the source-grid fractional index, or
    /// `None` when it falls outside the `ni × nj` extent. The shared planar
    /// body — kept as an inherent method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
    }

    /// Forward-project through the cached coefficients.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        transverse_mercator_forward_with(&self.constants, &self.params, lat, lon)
    }

    /// Inverse-project projected metres back to `(lat, lon)`.
    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        transverse_mercator_inverse_xy_with(&self.constants, &self.params, x, y)
    }

    /// Whether the projection is usable. `false` leaves
    /// [`inverse`](Self::inverse) returning `None` for every point, so callers
    /// can surface "not reprojectable" rather than render blank.
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined()
    }
}

impl PlanarGridProjector for TransverseMercatorProjector {
    fn grid_origin(&self) -> (f64, f64) {
        (self.params.x1_metres, self.params.y1_metres)
    }
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
    }
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        // The scale factor multiplies the rectifying radius: zero collapses
        // the whole plane onto the false origin, and a non-finite one makes
        // every index `NaN`.
        self.constants.well_defined()
            && self.params.scale_factor.is_finite()
            && self.params.scale_factor != 0.0
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

// ---------------------------------------------------------------------------
// GridGeometry — one typed value for "where does this grid sit on the Earth"
// ---------------------------------------------------------------------------

/// Where a grid sits on the Earth, as one value per grid family.
///
/// This is the lid on [`LatLonParams`] and friends. Before it, a consumer
/// carried every family's parameters side by side behind a `grid_type` string
/// and re-derived the dispatch itself, which is what
/// `fieldglass-napi`'s 51-field `MessageMeta` view still does; nothing stopped
/// a caller reading `latin1` off a Gaussian grid. A variant carries only the
/// parameters its own family defines, so that read does not compile.
///
/// Every method answers `None` rather than guessing when the family cannot be
/// placed, and [`GridGeometry::Unsupported`] carries the label it could not
/// handle instead of erroring — a host can then say *which* grid it declined.
///
/// The first cut covers what NOAA NODD and ECMWF publish. The other families
/// `core` projects (Mercator, rotated lat/lon, transverse Mercator, Lambert
/// azimuthal, geostationary) have projectors already and are additive here.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
#[serde(tag = "kind")]
pub enum GridGeometry {
    #[serde(rename = "latlon")]
    LatLon(LatLonParams),
    #[serde(rename = "gaussian")]
    Gaussian(GaussianParams),
    #[serde(rename = "lambert")]
    Lambert(LambertParams),
    #[serde(rename = "polar_stereo")]
    PolarStereo(PolarStereoParams),
    /// A grid that is a list of cell centres rather than a formula — NetCDF
    /// 2-D coordinates, GRIB2 §3.204, ICON §3.101. Answers with a cell, never
    /// a position inside one; see [`GridResampling::NearestOnly`].
    #[serde(rename = "lookup")]
    Lookup(crate::spatial_index::SpatialIndex),
    /// A family this type does not model yet. `label` is the grid type as the
    /// decoder named it, so the message can say what was declined.
    #[serde(rename = "unsupported")]
    Unsupported { label: String },
}

impl GridGeometry {
    /// The family tag, matching the `grid_type` strings the hosts already use
    /// (`"latlon"`, `"gaussian"`, `"lambert"`, `"polar_stereo"`), so #464 can
    /// swap the string dispatch for this type without renaming anything a
    /// consumer sees.
    ///
    /// This is exactly the serde tag — an unmodelled family reports
    /// `"unsupported"` here, not the grid it was, because a host deriving a DTO
    /// would otherwise read one string in JSON and another from this method.
    /// [`label`](Self::label) is the one to display.
    pub fn kind(&self) -> &str {
        match self {
            Self::LatLon(_) => "latlon",
            Self::Gaussian(_) => "gaussian",
            Self::Lambert(_) => "lambert",
            Self::PolarStereo(_) => "polar_stereo",
            Self::Lookup(_) => "lookup",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// The most specific name available: [`kind`](Self::kind) for a modelled
    /// family, and the decoder's own grid-type string for an unmodelled one.
    /// What a message saying which grid was declined should read.
    pub fn label(&self) -> &str {
        match self {
            Self::Unsupported { label } => label,
            other => other.kind(),
        }
    }

    /// `(ni, nj)` in grid points, or `None` for a family with no geometry.
    pub fn dims(&self) -> Option<(u32, u32)> {
        match self {
            Self::LatLon(p) => Some((p.ni, p.nj)),
            Self::Gaussian(p) => Some((p.ni, p.nj)),
            Self::Lambert(p) => Some((p.ni, p.nj)),
            Self::PolarStereo(p) => Some((p.ni, p.nj)),
            Self::Lookup(ix) => Some(ix.dims()),
            Self::Unsupported { .. } => None,
        }
    }

    /// Grid point `(i, j)` → `(lat, lon)` in degrees, or `None` when the index
    /// is off the grid or the family cannot be placed.
    pub fn forward(&self, i: u32, j: u32) -> Option<(f64, f64)> {
        let (ni, nj) = self.dims()?;
        if i >= ni || j >= nj {
            return None;
        }
        match self {
            Self::LatLon(p) => latlon_point(p, i, j),
            Self::Gaussian(p) => GaussianProjector::new(*p).grid_point_lonlat(i, j),
            Self::Lambert(p) => Some(LambertProjector::new(*p).grid_point_lonlat(i, j)),
            Self::PolarStereo(p) => Some(PolarStereoProjector::new(*p).grid_point_lonlat(i, j)),
            // Answered from the index's own centres (#445). The longitude
            // comes back normalised to [-180, 180]; see `SpatialIndex::centre`.
            Self::Lookup(ix) => ix.centre(i, j),
            Self::Unsupported { .. } => None,
        }
    }

    /// `(lat, lon)` in degrees → fractional grid index, or `None` when the
    /// point is outside the grid or the family cannot be placed. The direction
    /// a warp asks in.
    /// One-shot inverse. Builds the projection's constants and throws them
    /// away, so use [`inverse_at`](Self::inverse_at) for more than a handful of
    /// points; this routes through it so the two cannot answer differently.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        (self.inverse_at())(lat, lon)
    }

    /// The inverse map as a closure, with the projection's constants computed
    /// once instead of once per call.
    ///
    /// This is the seam a warp wants: it asks for an index per output pixel, and
    /// [`inverse`](Self::inverse) builds a projector — and with it the cone
    /// constant, its logarithms and the origin — on every one of them. A
    /// million-pixel raster pays that a million times. `SourceGrid::inverse_at`
    /// is already a closure for this reason; #464 wires this into it.
    ///
    /// An [`Unsupported`](Self::Unsupported) grid returns a closure that
    /// answers `None`, so a caller needs no second code path for it.
    pub fn inverse_at(&self) -> Box<dyn Fn(f64, f64) -> Option<GridIndex> + '_> {
        match self {
            Self::LatLon(p) => Box::new(move |lat, lon| latlon_inverse(p, lat, lon)),
            Self::Gaussian(p) => {
                // The Gaussian latitudes are an O(N²) Gauss-Legendre solve.
                // `gaussian_inverse` reaches them through a thread-local cache,
                // so the closure captures the params rather than a projector.
                Box::new(move |lat, lon| gaussian_inverse(p, lat, lon))
            }
            Self::Lambert(p) => {
                let proj = LambertProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::Lookup(ix) => Box::new(move |lat, lon| ix.nearest(lat, lon)),
            Self::Unsupported { .. } => Box::new(|_, _| None),
        }
    }

    /// What resampling this geometry supports. A lookup grid is
    /// [`GridResampling::NearestOnly`]; every formula grid is `Any`.
    pub fn resampling(&self) -> GridResampling {
        match self {
            Self::Lookup(ix) => ix.resampling(),
            _ => GridResampling::Any,
        }
    }

    /// The grid's geographic extent as `(lat_min, lat_max, lon_min, lon_max)`
    /// in degrees, or `None` for a family that cannot be placed.
    ///
    /// The projected families delegate to
    /// [`PlanarGridProjector::lonlat_bbox`], which subdivides each edge 512
    /// times rather than walking grid points: a conic's edges are curves and
    /// the extreme latitude sits between two points, not on one. It also skips
    /// perimeter samples that are not on the Earth at all, which an oversized
    /// §3.140 grid produces, and widens to the full 360° when the domain
    /// surrounds the projection pole and the enclosing longitude arc therefore
    /// degenerates.
    ///
    /// `lon_min` may fall below -180 (or `lon_max` above 180) to describe a
    /// window spanning the antimeridian — the existing convention, which the
    /// warp consumes through periodic trig. Do not normalise it into range
    /// without collapsing the span.
    pub fn lonlat_bbox(&self) -> Option<(f64, f64, f64, f64)> {
        match self {
            // The geographic families state their own corners, so the box is
            // the corners — unwrapped through `eastward_lon_span` so a grid
            // published from 180°E keeps its span instead of collapsing to one
            // cell.
            Self::LatLon(p) => Some(corner_bbox(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            )),
            Self::Gaussian(p) => Some(corner_bbox(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            )),
            // The extent of its own centres (#445), widened to the full circle
            // when they surround a pole and the enclosing arc stops meaning
            // anything — the same degeneracy `PolarStereo` handles below.
            Self::Lookup(ix) => ix.lonlat_bbox(),
            Self::Lambert(p) => Some(LambertProjector::new(*p).lonlat_bbox()),
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                let (lat_min, lat_max, lon_min, lon_max) = proj.lonlat_bbox();
                if proj.pole_inside_grid() {
                    // Every meridian is present and the enclosing arc has no
                    // empty gap to be the complement of, so the walk's
                    // longitudes mean nothing here.
                    let (lat_min, lat_max) = if p.south_pole {
                        (-90.0, lat_max)
                    } else {
                        (lat_min, 90.0)
                    };
                    Some((lat_min, lat_max, -180.0, 180.0))
                } else {
                    Some((lat_min, lat_max, lon_min, lon_max))
                }
            }
            Self::Unsupported { .. } => None,
        }
    }

    /// A PROJ definition string for the grid's coordinate reference system, or
    /// `None` for a family this type cannot place.
    ///
    /// What a browser map library wants: hand it this and the projected
    /// coordinates agree with [`forward`](Self::forward), which is what
    /// `tests/grid_geometry_proj.rs` checks against PROJ itself rather than by
    /// eye. The projected families emit absolute projection coordinates with no
    /// false easting or northing, because that is what `core`'s forward maps
    /// compute; the grid origin is applied on top, by
    /// [`inverse`](Self::inverse), not by the CRS.
    ///
    /// The Earth is the sphere the message declared (`+R=`), never a datum:
    /// a GRIB grid states a radius, and substituting WGS84 would move a
    /// continental grid by kilometres.
    pub fn proj4(&self) -> Option<String> {
        match self {
            // Geographic: the values `forward` returns are already lon/lat.
            // No grid carries a radius for these, so the CRS states the one
            // core defaults to; it does not affect an angular coordinate.
            Self::LatLon(_) | Self::Gaussian(_) => Some(format!(
                "+proj=longlat +R={DEFAULT_EARTH_RADIUS_M} +no_defs"
            )),
            // The centres are already geodetic, so the CRS is the sphere they
            // are stated on — the same answer the two geographic families give.
            Self::Lookup(_) => Some(format!(
                "+proj=longlat +R={DEFAULT_EARTH_RADIUS_M} +no_defs"
            )),
            Self::Lambert(p) => Some(format!(
                "+proj=lcc +lat_1={} +lat_2={} +lat_0={} +lon_0={} +R={} +units=m +no_defs",
                p.latin1, p.latin2, p.lad, p.lov, p.earth_radius_m
            )),
            Self::PolarStereo(p) => Some(format!(
                "+proj=stere +lat_0={} +lat_ts={} +lon_0={} +R={} +units=m +no_defs",
                if p.south_pole { -90.0 } else { 90.0 },
                if p.south_pole {
                    -p.lad.abs()
                } else {
                    p.lad.abs()
                },
                p.lov,
                p.earth_radius_m
            )),
            Self::Unsupported { .. } => None,
        }
    }
}

/// Box of a geographic grid from its two stated corners, in the same
/// `(lat_min, lat_max, lon_min, lon_max)` order the projected families use.
///
/// The longitude runs `lon_first` eastward by [`eastward_lon_span`] rather than
/// `min`/`max` of the two corners: a grid published from 180°E reports
/// `lon_last` numerically below `lon_first`, and taking the extremes collapses
/// the span to a single grid step. `lon_max` may therefore exceed 180, which is
/// the convention [`GridGeometry::lonlat_bbox`] documents.
fn corner_bbox(
    lat_first: f64,
    lon_first: f64,
    lat_last: f64,
    lon_last: f64,
) -> (f64, f64, f64, f64) {
    let span = eastward_lon_span(lon_first, lon_last);
    let lon_min = lon_first;
    (
        lat_first.min(lat_last),
        lat_first.max(lat_last),
        lon_min,
        lon_min + span,
    )
}

#[cfg(test)]
mod tests {
    // -----------------------------------------------------------------------
    // Lambert azimuthal equal-area (GRIB2 template 3.140)
    // -----------------------------------------------------------------------

    /// The §3.140 fixture's grid: ETRS89-LAEA's tangent point (52°N 10°E) on
    /// GRS80, 20 × 16 at 200 km from a first point at 35°N 10°W, scanning
    /// `+i, +j`. See `tools/build_grib2_lambert_azimuthal_fixture.py`.
    fn efas_params() -> LambertAzimuthalParams {
        LambertAzimuthalParams {
            semi_major_m: 6_378_137.0,
            semi_minor_m: 6_356_752.314,
            ni: 20,
            nj: 16,
            lat_first: 35.0,
            lon_first: -10.0,
            standard_parallel: 52.0,
            central_longitude: 10.0,
            dx_metres: 200_000.0,
            dy_metres: 200_000.0,
        }
    }

    /// `(i, j, lat, lon)` from eccodes 2.34.1's own
    /// `lambert_azimuthal_equal_area` iterator, read at full precision through
    /// the `latitudes`/`longitudes` keys.
    ///
    /// Unlike transverse Mercator, eccodes *does* geolocate this template — and
    /// it branches on `grib_is_earth_oblate`, running the authalic-latitude
    /// algorithm for an oblate shape. So the oracle is the house one, and it is
    /// ellipsoidal. PROJ 9.4.0 agrees with it to 0.0006 mm over all 320 grid
    /// points, which is what makes either of them trustworthy.
    const ECCODES_OBLATE: [(u32, u32, f64, f64); 6] = [
        (0, 0, 34.999999991, -10.000000000),
        (19, 0, 34.622366847, 31.637938749),
        (0, 15, 60.108219531, -24.240260690),
        (19, 15, 59.435027618, 46.673881242),
        (10, 8, 51.572695998, 12.561377772),
        (5, 3, 42.105393368, 0.059125317),
    ];

    /// The same grid on the WMO mean sphere, from PROJ. The spherical case is
    /// not a separate code path — it is the same authalic algorithm with the
    /// eccentricity at zero — so "it degenerates correctly" is checked, not
    /// argued.
    const PROJ_SPHERE_LAEA: [(u32, u32, f64, f64); 6] = [
        (0, 0, 35.000000000, -10.000000000),
        (19, 0, 34.604641267, 31.713042728),
        (0, 15, 60.098327642, -24.303142513),
        (19, 15, 59.390373763, 46.848782337),
        (10, 8, 51.561375604, 12.618538968),
        (5, 3, 42.098693508, 0.078012754),
    ];

    #[test]
    fn lambert_azimuthal_matches_eccodes_on_the_spheroid() {
        let projector = LambertAzimuthalProjector::new(efas_params());
        let mut worst: f64 = 0.0;
        for (i, j, lat, lon) in ECCODES_OBLATE {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        // The oracle is quoted to 1e-9°, about a tenth of a millimetre on the
        // ground, so a millimetre is the tightest bound it can support.
        assert!(
            worst < 1e-3,
            "worst deviation from eccodes was {worst} m, expected sub-millimetre"
        );
    }

    #[test]
    fn lambert_azimuthal_degenerates_to_the_sphere_when_the_axes_are_equal() {
        let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
            semi_major_m: DEFAULT_EARTH_RADIUS_M,
            semi_minor_m: DEFAULT_EARTH_RADIUS_M,
            ..efas_params()
        });
        let mut worst: f64 = 0.0;
        for (i, j, lat, lon) in PROJ_SPHERE_LAEA {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(
            worst < 1e-3,
            "spherical degenerate case deviated {worst} m from PROJ"
        );
    }

    /// The spheroid is load-bearing here too, and more so than for transverse
    /// Mercator: over the EFAS domain the mean-radius approximation is
    /// kilometres out, and it would also disagree with eccodes — which projects
    /// an oblate §3.140 on the true spheroid — so it could not pass the test
    /// above at any tolerance.
    #[test]
    fn the_spherical_approximation_would_be_kilometres_out_on_an_efas_grid() {
        let spheroid = efas_params();
        let mean_radius = (2.0 * spheroid.semi_major_m + spheroid.semi_minor_m) / 3.0;
        let sphere = LambertAzimuthalProjector::new(LambertAzimuthalParams {
            semi_major_m: mean_radius,
            semi_minor_m: mean_radius,
            ..spheroid
        });
        let exact = LambertAzimuthalProjector::new(spheroid);
        let mut worst: f64 = 0.0;
        for j in 0..spheroid.nj {
            for i in 0..spheroid.ni {
                let (a_lat, a_lon) = exact.grid_point_lonlat(i, j);
                let (b_lat, b_lon) = sphere.grid_point_lonlat(i, j);
                worst = worst.max(metres_apart(a_lat, a_lon, b_lat, b_lon));
            }
        }
        assert!(
            worst > 2_000.0,
            "expected the spherical approximation to be kilometres out, got {worst} m"
        );
    }

    #[test]
    fn lambert_azimuthal_round_trips_through_the_grid_index() {
        let p = efas_params();
        let projector = LambertAzimuthalProjector::new(p);
        for j in 0..p.nj {
            for i in 0..p.ni {
                let (lat, lon) = projector.grid_point_lonlat(i, j);
                let idx = projector
                    .inverse(lat, lon)
                    .unwrap_or_else(|| panic!("grid point ({i}, {j}) did not invert"));
                assert!(
                    (idx.i - i as f64).abs() < 1e-6 && (idx.j - j as f64).abs() < 1e-6,
                    "({i}, {j}) round-tripped to ({}, {})",
                    idx.i,
                    idx.j
                );
            }
        }
    }

    /// The first grid point lands back on the `La1`/`Lo1` the message states —
    /// to within the algorithm's own asymmetry, which is about a millimetre and
    /// is not ours to remove.
    ///
    /// `authalic_to_geodetic` is a three-term series, so it is not the exact
    /// inverse of the `authalic_q` that the forward map used: 35°N comes back
    /// as 34.999999991°, a centimetre-scale wobble that PROJ and eccodes share
    /// exactly. eccodes reports *the same* 34.999999991 for this fixture's
    /// first point, which is the real check — landing on the declared value
    /// more precisely than eccodes does would mean a different algorithm, not a
    /// better one. A sign or wrap error in the forward map still shows up here
    /// first, at degrees rather than nanodegrees.
    #[test]
    fn the_lambert_azimuthal_origin_is_the_declared_first_grid_point() {
        let p = efas_params();
        let projector = LambertAzimuthalProjector::new(p);
        let (lat, lon) = projector.grid_point_lonlat(0, 0);
        assert!(
            metres_apart(p.lat_first, p.lon_first, lat, lon) < 5e-3,
            "origin came back as ({lat}, {lon})"
        );
        // And it agrees with eccodes' own answer far more tightly than it
        // agrees with the declared value.
        assert!(
            metres_apart(34.999999991, -10.0, lat, lon) < 1e-4,
            "origin ({lat}, {lon}) disagrees with eccodes"
        );
    }

    /// The polar aspect, where the oblateness stretch is undefined — and where
    /// eccodes gets the *south* pole wrong.
    ///
    /// Its guard is `cosb1 == 0.0`, which holds at +90° by luck and fails at
    /// −90° because `authalic_q(-1)` is not the exact negation of the `qp` it is
    /// divided by. Asked for the latitudes of a south-polar §3.140 message,
    /// eccodes 2.48 answers
    /// `Invalid value: arcsin argument=7.60531e+06` and refuses to build the
    /// iterator at all — the projected plane has inflated by eight orders of
    /// magnitude. So this case has no eccodes oracle, and PROJ supplies it.
    #[test]
    fn lambert_azimuthal_handles_the_north_polar_aspect() {
        let p = LambertAzimuthalParams {
            standard_parallel: 90.0,
            central_longitude: 0.0,
            lat_first: 60.0,
            lon_first: 0.0,
            ni: 8,
            nj: 6,
            ..efas_params()
        };
        let projector = LambertAzimuthalProjector::new(p);
        assert!(projector.is_well_defined());
        // eccodes 2.34.1 geolocates this one; these are its answers.
        for (i, j, lat, lon) in [
            (0u32, 0u32, 60.0_f64, 0.0_f64),
            (1, 0, 59.9439, 3.4580),
            (2, 0, 59.7762, 6.8909),
        ] {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            assert!(
                metres_apart(lat, lon, got_lat, got_lon) < 20.0,
                "({i}, {j}) gave ({got_lat}, {got_lon}), eccodes says ({lat}, {lon})"
            );
        }
    }

    /// The south-polar aspect, against PROJ — the case eccodes cannot answer.
    #[test]
    fn lambert_azimuthal_handles_the_south_polar_aspect() {
        let p = LambertAzimuthalParams {
            standard_parallel: -90.0,
            central_longitude: 0.0,
            lat_first: -60.0,
            lon_first: 0.0,
            ni: 8,
            nj: 6,
            ..efas_params()
        };
        let projector = LambertAzimuthalProjector::new(p);
        assert!(
            projector.is_well_defined(),
            "the south-polar aspect is unusable"
        );
        let mut worst: f64 = 0.0;
        for (i, j, lat, lon) in [
            (0u32, 0u32, -59.999999998, 0.000000000),
            (7, 0, -57.352900900, 22.927574019),
            (0, 5, -50.586924754, 0.000000000),
            (7, 5, -48.462661034, 17.995848243),
            (4, 3, -53.617984807, 11.563846694),
        ] {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
            let idx = projector.inverse(got_lat, got_lon).expect("inverts");
            assert!((idx.i - i as f64).abs() < 1e-6 && (idx.j - j as f64).abs() < 1e-6);
        }
        assert!(worst < 1e-3, "worst deviation from PROJ was {worst} m");
    }

    /// The tangent point itself is where `rho` goes to zero. eccodes asserts on
    /// that case and aborts the process; answering with the tangent point is
    /// both correct and survivable.
    #[test]
    fn lambert_azimuthal_inverts_its_own_tangent_point() {
        let p = efas_params();
        let (lat, lon) = lambert_azimuthal_inverse_xy(&p, 0.0, 0.0);
        assert!(
            (lat - p.standard_parallel).abs() < 1e-9 && (lon - p.central_longitude).abs() < 1e-9,
            "the tangent point inverted to ({lat}, {lon})"
        );
    }

    /// The projection maps the globe onto a disc whose edge is the antipode of
    /// the tangent point. Beyond it there is no point on Earth, and past the
    /// antipode the forward map's scale is infinite — both must decline rather
    /// than fold back onto a plausible index.
    #[test]
    fn lambert_azimuthal_declines_off_the_projection_disc() {
        let p = efas_params();
        // The antipode of 52°N 10°E.
        let (x, y) = lambert_azimuthal_forward(&p, -52.0, -170.0);
        assert!(
            !x.is_finite() || !y.is_finite(),
            "the antipode projected to a finite ({x}, {y})"
        );
        // Well outside the disc: 4 Earth radii from the tangent point.
        let (lat, lon) = lambert_azimuthal_inverse_xy(&p, 4.0 * p.semi_major_m, 0.0);
        assert!(lat.is_nan() && lon.is_nan(), "got ({lat}, {lon})");
        // And the grid inverse declines for a point nowhere near the grid.
        let projector = LambertAzimuthalProjector::new(p);
        for (lat, lon) in [(-52.0, -170.0), (-80.0, 100.0), (5.0, -150.0)] {
            assert!(
                projector.inverse(lat, lon).is_none(),
                "({lat}, {lon}) resolved to a grid index"
            );
        }
    }

    #[test]
    fn lambert_azimuthal_rejects_a_degenerate_spheroid() {
        for (major, minor) in [
            (0.0, 0.0),
            (1.0, -1.0),
            (-6_371_229.0, -6_371_229.0),
            (f64::NAN, 6_371_229.0),
            (6_356_752.0, 6_378_137.0),
        ] {
            let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
                semi_major_m: major,
                semi_minor_m: minor,
                ..efas_params()
            });
            assert!(!projector.is_well_defined(), "({major}, {minor}) is usable");
            assert!(
                projector.inverse(52.0, 10.0).is_none(),
                "({major}, {minor}) resolved a grid index"
            );
        }
    }

    /// A longitude carried in [0, 360) has to land in the same place as its
    /// signed twin.
    #[test]
    fn lambert_azimuthal_wraps_longitude_before_projecting() {
        let p = efas_params();
        for (lat, lon) in [(35.0, -10.0), (52.0, 10.0), (60.1, -24.2)] {
            let signed = lambert_azimuthal_forward(&p, lat, lon);
            let unsigned = lambert_azimuthal_forward(&p, lat, lon + 360.0);
            assert!(
                (signed.0 - unsigned.0).abs() < 1e-6 && (signed.1 - unsigned.1).abs() < 1e-6,
                "{lon} and {} projected differently",
                lon + 360.0
            );
        }
    }

    /// A grid that runs off the projection disc must still produce a finite
    /// bounding box around the part of it that exists.
    ///
    /// Found by pushing the §3.140 extent past the antipode: 1 369 of 1 600
    /// points stop being points on Earth, every perimeter sample but the first
    /// corner comes back `NaN`, and the longitude bound came out `NaN` — which
    /// the warp consumes as a target extent and renders as nothing. The shared
    /// perimeter walk now skips non-projectable samples, which also closes the
    /// same hole for Lambert, whose forward map is documented to return `±inf`
    /// at the projection pole.
    #[test]
    fn a_bbox_over_a_partly_unprojectable_grid_stays_finite() {
        let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
            ni: 40,
            nj: 40,
            dx_metres: 900_000.0,
            dy_metres: 900_000.0,
            ..efas_params()
        });
        let (lat_min, lat_max, lon_min, lon_max) = projector.lonlat_bbox();
        assert!(
            lat_min.is_finite() && lat_max.is_finite(),
            "latitude bound was ({lat_min}, {lat_max})"
        );
        assert!(
            lon_min.is_finite() && lon_max.is_finite(),
            "longitude bound was ({lon_min}, {lon_max})"
        );
        assert!(lat_min < lat_max, "empty latitude span");
    }

    // -----------------------------------------------------------------------
    // Transverse Mercator (GRIB2 template 3.12)
    // -----------------------------------------------------------------------

    /// The §3.12 fixture's grid, field for field: British National Grid
    /// parameters on Airy 1830, `shapeOfTheEarth = 3` having rounded the axes
    /// to the metre and the IEEE-32 scale factor having rounded `0.9996012717`
    /// to `0.99960124`. Same extent as the real UKV domain, at 48 km instead of
    /// 2 km — see `tools/build_grib2_transverse_mercator_fixture.py`.
    fn ukv_params() -> TransverseMercatorParams {
        TransverseMercatorParams {
            semi_major_m: 6_377_563.0,
            semi_minor_m: 6_356_257.0,
            ni: 24,
            nj: 30,
            lat_ref: 49.0,
            lon_ref: -2.0,
            scale_factor: 0.999_601_244_926_452_6,
            false_easting_m: 400_000.0,
            false_northing_m: -100_000.0,
            x1_metres: -238_000.0,
            y1_metres: 1_222_000.0,
            dx_metres: 48_000.0,
            dy_metres: -48_000.0,
        }
    }

    /// `(x, y, lat, lon)` from PROJ 9.4.0, `cs2cs` from the tmerc definition
    /// above to `+proj=longlat` on the *same* spheroid — no datum shift, so any
    /// disagreement is the projection maths and nothing else.
    ///
    /// PROJ rather than eccodes because eccodes has no transverse-Mercator
    /// geoiterator at all: `codes_grib_iterator_new` on a §3.12 message answers
    /// "Function not yet implemented", at the pinned 2.34.1 and still at 2.48.
    /// There is no version of the usual oracle to fall back to, so the check is
    /// against the reference implementation of the projection itself.
    const PROJ_ELLIPSOID: [(f64, f64, f64, f64); 11] = [
        (-238000.0, 1222000.0, 60.374180125, -13.611297212),
        (-216000.0, 1222000.0, 60.408174975, -13.220013439),
        (-238000.0, 1194000.0, 60.127855836, -13.523227104),
        (-216000.0, 1194000.0, 60.161514449, -13.134756726),
        (-228000.0, 1208000.0, 60.266544782, -13.389971606),
        (400000.0, -100000.0, 49.000000000, -2.000000000),
        (0.0, 0.0, 49.766185845, -7.556449264),
        (856000.0, 1222000.0, 60.620756496, 6.348094228),
        (-238000.0, -184000.0, 47.925395454, -10.544600945),
        (1200000.0, 600000.0, 54.653549529, 10.433629205),
        (-600000.0, 600000.0, 54.301200951, -17.429687151),
    ];

    /// The same points with `+R=6371229`, the WMO mean sphere. Pinned because
    /// the spherical case is not a separate code path here — it is the same
    /// Krüger series with `n = 0` — and "it degenerates correctly" is a claim
    /// that has to be checked rather than argued.
    const PROJ_SPHERE: [(f64, f64, f64, f64); 11] = [
        (-238000.0, 1222000.0, 60.383180300, -13.655835175),
        (-216000.0, 1222000.0, 60.417371041, -13.263119140),
        (-238000.0, 1194000.0, 60.136433927, -13.567091090),
        (-216000.0, 1194000.0, 60.170284169, -13.177208281),
        (-228000.0, 1208000.0, 60.275421703, -13.433525242),
        (400000.0, -100000.0, 49.000000000, -2.000000000),
        (0.0, 0.0, 49.765852937, -7.572796392),
        (856000.0, 1222000.0, 60.631190800, 6.380489047),
        (-238000.0, -184000.0, 47.924585237, -10.568782438),
        (1200000.0, 600000.0, 54.654239552, 10.474202872),
        (-600000.0, 600000.0, 54.300446549, -17.479364963),
    ];

    /// Metres of great-circle error between two `(lat, lon)` pairs, close
    /// enough for a comparison at this scale.
    fn metres_apart(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
        const M_PER_DEG: f64 = 111_320.0;
        let d_lat = (lat_a - lat_b) * M_PER_DEG;
        let d_lon = (lon_a - lon_b) * M_PER_DEG * (lat_a * DEG2RAD).cos();
        d_lat.hypot(d_lon)
    }

    #[test]
    fn transverse_mercator_inverse_matches_proj_on_the_spheroid() {
        let p = ukv_params();
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in PROJ_ELLIPSOID {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        // A millimetre, three orders of magnitude below the 2 km grid spacing
        // and far below the accuracy the oracle itself is quoted to.
        assert!(
            worst < 1e-3,
            "worst deviation from PROJ was {worst} m, expected sub-millimetre"
        );
    }

    #[test]
    fn transverse_mercator_forward_matches_proj_on_the_spheroid() {
        let p = ukv_params();
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in PROJ_ELLIPSOID {
            let (got_x, got_y) = transverse_mercator_forward(&p, lat, lon);
            worst = worst.max((got_x - x).hypot(got_y - y));
        }
        // Looser than the inverse only because the oracle latitudes are quoted
        // to 1e-9°, which is itself about a tenth of a millimetre on the ground.
        assert!(
            worst < 1e-2,
            "worst forward deviation from PROJ was {worst} m"
        );
    }

    /// `a == b` must reduce to spherical transverse Mercator exactly, because
    /// that is the only spherical implementation there is.
    #[test]
    fn transverse_mercator_degenerates_to_the_sphere_when_the_axes_are_equal() {
        let p = TransverseMercatorParams {
            semi_major_m: DEFAULT_EARTH_RADIUS_M,
            semi_minor_m: DEFAULT_EARTH_RADIUS_M,
            ..ukv_params()
        };
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in PROJ_SPHERE {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(
            worst < 1e-3,
            "spherical degenerate case deviated {worst} m from PROJ"
        );
    }

    /// The spheroid is not a detail that can be dropped for this template. A
    /// UKV grid projected on the mean radius instead of Airy 1830 lands about
    /// 2.8 km away — nearly two grid cells at 1.5 km, and a visible coastline
    /// offset. This is what justifies carrying two axes through the seam
    /// rather than one mean radius, so it is asserted rather than commented.
    #[test]
    fn the_spherical_approximation_would_be_kilometres_out_on_a_ukv_grid() {
        let spheroid = ukv_params();
        let mean_radius = (2.0 * spheroid.semi_major_m + spheroid.semi_minor_m) / 3.0;
        let sphere = TransverseMercatorParams {
            semi_major_m: mean_radius,
            semi_minor_m: mean_radius,
            ..spheroid
        };
        let mut worst: f64 = 0.0;
        for (x, y, _, _) in PROJ_ELLIPSOID {
            let (a_lat, a_lon) = transverse_mercator_inverse_xy(&spheroid, x, y);
            let (b_lat, b_lon) = transverse_mercator_inverse_xy(&sphere, x, y);
            worst = worst.max(metres_apart(a_lat, a_lon, b_lat, b_lon));
        }
        assert!(
            (2_000.0..4_000.0).contains(&worst),
            "expected the spherical approximation to be a few km out, got {worst} m"
        );
    }

    /// The Krüger series is not UKV-specific, and neither is the template: a
    /// §3.12 message can carry any UTM zone. `lat_ref = 0` in particular takes
    /// a different path through the reference-arc term, where `xi_ref` is zero
    /// and a sign error in it would be invisible against a UKV grid.
    #[test]
    fn transverse_mercator_matches_proj_for_a_utm_zone() {
        // UTM zone 31N on WGS84.
        let p = TransverseMercatorParams {
            semi_major_m: 6_378_137.0,
            semi_minor_m: 6_356_752.314_245,
            lat_ref: 0.0,
            lon_ref: 3.0,
            scale_factor: 0.9996,
            false_easting_m: 500_000.0,
            false_northing_m: 0.0,
            ..ukv_params()
        };
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in [
            (500000.0, 0.0, 0.000000000, 3.000000000),
            (300000.0, 5000000.0, 45.125153848, 0.456876510),
            (700000.0, 5000000.0, 45.125153848, 5.543123490),
            (500000.0, 9000000.0, 81.060880975, 3.000000000),
            (166000.0, 1000000.0, 9.033978846, -0.037660965),
            (834000.0, 1000000.0, 9.033978846, 6.037660965),
        ] {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(worst < 1e-3, "worst deviation from PROJ was {worst} m");
    }

    /// A southern-hemisphere zone, where the reference latitude is negative and
    /// the false northing is the 5 000 km offset the southern UTM convention
    /// uses. Both signs would cancel in a north-only test.
    #[test]
    fn transverse_mercator_matches_proj_south_of_the_equator() {
        let p = TransverseMercatorParams {
            semi_major_m: 6_378_137.0,
            semi_minor_m: 6_356_752.314_245,
            lat_ref: -33.0,
            lon_ref: 151.0,
            scale_factor: 0.99994,
            false_easting_m: 300_000.0,
            false_northing_m: 5_000_000.0,
            ..ukv_params()
        };
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in [
            (300000.0, 5000000.0, -33.000000000, 151.000000000),
            (100000.0, 4800000.0, -34.783580919, 148.814928632),
            (500000.0, 5200000.0, -31.179175500, 153.097987240),
            (300000.0, 5600000.0, -27.587337109, 151.000000000),
        ] {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(worst < 1e-3, "worst deviation from PROJ was {worst} m");
    }

    /// The grid walk has to follow the scanning mode, and the sign of the
    /// increments is the only thing that carries it. A `j`-positive grid starts
    /// at the same `X1`/`Y1` and walks *up*; getting that backwards renders the
    /// field upside down, which no value check would catch.
    #[test]
    fn transverse_mercator_walks_the_grid_in_the_declared_scan_direction() {
        for (dx, dy) in [
            (48_000.0, -48_000.0),  // default scan: +i, -j
            (48_000.0, 48_000.0),   // j scans positively
            (-48_000.0, -48_000.0), // i scans negatively
            (-48_000.0, 48_000.0),
        ] {
            let p = TransverseMercatorParams {
                dx_metres: dx,
                dy_metres: dy,
                ..ukv_params()
            };
            let projector = TransverseMercatorProjector::new(p);
            // The origin is `X1`/`Y1` whichever way the grid scans.
            let (ox, oy) = projector.grid_origin();
            assert!((ox - p.x1_metres).abs() < 1e-9 && (oy - p.y1_metres).abs() < 1e-9);
            // And the last scanned point is the origin plus the full signed
            // extent — the corner a §3.12 message states as `X2`/`Y2`.
            let (lx, ly) = projector.grid_corners_xy()[3];
            assert!(
                (lx - (p.x1_metres + (p.ni as f64 - 1.0) * dx)).abs() < 1e-9
                    && (ly - (p.y1_metres + (p.nj as f64 - 1.0) * dy)).abs() < 1e-9,
                "({dx}, {dy}) walked to ({lx}, {ly})"
            );
            // Every point still inverts to its own index, in every direction.
            for (i, j) in [(0, 0), (p.ni - 1, 0), (0, p.nj - 1), (p.ni - 1, p.nj - 1)] {
                let (lat, lon) = projector.grid_point_lonlat(i, j);
                let idx = projector
                    .inverse(lat, lon)
                    .unwrap_or_else(|| panic!("({dx}, {dy}) lost corner ({i}, {j})"));
                assert!((idx.i - i as f64).abs() < 1e-6 && (idx.j - j as f64).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn transverse_mercator_round_trips_through_the_grid_index() {
        let p = ukv_params();
        let projector = TransverseMercatorProjector::new(p);
        for j in 0..p.nj {
            for i in 0..p.ni {
                let (lat, lon) = projector.grid_point_lonlat(i, j);
                let idx = projector
                    .inverse(lat, lon)
                    .unwrap_or_else(|| panic!("grid point ({i}, {j}) did not invert"));
                assert!(
                    (idx.i - i as f64).abs() < 1e-6 && (idx.j - j as f64).abs() < 1e-6,
                    "({i}, {j}) round-tripped to ({}, {})",
                    idx.i,
                    idx.j
                );
            }
        }
    }

    /// A point outside the grid must not be silently clamped onto its edge —
    /// that is how a warp paints a border of smeared data instead of background.
    #[test]
    fn transverse_mercator_rejects_points_outside_the_grid() {
        let projector = TransverseMercatorProjector::new(ukv_params());
        // The grid covers the UK and its shelf seas; these are all well
        // outside it, including two on the central meridian so the rejection
        // is about extent rather than an easy longitude miss.
        for (lat, lon) in [(0.0, 0.0), (30.0, -2.0), (-45.0, 170.0), (89.0, -2.0)] {
            assert!(
                projector.inverse(lat, lon).is_none(),
                "({lat}, {lon}) resolved to a grid index"
            );
        }
    }

    /// A longitude carried in [0, 360) has to land in the same place as its
    /// signed twin — the wrap trap `lambert_forward_with` documents, which bit
    /// the Eta Lambert grid.
    #[test]
    fn transverse_mercator_wraps_longitude_before_projecting() {
        let p = ukv_params();
        for (lat, lon) in [(60.3, -13.5), (49.0, -2.0), (60.2, -13.2)] {
            let signed = transverse_mercator_forward(&p, lat, lon);
            let unsigned = transverse_mercator_forward(&p, lat, lon + 360.0);
            assert!(
                (signed.0 - unsigned.0).abs() < 1e-6 && (signed.1 - unsigned.1).abs() < 1e-6,
                "{lon} and {} projected differently",
                lon + 360.0
            );
        }
    }

    /// Degenerate parameters must disable the projector rather than produce a
    /// plausible-looking index.
    ///
    /// `(1.0, -1.0)` is the case worth keeping: the flattening it implies is 2,
    /// the `2 - f` guard turns that into `n = 0`, and what comes out is a
    /// working projection on a one-metre sphere — where every point on Earth
    /// maps to within a metre of the false origin and therefore *inside* a
    /// UK-sized grid. It reports an index for every query, all of them wrong.
    /// Nothing downstream would notice.
    #[test]
    fn transverse_mercator_rejects_a_degenerate_spheroid() {
        for (major, minor) in [
            (0.0, 0.0),
            (1.0, -1.0),
            (-6_371_229.0, -6_371_229.0),
            (f64::NAN, 6_371_229.0),
            (6_371_229.0, f64::INFINITY),
            // Prolate: a spheroid squashed the other way is not something WMO's
            // shape table can describe, so it is corrupt rather than exotic.
            (6_356_257.0, 6_377_563.0),
        ] {
            let projector = TransverseMercatorProjector::new(TransverseMercatorParams {
                semi_major_m: major,
                semi_minor_m: minor,
                ..ukv_params()
            });
            assert!(!projector.is_well_defined(), "({major}, {minor}) is usable");
            assert!(
                projector.inverse(54.0, -2.0).is_none(),
                "({major}, {minor}) resolved a grid index"
            );
        }
    }

    /// A scale factor of zero collapses the plane onto the false origin; a
    /// non-finite one makes every index `NaN`. Both must decline rather than
    /// answer.
    #[test]
    fn transverse_mercator_rejects_a_degenerate_scale_factor() {
        for scale_factor in [0.0, f64::NAN, f64::INFINITY] {
            let projector = TransverseMercatorProjector::new(TransverseMercatorParams {
                scale_factor,
                ..ukv_params()
            });
            assert!(
                projector.inverse(54.0, -2.0).is_none(),
                "scale factor {scale_factor} resolved a grid index"
            );
        }
    }

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
            fn forward_xy(&self, _lat: f64, lon: f64) -> (f64, f64) {
                (lon, 0.0)
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
        let out = expand_reduced_to_regular(&vals(&[10.0, 20.0, 30.0, 40.0]), &[4], 4);
        assert_eq!(out, vals(&[10.0, 20.0, 30.0, 40.0]));
    }

    #[test]
    fn narrow_row_maps_by_longitude_and_wraps_at_antimeridian() {
        // Row of 4 points (a,b,c,d at 0°, 90°, 180°, 270°) widened to 8 columns
        // (0°, 45°, …, 315°). Each output column takes its nearest-longitude
        // source point, and the last column (315°) wraps to a (at 360°≡0°) —
        // not to d, which a proportional-index stretch would wrongly pick.
        let out = expand_reduced_to_regular(&vals(&[1.0, 2.0, 3.0, 4.0]), &[4], 8);
        assert_eq!(out, vals(&[1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 1.0]));
    }

    #[test]
    fn two_point_row_wraps() {
        // [a,b] at 0°/180° → 4 columns at 0°/90°/180°/270°: a, b, b, a (the 90°
        // and 270° ties round up / wrap).
        let out = expand_reduced_to_regular(&vals(&[1.0, 2.0]), &[2], 4);
        assert_eq!(out, vals(&[1.0, 2.0, 2.0, 1.0]));
    }

    #[test]
    fn single_point_row_fills_width() {
        // A one-point polar row spreads across the whole width.
        let out = expand_reduced_to_regular(&vals(&[7.0]), &[1], 3);
        assert_eq!(out, vals(&[7.0, 7.0, 7.0]));
    }

    #[test]
    fn masked_points_are_preserved() {
        let row = vec![Some(1.0), None, Some(3.0)];
        let out = expand_reduced_to_regular(&row, &[3], 3);
        assert_eq!(out, vec![Some(1.0), None, Some(3.0)]);
    }

    #[test]
    fn multiple_rows_are_widened_independently() {
        // Row 0: 2 points widened to 4; row 1: already 4 wide.
        let raw = vals(&[1.0, 2.0, 10.0, 20.0, 30.0, 40.0]);
        let out = expand_reduced_to_regular(&raw, &[2, 4], 4);
        assert_eq!(out.len(), 8);
        assert_eq!(&out[0..4], &vals(&[1.0, 2.0, 2.0, 1.0])[..]);
        assert_eq!(&out[4..8], &vals(&[10.0, 20.0, 30.0, 40.0])[..]);
    }

    #[test]
    fn a_short_values_slice_pads_the_missing_rows() {
        // A truncated field must not index out of bounds: the rows it does not
        // reach come back masked rather than panicking.
        let out = expand_reduced_to_regular(&vals(&[1.0, 2.0]), &[2, 4], 4);
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
