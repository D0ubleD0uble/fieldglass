//! End-to-end decode of the pre-standard local-use GRIB2 packing templates
//! 5.40010 (PNG) and 5.40000 (JPEG 2000) — part of #307.
//!
//! Both are NCEP pre-standard templates whose §5/§7 are byte-identical to a
//! registered image packing, so Fieldglass decodes them through the same codec
//! (5.40010 via the PNG path, 5.40000 via the JPEG 2000 path). Each fixture is
//! the matching committed image fixture with only its §5 template number
//! relabelled (built by `tools/build_grib2_local_template_fixtures.py`); the §7
//! codestream is untouched.
//!
//! Oracles (`*_expected.json`):
//! * `jpeg2000_local_40000.grib2` — eccodes decodes it as `grid_jpeg`, so the
//!   oracle is eccodes' decode of the relabelled file.
//! * `png_local_40010.grib2` — eccodes has no 5.40010 definition and cannot
//!   decode it (a genuine exceed-eccodes case), so the oracle is eccodes'
//!   decode of the original 5.41 fixture, whose §7 is identical.
//!
//! Provenance in `tests/fixtures/NOTICE.md`.

use fieldglass_grib2::Grib2Reader;
use serde_json::Value;
use std::path::Path;

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

/// Decode the first message and assert its local template number, the codec it
/// decodes through, and its values against the oracle.
fn assert_decode_matches_oracle(fixture: &str, expected_template: u16, packing_name: &str) {
    let (bytes, oracle) = load(fixture);
    let reader = Grib2Reader::from_bytes(bytes).expect("fixture parses");

    let msg = &reader.messages[0];
    assert_eq!(
        msg.drs.template_number, expected_template,
        "{fixture}: DRS template number",
    );
    // The local template decodes *through* a registered codec, so its
    // human-readable packing name is that codec's.
    assert_eq!(
        msg.drs.template_name(),
        packing_name,
        "{fixture}: decodes via the {packing_name} codec",
    );

    let present: Vec<f64> = reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{fixture}: decode succeeds: {e:?}"))
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

    for (idx, want) in oracle["samples"].as_object().expect("oracle samples") {
        let i: usize = idx.parse().expect("sample index is an integer");
        let want = want.as_f64().expect("sample value is a number");
        assert!(
            (present[i] - want).abs() < tol,
            "{fixture}: values[{i}] was {}, expected {want}",
            present[i]
        );
    }
}

#[test]
fn local_png_40010_decodes_via_png() {
    assert_decode_matches_oracle("png_local_40010.grib2", 40010, "png");
}

#[test]
fn local_jpeg2000_40000_decodes_via_jpeg2000() {
    assert_decode_matches_oracle("jpeg2000_local_40000.grib2", 40000, "jpeg");
}
