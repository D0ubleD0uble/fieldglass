//! Contours on a lookup grid's source view follow the traced, fractional grid
//! positions (#1447).
//!
//! A lookup grid (here NetCDF 2-D coordinate variables) answers `(lat, lon)`
//! with the nearest whole cell. Contours used to be geolocated and sent back
//! through that inverse, like the coastlines, so on the source view every
//! vertex landed on a cell center: up to half a cell from where the tracer put
//! it, and a quarter to a third of all segments shrank to a point. The source
//! view paints cell `(i, j)` at pixel `(i, j)`, so the traced vertices are
//! already its pixels and now go there directly.
//!
//! Both fixtures go through `render::contour_polylines` and
//! `Session::contour_polylines`, the calls the hosts make, and every vertex is
//! compared with the tracer's own.

#![cfg(all(feature = "netcdf", feature = "render", feature = "analysis"))]

use fieldglass::{DecodeOptions, RenderOptions, Session};

/// A MiRS swath: 96 fields of view by 100 scanlines.
const MIRS: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/mirs_swath_n21.nc");
/// A 260 × 200 window of the RTOFS tripolar mesh.
const RTOFS: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc");

/// A variable's first slice on its own axes, with its mask applied.
fn decoded(session: &Session, name: &str) -> (fieldglass::Field, Vec<Option<f64>>) {
    let v = session
        .variables()
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("{name} is listed"))
        .clone();
    let (y, x) = (
        v.detected_y_dim.expect("a y axis"),
        v.detected_x_dim.expect("an x axis"),
    );
    let field = session
        .decode_slice(
            v.index,
            y,
            x,
            &vec![0; v.dims.len()],
            &DecodeOptions::default(),
        )
        .expect("the slice decodes");
    let dense: Vec<f64> = match &field.values {
        fieldglass::Values::F64(v) => v.clone(),
        fieldglass::Values::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
        other => panic!("a values width this test does not read: {other:?}"),
    };
    let values = field
        .mask
        .iter()
        .zip(dense)
        .map(|(&m, v)| (m == 1).then_some(v))
        .collect();
    (field, values)
}

/// The tracer's segments over the levels `contour_polylines` picks by default,
/// rows flipped when the source view flips them, and each vertex drawn in the
/// middle of its cell's pixel, half a pixel on from its grid index (#1464).
fn traced(values: &[Option<f64>], ni: u32, nj: u32, flip: bool) -> Vec<[f64; 4]> {
    let (lo, hi) = fieldglass::min_max_ignoring_mask(values.iter().copied())
        .expect("the field has a present cell");
    let levels = fieldglass_core::nice_levels(lo, hi, fieldglass::AUTO_CONTOUR_LEVELS);
    let row = |j: f64| (if flip { f64::from(nj) - 1.0 - j } else { j }) + 0.5;
    fieldglass_core::contour_segments(values, ni as usize, nj as usize, &levels)
        .iter()
        .flat_map(|level| &level.segments)
        .map(|[(i0, j0), (i1, j1)]| [*i0 + 0.5, row(*j0), *i1 + 0.5, row(*j1)])
        .collect()
}

/// Every vertex `out` draws is the traced one, and no segment is lost.
fn assert_follows_tracer(what: &str, out: &fieldglass::ProjectedPolylines, want: &[[f64; 4]]) {
    assert!(
        !want.is_empty(),
        "{what}: the field has contours to compare"
    );
    assert!(
        out.seg_lengths.iter().all(|&n| n == 2),
        "{what}: each contour segment is its own run"
    );
    assert_eq!(out.seg_lengths.len(), want.len(), "{what}: segments drawn");
    let (mut worst, mut at) = (0.0f64, 0);
    for (k, (got, want)) in out.xy.as_chunks::<4>().0.iter().zip(want).enumerate() {
        let off = (0..4).map(|c| (got[c] - want[c]).abs()).fold(0.0, f64::max);
        if off > worst {
            (worst, at) = (off, k);
        }
    }
    assert!(
        worst < 1e-9,
        "{what}: segment {at} is drawn at {:?}, {worst:.3} px from where it was traced \
         ({:?}); a vertex snapped to a cell center",
        out.xy.as_chunks::<4>().0[at],
        want[at]
    );
}

#[test]
fn source_view_contours_on_a_lookup_grid_keep_their_fractional_positions() {
    for (file, bytes, vars) in [
        ("mirs_swath_n21.nc", MIRS, ["TPW", "TSkin"]),
        (
            "rtofs_tripolar_arctic.nc",
            RTOFS,
            ["ice_coverage", "ice_thickness"],
        ),
    ] {
        let session = Session::open(bytes.to_vec()).expect("the fixture opens");
        for var in vars {
            let (field, values) = decoded(&session, var);
            let source = field.source();
            for requested_flip in [false, true] {
                let mut options = RenderOptions::new("source", "nearest");
                options.flip_y = requested_flip;
                let want = traced(
                    &values,
                    source.ni,
                    source.nj,
                    source.scan.flips_source_rows(requested_flip),
                );
                let what = format!("{file} {var} (flip_y {requested_flip})");
                let free = fieldglass::render::contour_polylines(&source, &values, &options, None)
                    .expect("contours project");
                assert_follows_tracer(&what, &free, &want);
                let kept = session
                    .contour_polylines(&source, &values, &options, None)
                    .expect("contours project");
                assert_eq!(kept, free, "{what}: Session and render disagree");
            }
        }
    }
}
