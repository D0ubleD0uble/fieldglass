//! A colour change, or a change of field on the same grid, repaints without
//! warping again, through the handle the extension calls (#1233).
//!
//! `fieldglass`'s `warp_is_memoised.rs` holds the memo to its rules over the
//! projected values. This is the other half: that every kind of handle keeps
//! one and its `renderGrid` warps through it, and that the picture is the
//! byte-identical RGBA a fresh handle paints with the same options. Counted
//! with `WarpMemo::plans_made`, not timed: see that file for why.

use super::*;
use crate::netcdf_slice_tests::opts;
use serde_json::json;

/// Three RTOFS ice fields on one window of the tripolar mesh.
const RTOFS: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc");
/// A Lambert conformal HRRR message: a grid whose inverse map is a formula.
const LAMBERT: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2");
/// A Zarr v3 store with CF coordinates.
const STORE: &str = "../fieldglass-zarr/tests/fixtures/stores/cf_v3";

/// The colour controls the panel offers, one change after another, on
/// `projection`: colour map, reversal, log scale, a manual range.
fn colour_changes(projection: &str) -> Vec<RenderOptions> {
    vec![
        RenderOptions {
            colormap: Some("turbo".into()),
            ..opts(projection)
        },
        RenderOptions {
            colormap: Some("turbo".into()),
            reverse_colormap: Some(true),
            ..opts(projection)
        },
        RenderOptions {
            colormap: Some("cividis".into()),
            scale_mode: Some("log10".into()),
            range_min: Some(0.01),
            ..opts(projection)
        },
        RenderOptions {
            colormap: Some("rdbu".into()),
            range_min: Some(0.1),
            range_max: Some(0.6),
            ..opts(projection)
        },
    ]
}

/// Every slice target on a variable handle, one per variable with horizontal
/// axes, in the numbering `renderGrid` takes.
fn slice_targets(handle: &Handle) -> Vec<serde_json::Value> {
    handle
        .variables()
        .expect("lists")
        .iter()
        .filter_map(|v| {
            Some(json!({
                "variable": v.index,
                "yDim": v.detected_y_dim?,
                "xDim": v.detected_x_dim?,
                "sliceIndices": vec![0; v.dims.len()],
            }))
        })
        .collect()
}

/// `handle` paints `target` under `o` exactly as a handle that has drawn
/// nothing yet does: the RGBA to the byte, the size, the range it reports.
fn assert_paints_as_fresh(
    handle: &Handle,
    fresh: impl Fn() -> Handle,
    target: &serde_json::Value,
    o: impl Fn() -> RenderOptions,
) {
    let drawn = handle.render_grid(target.clone(), o()).expect("renders");
    let full = fresh()
        .render_grid(target.clone(), o())
        .expect("a fresh handle renders");
    assert_eq!(
        (
            drawn.width,
            drawn.height,
            &drawn.range,
            &drawn.projection_summary
        ),
        (
            full.width,
            full.height,
            &full.range,
            &full.projection_summary
        ),
        "{target} under {:?}",
        o()
    );
    assert!(
        drawn.rgba.as_ref() == full.rgba.as_ref(),
        "the picture after a repaint is a full render's, {target} under {:?}",
        o()
    );
}

/// Each of `colour_changes(projection)` in turn.
fn each_colour_change(projection: &str, mut f: impl FnMut(&dyn Fn() -> RenderOptions)) {
    for k in 0..colour_changes(projection).len() {
        f(&|| colour_changes(projection).swap_remove(k));
    }
}

#[test]
fn a_colour_change_on_a_curvilinear_slice_warps_once() {
    let fresh = || Handle::from_vec(RTOFS.to_vec()).expect("RTOFS opens");
    let handle = fresh();
    let targets = slice_targets(&handle);
    assert!(targets.len() >= 3, "three ice fields: {targets:?}");
    let base = || opts("equirectangular");

    assert_paints_as_fresh(&handle, fresh, &targets[0], base);
    assert_eq!(handle.plans_made(), 1, "the first draw plans");
    each_colour_change("equirectangular", |o| {
        assert_paints_as_fresh(&handle, fresh, &targets[0], o);
    });
    assert_eq!(handle.plans_made(), 1, "no colour change warps again");

    // The other fields are on the same mesh, so they read the same cells.
    for target in &targets[1..] {
        assert_paints_as_fresh(&handle, fresh, target, base);
    }
    assert_eq!(
        handle.plans_made(),
        1,
        "a change of field samples the same plan"
    );

    // A combined map is painted against `a`'s grid, which is the same one.
    let combined = json!({ "a": targets[1], "b": targets[2], "op": "a_minus_b" });
    assert_paints_as_fresh(&handle, fresh, &combined, base);
    assert_eq!(handle.plans_made(), 1, "nor does a combined map on it");

    // A new target is a new warp, and the source view needs none.
    assert_paints_as_fresh(&handle, fresh, &targets[0], || opts("robinson"));
    assert_paints_as_fresh(&handle, fresh, &targets[0], || opts("source"));
    assert_eq!(handle.plans_made(), 2, "another target plans once more");
}

#[test]
fn a_colour_change_on_a_grib_message_warps_once() {
    let fresh = || Handle::from_vec(LAMBERT.to_vec()).expect("HRRR opens");
    let handle = fresh();
    let base = || opts("equirectangular");
    assert_paints_as_fresh(&handle, fresh, &json!(0), base);
    each_colour_change("equirectangular", |o| {
        assert_paints_as_fresh(&handle, fresh, &json!(0), o);
    });
    let combined = json!({ "a": 0, "b": 0, "op": "a_minus_b" });
    assert_paints_as_fresh(&handle, fresh, &combined, base);
    assert_eq!(
        handle.plans_made(),
        1,
        "a GRIB message's geometry is handed over afresh each draw, and compares equal"
    );
}

#[test]
fn a_colour_change_on_a_store_slice_warps_once() {
    let fresh = || crate::Handle::from_store(STORE.to_string()).expect("the fixture is a store");
    let handle = fresh();
    let targets = slice_targets(&handle);
    let target = targets.first().expect("a drawable variable");
    assert_paints_as_fresh(&handle, fresh, target, || opts("mollweide"));
    each_colour_change("mollweide", |o| {
        assert_paints_as_fresh(&handle, fresh, target, o);
    });
    assert_eq!(handle.plans_made(), 1);
}
