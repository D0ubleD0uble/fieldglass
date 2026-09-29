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
//! parameters). The AEC stream is decoded by this workspace's own
//! `fieldglass_aec` (see ADR-0012), which is checked against libaec in its own
//! crate; these tests are the GRIB2 backstop, the whole reader against the
//! eccodes oracle. Provenance in `tests/fixtures/NOTICE.md`.

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

// Real ECMWF open-data IFS 2m-temperature field: CCSDS / libaec packing on a
// 0.25° global regular lat/lon grid — the packing ECMWF ships for its open
// data. Confirms the pure-Rust AEC decoder handles a real ECMWF codestream, not
// just re-encoded fixtures. Provenance in fixtures/NOTICE.md.
#[test]
fn ecmwf_open_data_ccsds_decodes() {
    assert_decode_matches_oracle("ecmwf_ccsds_latlon.grib2");
}

// ---------------------------------------------------------------------------
// ccsdsFlags pins (#756).
//
// eccodes hands libaec the unsigned reference-subtracted value as an n-bit
// pattern and libaec's default build never writes RSI padding, so a 5.42
// message re-flagged SIGNED (13) or PAD_RSI (36, 46) still holds an unsigned,
// unpadded stream. We decode it to the source field (ADR-0012 decision 5);
// eccodes 2.34.1 does not (max 2234.68 and 2251.58 against a source max of
// 314.675, and AEC_DATA_ERROR on flag 46). The value oracle
// for each fixture is therefore its *source's* eccodes values, copied into
// `<fixture>_expected.json`, never eccodes' decode of the re-flagged file.
// Provenance and the divergence are in fixtures/NOTICE.md.
// ---------------------------------------------------------------------------

/// Every flag fixture and the committed fixture it was re-flagged from.
const FLAG_FIXTURES: [(&str, &str); 4] = [
    ("ccsds_flags13_12bit.grib2", "ecmwf_ccsds_latlon.grib2"),
    (
        "ccsds_flags13_24bit.grib2",
        "ccsds_regular_latlon_24bit.grib2",
    ),
    ("ccsds_flags36_12bit.grib2", "ecmwf_ccsds_latlon.grib2"),
    ("ccsds_flags46_12bit.grib2", "ecmwf_ccsds_latlon.grib2"),
];

/// A missing fixture fails here rather than skipping in the tests below.
#[test]
fn ccsds_flag_fixtures_are_all_present() {
    for (fixture, source) in FLAG_FIXTURES {
        for name in [fixture, source] {
            assert!(
                Path::new("tests/fixtures").join(name).is_file(),
                "missing fixture {name}"
            );
        }
    }
    let found = std::fs::read_dir("tests/fixtures")
        .expect("read fixtures dir")
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            (name.starts_with("ccsds_flags") && name.ends_with(".grib2")).then_some(name)
        })
        .count();
    assert_eq!(
        found,
        FLAG_FIXTURES.len(),
        "ccsds_flags*.grib2 fixture count"
    );
}

/// The re-flagged file decodes to the same values, bit for bit, as the
/// fixture it came from, and to that fixture's eccodes oracle.
fn assert_flags_decode_to_source(fixture: &str, source: &str, flags: u8) {
    assert_decode_matches_oracle(fixture);
    let read = |name: &str| {
        Grib2Reader::from_bytes(std::fs::read(Path::new("tests/fixtures").join(name)).unwrap())
            .unwrap_or_else(|e| panic!("{name}: parse: {e:?}"))
    };
    let (new, old) = (read(fixture), read(source));
    let t = new.messages[0].drs.ccsds().expect("5.42");
    assert_eq!(t.ccsds_flags, flags, "{fixture}: ccsdsFlags");
    assert_ne!(
        t.ccsds_flags,
        old.messages[0].drs.ccsds().unwrap().ccsds_flags,
        "{fixture}: must differ from its source's flags"
    );
    let got = new.decode_message_values(0).expect("flagged decode");
    let want = old.decode_message_values(0).expect("source decode");
    assert!(got == want, "{fixture}: values differ from {source}");
}

#[test]
fn ccsds_flags13_signed_12bit_decodes_to_source() {
    assert_flags_decode_to_source("ccsds_flags13_12bit.grib2", "ecmwf_ccsds_latlon.grib2", 13);
}

#[test]
fn ccsds_flags13_signed_24bit_decodes_to_source() {
    // 24-bit clears 3BYTE (13 = SIGNED + PP + MSB): the four-byte sample path.
    assert_flags_decode_to_source(
        "ccsds_flags13_24bit.grib2",
        "ccsds_regular_latlon_24bit.grib2",
        13,
    );
}

#[test]
fn ccsds_flags36_pad_rsi_decodes_to_source() {
    assert_flags_decode_to_source("ccsds_flags36_12bit.grib2", "ecmwf_ccsds_latlon.grib2", 36);
}

#[test]
fn ccsds_flags46_pad_rsi_preprocess_decodes_to_source() {
    // PAD_RSI + PP + 3BYTE + MSB. eccodes itself fails here (AEC_DATA_ERROR),
    // as does a decoder that honours PAD_RSI: the stream has no padding. The
    // reader clears the flag before decoding (ADR-0012 decision 5, D1), which
    // makes this an exact decode.
    assert_flags_decode_to_source("ccsds_flags46_12bit.grib2", "ecmwf_ccsds_latlon.grib2", 46);
}

/// The PAD_RSI rule is what makes flag 46 decode, not something incidental:
/// the codec itself, asked to honour the flag, refuses this stream, and with
/// the flag cleared it gives the source's integers.
#[test]
fn ccsds_flags46_decodes_only_because_pad_rsi_is_cleared() {
    use fieldglass_aec::{Flags, Params, decode_to_bytes};

    let bytes = std::fs::read("tests/fixtures/ccsds_flags46_12bit.grib2").expect("read fixture");
    let reader = Grib2Reader::from_bytes(bytes.clone()).expect("fixture parses");
    let msg = &reader.messages[0];
    let t = msg.drs.ccsds().expect("5.42");
    // §7 less its five-byte header.
    let start = usize::try_from(msg.ds_range.start).unwrap() + 5;
    let end = usize::try_from(msg.ds_range.start + msg.ds_range.len).unwrap();
    let payload = &bytes[start..end];
    let count = msg.drs.num_data_points as usize;

    let params = |flags: Flags| {
        Params::new(
            t.bits_per_value,
            u16::from(t.block_size),
            t.reference_sample_interval,
            flags,
        )
        .expect("valid parameters")
    };
    let as_stored = Flags::from_bits_truncate(t.ccsds_flags);
    assert!(as_stored.contains(Flags::PAD_RSI));
    let mut out = vec![0; count * 2];
    assert!(
        decode_to_bytes(payload, &params(as_stored), &mut out).is_err(),
        "honouring PAD_RSI must fail on this unpadded stream, or the rule is untested"
    );
    decode_to_bytes(
        payload,
        &params(as_stored.difference(Flags::PAD_RSI)),
        &mut out,
    )
    .expect("decodes with PAD_RSI cleared");
}
