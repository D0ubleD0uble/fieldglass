//! End-to-end decode of GRIB2 CCSDS / AEC packing (DRS template 5.42).
//!
//! `ccsds_regular_latlon.grib2` is `regular_latlon_surface.grib2` (a 16×31
//! regular lat/lon surface field) re-encoded by eccodes 2.34.1 into
//! `grid_ccsds` (libaec) at 16-bit depth — so the decoded field matches the
//! source within the packing precision. The committed
//! `ccsds_regular_latlon_expected.json` is the `grib_get_data` oracle (counts,
//! min/max/mean, anchored samples) plus the §5 packing parameters. Grid type
//! is irrelevant to the §5/§7 CCSDS decode path (decode is decoupled from grid
//! geometry), so the regular lat/lon grid here is incidental. The AEC stream is
//! decoded by the pure-Rust `rust_aec` crate (see ADR-0001); this test is the
//! byte-for-byte cross-check against the eccodes oracle that gates that
//! dependency. Provenance in `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;

const CCSDS_FIXTURE: &[u8] = include_bytes!("fixtures/ccsds_regular_latlon.grib2");

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
fn ccsds_section_reports_template_5_42() {
    let reader = Grib2Reader::from_bytes(CCSDS_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 42, "DRS template 5.42");
    assert_eq!(msg.drs.template_name(), "ccsds");

    // §5 packing parameters from the eccodes oracle.
    let t = msg.drs.ccsds().expect("5.42 carries a CCSDS template");
    assert!((t.reference_value - 270.467).abs() < 1e-2, "referenceValue");
    assert_eq!(t.binary_scale_factor, -10, "binaryScaleFactor");
    assert_eq!(t.decimal_scale_factor, 0, "decimalScaleFactor");
    assert_eq!(t.bits_per_value, 16, "bitsPerValue");
    assert_eq!(t.ccsds_flags, 14, "ccsdsFlags");
    assert_eq!(t.block_size, 32, "ccsdsBlockSize");
    assert_eq!(t.reference_sample_interval, 128, "ccsdsRsi");
}

#[test]
fn decode_ccsds_matches_eccodes_oracle() {
    let reader = Grib2Reader::from_bytes(CCSDS_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("CCSDS decode succeeds: {e:?}"))
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
