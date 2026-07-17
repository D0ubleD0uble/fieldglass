//! Inverse-warp pipeline: paint a source GRIB grid into a target
//! projection raster.
//!
//! The caller provides a [`SourceGrid`] (raw values + a per-grid-type
//! `inverse_at` closure from [`crate::projection`]) and a [`TargetRaster`]
//! describing the output's lat/lon bounds and pixel dimensions. We walk
//! the output pixel grid, ask the source where each `(lat, lon)` lives,
//! and sample.
//!
//! Each target projection implements [`TargetProjection`]: the one thing
//! that varies between targets is how an output pixel maps back to the
//! `(lat, lon)` we hand to the source inverse map. [`warp`] is generic
//! over that trait; [`warp_to_equirectangular`] is the original
//! lat/lon-box entry retained as a thin wrapper. Web Mercator / ortho /
//! polar-stereo targets (#71) add their own [`TargetProjection`] impls.

use crate::projection::GridIndex;
use std::f64::consts::PI;

const DEG2RAD: f64 = PI / 180.0;
const RAD2DEG: f64 = 180.0 / PI;

/// Shift a centre-relative angle by whole turns of `2 * half_turn` until it lies
/// in `[-half_turn, half_turn]` — the span a map draws about its centre
/// meridian, with `±half_turn` the seam on its left and right edge. Pass `PI`
/// for radians (the world targets) or `180.0` for degrees ([`lon_to_px`]).
///
/// A value landing *exactly* on the seam keeps its sign, so it stays on the edge
/// it names. That tie-break is the whole point. The seam is double-valued — both
/// edges are the same meridian — and the value's own sign is the only
/// information available to place it. It is also what the data means: Natural
/// Earth clips a ring at the antimeridian and signs each piece on the side it
/// lies (the piece running to −180° is signed −180°, the piece running to +180°
/// is signed +180°), so preserving the sign lands each piece on its own edge.
///
/// Rounding to the *nearest* whole turn does not: `f64::round` breaks a tie away
/// from zero, so a value exactly on the seam was shifted a full turn onto the
/// opposite edge, and a ring touching it was drawn as a streak clear across the
/// map (or lost to the overlay's seam split). Ties here break *toward* zero
/// instead, at every half-integer multiple — `±half_turn` and `±3·half_turn`
/// alike, since a centre meridian of ±360° is as legal an input as 0°.
///
/// This relies on a seam value being *bit-exactly* `±half_turn` after the
/// caller's own arithmetic — `180.0 * DEG2RAD == PI` exactly in `f64`, which
/// `wrap_seam_tie_is_bit_exact` pins. A value one ULP off the seam is not on the
/// seam and correctly takes the shift.
///
/// A non-finite input passes through as non-finite, which callers already handle.
fn wrap_to_seam_span(value: f64, half_turn: f64) -> f64 {
    let turn = 2.0 * half_turn;
    let k = value / turn;
    // |k| ≤ 0.5 → n = 0 (already in span, seam included). Otherwise the nearest
    // whole turn, with a half-integer |k| resolving to the smaller |n|.
    let n = (k.abs() - 0.5).ceil().copysign(k);
    value - turn * n
}

/// The latitude beyond which Web Mercator's `ln(tan(...))` diverges. The
/// de-facto web-map clamp (Snyder; OSM/Google tile convention) yields a
/// square world at `±85.0511…°` — `2·atan(eⁿ) - 90°` for one full turn
/// of `x`. Targets clamp their `lat` extent to this band.
const WEB_MERCATOR_MAX_LAT: f64 = 85.051_128_779_806_59;

/// Stereographic radius of the equator on the unit sphere: `ρ =
/// 2·tan(π/4 - φ/2)` gives `ρ = 2` at `φ = 0`. The polar-stereographic
/// target maps its disc rim to this radius so the equator lands on the
/// rim, and inverts the same constant to recover latitude — the two uses
/// are the same number and must move together.
const POLAR_STEREO_EQUATOR_RHO: f64 = 2.0;

/// Resampling method when warping into the output raster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resampling {
    Nearest,
    Bilinear,
}

/// A source-grid sample at integer cell `(i, j)`. `None` means the cell
/// is bitmap-masked at the source and should not contribute to the warp.
type Sample<'a> = &'a (dyn Fn(usize, usize) -> Option<f64> + 'a);

/// Inverse-map callback supplied by the per-grid-type projection helper.
/// Returns the fractional source-grid coordinate corresponding to the
/// requested `(lat, lon)`, or `None` when off-grid.
type Inverse<'a> = &'a (dyn Fn(f64, f64) -> Option<GridIndex> + 'a);

/// Source grid descriptor for a warp call. `sample` reads from whatever
/// underlying storage the caller has (Float64 slice, `Vec<Option<f64>>`,
/// bitmap-masked typed array — anything); `inverse_at` is the projection
/// helper from [`crate::projection`].
pub struct SourceGrid<'a> {
    pub ni: u32,
    pub nj: u32,
    pub sample: Sample<'a>,
    pub inverse_at: Inverse<'a>,
    /// `true` when the grid is periodic in `i` — a global west-to-east grid
    /// whose next column past `ni - 1` is column 0 again (see
    /// [`crate::projection::lon_grid_is_global`]). The resampler then wraps
    /// column indices instead of clamping, so the seam gap between the last
    /// and first columns interpolates across the wrap rather than rendering
    /// as a one-pixel hole at the seam meridian.
    pub periodic_i: bool,
}

/// Target raster definition for the warp. `lat_max` sits at output pixel
/// row 0 (north-up convention).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetRaster {
    pub width: u32,
    pub height: u32,
    pub lat_max: f64,
    pub lat_min: f64,
    pub lon_min: f64,
    pub lon_max: f64,
}

/// Output of a warp call. `values` holds the resampled source values
/// row-major (length `width * height`); `mask` carries the per-pixel
/// presence flag (1 = present, 0 = absent / off-grid / masked).
pub struct WarpedRaster {
    pub values: Vec<f64>,
    pub mask: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// A render target: the geographic `(lat, lon)` that each output pixel
/// represents. This is the *inverse* of the target projection's forward
/// map — the warp walks pixels and asks where each one lives so it can
/// sample the source there.
///
/// `pixel_to_lonlat` returns `None` for pixels outside the projection's
/// valid domain (e.g. the back hemisphere of an orthographic globe, or
/// the corners of a polar-stereographic disc), which the warp leaves
/// masked. Row `0` is the north / top edge by convention.
///
/// **Inverse-only, by design.** The warp is the only consumer today and it
/// only ever needs pixel → `(lat, lon)`, so the forward map `(lat, lon)` →
/// pixel is intentionally absent rather than carried unused. The coastline /
/// graticule overlay (#72) is the first consumer that *will* need the
/// forward direction — to project polyline vertices onto the warped raster —
/// and should extend this trait with a `lonlat_to_pixel` (returning `None`
/// off the visible disc) at that point, shaped to that consumer's batching
/// needs rather than guessed at here.
pub trait TargetProjection {
    /// The loop-invariant precomputed form, built once per warp.
    type Prepared: PreparedTarget;

    /// Output raster dimensions in pixels.
    fn dims(&self) -> (u32, u32);

    /// Hoist every per-raster-constant quantity (Mercator Y of the extent,
    /// projection-centre trig, etc.) out of the per-pixel map. [`warp`]
    /// calls this once before the loop, mirroring the source-side
    /// `Projector` pattern in [`crate::projection`] (build once, call per
    /// pixel).
    fn prepare(&self) -> Self::Prepared;

    /// Convenience one-off lookup that re-runs [`Self::prepare`] each call.
    /// Handy for tests and single-point queries; on a hot path call
    /// `prepare` once and reuse the [`PreparedTarget`].
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        self.prepare().pixel_to_lonlat(px, py)
    }
}

/// Forward geographic→pixel map: fractional output pixel `(x, y)` for a
/// geographic `(lat, lon)` in degrees. Implemented by every prepared target
/// (as the inverse of [`PreparedTarget::pixel_to_lonlat`]) and by the
/// source-projection overlay map ([`crate::overlay::SourceOverlayTarget`]);
/// consumed by [`crate::overlay::project_polylines`] to place coastline /
/// graticule vertices onto a raster (#72). Split out from [`PreparedTarget`]
/// so a forward-only map need not invent a `pixel_to_lonlat` it cannot serve.
///
/// Returns `None` only for points outside the projection's *visible* domain —
/// the back hemisphere of an orthographic globe, or the far hemisphere past a
/// polar-stereographic disc's equator rim. The lat/lon-box targets
/// (equirectangular, Web Mercator) have no such geometric cutoff, so they
/// always return `Some`, even for pixels outside the raster; the caller clips
/// those rectangularly (the canvas does, in practice) and splits runs at the
/// antimeridian seam. Each box target maps `lon` to the ±360 representative
/// nearest its window centre, so a vertex just outside an edge projects to a
/// pixel just past that edge (continuous) rather than wrapping to the far side.
pub trait ForwardMap {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)>;
}

/// The precomputed per-pixel map of a [`TargetProjection`], with all
/// raster-invariant work already hoisted out. `Copy` so [`warp`] can hold
/// it cheaply across the loop; also a [`ForwardMap`] (the inverse direction)
/// for the overlay layer.
pub trait PreparedTarget: ForwardMap + Copy {
    /// Geographic `(lat, lon)` in degrees for output pixel `(px, py)`, or
    /// `None` when the pixel lies outside the projection domain.
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)>;
}

/// Walk the target pixel grid, map each pixel to `(lat, lon)`, inverse-map
/// that into the source, and sample. Returns a row-major Vec of the warped
/// values plus a presence mask (`1` present, `0` off-grid / masked / outside
/// the target's projection domain).
pub fn warp<T: TargetProjection>(
    source: &SourceGrid<'_>,
    target: &T,
    method: Resampling,
) -> WarpedRaster {
    let (w, h) = target.dims();
    let width = w as usize;
    let height = h as usize;
    let mut values = vec![0.0f64; width * height];
    let mut mask = vec![0u8; width * height];
    if width == 0 || height == 0 {
        return WarpedRaster {
            values,
            mask,
            width: w,
            height: h,
        };
    }

    // Hoist the raster-invariant projection setup once; the inner loop then
    // only does the genuinely per-pixel arithmetic.
    let prepared = target.prepare();
    for py in 0..h {
        for px in 0..w {
            let out = py as usize * width + px as usize;
            let Some((lat, lon)) = prepared.pixel_to_lonlat(px, py) else {
                continue;
            };
            let Some(idx) = (source.inverse_at)(lat, lon) else {
                continue;
            };
            if let Some(v) = sample_source(source, idx, method) {
                values[out] = v;
                mask[out] = 1;
            }
        }
    }

    WarpedRaster {
        values,
        mask,
        width: w,
        height: h,
    }
}

/// Hoisted equirectangular map: row 0 at `lat_max`, stepping by `d_lat`
/// (negative, north-up) and `d_lon` per pixel.
#[derive(Debug, Clone, Copy)]
pub struct EquirectPrepared {
    lat_max: f64,
    d_lat: f64,
    lon_min: f64,
    lon_mid: f64,
    d_lon: f64,
}

impl TargetProjection for TargetRaster {
    type Prepared = EquirectPrepared;

    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn prepare(&self) -> EquirectPrepared {
        EquirectPrepared {
            lat_max: self.lat_max,
            d_lat: span_step(self.lat_min - self.lat_max, self.height),
            lon_min: self.lon_min,
            lon_mid: (self.lon_min + self.lon_max) / 2.0,
            d_lon: span_step(self.lon_max - self.lon_min, self.width),
        }
    }
}

impl ForwardMap for EquirectPrepared {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        let py = if self.d_lat == 0.0 {
            0.0
        } else {
            (lat - self.lat_max) / self.d_lat
        };
        Some((lon_to_px(lon, self.lon_min, self.lon_mid, self.d_lon), py))
    }
}

impl PreparedTarget for EquirectPrepared {
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        let lat = self.lat_max + py as f64 * self.d_lat;
        let lon = self.lon_min + px as f64 * self.d_lon;
        Some((lat, lon))
    }
}

/// Walk the target pixel grid, inverse-map each output `(lat, lon)`
/// into the source, and sample. Thin wrapper over [`warp`] for the
/// equirectangular (lat/lon-box) target.
pub fn warp_to_equirectangular(
    source: &SourceGrid<'_>,
    target: &TargetRaster,
    method: Resampling,
) -> WarpedRaster {
    warp(source, target, method)
}

/// Spherical Web Mercator (EPSG:3857) Y coordinate for a latitude, in
/// the dimensionless `R = 1` system: `y = ln(tan(π/4 + φ/2))`. Only the
/// *ratio* of Y values matters for pixel interpolation, so the radius
/// cancels and we never carry one.
fn mercator_y(lat_deg: f64) -> f64 {
    let lat = lat_deg * DEG2RAD;
    (PI / 4.0 + lat / 2.0).tan().ln()
}

/// Inverse of [`mercator_y`]: `φ = 2·atan(eʸ) - π/2`.
fn mercator_lat(y: f64) -> f64 {
    (2.0 * y.exp().atan() - PI / 2.0) * RAD2DEG
}

/// Web Mercator target. Longitude is linear across the output width (as in
/// equirectangular); latitude is linear in the Mercator Y coordinate, so
/// rows bunch toward the poles the way every web-map tile does. The `lat`
/// extent is clamped to the `±85.0511°` valid band — the projection
/// diverges at the poles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebMercator {
    width: u32,
    height: u32,
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
}

impl WebMercator {
    /// Build a Web Mercator target, clamping the latitude extent into the
    /// projection's valid band so the Y transform stays finite. Fields are
    /// private so the clamp can't be bypassed with a struct literal.
    pub fn new(
        width: u32,
        height: u32,
        lat_min: f64,
        lat_max: f64,
        lon_min: f64,
        lon_max: f64,
    ) -> Self {
        Self {
            width,
            height,
            lat_min: lat_min.clamp(-WEB_MERCATOR_MAX_LAT, WEB_MERCATOR_MAX_LAT),
            lat_max: lat_max.clamp(-WEB_MERCATOR_MAX_LAT, WEB_MERCATOR_MAX_LAT),
            lon_min,
            lon_max,
        }
    }

    /// The clamped lat/lon-box extent actually rendered, as
    /// `(lat_min, lat_max, lon_min, lon_max)` — echoed back so a UI can
    /// pre-fill the manual-bounds inputs with the post-clamp band.
    pub fn extent(&self) -> (f64, f64, f64, f64) {
        (self.lat_min, self.lat_max, self.lon_min, self.lon_max)
    }
}

/// Hoisted Web Mercator map: row 0 at Mercator `y_max`, stepping by `d_y`
/// in Mercator Y (so latitude bunches poleward) and `d_lon` per pixel.
#[derive(Debug, Clone, Copy)]
pub struct WebMercatorPrepared {
    y_max: f64,
    d_y: f64,
    lon_min: f64,
    lon_mid: f64,
    d_lon: f64,
}

impl TargetProjection for WebMercator {
    type Prepared = WebMercatorPrepared;

    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn prepare(&self) -> WebMercatorPrepared {
        // Row 0 is the north edge → `y_max`; rows step linearly down to
        // `y_min` at the bottom.
        let y_max = mercator_y(self.lat_max);
        let y_min = mercator_y(self.lat_min);
        WebMercatorPrepared {
            y_max,
            d_y: span_step(y_min - y_max, self.height),
            lon_min: self.lon_min,
            lon_mid: (self.lon_min + self.lon_max) / 2.0,
            d_lon: span_step(self.lon_max - self.lon_min, self.width),
        }
    }
}

impl ForwardMap for WebMercatorPrepared {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        let y = mercator_y(lat);
        if !y.is_finite() {
            return None; // A pole — outside Web Mercator's finite Y range.
        }
        let py = if self.d_y == 0.0 {
            0.0
        } else {
            (y - self.y_max) / self.d_y
        };
        Some((lon_to_px(lon, self.lon_min, self.lon_mid, self.d_lon), py))
    }
}

impl PreparedTarget for WebMercatorPrepared {
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        let lat = mercator_lat(self.y_max + py as f64 * self.d_y);
        let lon = self.lon_min + px as f64 * self.d_lon;
        Some((lat, lon))
    }
}

/// Orthographic ("globe view") target centred on `(lat0, lon0)`. The
/// visible hemisphere is fitted to the output raster as the unit disc:
/// the centre pixel is `(lat0, lon0)`, the disc rim is the great circle
/// 90° away, and pixels in the square's corners (outside the disc) are
/// `None`. Inverse map per Snyder, PP-1395 §20 (sphere), eqs 20-14/20-18.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orthographic {
    width: u32,
    height: u32,
    lat0: f64,
    lon0: f64,
}

impl Orthographic {
    /// Build an orthographic target centred on `(lat0, lon0)`. `lat0` is
    /// clamped to `[-90, 90]` so the centre trig stays well-defined; any
    /// `lon0` is fine (the inverse normalises longitude downstream). Fields
    /// are private so the clamp can't be bypassed with a struct literal.
    pub fn new(width: u32, height: u32, lat0: f64, lon0: f64) -> Self {
        Self {
            width,
            height,
            lat0: lat0.clamp(-90.0, 90.0),
            lon0,
        }
    }
}

/// Hoisted orthographic map: the projection-centre trig (`sin φ₀`,
/// `cos φ₀`, `λ₀`) and the raster dims needed to place each pixel on the
/// unit disc.
#[derive(Debug, Clone, Copy)]
pub struct OrthographicPrepared {
    width: u32,
    height: u32,
    lat0: f64,
    lon0: f64,
    sin_lat0: f64,
    cos_lat0: f64,
    lon0_rad: f64,
}

impl TargetProjection for Orthographic {
    type Prepared = OrthographicPrepared;

    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn prepare(&self) -> OrthographicPrepared {
        let lat0_rad = self.lat0 * DEG2RAD;
        OrthographicPrepared {
            width: self.width,
            height: self.height,
            lat0: self.lat0,
            lon0: self.lon0,
            sin_lat0: lat0_rad.sin(),
            cos_lat0: lat0_rad.cos(),
            lon0_rad: self.lon0 * DEG2RAD,
        }
    }
}

impl ForwardMap for OrthographicPrepared {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        // Snyder PP-1395 §20 forward (sphere, R = 1), eqs 20-3/20-4. `cos_c`
        // is the cosine of the angular distance from the centre; the visible
        // hemisphere is `cos_c >= 0` (the rim sits at `cos_c = 0`).
        let phi = lat * DEG2RAD;
        let dlon = lon * DEG2RAD - self.lon0_rad;
        let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
        let cos_c = self.sin_lat0 * sin_phi + self.cos_lat0 * cos_phi * dlon.cos();
        // The rim (great circle 90° from centre) has `cos_c == 0` in exact
        // arithmetic; a small negative tolerance keeps rim points — which
        // `pixel_to_lonlat` maps as visible — on the disc despite float error.
        if cos_c < -1e-9 {
            return None; // Back of the globe.
        }
        let x = cos_phi * dlon.sin();
        let y = self.cos_lat0 * sin_phi - self.sin_lat0 * cos_phi * dlon.cos();
        // Invert `pixel_unit_coord`, north-up (y points up → row 0 at top).
        Some((
            unit_coord_to_pixel(x, self.width),
            unit_coord_to_pixel(-y, self.height),
        ))
    }
}

impl PreparedTarget for OrthographicPrepared {
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        // Map the pixel into the unit disc, north-up: x ∈ [-1, 1] L→R,
        // y ∈ [1, -1] T→B. A 1-px axis degenerates to its centre (0).
        let x = pixel_unit_coord(px, self.width);
        let y = -pixel_unit_coord(py, self.height);
        let rho = (x * x + y * y).sqrt();
        if rho > 1.0 {
            return None; // Outside the globe disc — the back of the sphere.
        }
        if rho == 0.0 {
            return Some((self.lat0, self.lon0));
        }
        // ρ = sin c for the unit sphere, so c = asin(ρ).
        let c = rho.asin();
        let (sin_c, cos_c) = (c.sin(), c.cos());
        // The bracket is sin(latitude) and is analytically in [-1, 1]; clamp
        // before asin so a rim pixel whose rounding lands one ULP past 1.0
        // can never produce a NaN latitude.
        let lat = (cos_c * self.sin_lat0 + y * sin_c * self.cos_lat0 / rho)
            .clamp(-1.0, 1.0)
            .asin();
        let lon = self.lon0_rad
            + (x * sin_c).atan2(rho * cos_c * self.cos_lat0 - y * sin_c * self.sin_lat0);
        Some((lat * RAD2DEG, lon * RAD2DEG))
    }
}

/// Polar stereographic target centred on a pole — the conformal,
/// true-shape view for high-latitude fields. The pole sits at the disc
/// centre and the opposite-side cutoff (the equator) maps to the disc
/// rim; pixels beyond the rim (the far hemisphere) are `None`. `lon0`
/// orients the meridian pointing toward the bottom of the raster.
///
/// Forward `ρ = 2·tan(π/4 - φ_signed/2)` on the unit sphere puts the
/// equator at `ρ = 2`, so the raster's unit disc is scaled by 2 (Snyder,
/// PP-1395 §21, sphere, polar aspect).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PolarStereographic {
    width: u32,
    height: u32,
    /// `true` ⇒ south-pole-centred; `false` ⇒ north-pole-centred.
    south_pole: bool,
    /// Orientation longitude pointing toward the bottom edge, degrees.
    lon0: f64,
}

impl PolarStereographic {
    /// Build a polar stereographic target centred on the chosen pole.
    /// `lon0` orients the meridian toward the bottom edge; any value is
    /// fine. Fields are private to keep construction symmetric with the
    /// other targets (validated, single entry point).
    pub fn new(width: u32, height: u32, south_pole: bool, lon0: f64) -> Self {
        Self {
            width,
            height,
            south_pole,
            lon0,
        }
    }
}

/// Hoisted polar stereographic map: the hemisphere `sign`, orientation
/// `lon0`, and the raster dims needed to place each pixel on the disc.
#[derive(Debug, Clone, Copy)]
pub struct PolarStereographicPrepared {
    width: u32,
    height: u32,
    sign: f64,
    lon0: f64,
}

impl TargetProjection for PolarStereographic {
    type Prepared = PolarStereographicPrepared;

    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn prepare(&self) -> PolarStereographicPrepared {
        PolarStereographicPrepared {
            width: self.width,
            height: self.height,
            sign: if self.south_pole { -1.0 } else { 1.0 },
            lon0: self.lon0,
        }
    }
}

impl ForwardMap for PolarStereographicPrepared {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        let sign = self.sign;
        let lat_signed = sign * lat; // Latitude measured from the disc's pole.
        if lat_signed < 0.0 {
            return None; // Past the equator rim — the far hemisphere.
        }
        // Colatitude from the pole; forward ρ = ρ_eq·tan(c/2) (Snyder §21).
        let c = (PI / 2.0 - lat_signed * DEG2RAD) / 2.0;
        let rho = POLAR_STEREO_EQUATOR_RHO * c.tan();
        // Invert the disc bearing: lon - lon0 = atan2(x, -sign·y).
        let theta = (lon - self.lon0) * DEG2RAD;
        let x = rho * theta.sin();
        let y = -sign * rho * theta.cos();
        // Disc coords are scaled by ρ_eq; undo it, then invert the pixel map.
        Some((
            unit_coord_to_pixel(x / POLAR_STEREO_EQUATOR_RHO, self.width),
            unit_coord_to_pixel(-y / POLAR_STEREO_EQUATOR_RHO, self.height),
        ))
    }
}

impl PreparedTarget for PolarStereographicPrepared {
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        // Disc scaled so the equator sits at the rim (ρ = the equator radius).
        let x = pixel_unit_coord(px, self.width) * POLAR_STEREO_EQUATOR_RHO;
        let y = -pixel_unit_coord(py, self.height) * POLAR_STEREO_EQUATOR_RHO;
        let rho = (x * x + y * y).sqrt();
        let sign = self.sign;
        if rho == 0.0 {
            return Some((sign * 90.0, self.lon0));
        }
        // ρ = ρ_eq·tan(c/2) ⇒ c = 2·atan(ρ/ρ_eq); latitude = pole − c.
        let c = 2.0 * (rho / POLAR_STEREO_EQUATOR_RHO).atan();
        let lat = sign * (PI / 2.0 - c) * RAD2DEG;
        if sign > 0.0 && lat < 0.0 || sign < 0.0 && lat > 0.0 {
            return None; // Past the equator — the far hemisphere.
        }
        // North: λ = lon0 + atan2(x, -y); the south aspect flips y.
        let lon = self.lon0 + x.atan2(-sign * y) * RAD2DEG;
        Some((lat, lon))
    }
}

/// Mollweide ("Babinet" / homalographic) equal-area world target centred on
/// the meridian `lon0`. The whole globe maps into an ellipse whose width is
/// twice its height; the raster is sized 2:1 so that ellipse fills it, and
/// pixels in the corners outside the ellipse are `None` (rendered as
/// background, as the azimuthal targets already do). Latitude is spaced so
/// every equal area on the sphere occupies an equal area on the map — the
/// property that makes it a publication favourite for global fields.
///
/// Working in the *normalized* frame where the bounding ellipse is the unit
/// disc (`X = x/(2√2)`, `Y = y/√2` of Snyder's `R = 1` coordinates), the map
/// reduces to `X = Δλ·cosθ/π`, `Y = sinθ`, with the auxiliary angle `θ`
/// solving `2θ + sin 2θ = π·sinφ` (Snyder, PP-1395 §31, sphere).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mollweide {
    width: u32,
    height: u32,
    lon0: f64,
}

impl Mollweide {
    /// Width : height of the map body — the bounding ellipse is exactly 2:1.
    /// A raster in this ratio holds the map at its true proportions.
    pub const ASPECT_RATIO: f64 = 2.0;

    /// Build a Mollweide target centred on meridian `lon0` (any value; the
    /// inverse normalises longitude downstream). Fields are private to keep
    /// construction symmetric with the other targets (single entry point).
    pub fn new(width: u32, height: u32, lon0: f64) -> Self {
        Self {
            width,
            height,
            lon0,
        }
    }
}

/// Hoisted Mollweide map: the centre meridian in radians plus the raster dims
/// needed to place each pixel in the normalized unit-disc frame.
#[derive(Debug, Clone, Copy)]
pub struct MollweidePrepared {
    width: u32,
    height: u32,
    lon0_rad: f64,
}

/// Solve `2θ + sin 2θ = π·sinφ` for the Mollweide auxiliary angle `θ` by
/// Newton's method (Snyder eq. 31-4). Converges in a handful of steps for the
/// interior; the poles (`|sinφ| = 1`) are handled in closed form to keep the
/// `cos 2θ` derivative from vanishing.
fn mollweide_theta(phi: f64) -> f64 {
    let s = phi.sin();
    if (1.0 - s.abs()) < 1e-12 {
        return (PI / 2.0).copysign(s);
    }
    let target = PI * s;
    let mut theta = phi; // Snyder's suggested initial estimate.
    for _ in 0..12 {
        let delta =
            (2.0 * theta + (2.0 * theta).sin() - target) / (2.0 + 2.0 * (2.0 * theta).cos());
        theta -= delta;
        if delta.abs() < 1e-12 {
            break;
        }
    }
    theta
}

impl TargetProjection for Mollweide {
    type Prepared = MollweidePrepared;

    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn prepare(&self) -> MollweidePrepared {
        MollweidePrepared {
            width: self.width,
            height: self.height,
            // ±360° names the same meridian as 0°, and the seam tie-break
            // reads the sign of `lon - lon0`, so a centre outside [-180, 180]
            // would flip which rim a seam vertex takes. Canonicalise it first.
            lon0_rad: wrap_to_seam_span(self.lon0, 180.0) * DEG2RAD,
        }
    }
}

impl ForwardMap for MollweidePrepared {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        let phi = lat * DEG2RAD;
        // Bring longitude into the ±π the map spans; a point on the seam keeps
        // its sign, so it lands on its own rim (see `wrap_to_seam_span`).
        let dlon = wrap_to_seam_span(lon * DEG2RAD - self.lon0_rad, PI);
        let theta = mollweide_theta(phi);
        // Normalized frame: the bounding ellipse is the unit disc.
        let x = dlon * theta.cos() / PI;
        let y = theta.sin();
        // Invert `pixel_unit_coord`, north-up (y points up → row 0 at top).
        Some((
            unit_coord_to_pixel(x, self.width),
            unit_coord_to_pixel(-y, self.height),
        ))
    }
}

impl PreparedTarget for MollweidePrepared {
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        // Pixel → normalized unit-disc frame, north-up: X ∈ [-1, 1] L→R,
        // Y ∈ [1, -1] T→B. The bounding ellipse is the unit circle here.
        let x = pixel_unit_coord(px, self.width);
        let y = -pixel_unit_coord(py, self.height);
        if x * x + y * y > 1.0 {
            return None; // Corner outside the map ellipse — background.
        }
        // Y = sinθ, so θ = asin(Y); clamp guards a rim pixel rounded past 1.
        let theta = y.clamp(-1.0, 1.0).asin();
        // φ = asin((2θ + sin2θ) / π); the bracket is in [-1, 1] analytically.
        let lat = ((2.0 * theta + (2.0 * theta).sin()) / PI)
            .clamp(-1.0, 1.0)
            .asin();
        // λ = λ0 + π·X / cosθ. At a pole cosθ → 0 and X → 0 (forced by the disc
        // bound), so the meridian is indeterminate; report the centre meridian.
        let cos_theta = theta.cos();
        let lon = if cos_theta.abs() < 1e-12 {
            self.lon0_rad
        } else {
            self.lon0_rad + PI * x / cos_theta
        };
        Some((lat * RAD2DEG, lon * RAD2DEG))
    }
}

/// Robinson's tabulated parallels (Robinson 1974; reprinted as Snyder,
/// PP-1395 Table 27), every 5° of latitude from the equator to the pole.
/// `ROBINSON_X[i]` is the length of that parallel relative to the equator's
/// and `ROBINSON_Y[i]` its distance from the equator relative to the pole's.
const ROBINSON_NODES: usize = 19;

/// Latitude step between successive table nodes, in degrees.
const ROBINSON_STEP: f64 = 5.0;

const ROBINSON_X: [f64; ROBINSON_NODES] = [
    1.0000, 0.9986, 0.9954, 0.9900, 0.9822, 0.9730, 0.9600, 0.9427, 0.9216, 0.8962, 0.8679, 0.8350,
    0.7986, 0.7597, 0.7186, 0.6732, 0.6213, 0.5722, 0.5322,
];

const ROBINSON_Y: [f64; ROBINSON_NODES] = [
    0.0000, 0.0620, 0.1240, 0.1860, 0.2480, 0.3100, 0.3720, 0.4340, 0.4958, 0.5571, 0.6176, 0.6769,
    0.7346, 0.7903, 0.8435, 0.8936, 0.9394, 0.9761, 1.0000,
];

/// Snyder's scaling constants for the table: `x = 0.8487·R·X·Δλ`,
/// `y = 1.3523·R·Y` (PP-1395 §Robinson, sphere).
const ROBINSON_FX: f64 = 0.8487;
const ROBINSON_FY: f64 = 1.3523;

/// A natural cubic spline through the 19 table nodes, held as the value at
/// each knot plus the spline's second derivative there.
///
/// Robinson published the table but never the interpolant, so every
/// implementation picks one and they differ slightly between them (Aitken
/// interpolation and cubic splines are both reported in the literature). We
/// take the natural cubic spline: it reproduces the published table exactly at
/// the nodes, is C² in between, and — verified over the whole domain by test —
/// stays monotone in `Y` and inside each node bracket, so it never overshoots
/// the table and the inverse below is single-valued.
#[derive(Debug, Clone, Copy)]
struct RobinsonSpline {
    /// Tabulated value at each knot.
    v: [f64; ROBINSON_NODES],
    /// Second derivative of the spline at each knot (w.r.t. latitude in
    /// degrees), from the natural end conditions `m[0] = m[n-1] = 0`.
    m: [f64; ROBINSON_NODES],
}

impl RobinsonSpline {
    /// Fit the natural cubic spline through `v` on the uniform 5° knot grid,
    /// solving the tridiagonal system for the knot second derivatives with the
    /// Thomas algorithm.
    fn new(v: [f64; ROBINSON_NODES]) -> Self {
        let n = ROBINSON_NODES;
        let h = ROBINSON_STEP;
        let mut m = [0.0f64; ROBINSON_NODES];
        // Interior rows: m[i-1] + 4·m[i] + m[i+1] = 6·(v[i-1] - 2v[i] + v[i+1]) / h².
        // The natural end conditions fix m[0] = m[n-1] = 0, so only the n-2
        // interior unknowns are solved. `cp`/`dp` are the Thomas sweep's
        // modified super-diagonal and right-hand side.
        let mut cp = [0.0f64; ROBINSON_NODES];
        let mut dp = [0.0f64; ROBINSON_NODES];
        for i in 1..n - 1 {
            let d = 6.0 * (v[i - 1] - 2.0 * v[i] + v[i + 1]) / (h * h);
            // Sub-diagonal is 1 and the diagonal 4 on every interior row.
            let denom = 4.0 - cp[i - 1];
            cp[i] = 1.0 / denom;
            dp[i] = (d - dp[i - 1]) / denom;
        }
        for i in (1..n - 1).rev() {
            m[i] = dp[i] - cp[i] * m[i + 1];
        }
        Self { v, m }
    }

    /// Index of the node cell containing `phi` (degrees, `0..=90`), clamped to
    /// the last cell so the pole itself evaluates on `[85°, 90°]`.
    fn cell(phi: f64) -> usize {
        ((phi / ROBINSON_STEP) as usize).min(ROBINSON_NODES - 2)
    }

    /// Evaluate the spline at latitude `phi` (degrees, `0..=90`).
    fn eval(&self, phi: f64) -> f64 {
        let i = Self::cell(phi);
        let h = ROBINSON_STEP;
        let t = (phi - i as f64 * h) / h;
        let (m0, m1) = (self.m[i], self.m[i + 1]);
        let hh6 = h * h / 6.0;
        (m0 * (1.0 - t).powi(3) + m1 * t.powi(3)) * hh6
            + (self.v[i] - m0 * hh6) * (1.0 - t)
            + (self.v[i + 1] - m1 * hh6) * t
    }

    /// Derivative of the spline at latitude `phi` (per degree).
    fn deriv(&self, phi: f64) -> f64 {
        let i = Self::cell(phi);
        let h = ROBINSON_STEP;
        let t = (phi - i as f64 * h) / h;
        let (m0, m1) = (self.m[i], self.m[i + 1]);
        -m0 * h * (1.0 - t).powi(2) / 2.0
            + m1 * h * t * t / 2.0
            + (self.v[i + 1] - self.v[i]) / h
            + (m0 - m1) * h / 6.0
    }

    /// Invert a *strictly increasing* spline: the latitude in `0..=90` whose
    /// spline value is `target`. Used for the `Y` table only (`X` decreases
    /// but is never inverted — a pixel's latitude comes from `Y` alone).
    ///
    /// The knot values bracket the root, so a binary search picks the cell and
    /// Newton — safeguarded by bisection, since a bad step would otherwise
    /// leave the cell — converges to it.
    fn invert_increasing(&self, target: f64) -> f64 {
        let t = target.clamp(self.v[0], self.v[ROBINSON_NODES - 1]);
        // Cell whose knot values bracket `t` (`v` is increasing).
        let mut i = ROBINSON_NODES - 2;
        for k in 0..ROBINSON_NODES - 1 {
            if t <= self.v[k + 1] {
                i = k;
                break;
            }
        }
        let (mut lo, mut hi) = (i as f64 * ROBINSON_STEP, (i + 1) as f64 * ROBINSON_STEP);
        // Seed with the linear estimate inside the cell — already within a
        // fraction of a degree of the root for this table.
        let span = self.v[i + 1] - self.v[i];
        let mut phi = if span > 0.0 {
            lo + (t - self.v[i]) / span * ROBINSON_STEP
        } else {
            lo
        };
        for _ in 0..24 {
            let f = self.eval(phi) - t;
            if f.abs() < 1e-13 {
                break;
            }
            if f > 0.0 {
                hi = phi
            } else {
                lo = phi
            }
            let d = self.deriv(phi);
            let next = phi - f / d;
            // Keep the iterate inside the bracket; fall back to bisection when
            // Newton would jump out of it.
            phi = if d > 0.0 && next > lo && next < hi {
                next
            } else {
                0.5 * (lo + hi)
            };
        }
        phi
    }
}

/// Robinson pseudocylindrical *compromise* world target centred on the
/// meridian `lon0` — neither equal-area nor conformal, shaped by eye to look
/// right, and the projection most world atlases used through the 1990s.
/// Parallels are straight lines, spaced by the table's `Y` and so drawn closer
/// together toward the poles; meridians are smooth curves; the poles are lines
/// about half the equator's length. The map body's corners are rounded, so a
/// pixel outside the body is `None` and reads as background, as the elliptical
/// and azimuthal targets already do.
///
/// Working in the *normalized* frame where the map body's bounding box is
/// `[-1, 1]²` (`X = x/(0.8487·π)`, `Y = y/1.3523` of Snyder's `R = 1`
/// coordinates), the map reduces to `X = X(φ)·Δλ/π`, `Y = Y(φ)`, with `X(φ)`
/// and `Y(φ)` interpolated from Robinson's published table (see
/// [`RobinsonSpline`] for the choice of interpolant).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Robinson {
    width: u32,
    height: u32,
    lon0: f64,
}

impl Robinson {
    /// Width : height of the map body, `0.8487·π : 1.3523` ≈ `1.9717 : 1`.
    /// A raster in this ratio holds the map at its true proportions.
    pub const ASPECT_RATIO: f64 = ROBINSON_FX * PI / ROBINSON_FY;

    /// Build a Robinson target centred on meridian `lon0` (any value; the
    /// inverse normalises longitude downstream).
    pub fn new(width: u32, height: u32, lon0: f64) -> Self {
        Self {
            width,
            height,
            lon0,
        }
    }
}

/// Hoisted Robinson map: the fitted table splines, the centre meridian in
/// radians, and the raster dims needed to place each pixel in the normalized
/// frame.
#[derive(Debug, Clone, Copy)]
pub struct RobinsonPrepared {
    width: u32,
    height: u32,
    lon0_rad: f64,
    x_spline: RobinsonSpline,
    y_spline: RobinsonSpline,
}

impl TargetProjection for Robinson {
    type Prepared = RobinsonPrepared;

    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn prepare(&self) -> RobinsonPrepared {
        RobinsonPrepared {
            width: self.width,
            height: self.height,
            // ±360° names the same meridian as 0°, and the seam tie-break
            // reads the sign of `lon - lon0`, so a centre outside [-180, 180]
            // would flip which rim a seam vertex takes. Canonicalise it first.
            lon0_rad: wrap_to_seam_span(self.lon0, 180.0) * DEG2RAD,
            x_spline: RobinsonSpline::new(ROBINSON_X),
            y_spline: RobinsonSpline::new(ROBINSON_Y),
        }
    }
}

impl ForwardMap for RobinsonPrepared {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        // The table is tabulated on |φ| and the map is symmetric about the
        // equator, so evaluate on the absolute latitude and re-sign `Y`.
        let abs_lat = lat.abs().min(90.0);
        // Bring longitude into the ±π the map spans; a point on the seam keeps
        // its sign, so it lands on its own rim (see `wrap_to_seam_span`).
        let dlon = wrap_to_seam_span(lon * DEG2RAD - self.lon0_rad, PI);
        // Normalized frame: the map body's bounding box is [-1, 1]².
        let x = self.x_spline.eval(abs_lat) * dlon / PI;
        let y = self.y_spline.eval(abs_lat).copysign(lat);
        Some((
            unit_coord_to_pixel(x, self.width),
            unit_coord_to_pixel(-y, self.height),
        ))
    }
}

impl PreparedTarget for RobinsonPrepared {
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        // Pixel → normalized frame, north-up: X ∈ [-1, 1] L→R, Y ∈ [1, -1] T→B.
        let x = pixel_unit_coord(px, self.width);
        let y = -pixel_unit_coord(py, self.height);
        // Y = Y(|φ|) is strictly increasing, so the row alone fixes the
        // latitude; the sign of Y picks the hemisphere.
        let abs_lat = self.y_spline.invert_increasing(y.abs());
        let lat = abs_lat.copysign(y);
        // Δλ = π·X / X(φ). Past |Δλ| = π the pixel is beyond that parallel's
        // end — one of the rounded corners outside the map body.
        let half_width = self.x_spline.eval(abs_lat);
        if x.abs() > half_width {
            return None;
        }
        let lon = self.lon0_rad + PI * x / half_width;
        Some((lat, lon * RAD2DEG))
    }
}

/// Equal Earth polynomial coefficients (Šavrič, Patterson & Jenny 2018/2019).
const EQUAL_EARTH_A1: f64 = 1.340264;
const EQUAL_EARTH_A2: f64 = -0.081106;
const EQUAL_EARTH_A3: f64 = 0.000893;
const EQUAL_EARTH_A4: f64 = 0.003796;

/// `√3/2 = sin 60°`, the factor relating the parametric latitude `θ` to the
/// latitude: `sin θ = (√3/2)·sin φ`. Written as a literal because `sqrt` is not
/// available in a `const`; pinned against `(3.0).sqrt() / 2.0` by test.
const EQUAL_EARTH_M: f64 = 0.866_025_403_784_438_6;

/// The parametric latitude at the pole: `θ = asin(√3/2) = π/3`, exactly.
const EQUAL_EARTH_THETA_MAX: f64 = PI / 3.0;

/// `fy(θ) = θ·(A1 + A2·θ² + θ⁶·(A3 + A4·θ²))` — the projected `y` on the unit
/// sphere.
const fn equal_earth_fy(theta: f64) -> f64 {
    let t2 = theta * theta;
    let t6 = t2 * t2 * t2;
    theta * (EQUAL_EARTH_A1 + EQUAL_EARTH_A2 * t2 + t6 * (EQUAL_EARTH_A3 + EQUAL_EARTH_A4 * t2))
}

/// `fy'(θ) = A1 + 3·A2·θ² + θ⁶·(7·A3 + 9·A4·θ²)` — the derivative of `fy`,
/// which also scales `x` in the forward map.
const fn equal_earth_dfy(theta: f64) -> f64 {
    let t2 = theta * theta;
    let t6 = t2 * t2 * t2;
    EQUAL_EARTH_A1
        + 3.0 * EQUAL_EARTH_A2 * t2
        + t6 * (7.0 * EQUAL_EARTH_A3 + 9.0 * EQUAL_EARTH_A4 * t2)
}

/// Half-width of the map body: `x` at the equator (`θ = 0`) on the ±180°
/// meridian, where `fy'(0) = A1`.
const EQUAL_EARTH_X_MAX: f64 = PI / (EQUAL_EARTH_M * EQUAL_EARTH_A1);

/// Half-height of the map body: `y` at the pole.
const EQUAL_EARTH_Y_MAX: f64 = equal_earth_fy(EQUAL_EARTH_THETA_MAX);

/// Equal Earth equal-area world target centred on the meridian `lon0` — the
/// 2018 projection designed to keep areas true while looking close to Robinson
/// (which is not equal-area), now the usual choice for a global thematic map.
/// Like Robinson the map body has rounded corners, so a pixel outside it is
/// `None` and reads as background.
///
/// Working in the *normalized* frame where the map body's bounding box is
/// `[-1, 1]²`, the forward map on the unit sphere (Šavrič, Patterson & Jenny,
/// sphere case `β = φ`, `R_A = R = 1`) is
/// `sin θ = (√3/2)·sin φ`, `x = Δλ·cos θ / ((√3/2)·fy'(θ))`, `y = fy(θ)`,
/// with `fy` the published quartic-in-`θ²` polynomial, then divided through by
/// the half-extents [`EQUAL_EARTH_X_MAX`] / [`EQUAL_EARTH_Y_MAX`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqualEarth {
    width: u32,
    height: u32,
    lon0: f64,
}

impl EqualEarth {
    /// Width : height of the map body ≈ `2.0546 : 1` (a touch wider than
    /// Mollweide's exact 2:1). A raster in this ratio holds the map at its true
    /// proportions.
    pub const ASPECT_RATIO: f64 = EQUAL_EARTH_X_MAX / EQUAL_EARTH_Y_MAX;

    /// Build an Equal Earth target centred on meridian `lon0` (any value; the
    /// inverse normalises longitude downstream).
    pub fn new(width: u32, height: u32, lon0: f64) -> Self {
        Self {
            width,
            height,
            lon0,
        }
    }
}

/// Hoisted Equal Earth map: the centre meridian in radians plus the raster
/// dims needed to place each pixel in the normalized frame.
#[derive(Debug, Clone, Copy)]
pub struct EqualEarthPrepared {
    width: u32,
    height: u32,
    lon0_rad: f64,
}

/// Solve `fy(θ) = y` for the Equal Earth parametric latitude `θ` by Newton's
/// method, seeded at `θ₀ = y` as the authors suggest. `fy` is strictly
/// increasing on the domain (`fy' ≥ fy'(θ_max) > 0`), so the iteration is
/// well-conditioned everywhere including the poles.
fn equal_earth_theta(y: f64) -> f64 {
    let mut theta = y;
    for _ in 0..12 {
        let delta = (equal_earth_fy(theta) - y) / equal_earth_dfy(theta);
        theta -= delta;
        if delta.abs() < 1e-12 {
            break;
        }
    }
    theta
}

impl TargetProjection for EqualEarth {
    type Prepared = EqualEarthPrepared;

    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn prepare(&self) -> EqualEarthPrepared {
        EqualEarthPrepared {
            width: self.width,
            height: self.height,
            // ±360° names the same meridian as 0°, and the seam tie-break
            // reads the sign of `lon - lon0`, so a centre outside [-180, 180]
            // would flip which rim a seam vertex takes. Canonicalise it first.
            lon0_rad: wrap_to_seam_span(self.lon0, 180.0) * DEG2RAD,
        }
    }
}

impl ForwardMap for EqualEarthPrepared {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        let phi = lat * DEG2RAD;
        // Bring longitude into the ±π the map spans; a point on the seam keeps
        // its sign, so it lands on its own rim (see `wrap_to_seam_span`).
        let dlon = wrap_to_seam_span(lon * DEG2RAD - self.lon0_rad, PI);
        // sin θ = (√3/2)·sin φ; the clamp guards |sin φ| rounded past 1.
        let theta = (EQUAL_EARTH_M * phi.sin()).clamp(-1.0, 1.0).asin();
        let x = dlon * theta.cos() / (EQUAL_EARTH_M * equal_earth_dfy(theta));
        let y = equal_earth_fy(theta);
        // Normalized frame: divide through by the half-extents.
        Some((
            unit_coord_to_pixel(x / EQUAL_EARTH_X_MAX, self.width),
            unit_coord_to_pixel(-y / EQUAL_EARTH_Y_MAX, self.height),
        ))
    }
}

impl PreparedTarget for EqualEarthPrepared {
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        // Pixel → normalized frame, north-up: X ∈ [-1, 1] L→R, Y ∈ [1, -1] T→B.
        let x = pixel_unit_coord(px, self.width) * EQUAL_EARTH_X_MAX;
        let y = -pixel_unit_coord(py, self.height) * EQUAL_EARTH_Y_MAX;
        let theta = equal_earth_theta(y);
        // φ = asin(sin θ / (√3/2)); the clamp guards the pole rounding past 1.
        let lat = (theta.sin() / EQUAL_EARTH_M).clamp(-1.0, 1.0).asin();
        // Δλ = (√3/2)·x·fy'(θ) / cos θ. cos θ ≥ cos 60° = ½, so no singularity
        // at the poles — unlike Mollweide, Equal Earth's pole is a line.
        let dlon = EQUAL_EARTH_M * x * equal_earth_dfy(theta) / theta.cos();
        if dlon.abs() > PI {
            return None; // Beyond the parallel's end — outside the map body.
        }
        Some((lat * RAD2DEG, (self.lon0_rad + dlon) * RAD2DEG))
    }
}

/// Per-pixel step that spreads `span` evenly across `n` pixels (`span /
/// (n - 1)`). A single-pixel axis collapses to a zero step rather than
/// dividing by zero.
fn span_step(span: f64, n: u32) -> f64 {
    if n <= 1 { 0.0 } else { span / (n as f64 - 1.0) }
}

/// Map a pixel index to a `[-1, 1]` coordinate across `n` pixels, north/
/// west-up. A single-pixel axis collapses to its centre (`0.0`) rather
/// than dividing by zero.
fn pixel_unit_coord(px: u32, n: u32) -> f64 {
    if n <= 1 {
        0.0
    } else {
        (px as f64 / (n as f64 - 1.0)) * 2.0 - 1.0
    }
}

/// Inverse of [`pixel_unit_coord`]: a `[-1, 1]` unit coordinate back to a
/// fractional pixel index across `n` pixels. A single-pixel axis collapses
/// to pixel `0`.
fn unit_coord_to_pixel(u: f64, n: u32) -> f64 {
    if n <= 1 {
        0.0
    } else {
        (u + 1.0) / 2.0 * (n as f64 - 1.0)
    }
}

/// Fractional pixel-x for a longitude in a `lon_min`-anchored linear window
/// whose centre longitude is `lon_mid`. The longitude is shifted by whichever
/// multiple of 360° puts it nearest `lon_mid`, so a vertex just *outside* an
/// edge of a sub-global window projects to a pixel just past that edge
/// (continuous, possibly negative or `> width`) instead of wrapping to the far
/// side of the window and tripping the overlay's antimeridian-seam split.
///
/// A point ~180° from `lon_mid` still lands ~half the window-width away, so a
/// genuine seam crossing (a polyline jumping the antimeridian opposite the
/// window) keeps its large pixel jump and is split as before. A full-globe
/// window (`lon_max - lon_min == 360`) reduces to the obvious linear map. A
/// zero-width window collapses to pixel `0`.
fn lon_to_px(lon: f64, lon_min: f64, lon_mid: f64, d_lon: f64) -> f64 {
    if d_lon == 0.0 {
        return 0.0;
    }
    let centered = lon - lon_mid;
    let nearest = wrap_to_seam_span(centered, 180.0);
    (lon_mid + nearest - lon_min) / d_lon
}

/// Pull a value from the source grid at fractional `(i, j)` using the
/// selected resampling method. Returns `None` when the requested sample
/// (or any of its bilinear neighbours) is bitmap-masked.
fn sample_source(source: &SourceGrid<'_>, idx: GridIndex, method: Resampling) -> Option<f64> {
    let ni = source.ni as i64;
    let nj = source.nj as i64;
    match method {
        Resampling::Nearest => {
            // A periodic grid wraps the column index (a seam-gap sample past
            // `ni - 1` rounds to `ni`, which is column 0 again); a bounded
            // grid clamps to the edge column.
            let i = if source.periodic_i {
                idx.i.round().rem_euclid(ni as f64) as usize
            } else {
                idx.i.round().clamp(0.0, (ni - 1) as f64) as usize
            };
            let j = idx.j.round().clamp(0.0, (nj - 1) as f64) as usize;
            (source.sample)(i, j)
        }
        Resampling::Bilinear => {
            // Floor + clamp the lower corner. The upper corner saturates
            // at the source-grid edge — letting `i1` exceed `ni - 1` would
            // mask the right column, and the same for `j1` and the bottom
            // row, producing a 1-pixel transparent border. At the edge
            // `fi` (or `fj`) is 0 anyway so the saturated column
            // contributes nothing to the weighted sum.
            //
            // On a periodic grid the columns wrap instead: `i0` reduces
            // modulo `ni` (a seam-gap index sits in `(ni - 1, ni)`) and the
            // eastern neighbour of the last column is column 0, so the seam
            // interpolates between the last and first columns.
            //
            // Off-grid points (negative or far past the edge) should
            // already be `None` from the inverse map; the clamp here is
            // defensive against accumulated float error pushing `idx.i`
            // a hair past `ni - 1`.
            let i0_f = idx.i.floor();
            let j0_f = idx.j.floor();
            let i0 = if source.periodic_i {
                i0_f.rem_euclid(ni as f64) as i64
            } else {
                i0_f as i64
            };
            let j0 = j0_f as i64;
            if i0 < 0 || j0 < 0 || i0 >= ni || j0 >= nj {
                return None;
            }
            let i0u = i0 as usize;
            let j0u = j0 as usize;
            let i1 = if source.periodic_i {
                (i0u + 1) % ni as usize
            } else {
                i0u.saturating_add(1).min((ni as usize).saturating_sub(1))
            };
            let j1 = j0u.saturating_add(1).min((nj as usize).saturating_sub(1));
            let v00 = (source.sample)(i0u, j0u)?;
            let v01 = (source.sample)(i1, j0u)?;
            let v10 = (source.sample)(i0u, j1)?;
            let v11 = (source.sample)(i1, j1)?;
            let fi = idx.i - i0_f;
            let fj = idx.j - j0_f;
            let w00 = (1.0 - fi) * (1.0 - fj);
            let w01 = fi * (1.0 - fj);
            let w10 = (1.0 - fi) * fj;
            let w11 = fi * fj;
            Some(v00 * w00 + v01 * w01 + v10 * w10 + v11 * w11)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{LatLonParams, eastward_lon_span, latlon_inverse, lon_grid_is_global};

    /// Build a source grid whose value at `(i, j)` is `j * 100 + i` — the
    /// pattern makes warp behaviour transparent to inspect.
    fn indexed_latlon_source(
        p: LatLonParams,
    ) -> (LatLonParams, impl Fn(usize, usize) -> Option<f64>) {
        let cell = move |i: usize, j: usize| Some((j * 100 + i) as f64);
        (p, cell)
    }

    /// Build a `SourceGrid` whose lifetime can outlive the call. The
    /// inverse closure captures `p` by move, then `Box::leak` extends
    /// its lifetime to `'static` — leaks one closure per call (a few
    /// dozen bytes) which the process reclaims on exit. Acceptable for
    /// test helpers; production callers (`napi/src/lib.rs`) build the
    /// closure on the stack and never leak.
    fn make_source<'a, F: Fn(usize, usize) -> Option<f64> + Sync + 'a>(
        p: &'a LatLonParams,
        sample: &'a F,
    ) -> SourceGrid<'a> {
        let ni = p.ni;
        let nj = p.nj;
        let periodic_i = lon_grid_is_global(eastward_lon_span(p.lon_first, p.lon_last), ni);
        let inverse = move |lat: f64, lon: f64| latlon_inverse(p, lat, lon);
        let inverse_ref: &'a dyn Fn(f64, f64) -> Option<GridIndex> = Box::leak(Box::new(inverse));
        SourceGrid {
            ni,
            nj,
            sample,
            inverse_at: inverse_ref,
            periodic_i,
        }
    }

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
    }

    /// A 4-column global grid: 90° step, columns at 0/90/180/270, so one more
    /// step past the last column wraps to the first (span 270° + 90° = 360°).
    fn global_latlon_params() -> LatLonParams {
        LatLonParams {
            ni: 4,
            nj: 5,
            lat_first: 50.0,
            lon_first: 0.0,
            lat_last: 10.0,
            lon_last: 270.0,
        }
    }

    #[test]
    fn periodic_nearest_wraps_the_seam_column() {
        let (p, cell) = indexed_latlon_source(global_latlon_params());
        let source = make_source(&p, &cell);
        assert!(source.periodic_i, "4×90° grid is global");
        // A seam-gap index past the midpoint rounds to `ni`, which is column
        // 0 again on a periodic grid (a bounded grid would clamp to `ni - 1`).
        let v = sample_source(&source, GridIndex { i: 3.8, j: 0.0 }, Resampling::Nearest);
        assert_eq!(v, Some(0.0));
        // The western half of the gap stays nearest to the last column.
        let v = sample_source(&source, GridIndex { i: 3.4, j: 0.0 }, Resampling::Nearest);
        assert_eq!(v, Some(3.0));
    }

    #[test]
    fn periodic_bilinear_interpolates_across_the_seam() {
        let (p, cell) = indexed_latlon_source(global_latlon_params());
        let source = make_source(&p, &cell);
        // Halfway through the seam gap blends the last column (value 3) and
        // the wrapped first column (value 0) equally.
        let v = sample_source(&source, GridIndex { i: 3.5, j: 0.0 }, Resampling::Bilinear)
            .expect("seam sample");
        assert!(near(v, 1.5, 1e-12), "got {v}");
    }

    #[test]
    fn global_grid_warps_with_no_seam_hole() {
        // The bug this guards: a global grid's seam gap (here the 90° between
        // column 3 at 270° and the wrap of column 0 at 360°) rendered masked,
        // a transparent line at the wrap meridian in every full-globe target.
        // With periodic sampling the full 0..360° window paints wall to wall.
        let (p, cell) = indexed_latlon_source(global_latlon_params());
        let source = make_source(&p, &cell);
        let target = TargetRaster {
            width: 16,
            height: 5,
            lat_max: 50.0,
            lat_min: 10.0,
            lon_min: 0.0,
            lon_max: 360.0,
        };
        for method in [Resampling::Nearest, Resampling::Bilinear] {
            let out = warp_to_equirectangular(&source, &target, method);
            let holes = out.mask.iter().filter(|&&m| m == 0).count();
            assert_eq!(holes, 0, "{method:?}: {holes} masked pixels in seam");
        }
    }

    #[test]
    fn bounded_grid_still_clamps_at_the_east_edge() {
        // Regional grid (40° span + 10° step ≠ 360°): not periodic, so both
        // resamplers saturate at the last column instead of wrapping.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 50.0,
            lon_first: 100.0,
            lat_last: 10.0,
            lon_last: 140.0,
        });
        let source = make_source(&p, &cell);
        assert!(!source.periodic_i);
        let v = sample_source(&source, GridIndex { i: 4.4, j: 0.0 }, Resampling::Nearest);
        assert_eq!(v, Some(4.0));
        let v = sample_source(&source, GridIndex { i: 4.0, j: 0.0 }, Resampling::Bilinear);
        assert_eq!(v, Some(4.0));
    }

    #[test]
    fn identity_warp_passes_through_indexed_values() {
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 50.0,
            lon_first: 100.0,
            lat_last: 10.0,
            lon_last: 140.0,
        });
        let source = make_source(&p, &cell);
        let target = TargetRaster {
            width: 5,
            height: 5,
            lat_max: 50.0,
            lat_min: 10.0,
            lon_min: 100.0,
            lon_max: 140.0,
        };
        let out = warp_to_equirectangular(&source, &target, Resampling::Nearest);
        assert_eq!(out.width, 5);
        assert_eq!(out.height, 5);
        for j in 0..5 {
            for i in 0..5 {
                let k = j * 5 + i;
                assert_eq!(out.mask[k], 1, "pixel ({i},{j}) should be present");
                assert!(
                    near(out.values[k], (j * 100 + i) as f64, 1e-9),
                    "pixel ({i},{j}) value mismatch"
                );
            }
        }
    }

    #[test]
    fn out_of_coverage_pixels_mask_cleanly() {
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 50.0,
            lon_first: 100.0,
            lat_last: 10.0,
            lon_last: 140.0,
        });
        let source = make_source(&p, &cell);
        let target = TargetRaster {
            width: 9,
            height: 9,
            lat_max: 80.0,
            lat_min: -20.0,
            lon_min: 60.0,
            lon_max: 180.0,
        };
        let out = warp_to_equirectangular(&source, &target, Resampling::Nearest);
        assert_eq!(out.mask[0], 0, "top-left should be off-grid");
        let present = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0 && present < out.mask.len());
    }

    #[test]
    fn bilinear_interpolates_midpoints() {
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 50.0,
            lon_first: 100.0,
            lat_last: 10.0,
            lon_last: 140.0,
        });
        let source = make_source(&p, &cell);
        let target = TargetRaster {
            width: 9,
            height: 9,
            lat_max: 50.0,
            lat_min: 10.0,
            lon_min: 100.0,
            lon_max: 140.0,
        };
        let out = warp_to_equirectangular(&source, &target, Resampling::Bilinear);
        // Pixel (1, 0) at lat=50, lon=105 sits halfway between source
        // cells (0,0)=0 and (1,0)=1 → expected 0.5.
        assert!(near(out.values[1], 0.5, 1e-6));
        // Pixel (0, 1) at lat=45, lon=100 sits halfway between (0,0)=0
        // and (0,1)=100 → expected 50.
        assert!(near(out.values[9], 50.0, 1e-6));
    }

    #[test]
    fn bilinear_renders_source_right_and_bottom_edges() {
        // Regression: an earlier bilinear path rejected pixels whose
        // floor-corner sat on the last source column/row because the
        // 4-neighbour stencil would have walked off the grid. The fix
        // saturates the upper neighbour at `ni-1`/`nj-1` — fi/fj are 0
        // at the edge so the saturated column contributes nothing.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 4,
            nj: 4,
            lat_first: 30.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 30.0,
        });
        let source = make_source(&p, &cell);
        // Target identical to source extents: the rightmost column at
        // x = 3 and bottom row at y = 3 must both render present.
        let out = warp_to_equirectangular(
            &source,
            &TargetRaster {
                width: 4,
                height: 4,
                lat_max: 30.0,
                lat_min: 0.0,
                lon_min: 0.0,
                lon_max: 30.0,
            },
            Resampling::Bilinear,
        );
        for j in 0..4 {
            let right = (j * 4 + 3) as usize;
            assert_eq!(out.mask[right], 1, "right-edge pixel j={j} masked");
        }
        for i in 0..4 {
            let bottom = (3 * 4 + i) as usize;
            assert_eq!(out.mask[bottom], 1, "bottom-edge pixel i={i} masked");
        }
        // Corner value should equal the source corner exactly.
        assert!(
            near(out.values[15], 303.0, 1e-9),
            "BR corner = src[3][3]=303"
        );
    }

    #[test]
    fn bilinear_masks_when_neighbour_is_masked() {
        let p = LatLonParams {
            ni: 4,
            nj: 4,
            lat_first: 3.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 3.0,
        };
        let mask_at_1_1 = |i: usize, j: usize| if i == 1 && j == 1 { None } else { Some(1.0) };
        let source = make_source(&p, &mask_at_1_1);
        let target = TargetRaster {
            width: 7,
            height: 7,
            lat_max: 3.0,
            lat_min: 0.0,
            lon_min: 0.0,
            lon_max: 3.0,
        };
        let out = warp_to_equirectangular(&source, &target, Resampling::Bilinear);
        // The stencil at (px=3, py=3) — lat=1.5, lon=1.5 — touches the
        // masked source cell (1,1) and should mask the output.
        assert_eq!(out.mask[3 * 7 + 3], 0);
    }

    #[test]
    fn single_row_or_column_targets_render_without_panic() {
        // width == 1 / height == 1 exercise the dLat / dLon zero-spacing
        // branches that the multi-pixel cases skip.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 50.0,
            lon_first: 100.0,
            lat_last: 10.0,
            lon_last: 140.0,
        });
        let source = make_source(&p, &cell);
        let single_row = warp_to_equirectangular(
            &source,
            &TargetRaster {
                width: 4,
                height: 1,
                lat_max: 30.0,
                lat_min: 30.0,
                lon_min: 100.0,
                lon_max: 140.0,
            },
            Resampling::Nearest,
        );
        assert_eq!(single_row.height, 1);
        assert!(single_row.mask.contains(&1));
        let single_col = warp_to_equirectangular(
            &source,
            &TargetRaster {
                width: 1,
                height: 4,
                lat_max: 50.0,
                lat_min: 10.0,
                lon_min: 120.0,
                lon_max: 120.0,
            },
            Resampling::Nearest,
        );
        assert_eq!(single_col.width, 1);
        assert!(single_col.mask.contains(&1));
    }

    #[test]
    fn mercator_y_round_trips_latitude() {
        for lat in [-80.0, -45.0, 0.0, 30.0, 60.0, 85.0] {
            let back = mercator_lat(mercator_y(lat));
            assert!(near(back, lat, 1e-9), "lat {lat} → {back}");
        }
        // Equator maps to Y = 0.
        assert!(near(mercator_y(0.0), 0.0, 1e-12));
    }

    #[test]
    fn web_mercator_clamps_latitude_band() {
        // Poles are outside Web Mercator's domain — `new` must pull the
        // extent into the valid band rather than producing infinite Y.
        let t = WebMercator::new(4, 4, -90.0, 90.0, -180.0, 180.0);
        let (lat_min, lat_max, ..) = t.extent();
        assert!(lat_max < 85.06 && lat_max > 85.05, "clamped max {lat_max}");
        assert!(
            lat_min > -85.06 && lat_min < -85.05,
            "clamped min {lat_min}"
        );
        // Every pixel must produce a finite (lat, lon).
        for py in 0..4 {
            for px in 0..4 {
                let (lat, lon) = t.pixel_to_lonlat(px, py).expect("in domain");
                assert!(lat.is_finite() && lon.is_finite());
            }
        }
    }

    #[test]
    fn web_mercator_rows_bunch_toward_poles() {
        // For a symmetric band the centre row sits at the equator and the
        // latitude step grows away from it — the defining property of
        // Mercator vs equirectangular's constant step.
        let t = WebMercator::new(1, 5, -80.0, 80.0, 0.0, 0.0);
        let lats: Vec<f64> = (0..5)
            .map(|py| t.pixel_to_lonlat(0, py).unwrap().0)
            .collect();
        assert!(
            near(lats[2], 0.0, 1e-9),
            "centre row at equator, got {}",
            lats[2]
        );
        // Top edge is north (row 0 = lat_max).
        assert!(lats[0] > lats[4], "row 0 should be the northern edge");
        // A fixed pixel step covers *less* latitude near the poles — that
        // pole-ward stretch is exactly why Mercator inflates polar areas.
        let outer = lats[0] - lats[1];
        let inner = lats[1] - lats[2];
        assert!(
            outer < inner,
            "pole-ward step {outer} should be smaller than equator step {inner}"
        );
    }

    #[test]
    fn web_mercator_warps_indexed_source() {
        // A Mercator target spanning the source's lat/lon box should sample
        // the source everywhere it overlaps and produce a non-empty mask.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 60.0,
            lon_first: -20.0,
            lat_last: -60.0,
            lon_last: 20.0,
        });
        let source = make_source(&p, &cell);
        let target = WebMercator::new(16, 16, -60.0, 60.0, -20.0, 20.0);
        let out = warp(&source, &target, Resampling::Bilinear);
        assert_eq!(out.width, 16);
        assert_eq!(out.height, 16);
        let present = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "mercator warp produced an empty mask");
        // Corners of the box are at the source corners → present.
        assert_eq!(out.mask[0], 1, "top-left present");
    }

    #[test]
    fn orthographic_centre_pixel_is_projection_centre() {
        // Odd dims so a pixel lands exactly on the disc centre.
        let t = Orthographic::new(5, 5, 30.0, -45.0);
        let (lat, lon) = t.pixel_to_lonlat(2, 2).expect("centre on disc");
        assert!(near(lat, 30.0, 1e-9), "centre lat {lat}");
        assert!(near(lon, -45.0, 1e-9), "centre lon {lon}");
    }

    #[test]
    fn orthographic_new_clamps_centre_latitude() {
        // An out-of-range centre latitude is pulled to the pole rather than
        // feeding a nonsensical value into the centre trig. Centre pixel of an
        // odd raster sits exactly at the (clamped) centre.
        let t = Orthographic::new(3, 3, 120.0, 10.0);
        let (lat, _) = t.pixel_to_lonlat(1, 1).expect("centre");
        assert!(near(lat, 90.0, 1e-9), "clamped centre lat {lat}");
    }

    #[test]
    fn orthographic_corners_are_off_disc() {
        // The square's corners sit at radius √2 > 1 — the back of the globe.
        let t = Orthographic::new(8, 8, 0.0, 0.0);
        assert!(t.pixel_to_lonlat(0, 0).is_none(), "TL corner off-disc");
        assert!(t.pixel_to_lonlat(7, 7).is_none(), "BR corner off-disc");
        // An interior pixel near the disc centre is on the globe.
        assert!(
            t.pixel_to_lonlat(3, 3).is_some(),
            "near-centre pixel on disc"
        );
    }

    #[test]
    fn orthographic_inverse_never_yields_nan_at_the_rim() {
        // The inverse latitude is asin() of a bracket that is analytically
        // sin(latitude) ∈ [-1, 1]; a rim pixel whose rounding pushes it one
        // ULP past 1.0 would otherwise NaN. Sweep every pixel across a range
        // of centre latitudes and assert all on-disc results stay finite.
        for &lat0 in &[0.0, 12.5, 23.5, 45.0, 60.0, 84.0, 89.0, 90.0, -67.0, -89.0] {
            for &n in &[15, 16, 31, 64, 257] {
                let t = Orthographic::new(n, n, lat0, 0.0);
                for py in 0..n {
                    for px in 0..n {
                        if let Some((lat, lon)) = t.pixel_to_lonlat(px, py) {
                            assert!(
                                lat.is_finite() && lon.is_finite(),
                                "NaN at ({px},{py}) n={n} lat0={lat0}: ({lat}, {lon})"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn orthographic_round_trips_visible_hemisphere() {
        // Forward-project a few points on the visible hemisphere into the
        // unit disc, convert to the nearest pixel of a fine raster, and
        // confirm the inverse lands back near the original (lat, lon).
        let t = Orthographic::new(1001, 1001, 40.0, 10.0);
        let (w, h) = t.dims();
        let (lat0, lon0) = (40.0_f64.to_radians(), 10.0_f64.to_radians());
        for (lat_d, lon_d) in [(40.0_f64, 10.0_f64), (55.0, 25.0), (20.0, -10.0)] {
            let (lat, lon) = (lat_d.to_radians(), lon_d.to_radians());
            // Snyder 20-3/20-4 forward (R = 1).
            let x = lat.cos() * (lon - lon0).sin();
            let y = lat0.cos() * lat.sin() - lat0.sin() * lat.cos() * (lon - lon0).cos();
            // Disc → pixel (north-up): invert `pixel_unit_coord`.
            let px = ((x + 1.0) / 2.0 * (w as f64 - 1.0)).round() as u32;
            let py = ((1.0 - y) / 2.0 * (h as f64 - 1.0)).round() as u32;
            let (rlat, rlon) = t.pixel_to_lonlat(px, py).expect("on disc");
            assert!(near(rlat, lat_d, 0.1), "lat {lat_d} → {rlat}");
            assert!(near(rlon, lon_d, 0.1), "lon {lon_d} → {rlon}");
        }
    }

    #[test]
    fn polar_stereographic_centre_is_the_pole() {
        let north = PolarStereographic::new(5, 5, false, 0.0);
        let (lat, _) = north.pixel_to_lonlat(2, 2).expect("pole");
        assert!(near(lat, 90.0, 1e-9), "north centre lat {lat}");
        let south = PolarStereographic::new(5, 5, true, 0.0);
        let (lat, _) = south.pixel_to_lonlat(2, 2).expect("pole");
        assert!(near(lat, -90.0, 1e-9), "south centre lat {lat}");
    }

    #[test]
    fn polar_stereographic_rim_is_the_equator_and_beyond_is_none() {
        let t = PolarStereographic::new(101, 101, false, 0.0);
        // Rightmost-centre pixel sits on the disc rim → equator (lat ≈ 0).
        let (lat, _) = t.pixel_to_lonlat(100, 50).expect("rim on disc");
        assert!(near(lat, 0.0, 1e-6), "rim lat {lat}");
        // The square's corner is past the rim (far hemisphere) → None.
        assert!(t.pixel_to_lonlat(0, 0).is_none(), "corner past equator");
    }

    #[test]
    fn polar_stereographic_lon0_is_a_pure_rotation() {
        // `lon0` orients the disc by rotating every output longitude by a
        // constant offset, leaving latitude untouched. Guards the otherwise
        // preset-pinned orientation parameter (presets only ever pass 0).
        const L: f64 = 30.0;
        for south_pole in [false, true] {
            let base = PolarStereographic::new(101, 101, south_pole, 0.0);
            let rotated = PolarStereographic::new(101, 101, south_pole, L);
            for (px, py) in [(50u32, 30u32), (70, 60), (40, 80)] {
                let (lat0, lon0) = base.pixel_to_lonlat(px, py).expect("on disc");
                let (lat1, lon1) = rotated.pixel_to_lonlat(px, py).expect("on disc");
                assert!(near(lat0, lat1, 1e-9), "lat must be unaffected by lon0");
                let delta = ((lon1 - lon0 + 180.0).rem_euclid(360.0)) - 180.0;
                assert!(near(delta, L, 1e-6), "south_pole={south_pole} Δlon {delta}");
            }
        }
    }

    #[test]
    fn polar_stereographic_warps_indexed_source() {
        // A north-pole disc over a source covering 0..80°N should sample a
        // non-empty region.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 9,
            nj: 9,
            lat_first: 80.0,
            lon_first: -180.0,
            lat_last: 0.0,
            lon_last: 180.0,
        });
        let source = make_source(&p, &cell);
        let target = PolarStereographic::new(32, 32, false, 0.0);
        let out = warp(&source, &target, Resampling::Nearest);
        let present = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present > 0,
            "polar-stereo target warp produced an empty mask"
        );
    }

    /// pixel → (lat, lon) → pixel must land back on the original pixel for
    /// every prepared target: `lonlat_to_pixel` is the inverse of
    /// `pixel_to_lonlat`. Only visited pixels that `pixel_to_lonlat` maps
    /// (on-disc for the azimuthal targets).
    fn assert_pixel_round_trip<P: PreparedTarget>(prep: &P, w: u32, h: u32) {
        for py in 0..h {
            for px in 0..w {
                if let Some((lat, lon)) = prep.pixel_to_lonlat(px, py) {
                    let (rx, ry) = prep
                        .lonlat_to_pixel(lat, lon)
                        .unwrap_or_else(|| panic!("({px},{py}) → ({lat},{lon}) → None"));
                    assert!(
                        near(rx, px as f64, 1e-6) && near(ry, py as f64, 1e-6),
                        "({px},{py}) round-tripped to ({rx},{ry})"
                    );
                }
            }
        }
    }

    #[test]
    fn equirectangular_forward_inverts_pixel_map() {
        let prep = TargetRaster {
            width: 13,
            height: 11,
            lat_max: 60.0,
            lat_min: -30.0,
            lon_min: -45.0,
            lon_max: 75.0,
        }
        .prepare();
        assert_pixel_round_trip(&prep, 13, 11);
    }

    #[test]
    fn web_mercator_forward_inverts_pixel_map() {
        let prep = WebMercator::new(13, 11, -70.0, 70.0, -20.0, 40.0).prepare();
        assert_pixel_round_trip(&prep, 13, 11);
    }

    #[test]
    fn orthographic_forward_inverts_and_rejects_back_hemisphere() {
        let t = Orthographic::new(21, 21, 35.0, -10.0);
        let prep = t.prepare();
        assert_pixel_round_trip(&prep, 21, 21);
        // The antipode of the centre is on the far side → None.
        assert!(
            prep.lonlat_to_pixel(-35.0, 170.0).is_none(),
            "antipode must be off the visible hemisphere"
        );
    }

    #[test]
    fn polar_stereographic_forward_inverts_and_rejects_far_hemisphere() {
        for south_pole in [false, true] {
            let prep = PolarStereographic::new(21, 21, south_pole, 25.0).prepare();
            assert_pixel_round_trip(&prep, 21, 21);
            // A point in the opposite hemisphere is past the equator rim.
            let far_lat = if south_pole { 45.0 } else { -45.0 };
            assert!(
                prep.lonlat_to_pixel(far_lat, 0.0).is_none(),
                "south_pole={south_pole}: opposite hemisphere must be None"
            );
        }
    }

    #[test]
    fn mollweide_forward_inverts_interior_and_rejects_ellipse_corners() {
        // 2:1 raster so the unit-disc frame fills it as the correct ellipse.
        // Round-trip only *interior* pixels: the ellipse boundary is the whole
        // ±180° meridian, represented on both the left and right rim, so a bare
        // (φ, ±180) is genuinely double-valued and its exact pixel can't be
        // recovered. Exclude a thin boundary margin; the seam is tested below.
        let (w, h) = (43u32, 21u32);
        let prep = Mollweide::new(w, h, 10.0).prepare();
        for py in 0..h {
            for px in 0..w {
                let x = pixel_unit_coord(px, w);
                let y = pixel_unit_coord(py, h);
                if x * x + y * y > 0.98 {
                    continue; // Near/at the rim — the seam, tested separately.
                }
                let (lat, lon) = prep.pixel_to_lonlat(px, py).expect("interior on map");
                let (rx, ry) = prep.lonlat_to_pixel(lat, lon).expect("forward on map");
                assert!(
                    near(rx, px as f64, 1e-6) && near(ry, py as f64, 1e-6),
                    "({px},{py}) → ({lat},{lon}) → ({rx},{ry})"
                );
            }
        }
        // The four raster corners lie outside the bounding ellipse → background.
        for (px, py) in [(0u32, 0u32), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert!(
                prep.pixel_to_lonlat(px, py).is_none(),
                "corner ({px},{py}) must be off the map ellipse"
            );
        }
    }

    #[test]
    fn mollweide_seam_meridian_maps_to_a_rim() {
        // The ±180°-from-centre meridian is the ellipse boundary. A point on it
        // forward-projects onto the rim (|X| = 1) at the correct latitude row,
        // and its inverse recovers the seam meridian. Centre on lon0 = 20°, so
        // the seam is at lon = 200° ≡ -160°.
        let (w, h) = (401u32, 201u32);
        let prep = Mollweide::new(w, h, 20.0).prepare();
        let (px, py) = prep.lonlat_to_pixel(0.0, -160.0).expect("seam on map");
        // Equator row is the vertical centre; the seam sits on a rim column.
        assert!(
            near(py, (h as f64 - 1.0) / 2.0, 1e-6),
            "seam equator row {py}"
        );
        assert!(
            near(px, 0.0, 1e-6) || near(px, w as f64 - 1.0, 1e-6),
            "seam must land on a rim column, got {px}"
        );
    }

    #[test]
    fn wrap_to_seam_span_breaks_every_seam_tie_toward_zero() {
        // Exactly on the seam: the sign is the tie-break, so it survives. A
        // nearest-turn round would send ±π to the opposite rim (`f64::round`
        // breaks the ±0.5 tie away from zero).
        assert_eq!(wrap_to_seam_span(PI, PI), PI, "+π stays on the +π rim");
        assert_eq!(wrap_to_seam_span(-PI, PI), -PI, "-π stays on the -π rim");
        // …and so does the tie a whole turn out: a centre meridian of ±360° is
        // a legal input, which puts a seam vertex at ±3π rather than ±π.
        assert!(
            near(wrap_to_seam_span(3.0 * PI, PI), PI, 1e-9),
            "+3π ≡ +π must reach the +π rim, got {}",
            wrap_to_seam_span(3.0 * PI, PI)
        );
        assert!(
            near(wrap_to_seam_span(-3.0 * PI, PI), -PI, 1e-9),
            "-3π ≡ -π must reach the -π rim, got {}",
            wrap_to_seam_span(-3.0 * PI, PI)
        );
        // Inside the span: untouched.
        for d in [0.0, 1.0, -1.0, PI - 1e-9, -PI + 1e-9] {
            assert_eq!(wrap_to_seam_span(d, PI), d, "{d} is already in span");
        }
        // Outside: still shifted to the nearest equivalent, as before.
        for (input, want) in [
            (PI + 0.5, -PI + 0.5),
            (-PI - 0.5, PI - 0.5),
            (2.0 * PI, 0.0),
            (-2.0 * PI, 0.0),
            (4.0 * PI, 0.0),
        ] {
            assert!(
                near(wrap_to_seam_span(input, PI), want, 1e-9),
                "{input} wrapped to {}, want {want}",
                wrap_to_seam_span(input, PI)
            );
        }
        // Degrees behave the same — this is the form `lon_to_px` uses.
        assert_eq!(wrap_to_seam_span(180.0, 180.0), 180.0);
        assert_eq!(wrap_to_seam_span(-180.0, 180.0), -180.0);
        assert!(near(wrap_to_seam_span(190.0, 180.0), -170.0, 1e-9));
        assert!(near(wrap_to_seam_span(-190.0, 180.0), 170.0, 1e-9));
        // Non-finite passes through rather than becoming a bogus finite angle.
        assert!(wrap_to_seam_span(f64::NAN, PI).is_nan());
    }

    #[test]
    fn wrap_seam_tie_is_bit_exact() {
        // The seam tie-break only fires on a value landing *exactly* on the
        // seam, so a caller converting 180° to radians must hit `PI` on the
        // nose. It does — but one ULP either way and a seam vertex would take
        // the shift branch and flip rims again, silently. Pin it.
        assert_eq!(180.0 * DEG2RAD, PI, "180° must convert to PI bit-exactly");
        assert_eq!(-180.0 * DEG2RAD, -PI);
    }

    /// A vertex *on* the map's seam meridian must land on the edge its sign
    /// names, next to its own neighbours — not teleport to the opposite edge.
    /// Natural Earth clips a ring at the antimeridian and signs each piece
    /// accordingly (Wrangel Island, ~71°N, is split into a −180° piece and a
    /// +180° piece; Antarctica likewise), so a flip drew a streak clear across
    /// the map, or lost the vertex to the overlay's seam split.
    ///
    /// `lon0` is the map's centre meridian, so the seam sits at `lon0 ± 180`.
    /// The centre is canonicalised into [-180, 180] first — `lon0 ± 180` off a
    /// raw ±360 centre would name the *same* meridian twice rather than the two
    /// sides of the seam.
    fn assert_seam_vertex_keeps_its_edge<P: ForwardMap>(prep: &P, w: u32, lon0: f64, label: &str) {
        let centre = wrap_to_seam_span(lon0, 180.0);
        for lat in [0.0, 60.0, 71.0, -71.0, -84.7] {
            let probe = |lon: f64| {
                prep.lonlat_to_pixel(lat, lon)
                    .unwrap_or_else(|| panic!("{label} @ {lon0}: ({lat}, {lon}) left the map"))
                    .0
            };
            // The seam approached from each side. `-0.5` is a hair *inside* the
            // west half, `+0.5` a hair inside the east half.
            let (west, west_neighbour) = (probe(centre - 180.0), probe(centre - 179.5));
            let (east, east_neighbour) = (probe(centre + 180.0), probe(centre + 179.5));
            let mid = (w as f64 - 1.0) / 2.0;
            assert!(
                west < mid && east > mid,
                "{label} @ lon0={lon0} at {lat}°: seam −180 → {west} and +180 → {east} \
                 must straddle centre {mid}"
            );
            // Each seam vertex sits beside its own neighbour, not an edge away.
            let span = w as f64;
            assert!(
                (west - west_neighbour).abs() < 0.05 * span,
                "{label} @ lon0={lon0} at {lat}°: seam −180 ({west}) is {} px from its \
                 neighbour ({west_neighbour})",
                (west - west_neighbour).abs()
            );
            assert!(
                (east - east_neighbour).abs() < 0.05 * span,
                "{label} @ lon0={lon0} at {lat}°: seam +180 ({east}) is {} px from its \
                 neighbour ({east_neighbour})",
                (east - east_neighbour).abs()
            );
        }
    }

    #[test]
    fn world_targets_keep_a_seam_vertex_on_its_own_rim() {
        let (w, h) = (2880u32, 1440u32);
        // ±360 is the same map as 0, and the picker accepts it (min=-360,
        // max=360) — the centre is canonicalised so the seam behaves the same.
        for lon0 in [0.0, 20.0, -150.0, 180.0, 360.0, -360.0] {
            assert_seam_vertex_keeps_its_edge(
                &Mollweide::new(w, h, lon0).prepare(),
                w,
                lon0,
                "mollweide",
            );
            assert_seam_vertex_keeps_its_edge(
                &Robinson::new(w, h, lon0).prepare(),
                w,
                lon0,
                "robinson",
            );
            assert_seam_vertex_keeps_its_edge(
                &EqualEarth::new(w, h, lon0).prepare(),
                w,
                lon0,
                "equal earth",
            );
        }
    }

    #[test]
    fn box_targets_keep_a_seam_vertex_on_its_own_edge() {
        // `lon_to_px` shares the tie: on a global window the seam is the window
        // edge, and a ±180 vertex flipped to the far edge — where the width-based
        // split then discarded it, nicking the coastline rather than streaking.
        let (w, h) = (1440u32, 721u32);
        let equirect = TargetRaster {
            width: w,
            height: h,
            lat_min: -90.0,
            lat_max: 90.0,
            lon_min: -180.0,
            lon_max: 180.0,
        };
        assert_seam_vertex_keeps_its_edge(&equirect.prepare(), w, 0.0, "equirectangular");
        assert_seam_vertex_keeps_its_edge(
            &WebMercator::new(w, h, -85.0, 85.0, -180.0, 180.0).prepare(),
            w,
            0.0,
            "web mercator",
        );
    }

    #[test]
    fn mollweide_known_points_match_published_geometry() {
        // Snyder PP-1395 §31 anchor points on the unit sphere, expressed in the
        // normalized frame (X = x/(2√2), Y = y/√2): the map centre is (0,0), the
        // poles sit at Y = ±1 on the centre meridian, the ±90° meridians at the
        // equator reach X = ±0.5, and the ±180° meridians reach the ellipse rim
        // X = ±1. Verify `pixel_to_lonlat` recovers these by sampling the exact
        // pixels they map to on a fine raster centred on lon0 = 0.
        let w = 2001u32;
        let h = 1001u32;
        let prep = Mollweide::new(w, h, 0.0).prepare();
        let at = |x: f64, y: f64| {
            // Normalized (x, y) north-up → pixel (invert pixel_unit_coord).
            let px = ((x + 1.0) / 2.0 * (w as f64 - 1.0)).round() as u32;
            let py = ((1.0 - y) / 2.0 * (h as f64 - 1.0)).round() as u32;
            prep.pixel_to_lonlat(px, py).expect("on map")
        };
        // Centre → (0, 0).
        let (lat, lon) = at(0.0, 0.0);
        assert!(
            near(lat, 0.0, 0.05) && near(lon, 0.0, 0.05),
            "centre ({lat},{lon})"
        );
        // North pole → (90, ·); the meridian is indeterminate there.
        let (lat, _) = at(0.0, 1.0);
        assert!(near(lat, 90.0, 0.2), "north pole lat {lat}");
        // South pole → (-90, ·).
        let (lat, _) = at(0.0, -1.0);
        assert!(near(lat, -90.0, 0.2), "south pole lat {lat}");
        // Equator at X = 0.5 → 90°E; at X = -0.5 → 90°W.
        let (lat, lon) = at(0.5, 0.0);
        assert!(
            near(lat, 0.0, 0.05) && near(lon, 90.0, 0.1),
            "90E ({lat},{lon})"
        );
        let (_, lon) = at(-0.5, 0.0);
        assert!(near(lon, -90.0, 0.1), "90W lon {lon}");
        // Equator rim X = ±1 → ±180° meridian.
        let (_, lon) = at(1.0, 0.0);
        assert!(near(lon.abs(), 180.0, 0.2), "east rim lon {lon}");
    }

    #[test]
    fn mollweide_forward_matches_closed_form_interior_points() {
        // Independently computed forward values (normalized frame, lon0 = 0):
        // X = Δλ·cosθ/π, Y = sinθ with θ solving 2θ+sin2θ = π·sinφ. Values from
        // an out-of-band solver (documented in the PR); assert `lonlat_to_pixel`
        // places each at the matching fractional pixel of a unit-square-mapped
        // raster (so pixel/(n-1) recovers the normalized coordinate directly).
        let (w, h) = (1001u32, 1001u32);
        let prep = Mollweide::new(w, h, 0.0).prepare();
        // (lat, lon, X, Y)
        let cases = [
            (30.0, 45.0, 0.228_692_754_4, 0.403_972_753_3),
            (-45.0, -100.0, -0.447_726_274_4, -0.592_041_749_8),
            (60.0, 150.0, 0.539_268_700_2, 0.762_386_088_1),
        ];
        for (lat, lon, x, y) in cases {
            let (px, py) = prep.lonlat_to_pixel(lat, lon).expect("on map");
            let want_px = (x + 1.0) / 2.0 * (w as f64 - 1.0);
            let want_py = (1.0 - y) / 2.0 * (h as f64 - 1.0);
            assert!(
                near(px, want_px, 1e-3) && near(py, want_py, 1e-3),
                "({lat},{lon}) → ({px},{py}), want ({want_px},{want_py})"
            );
        }
    }

    #[test]
    fn mollweide_warps_indexed_source() {
        // A global source warped into the Mollweide ellipse samples a non-empty
        // interior and leaves the corners (outside the ellipse) masked.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 13,
            nj: 13,
            lat_first: 90.0,
            lon_first: -180.0,
            lat_last: -90.0,
            lon_last: 180.0,
        });
        let source = make_source(&p, &cell);
        let target = Mollweide::new(64, 32, 0.0);
        let out = warp(&source, &target, Resampling::Nearest);
        let present = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "Mollweide warp produced an empty mask");
        // Top-left corner pixel is outside the ellipse → masked.
        assert_eq!(out.mask[0], 0, "ellipse corner should be masked");
    }

    #[test]
    fn robinson_spline_reproduces_the_published_table_at_its_nodes() {
        // The acceptance bar for a table projection: at every tabulated
        // latitude the interpolant must return the published value exactly, so
        // the map is Robinson's and not merely Robinson-shaped.
        let x = RobinsonSpline::new(ROBINSON_X);
        let y = RobinsonSpline::new(ROBINSON_Y);
        for i in 0..ROBINSON_NODES {
            let phi = i as f64 * ROBINSON_STEP;
            assert!(
                near(x.eval(phi), ROBINSON_X[i], 1e-12),
                "X({phi}) = {} want {}",
                x.eval(phi),
                ROBINSON_X[i]
            );
            assert!(
                near(y.eval(phi), ROBINSON_Y[i], 1e-12),
                "Y({phi}) = {} want {}",
                y.eval(phi),
                ROBINSON_Y[i]
            );
        }
    }

    #[test]
    fn robinson_spline_is_monotone_and_never_overshoots_the_table() {
        // Two properties the inverse leans on. Y must be strictly increasing in
        // latitude, or a map row would not fix a single latitude. And neither
        // curve may overshoot the bracket of the two nodes it sits between —
        // the failure mode of a badly chosen interpolant, which would bulge the
        // parallels between the tabulated ones.
        let xs = RobinsonSpline::new(ROBINSON_X);
        let ys = RobinsonSpline::new(ROBINSON_Y);
        let mut prev = f64::NEG_INFINITY;
        for k in 0..=9000 {
            let phi = k as f64 * 0.01;
            let (xv, yv) = (xs.eval(phi), ys.eval(phi));
            assert!(yv > prev, "Y must strictly increase, stalled at {phi}");
            prev = yv;
            assert!(ys.deriv(phi) > 0.0, "dY/dφ must stay positive at {phi}");

            let i = RobinsonSpline::cell(phi);
            assert!(
                yv >= ROBINSON_Y[i] - 1e-12 && yv <= ROBINSON_Y[i + 1] + 1e-12,
                "Y({phi}) = {yv} escaped its node bracket"
            );
            // X decreases with latitude, so its bracket runs the other way.
            assert!(
                xv <= ROBINSON_X[i] + 1e-12 && xv >= ROBINSON_X[i + 1] - 1e-12,
                "X({phi}) = {xv} escaped its node bracket"
            );
        }
    }

    #[test]
    fn robinson_forward_matches_the_table_and_interpolates_between_it() {
        // On the ±180° meridian the normalized |X| is exactly the parallel's
        // tabulated length and Y its tabulated distance from the equator, so
        // the forward map can be read straight against the published table.
        // (|X|, not X: ±180° is the seam, drawn on both rims, and the longitude
        // wrap puts it on whichever one it rounds to — see the round-trip test.)
        let (w, h) = (1001u32, 1001u32);
        let prep = Robinson::new(w, h, 0.0).prepare();
        // Pixel → the normalized frame (the inverse of `unit_coord_to_pixel`).
        let norm = |px: f64, py: f64| {
            (
                (px / (w as f64 - 1.0) * 2.0 - 1.0).abs(),
                -(py / (h as f64 - 1.0) * 2.0 - 1.0),
            )
        };
        for i in 0..ROBINSON_NODES {
            let lat = i as f64 * ROBINSON_STEP;
            let (px, py) = prep.lonlat_to_pixel(lat, 180.0).expect("on map");
            let (x, y) = norm(px, py);
            assert!(
                near(x, ROBINSON_X[i], 1e-9) && near(y, ROBINSON_Y[i], 1e-9),
                "{lat}°N on the rim → ({x}, {y}), want ({}, {})",
                ROBINSON_X[i],
                ROBINSON_Y[i]
            );
        }
        // Off-node latitudes exercise the interpolant itself. Values from an
        // out-of-band solve of the same natural spline (documented in the PR).
        for (lat, want_x, want_y) in [
            (22.5, 0.977_895_570_5, 0.279_000_606_3),
            (67.8, 0.737_018_376_4, 0.820_439_608_0),
        ] {
            let (px, py) = prep.lonlat_to_pixel(lat, 180.0).expect("on map");
            let (x, y) = norm(px, py);
            assert!(
                near(x, want_x, 1e-9) && near(y, want_y, 1e-9),
                "{lat}°N → ({x}, {y}), want ({want_x}, {want_y})"
            );
        }
    }

    #[test]
    fn robinson_forward_inverts_interior_and_rejects_the_rounded_corners() {
        let (w, h) = (79u32, 41u32);
        let prep = Robinson::new(w, h, 25.0).prepare();
        for py in 0..h {
            for px in 0..w {
                let Some((lat, lon)) = prep.pixel_to_lonlat(px, py) else {
                    continue; // Outside the map body — checked below.
                };
                let (rx, ry) = prep.lonlat_to_pixel(lat, lon).expect("forward on map");
                // Skip the rim: the ±180° meridian is drawn on both edges, so a
                // bare (φ, ±180) is double-valued and its column can't be
                // recovered. Everything inside must round-trip.
                if (rx - px as f64).abs() > 0.5 && (rx - (w as f64 - 1.0 - px as f64)).abs() < 0.5 {
                    continue;
                }
                assert!(
                    near(rx, px as f64, 1e-6) && near(ry, py as f64, 1e-6),
                    "({px},{py}) → ({lat},{lon}) → ({rx},{ry})"
                );
            }
        }
        // The pole line is barely half the equator's length, so the raster's
        // top corners fall well outside the map body → background.
        for (px, py) in [(0u32, 0u32), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert!(
                prep.pixel_to_lonlat(px, py).is_none(),
                "corner ({px},{py}) must be off the map body"
            );
        }
        // The centre of the map is the centre meridian on the equator.
        let (lat, lon) = prep.pixel_to_lonlat(w / 2, h / 2).expect("centre on map");
        assert!(
            near(lat, 0.0, 1e-9) && near(lon, 25.0, 1e-9),
            "({lat},{lon})"
        );
    }

    #[test]
    fn equal_earth_constants_match_the_published_derivation() {
        // √3/2 is spelled as a literal (no `sqrt` in a `const`) — pin it.
        assert!(near(EQUAL_EARTH_M, 3.0f64.sqrt() / 2.0, 1e-15));
        // θ at the pole solves sin θ = √3/2, i.e. exactly 60°.
        assert!(near(EQUAL_EARTH_THETA_MAX, EQUAL_EARTH_M.asin(), 1e-12));
        // Half-extents on the unit sphere, from the published polynomial.
        assert!(near(EQUAL_EARTH_X_MAX, 2.706_629_983_7, 1e-9));
        assert!(near(EQUAL_EARTH_Y_MAX, 1.317_362_759_2, 1e-9));
        // The map is a touch wider than Mollweide's 2:1 and than Robinson's.
        assert!(near(EqualEarth::ASPECT_RATIO, 2.054_582_130_0, 1e-9));
        assert!(near(Robinson::ASPECT_RATIO, 1.971_655_464_8, 1e-9));
    }

    #[test]
    fn equal_earth_forward_matches_closed_form_interior_points() {
        // Independently computed forward values in the normalized frame
        // (lon0 = 0) from the published sphere equations: sinθ = (√3/2)·sinφ,
        // x = Δλ·cosθ/((√3/2)·fy'(θ)), y = fy(θ), each over its half-extent.
        let (w, h) = (1001u32, 1001u32);
        let prep = EqualEarth::new(w, h, 0.0).prepare();
        let cases = [
            (30.0, 45.0, 0.233_842_609_7, 0.450_092_516_8),
            (-45.0, -100.0, -0.476_137_199_8, -0.652_994_841_1),
            (60.0, 150.0, 0.627_797_944_3, 0.826_120_844_8),
        ];
        for (lat, lon, x, y) in cases {
            let (px, py) = prep.lonlat_to_pixel(lat, lon).expect("on map");
            let want_px = (x + 1.0) / 2.0 * (w as f64 - 1.0);
            let want_py = (1.0 - y) / 2.0 * (h as f64 - 1.0);
            assert!(
                near(px, want_px, 1e-6) && near(py, want_py, 1e-6),
                "({lat},{lon}) → ({px},{py}), want ({want_px},{want_py})"
            );
        }
    }

    #[test]
    fn equal_earth_anchor_points_sit_where_the_geometry_says() {
        let (w, h) = (2001u32, 1001u32);
        let prep = EqualEarth::new(w, h, 0.0).prepare();
        let (cx, cy) = ((w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0);
        // Map centre → (0°, 0°).
        let (px, py) = prep.lonlat_to_pixel(0.0, 0.0).expect("on map");
        assert!(near(px, cx, 1e-9) && near(py, cy, 1e-9), "centre");
        // The poles are *lines*, not points: the north pole at any longitude
        // sits on the top row, and its ±180° end stops short of the equator's
        // full half-width. (The seam lands on whichever rim the longitude wrap
        // rounds to, so measure the distance from the centre meridian.)
        let (px_pole, py_pole) = prep.lonlat_to_pixel(90.0, 0.0).expect("on map");
        assert!(near(py_pole, 0.0, 1e-9), "north pole row {py_pole}");
        assert!(near(px_pole, cx, 1e-9), "pole on the centre meridian");
        let (px_end, py_end) = prep.lonlat_to_pixel(90.0, 180.0).expect("on map");
        assert!(near(py_end, 0.0, 1e-9), "pole line stays on the top row");
        let pole_half = (px_end - cx).abs();
        assert!(pole_half > 0.0, "the pole is a line, not a point");
        assert!(
            pole_half < cx,
            "the pole line is shorter than the equator: {pole_half} vs {cx}"
        );
        // The equator's ±180° ends reach the frame edge.
        let (px_e, _) = prep.lonlat_to_pixel(0.0, 180.0).expect("on map");
        assert!(near((px_e - cx).abs(), cx, 1e-9), "equator reaches the rim");
    }

    #[test]
    fn equal_earth_forward_inverts_interior_and_rejects_the_rounded_corners() {
        let (w, h) = (83u32, 41u32);
        let prep = EqualEarth::new(w, h, -60.0).prepare();
        for py in 0..h {
            for px in 0..w {
                let Some((lat, lon)) = prep.pixel_to_lonlat(px, py) else {
                    continue;
                };
                let (rx, ry) = prep.lonlat_to_pixel(lat, lon).expect("forward on map");
                // Skip the doubled ±180° rim, as in the Robinson round-trip.
                if (rx - px as f64).abs() > 0.5 && (rx - (w as f64 - 1.0 - px as f64)).abs() < 0.5 {
                    continue;
                }
                assert!(
                    near(rx, px as f64, 1e-6) && near(ry, py as f64, 1e-6),
                    "({px},{py}) → ({lat},{lon}) → ({rx},{ry})"
                );
            }
        }
        for (px, py) in [(0u32, 0u32), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert!(
                prep.pixel_to_lonlat(px, py).is_none(),
                "corner ({px},{py}) must be off the map body"
            );
        }
    }

    #[test]
    fn equal_earth_preserves_relative_area() {
        // The whole point of the projection. Two equal-area caps of the sphere
        // must cover the same map area, which Robinson (a compromise) does not.
        // A latitude band's map area is ∫ 2·x(φ, 180°) dy over the band, since
        // the parallel at φ runs from -x to +x. Integrate by trapezoid on the
        // forward map. `lonlat_to_pixel` returns a continuous coordinate, so the
        // raster size only sets a constant scale, which cancels in the ratio.
        let prep = EqualEarth::new(1001, 1001, 0.0).prepare();
        let centre_x = prep.lonlat_to_pixel(0.0, 0.0).expect("on map").0;
        let band_area = |lat0: f64, lat1: f64| {
            let n = 2000;
            let edge = |lat: f64| {
                let (px, py) = prep.lonlat_to_pixel(lat, 180.0).expect("on map");
                ((px - centre_x).abs(), py)
            };
            (0..n)
                .map(|i| {
                    let a = lat0 + (lat1 - lat0) * i as f64 / n as f64;
                    let b = lat0 + (lat1 - lat0) * (i + 1) as f64 / n as f64;
                    let (half_a, ya) = edge(a);
                    let (half_b, yb) = edge(b);
                    (half_a + half_b) / 2.0 * (ya - yb).abs()
                })
                .sum::<f64>()
        };
        // Sphere area of a band ∝ sin φ1 − sin φ0. Pick two bands with equal
        // sphere area: 0°–30° (sin = 0.5) and 30°–90° (1 − 0.5 = 0.5).
        let low = band_area(0.0, 30.0);
        let high = band_area(30.0, 90.0);
        assert!(
            (low - high).abs() / low < 2e-3,
            "equal sphere areas must map to equal map areas: {low} vs {high}"
        );
    }

    #[test]
    fn world_targets_warp_indexed_source() {
        // A global source warped into each world target samples a non-empty
        // interior and leaves the corners (outside the map body) masked.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 13,
            nj: 13,
            lat_first: 90.0,
            lon_first: -180.0,
            lat_last: -90.0,
            lon_last: 180.0,
        });
        let source = make_source(&p, &cell);
        let robin = warp(&source, &Robinson::new(64, 32, 0.0), Resampling::Nearest);
        assert!(
            robin.mask.contains(&1),
            "Robinson warp produced an empty mask"
        );
        assert_eq!(robin.mask[0], 0, "Robinson corner should be masked");
        let ee = warp(&source, &EqualEarth::new(64, 32, 0.0), Resampling::Nearest);
        assert!(
            ee.mask.contains(&1),
            "Equal Earth warp produced an empty mask"
        );
        assert_eq!(ee.mask[0], 0, "Equal Earth corner should be masked");
    }

    #[test]
    fn equirectangular_forward_normalises_longitude_across_seam() {
        // A 0..360 window: a coastline vertex at lon = -170 (≡ 190) must land
        // near the right edge, not at a negative pixel.
        let prep = TargetRaster {
            width: 361,
            height: 2,
            lat_max: 1.0,
            lat_min: -1.0,
            lon_min: 0.0,
            lon_max: 360.0,
        }
        .prepare();
        let (px, _) = prep.lonlat_to_pixel(0.0, -170.0).unwrap();
        assert!(
            near(px, 190.0, 1e-6),
            "lon -170 in [0,360] window → px {px}"
        );
    }

    #[test]
    fn generic_warp_matches_equirectangular_wrapper() {
        // `warp_to_equirectangular` is now a thin wrapper over `warp` with a
        // `TargetRaster` target — they must produce byte-identical output.
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 50.0,
            lon_first: 100.0,
            lat_last: 10.0,
            lon_last: 140.0,
        });
        let source = make_source(&p, &cell);
        let target = TargetRaster {
            width: 7,
            height: 7,
            lat_max: 50.0,
            lat_min: 10.0,
            lon_min: 100.0,
            lon_max: 140.0,
        };
        let via_wrapper = warp_to_equirectangular(&source, &target, Resampling::Bilinear);
        let via_generic = warp(&source, &target, Resampling::Bilinear);
        assert_eq!(via_wrapper.mask, via_generic.mask);
        assert_eq!(via_wrapper.values, via_generic.values);
    }

    #[test]
    fn degenerate_target_returns_empty_mask() {
        let (p, cell) = indexed_latlon_source(LatLonParams {
            ni: 5,
            nj: 5,
            lat_first: 50.0,
            lon_first: 100.0,
            lat_last: 10.0,
            lon_last: 140.0,
        });
        let source = make_source(&p, &cell);
        let out = warp_to_equirectangular(
            &source,
            &TargetRaster {
                width: 0,
                height: 5,
                lat_max: 50.0,
                lat_min: 10.0,
                lon_min: 100.0,
                lon_max: 140.0,
            },
            Resampling::Nearest,
        );
        assert_eq!(out.width, 0);
        assert!(out.values.is_empty());
    }
}
