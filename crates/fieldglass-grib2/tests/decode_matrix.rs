//! GRIB2 matrix-of-values packing (DRT 5.1, `grid_simple_matrix`), cross-checked
//! against eccodes.
//!
//! Template 5.1 is experimental; eccodes only handles the
//! `matrixBitmapsPresent = 0` case, where §7 is one simple-packed value per grid
//! point (NR/NC are descriptive §5 metadata) and the field decodes exactly like
//! template 5.0. That flat case is what this fixture pins, value-for-value
//! against eccodes' `grib_get_data`. The true per-point matrix
//! (`matrixBitmapsPresent = 1`, secondary bitmaps) is the eccodes-unsupported
//! variant and is rejected rather than mis-decoded. See
//! `tools/build_grib2_matrix_fixtures.py` and `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;

const MATRIX_FLAT: &[u8] = include_bytes!("fixtures/matrix_simple_regular_latlon.grib2");
const MATRIX_FLAT_REF: &str = include_str!("fixtures/matrix_simple_regular_latlon.eccodes.ref.txt");

fn eccodes_values() -> Vec<f64> {
    MATRIX_FLAT_REF
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().parse().expect("reference value parses"))
        .collect()
}

#[test]
fn matrix_simple_section5_metadata() {
    let reader = Grib2Reader::from_bytes(MATRIX_FLAT.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 1, "§5 template 5.1");
    assert_eq!(msg.drs.template_name(), "grid_simple_matrix");
    let t = msg
        .drs
        .matrix_simple()
        .expect("5.1 carries the matrix template");
    assert_eq!(t.matrix_bitmaps_present, 0);
    assert_eq!((t.nr, t.nc), (2, 3));
    assert_eq!(t.number_of_coded_values, 496);
    assert_eq!(t.bits_per_value, 12);
}

#[test]
fn matrix_simple_flat_decodes_matching_eccodes() {
    let reader = Grib2Reader::from_bytes(MATRIX_FLAT.to_vec()).expect("parse");
    // matrixBitmapsPresent = 0 → one value per grid point, so it decodes through
    // the ordinary scalar path and renders like any simple-packed field.
    let values = reader
        .decode_message_values(0)
        .expect("flat matrix decodes");
    let expected = eccodes_values();
    assert_eq!(values.len(), expected.len(), "one value per grid point");
    for (i, (got, want)) in values.iter().zip(&expected).enumerate() {
        let got = got.expect("no bitmap, so every point is present");
        assert!(
            (got - want).abs() <= 1e-4 * want.abs().max(1.0),
            "value {i}: got {got}, expected {want}",
        );
    }
}
