//! End-to-end decode of GRIB2 run-length packing (DRS template 5.200) against
//! the bundled eccodes oracles.
//!
//! Run-length packing (JMA radar, rain-gauge analysis, and nowcast products)
//! encodes a field as runs of quantised level indices resolved through a
//! level → value table; level 0 marks missing. eccodes 2.34.1 (the pin)
//! decodes `grid_run_length` but cannot CLI-encode it, so the two fixtures are
//! hand-built by `tools/build_grib2_runlength_fixtures.py` (§0–§4 reused from
//! `regular_latlon_surface.grib2`) and eccodes' *decode* is the value oracle:
//!
//! * `runlength_regular_latlon.grib2` — 8 bits/value, decimalScaleFactor = 1.
//!   A run longer than `range` exercises the multi-digit base-`range` run
//!   length; a level-0 run exercises missing.
//! * `runlength_4bit_regular_latlon.grib2` — 4 bits/value, decimalScaleFactor
//!   raw byte 129 (sign-magnitude → −1). Exercises sub-byte code packing,
//!   base-10 multi-digit runs, single-point runs, and a negative decimal
//!   scale.
//!
//! Each ships a sibling `*_expected.json` (eccodes `grib_get_data` / `grib_get`:
//! count, missing count, min/max/mean over present points, anchored samples,
//! and the full §5 run-length parameters). Provenance in `tests/fixtures/NOTICE.md`.

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
/// count, missing count, min/max/mean over present points, and anchored
/// samples (a `null` sample is a missing point) within the oracle's tolerance.
fn assert_decode_matches_oracle(fixture: &str) {
    let (bytes, oracle) = load(fixture);
    let reader = Grib2Reader::from_bytes(bytes).expect("fixture parses");

    let msg = &reader.messages[0];
    assert_eq!(
        msg.drs.template_number, 200,
        "{fixture}: DRS template 5.200"
    );
    assert_eq!(
        msg.drs.template_name(),
        "run_length",
        "{fixture}: template name"
    );

    // §5 run-length parameters must match the eccodes oracle exactly.
    let s5 = &oracle["section5"];
    let t = msg
        .drs
        .run_length()
        .unwrap_or_else(|| panic!("{fixture}: §5 carries the run-length template"));
    assert_eq!(
        t.bits_per_value as u64,
        s5["bitsPerValue"].as_u64().unwrap(),
        "{fixture}: bitsPerValue",
    );
    assert_eq!(
        t.max_level_value as u64,
        s5["maxLevelValue"].as_u64().unwrap(),
        "{fixture}: maxLevelValue",
    );
    assert_eq!(
        t.number_of_level_values as u64,
        s5["numberOfLevelValues"].as_u64().unwrap(),
        "{fixture}: numberOfLevelValues",
    );
    // The oracle records eccodes' raw decimalScaleFactor byte; our parser
    // applies the same single-octet sign-magnitude rule (raw > 127 → negative),
    // captured in `decimalScaleFactorSigned`.
    assert_eq!(
        t.decimal_scale_factor as i64,
        s5["decimalScaleFactorSigned"].as_i64().unwrap(),
        "{fixture}: decimalScaleFactor (signed)",
    );

    let decoded = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{fixture}: run-length decode succeeds: {e:?}"));

    let count = oracle["count"].as_u64().expect("oracle count") as usize;
    let missing_count = oracle["missing_count"].as_u64().expect("oracle missing") as usize;
    let tol = oracle["tolerance_absolute"]
        .as_f64()
        .expect("oracle tolerance");
    assert_eq!(decoded.len(), count, "{fixture}: value count");
    assert_eq!(
        decoded.iter().filter(|v| v.is_none()).count(),
        missing_count,
        "{fixture}: missing count",
    );

    let present: Vec<f64> = decoded.iter().filter_map(|v| *v).collect();
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
        match (want.as_f64(), decoded[i]) {
            (Some(w), Some(got)) => assert!(
                (got - w).abs() < tol,
                "{fixture}: values[{i}] was {got}, expected {w}"
            ),
            (None, None) => {}
            (w, got) => panic!("{fixture}: values[{i}] was {got:?}, expected {w:?}"),
        }
    }
}

#[test]
fn runlength_decodes_8bit() {
    assert_decode_matches_oracle("runlength_regular_latlon.grib2");
}

#[test]
fn runlength_decodes_4bit_negative_decimal_scale() {
    assert_decode_matches_oracle("runlength_4bit_regular_latlon.grib2");
}
