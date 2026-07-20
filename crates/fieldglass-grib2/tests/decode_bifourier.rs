//! GRIB2 bi-Fourier (spectral) decode, cross-checked against eccodes.
//!
//! A bi-Fourier message (§3.61/62/63 + §5.53) stores the field's spectral
//! coefficients for a limited-area (ACCORD/ALADIN/AROME) model, not values on
//! a grid, so the check is against eccodes' own coefficient output:
//! `grib_get_data` on such a message prints a bare `Values` column (no
//! latitude/longitude, since eccodes decodes the packing but does not synthesise
//! a grid). Fixtures are round-tripped through eccodes — see
//! `tools/build_grib2_bifourier_fixtures.py` and `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;

/// Parse eccodes' one-value-per-line `grib_get_data` output (header stripped by
/// the builder).
fn parse_ref(text: &str) -> Vec<f64> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("reference coefficient parses"))
        .collect()
}

/// Decode `bytes` as a single bi-Fourier message and compare, value for value,
/// against eccodes' `expected` coefficients. Both sides decode the identical
/// packed bytes, so agreement is to floating-point precision (the quantisation
/// is baked into the fixture at encode time).
fn check(bytes: &[u8], expected: &str, bif: (u32, u32), trunc: u8) {
    let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 53, "§5 template 5.53");
    assert_eq!(msg.drs.template_name(), "bifourier_complex");
    assert_eq!(msg.gds.template_name(), "bifourier");
    // Coefficients, not a grid: the scalar path must refuse it and point at the
    // bi-Fourier entry point.
    let scalar = reader.decode_message_values(0);
    assert!(
        format!("{:?}", scalar.unwrap_err()).contains("decode_bifourier_message"),
        "scalar decode refers callers to the bi-Fourier entry point"
    );

    let bf = msg.gds.bifourier().expect("§3 bi-Fourier template");
    assert_eq!((bf.bif_i, bf.bif_j), bif);
    assert_eq!(bf.truncation_type, trunc);

    let coeffs = reader
        .decode_bifourier_message(0)
        .expect("bi-Fourier decodes");
    let want = parse_ref(expected);
    assert_eq!((coeffs.bif_i, coeffs.bif_j), bif);
    assert_eq!(
        coeffs.coefficients.len(),
        want.len(),
        "coefficient count = size_bif"
    );
    assert_eq!(coeffs.len(), want.len());
    assert!(!coeffs.is_empty());
    for (i, (got, w)) in coeffs.coefficients.iter().zip(&want).enumerate() {
        assert!(
            (got - w).abs() <= 1e-4 * w.abs().max(1.0),
            "coefficient {i}: got {got}, expected {w}",
        );
    }
}

#[test]
fn bifourier_ellipse_keepaxes_matches_eccodes() {
    check(
        include_bytes!("fixtures/bifourier_ellipse_keepaxes.grib2"),
        include_str!("fixtures/bifourier_ellipse_keepaxes.eccodes.ref.txt"),
        (4, 4),
        88,
    );
}

#[test]
fn bifourier_diamond_no_axes_matches_eccodes() {
    check(
        include_bytes!("fixtures/bifourier_diamond_no_axes.grib2"),
        include_str!("fixtures/bifourier_diamond_no_axes.eccodes.ref.txt"),
        (5, 5),
        99,
    );
}

#[test]
fn bifourier_rectangle_keepaxes_matches_eccodes() {
    check(
        include_bytes!("fixtures/bifourier_rectangle_keepaxes.grib2"),
        include_str!("fixtures/bifourier_rectangle_keepaxes.eccodes.ref.txt"),
        (3, 4),
        77,
    );
}

/// Same geometry as the ellipse case but with an IEEE 32-bit unpacked subset
/// (`unpackedSubsetPrecision == 1`), exercising the 4-byte read path.
#[test]
fn bifourier_ieee32_unpacked_subset_matches_eccodes() {
    check(
        include_bytes!("fixtures/bifourier_ellipse_ieee32.grib2"),
        include_str!("fixtures/bifourier_ellipse_ieee32.eccodes.ref.txt"),
        (4, 4),
        88,
    );
    let reader =
        Grib2Reader::from_bytes(include_bytes!("fixtures/bifourier_ellipse_ieee32.grib2").to_vec())
            .expect("parse");
    assert_eq!(
        reader.messages[0]
            .drs
            .bifourier()
            .expect("bi-Fourier §5")
            .unpacked_subset_precision,
        1,
        "this fixture uses the IEEE 32-bit unpacked subset"
    );
}
