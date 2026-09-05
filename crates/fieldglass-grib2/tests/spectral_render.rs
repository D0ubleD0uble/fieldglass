//! Inverse spherical-harmonic transform (#303): synthesize a lat/lon grid from
//! a decoded GRIB2 spectral field and check it against the definitive-formula
//! oracle.
//!
//! eccodes cannot synthesise a grid from spectral coefficients, so the oracle
//! is computed directly from ECMWF's spectral definition by
//! `tools/build_grib2_spectral_render_oracle.py` (cross-validated against
//! pyshtools during development). The unit tests in `fieldglass_core::sht`
//! pin the convention with exact analytic single-coefficient cases; this test
//! exercises the whole path on a realistic T63 temperature field.

use fieldglass_grib2::{GlobalGrid, Grib2Reader};

const SPECTRAL_T63: &[u8] = include_bytes!("fixtures/spectral_simple_t63.grib2");
const ORACLE: &str = include_str!("fixtures/spectral_render_t63.oracle.txt");

/// The fixed 5° regular lat/lon grid the oracle builder uses: latitudes 90..-90
/// (37) and longitudes 0..355 (72), latitude-major.
fn grid() -> (Vec<f64>, Vec<f64>) {
    let lats = (0..37).map(|i| 90.0 - 5.0 * i as f64).collect();
    let lons = (0..72).map(|j| 5.0 * j as f64).collect();
    (lats, lons)
}

#[test]
fn spectral_synthesis_matches_definitive_oracle() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    assert_eq!(
        reader.decode_spectral_message(0).expect("decodes").j,
        63,
        "T63 truncation"
    );

    let (lats, lons) = grid();
    let field = reader
        .synthesize_spectral_message(0, &lats, &lons)
        .expect("synthesize");

    let oracle: Vec<f64> = ORACLE
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("oracle value parses"))
        .collect();

    assert_eq!(field.len(), oracle.len(), "37×72 = 2664 grid points");
    assert_eq!(field.len(), 37 * 72);

    // Same recurrence and formula on both sides (Rust f64 vs numpy f64), so they
    // agree to floating-point precision.
    let mut max_abs = 0.0f64;
    for (i, (got, want)) in field.iter().zip(&oracle).enumerate() {
        let d = (got - want).abs();
        max_abs = max_abs.max(d);
        assert!(
            d <= 1e-6 * want.abs().max(1.0),
            "grid point {i}: got {got}, oracle {want} (Δ={d})",
        );
    }
    // Sanity: a real ~281 K temperature field, and the North Pole (first row) is
    // longitude-independent.
    let pole = &field[0..72];
    assert!(
        pole.iter().all(|&v| (v - pole[0]).abs() < 1e-9),
        "pole is zonal"
    );
    assert!(
        max_abs < 1e-3,
        "agreement well within tolerance (max Δ={max_abs})"
    );
}

/// The convention this crate no longer has to be told: `synthesize_spectral_global`
/// picks the shared grid and hands it back with the field, so the two cannot
/// disagree (#546).
#[test]
fn the_global_synthesis_pairs_the_field_with_the_grid_it_is_on() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    let (grid, field) = reader
        .synthesize_spectral_global(0)
        .expect("synthesize onto the shared grid");

    // The pinned 0.5° grid, whatever the truncation.
    assert_eq!(grid, GlobalGrid::FINEST);
    assert_eq!(grid.dims(), (720, 361));
    assert_eq!(field.len(), grid.len());

    // Same values as evaluating the message on that grid by hand: the paired
    // call is a convenience over the explicit one, not a second transform.
    let (lats, lons) = grid.axes();
    assert_eq!(
        field,
        reader
            .synthesize_spectral_message(0, &lats, &lons)
            .expect("synthesize on the same axes")
    );

    // No duplicated wrap column: the eastern edge is a step short of the seam,
    // so the row does not repeat longitude 0 at both ends. A field doubled at
    // the antimeridian is what getting this wrong looks like.
    let step = 360.0 / grid.ni as f64;
    assert!((lons[grid.ni - 1] - (360.0 - step)).abs() < 1e-9);
    // Checked on the equator, not on a pole row: a pole row is zonal whatever
    // the longitudes are, so it would pass with the seam column duplicated.
    let equator = &field[(grid.nj / 2) * grid.ni..][..grid.ni];
    assert!(
        equator.iter().any(|&v| (v - equator[0]).abs() > 1e-9),
        "the equator row is longitude-dependent, or this proves nothing"
    );
    assert!(
        (equator[0] - equator[grid.ni - 1]).abs() > 1e-9,
        "column 0 and the last column sit a step apart, so they must differ"
    );

    // Pole to pole, north first: the first and last rows are each zonal.
    for (name, row) in [
        ("north pole", &field[..grid.ni]),
        ("south pole", &field[field.len() - grid.ni..]),
    ] {
        assert!(
            row.iter().all(|&v| (v - row[0]).abs() < 1e-9),
            "the {name} row must be longitude-independent"
        );
    }
}
