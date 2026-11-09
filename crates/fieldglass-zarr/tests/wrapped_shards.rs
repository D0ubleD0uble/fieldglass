//! Shards behind bytes→bytes codecs (#1303): `[sharding_indexed, gzip]`.
//!
//! `sharding_indexed` is an array→bytes codec, so bytes→bytes codecs may
//! follow it, and each runs over the whole shard, its inner chunks and its
//! index together. The stored object is then a compressed shard: a read
//! undoes the codecs after the sharding codec, and reads the shard it gets
//! back as any other. Its index cannot be fetched by range, so a host fetches
//! such a shard whole.
//!
//! Over `wrapped_shards`, written by zarr-python, whose oracle
//! (`wrapped_shards_oracle.json`) states the values zarr-python reads back and
//! each shard's length once its codecs are undone by numcodecs and the
//! standard library rather than through this crate.
#![cfg(feature = "codecs")]

use std::ops::Range;
use std::path::Path;

use fieldglass_core::array::ArraySource;
use fieldglass_core::bytes::{MemoryObjects, PrefetchedObjects};
use fieldglass_zarr::{ChunkDecoder, FieldglassError, ZarrStore};
use serde_json::Value;

const STORE: &str = "tests/fixtures/wrapped_shards";

fn files() -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).expect("fixture directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let key = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((key, std::fs::read(&path).expect("read")));
            }
        }
    }
    let mut out = Vec::new();
    walk(Path::new(STORE), Path::new(STORE), &mut out);
    out
}

fn oracle() -> Value {
    let text =
        std::fs::read_to_string("tests/fixtures/wrapped_shards_oracle.json").expect("oracle");
    serde_json::from_str(&text).expect("JSON")
}

fn values(list: &Value) -> Vec<Option<f64>> {
    list.as_array()
        .expect("values")
        .iter()
        .map(|v| Some(v.as_f64().expect("a number")))
        .collect()
}

fn region(value: &Value) -> Vec<Range<u64>> {
    value
        .as_array()
        .expect("a region")
        .iter()
        .map(|axis| {
            let axis = axis.as_array().expect("an axis");
            axis[0].as_u64().expect("start")..axis[1].as_u64().expect("stop")
        })
        .collect()
}

fn whole(case: &Value) -> Vec<Range<u64>> {
    case["shape"]
        .as_array()
        .expect("a shape")
        .iter()
        .map(|n| 0..n.as_u64().expect("an extent"))
        .collect()
}

fn arrays(oracle: &Value) -> Vec<(String, Value)> {
    let names: Vec<String> = oracle
        .as_object()
        .expect("arrays")
        .keys()
        .cloned()
        .collect();
    // Every case the builder writes, so a case dropped there is noticed here.
    assert_eq!(
        names,
        [
            "crc32c_zstd",
            "gzip",
            "index_start",
            "inner_gzip",
            "plain_index",
            "transposed"
        ]
    );
    names
        .into_iter()
        .map(|name| {
            let case = oracle[&name].clone();
            (name, case)
        })
        .collect()
}

fn bytes_of<'a>(all: &'a [(String, Vec<u8>)], key: &str) -> &'a [u8] {
    &all.iter().find(|(k, _)| k == key).expect("a shard").1
}

fn decoder(all: &[(String, Vec<u8>)], name: &str) -> ChunkDecoder {
    let meta = std::str::from_utf8(bytes_of(all, &format!("{name}/zarr.json"))).expect("UTF-8");
    ChunkDecoder::from_v3_metadata(meta).expect("the metadata reads")
}

/// The issue's case and its kin read the values zarr-python reads, whole and
/// by region. Before #1303 the array took the plain-chunk path, its trailing
/// codec was held to the shard's element bytes, and every shard was reported
/// as corrupt.
#[test]
fn a_shard_behind_bytes_codecs_reads_what_zarr_python_reads() {
    let oracle = oracle();
    let store = ZarrStore::open(MemoryObjects::from_iter(files())).expect("opens");
    for (name, case) in arrays(&oracle) {
        assert_eq!(
            store.read_region(&name, &whole(&case)).expect("reads"),
            values(&case["values"]),
            "{name}"
        );
        for read in case["regions"].as_array().expect("regions") {
            let region = region(&read["region"]);
            assert_eq!(
                store.read_region(&name, &region).expect("reads"),
                values(&read["values"]),
                "{name} {region:?}"
            );
        }
    }
}

/// The decoder alone: such an array is sharded, and `decode_shard` undoes the
/// codecs after the sharding codec before it reads the index. The absent inner
/// chunks the oracle counts in each shard come back absent.
#[test]
fn decode_shard_undoes_the_codecs_after_the_sharding_codec() {
    let oracle = oracle();
    let all = files();
    for (name, case) in arrays(&oracle) {
        let decoder = decoder(&all, &name);
        assert!(decoder.is_sharded(), "{name}");
        // Not a shard a read can index by range: its index is compressed.
        assert!(decoder.chain().sharding().is_none(), "{name}");
        for shard in case["shards"].as_array().expect("shards") {
            let key = shard["key"].as_str().expect("a key");
            let decoded = decoder
                .decode_shard(bytes_of(&all, key))
                .unwrap_or_else(|e| panic!("{key}: {e}"));
            let absent = (0..decoded.len())
                .filter(|&i| decoded.chunk(i).is_none())
                .count();
            assert_eq!(
                absent as u64,
                shard["absent_inner_chunks"].as_u64().expect("a count"),
                "{key}"
            );
        }
        // And a plain-chunk decode says what the chunks are, rather than that
        // they are corrupt.
        let first = case["shards"][0]["key"].as_str().expect("a key");
        let err = decoder.decode(bytes_of(&all, first)).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::WrongLayout(m) if m.contains("decode_shard")),
            "{name}: {err:?}"
        );
    }
}

/// The shard behind the codecs can be longer than its elements and its index
/// together, because its inner chunks went through codecs of their own; the
/// bound a decode holds the trailing codecs to allows for that. `inner_gzip`
/// and `crc32c_zstd` are both such shards; a bound of the elements and the
/// index alone would refuse them.
#[test]
fn a_shard_longer_than_its_elements_and_index_is_read() {
    let oracle = oracle();
    for name in ["crc32c_zstd", "inner_gzip"] {
        let case = &oracle[name];
        let floor =
            case["element_bytes"].as_u64().unwrap() + case["index_length"].as_u64().unwrap();
        for shard in case["shards"].as_array().expect("shards") {
            assert!(
                shard["shard_length"].as_u64().unwrap() > floor,
                "{name}: the fixture no longer pins the inner codecs' growth"
            );
        }
    }
    // The values themselves are read in the first test; this one keeps the
    // fixture honest about what it pins.
}

/// The bound counts the index wherever it sits, with its checksum when it has
/// one. `index_start` (index first, with `crc32c`) and `plain_index` (index
/// last, without) hold every inner chunk, so each shard is exactly its
/// elements and its index: a bound short of either by a byte refuses it.
/// Neither has inner codecs to widen the bound, as `crc32c_zstd` does.
#[test]
fn a_dense_shard_meets_its_bound_exactly() {
    let oracle = oracle();
    let all = files();
    for (name, index) in [("index_start", 68), ("plain_index", 64)] {
        let case = &oracle[name];
        assert_eq!(case["index_length"], index, "{name}");
        let exact =
            case["element_bytes"].as_u64().unwrap() + case["index_length"].as_u64().unwrap();
        let decoder = decoder(&all, name);
        for shard in case["shards"].as_array().expect("shards") {
            assert_eq!(shard["shard_length"].as_u64(), Some(exact), "{name}");
            assert_eq!(shard["absent_inner_chunks"], 0, "{name}");
            let key = shard["key"].as_str().expect("a key");
            let decoded = decoder
                .decode_shard(bytes_of(&all, key))
                .unwrap_or_else(|e| panic!("{key}: {e}"));
            assert!(
                (0..decoded.len()).all(|i| decoded.chunk(i).is_some()),
                "{key}"
            );
        }
    }
}

/// A shard bounded by what its sharding codec could have written: a gzip
/// member over the right shard plus padding past that bound is refused rather
/// than inflated without limit.
#[test]
fn a_shard_past_its_bound_is_refused() {
    let all = files();
    let decoder = decoder(&all, "gzip");
    // A valid gzip member of a megabyte of zeros, where the shard can be at
    // most a few hundred bytes.
    let bomb = gzip_of_zeros(1 << 20);
    let err = decoder.decode_shard(&bomb).unwrap_err();
    assert!(
        matches!(&err, FieldglassError::Parse(m) if m.contains("ceiling")),
        "{err:?}"
    );
}

/// A gzip member holding `len` zero bytes in stored deflate blocks, built by
/// hand so the test needs no compressor.
fn gzip_of_zeros(len: usize) -> Vec<u8> {
    let mut out = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0xFF];
    let mut left = len;
    while left > 0 {
        let n = left.min(0xFFFF);
        left -= n;
        out.push(u8::from(left == 0));
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&(!(n as u16)).to_le_bytes());
        out.extend(std::iter::repeat_n(0u8, n));
    }
    // CRC-32 of `len` zeros, then the length.
    let mut crc = !0u32;
    for _ in 0..len {
        crc ^= 0;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    out.extend_from_slice(&(!crc).to_le_bytes());
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out
}

/// The ranged path is not offered for such a shard: its index is inside the
/// compressed object, not at its tail in raw bytes. `shard_indexes` plans no
/// index, a read given the shard whole reads it, and a source that holds it
/// only by range is refused plainly rather than read as raw bytes.
#[test]
fn a_wrapped_shard_is_never_read_by_range() {
    let all = files();
    let store = ZarrStore::open(MemoryObjects::from_iter(all.clone())).expect("opens");
    let region = [0..6u64, 0..8];
    assert_eq!(store.shard_indexes("gzip", &region).expect("plans"), None);
    assert_eq!(
        store.inner_chunk_ranges("gzip", &region).expect("plans"),
        None
    );

    // The metadata whole, and the shards only as ranges of themselves: their
    // last 68 bytes, where an uncompressed shard's index would be.
    let mut held = PrefetchedObjects::new();
    for (key, bytes) in &all {
        if key.starts_with("gzip/c/") {
            let size = bytes.len() as u64;
            let at = size - 68;
            held.insert_range(key, size, at, bytes[at as usize..].to_vec())
                .expect("a range of the shard");
        } else {
            held.insert(key, bytes.clone());
        }
    }
    let store = ZarrStore::open(held).expect("opens");
    // What the read owes is the shards whole, not their index ranges.
    let owed = store.unfetched_for("gzip", &region).expect("plans");
    assert_eq!(owed, ["\"gzip/c/0/0\"", "\"gzip/c/1/0\""]);
    let err = store.read_region("gzip", &region).unwrap_err();
    assert!(
        matches!(&err, FieldglassError::Parse(m) if m.contains("\"gzip/c/0/0\"") && m.contains("never given")),
        "{err:?}"
    );
}

/// Only bytes→bytes codecs may follow `sharding_indexed`: the specification
/// orders a chain as array→array codecs, one array→bytes codec, then
/// bytes→bytes codecs. Anything else there is refused when the metadata is
/// read, not at the first shard.
#[test]
fn a_codec_after_the_sharding_codec_that_is_not_bytes_to_bytes_is_refused() {
    let sharding = r#"{"name": "sharding_indexed", "configuration": {"chunk_shape": [2, 2],
        "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
        "index_codecs": [{"name": "bytes", "configuration": {"endian": "little"}},
                         {"name": "crc32c"}]}}"#;
    let meta = |after: &str| {
        format!(
            r#"{{"zarr_format": 3, "node_type": "array", "shape": [4, 4],
            "data_type": "float32", "fill_value": 0,
            "chunk_grid": {{"name": "regular", "configuration": {{"chunk_shape": [4, 4]}}}},
            "chunk_key_encoding": {{"name": "default"}},
            "codecs": [{sharding}, {after}]}}"#
        )
    };
    for (after, name) in [
        (
            r#"{"name": "bytes", "configuration": {"endian": "little"}}"#,
            "`bytes`",
        ),
        (
            r#"{"name": "transpose", "configuration": {"order": [1, 0]}}"#,
            "`transpose`",
        ),
        (sharding, "`sharding_indexed`"),
    ] {
        let err = ChunkDecoder::from_v3_metadata(&meta(after)).unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains(name) && m.contains("after `sharding_indexed`")),
            "{name}: {err:?}"
        );
    }
    // The bytes→bytes codecs read.
    for after in [
        r#"{"name": "gzip", "configuration": {"level": 5}}"#,
        r#"{"name": "crc32c"}"#,
    ] {
        let decoder = ChunkDecoder::from_v3_metadata(&meta(after)).expect("reads");
        assert!(decoder.is_sharded());
    }
    // A shard nested in a shard is refused by name, also when its inner
    // chain wraps it in a `gzip`.
    let nested = sharding.replace(
        r#""codecs": [{"name": "bytes", "configuration": {"endian": "little"}}]"#,
        &format!(r#""codecs": [{sharding}, {{"name": "gzip"}}]"#),
    );
    assert_ne!(nested, sharding);
    let only = format!(
        r#"{{"zarr_format": 3, "node_type": "array", "shape": [4, 4],
        "data_type": "float32", "fill_value": 0,
        "chunk_grid": {{"name": "regular", "configuration": {{"chunk_shape": [4, 4]}}}},
        "chunk_key_encoding": {{"name": "default"}},
        "codecs": [{nested}]}}"#
    );
    let err = ChunkDecoder::from_v3_metadata(&only).unwrap_err();
    assert!(
        matches!(&err, FieldglassError::UnsupportedSection(m) if m.contains("nested")),
        "{err:?}"
    );
}
