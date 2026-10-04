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
const SPECTRAL_T383: &[u8] = include_bytes!("fixtures/spectral_simple_t383.grib2");
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
    // The oracle is the full T63 sum at these points; the 5° grid itself
    // resolves only T35, so the full sum is the explicit call (#637).
    let field = reader
        .synthesize_spectral_message_full(0, &lats, &lons)
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

/// A grid of the caller's own is band-limited by default to what it resolves
/// (#637, MIR's rule): the 5° grid carries T35, so a T63 message on it is the
/// T35 triangle, and the full sum is a separate, explicit call.
#[test]
fn a_callers_grid_is_band_limited_to_what_it_resolves() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    let (lats, lons) = grid();
    assert_eq!(
        fieldglass_core::sht::points_band_limit(&lats, &lons),
        Some(35)
    );
    let coeffs = reader.decode_spectral_message(0).expect("spectral decodes");
    let t35 = fieldglass_core::sht::synthesize_band_limited(
        &coeffs.coefficients,
        coeffs.j,
        35,
        &lats,
        &lons,
    )
    .expect("T35");
    let default = reader
        .synthesize_spectral_message(0, &lats, &lons)
        .expect("default");
    assert_eq!(default, t35);
    let full = reader
        .synthesize_spectral_message_full(0, &lats, &lons)
        .expect("full");
    assert_ne!(default, full);
    // A grid that resolves the whole field gets all of it by default: the
    // pinned 0.5° grid carries T359.
    let (grid, map) = reader.synthesize_spectral_global(0).expect("map");
    let (lats, lons) = grid.axes();
    assert_eq!(
        reader
            .synthesize_spectral_message(0, &lats[..3], &lons)
            .expect("three rows"),
        map[..3 * lons.len()]
    );
}

/// A grid of two regions resolves what each region is sampled at, not the gap
/// between them (#812). Two polar caps 140° apart, or two longitude sectors
/// 170° apart, both at 0.5°, used to read as a 140° or 170° step: T0, the
/// field's global mean at every point.
#[test]
fn a_grid_of_two_regions_is_synthesised_at_its_own_spacing() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_T383.to_vec()).expect("parse");
    let axis = |from: f64, to: f64, step: f64| -> Vec<f64> {
        let n = ((to - from) / step).round() as usize;
        (0..=n).map(|k| from + k as f64 * step).collect()
    };
    let coeffs = reader.decode_spectral_message(0).expect("spectral decodes");
    let cases = [
        // Two latitude caps over a 0.5° sector.
        (
            [axis(-80.0, -70.0, 0.5), axis(70.0, 80.0, 0.5)].concat(),
            axis(0.0, 10.0, 0.5),
        ),
        // Two longitude sectors over a 0.5° latitude band.
        (
            axis(-10.0, 10.0, 0.5),
            [axis(0.0, 10.0, 0.5), axis(180.0, 190.0, 0.5)].concat(),
        ),
    ];
    for (lats, lons) in cases {
        assert_eq!(
            fieldglass_core::sht::points_band_limit(&lats, &lons),
            Some(359)
        );
        let values = reader
            .synthesize_spectral_message(0, &lats, &lons)
            .expect("synthesises");
        let (lo, hi) = values
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &v| {
                (lo.min(v), hi.max(v))
            });
        assert!(hi - lo > 1.0, "a constant field: {lo} .. {hi}");
        // Exactly the T359 triangle, the most a 0.5° step carries.
        let t359 = fieldglass_core::sht::synthesize_band_limited(
            &coeffs.coefficients,
            coeffs.j,
            359,
            &lats,
            &lons,
        )
        .expect("T359");
        assert_eq!(values, t359);
    }
}
