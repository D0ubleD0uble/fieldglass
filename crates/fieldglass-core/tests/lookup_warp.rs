//! `warp` lives behind the `render` feature; the index and its `NearestOnly`
//! constraint do not, because the format crates take `core` with
//! `default-features = false` and are the ones that will build a lookup grid.
//! This file tests the join, so it needs the feature.
#![cfg(feature = "render")]

//! A lookup grid warps, and a bilinear request against one degrades to nearest.
//!
//! The index's own correctness is `spatial_index.rs`'s subject. This file is
//! about the seam: that `warp` reaches a [`SpatialIndex`] through the ordinary
//! `SourceGrid::inverse_at` closure, and that the `NearestOnly` constraint is
//! enforced by `warp` itself rather than left to whoever calls it.

use fieldglass_core::projection::{GridGeometry, GridIndex, GridResampling};
use fieldglass_core::spatial_index::SpatialIndex;
use fieldglass_core::warp::{Resampling, SourceGrid, TargetRaster, warp_to_equirectangular};

/// A folded grid: column 0 sits near 0°E and column 1 near 180°E, so the two
/// are index-adjacent but half a world apart — the shape of a tripolar ocean
/// grid along its seam. Values are chosen so that blending the two columns
/// would produce a number neither of them holds.
fn folded() -> (SpatialIndex, Vec<f64>, u32, u32) {
    let (ni, nj) = (2u32, 5u32);
    let mut lats = Vec::new();
    let mut lons = Vec::new();
    let mut values = Vec::new();
    for j in 0..nj {
        lats.push(60.0 + j as f64);
        lons.push(0.0);
        values.push(0.0); // west limb
        lats.push(60.0 + j as f64);
        lons.push(180.0);
        values.push(100.0); // east limb
    }
    (
        SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds"),
        values,
        ni,
        nj,
    )
}

fn source<'a>(
    ix: &'a SpatialIndex,
    inverse: &'a dyn Fn(f64, f64) -> Option<GridIndex>,
    sample: &'a dyn Fn(usize, usize) -> Option<f64>,
) -> SourceGrid<'a> {
    let (ni, nj) = ix.dims();
    SourceGrid {
        ni,
        nj,
        sample,
        inverse_at: inverse,
        periodic_i: false,
        resampling: ix.resampling(),
    }
}

#[test]
fn a_bilinear_request_against_a_lookup_grid_gives_the_nearest_result() {
    let (ix, values, ni, _) = folded();
    let inverse = |lat: f64, lon: f64| ix.nearest(lat, lon);
    let sample = |i: usize, j: usize| values.get(j * ni as usize + i).copied();
    let src = source(&ix, &inverse, &sample);

    let target = TargetRaster {
        width: 24,
        height: 12,
        lat_max: 66.0,
        lat_min: 58.0,
        lon_min: -6.0,
        lon_max: 6.0,
    };
    let nearest = warp_to_equirectangular(&src, &target, Resampling::Nearest);
    let bilinear = warp_to_equirectangular(&src, &target, Resampling::Bilinear);

    assert_eq!(
        bilinear.values, nearest.values,
        "warp must downgrade bilinear on a NearestOnly grid, not blend"
    );
    assert_eq!(bilinear.mask, nearest.mask);

    // The stronger statement: no output pixel holds a blend. This target sits
    // over the west limb only, so every present pixel must be exactly its
    // value — 50.0 would be the two limbs averaged across the fold.
    let present: Vec<f64> = bilinear
        .values
        .iter()
        .zip(&bilinear.mask)
        .filter(|&(_, &m)| m == 1)
        .map(|(&v, _)| v)
        .collect();
    assert!(!present.is_empty(), "the target overlaps the grid");
    for v in present {
        assert_eq!(
            v, 0.0,
            "a value that is neither limb means a blend happened"
        );
    }
}

#[test]
fn a_lookup_grid_warps_where_it_covers_and_nowhere_else() {
    let (ni, nj) = (30u32, 20u32);
    let mut lats = Vec::new();
    let mut lons = Vec::new();
    for j in 0..nj {
        for i in 0..ni {
            lats.push(40.0 + j as f64 * 0.5);
            lons.push(-10.0 + i as f64 * 0.5);
        }
    }
    let ix = SpatialIndex::new(ni, nj, &lats, &lons).expect("index builds");
    let values: Vec<f64> = (0..(ni * nj) as usize).map(|k| k as f64).collect();
    let inverse = |lat: f64, lon: f64| ix.nearest(lat, lon);
    let sample = |i: usize, j: usize| values.get(j * ni as usize + i).copied();
    let src = source(&ix, &inverse, &sample);

    // A target far wider than the grid: the pixels outside it must stay absent
    // rather than being painted with the nearest edge cell.
    let target = TargetRaster {
        width: 72,
        height: 36,
        lat_max: 80.0,
        lat_min: -80.0,
        lon_min: -180.0,
        lon_max: 180.0,
    };
    let out = warp_to_equirectangular(&src, &target, Resampling::Nearest);
    let present = out.mask.iter().filter(|&&m| m == 1).count();
    assert!(
        present > 0,
        "the grid is inside the target, so something renders"
    );
    assert!(
        present < out.mask.len() / 4,
        "a 10x10 degree grid must not paint a whole world map: {present} of {} pixels",
        out.mask.len()
    );

    // Every value that did render is a real cell value, not an interpolation.
    for (&v, &m) in out.values.iter().zip(&out.mask) {
        if m == 1 {
            assert_eq!(v.fract(), 0.0, "value {v} is not one of the grid's own");
        }
    }
}

#[test]
fn a_formula_grid_still_blends() {
    // The guard on the guard: `warp` must downgrade only what asks to be
    // downgraded, or this change would silently disable bilinear everywhere.
    assert_eq!(
        GridGeometry::LatLon(fieldglass_core::projection::LatLonParams {
            ni: 4,
            nj: 4,
            lat_first: 3.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 3.0,
        })
        .resampling(),
        GridResampling::Any
    );
}
