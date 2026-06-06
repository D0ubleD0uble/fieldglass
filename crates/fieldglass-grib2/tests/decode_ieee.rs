//! End-to-end decode of GRIB2 IEEE floating-point packing (DRS template 5.4).
//!
//! Both fixtures are `regular_latlon_surface.grib2` (a 16×31 ECMWF 2-metre
//! temperature field, simple packing 5.0) re-encoded by eccodes 2.34.1 —
//! `ieee32` at `precision = 1` (32-bit), `ieee64` at `precision = 2`
//! (64-bit). Template 5.4 stores each value verbatim as a big-endian IEEE
//! float with no reference/scale transform, so both fixtures decode to the
//! same field (the source values were already f32-exact). The committed
//! `ieee64_regular_latlon_expected.json` is the `grib_get_data` oracle
//! (counts, min/max/mean, anchored samples). Provenance in
//! `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;

const IEEE32_FIXTURE: &[u8] = include_bytes!("fixtures/ieee32_regular_latlon.grib2");
const IEEE64_FIXTURE: &[u8] = include_bytes!("fixtures/ieee64_regular_latlon.grib2");

const COUNT: usize = 496;
const MIN: f64 = 270.0;
const MAX: f64 = 310.631_835_94;
const MEAN: f64 = 291.118_451_518_387;

/// Anchored samples from the eccodes oracle, shared by both precisions.
const SAMPLES: &[(usize, f64)] = &[
    (0, 278.533_203_12),
    (1, 279.494_140_62),
    (100, 278.163_085_94),
    (250, 291.281_25),
    (495, 300.415_039_06),
];

#[test]
fn ieee_section_reports_template_5_4() {
    for fixture in [IEEE32_FIXTURE, IEEE64_FIXTURE] {
        let reader = Grib2Reader::from_bytes(fixture.to_vec()).expect("fixture parses");
        let msg = &reader.messages[0];
        assert_eq!(msg.drs.template_number, 4, "DRS template 5.4");
        assert_eq!(msg.drs.template_name(), "ieee");
    }
}

fn assert_matches_oracle(fixture: &[u8], label: &str) {
    let reader = Grib2Reader::from_bytes(fixture.to_vec()).expect("fixture parses");
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
