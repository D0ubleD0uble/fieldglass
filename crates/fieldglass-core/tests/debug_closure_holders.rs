//! The two `Debug` impls in this crate that are written out rather than derived.
//!
//! `SourceGrid` and `SourceOverlayTarget` each hold a `dyn Fn`, which has no
//! `Debug`, so `#[derive(Debug)]` does not compile for them and the impl is
//! hand-written. A hand-written one can print the wrong field, or the wrong
//! number of them, and nothing else in the workspace would notice — the
//! equivalent impl on a napi DTO named four fields that did not exist while
//! being written. So the contract is pinned here: the plain fields print with
//! their values, and each closure prints as a placeholder rather than being
//! elided from the output altogether.
//!
//! Printing a *placeholder* rather than dropping the field is the part worth
//! holding. `SourceGrid`'s two closures are where a warp actually gets its
//! values and its geometry from; a `Debug` that silently omitted them would
//! show a struct that looks complete and is not.

// `warp` and `overlay` live behind the `render` feature, and the format crates
// take `core` with `default-features = false`. The crate doc sits above this so
// that `missing_docs` still sees one when the `cfg` strips the rest.
#![cfg(feature = "render")]

use fieldglass_core::projection::{GridIndex, GridResampling};
use fieldglass_core::warp::SourceGrid;
use fieldglass_core::{SourceOverlayTarget, TargetRaster};

#[test]
fn source_grid_prints_its_geometry_and_marks_its_closures() {
    let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    let sample = |i: usize, j: usize| values.get(j * 3 + i).copied();
    let inverse = |lat: f64, lon: f64| {
        Some(GridIndex {
            i: lon / 10.0,
            j: lat / 10.0,
        })
    };
    let grid = SourceGrid {
        ni: 3,
        nj: 2,
        sample: &sample,
        inverse_at: &inverse,
        periodic_i: true,
        resampling: GridResampling::NearestOnly,
    };

    let printed = format!("{grid:?}");

    // The struct names itself, so a `Debug` copied from a neighbouring type
    // (the way these two were written) does not pass by printing the fields of
    // one under the name of the other.
    assert!(
        printed.starts_with("SourceGrid {"),
        "should name its own type: {printed}"
    );
    // Every plain field, with the value it holds — not just the field name.
    assert!(printed.contains("ni: 3"), "{printed}");
    assert!(printed.contains("nj: 2"), "{printed}");
    assert!(printed.contains("periodic_i: true"), "{printed}");
    assert!(printed.contains("resampling: NearestOnly"), "{printed}");
    // Both closures are present as placeholders. A dropped field would leave a
    // struct that reads as complete while hiding where the values come from.
    assert!(printed.contains("sample: \"<closure>\""), "{printed}");
    assert!(printed.contains("inverse_at: \"<closure>\""), "{printed}");
}

#[test]
fn source_grid_debug_follows_the_values_it_is_given() {
    // The same shape twice, differing only in the two scalar fields, so a
    // `Debug` that printed constants instead of `self` would fail here while
    // passing the test above.
    let sample = |_: usize, _: usize| None;
    let inverse = |_: f64, _: f64| None;
    let build = |ni, nj, periodic| SourceGrid {
        ni,
        nj,
        sample: &sample,
        inverse_at: &inverse,
        periodic_i: periodic,
        resampling: GridResampling::Any,
    };

    let a = format!("{:?}", build(7, 11, false));
    let b = format!("{:?}", build(13, 17, true));

    assert!(a.contains("ni: 7") && a.contains("nj: 11"), "{a}");
    assert!(a.contains("periodic_i: false"), "{a}");
    assert!(b.contains("ni: 13") && b.contains("nj: 17"), "{b}");
    assert!(b.contains("periodic_i: true"), "{b}");
    assert_ne!(a, b, "the two grids must not print identically");
}

#[test]
fn source_overlay_target_prints_its_only_field_as_a_placeholder() {
    let inverse = |lat: f64, lon: f64| Some(GridIndex { i: lon, j: lat });
    let target = SourceOverlayTarget::new(&inverse);

    let printed = format!("{target:?}");

    assert!(
        printed.starts_with("SourceOverlayTarget {"),
        "should name its own type: {printed}"
    );
    // Its only field is the closure. Eliding it would print an empty struct,
    // which says nothing about what the target actually wraps.
    assert!(printed.contains("inverse: \"<closure>\""), "{printed}");
}

#[test]
fn the_derived_neighbours_still_print_their_values() {
    // `TargetRaster` sits beside `SourceGrid` in the same call and does derive
    // `Debug`. Asserting it here is what makes the two impls above comparable:
    // a reader can see that the hand-written ones are meant to read the same
    // way a derived one does, closures aside.
    let raster = TargetRaster {
        width: 4,
        height: 2,
        lat_max: 60.0,
        lat_min: 30.0,
        lon_min: -10.0,
        lon_max: 10.0,
        lon_periodic: false,
    };

    let printed = format!("{raster:?}");

    assert!(printed.starts_with("TargetRaster {"), "{printed}");
    assert!(printed.contains("width: 4"), "{printed}");
    assert!(printed.contains("lat_max: 60.0"), "{printed}");
    assert!(printed.contains("lon_periodic: false"), "{printed}");
    assert!(printed.contains("lon_min: -10.0"), "{printed}");
}
