//! End-to-end decode of GRIB1 `grid_simple_matrix` packing.
//!
//! `matrix_simple_cmc_wind.grib1` is `cmc_wind_300_2010052400_p012.grib`
//! re-encoded by eccodes 2.34.1 as `packingType=grid_simple_matrix`. eccodes
//! emits the `matrixOfValues = 0` form — a simple-packed body sitting behind
//! the matrix sub-header — so the decoded field equals the original. The
//! committed `_expected.json` is the `grib_get_data` oracle. Provenance in
//! `tests/fixtures/NOTICE.md`.

use fieldglass_grib1::{Grib1Reader, parse_bds_header};

const MATRIX_FIXTURE: &[u8] = include_bytes!("fixtures/matrix_simple_cmc_wind.grib1");
const MATRIX_OF_VALUES_FIXTURE: &[u8] = include_bytes!("fixtures/hand_matrix_of_values.grib1");

const COUNT: usize = 12_825;
const MIN: f64 = 0.209_608;
const MAX: f64 = 75.209_608;
const MEAN: f64 = 22.178_321_080_582_965;

const SAMPLES: &[(usize, f64)] = &[
    (0, 5.459_608),
    (1, 5.709_608),
    (2, 5.959_608),
    (100, 11.959_608),
    (1000, 45.959_606),
    (6000, 60.709_606),
    (12000, 36.709_606),
    (12824, 11.709_608),
];

#[test]
fn matrix_header_reports_simple_matrix_flags() {
    // grid_simple_matrix: complexPacking=0, integerPointValues=0,
    // additionalFlagPresent=1 — distinct from grid_ieee (integer=1).
    let reader = Grib1Reader::from_bytes(MATRIX_FIXTURE.to_vec()).expect("fixture parses");
    let (s, e) = reader.messages[0].bds_range;
    let bds = parse_bds_header(&MATRIX_FIXTURE[s..e]).expect("BDS header parses");
    assert!(!bds.is_spherical_harmonic);
    assert!(!bds.is_complex_packing);
    assert!(!bds.is_integer_data, "integerPointValues clear");
    assert!(bds.has_extra_flags, "additionalFlagPresent set");
}

#[test]
fn decode_simple_matrix_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(MATRIX_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("grid_simple_matrix decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    assert_eq!(present.len(), COUNT);
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;

    let tol = 1e-3;
    assert!((min - MIN).abs() < tol, "min {min} vs {MIN}");
    assert!((max - MAX).abs() < tol, "max {max} vs {MAX}");
    assert!((mean - MEAN).abs() < tol, "mean {mean} vs {MEAN}");
    for (i, want) in SAMPLES {
        let got = present[*i];
        assert!(
            (got - want).abs() < tol,
            "values[{i}] was {got}, expected {want}"
        );
    }
}

// ---------------------------------------------------------------------------
// matrixOfValues = 1 — a genuine NR×NC matrix at every grid point.
// ---------------------------------------------------------------------------
//
// eccodes 2.34.1 can neither encode nor decode this variant (it asserts out),
// so there is no grib_get_data oracle. `hand_matrix_of_values.grib1` is a
// hand-assembled message: a 16×31 regular_ll grid (496 points), NR=1, NC=2
// (datum size 2), an all-present primary BMS and all-present secondary bitmaps,
// R=0/E=0/D=0 and 8-bit packing with coded byte k = k % 256. With nothing
// masked the decoded matrix value at flat index k therefore equals k % 256 —
// an exactly hand-computable oracle. The decoder is cross-checked against
// eccodes' data.grid_simple_matrix.def + DataG1SecondaryBitmap accessor.
// Construction in tests/fixtures/NOTICE.md.

const MV_NI: usize = 16;
const MV_NJ: usize = 31;
const MV_NR: usize = 1;
const MV_NC: usize = 2;

#[test]
fn matrix_of_values_header_reports_matrix_bit() {
    let reader =
        Grib1Reader::from_bytes(MATRIX_OF_VALUES_FIXTURE.to_vec()).expect("fixture parses");
    let (s, e) = reader.messages[0].bds_range;
    let bds = parse_bds_header(&MATRIX_OF_VALUES_FIXTURE[s..e]).expect("BDS header parses");
    assert!(!bds.is_complex_packing);
    assert!(!bds.is_integer_data);
    assert!(bds.has_extra_flags, "additionalFlagPresent set");
}

#[test]
fn scalar_decode_rejects_matrix_of_values() {
    // decode_message_values must refuse a true matrix field rather than
    // mis-decode it as one-value-per-point.
    let reader =
        Grib1Reader::from_bytes(MATRIX_OF_VALUES_FIXTURE.to_vec()).expect("fixture parses");
    let err = reader
        .decode_message_values(0)
        .expect_err("matrixOfValues=1 rejected by scalar path");
    match err {
        fieldglass_core::FieldglassError::UnsupportedSection(msg) => {
            assert!(msg.contains("matrixOfValues"), "msg = {msg:?}");
            assert!(
                msg.contains("decode_matrix_message"),
                "msg points to API: {msg:?}"
            );
        }
        other => panic!("expected UnsupportedSection, got {other:?}"),
    }
}

#[test]
fn decode_matrix_of_values_matches_hand_computed_oracle() {
    let reader =
        Grib1Reader::from_bytes(MATRIX_OF_VALUES_FIXTURE.to_vec()).expect("fixture parses");
    let field = reader
        .decode_matrix_message(0)
        .expect("matrix-of-values decode succeeds");

    assert_eq!((field.ni, field.nj), (MV_NI, MV_NJ));
    assert_eq!((field.nr, field.nc), (MV_NR, MV_NC));

    let datum = MV_NR * MV_NC;
    let total = MV_NI * MV_NJ * datum;
    assert_eq!(field.values.len(), total, "flattened matrix length");

    // Every cell present; value at flat index k == coded byte == k % 256.
    for (k, v) in field.values.iter().enumerate() {
        let got = v.expect("no masked cells in the all-present fixture");
        let want = (k % 256) as f64;
        assert!(
            (got - want).abs() < 1e-9,
            "values[{k}] was {got}, expected {want}"
        );
    }
}
