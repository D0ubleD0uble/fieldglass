//! A spectral message past what the synthesis grid carries, through `Session`
//! (#637): the decoded field is band-limited and says so, the message list
//! says so before decoding, and `probe_message` reads both the value the map
//! shows and the file's full-detail value at the same cell.
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

/// Below the limit nothing is labeled, and the exact probe is the ordinary
/// one to the last bit, since the map is the full sum there.
#[test]
fn a_field_the_grid_carries_is_unlabeled_and_probes_the_same_either_way() {
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
        let probe = session.probe(&field, lat, lon).expect("on the grid");
        let message = session
            .probe_message(0, lat, lon)
            .expect("probe_message")
            .expect("on the grid");
        assert_eq!(
            (
                message.lat,
                message.lon,
                message.i,
                message.j,
                message.value
            ),
            (probe.lat, probe.lon, probe.i, probe.j, probe.value),
            "{fixture}"
        );
        // One value: nothing is band-limited, so there is no second one.
        assert_eq!(message.full_detail, None, "{fixture}");
        assert_eq!(
            session.probe_full_detail(0, lat, lon).expect("full detail"),
            None,
            "{fixture}"
        );
    }
}

/// A click on a band-limited map reads both: the value the map shows there
/// (the T359 sum, matching the color) and the full-detail value (the T383
/// sum) at the same cell node. The oracle's first seven points are nodes of
/// the synthesis grid, so each probe lands exactly on its oracle point; the
/// rest are off it, and the full-detail value is then the one at the node the
/// probe snapped to, not at the raw click.
#[test]
fn a_probe_reads_the_shown_value_and_the_full_detail_value_at_one_cell() {
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
            let detail = session
                .probe_full_detail(0, lat, lon)
                .expect("full detail")
                .expect("band-limited");
            assert_eq!(detail.truncation, label().expect("label"));
            if point == &oracle["points"][3] {
                // `probe_message` is the two together: the same cell, the
                // point echoed back, the map's own value beside the full
                // detail. It decodes on every call, so once per fixture.
                let both = session
                    .probe_message(0, lat, lon)
                    .expect("probe_message")
                    .expect("on the map");
                assert_eq!(
                    (both.i, both.j, both.lat, both.lon, both.value),
                    (map.i, map.j, map.lat, map.lon, map.value)
                );
                assert_eq!(both.full_detail, Some(detail.clone()));
            }
            if !on_grid {
                // At the node, not the click: the full sum there equals the
                // one a click exactly on that node reads.
                let (i, j) = (map.i.round(), map.j.round());
                let node = session
                    .probe_full_detail(0, 90.0 - j * 0.5, i * 0.5)
                    .expect("full detail")
                    .expect("on the map");
                assert_eq!(node.value.to_bits(), detail.value.to_bits());
                continue;
            }
            let (full, truncated) = (
                point["full"].as_f64().unwrap(),
                point["truncated"].as_f64().unwrap(),
            );
            let shown = map.value.expect("map value");
            assert!(
                (shown - truncated).abs() < TOLERANCE,
                "{fixture} ({lat}, {lon}): shown {shown} vs T359 {truncated}"
            );
            assert!(
                (detail.value - full).abs() < TOLERANCE,
                "{fixture} ({lat}, {lon}): full detail {} vs T383 {full}",
                detail.value
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

/// Two messages in one session, as a host opens a file holding both: the
/// format readers walk concatenated messages.
fn session_of(paths: &[&str]) -> Session {
    let bytes: Vec<u8> = paths
        .iter()
        .flat_map(|p| std::fs::read(p).unwrap_or_else(|e| panic!("{p}: {e}")))
        .collect();
    Session::open(bytes).expect("open")
}

/// A combined map of a band-limited message with itself reads the combined
/// field for its shown value and `op` over the two full sums beside it (#815):
/// `A − A` is zero either way, `A + A` is twice each of the single probe's
/// values, `A / A` is one, all labeled with the message's own truncation.
///
/// Three ops at one node per edition, since each call decodes both operands;
/// the conformance cases hold every op (`conformance/probe_combined.json`).
#[test]
fn a_combined_probe_reads_both_values_of_a_band_limited_operand() {
    for (fixture, _) in SUBJECTS {
        let session = session(fixture);
        // A node of the synthesis grid, south and just west of the meridian.
        let (lat, lon) = (-30.0, 359.5);
        let single = session
            .probe_message(0, lat, lon)
            .expect("probe_message")
            .expect("on the grid");
        let full = single.full_detail.clone().expect("band-limited").value;
        let shown = single.value.expect("shown");
        for (tag, want_shown, want_full) in [
            ("a_minus_b", 0.0, 0.0),
            ("a_plus_b", shown + shown, full + full),
            ("ratio", 1.0, 1.0),
        ] {
            let op = fieldglass::op_from_wire(tag).expect("op");
            let p = session
                .probe_combined(0, 0, op, lat, lon)
                .expect("probe_combined")
                .expect("on the grid");
            // The same cell, the point echoed back.
            assert_eq!(
                (p.lat, p.lon, p.i, p.j),
                (single.lat, single.lon, single.i, single.j),
                "{fixture} {tag}"
            );
            assert_eq!(p.value, Some(want_shown), "{fixture} {tag} ({lat}, {lon})");
            let detail = p.full_detail.expect("a band-limited operand");
            assert_eq!(detail.value, want_full, "{fixture} {tag} ({lat}, {lon})");
            assert_eq!(Some(detail.truncation), label(), "{fixture} {tag}");
        }
    }
}

/// One band-limited operand against one the grid carries: the full-detail
/// value is the T383 message's full sum against the T63 message's own value,
/// which already holds every wavenumber its file does, on whichever side the
/// T383 message is, and labeled T383. Neither band-limited is one value.
#[test]
fn a_combined_probe_reads_the_other_operand_as_it_is() {
    let session = session_of(&[
        "../fieldglass-grib2/tests/fixtures/spectral_simple_t63.grib2",
        SUBJECTS[1].0,
    ]);
    let minus = fieldglass::op_from_wire("a_minus_b").expect("op");
    let (lat, lon) = (45.5, 120.0);
    let t63 = session
        .probe_message(0, lat, lon)
        .expect("t63")
        .expect("on the grid");
    let t383 = session
        .probe_message(1, lat, lon)
        .expect("t383")
        .expect("on the grid");
    assert_eq!(t63.full_detail, None, "T63 is not band-limited");
    let (low, shown, full) = (
        t63.value.expect("t63 value"),
        t383.value.expect("t383 shown"),
        t383.full_detail.expect("t383 band-limited").value,
    );
    assert_ne!(shown, full, "the two T383 values differ at this cell");

    let p = session
        .probe_combined(0, 1, minus, lat, lon)
        .expect("T63 − T383")
        .expect("on the grid");
    assert_eq!(p.value, Some(low - shown));
    let detail = p.full_detail.expect("one operand is band-limited");
    assert_eq!(detail.value, low - full);
    assert_eq!(Some(detail.truncation), label());

    let p = session
        .probe_combined(1, 0, minus, lat, lon)
        .expect("T383 − T63")
        .expect("on the grid");
    assert_eq!(p.value, Some(shown - low));
    let detail = p.full_detail.expect("one operand is band-limited");
    assert_eq!(detail.value, full - low);
    assert_eq!(Some(detail.truncation), label());

    let p = session
        .probe_combined(0, 0, minus, lat, lon)
        .expect("T63 − T63")
        .expect("on the grid");
    assert_eq!((p.value, p.full_detail), (Some(0.0), None));
}

/// Off the grid is `None`, and the call refuses what `combine` refuses, an
/// index past the end, and a container addressed by variables, each with its
/// own code.
#[test]
fn a_combined_probe_refuses_what_combine_refuses() {
    let minus = fieldglass::op_from_wire("a_minus_b").expect("op");
    let spectral = session(SUBJECTS[1].0);
    assert_eq!(
        spectral
            .probe_combined(0, 0, minus, 120.0, 45.5)
            .expect("off the grid is an answer"),
        None
    );
    let code = |r: Result<_, fieldglass::Error>| r.expect_err("refused").code();
    assert_eq!(
        code(spectral.probe_combined(0, 1, minus, 0.0, 0.0)),
        "no_such_message"
    );
    let unaligned = session_of(&[
        SUBJECTS[1].0,
        "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2",
    ]);
    assert_eq!(
        code(unaligned.probe_combined(0, 1, minus, 45.5, 120.0)),
        "unsupported"
    );
    let nc = session("../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc");
    assert_eq!(
        code(nc.probe_combined(0, 0, minus, 0.0, 0.0)),
        "wrong_addressing"
    );
}
