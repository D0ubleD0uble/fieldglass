//! End-to-end decode of GRIB2 complex packing with spatial differencing
//! (DRS template 5.3) against the bundled eccodes oracles.
//!
//! Two fixtures exercise both differencing orders:
//!
//! * `gfs_c255_latlon.grib2` — a real NCEP GFS message, 1st-order spatial
//!   differencing (`orderOfSpatialDifferencing = 1`, 3 extra-descriptor
//!   octets) on a 144×73 grid with a §6 bitmap, leaving 2847 present points.
//! * `complex_spd2_regular_latlon.grib2` — the ECMWF 16×31 2-metre
//!   temperature field re-encoded by eccodes 2.34.1 into 2nd-order spatial
//!   differencing (`orderOfSpatialDifferencing = 2`, 2 extra-descriptor
//!   octets, 9 groups over 496 points).
//!
//! Each fixture ships a sibling `*_expected.json` produced by eccodes
//! `grib_get_data` / `grib_get` (counts, min/max/mean, anchored samples, and
//! the full §5 parameters). Provenance is recorded in `tests/fixtures/NOTICE.md`.

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

/// Decode the first message and assert it matches the oracle's count,
/// min/max/mean, and anchored samples within the oracle's stated tolerance.
fn assert_decode_matches_oracle(fixture: &str, expected_order: u64) {
    let (bytes, oracle) = load(fixture);
    let reader = Grib2Reader::from_bytes(bytes).expect("fixture parses");

    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 3, "{fixture}: DRS template 5.3");
    assert_eq!(
        msg.drs.template_name(),
        "complex_spatial_diff",
        "{fixture}: template name",
    );
    let spd = msg
        .drs
        .complex_spatial_diff()
        .unwrap_or_else(|| panic!("{fixture}: §5 carries the spatial-diff template"));
    assert_eq!(
        spd.spatial_diff_order as u64, expected_order,
        "{fixture}: order of spatial differencing",
    );

    // The oracle lists the present (un-masked) values in scan order; compact
    // out any §6-bitmap holes the same way before comparing.
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{fixture}: 5.3 decode succeeds: {e:?}"))
        .into_iter()
        .flatten()
        .collect();

    let count = oracle["count"].as_u64().expect("oracle count") as usize;
    let tol = oracle["tolerance_absolute"]
        .as_f64()
        .expect("oracle tolerance");
    assert_eq!(present.len(), count, "{fixture}: present value count");

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
        "{fixture}: mean {mean} vs {want_mean}",
    );

    for (idx, want) in oracle["samples"].as_object().expect("oracle samples") {
        let i: usize = idx.parse().expect("sample index is an integer");
        let want = want.as_f64().expect("sample value is a number");
        let got = present[i];
        assert!(
            (got - want).abs() < tol,
            "{fixture}: values[{i}] was {got}, expected {want}",
        );
    }
}

#[test]
fn gfs_c255_decodes_order_1_spatial_differencing() {
    assert_decode_matches_oracle("gfs_c255_latlon.grib2", 1);
}

#[test]
fn complex_spd2_decodes_order_2_spatial_differencing() {
    assert_decode_matches_oracle("complex_spd2_regular_latlon.grib2", 2);
}

// Real NCEP HRRR surface-temperature field: order-2 spatial differencing on a
// Lambert (3.30) grid — the packing/grid combination NCEP actually ships for
// HRRR/NAM today (they moved off JPEG 2000). Provenance in fixtures/NOTICE.md.
#[test]
fn hrrr_lambert_decodes_order_2_spatial_differencing() {
    assert_decode_matches_oracle("hrrr_complex_spd_lambert.grib2", 2);
}
