//! GRIB1 spherical-harmonic (spectral) decode, cross-checked against eccodes.
//!
//! A spectral message stores the field's coefficients, not values on a grid, so
//! the check here is against eccodes' own coefficient output (`grib_get_data` on
//! a spectral message prints a bare `Value` column — no latitude or longitude,
//! because there is no grid to place them on). The snapshot beside the fixture
//! is exactly that output.

use fieldglass_grib1::{Grib1Reader, GridDescription};

const SPECTRAL_T63: &[u8] = include_bytes!("fixtures/spectral_complex_t63.grib1");
const SPECTRAL_SIMPLE_T63: &[u8] = include_bytes!("fixtures/spectral_simple_t63.grib1");

/// The same field re-encoded by eccodes as `spectral_simple`.
fn simple_reference() -> Vec<f64> {
    parse_reference(include_str!("fixtures/spectral_simple_t63.eccodes.ref.txt"))
}

fn parse_reference(raw: &str) -> Vec<f64> {
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("reference value parses"))
        .collect()
}

/// eccodes' 4160 coefficients for the same message — one per line, exactly as
/// `grib_get_data` prints them. Regenerate with:
///
/// ```sh
/// grib_get_data spectral_complex_t63.grib1 | tail -n +2 \
///   > spectral_complex_t63.eccodes.ref.txt
/// ```
fn eccodes_reference() -> Vec<f64> {
    parse_reference(include_str!(
        "fixtures/spectral_complex_t63.eccodes.ref.txt"
    ))
}

#[test]
fn spectral_gds_reports_the_truncation() {
    let reader = Grib1Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    let gds = msg.gds.as_ref().expect("spectral message has a GDS");
    let GridDescription::SphericalHarmonic(sh) = gds else {
        panic!(
            "expected a spherical-harmonic grid, got {}",
            gds.grid_type_name()
        );
    };
    // T63 triangular truncation: J = K = M = 63, associated Legendre (1),
    // complex representation mode (2).
    assert_eq!((sh.j, sh.k, sh.m), (63, 63, 63));
    assert_eq!(sh.representation_type, 1);
    assert_eq!(sh.representation_mode, 2);
    // A spectral message has no grid, so it reports no dimensions and no
    // bounds — that is what keeps it off the scalar decode path.
    assert_eq!(gds.dimensions(), None);
    assert_eq!(gds.bounds(), None);
    assert_eq!(gds.grid_type_name(), "spherical_harmonic");
}

#[test]
fn spectral_complex_coefficients_match_eccodes() {
    let reader = Grib1Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    let field = reader.decode_spectral_message(0).expect("decode spectral");
    let want = eccodes_reference();

    // (T+1)(T+2) = 64 * 65 = 4160 stored values — 2080 complex coefficients.
    assert_eq!(field.coefficients.len(), 4160, "stored value count");
    assert_eq!(field.len(), 2080, "complex coefficient count");
    assert_eq!(field.coefficients.len(), want.len());

    // eccodes prints ~9 significant figures, so compare relative to magnitude.
    // The coefficients span many orders of magnitude (the mean is ~289, the
    // high-degree terms ~1e-6), which is the whole reason for the Laplacian
    // scaling — an absolute tolerance would be meaningless across that range.
    for (i, (&got, &expected)) in field.coefficients.iter().zip(want.iter()).enumerate() {
        let tol = 1e-6 * expected.abs().max(1e-6);
        assert!(
            (got - expected).abs() <= tol,
            "coefficient {i}: got {got}, eccodes says {expected}"
        );
    }
}

#[test]
fn spectral_field_mean_and_zonal_symmetry_hold() {
    let reader = Grib1Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    let field = reader.decode_spectral_message(0).expect("decode spectral");

    // coefficients[0] is Re(X[0][0]) — the field mean. This is a temperature
    // field, so it should be a plausible global mean in kelvin. It comes from
    // the sub-truncation's IBM floats, unscaled, so a mistake in that block
    // (wrong float format, wrong offset) shows up here immediately.
    let mean = field.coefficients[0];
    assert!(
        (200.0..350.0).contains(&mean),
        "field mean {mean} K is not a plausible temperature"
    );

    // Zonal wavenumber m = 0 has no imaginary part: the m = 0 coefficients are
    // real by construction. They are the first (T+1) complex coefficients of the
    // traversal, and every one of their imaginary parts must be exactly zero —
    // eccodes forces the packed ones, and the unpacked ones are stored as zero.
    for n in 0..=63usize {
        let im = field.coefficients[2 * n + 1];
        assert_eq!(im, 0.0, "Im(X[0][{n}]) must be zero, got {im}");
    }
}

#[test]
fn spectral_simple_coefficients_match_eccodes() {
    // The other spectral packing: no sub-truncation and no Laplacian, but the
    // real part of the (0, 0) coefficient is lifted out of the packed stream
    // into the section header as a bare IBM float, so the data begins four
    // octets later than a reader might assume.
    let reader = Grib1Reader::from_bytes(SPECTRAL_SIMPLE_T63.to_vec()).expect("parse");
    assert_eq!(reader.packing_label(0), Some("spectral_simple"));

    let field = reader.decode_spectral_message(0).expect("decode spectral");
    let want = simple_reference();
    assert_eq!(field.coefficients.len(), 4160);
    assert_eq!(field.coefficients.len(), want.len());

    for (i, (&got, &expected)) in field.coefficients.iter().zip(want.iter()).enumerate() {
        let tol = 1e-6 * expected.abs().max(1e-6);
        assert!(
            (got - expected).abs() <= tol,
            "coefficient {i}: got {got}, eccodes says {expected}"
        );
    }

    // The trap this packing sets: the imaginary part of (0, 0) is mathematically
    // zero, but it really is stored in the packed stream, and eccodes does *not*
    // force it back to zero the way complex packing does. It decodes to
    // quantisation noise (~9.5e-6 here), so a decoder that "helpfully" zeroed it
    // would silently disagree with eccodes.
    assert_ne!(
        field.coefficients[1], 0.0,
        "spectral_simple keeps the stored Im(X[0][0]); it must not be zeroed"
    );
    assert!(
        field.coefficients[1].abs() < 1e-3,
        "...but it is only noise"
    );
}

#[test]
fn spectral_message_is_kept_off_the_scalar_grid_path() {
    // The load-bearing guard: coefficients are not one scalar per grid point, so
    // painting them as a 2-D raster would be nonsense. `decode_message_values`
    // must refuse and say where to go instead, rather than hand back 4160
    // numbers that look like a field.
    let reader = Grib1Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    let err = reader
        .decode_message_values(0)
        .expect_err("a spectral message must not decode as a grid");
    let msg = err.to_string();
    assert!(
        msg.contains("decode_spectral_message"),
        "the error should name the call that does work: {msg}"
    );
}

#[test]
fn packing_type_label_names_the_spectral_variant() {
    let reader = Grib1Reader::from_bytes(SPECTRAL_T63.to_vec()).expect("parse");
    assert_eq!(reader.packing_label(0), Some("spectral_complex"));
}
