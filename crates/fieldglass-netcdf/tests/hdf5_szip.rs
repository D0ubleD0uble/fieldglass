//! The szip filter (HDF5 filter id 4, issue #421).
//!
//! szip is common across the NASA EOS archive (AIRS, MODIS and relatives). Its
//! coder is CCSDS 121.0 adaptive entropy coding, decoded by `fieldglass-aec`;
//! the reader adds HDF5's framing: the `cd_values` order and the 4-byte size
//! prefix on every chunk.
//!
//! Two fixtures, both built by `tools/build_hdf5_fixtures.py`, each with an
//! oracle holding the h5py (libhdf5 2.0.0, libaec 1.1.4) read-back of every
//! value:
//!
//! - `hdf5_szip.h5`: datasets libhdf5 wrote itself.
//! - `hdf5_szip_hand.h5`: chunks its writer never produces, compressed with the
//!   same libsz and stored with `write_direct_chunk`. Scanlines shorter than a
//!   block (RSI 1), and szip after deflate.
//!
//! A third, `hdf5_szip_long_stream.h5`, holds one chunk the reader must
//! refuse: its stream codes more 64-bit pixels than the chunk holds, and
//! libhdf5 reads it back scrambled (#794).
//!
//! See `tests/fixtures/NOTICE.md` for what each dataset covers.

use fieldglass_core::FieldglassError;
use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};
use serde_json::Value;
use std::collections::BTreeMap;

const SZIP: &[u8] = include_bytes!("fixtures/hdf5_szip.h5");
const SZIP_ORACLE: &str = include_str!("fixtures/hdf5_szip.h5.oracle.json");
const HAND: &[u8] = include_bytes!("fixtures/hdf5_szip_hand.h5");
const HAND_ORACLE: &str = include_str!("fixtures/hdf5_szip_hand.h5.oracle.json");
const LONG: &[u8] = include_bytes!("fixtures/hdf5_szip_long_stream.h5");
const LONG_ORACLE: &str = include_str!("fixtures/hdf5_szip_long_stream.h5.oracle.json");

/// Datasets in each fixture. A regenerated fixture that lost one fails here
/// rather than quietly testing less.
const SZIP_DATASETS: usize = 12;
const HAND_DATASETS: usize = 3;

/// `MAX_DECOMPRESSED_CHUNK` in `hdf5/filter.rs`.
const MAX_DECOMPRESSED_CHUNK: u32 = 256 << 20;

fn decode_all(bytes: &[u8]) -> BTreeMap<String, Result<Vec<Option<f64>>, FieldglassError>> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
    let probe = match &reader.backing {
        NetcdfBacking::Hdf5(p) => p.clone(),
        other => panic!("expected HDF5 backing, got {}", other.label()),
    };
    list_root_children(bytes, &probe)
        .expect("list children")
        .into_iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .enumerate()
        .map(|(index, c)| (c.name, reader.decode_variable_raw(index)))
        .collect()
}

fn objects(oracle: &str) -> serde_json::Map<String, Value> {
    let v: Value = serde_json::from_str(oracle).expect("oracle is JSON");
    v["objects"].as_object().expect("objects").clone()
}

/// Every value, compared one by one with the h5py read-back.
fn assert_matches_oracle(bytes: &[u8], oracle: &str, count: usize) {
    let expected = objects(oracle);
    let decoded = decode_all(bytes);
    assert_eq!(expected.len(), count, "oracle dataset count changed");
    assert_eq!(decoded.len(), count, "fixture dataset count changed");
    for (name, want) in &expected {
        let got = decoded
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing from the fixture"))
            .as_ref()
            .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
        let want: Vec<Option<f64>> = want["values"]
            .as_array()
            .expect("values")
            .iter()
            .map(|v| Some(v.as_f64().expect("numeric value")))
            .collect();
        assert_eq!(got.len(), want.len(), "{name}: value count");
        if let Some(i) = (0..want.len()).find(|&i| got[i] != want[i]) {
            panic!(
                "{name}: value {i} is {:?}, h5py reads {:?}",
                got[i], want[i]
            );
        }
    }
}

#[test]
fn every_libhdf5_written_szip_dataset_matches_h5py() {
    assert_matches_oracle(SZIP, SZIP_ORACLE, SZIP_DATASETS);
}

#[test]
fn every_hand_built_szip_dataset_matches_h5py() {
    assert_matches_oracle(HAND, HAND_ORACLE, HAND_DATASETS);
}

/// The fixtures cover the matrix the issue asks for. Read from the oracle,
/// which records what the file really carries, so a regenerated fixture that
/// lost a case fails here.
#[test]
fn the_fixtures_cover_the_szip_parameter_matrix() {
    let all: Vec<(String, Value)> = objects(SZIP_ORACLE)
        .into_iter()
        .chain(objects(HAND_ORACLE))
        .collect();
    let has = |what: &str, f: &dyn Fn(&Value) -> bool| {
        assert!(
            all.iter().any(|(_, o)| f(o)),
            "no fixture dataset has {what}"
        );
    };
    let sz = |o: &Value, k: &str| o["szip"][k].as_u64().unwrap();
    let dtype = |o: &Value| o["dtype"].as_str().unwrap().to_string();
    let ids = |o: &Value| -> Vec<u64> {
        o["pipeline"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["id"].as_u64().unwrap())
            .collect()
    };

    for (d, ppb) in [
        ("<i2", 16),
        ("<i2", 10),
        (">i4", 32),
        ("<f4", 18),
        ("<f8", 8),
        ("<f8", 18),
        ("|u1", 8),
    ] {
        has(&format!("{d} at {ppb} pixels per block"), &|o| {
            dtype(o) == d && sz(o, "pixels_per_block") == ppb
        });
    }
    // MSB: bit 16 of the option mask, which libhdf5 sets for big-endian data.
    has("the MSB option", &|o| sz(o, "options_mask") & 16 != 0);
    has("a pixel narrower than the element (D2)", &|o| {
        sz(o, "bits_per_pixel") == 16 && dtype(o) == "<i4"
    });
    has("shuffle before szip", &|o| ids(o) == [2, 4]);
    has("deflate before szip", &|o| ids(o) == [1, 4]);
    has("padded scanlines", &|o| {
        o["szip"]["padded"].as_bool().unwrap()
    });
    has("fewer pixels per scanline than per block (RSI 1)", &|o| {
        sz(o, "pixels_per_scanline") < sz(o, "pixels_per_block") && sz(o, "rsi") == 1
    });
    has("a chunk ending part-way through a scanline", &|o| {
        let elems: u64 = o["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_u64().unwrap())
            .product();
        !elems.is_multiple_of(sz(o, "pixels_per_scanline"))
    });
    has(
        "a chunk stored with its szip bit set in the filter mask",
        &|o| {
            o["chunk_filter_masks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m.as_u64() == Some(1))
        },
    );
}

/// Rewrite the size prefix of `name`'s first chunk in place.
fn with_prefix(bytes: &[u8], oracle: &str, name: &str, prefix: u32) -> Vec<u8> {
    let o = &objects(oracle)[name];
    assert_eq!(o["chunk_filter_masks"][0], 0, "{name}: szip must have run");
    let at = usize::try_from(o["chunk_byte_offsets"][0].as_u64().unwrap()).unwrap();
    let mut out = bytes.to_vec();
    out[at..at + 4].copy_from_slice(&prefix.to_le_bytes());
    out
}

fn parse_error(bytes: &[u8], name: &str) -> String {
    match decode_all(bytes).remove(name).expect("present") {
        Err(FieldglassError::Parse(m)) => m,
        other => panic!("{name}: expected a Parse error, got {other:?}"),
    }
}

/// The size prefix of a real chunk, with the rest of the chunk untouched:
/// one byte either side of the chunk's length is refused, and so is the
/// ceiling plus one, before anything is allocated for it. `i2_ppb16`'s
/// chunks are 8 × 32 two-byte values.
#[test]
fn a_chunk_whose_size_prefix_is_wrong_is_refused() {
    let chunk = 8 * 32 * 2;
    let pristine = with_prefix(SZIP, SZIP_ORACLE, "i2_ppb16", chunk);
    assert_eq!(
        pristine, SZIP,
        "the recorded offset must point at the prefix"
    );

    for prefix in [chunk - 1, chunk + 1] {
        let m = parse_error(
            &with_prefix(SZIP, SZIP_ORACLE, "i2_ppb16", prefix),
            "i2_ppb16",
        );
        assert!(
            m.contains(&format!(
                "declares {prefix} bytes, but the chunk is {chunk}"
            )),
            "{m}"
        );
    }
    let m = parse_error(
        &with_prefix(SZIP, SZIP_ORACLE, "i2_ppb16", MAX_DECOMPRESSED_CHUNK + 1),
        "i2_ppb16",
    );
    assert!(m.contains("ceiling"), "{m}");
}

/// Behind deflate the prefix is deflate's output length, which the reader
/// cannot know exactly, so it is bounded by the chunk's length plus an eighth
/// plus 4 KiB (4,168 bytes for this 64-byte chunk) rather than required to
/// equal anything. A wrong prefix inside that is then caught, if at all, by
/// the filters after it and by the chunk-length check.
#[test]
fn behind_deflate_the_prefix_is_bounded_by_the_chunk() {
    let o = &objects(HAND_ORACLE)["deflate_szip"];
    let at = usize::try_from(o["chunk_byte_offsets"][0].as_u64().unwrap()).unwrap();
    let prefix = u32::from_le_bytes(HAND[at..at + 4].try_into().unwrap());
    assert_ne!(
        prefix, 64,
        "the prefix must be deflate's length, not the chunk's"
    );

    // One byte short cuts the zlib stream, and inflate says so.
    let m = parse_error(
        &with_prefix(HAND, HAND_ORACLE, "deflate_szip", prefix - 1),
        "deflate_szip",
    );
    assert!(m.contains("deflate"), "{m}");
    // One byte long decodes one more pixel from the stream's fill, which
    // inflate ignores as bytes past the end of its stream, as zlib does
    // under libhdf5. The values are still right.
    let long = decode_all(&with_prefix(HAND, HAND_ORACLE, "deflate_szip", prefix + 1));
    assert_eq!(
        long["deflate_szip"].as_ref().unwrap(),
        decode_all(HAND)["deflate_szip"].as_ref().unwrap()
    );
    // Past the bound, and far past it, is refused before allocating.
    for over in [64 + 8 + 4096 + 1, MAX_DECOMPRESSED_CHUNK + 1] {
        let m = parse_error(
            &with_prefix(HAND, HAND_ORACLE, "deflate_szip", over),
            "deflate_szip",
        );
        assert!(m.contains("past the 4168-byte ceiling"), "{m}");
    }
}

/// A 64-bit chunk whose size prefix is right, with no filter before szip, but
/// whose stream codes twice the pixels (#794). Every length rule the reader
/// applies passes; byte planes laid out by the shorter length would put the
/// bytes in the wrong places, and libhdf5 returns exactly that: the oracle
/// holds its read-back, which differs from what the chunk should hold. The
/// reader refuses the chunk instead.
#[test]
fn a_chunk_whose_stream_codes_more_pixels_is_refused() {
    let v: Value = serde_json::from_str(LONG_ORACLE).unwrap();
    let o = &v["objects"]["f8_long_stream"];
    assert_eq!(o["pipeline"].as_array().unwrap().len(), 1, "szip alone");
    assert_eq!(o["szip"]["bits_per_pixel"], 64);
    assert_ne!(
        o["values"], v["source_values"],
        "libhdf5's read-back must be the scrambled one, or this proves nothing"
    );
    assert!(v["wrong_bytes"].as_u64().unwrap() > 0);

    let m = parse_error(LONG, "f8_long_stream");
    assert!(m.contains("shorter than its stream"), "{m}");
}

/// Filter id 4 is decoded, so the "unsupported filter" error no longer names
/// it and no szip dataset reports it.
#[test]
fn szip_is_no_longer_an_unsupported_filter() {
    for (name, r) in decode_all(SZIP).into_iter().chain(decode_all(HAND)) {
        if let Err(e) = r {
            panic!("{name}: {e}");
        }
    }
}
