//! A spectral message past what the synthesis grid carries, through `Session`
//! (#637): the decoded field is band-limited and says so, the message list
//! says so before decoding, and `probe_message` reads the file's own value.
//!
//! The values have their oracle in the format crates
//! (`tests/spectral_truncation.rs` in each, against pyshtools). What is pinned
//! here is the routing: that the label reaches the DTOs every host binds, and
//! that the exact probe lands on the cell the ordinary probe does.

use fieldglass::{DecodeOptions, Dtype, Session, SpectralTruncation};
use serde_json::Value;

/// Both editions: they are separate arms of `Session::decode`.
const SUBJECTS: &[(&str, &str)] = &[
    (
        "../fieldglass-grib1/tests/fixtures/spectral_simple_t383.grib1",
        "../fieldglass-grib1/tests/fixtures/spectral_simple_t383.truncation.oracle.json",
    ),
    (
        "../fieldglass-grib2/tests/fixtures/spectral_simple_t383.grib2",
        "../fieldglass-grib2/tests/fixtures/spectral_simple_t383.truncation.oracle.json",
    ),
];

const TOLERANCE: f64 = 1e-7;

fn session(path: &str) -> Session {
    Session::open(std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))).expect("open")
}

fn oracle(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("oracle")).expect("parses")
}

fn label() -> Option<SpectralTruncation> {
    serde_json::from_str(r#"{"declared":383,"truncatedTo":359}"#).expect("label")
}

#[test]
fn the_label_reaches_the_message_and_the_field() {
    for (fixture, _) in SUBJECTS {
        let session = session(fixture);
        let info = session.message(0).expect("message");
        assert_eq!(info.truncation, label(), "{fixture}: message");
        // The declared size label still describes the file.
        assert_eq!(info.size_label.as_deref(), Some("T383"), "{fixture}");
        let field = session
            .decode(0, &DecodeOptions::new(Dtype::Auto))
            .expect("decode");
        assert_eq!(field.truncation, label(), "{fixture}: field");
        assert_eq!((field.ni, field.nj), (720, 361), "{fixture}");
    }
}

/// Below the limit nothing is labelled, and the exact probe is the ordinary
/// one to the last bit, since the map is the full sum there.
#[test]
fn a_field_the_grid_carries_is_unlabelled_and_probes_the_same_either_way() {
    for fixture in [
        "../fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1",
        "../fieldglass-grib2/tests/fixtures/spectral_complex_t63.grib2",
        "../fieldglass-grib2/tests/fixtures/healpix_n4_ring.grib2",
        "../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2",
    ] {
        let session = session(fixture);
        assert_eq!(session.message(0).expect("message").truncation, None);
        let field = session
            .decode(0, &DecodeOptions::new(Dtype::Auto))
            .expect("decode");
        assert_eq!(field.truncation, None, "{fixture}");
        // Inside every one of these grids, HRRR's CONUS Lambert included.
        let (lat, lon) = (38.3, -97.2);
        assert!(session.probe(&field, lat, lon).is_some(), "{fixture}");
        assert_eq!(
            session.probe_message(0, lat, lon).expect("probe_message"),
            session.probe(&field, lat, lon),
            "{fixture}"
        );
    }
}

/// The map reads the band-limited field and `probe_message` the full one, at
/// the same cell. The oracle's first seven points are nodes of the synthesis
/// grid, so each probe lands exactly on its oracle point.
#[test]
fn the_exact_probe_reads_the_full_sum_where_the_map_reads_the_band_limited_one() {
    for (fixture, oracle_path) in SUBJECTS {
        let session = session(fixture);
        let oracle = oracle(oracle_path);
        let field = session
            .decode(0, &DecodeOptions::new(Dtype::Auto))
            .expect("decode");
        for point in oracle["points"].as_array().expect("points") {
            let (lat, lon) = (
                point["lat"].as_f64().unwrap(),
                point["lon"].as_f64().unwrap(),
            );
            let on_grid = (lat * 2.0).fract() == 0.0 && (lon * 2.0).fract() == 0.0;
            let map = session.probe(&field, lat, lon).expect("on the map");
            let exact = session
                .probe_message(0, lat, lon)
                .expect("probe_message")
                .expect("on the map");
            // The same cell, and the point echoed back.
            assert_eq!(
                (exact.i, exact.j, exact.lat, exact.lon),
                (map.i, map.j, map.lat, map.lon)
            );
            if !on_grid {
                continue;
            }
            let (full, truncated) = (
                point["full"].as_f64().unwrap(),
                point["truncated"].as_f64().unwrap(),
            );
            let map_value = map.value.expect("map value");
            let exact_value = exact.value.expect("exact value");
            assert!(
                (map_value - truncated).abs() < TOLERANCE,
                "{fixture} ({lat}, {lon}): map {map_value} vs T359 {truncated}"
            );
            assert!(
                (exact_value - full).abs() < TOLERANCE,
                "{fixture} ({lat}, {lon}): probe {exact_value} vs T383 {full}"
            );
        }
    }
}

/// A combined field is band-limited when either operand is, and says so.
#[test]
fn a_combined_field_keeps_the_label() {
    let a = session(SUBJECTS[1].0);
    let b = session("../fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2");
    let opts = DecodeOptions::new(Dtype::Auto);
    let fa = a.decode(0, &opts).expect("a");
    let fb = b.decode(0, &opts).expect("b");
    let op = fieldglass::op_from_wire("a_minus_b").expect("op");
    assert_eq!(a.combine(&fa, &fb, op).expect("a-b").truncation, label());
    assert_eq!(a.combine(&fb, &fa, op).expect("b-a").truncation, label());
    assert_eq!(a.combine(&fb, &fb, op).expect("b-b").truncation, None);
}
