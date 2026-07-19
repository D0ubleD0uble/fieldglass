//! End-to-end decode of GRIB2 simple packing with logarithmic pre-processing
//! (DRS template 5.61) against the bundled eccodes oracles.
//!
//! Template 5.61 packs the natural logarithm of the field with ordinary simple
//! packing, so the decode is simple unpacking followed by `Y = exp(X) - B`,
//! where `B` is the §5 `preProcessingParameter`. eccodes 2.34.1 (the pin) both
//! encodes and decodes `grid_simple_log_preprocessing`, so the two fixtures are
//! eccodes re-encodings of `regular_latlon_surface.grib2` (built by
//! `tools/build_grib2_log_preprocessing_fixtures.py`) with eccodes' decode as
//! the value oracle:
//!
//! * `log_regular_latlon.grib2` — all-positive field, `preProcessingParameter
//!   = 0`, so decode is `Y = exp(X)`.
//! * `log_negative_regular_latlon.grib2` — the field shifted −300 K so it holds
//!   non-positive values, giving a non-zero `preProcessingParameter` and
//!   exercising the `Y = exp(X) - B` branch.
//!
//! Each ships a sibling `*_expected.json` (count, min/max/mean, anchored
//! samples, and the §5 log-packing parameters). Because the decode reconstructs
//! values through `exp()`, the oracle tolerance is slightly looser than the
//! linear packings'. Provenance in `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;
use serde_json::Value;
use std::path::Path;

/// Load a fixture's bytes and its `*_expected.json` value oracle.
fn load(fixture: &str) -> (Vec<u8>, Value) {
    let dir = Path::new("tests/fixtures");
    let bytes =
        std::fs::read(dir.join(fixture)).unwrap_or_else(|e| panic!("read fixture {fixture}: {e}"));
    let stem = fixture
        .strip_suffix(".grib2")
        .expect("fixture is a .grib2 file");
    let oracle_path = dir.join(format!("{stem}_expected.json"));
    let text = std::fs::read_to_string(&oracle_path)
        .unwrap_or_else(|e| panic!("read oracle {}: {e}", oracle_path.display()));
    let oracle = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse oracle {}: {e}", oracle_path.display()));
    (bytes, oracle)
}

/// Decode the first message and assert it matches the oracle's §5 parameters,
/// count, min/max/mean, and anchored samples within the oracle's tolerance.
fn assert_decode_matches_oracle(fixture: &str) {
    let (bytes, oracle) = load(fixture);
    let reader = Grib2Reader::from_bytes(bytes).expect("fixture parses");

    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 61, "{fixture}: DRS template 5.61");
    assert_eq!(
        msg.drs.template_name(),
        "simple_log_preprocessing",
        "{fixture}: template name"
    );

    // §5 log-packing parameters must match the eccodes oracle.
    let s5 = &oracle["section5"];
    let t = msg
        .drs
        .log_preprocessing()
        .unwrap_or_else(|| panic!("{fixture}: §5 carries the log-preprocessing template"));
    assert_eq!(
        t.bits_per_value as u64,
        s5["bitsPerValue"].as_u64().unwrap(),
        "{fixture}: bitsPerValue",
    );
    assert_eq!(
        t.binary_scale_factor as i64,
        s5["binaryScaleFactor"].as_i64().unwrap(),
        "{fixture}: binaryScaleFactor",
    );
    assert_eq!(
        t.decimal_scale_factor as i64,
        s5["decimalScaleFactor"].as_i64().unwrap(),
        "{fixture}: decimalScaleFactor",
    );
    assert!(
        (t.pre_processing_parameter as f64 - s5["preProcessingParameter"].as_f64().unwrap()).abs()
            < 1e-3,
        "{fixture}: preProcessingParameter",
    );

    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{fixture}: log-preprocessing decode succeeds: {e:?}"))
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    let count = oracle["count"].as_u64().expect("oracle count") as usize;
    let tol = oracle["tolerance_absolute"]
        .as_f64()
        .expect("oracle tolerance");
    assert_eq!(present.len(), count, "{fixture}: value count");

    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;

    let want_min = oracle["min"].as_f64().expect("oracle min");
    let want_max = oracle["max"].as_f64().expect("oracle max");
    let want_mean = oracle["mean"].as_f64().expect("oracle mean");
    assert!(
        (min - want_min).abs() < tol,
        "{fixture}: min {min} vs {want_min}"
    );
    assert!(
        (max - want_max).abs() < tol,
        "{fixture}: max {max} vs {want_max}"
    );
    assert!(
        (mean - want_mean).abs() < tol,
        "{fixture}: mean {mean} vs {want_mean}"
    );

    for (idx, want) in oracle["samples"].as_object().expect("oracle samples") {
        let i: usize = idx.parse().expect("sample index is an integer");
        let want = want.as_f64().expect("sample value is a number");
        let got = present[i];
        assert!(
            (got - want).abs() < tol,
            "{fixture}: values[{i}] was {got}, expected {want}"
        );
    }
}

#[test]
fn log_preprocessing_decodes_zero_parameter() {
    // preProcessingParameter == 0 → Y = exp(X).
    assert_decode_matches_oracle("log_regular_latlon.grib2");
}

#[test]
fn log_preprocessing_decodes_nonzero_parameter() {
    // preProcessingParameter != 0 → Y = exp(X) - B.
    assert_decode_matches_oracle("log_negative_regular_latlon.grib2");
}
