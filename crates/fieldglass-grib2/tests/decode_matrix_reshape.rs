//! GRIB2 matrix-of-values reshape (template 5.1, `matrixBitmapsPresent = 1`):
//! end-to-end decode of a true per-point `NR × NC` matrix.
//!
//! Stock eccodes crashes on this variant, so there is no eccodes oracle. The
//! fixture is hand-assembled by `tools/build_grib2_matrix_reshape_fixture.py`
//! (byte-editing an eccodes `matrixBitmapsPresent = 0` skeleton, independently of
//! the Rust decoder) to encode the SAME logical field as the GRIB1
//! `hand_matrix_of_values.grib1` fixture: a 16×31 grid, NR=1/NC=2, all cells
//! present, 8-bit, coded byte k = k % 256. So the decoded matrix value at flat
//! index k is k % 256 — the exact hand-computable oracle the independently
//! validated GRIB1 matrix decoder is checked against, giving a cross-edition
//! check on the shared `expand_matrix` reshape. See `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/matrix_reshape_16x31.grib2");

#[test]
fn matrix_reshape_section5_metadata() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 1, "§5 template 5.1");
    let t = msg.drs.matrix_simple().expect("5.1 matrix template");
    assert_eq!(t.matrix_bitmaps_present, 1, "true-matrix variant");
    assert_eq!((t.nr, t.nc), (1, 2));
    assert_eq!(t.number_of_coded_values, 992);
}

#[test]
fn scalar_path_rejects_the_true_matrix() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("parse");
    let err = reader
        .decode_message_values(0)
        .expect_err("scalar path must refuse");
    let msg = format!("{err:?}");
    assert!(msg.contains("matrixBitmapsPresent=1"), "{msg}");
    assert!(
        msg.contains("decode_matrix_message"),
        "points at the API: {msg}"
    );
}

#[test]
fn matrix_reshape_matches_hand_computed_oracle() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("parse");
    let field = reader
        .decode_matrix_message(0)
        .expect("matrix decode succeeds");

    assert_eq!((field.ni, field.nj), (16, 31));
    assert_eq!((field.nr, field.nc), (1, 2));
    let total = field.ni * field.nj * field.nr * field.nc;
    assert_eq!(field.values.len(), total, "flattened Ni·Nj·NR·NC");
    assert_eq!(total, 992);

    // Same oracle as the GRIB1 hand fixture: every cell present, value at flat
    // index k == coded byte == k % 256.
    for (k, v) in field.values.iter().enumerate() {
        let got = v.expect("no masked cells in the all-present fixture");
        let want = (k % 256) as f64;
        assert!(
            (got - want).abs() < 1e-9,
            "values[{k}] = {got}, expected {want}"
        );
    }
}
