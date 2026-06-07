//! End-to-end decode of GRIB2 complex packing (DRS template 5.2).
//!
//! `complex_regular_latlon.grib2` is `regular_latlon_surface.grib2` (a 16×31
//! ECMWF 2-metre temperature field, simple packing 5.0) re-encoded by eccodes
//! 2.34.1 into `grid_complex` — general group splitting, no inline missing
//! values (the envelope the decoder supports). The committed
//! `complex_regular_latlon_expected.json` is the `grib_get_data` oracle
//! (counts, min/max/mean, anchored samples) plus the full §5 group
//! parameters. Provenance in `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;

const COMPLEX_FIXTURE: &[u8] = include_bytes!("fixtures/complex_regular_latlon.grib2");

const COUNT: usize = 496;
const MIN: f64 = 270.466_796_88;
const MAX: f64 = 311.098_632_81;
const MEAN: f64 = 291.585_248_393_467_8;

/// Anchored samples from the eccodes oracle.
const SAMPLES: &[(usize, f64)] = &[
    (0, 279.0),
    (1, 279.960_937_5),
    (100, 278.629_882_81),
    (250, 291.748_046_88),
    (495, 300.881_835_94),
];

#[test]
fn complex_section_reports_template_5_2() {
    let reader = Grib2Reader::from_bytes(COMPLEX_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 2, "DRS template 5.2");
    assert_eq!(msg.drs.template_name(), "complex");
}

#[test]
fn decode_complex_matches_eccodes_oracle() {
    let reader = Grib2Reader::from_bytes(COMPLEX_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("complex decode succeeds: {e:?}"))
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    assert_eq!(present.len(), COUNT, "value count");
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
