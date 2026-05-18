//! Inverse-warp pipeline: paint a source GRIB grid into a target
//! equirectangular raster.
//!
//! The caller provides a [`SourceGrid`] (raw values + a per-grid-type
//! `inverse_at` closure from [`crate::projection`]) and a [`TargetRaster`]
//! describing the output's lat/lon bounds and pixel dimensions. We walk
//! the output pixel grid, ask the source where each `(lat, lon)` lives,
//! and sample.
//!
//! "Equirectangular" is the only target supported here today — picker
//! UX in #45 defaults to source-projection (no warp) and equirectangular
//! (this code). Web Mercator / ortho / polar-stereo targets are tracked
//! under #71 and slot in by writing a different pixel-to-`(lat, lon)`
//! conversion ahead of the inverse map.

use crate::projection::GridIndex;

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

/// Walk the target pixel grid, inverse-map each output `(lat, lon)`
/// into the source, and sample. Returns a row-major Vec of the warped
/// values plus a presence mask.
pub fn warp_to_equirectangular(
    source: &SourceGrid<'_>,
    target: &TargetRaster,
    method: Resampling,
) -> WarpedRaster {
    let width = target.width as usize;
    let height = target.height as usize;
    let mut values = vec![0.0f64; width * height];
    let mut mask = vec![0u8; width * height];
    if width == 0 || height == 0 {
        return WarpedRaster {
            values,
            mask,
            width: target.width,
            height: target.height,
        };
    }

    let d_lat = if height == 1 {
        0.0
    } else {
        (target.lat_min - target.lat_max) / (height as f64 - 1.0)
    };
    let d_lon = if width == 1 {
        0.0
    } else {
        (target.lon_max - target.lon_min) / (width as f64 - 1.0)
    };

    for py in 0..height {
        let lat = target.lat_max + py as f64 * d_lat;
        for px in 0..width {
            let lon = target.lon_min + px as f64 * d_lon;
            let out = py * width + px;
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
        width: target.width,
        height: target.height,
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
            // Floor and ceil with clamps. If any of the 4 corners is
            // masked we conservatively mask the output — mixing a real
            // value with a fill is worse than reporting "no value here".
            let i0_f = idx.i.floor();
            let j0_f = idx.j.floor();
            let i0 = i0_f as i64;
            let j0 = j0_f as i64;
            if i0 < 0 || j0 < 0 || i0 + 1 >= ni || j0 + 1 >= nj {
                return None;
            }
            let i0u = i0 as usize;
            let j0u = j0 as usize;
            let i1 = i0u + 1;
            let j1 = j0u + 1;
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
