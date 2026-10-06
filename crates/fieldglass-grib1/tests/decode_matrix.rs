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
    let range = reader.messages[0].bds_range;
    let (s, e) = (range.start as usize, (range.start + range.len) as usize);
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
    let range = reader.messages[0].bds_range;
    let (s, e) = (range.start as usize, (range.start + range.len) as usize);
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

/// #802: a primary bitmap that marks every point absent makes N = 0, so the
/// secondary bitmaps and the coded stream are empty and every section-length
/// check passes, while NR = NC = 0xFFFF asks for `496 · 65535²` cells (about
/// 2 TB of `Option<f64>`). Built from the committed fixture: zero the BMS body,
/// set BDS octets 12-13 (N) to 0 and octets 15-18 (NR, NC) to 0xFFFF. Before
/// the cap this aborted the process on the allocation; it must be a parse
/// error that names the cap.
#[test]
fn an_all_absent_bitmap_with_a_huge_matrix_is_refused_before_allocating() {
    let reader =
        Grib1Reader::from_bytes(MATRIX_OF_VALUES_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let bms = msg.bms_range.expect("the fixture carries a BMS");
    let bds = msg.bds_range.start as usize;

    let mut hostile = MATRIX_OF_VALUES_FIXTURE.to_vec();
    hostile[bms.start as usize + 6..(bms.start + bms.len) as usize].fill(0);
    hostile[bds + 11..bds + 13].copy_from_slice(&0u16.to_be_bytes()); // N
    hostile[bds + 14..bds + 18].fill(0xFF); // NR, NC

    let reader = Grib1Reader::from_bytes(hostile).expect("the edit keeps the framing");
    let err = reader
        .decode_matrix_message(0)
        .expect_err("a field past the cell cap must be refused");
    assert!(
        matches!(&err, fieldglass_grib1::FieldglassError::Parse(m) if m.contains("cell cap")),
        "got {err:?}"
    );
}

/// NR = 0 with 0 bits per value breaks two rules, and both editions report the
/// same one: bits per value is checked first, then NR·NC (#846). Built from the
/// committed fixture: BDS octet 11 (bits per value) and octets 15-16 (NR). The
/// GRIB2 half is `checks_bits_per_value_before_the_datum_as_grib1_does` in
/// `fieldglass-grib2`'s `matrix.rs`.
#[test]
fn checks_bits_per_value_before_the_datum_as_grib2_does() {
    let reader =
        Grib1Reader::from_bytes(MATRIX_OF_VALUES_FIXTURE.to_vec()).expect("fixture parses");
    let bds = reader.messages[0].bds_range.start as usize;
    let edit = |bits: u8| {
        let mut bytes = MATRIX_OF_VALUES_FIXTURE.to_vec();
        bytes[bds + 10] = bits; // bits per value
        bytes[bds + 14..bds + 16].copy_from_slice(&0u16.to_be_bytes()); // NR
        let reader = Grib1Reader::from_bytes(bytes).expect("the edit keeps the framing");
        reader.decode_matrix_message(0).expect_err("refused")
    };
    assert_eq!(
        edit(0).to_string(),
        fieldglass_grib1::FieldglassError::Parse(
            "grid_simple_matrix bits_per_value 0 is unsupported (expected 1..=32)".into()
        )
        .to_string()
    );
    let err = edit(8);
    assert!(err.to_string().contains("datum size NR×NC = 0×"), "{err}");
}
