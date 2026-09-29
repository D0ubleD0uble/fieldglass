//! The decoder against the committed libaec corpus, and the corpus itself.
//!
//! Every case decodes to its oracle's bytes: libaec's output where libaec is
//! correct, and the encoder's input where libaec's decoder refuses a stream
//! the standard reads (ADR-0012 decision 4). The corpus is also checked whole:
//! every case's stream is present and unaltered, every stream is a case, and
//! libaec's recorded verdicts have the shape each kind of case promises. A
//! corpus that lost streams, gained stray ones, or had a stream rewritten by a
//! text hook (an appended newline is enough) fails here, not as a silent drop
//! in coverage.

mod common;

use std::collections::BTreeSet;

use common::{
    AEC_CASES, FIXTURES, GRID_ROWS, LIBAEC_COMMIT, SZ_CASES, int, manifest, narrow_u8, narrow_u16,
    rows, sha256_hex, text, uint,
};
use fieldglass_aec::{AecError, Flags, Params, decode_to_bytes};

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

/// The decoder against every case: libaec's output where libaec is right,
/// and the encoder's input where libaec's decoder refuses a valid stream.
#[test]
fn every_case_decodes_to_the_oracles_bytes() {
    let manifest = manifest();
    let mut checked = 0;
    let mut failures = Vec::new();
    for case in rows(&manifest, "aec_cases", AEC_CASES) {
        let name = text(case, "name");
        let params = case_params(case);
        let samples = usize::try_from(uint(case, "samples")).unwrap();
        let path = format!("{FIXTURES}/{}", text(case, "stream"));
        let stream = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let mut out = vec![0u8; samples * params.bytes_per_sample()];
        let result = decode_to_bytes(&stream, &params, &mut out);
        let verdict = match (text(case, "kind"), result) {
            // Complete decodes, including the trailing-fill case libaec
            // errors on after its last sample: libaec's bytes, and `Ok`.
            ("option" | "field" | "trailing" | "trailing_fill", Ok(())) => {
                (sha256_hex(&out) == text(case, "output_sha256")).then_some(())
            }
            // libaec stops part-way with an error; the standard's reading is
            // the field the encoder was given.
            ("libaec_rejects", Ok(())) => {
                (sha256_hex(&out) == text(case, "source_sha256")).then_some(())
            }
            // libaec returns short output with success; this is an error.
            ("truncated", Err(AecError::Truncated { decoded, requested })) => {
                (decoded < requested && requested == samples).then_some(())
            }
            (_, Err(AecError::InvalidCode { .. })) => {
                failures.push(format!(
                    "{name}: a libaec-encoded stream tripped InvalidCode"
                ));
                continue;
            }
            _ => None,
        };
        if verdict.is_none() {
            failures.push(format!("{name}: {}", text(case, "kind")));
        }
        checked += 1;
    }
    assert!(
        failures.is_empty(),
        "{} failures: {failures:#?}",
        failures.len()
    );
    assert_eq!(checked, AEC_CASES);
}

/// Before the error, a truncated stream decodes to the same samples as the
/// whole one, and gets to within a block of where libaec's short output
/// stopped.
///
/// The truncated cases are cuts of `trailing_garbage_b16`'s stream (the same
/// parameters and field, with the garbage dropped), so that case's decode is
/// the reference for the prefix.
#[test]
fn a_truncated_stream_decodes_the_whole_streams_prefix_before_its_error() {
    let manifest = manifest();
    let cases = rows(&manifest, "aec_cases", AEC_CASES);
    let whole = cases
        .iter()
        .find(|c| text(c, "name") == "trailing_garbage_b16")
        .expect("trailing_garbage_b16");
    let params = case_params(whole);
    let width = params.bytes_per_sample();
    let samples = usize::try_from(uint(whole, "samples")).unwrap();
    let whole_stream = std::fs::read(format!("{FIXTURES}/{}", text(whole, "stream"))).unwrap();
    let mut reference = vec![0u8; samples * width];
    decode_to_bytes(&whole_stream, &params, &mut reference).unwrap();

    let mut seen = 0;
    for case in cases.iter().filter(|c| text(c, "kind") == "truncated") {
        let name = text(case, "name");
        assert_eq!(case_params(case), params, "{name}");
        let stream = std::fs::read(format!("{FIXTURES}/{}", text(case, "stream"))).unwrap();
        assert!(
            whole_stream.starts_with(&stream),
            "{name} is a cut of the whole stream"
        );

        // libaec's short output is a prefix of the whole field.
        let libaec_len = usize::try_from(uint(case, "total_out")).unwrap();
        assert_eq!(
            sha256_hex(&reference[..libaec_len]),
            text(case, "output_sha256"),
            "{name}"
        );

        let mut out = vec![0u8; samples * width];
        let Err(AecError::Truncated { decoded, requested }) =
            decode_to_bytes(&stream, &params, &mut out)
        else {
            panic!("{name}: not a truncation error");
        };
        assert_eq!(requested, samples, "{name}");
        let ours = decoded * width;
        assert_eq!(out[..ours], reference[..ours], "{name}");
        let block_bytes = usize::from(params.block_size()) * width;
        assert!(
            ours + block_bytes >= libaec_len,
            "{name}: {ours} of libaec's {libaec_len}"
        );
        seen += 1;
    }
    assert_eq!(seen, 2);
}

fn case_params(case: &serde_json::Value) -> Params {
    Params::new(
        narrow_u8(case, "bits_per_sample"),
        narrow_u16(case, "block_size"),
        narrow_u16(case, "rsi"),
        Flags::from_bits_truncate(narrow_u8(case, "flags")),
    )
    .unwrap_or_else(|e| panic!("{}: {e}", text(case, "name")))
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
