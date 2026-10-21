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

/// The first drawable variable of a store, as the arguments of `slice`.
fn a_drawable_slice(store: &crate::ZarrStore) -> (u32, u32, u32, Vec<u32>) {
    store
        .variables()
        .iter()
        .find_map(|v| {
            Some((
                u32::try_from(v.variable_index).ok()?,
                u32::try_from(v.detected_y_dim?).ok()?,
                u32::try_from(v.detected_x_dim?).ok()?,
                vec![0; v.dims.len()],
            ))
        })
        .expect("a drawable variable")
}

/// A repaint of a Zarr slice reads the slice it already decoded (#1273): a
/// colour change, a probe, contours and an export of the slice on screen
/// decode nothing, and a combined map decodes each of its slices once. Counted
/// through the handle's own decode counter, as the plans are.
#[test]
fn a_repaint_of_a_store_slice_does_not_decode_it_again() {
    let handle = crate::Handle::from_store(STORE.to_string()).expect("the fixture is a store");
    let targets = slice_targets(&handle);
    let target = targets.first().expect("a drawable variable");
    let base = || opts("mollweide");

    handle.render_grid(target.clone(), base()).expect("renders");
    assert_eq!(handle.zarr_decodes_made(), 1, "the first draw decodes");
    each_colour_change("mollweide", |o| {
        handle.render_grid(target.clone(), o()).expect("renders");
    });
    handle
        .probe_pixel(target.clone(), base(), 10, 10)
        .expect("probes");
    handle
        .project_contours(target.clone(), base(), None)
        .expect("contours");
    handle
        .export_csv(target.clone(), "matrix".into())
        .expect("exports");
    assert_eq!(
        handle.zarr_decodes_made(),
        1,
        "no repaint, probe, contour or export reads the slice again"
    );

    // The picture is the one a handle that has decoded nothing yet paints.
    let fresh = crate::Handle::from_store(STORE.to_string()).expect("the fixture is a store");
    let drawn = handle.render_grid(target.clone(), base()).expect("renders");
    let full = fresh.render_grid(target.clone(), base()).expect("renders");
    assert!(drawn.rgba.as_ref() == full.rgba.as_ref());

    // A combined map reads each of its two slices once, and `a` is kept.
    let combined = json!({ "a": target, "b": target, "op": "a_minus_b" });
    handle
        .render_grid(combined.clone(), base())
        .expect("a combined map renders");
    assert_eq!(handle.zarr_decodes_made(), 1, "both slices were kept");
    handle
        .render_grid(
            combined,
            RenderOptions {
                colormap: Some("turbo".into()),
                ..base()
            },
        )
        .expect("renders");
    assert_eq!(handle.zarr_decodes_made(), 1);
    assert_eq!(handle.plans_made(), 1, "and the warp is shared throughout");
}

/// The store keeps the geometry it placed a slice on, so the warp memo
/// recognises it by address rather than by comparing it.
#[test]
fn a_store_slice_is_served_with_the_same_placement_each_time() {
    let store = crate::ZarrStore::from_directory(STORE.to_string()).expect("a store");
    let (variable, y, x, idx) = a_drawable_slice(&store);
    let first = store.slice(variable, y, x, &idx).expect("decodes");
    let again = store.slice(variable, y, x, &idx).expect("kept");
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    assert!(std::sync::Arc::ptr_eq(
        &first.placed.geometry,
        &again.placed.geometry
    ));
    // The whole field is the decoded one, geometry back in place.
    assert_eq!(first.field().georef.geometry, *first.placed.geometry);
}

/// The slice memo is bounded like the plane memo: at most `ENTRIES` slices and
/// `CELLS` cells, the newest always kept, the least recently used going first.
#[test]
fn the_slice_memo_is_bounded_in_slices_and_cells() {
    let store = crate::ZarrStore::from_directory(STORE.to_string()).expect("a store");
    let (variable, y, x, idx) = a_drawable_slice(&store);
    let slice = store.slice(variable, y, x, &idx).expect("decodes");
    let key = |variable| crate::PlaneKey {
        variable,
        y_dim: 0,
        x_dim: 1,
        fixed: vec![0, 0],
    };
    let mut memo = crate::SliceMemo::default();
    for k in 0..10 {
        memo.insert(key(k), slice.clone());
    }
    let held: Vec<usize> = memo.entries.iter().map(|(k, _)| k.variable).collect();
    assert_eq!(held, [6, 7, 8, 9]);
    assert!(memo.get(&key(6)).is_some(), "a hit moves to the back");
    memo.insert(key(10), slice.clone());
    let held: Vec<usize> = memo.entries.iter().map(|(k, _)| k.variable).collect();
    assert_eq!(held, [8, 9, 6, 10]);
}

/// A combined map whose two slices together exceed the cell budget decodes each
/// of them once across repeated colour changes, probes, contours and zonal
/// means (#1319). The budget is shrunk below one slice, so the pair is over it
/// as an RTOFS pair is over the real one.
#[test]
fn a_combined_pair_over_the_cell_budget_decodes_each_slice_once() {
    let handle = crate::Handle::from_store(STORE.to_string()).expect("a store");
    handle.set_memo_cells(1);
    let targets = slice_targets(&handle);
    assert!(targets.len() >= 2, "two drawable variables: {targets:?}");
    let combined = json!({ "a": targets[0], "b": targets[1], "op": "a_plus_b" });

    handle
        .render_grid(combined.clone(), opts("mollweide"))
        .expect("renders");
    assert_eq!(handle.zarr_decodes_made(), 2, "each slice decodes once");
    each_colour_change("mollweide", |o| {
        handle.render_grid(combined.clone(), o()).expect("renders");
    });
    for (px, py) in [(10, 10), (40, 30), (70, 50)] {
        handle
            .probe_pixel(combined.clone(), opts("mollweide"), px, py)
            .expect("probes");
    }
    handle
        .project_contours(combined.clone(), opts("mollweide"), None)
        .expect("contours");
    handle
        .zonal_mean(combined.clone())
        .expect("a zonal mean of the pair");
    assert_eq!(
        handle.zarr_decodes_made(),
        2,
        "no colour change, probe, contour or zonal mean reads either slice again"
    );
}

/// A slice outside the pair frees it for eviction: the exemption lasts while the
/// combined map is the one on screen (#1319).
#[test]
fn a_plain_slice_releases_the_combined_pair_to_the_budget() {
    let dir = three_dimensional_store();
    let handle = crate::Handle::from_store(dir.to_string_lossy().into_owned()).expect("a store");
    handle.set_memo_cells(1);
    let at =
        |time: u32| json!({ "variable": 0, "yDim": 1, "xDim": 2, "sliceIndices": [time, 0, 0] });
    let combined = json!({ "a": at(0), "b": at(1), "op": "a_minus_b" });
    handle
        .render_grid(combined.clone(), opts("source"))
        .expect("renders");
    handle
        .probe_pixel(combined.clone(), opts("source"), 0, 0)
        .expect("probes");
    assert_eq!(handle.zarr_decodes_made(), 2);
    probe_value(&handle, &at(2));
    assert_eq!(handle.zarr_decodes_made(), 3);
    probe_value(&handle, &at(0));
    assert_eq!(
        handle.zarr_decodes_made(),
        4,
        "a plain slice is held to the budget again"
    );
    drop(handle);
    let _ = std::fs::remove_dir_all(dir);
}

/// The plain zonal mean of a slice on screen reads the kept slice (#1319): it
/// is one more read of the memo and decodes nothing. Counted by the memo's
/// reads, because a zonal mean that decoded the slice itself would leave the
/// decode counter alone.
#[test]
fn the_plain_zonal_mean_reads_the_kept_slice() {
    let handle = crate::Handle::from_store(STORE.to_string()).expect("a store");
    let targets = slice_targets(&handle);
    handle
        .render_grid(targets[0].clone(), opts("mollweide"))
        .expect("renders");
    let (_, hits) = handle.memo_state();
    let first = handle.zonal_mean(targets[0].clone()).expect("a zonal mean");
    let (_, after) = handle.memo_state();
    assert_eq!(after, hits + 1, "the zonal mean read the kept slice");
    assert_eq!(handle.zarr_decodes_made(), 1);
    // And it is the zonal mean of the field the session decodes, which no
    // memo touched: the engine's own placement (`Field::source`), not the one
    // the kept slice rebuilt.
    let store = crate::ZarrStore::from_directory(STORE.to_string()).expect("a store");
    let (variable, y_dim, x_dim, indices) = a_drawable_slice(&store);
    let field = store
        .session
        .decode_slice(
            variable,
            y_dim,
            x_dim,
            &indices,
            &fieldglass::DecodeOptions::default(),
        )
        .expect("the slice decodes");
    let oracle = fieldglass::render::zonal_mean(
        &field.source(),
        &field_values(&field),
        field.parameter.as_deref(),
        field.units.as_deref(),
    )
    .map(LineResult::from)
    .expect("a zonal mean");
    assert_eq!(format!("{first:?}"), format!("{oracle:?}"));
}

/// The pin follows the combined map on screen (#1319): a probe of A or B keeps
/// it, a plain render of either, or a read of a slice outside the pair, releases
/// it, and a combine that is refused leaves none behind.
#[test]
fn the_pin_follows_the_combined_map_on_screen() {
    let dir = three_dimensional_store();
    let handle = crate::Handle::from_store(dir.to_string_lossy().into_owned()).expect("a store");
    let at =
        |time: u32| json!({ "variable": 0, "yDim": 1, "xDim": 2, "sliceIndices": [time, 0, 0] });
    let combined = json!({ "a": at(0), "b": at(1), "op": "a_minus_b" });
    let pinned = || handle.memo_state().0;

    handle
        .render_grid(combined.clone(), opts("source"))
        .expect("renders");
    assert_eq!(pinned(), 2);
    probe_value(&handle, &at(0));
    probe_value(&handle, &at(1));
    assert_eq!(pinned(), 2, "a probe of A or B keeps the pin");
    handle.render_grid(at(0), opts("source")).expect("renders");
    assert_eq!(pinned(), 0, "a plain render of A releases it");
    handle
        .render_grid(combined.clone(), opts("source"))
        .expect("renders");
    assert_eq!(pinned(), 2);
    probe_value(&handle, &at(2));
    assert_eq!(pinned(), 0, "a slice outside the pair releases it");

    // A pair that is refused (B names no variable) is not on screen.
    let refused = json!({ "a": at(0), "b": { "variable": 99, "yDim": 1, "xDim": 2, "sliceIndices": [0, 0, 0] }, "op": "a_minus_b" });
    assert!(handle.render_grid(refused, opts("source")).is_err());
    assert_eq!(pinned(), 0, "a refused combine leaves no pin");
    drop(handle);
    let _ = std::fs::remove_dir_all(dir);
}

/// The NetCDF handle keeps both planes of a combined map that together are over
/// its budget (#1319), as the Zarr handle does: two RTOFS ice planes each over
/// half the real 16 M-cell budget evicted one another on every colour change
/// and probe. The budget is shrunk below one plane here. Counted with the plane
/// memo's inserts, which is one a decode.
#[test]
fn a_netcdf_pair_over_the_cell_budget_decodes_each_plane_once() {
    let handle = Handle::from_vec(RTOFS.to_vec()).expect("RTOFS opens");
    handle.set_memo_cells(1);
    let targets = slice_targets(&handle);
    let combined = json!({ "a": targets[0], "b": targets[1], "op": "a_plus_b" });

    handle
        .render_grid(combined.clone(), opts("mollweide"))
        .expect("renders");
    assert_eq!(handle.netcdf_decodes_made(), 2, "each plane decodes once");
    // The sum is negative, so no log scale: a colour map and a reversal.
    for reverse in [false, true] {
        let o = RenderOptions {
            colormap: Some("turbo".into()),
            reverse_colormap: Some(reverse),
            ..opts("mollweide")
        };
        handle.render_grid(combined.clone(), o).expect("renders");
    }
    for (px, py) in [(10, 10), (40, 30), (70, 50)] {
        handle
            .probe_pixel(combined.clone(), opts("mollweide"), px, py)
            .expect("probes");
    }
    handle
        .project_contours(combined.clone(), opts("mollweide"), None)
        .expect("contours");
    // A curvilinear mesh has no zonal mean; the refusal comes after the planes
    // are read, which is what is counted.
    let _ = handle.zonal_mean(combined);
    assert_eq!(
        handle.netcdf_decodes_made(),
        2,
        "no colour change, probe, contour or zonal mean reads either plane again"
    );
    // A plain render releases the pair.
    handle
        .render_grid(targets[2].clone(), opts("mollweide"))
        .expect("renders");
    assert_eq!(handle.memo_state().0, 0);
}

/// A combined pair's values, name and units are what `Session::combine` over
/// the two whole fields gives (#1319). That the pair shares one geometry
/// (`ptr_eq`) has held since #1273; the combine builds no whole field, and what
/// guards that is `ZarrSlice::field` being compiled for tests only.
#[test]
fn a_combined_pair_shares_a_geometry_and_matches_the_session_combine() {
    let store = crate::ZarrStore::from_directory(STORE.to_string()).expect("a store");
    let drawable: Vec<_> = store
        .variables()
        .iter()
        .filter_map(|v| {
            Some((
                u32::try_from(v.variable_index).ok()?,
                u32::try_from(v.detected_y_dim?).ok()?,
                u32::try_from(v.detected_x_dim?).ok()?,
                vec![0; v.dims.len()],
            ))
        })
        .collect();
    let (a, b) = (&drawable[0], &drawable[1]);
    let (first, values) = store
        .combined(a.0, a.1, a.2, &a.3, b.0, &b.3, "a_minus_b")
        .expect("combines");
    let second = store.slice(b.0, a.1, a.2, &b.3).expect("kept");
    assert!(
        std::sync::Arc::ptr_eq(&first.placed.geometry, &second.placed.geometry),
        "one geometry, so the gate compares by address"
    );
    let whole = store
        .session
        .combine(
            &first.field(),
            &second.field(),
            fieldglass::op_from_wire("a_minus_b").expect("an op"),
        )
        .expect("aligned");
    assert_eq!(values, field_values(&whole));
    assert_eq!(first.rest.parameter, whole.parameter);
    assert_eq!(first.rest.units, whole.units);
    assert_eq!(store.decodes.load(std::sync::atomic::Ordering::Relaxed), 2);
}

/// The cell budgets, and the eviction that enforces them (#1319). The budget is
/// shrunk for the walk-through, so no test holds millions of cells; the real
/// budgets are asserted by value.
#[test]
fn the_memos_evict_past_their_cell_budget() {
    assert_eq!(crate::SliceMemo::CELLS, 8 << 20);
    assert_eq!(crate::PlaneMemo::CELLS, 16 << 20);
    assert_eq!(crate::SliceMemo::default().cells, crate::SliceMemo::CELLS);
    assert_eq!(crate::PlaneMemo::default().cells, crate::PlaneMemo::CELLS);

    let sized = |cells: usize| std::sync::Arc::new(vec![None; cells]);
    let key = |variable| crate::PlaneKey {
        variable,
        y_dim: 0,
        x_dim: 1,
        fixed: vec![0, 0],
    };
    let held = |memo: &crate::PlaneMemo| -> Vec<usize> {
        memo.entries.iter().map(|(k, _)| k.variable).collect()
    };
    let fresh = || crate::PlaneMemo {
        cells: 10,
        ..Default::default()
    };

    let mut memo = fresh();
    memo.insert(key(0), sized(5));
    memo.insert(key(1), sized(5));
    assert_eq!(held(&memo), [0, 1], "exactly the budget fits");
    memo.insert(key(2), sized(1));
    assert_eq!(held(&memo), [1, 2], "the oldest went");
    memo.insert(key(3), sized(11));
    assert_eq!(held(&memo), [3], "one entry over the budget is kept, alone");

    // Without a pin two entries over half the budget push one another out.
    let mut memo = fresh();
    memo.insert(key(0), sized(6));
    memo.insert(key(1), sized(6));
    assert_eq!(held(&memo), [1]);
    // A pinned pair survives the budget; an entry outside it releases the pin.
    let mut memo = fresh();
    memo.pin(&key(0), &key(1));
    memo.insert(key(0), sized(6));
    memo.insert(key(1), sized(6));
    assert_eq!(held(&memo), [0, 1], "the pair is exempt");
    memo.unpin_unless(&key(0));
    assert!(!memo.pair.is_empty(), "a member of the pair keeps the pin");
    memo.unpin_unless(&key(2));
    assert!(memo.pair.is_empty());
    memo.insert(key(2), sized(1));
    assert_eq!(held(&memo), [1, 2]);
}

/// The value under the top-left pixel of the source view of a slice.
fn probe_value(handle: &crate::Handle, target: &serde_json::Value) -> Option<f64> {
    handle
        .probe_pixel(target.clone(), opts("source"), 0, 0)
        .expect("probes")
        .expect("on the grid")
        .value
}

/// Another slice on the same grid is a new decode with its own values, the
/// memo does not answer one slice with another, and the two share one geometry
/// so the warp memo keeps recognising it by address (#1273 review).
#[test]
fn another_variable_on_the_grid_decodes_once_and_shares_the_geometry() {
    let handle = crate::Handle::from_store(STORE.to_string()).expect("a store");
    let targets = slice_targets(&handle);
    assert!(targets.len() >= 2, "two drawable variables: {targets:?}");
    let (a, t) = (&targets[0], &targets[1]);

    let va = probe_value(&handle, a);
    assert_eq!(handle.zarr_decodes_made(), 1);
    let vt = probe_value(&handle, t);
    assert_eq!(handle.zarr_decodes_made(), 2, "another variable decodes");
    assert_ne!(va, vt, "and answers with its own values");
    assert_eq!(probe_value(&handle, a), va);
    assert_eq!(probe_value(&handle, t), vt);
    assert_eq!(handle.zarr_decodes_made(), 2, "both are kept");

    // A combined map is the one a fresh handle paints.
    let combined = json!({ "a": a, "b": t, "op": "a_minus_b" });
    let fresh = crate::Handle::from_store(STORE.to_string()).expect("a store");
    let drawn = handle
        .render_grid(combined.clone(), opts("mollweide"))
        .expect("renders");
    let full = fresh
        .render_grid(combined, opts("mollweide"))
        .expect("renders");
    assert!(drawn.rgba.as_ref() == full.rgba.as_ref());
    assert_eq!(handle.zarr_decodes_made(), 2);

    // Same grid, one geometry.
    let store = crate::ZarrStore::from_directory(STORE.to_string()).expect("a store");
    let drawable: Vec<_> = store
        .variables()
        .iter()
        .filter_map(|v| {
            Some((
                u32::try_from(v.variable_index).ok()?,
                u32::try_from(v.detected_y_dim?).ok()?,
                u32::try_from(v.detected_x_dim?).ok()?,
                vec![0; v.dims.len()],
            ))
        })
        .collect();
    let first = store
        .slice(drawable[0].0, drawable[0].1, drawable[0].2, &drawable[0].3)
        .expect("decodes");
    let second = store
        .slice(drawable[1].0, drawable[1].1, drawable[1].2, &drawable[1].3)
        .expect("decodes");
    assert!(!std::sync::Arc::ptr_eq(&first, &second));
    assert!(
        std::sync::Arc::ptr_eq(&first.placed.geometry, &second.placed.geometry),
        "a slice on a kept slice's grid shares its geometry"
    );
}

/// A v3 store with one `v(time, y, x)` array of 3 x 2 x 3 float32, value
/// `100 * time + 3 * y + x + 1`, one chunk a time step, in a fresh directory.
fn three_dimensional_store() -> std::path::PathBuf {
    // Under the workspace target directory, not a shared temp directory.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join(format!("fieldglass-1273-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("v/c")).expect("a directory");
    std::fs::write(
        dir.join("zarr.json"),
        r#"{"zarr_format":3,"node_type":"group","attributes":{}}"#,
    )
    .expect("writes");
    std::fs::write(
        dir.join("v/zarr.json"),
        r#"{"zarr_format":3,"node_type":"array","shape":[3,2,3],"data_type":"float32",
"chunk_grid":{"name":"regular","configuration":{"chunk_shape":[1,2,3]}},
"chunk_key_encoding":{"name":"default","configuration":{"separator":"/"}},
"fill_value":0,"codecs":[{"name":"bytes","configuration":{"endian":"little"}}],
"dimension_names":["time","y","x"],"attributes":{}}"#,
    )
    .expect("writes");
    for time in 0..3u32 {
        std::fs::create_dir_all(dir.join(format!("v/c/{time}/0"))).expect("a directory");
        let bytes: Vec<u8> = (0..6u32)
            .flat_map(|k| ((100 * time + k + 1) as f32).to_le_bytes())
            .collect();
        std::fs::write(dir.join(format!("v/c/{time}/0/0")), bytes).expect("writes");
    }
    dir
}

/// The memo's key: two fixed indices are two slices, and the drawn axes'
/// indices are not part of it (#1273 review).
#[test]
fn the_memo_key_tells_fixed_indices_apart_and_ignores_the_drawn_axes() {
    let dir = three_dimensional_store();
    let handle = crate::Handle::from_store(dir.to_string_lossy().into_owned()).expect("a store");
    let at = |time: u32, y: u32, x: u32| json!({ "variable": 0, "yDim": 1, "xDim": 2, "sliceIndices": [time, y, x] });
    let v0 = probe_value(&handle, &at(0, 0, 0));
    assert_eq!(handle.zarr_decodes_made(), 1);
    let v1 = probe_value(&handle, &at(1, 0, 0));
    assert_eq!(handle.zarr_decodes_made(), 2, "another time step decodes");
    assert_ne!(v0, v1);
    assert_eq!(v1.map(|v| v - v0.unwrap()), Some(100.0));
    // The same time step with other indices on the drawn axes is the same slice.
    assert_eq!(probe_value(&handle, &at(0, 1, 2)), v0);
    assert_eq!(probe_value(&handle, &at(1, 1, 1)), v1);
    assert_eq!(
        handle.zarr_decodes_made(),
        2,
        "the drawn axes are not the key"
    );
    drop(handle);
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------
// Probing a grid already drawn (#1313)
// ---------------------------------------------------------------------------

/// The pixels a probe test reads: a spread over the raster `w × h`, its four
/// corners, and a pixel past two of its edges.
fn probe_pixels(w: u32, h: u32) -> Vec<(u32, u32)> {
    let mut pixels = vec![
        (0, 0),
        (w - 1, 0),
        (0, h - 1),
        (w - 1, h - 1),
        (w, 0),
        (0, h),
    ];
    for a in 1..8 {
        for b in 1..8 {
            pixels.push((w * a / 8, h * b / 8));
        }
    }
    pixels
}

/// A probe of `target` under `o()` after a draw of it reads the kept plan,
/// makes no plan of its own, and answers each of `probe_pixels` as a handle
/// that has drawn nothing (and so resolves the pixel afresh) does, to the
/// last field.
fn assert_probes_as_uncached(
    fresh: impl Fn() -> Handle,
    target: &serde_json::Value,
    o: impl Fn() -> RenderOptions,
) {
    let drawn = fresh();
    let uncached = fresh();
    let image = drawn.render_grid(target.clone(), o()).expect("renders");
    let (plans, served) = (drawn.plans_made(), drawn.probes_served());
    let pixels = probe_pixels(image.width as u32, image.height as u32);
    for &(px, py) in &pixels {
        let kept = drawn
            .probe_pixel(target.clone(), o(), px, py)
            .expect("probes");
        let plain = uncached
            .probe_pixel(target.clone(), o(), px, py)
            .expect("probes");
        assert_eq!(
            format!("{kept:?}"),
            format!("{plain:?}"),
            "{target} under {:?} at ({px}, {py})",
            o()
        );
    }
    assert_eq!(drawn.plans_made(), plans, "a probe plans nothing");
    assert_eq!(
        drawn.probes_served() - served,
        pixels.len() as u64,
        "every probe of the drawn grid reads the kept plan"
    );
    assert_eq!(
        (uncached.plans_made(), uncached.probes_served()),
        (0, 0),
        "the handle that drew nothing has no plan to read"
    );
}

#[test]
fn a_probe_of_a_curvilinear_slice_reads_the_kept_plan() {
    let fresh = || Handle::from_vec(RTOFS.to_vec()).expect("RTOFS opens");
    let targets = slice_targets(&fresh());
    for projection in [
        "equirectangular",
        "mollweide",
        "orthographic",
        "polar_stereographic",
    ] {
        assert_probes_as_uncached(fresh, &targets[0], || opts(projection));
    }
    // Flipped, and asking for a blend the grid cannot do.
    assert_probes_as_uncached(fresh, &targets[0], || RenderOptions {
        flip_y: true,
        resampling: "bilinear".into(),
        ..opts("equirectangular")
    });
    let combined = json!({ "a": targets[1], "b": targets[2], "op": "a_minus_b" });
    assert_probes_as_uncached(fresh, &combined, || opts("equirectangular"));
}

#[test]
fn a_probe_of_a_regular_grid_reads_the_kept_plan() {
    let fresh = || Handle::from_vec(LAMBERT.to_vec()).expect("HRRR opens");
    for projection in ["equirectangular", "orthographic", "robinson"] {
        assert_probes_as_uncached(fresh, &json!(0), || opts(projection));
    }
    // A bilinear plan keeps the fractional index, not the cell.
    assert_probes_as_uncached(fresh, &json!(0), || RenderOptions {
        resampling: "bilinear".into(),
        ..opts("equirectangular")
    });
    let combined = json!({ "a": 0, "b": 0, "op": "a_minus_b" });
    assert_probes_as_uncached(fresh, &combined, || opts("mollweide"));
}

#[test]
fn a_probe_of_a_store_slice_reads_the_kept_plan() {
    let fresh = || crate::Handle::from_store(STORE.to_string()).expect("a store");
    let targets = slice_targets(&fresh());
    assert_probes_as_uncached(fresh, &targets[0], || opts("mollweide"));
    assert_probes_as_uncached(fresh, &targets[0], || opts("equirectangular"));
}

/// A probe the kept plan does not serve resolves the pixel as it always did,
/// and keeps the plan it has rather than making one.
#[test]
fn a_probe_the_plan_does_not_serve_falls_back_and_plans_nothing() {
    let fresh = || Handle::from_vec(RTOFS.to_vec()).expect("RTOFS opens");
    let handle = fresh();
    let uncached = fresh();
    let target = slice_targets(&handle).remove(0);
    handle
        .render_grid(target.clone(), opts("equirectangular"))
        .expect("renders");
    let others = || {
        vec![
            opts("mollweide"),
            RenderOptions {
                bounds_lat_min: Some(60.0),
                bounds_lat_max: Some(80.0),
                bounds_lon_min: Some(-60.0),
                bounds_lon_max: Some(60.0),
                ..opts("equirectangular")
            },
            RenderOptions {
                width: Some(300),
                height: Some(200),
                ..opts("equirectangular")
            },
            opts("source"),
        ]
    };
    for k in 0..others().len() {
        for (px, py) in [(5, 5), (100, 60), (299, 199)] {
            let kept = handle
                .probe_pixel(target.clone(), others().swap_remove(k), px, py)
                .expect("probes");
            let plain = uncached
                .probe_pixel(target.clone(), others().swap_remove(k), px, py)
                .expect("probes");
            assert_eq!(
                format!("{kept:?}"),
                format!("{plain:?}"),
                "{:?} at ({px}, {py})",
                others()[k]
            );
        }
    }
    assert_eq!(
        handle.probes_served(),
        0,
        "none of them was the drawn window"
    );
    assert_eq!(handle.plans_made(), 1, "and none planned");
    // The drawn window is still the kept one.
    handle
        .probe_pixel(target, opts("equirectangular"), 5, 5)
        .expect("probes");
    assert_eq!(handle.probes_served(), 1);
}

/// A global grid, which is periodic in longitude, with a bilinear plan (the
/// plan keeps the fractional index) wraps at the seam as the uncached probe
/// does.
#[test]
fn a_probe_across_the_seam_of_a_global_grid_reads_the_kept_plan() {
    const GLOBAL: &[u8] =
        include_bytes!("../../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2");
    let fresh = || Handle::from_vec(GLOBAL.to_vec()).expect("GFS opens");
    for projection in ["equirectangular", "mollweide"] {
        assert_probes_as_uncached(fresh, &json!(0), || RenderOptions {
            resampling: "bilinear".into(),
            ..opts(projection)
        });
    }
}

/// The plan holds cells, never values: a probe of another field on the grid
/// that was drawn reads that field's values.
#[test]
fn a_probe_of_another_field_reads_its_own_values_through_the_plan() {
    let fresh = || Handle::from_vec(RTOFS.to_vec()).expect("RTOFS opens");
    let drawn = fresh();
    let uncached = fresh();
    let targets = slice_targets(&drawn);
    let o = || opts("equirectangular");
    let image = drawn.render_grid(targets[0].clone(), o()).expect("renders");
    let mut differs = 0;
    for (px, py) in probe_pixels(image.width as u32, image.height as u32) {
        let kept = drawn
            .probe_pixel(targets[1].clone(), o(), px, py)
            .expect("probes");
        let plain = uncached
            .probe_pixel(targets[1].clone(), o(), px, py)
            .expect("probes");
        assert_eq!(format!("{kept:?}"), format!("{plain:?}"), "({px}, {py})");
        let first = drawn
            .probe_pixel(targets[0].clone(), o(), px, py)
            .expect("probes");
        differs += usize::from(
            kept.as_ref().and_then(|p| p.value) != first.as_ref().and_then(|p| p.value),
        );
    }
    assert!(differs > 0, "the two fields differ somewhere probed");
    assert_eq!(drawn.plans_made(), 1);
}

#[test]
fn a_combined_probe_of_a_store_reads_the_kept_plan() {
    let fresh = || crate::Handle::from_store(STORE.to_string()).expect("a store");
    let targets = slice_targets(&fresh());
    let combined = json!({ "a": targets[0], "b": targets[1], "op": "a_minus_b" });
    assert_probes_as_uncached(fresh, &combined, || opts("mollweide"));
}
