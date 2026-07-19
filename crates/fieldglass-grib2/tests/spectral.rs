//! GRIB2 spherical-harmonic (spectral) decode, cross-checked against eccodes.
//!
//! A spectral message (§3.50 + §5.50) stores the field's coefficients, not
//! values on a grid, so the check here is against eccodes' own coefficient
//! output (`grib_get_data` on a spectral message prints a bare `Value` column —
//! no latitude or longitude, because eccodes decodes the packing but does not
//! synthesise a grid).

use fieldglass_grib2::Grib2Reader;

const SPECTRAL_SIMPLE_T63: &[u8] = include_bytes!("fixtures/spectral_simple_t63.grib2");

/// eccodes' 4160 coefficients for the same message — one per line, exactly as
/// `grib_get_data` prints them. Regenerate with:
///
/// ```sh
/// grib_get_data spectral_simple_t63.grib2 | tail -n +2 \
///   > spectral_simple_t63.eccodes.ref.txt
/// ```
fn eccodes_reference() -> Vec<f64> {
    include_str!("fixtures/spectral_simple_t63.eccodes.ref.txt")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("reference coefficient parses"))
        .collect()
}

#[test]
fn spectral_gds_reports_the_truncation() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_SIMPLE_T63.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.gds.template_number, 50, "§3 template 3.50");
    assert_eq!(msg.gds.template_name(), "spherical_harmonic");
    let sh = msg
        .gds
        .spherical_harmonic()
        .expect("§3.50 carries the spherical-harmonic template");
    assert_eq!((sh.j, sh.k, sh.m), (63, 63, 63), "T63 truncation");

    assert_eq!(msg.drs.template_number, 50, "§5 template 5.50");
    assert_eq!(msg.drs.template_name(), "spectral_simple");
}

#[test]
fn spectral_simple_decodes_coefficients_matching_eccodes() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_SIMPLE_T63.to_vec()).expect("parse");
    let coeffs = reader
        .decode_spectral_message(0)
        .expect("spectral_simple decodes");
    let expected = eccodes_reference();

    assert_eq!((coeffs.j, coeffs.k, coeffs.m), (63, 63, 63));
    assert_eq!(
        coeffs.coefficients.len(),
        expected.len(),
        "coefficient count = (J+1)(J+2) = 4160",
    );
    assert_eq!(
        coeffs.len(),
        expected.len() / 2,
        "2080 complex coefficients"
    );

    // The (0,0) real part is copied through unscaled; the rest are simple
    // unpacked, so a scaled-integer tolerance covers the round-trip.
    for (i, (got, want)) in coeffs.coefficients.iter().zip(&expected).enumerate() {
        assert!(
            (got - want).abs() < 1e-4 * want.abs().max(1.0),
            "coefficient {i}: got {got}, expected {want}",
        );
    }
}

const SPECTRAL_COMPLEX_T63: &[u8] = include_bytes!("fixtures/spectral_complex_t63.grib2");

/// eccodes' 4160 coefficients for the spectral_complex message. Regenerate with:
///
/// ```sh
/// grib_get_data spectral_complex_t63.grib2 | tail -n +2 \
///   > spectral_complex_t63.eccodes.ref.txt
/// ```
fn eccodes_complex_reference() -> Vec<f64> {
    include_str!("fixtures/spectral_complex_t63.eccodes.ref.txt")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("reference coefficient parses"))
        .collect()
}

#[test]
fn spectral_complex_gds_and_drs_report_the_truncation() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_COMPLEX_T63.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.gds.template_name(), "spherical_harmonic");
    assert_eq!(msg.drs.template_number, 51, "§5 template 5.51");
    assert_eq!(msg.drs.template_name(), "spectral_complex");
    let t = msg
        .drs
        .spectral_complex()
        .expect("§5.51 carries the spectral-complex template");
    assert_eq!(t.bits_per_value, 16);
    assert_eq!((t.js, t.ks, t.ms), (20, 20, 20), "KS=20 sub-truncation");
    assert_eq!(t.laplacian_scaling_factor, 722_000, "P = 0.722");
}

#[test]
fn spectral_complex_decodes_coefficients_matching_eccodes() {
    let reader = Grib2Reader::from_bytes(SPECTRAL_COMPLEX_T63.to_vec()).expect("parse");
    let coeffs = reader
        .decode_spectral_message(0)
        .expect("spectral_complex decodes");
    let expected = eccodes_complex_reference();

    assert_eq!((coeffs.j, coeffs.k, coeffs.m), (63, 63, 63));
    assert_eq!(
        coeffs.coefficients.len(),
        expected.len(),
        "coefficient count = (J+1)(J+2) = 4160",
    );

    // The unpacked sub-truncation is exact (IEEE f32); the packed part is
    // 16-bit simple packing with Laplacian de-scaling, and eccodes applies the
    // identical arithmetic, so a tight relative tolerance (with a small
    // absolute floor for the near-zero high-degree coefficients) holds.
    for (i, (got, want)) in coeffs.coefficients.iter().zip(&expected).enumerate() {
        assert!(
            (got - want).abs() <= 1e-5 * want.abs() + 1e-6,
            "coefficient {i}: got {got}, expected {want}",
        );
    }
}

#[test]
fn spectral_message_refuses_grid_decode() {
    // A spectral message has no grid, so the scalar decode path names the
    // spectral entry point instead of mis-decoding.
    let reader = Grib2Reader::from_bytes(SPECTRAL_SIMPLE_T63.to_vec()).expect("parse");
    let err = reader
        .decode_message_values(0)
        .expect_err("spectral has no grid values");
    assert!(
        format!("{err:?}").contains("decode_spectral_message"),
        "error points to the spectral decoder, got: {err:?}",
    );
}
