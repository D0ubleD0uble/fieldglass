//! `Session::decode` on the families that carry no raster of their own (#580).
//!
//! Spectral coefficients and HEALPix pixels are not values on a grid, so the
//! reader puts them on one and `Session` hands back an ordinary `"latlon"`
//! field. The point of the tests here is **the routing seam, not the handler**:
//! #330 is the precedent — synthesis worked and every operation except render
//! still took the raw path — so every operation a `Field` reaches is exercised,
//! not only the one that motivated the change.
//!
//! The synthesised *values* have their own oracles in the format crates
//! (ECMWF's definitive spectral formula, eccodes' HEALPix pixel centres). What
//! is pinned here is that the umbrella routes to them, places them where it
//! says it does, and keeps the message list describing the file.

use fieldglass::{DecodeOptions, Session, WarpOptions};
use fieldglass_core::GlobalGrid;

/// Every synthesis subject, with the grid its rule chooses.
///
/// Both GRIB editions' spectral path is here even though the conformance suite
/// carries only the GRIB2 one: the two are separate arms of `Session::decode`,
/// and 23 conformance cases apiece would cost several seconds of the workspace
/// test run for a second copy of the same answer.
const SUBJECTS: &[(&str, &str, (usize, usize))] = &[
    (
        "grib1 spectral (simple)",
        "../fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1",
        (720, 361),
    ),
    (
        "grib1 spectral (complex)",
        "../fieldglass-grib1/tests/fixtures/spectral_complex_t63.grib1",
        (720, 361),
    ),
    (
        "grib2 spectral (simple)",
        "../fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2",
        (720, 361),
    ),
    (
        "grib2 spectral (complex)",
        "../fieldglass-grib2/tests/fixtures/spectral_complex_t63.grib2",
        (720, 361),
    ),
    (
        // A different rule reaching a different grid: HEALPix samples at its
        // own pixel scale, so `Nside 4` is 26 × 14 rather than the spectral pin.
        "grib2 healpix (ring)",
        "../fieldglass-grib2/tests/fixtures/healpix_n4_ring.grib2",
        (26, 14),
    ),
    (
        "grib2 healpix (nested)",
        "../fieldglass-grib2/tests/fixtures/healpix_n2_nested.grib2",
        (14, 8),
    ),
];

fn session(path: &str) -> Session {
    Session::open(std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))).expect("open")
}

/// The field lands on the grid the convention states, corner for corner.
///
/// The eastern corner is the assertion that matters: it must be the last
/// longitude the field was *evaluated* at, so it is read back off
/// [`GlobalGrid`] rather than respelled here. Declaring `360` for a grid that
/// stops a step short makes the warp read one column twice, and nothing else in
/// this file would notice.
#[test]
fn a_synthesised_field_is_an_ordinary_latlon_field_on_the_declared_grid() {
    for &(name, path, (ni, nj)) in SUBJECTS {
        let session = session(path);
        let field = session
            .decode(0, &DecodeOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let grid = GlobalGrid::new(ni, nj);

        assert_eq!((field.ni as usize, field.nj as usize), (ni, nj), "{name}");
        assert_eq!(field.values.to_f64().len(), grid.len(), "{name}");
        assert_eq!(field.mask.len(), grid.len(), "{name}");
        assert_eq!(field.georef.kind, "latlon", "{name}");
        assert!(
            field.georef.periodic_x,
            "{name}: a global grid closes on itself"
        );
        assert_eq!(
            field.georef.bounds_lonlat,
            Some([-90.0, 90.0, 0.0, grid.lon_last()]),
            "{name}: the declared corner must be the last longitude the field \
             was evaluated at"
        );
        assert!(
            field.georef.bounds_lonlat.expect("just asserted")[3] < 360.0,
            "{name}: no duplicated wrap column"
        );
        // Whatever the source scanned like, a synthesised grid runs west-to-east
        // from 0° and north-down from the pole: nothing of the source layout
        // survives an inverse transform or a resample.
        //
        // **This assertion does not currently discriminate**, and the guard
        // below says so rather than leaving it to be discovered: every
        // synthesis fixture in the committed corpus already declares a
        // north-down scan (the GRIB1 and GRIB2 spectral messages state no
        // scanning mode at all, and both HEALPix ones state `0`), so replacing
        // the rule with the message's own scan passes this file, the
        // conformance suite and the display golden alike — measured, not
        // assumed. It is asserted anyway because it is the contract; the guard
        // is what turns red the day a fixture can tell the two apart.
        assert!(!field.georef.scan.j_positive, "{name}");
        assert!(!field.georef.scan.i_negative, "{name}");
        assert!(!field.georef.scan.j_consecutive, "{name}");
        assert!(field.stats.valid_count > 0, "{name}: the field has values");
    }
}

/// The guard on the assertion above: no committed synthesis fixture declares a
/// scan that differs from the synthesised one.
///
/// While this passes, "a synthesised field is north-down" is a statement of
/// intent rather than a discriminating check. When it fails, a fixture has
/// arrived that *can* tell the two apart — make the assertion above compare the
/// declared scan with the synthesised one on that fixture, and delete this.
#[test]
fn no_committed_synthesis_fixture_can_tell_the_scan_rule_apart_yet() {
    for &(name, path, _) in SUBJECTS {
        let declared = session(path)
            .message(0)
            .expect("message")
            .grid
            .expect("a declared grid")
            .scan;
        assert!(
            !declared.j_positive && !declared.i_negative && !declared.j_consecutive,
            "{name} declares {declared:?}, which differs from the synthesised \
             north-down scan — this fixture can now discriminate the rule, so \
             assert on it above rather than keeping this guard"
        );
    }
}

/// Every operation, not only the one that motivated the change (#330).
#[test]
fn every_operation_runs_on_a_synthesised_field_with_no_special_case() {
    for &(name, path, _) in SUBJECTS {
        let session = session(path);
        let field = session
            .decode(0, &DecodeOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));

        let warped = session
            .warp(&field, &WarpOptions::default())
            .unwrap_or_else(|e| panic!("{name}: warp: {e}"));
        assert_eq!(
            (warped.width, warped.height),
            (field.ni, field.nj),
            "{name}: warp"
        );
        assert!(
            warped.mask.contains(&1),
            "{name}: the warp landed no cells, so it proves nothing"
        );

        let raster = session
            .render(&field, &Default::default(), false)
            .unwrap_or_else(|e| panic!("{name}: render: {e}"));
        assert_eq!(
            raster.rgba.len(),
            field.ni as usize * field.nj as usize * 4,
            "{name}: render"
        );

        let probe = session
            .probe(&field, 45.0, 10.0)
            .unwrap_or_else(|| panic!("{name}: probe landed nowhere"));
        assert!(probe.value.is_some(), "{name}: probe read no value");

        let isolines = session
            .contours(&field, &[])
            .unwrap_or_else(|e| panic!("{name}: contours: {e}"));
        assert!(
            isolines.iter().any(|l| !l.segments.is_empty()),
            "{name}: the automatic levels drew nothing"
        );

        // Combine with itself: `a_minus_b` is zero everywhere present, which is
        // a value the difference map can be read against.
        let difference = session
            .combine(&field, &field, fieldglass_core::CombineOp::Difference)
            .unwrap_or_else(|e| panic!("{name}: combine: {e}"));
        assert_eq!(difference.stats.max, Some(0.0), "{name}: combine");
        assert_eq!(
            difference.stats.valid_count, field.stats.valid_count,
            "{name}: combine dropped cells"
        );
    }
}

/// The message list keeps describing the **file**, not the grid we chose for it.
///
/// A message view that reported only the synthesised raster would be describing
/// Fieldglass rather than the data (`docs/architecture/planned/03-composition.md`).
#[test]
fn the_message_list_still_reports_the_native_shape() {
    for (path, size_label, declared) in [
        (
            "../fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1",
            "T63",
            "spherical_harmonic",
        ),
        (
            "../fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2",
            "T63",
            "spherical_harmonic",
        ),
        (
            "../fieldglass-grib2/tests/fixtures/healpix_n4_ring.grib2",
            "Nside 4",
            "healpix",
        ),
    ] {
        let info = session(path).message(0).expect("message");
        assert_eq!(info.size_label.as_deref(), Some(size_label), "{path}");
        let grid = info.grid.as_ref().unwrap_or_else(|| panic!("{path}"));
        assert_eq!(grid.kind, "unsupported", "{path}: the declared grid");
        assert_eq!(grid.label, declared, "{path}: the declared grid");
    }
}

/// Bi-Fourier is rasterless too and still declines: there is no inverse
/// bi-Fourier transform, so it must not be swept into the synthesis path and
/// drawn from a grid nothing filled.
#[test]
fn a_bifourier_message_is_still_declined() {
    let err = session("../fieldglass-grib2/tests/fixtures/bifourier_ellipse_ieee32.grib2")
        .decode(0, &DecodeOptions::default())
        .expect_err("bi-Fourier has no grid to decode onto");
    assert_eq!(err.code(), "decode");
    assert!(
        err.message().contains("bi-Fourier"),
        "the refusal should name the family: {err}"
    );
}
