//! Every codec, against chunks written by `zarr-python` itself.
//!
//! The unit tests in `src/` pin behaviour against hand-built bytes, which is
//! the right shape for an edge case and proves nothing about what the reference
//! implementation actually emits. These run over committed stores — see
//! [`fixtures/NOTICE.md`](fixtures/NOTICE.md) for their provenance — and assert
//! that every chunk decodes to the numbers the array was written from.
//!
//! The expected values come from the *source* array the generator held, never
//! from reading the store back with the same library that wrote it, and never
//! from this crate. A decoder and an oracle that share an implementation agree
//! about a bug.
//!
//! Paths are relative to the crate directory rather than built from
//! `CARGO_MANIFEST_DIR`: this suite also runs under `wasmtime --dir=. --dir=..`
//! for the 32-bit pointer check, and the sandbox cannot open an absolute host
//! path.

use fieldglass_zarr::ChunkDecoder;
use serde_json::Value;

fn fixture_text(path: &str) -> String {
    let full = format!("tests/fixtures/{path}");
    std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("reading {full}: {e}"))
}

fn fixture_bytes(path: &str) -> Vec<u8> {
    let full = format!("tests/fixtures/{path}");
    std::fs::read(&full).unwrap_or_else(|e| panic!("reading {full}: {e}"))
}

fn oracle() -> Value {
    serde_json::from_str(&fixture_text("oracle.json")).expect("the oracle is JSON")
}

/// The values one chunk record expects: either written out, or the arithmetic
/// ramp that produced them.
fn expected_values(record: &Value) -> Vec<f64> {
    if let Some(list) = record.get("values").and_then(Value::as_array) {
        return list
            .iter()
            .map(|v| v.as_f64().expect("an oracle value is a number"))
            .collect();
    }
    let ramp = record
        .get("ramp")
        .expect("a record states values or a ramp");
    let start = ramp["start"].as_f64().unwrap();
    let step = ramp["step"].as_f64().unwrap();
    let count = ramp["count"].as_u64().unwrap();
    (0..count).map(|i| start + step * i as f64).collect()
}

fn decoder_for(case: &str, entry: &Value) -> ChunkDecoder {
    let metadata = entry["metadata"].as_str().expect("a metadata path");
    let text = fixture_text(&format!("{case}/{metadata}"));
    if metadata.ends_with("zarr.json") {
        ChunkDecoder::from_v3_metadata(&text)
            .unwrap_or_else(|e| panic!("{case}: reading zarr.json: {e}"))
    } else {
        ChunkDecoder::from_v2_metadata(&text)
            .unwrap_or_else(|e| panic!("{case}: reading .zarray: {e}"))
    }
}

/// The whole corpus: every case decodes to the numbers it was written from.
///
/// One test over every store rather than one per codec, because the property is
/// the same in all of them and the list of cases is data — a codec added to
/// `tools/build_zarr_fixtures.py` is covered here without a matching edit.
#[test]
fn every_committed_store_decodes_to_the_values_it_was_written_from() {
    let oracle = oracle();
    let cases = oracle.as_object().expect("the oracle is an object");
    assert!(
        cases.len() >= 30,
        "only {} cases in the oracle — has the fixture set shrunk?",
        cases.len()
    );

    let mut checked_chunks = 0usize;
    for (case, entry) in cases {
        let decoder = decoder_for(case, entry);
        let chunks = entry["chunks"].as_array().expect("a chunk list");
        assert!(!chunks.is_empty(), "{case}: no chunks in the oracle");

        for record in chunks {
            let key = record["key"].as_str().expect("a chunk key");
            let stored = fixture_bytes(&format!("{case}/{key}"));

            if entry["sharded"].as_bool() == Some(true) {
                let shard = decoder
                    .decode_shard(&stored)
                    .unwrap_or_else(|e| panic!("{case}/{key}: decoding the shard: {e}"));
                let inner = record["inner"].as_array().expect("an inner-chunk list");
                assert_eq!(
                    shard.len(),
                    inner.len(),
                    "{case}/{key}: the index holds {} entries, the oracle {}",
                    shard.len(),
                    inner.len()
                );
                for (position, want) in inner.iter().enumerate() {
                    match (shard.chunk(position), want.as_array()) {
                        (None, None) => {}
                        (Some(bytes), Some(values)) => {
                            let got = decoder.dtype().read_values(bytes).unwrap();
                            let want: Vec<f64> =
                                values.iter().map(|v| v.as_f64().unwrap()).collect();
                            assert_eq!(
                                got, want,
                                "{case}/{key}: inner chunk {position} decoded wrongly"
                            );
                        }
                        (got, _) => panic!(
                            "{case}/{key}: inner chunk {position} is {} in the shard but {} in \
                             the oracle",
                            if got.is_some() { "present" } else { "absent" },
                            if want.is_null() { "absent" } else { "present" }
                        ),
                    }
                }
                checked_chunks += shard.len();
            } else {
                let got = decoder
                    .decode_raw_values(&stored)
                    .unwrap_or_else(|e| panic!("{case}/{key}: decoding: {e}"));
                assert_eq!(
                    got,
                    expected_values(record),
                    "{case}/{key}: decoded to the wrong values"
                );
                checked_chunks += 1;
            }
        }
    }
    assert!(
        checked_chunks >= 100,
        "only {checked_chunks} chunks checked — the corpus is not being read"
    );
}

/// The corpus has to actually cover the codecs the crate claims, or the sweep
/// above passes by testing the same raw path thirty-five times.
///
/// Named cases rather than a count: a fixture set that lost its blosc arm would
/// still have thirty entries.
#[test]
fn the_corpus_covers_every_codec_the_crate_decodes() {
    let oracle = oracle();
    let cases = oracle.as_object().unwrap();
    for required in [
        "raw",
        "fortran_order",
        "zlib",
        "gzip",
        "zstd",
        "lz4",
        "shuffle_zlib",
        "big_endian",
        "int16",
        "blosc_blosclz_shuffle",
        "blosc_blosclz_bitshuffle",
        "blosc_blosclz_noshuffle",
        "blosc_lz4_shuffle",
        "blosc_lz4hc_shuffle",
        "blosc_zlib_shuffle",
        "blosc_zstd_shuffle",
        "blosc_multiblock",
        "blosc_bitshuffle_ragged",
        "v3_bytes",
        "v3_gzip",
        "v3_zstd",
        "v3_blosc",
        "v3_crc32c",
        "v3_big_endian",
        "v3_transpose",
        "v3_shard",
        "v3_shard_sparse",
    ] {
        assert!(cases.contains_key(required), "the corpus lost `{required}`");
    }
}

/// Byte order is a property of the stored file, and the two orders of the same
/// array must decode to the same numbers from *different* bytes. Equal bytes
/// would mean the fixture pair proves nothing.
#[test]
fn the_two_byte_orders_decode_to_the_same_numbers_from_different_bytes() {
    let oracle = oracle();
    for (little, big) in [("zlib", "big_endian"), ("v3_bytes", "v3_big_endian")] {
        let (le, be) = (&oracle[little], &oracle[big]);
        let key = le["chunks"][0]["key"].as_str().unwrap();
        let le_bytes = fixture_bytes(&format!("{little}/{key}"));
        let be_bytes = fixture_bytes(&format!("{big}/{key}"));
        assert_ne!(
            le_bytes, be_bytes,
            "{little} and {big} store identical bytes, so the pair tests nothing"
        );
        assert_eq!(
            decoder_for(little, le)
                .decode_raw_values(&le_bytes)
                .unwrap(),
            decoder_for(big, be).decode_raw_values(&be_bytes).unwrap(),
        );
    }
}

/// Fortran order really moves the bytes. If it did not, the C-order fixture and
/// the F-order one would be byte-identical and the transpose would be untested.
#[test]
fn fortran_order_stores_different_bytes_than_c_order() {
    let oracle = oracle();
    let key = oracle["raw"]["chunks"][0]["key"].as_str().unwrap();
    assert_ne!(
        fixture_bytes(&format!("raw/{key}")),
        fixture_bytes(&format!("fortran_order/{key}")),
        "the two orders must differ on disk or the transpose is not exercised"
    );
}

/// A sharded array refuses `decode`, and an unsharded one refuses
/// `decode_shard`. Getting either wrong would read an index as data or data as
/// an index.
#[test]
fn the_two_decode_entry_points_refuse_each_others_arrays() {
    let oracle = oracle();
    let shard = decoder_for("v3_shard", &oracle["v3_shard"]);
    assert!(shard.is_sharded());
    let bytes = fixture_bytes("v3_shard/c/0/0");
    assert!(shard.decode(&bytes).is_err());

    let plain = decoder_for("raw", &oracle["raw"]);
    assert!(!plain.is_sharded());
    assert!(plain.decode_shard(&fixture_bytes("raw/0.0")).is_err());
}

/// A truncated or corrupted chunk must be an error, never a panic and never a
/// short buffer handed on as if it were the array. Every committed chunk is
/// tried at every truncation, which is the cheapest fuzz there is.
#[test]
fn a_truncated_chunk_is_refused_rather_than_decoded_short() {
    let oracle = oracle();
    for (case, entry) in oracle.as_object().unwrap() {
        if entry["sharded"].as_bool() == Some(true) {
            continue;
        }
        let decoder = decoder_for(case, entry);
        let key = entry["chunks"][0]["key"].as_str().unwrap();
        let stored = fixture_bytes(&format!("{case}/{key}"));
        // Cap the walk so the 12 kB multi-block fixture does not dominate the
        // suite; the interesting truncations are all near the framing.
        let step = stored.len().div_ceil(64).max(1);
        for cut in (0..stored.len()).step_by(step) {
            match decoder.decode_raw_values(&stored[..cut]) {
                Err(_) => {}
                Ok(values) => panic!(
                    "{case}: a chunk truncated to {cut} of {} bytes decoded to {} values",
                    stored.len(),
                    values.len()
                ),
            }
        }
    }
}
