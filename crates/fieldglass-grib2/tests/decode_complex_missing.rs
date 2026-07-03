//! End-to-end decode of GRIB2 complex packing with inline missing-value
//! management (templates 5.2 / 5.3, Code Table 5.5 values 1 and 2) and
//! row-by-row group splitting (Code Table 5.4 value 0).
//!
//! Four derived fixtures pin these envelopes (provenance and byte-patch
//! recipes in `tests/fixtures/NOTICE.md`):
//!
//! - `complex_mvm1_regular_latlon.grib2` — 5.2, management 1 (primary
//!   substitutes), 46 of 496 points missing: a 40-point run plus scattered
//!   singles, so both the whole-missing-group and embedded-sentinel decode
//!   paths are exercised.
//! - `complex_spd2_mvm1_regular_latlon.grib2` — 5.3 second-order spatial
//!   differencing with the same missing pattern; the differencing recurrence
//!   must skip the missing points.
//! - `complex_mvm2_regular_latlon.grib2` — 5.2, management 2 (primary +
//!   secondary): two additional points whose packed offset equals the
//!   secondary sentinel become missing (48 total).
//! - `complex_rowbyrow_regular_latlon.grib2` — 5.2 with the splitting-method
//!   octet patched to 0; decodes identically to the general-splitting
//!   original.
//!
//! Each `<fixture>_expected.json` is the eccodes 2.34.1 `grib_get_data`
//! oracle: count, the exact missing indexes, min/max/mean over the defined
//! points, and anchored samples (missing samples recorded as `null`).

use fieldglass_grib2::Grib2Reader;
use serde_json::Value;
use std::path::Path;

fn assert_fixture_matches_oracle(fixture: &str) {
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

    // The oracle's missing indexes must be exactly our `None` positions.
    let expected_missing: Vec<usize> = oracle["missing_indexes"]
        .as_array()
        .expect("missing_indexes")
        .iter()
        .map(|v| v.as_u64().expect("index") as usize)
        .collect();
    let got_missing: Vec<usize> = decoded
        .iter()
        .enumerate()
        .filter_map(|(i, v)| v.is_none().then_some(i))
        .collect();
    assert_eq!(
        got_missing, expected_missing,
        "{fixture}: missing positions"
    );

    let tol = oracle["tolerance_absolute"].as_f64().expect("tolerance");
    let defined: Vec<f64> = decoded.iter().filter_map(|v| *v).collect();
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
        match (decoded[i], want.as_f64()) {
            (Some(got), Some(want)) => assert!(
                (got - want).abs() < tol,
                "{fixture}: values[{i}] was {got}, oracle {want}",
            ),
            (None, None) => {}
            (got, _) => panic!("{fixture}: values[{i}] was {got:?}, oracle {want}"),
        }
    }
}

#[test]
fn complex_mvm1_matches_eccodes_oracle() {
    assert_fixture_matches_oracle("complex_mvm1_regular_latlon");
}

#[test]
fn complex_spd2_mvm1_matches_eccodes_oracle() {
    assert_fixture_matches_oracle("complex_spd2_mvm1_regular_latlon");
}

#[test]
fn complex_mvm2_matches_eccodes_oracle() {
    assert_fixture_matches_oracle("complex_mvm2_regular_latlon");
}

#[test]
fn complex_rowbyrow_matches_eccodes_oracle() {
    assert_fixture_matches_oracle("complex_rowbyrow_regular_latlon");
}

/// The row-by-row fixture is the committed general-splitting fixture with one
/// §5 byte patched, so beyond matching the oracle it must decode identically
/// to the original.
#[test]
fn rowbyrow_decodes_identically_to_general_splitting() {
    let dir = Path::new("tests/fixtures");
    let decode = |name: &str| {
        let bytes = std::fs::read(dir.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        Grib2Reader::from_bytes(bytes)
            .unwrap_or_else(|e| panic!("{name}: parse: {e:?}"))
            .decode_message_values(0)
            .unwrap_or_else(|e| panic!("{name}: decode: {e:?}"))
    };
    assert_eq!(
        decode("complex_rowbyrow_regular_latlon.grib2"),
        decode("complex_regular_latlon.grib2"),
    );
}
