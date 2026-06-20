//! End-to-end decode of GRIB2 PNG packing (DRS template 5.41).
//!
//! `png_eta_lambert.grib2` is `eta_lambert_msg0.grib2` (a 93×65 Lambert
//! pressure field, simple packing 5.0) re-encoded by eccodes 2.34.1 into
//! `grid_png` at its native 13-bit depth — a full-fidelity round-trip, so the
//! decoded range matches the simple-packed source exactly. The committed
//! `png_eta_lambert_expected.json` is the `grib_get_data` oracle (counts,
//! min/max/mean, anchored samples) plus the §5 packing parameters. Grid type
//! is irrelevant to the §5/§7 PNG decode path (decode is decoupled from grid
//! geometry), so the Lambert grid here is incidental. Provenance in
//! `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;

const PNG_FIXTURE: &[u8] = include_bytes!("fixtures/png_eta_lambert.grib2");

const COUNT: usize = 6045;
const MIN: f64 = 97_392.0;
const MAX: f64 = 102_712.0;
const MEAN: f64 = 101_439.169_892_473_12;

/// Anchored samples from the eccodes oracle.
const SAMPLES: &[(usize, f64)] = &[
    (0, 101_333.0),
    (1, 101_342.0),
    (100, 101_454.0),
    (1000, 101_890.0),
    (6044, 100_828.0),
];

#[test]
fn png_section_reports_template_5_41() {
    let reader = Grib2Reader::from_bytes(PNG_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 41, "DRS template 5.41");
    assert_eq!(msg.drs.template_name(), "png");

    // §5 packing parameters from the eccodes oracle.
    let t = msg.drs.png().expect("5.41 carries a PNG template");
    assert!((t.reference_value - 97_392.0).abs() < 1.0, "referenceValue");
    assert_eq!(t.binary_scale_factor, 0, "binaryScaleFactor");
    assert_eq!(t.decimal_scale_factor, 0, "decimalScaleFactor");
    assert_eq!(t.bits_per_value, 13, "bitsPerValue");
}

#[test]
fn decode_png_matches_eccodes_oracle() {
    let reader = Grib2Reader::from_bytes(PNG_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("PNG decode succeeds: {e:?}"))
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
