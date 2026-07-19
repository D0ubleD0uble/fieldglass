//! End-to-end decode of GRIB2 second-order packing (DRS templates 5.50001 and
//! 5.50002) against the bundled eccodes oracles.
//!
//! Second-order (general-extended) packing is the GRIB1 `grid_second_order`
//! codec carried into GRIB2: §5 holds the R/E/D transform, the group-descriptor
//! bit widths, the group count, and the spatial-predictor-differencing (SPD)
//! seeds; §7 holds the per-group widths, lengths, first-order references, and
//! the second-order packed offsets. Unlike run-length, eccodes 2.34.1 (the pin)
//! *can* CLI-encode it, so the fixtures are repacked from
//! `regular_latlon_surface.grib2` by `tools/build_grib2_second_order_fixtures.py`:
//!
//! * `second_order_regular_latlon.grib2` — `grid_second_order` (template
//!   5.50002, boustrophedonicOrdering = 0). The common operational case.
//! * `second_order_no_boust_regular_latlon.grib2` —
//!   `grid_second_order_no_boustrophedonic` (template 5.50001). Same field with
//!   no `secondOrderFlags` octet.
//! * `second_order_boust_regular_latlon.grib2` — the 5.50002 fixture with
//!   `secondOrderFlags = 0x80` (boustrophedonicOrdering = 1), so eccodes
//!   reverses the odd rows on decode. Exercises the alternating-row path.
//!
//! Each ships a sibling `*_expected.json` carrying the **full** eccodes decode
//! (`grib_get_data`, in scan order) plus the §5 parameters and summary stats.
//! The test asserts value-for-value agreement with that decode. Provenance in
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
/// count, and — the primary check — every decoded value against eccodes' full
/// decode within the oracle's tolerance.
fn assert_decode_matches_oracle(fixture: &str, template_number: u16) {
    let (bytes, oracle) = load(fixture);
    let reader = Grib2Reader::from_bytes(bytes).expect("fixture parses");

    let msg = &reader.messages[0];
    assert_eq!(
        msg.drs.template_number, template_number,
        "{fixture}: DRS template number"
    );
    assert_eq!(
        msg.drs.template_name(),
        "second_order",
        "{fixture}: template name"
    );

    // §5 second-order parameters must match the eccodes oracle exactly.
    let s5 = &oracle["section5"];
    let t = msg
        .drs
        .second_order()
        .unwrap_or_else(|| panic!("{fixture}: §5 carries the second-order template"));
    for (field, got) in [
        ("bitsPerValue", t.bits_per_value as u64),
        ("numberOfGroups", t.num_groups as u64),
        (
            "widthOfFirstOrderValues",
            t.width_of_first_order_values as u64,
        ),
        ("widthOfWidths", t.width_of_widths as u64),
        ("widthOfLengths", t.width_of_lengths as u64),
        ("orderOfSPD", t.order_of_spd as u64),
        ("widthOfSPD", t.width_of_spd as u64),
    ] {
        assert_eq!(
            got,
            s5[field]
                .as_u64()
                .unwrap_or_else(|| panic!("{fixture}: oracle {field}")),
            "{fixture}: {field}",
        );
    }
    // The boustrophedonic flag lives on 5.50002 only; 5.50001 is always false.
    let want_boust = s5["boustrophedonicOrdering"].as_u64().unwrap_or(0) == 1;
    assert_eq!(t.boustrophedonic, want_boust, "{fixture}: boustrophedonic");

    let decoded = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{fixture}: second-order decode succeeds: {e:?}"));

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

    // Primary check: value-for-value against eccodes' full decode.
    let want_values = oracle["values"].as_array().expect("oracle values");
    assert_eq!(want_values.len(), decoded.len(), "{fixture}: oracle length");
    for (i, (got, want)) in decoded.iter().zip(want_values).enumerate() {
        match (got, want.as_f64()) {
            (Some(g), Some(w)) => assert!(
                (g - w).abs() < tol,
                "{fixture}: values[{i}] was {g}, expected {w} (Δ {})",
                (g - w).abs()
            ),
            (None, None) => {}
            (g, w) => panic!("{fixture}: values[{i}] was {g:?}, expected {w:?}"),
        }
    }

    // Cross-check the summary stats over present points.
    let present: Vec<f64> = decoded.iter().filter_map(|v| *v).collect();
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;
    assert!(
        (min - oracle["min"].as_f64().unwrap()).abs() < tol,
        "{fixture}: min {min}"
    );
    assert!(
        (max - oracle["max"].as_f64().unwrap()).abs() < tol,
        "{fixture}: max {max}"
    );
    assert!(
        (mean - oracle["mean"].as_f64().unwrap()).abs() < tol,
        "{fixture}: mean {mean}"
    );
}

#[test]
fn second_order_50002_decodes() {
    assert_decode_matches_oracle("second_order_regular_latlon.grib2", 50002);
}

#[test]
fn second_order_50001_no_boustrophedonic_decodes() {
    assert_decode_matches_oracle("second_order_no_boust_regular_latlon.grib2", 50001);
}

#[test]
fn second_order_50002_boustrophedonic_decodes() {
    assert_decode_matches_oracle("second_order_boust_regular_latlon.grib2", 50002);
}

/// The 5.50001 and 5.50002 (non-boustrophedonic) fixtures encode the same
/// source field, so their decodes must agree value-for-value — the two
/// templates differ only by the `secondOrderFlags` octet.
#[test]
fn second_order_50001_and_50002_agree() {
    let r1 = Grib2Reader::from_bytes(
        std::fs::read("tests/fixtures/second_order_no_boust_regular_latlon.grib2").unwrap(),
    )
    .unwrap();
    let r2 = Grib2Reader::from_bytes(
        std::fs::read("tests/fixtures/second_order_regular_latlon.grib2").unwrap(),
    )
    .unwrap();
    let v1 = r1.decode_message_values(0).unwrap();
    let v2 = r2.decode_message_values(0).unwrap();
    assert_eq!(v1, v2, "5.50001 and 5.50002 decode the same field");
}
