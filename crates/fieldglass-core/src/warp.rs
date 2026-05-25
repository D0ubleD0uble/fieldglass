//! Inverse-warp pipeline: paint a source GRIB grid into a target
//! equirectangular raster.
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
pub trait TargetProjection {
    /// Output raster dimensions in pixels.
    fn dims(&self) -> (u32, u32);
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

    for py in 0..h {
        for px in 0..w {
            let out = py as usize * width + px as usize;
            let Some((lat, lon)) = target.pixel_to_lonlat(px, py) else {
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

impl TargetProjection for TargetRaster {
    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        let d_lat = if self.height <= 1 {
            0.0
        } else {
            (self.lat_min - self.lat_max) / (self.height as f64 - 1.0)
        };
        let d_lon = if self.width <= 1 {
            0.0
        } else {
            (self.lon_max - self.lon_min) / (self.width as f64 - 1.0)
        };
        let lat = self.lat_max + py as f64 * d_lat;
        let lon = self.lon_min + px as f64 * d_lon;
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
/// extent is clamped to `±`[`WEB_MERCATOR_MAX_LAT`] — the projection
/// diverges at the poles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WebMercator {
    pub width: u32,
    pub height: u32,
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
}

impl WebMercator {
    /// Build a Web Mercator target, clamping the latitude extent into the
    /// projection's valid band so the Y transform stays finite.
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
}

impl TargetProjection for WebMercator {
    fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        // Row 0 is the north edge → `y_max`; rows step linearly down to
        // `y_min` at the bottom.
        let y_max = mercator_y(self.lat_max);
        let y_min = mercator_y(self.lat_min);
        let d_y = if self.height <= 1 {
            0.0
        } else {
            (y_min - y_max) / (self.height as f64 - 1.0)
        };
        let d_lon = if self.width <= 1 {
            0.0
        } else {
            (self.lon_max - self.lon_min) / (self.width as f64 - 1.0)
        };
        let lat = mercator_lat(y_max + py as f64 * d_y);
        let lon = self.lon_min + px as f64 * d_lon;
        Some((lat, lon))
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
        assert!(
            t.lat_max < 85.06 && t.lat_max > 85.05,
            "clamped max {}",
            t.lat_max
        );
        assert!(
            t.lat_min > -85.06 && t.lat_min < -85.05,
            "clamped min {}",
            t.lat_min
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
