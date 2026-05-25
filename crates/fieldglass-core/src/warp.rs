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

/// The precomputed per-pixel map of a [`TargetProjection`], with all
/// raster-invariant work already hoisted out. `Copy` so [`warp`] can hold
/// it cheaply across the loop.
pub trait PreparedTarget: Copy {
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
            d_lon: span_step(self.lon_max - self.lon_min, self.width),
        }
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
            d_lon: span_step(self.lon_max - self.lon_min, self.width),
        }
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
        let lat = (cos_c * self.sin_lat0 + y * sin_c * self.cos_lat0 / rho).asin();
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

/// Pull a value from the source grid at fractional `(i, j)` using the
/// selected resampling method. Returns `None` when the requested sample
/// (or any of its bilinear neighbours) is bitmap-masked.
fn sample_source(source: &SourceGrid<'_>, idx: GridIndex, method: Resampling) -> Option<f64> {
    let ni = source.ni as i64;
    let nj = source.nj as i64;
    match method {
        Resampling::Nearest => {
            let i = idx.i.round().clamp(0.0, (ni - 1) as f64) as usize;
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
            // Off-grid points (negative or far past the edge) should
            // already be `None` from the inverse map; the clamp here is
            // defensive against accumulated float error pushing `idx.i`
            // a hair past `ni - 1`.
            let i0_f = idx.i.floor();
            let j0_f = idx.j.floor();
            let i0 = i0_f as i64;
            let j0 = j0_f as i64;
            if i0 < 0 || j0 < 0 || i0 >= ni || j0 >= nj {
                return None;
            }
            let i0u = i0 as usize;
            let j0u = j0 as usize;
            let i1 = (i0u + 1).min((ni as usize).saturating_sub(1));
            let j1 = (j0u + 1).min((nj as usize).saturating_sub(1));
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
    use crate::projection::{LatLonParams, latlon_inverse};

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
        let inverse = move |lat: f64, lon: f64| latlon_inverse(p, lat, lon);
        let inverse_ref: &'a dyn Fn(f64, f64) -> Option<GridIndex> = Box::leak(Box::new(inverse));
        SourceGrid {
            ni,
            nj,
            sample,
            inverse_at: inverse_ref,
        }
    }

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() < tol
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
