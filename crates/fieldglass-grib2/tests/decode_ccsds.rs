//! End-to-end decode of GRIB2 CCSDS / AEC packing (DRS template 5.42) against
//! the bundled eccodes oracles.
//!
//! `ccsds_regular_latlon.grib2` is `regular_latlon_surface.grib2` (a 16×31
//! regular lat/lon surface field) re-encoded by eccodes 2.34.1 into
//! `grid_ccsds` (libaec). Three fixtures pack the same field at different
//! sample widths so all three AEC option-ID-length code paths are exercised:
//!
//! * `ccsds_regular_latlon_8bit.grib2` — 8 bits/value (AEC `id_len` = 3).
//! * `ccsds_regular_latlon.grib2` — 16 bits/value (`id_len` = 4).
//! * `ccsds_regular_latlon_24bit.grib2` — 24 bits/value (`id_len` = 5, the
//!   wide-sample / multi-byte path ECMWF uses for many operational fields).
//!
//! Each ships a sibling `*_expected.json` produced by eccodes `grib_get_data` /
//! `grib_get` (count, min/max/mean, anchored samples, and the full §5 CCSDS
//! parameters). The AEC stream is decoded by the pure-Rust `rust_aec` crate
//! (see ADR-0001); these tests are the byte-for-byte cross-check against the
//! eccodes oracle that gates that dependency. Provenance in
//! `tests/fixtures/NOTICE.md`.

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
    assert_eq!(msg.drs.template_number, 42, "{fixture}: DRS template 5.42");
    assert_eq!(msg.drs.template_name(), "ccsds", "{fixture}: template name");

    // §5 packing parameters must match the eccodes oracle exactly.
    let s5 = &oracle["section5"];
    let t = msg
        .drs
        .ccsds()
        .unwrap_or_else(|| panic!("{fixture}: §5 carries the CCSDS template"));
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
        t.ccsds_flags as u64,
        s5["ccsdsFlags"].as_u64().unwrap(),
        "{fixture}: ccsdsFlags",
    );
    assert_eq!(
        t.block_size as u64,
        s5["ccsdsBlockSize"].as_u64().unwrap(),
        "{fixture}: ccsdsBlockSize",
    );
    assert_eq!(
        t.reference_sample_interval as u64,
        s5["ccsdsRsi"].as_u64().unwrap(),
        "{fixture}: ccsdsRsi",
    );

    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{fixture}: CCSDS decode succeeds: {e:?}"))
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
fn ccsds_decodes_16bit_id_len_4() {
    assert_decode_matches_oracle("ccsds_regular_latlon.grib2");
}

#[test]
fn ccsds_decodes_8bit_id_len_3() {
    assert_decode_matches_oracle("ccsds_regular_latlon_8bit.grib2");
}

#[test]
fn ccsds_decodes_24bit_id_len_5() {
    assert_decode_matches_oracle("ccsds_regular_latlon_24bit.grib2");
}
