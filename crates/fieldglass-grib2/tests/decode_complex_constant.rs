//! End-to-end decode of GRIB2 complex packing with `NG == 0` — the
//! constant-field case (eccodes ECC-2095, templates 5.2 / 5.3).
//!
//! eccodes detects a constant complex-packed field by
//! `numberOfGroupsOfDataValues == 0` before reading anything from §7 and
//! returns the §5 reference value verbatim for every point — no
//! `2^E · 10^-D` transform. The two fixtures pin that behaviour for both
//! templates (byte-patch recipes in `tests/fixtures/NOTICE.md`):
//!
//! - `complex_ng0_regular_latlon.grib2` — 5.2, NG patched to 0, §7 truncated
//!   to its bare header.
//! - `complex_spd2_ng0_regular_latlon.grib2` — 5.3 second-order spatial
//!   differencing with the same patch; not even the spatial-differencing
//!   extra descriptors remain in §7.
//!
//! Each `<fixture>_expected.json` oracle was decoded with eccodes 2.47.3:
//! the ECC-2095 behaviour shipped in eccodes 2.42.0, so the otherwise-pinned
//! 2.34.1 cannot serve as the value oracle here (it predates the fix and
//! mis-decodes NG == 0).

use fieldglass_grib2::Grib2Reader;
use serde_json::Value;
use std::path::Path;

/// Decode the fixture, check it against the eccodes oracle (count, no
/// missing points, min/max/mean and anchored samples within tolerance), and
/// then assert every point equals the oracle's §5 `referenceValue` exactly
/// (f32 widened to f64, bitwise — the constant path applies no scale
/// transform, so no tolerance is allowed there).
fn assert_constant_field_matches_oracle(fixture: &str) {
    let dir = Path::new("tests/fixtures");
    let bytes = std::fs::read(dir.join(format!("{fixture}.grib2")))
        .unwrap_or_else(|e| panic!("{fixture}: read fixture: {e}"));
    let oracle: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(format!("{fixture}_expected.json")))
            .unwrap_or_else(|e| panic!("{fixture}: read oracle: {e}")),
    )
    .unwrap_or_else(|e| panic!("{fixture}: parse oracle: {e}"));

    let reader =
        Grib2Reader::from_bytes(bytes).unwrap_or_else(|e| panic!("{fixture}: parse: {e:?}"));
    let decoded = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{fixture}: decode: {e:?}"));

    assert_eq!(
        decoded.len(),
        oracle["count"].as_u64().expect("count") as usize,
        "{fixture}: value count",
    );
    assert_eq!(
        oracle["missing_count"].as_u64().expect("missing_count"),
        0,
        "{fixture}: oracle declares missing points in a constant field",
    );

    let defined: Vec<f64> = decoded
        .iter()
        .enumerate()
        .map(|(i, v)| v.unwrap_or_else(|| panic!("{fixture}: values[{i}] missing")))
        .collect();

    let tol = oracle["tolerance_absolute"].as_f64().expect("tolerance");
    let min = defined.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = defined.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = defined.iter().sum::<f64>() / defined.len() as f64;
    for (name, got) in [("min", min), ("max", max), ("mean", mean)] {
        let want = oracle[name].as_f64().expect(name);
        assert!(
            (got - want).abs() < tol,
            "{fixture}: {name} was {got}, oracle {want}",
        );
    }
    for (idx, want) in oracle["samples"].as_object().expect("samples") {
        let i: usize = idx.parse().expect("sample index");
        let want = want.as_f64().expect("sample value");
        let got = defined[i];
        assert!(
            (got - want).abs() < tol,
            "{fixture}: values[{i}] was {got}, oracle {want}",
        );
    }

    let reference = oracle["section5"]["referenceValue"]
        .as_f64()
        .expect("referenceValue");
    for (i, got) in defined.iter().enumerate() {
        assert!(
            *got == reference,
            "{fixture}: values[{i}] was {got}, expected referenceValue {reference} verbatim",
        );
    }
}

#[test]
fn complex_ng0_is_constant_reference_value() {
    assert_constant_field_matches_oracle("complex_ng0_regular_latlon");
}

#[test]
fn complex_spd2_ng0_is_constant_reference_value() {
    assert_constant_field_matches_oracle("complex_spd2_ng0_regular_latlon");
}
