//! The committed libaec corpus is whole: every case's stream is present and
//! unaltered, every stream is a case, and libaec's recorded verdicts have the
//! shape each kind of case promises.
//!
//! This is the gate that keeps the digest comparison honest once the decoder
//! lands: a corpus that lost streams, gained stray ones, or had a stream
//! rewritten by a text hook (an appended newline is enough) fails here, not
//! as a silent drop in coverage.

mod common;

use std::collections::BTreeSet;

use common::{
    AEC_CASES, FIXTURES, GRID_ROWS, LIBAEC_COMMIT, SZ_CASES, int, manifest, narrow_u8, narrow_u16,
    rows, sha256_hex, text, uint,
};
use fieldglass_aec::{Flags, Params};

#[test]
fn the_header_names_the_pinned_libaec_and_the_pinned_counts() {
    let manifest = manifest();
    let header = &manifest["header"];
    assert_eq!(text(header, "libaec_version"), "1.1.7");
    assert_eq!(text(header, "libaec_commit"), LIBAEC_COMMIT);
    assert_eq!(text(header, "generator"), "tools/build_aec_fixtures.py");
    let counts = &header["counts"];
    assert_eq!(uint(counts, "params_grid"), GRID_ROWS as u64);
    assert_eq!(uint(counts, "aec_cases"), AEC_CASES as u64);
    assert_eq!(uint(counts, "sz_cases"), SZ_CASES as u64);
    rows(&manifest, "params_grid", GRID_ROWS);
    // check_code_options.c's id assertions, all of which the generator made.
    assert!(uint(header, "check_code_options_assertions") > 100_000);
}

#[test]
fn every_stream_is_present_unaltered_and_referenced_once() {
    let manifest = manifest();
    let mut referenced = BTreeSet::new();
    let aec = rows(&manifest, "aec_cases", AEC_CASES);
    let sz = rows(&manifest, "sz_cases", SZ_CASES);
    for case in aec.iter().chain(sz) {
        let stream = text(case, "stream");
        assert!(
            referenced.insert(stream.to_owned()),
            "{stream} is referenced twice"
        );
        let path = format!("{FIXTURES}/{stream}");
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(!bytes.is_empty(), "{path} is empty");
        assert_eq!(
            sha256_hex(&bytes),
            text(case, "stream_sha256"),
            "{path} is not the stream libaec wrote"
        );
    }

    let on_disk: BTreeSet<String> = std::fs::read_dir(format!("{FIXTURES}/streams"))
        .expect("tests/fixtures/streams")
        .map(|entry| format!("streams/{}", entry.unwrap().file_name().to_string_lossy()))
        .collect();
    let stray: Vec<_> = on_disk.difference(&referenced).collect();
    assert!(stray.is_empty(), "streams no case references: {stray:?}");
    assert_eq!(on_disk.len(), AEC_CASES + SZ_CASES);
}

#[test]
fn case_names_are_unique() {
    let manifest = manifest();
    let mut names = BTreeSet::new();
    let aec = rows(&manifest, "aec_cases", AEC_CASES);
    let sz = rows(&manifest, "sz_cases", SZ_CASES);
    for case in aec.iter().chain(sz) {
        let name = text(case, "name");
        assert!(names.insert(name), "duplicate case name {name}");
    }
}

/// libaec's verdict on each kind of case, as ADR-0012 decision 4 describes it.
#[test]
fn libaecs_verdicts_have_the_shape_each_kind_promises() {
    let manifest = manifest();
    let mut kinds = BTreeSet::new();
    for case in rows(&manifest, "aec_cases", AEC_CASES) {
        let name = text(case, "name");
        let kind = text(case, "kind");
        kinds.insert(kind.to_owned());
        let params = Params::new(
            narrow_u8(case, "bits_per_sample"),
            narrow_u16(case, "block_size"),
            narrow_u16(case, "rsi"),
            Flags::from_bits_truncate(narrow_u8(case, "flags")),
        )
        .unwrap_or_else(|e| panic!("{name}: libaec decoded it, but {e}"));
        let full = uint(case, "samples") * params.bytes_per_sample() as u64;
        let status = int(case, "libaec_status");
        let total_out = uint(case, "total_out");
        let (want_ok, want_full) = match kind {
            // Complete decodes: the digest is the oracle.
            "option" | "field" | "trailing" => (true, true),
            // libaec returns success with short output; fieldglass-aec errors.
            "truncated" => (true, false),
            // libaec errors after producing every sample; fieldglass-aec
            // never reads the bytes that trip it.
            "trailing_fill" => (false, true),
            // libaec's decoder refuses a stream its encoder wrote.
            "libaec_rejects" => (false, false),
            other => panic!("{name}: unknown kind {other}"),
        };
        assert_eq!(status == 0, want_ok, "{name}: libaec status {status}");
        assert_eq!(
            total_out == full,
            want_full,
            "{name}: {total_out} of {full} bytes"
        );
        assert!(total_out <= full, "{name}: more output than asked for");
        if kind == "libaec_rejects" {
            assert_eq!(text(case, "source_sha256").len(), 64, "{name}");
        }
    }
    let expected: BTreeSet<String> = [
        "option",
        "field",
        "trailing",
        "truncated",
        "trailing_fill",
        "libaec_rejects",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(kinds, expected, "every kind of case is present");
}

#[test]
fn every_option_the_encoder_can_be_forced_into_is_present() {
    // check_code_options.c's options, at each of its widths and orderings.
    let manifest = manifest();
    let names: BTreeSet<&str> = rows(&manifest, "aec_cases", AEC_CASES)
        .iter()
        .map(|c| text(c, "name"))
        .collect();
    for bps in [8, 16, 24, 32] {
        for ordering in ["nopp_lsb_u", "pp_lsb_u", "pp_lsb_s", "pp_msb_u", "pp_msb_s"] {
            let kmax = bps - 3;
            let options = ["zero", "se", "uncompressed", "fs"]
                .into_iter()
                .map(String::from)
                .chain((1..=kmax).map(|k| format!("split_k{k}")));
            for option in options {
                let name = format!("opt_b{bps:02}_{ordering}_{option}");
                assert!(names.contains(name.as_str()), "missing {name}");
            }
        }
    }
}

#[test]
fn every_szip_case_decoded_completely_under_libsz() {
    let manifest = manifest();
    let mut pixel_bits = BTreeSet::new();
    for case in rows(&manifest, "sz_cases", SZ_CASES) {
        let name = text(case, "name");
        assert_eq!(int(case, "libsz_status"), 0, "{name}");
        assert_eq!(uint(case, "total_out"), uint(case, "dest_len"), "{name}");
        pixel_bits.insert(uint(case, "bits_per_pixel"));
    }
    assert_eq!(pixel_bits, BTreeSet::from([8, 12, 16, 24, 32, 64]));
}
