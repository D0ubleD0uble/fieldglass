//! End-to-end decode of GRIB1 `grid_ieee` raw IEEE-754 float packing.
//!
//! Both fixtures are `cmc_wind_300_2010052400_p012.grib` (a `grid_simple`
//! field) re-encoded by eccodes 2.34.1 — `ieee32` at `precision = 1`
//! (32-bit), `ieee64` at `precision = 2` (64-bit). The committed
//! `*_expected.json` files are the `grib_get_data` oracle (counts, min/max/mean
//! and anchored samples; the 32-bit oracle carries f32 rounding, hence the
//! 1e-3 tolerance). Provenance in `tests/fixtures/NOTICE.md`.

use fieldglass_grib1::{Grib1Reader, parse_bds_header};

const IEEE32_FIXTURE: &[u8] = include_bytes!("fixtures/ieee32_cmc_wind.grib1");
const IEEE64_FIXTURE: &[u8] = include_bytes!("fixtures/ieee64_cmc_wind.grib1");

const COUNT: usize = 12_825;
const MIN: f64 = 0.209_608;
const MAX: f64 = 75.209_608;
const MEAN: f64 = 22.178_321_080_582_965;

/// Anchored samples shared by both precisions (32-bit lands within 1e-3).
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
fn ieee_header_reports_raw_packing_flags() {
    // grid_ieee: complexPacking=0, integerPointValues=1, additionalFlagPresent=1.
    for fixture in [IEEE32_FIXTURE, IEEE64_FIXTURE] {
        let reader = Grib1Reader::from_bytes(fixture.to_vec()).expect("fixture parses");
        let (s, e) = reader.messages[0].bds_range;
        let bds = parse_bds_header(&fixture[s..e]).expect("BDS header parses");
        assert!(!bds.is_spherical_harmonic);
        assert!(!bds.is_complex_packing);
        assert!(bds.is_integer_data, "integerPointValues flag set");
        assert!(bds.has_extra_flags, "additionalFlagPresent flag set");
    }
}

fn assert_matches_oracle(fixture: &[u8], label: &str) {
    let reader = Grib1Reader::from_bytes(fixture.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{label} decode succeeds: {e:?}"))
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    assert_eq!(present.len(), COUNT, "{label} value count");
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;

    let tol = 1e-3;
    assert!((min - MIN).abs() < tol, "{label} min {min} vs {MIN}");
    assert!((max - MAX).abs() < tol, "{label} max {max} vs {MAX}");
    assert!((mean - MEAN).abs() < tol, "{label} mean {mean} vs {MEAN}");
    for (i, want) in SAMPLES {
        let got = present[*i];
        assert!(
            (got - want).abs() < tol,
            "{label} values[{i}] was {got}, expected {want}"
        );
    }
}

#[test]
fn decode_ieee32_matches_eccodes_oracle() {
    assert_matches_oracle(IEEE32_FIXTURE, "ieee32");
}

#[test]
fn decode_ieee64_matches_eccodes_oracle() {
    assert_matches_oracle(IEEE64_FIXTURE, "ieee64");
}

/// Precision 3 (128-bit IEEE) is unimplemented in eccodes too; we surface a
/// precise `UnsupportedSection` rather than mis-decoding. Patch the precision
/// octet (byte 12 of the BDS) of the 64-bit fixture from `2` to `3`.
#[test]
fn ieee_precision_128bit_is_rejected() {
    let mut bytes = IEEE64_FIXTURE.to_vec();
    let reader = Grib1Reader::from_bytes(bytes.clone()).expect("fixture parses");
    let (s, _) = reader.messages[0].bds_range;
    // BDS octet 12 (precision) is at byte offset 11 within the section.
    assert_eq!(bytes[s + 11], 2, "fixture is 64-bit before patch");
    bytes[s + 11] = 3;

    let reader = Grib1Reader::from_bytes(bytes).expect("patched fixture parses");
    let err = reader
        .decode_message_values(0)
        .expect_err("128-bit precision rejected");
    match err {
        fieldglass_core::FieldglassError::UnsupportedSection(msg) => {
            assert!(msg.contains("grid_ieee"), "msg = {msg:?}");
            assert!(msg.contains("128"), "msg names 128-bit: {msg:?}");
        }
        other => panic!("expected UnsupportedSection, got {other:?}"),
    }
}
