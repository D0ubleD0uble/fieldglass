//! Shards behind a `transpose` (#1343): `[transpose, sharding_indexed]`.
//!
//! `sharding_indexed` is an array→bytes codec, so array→array codecs may come
//! before it, and the sharding codec is then handed each shard in the
//! transposed axis order. Its inner grid, its index and every inner chunk are
//! laid out in that order, and a read has to put the values back in the
//! array's own, on a shard read whole and on one read by range.
//!
//! Over `transposed_shards`, written by zarr-python, whose oracle
//! (`transposed_shards_oracle.json`) states the values zarr-python reads back
//! and, for each region, the byte range of every inner chunk it touches, read
//! from the shard files with Python's `struct` rather than through this crate.

use std::ops::Range;
use std::path::Path;

use fieldglass_core::array::ArraySource;
use fieldglass_core::bytes::{MemoryObjects, PrefetchedObjects};
use fieldglass_core::testing::Recording;
use fieldglass_zarr::{ChunkDecoder, FieldglassError, IndexLocation, ZarrStore};
use serde_json::Value;

const STORE: &str = "tests/fixtures/transposed_shards";

/// One planned inner chunk: its shard's key and grid position, its place on
/// the shard's grid, and its byte offset and length.
type Planned = (String, Vec<u64>, Vec<u64>, u64, u64);

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
        std::fs::read_to_string("tests/fixtures/transposed_shards_oracle.json").expect("oracle");
    serde_json::from_str(&text).expect("JSON")
}

fn u64s(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .expect("a list")
        .iter()
        .map(|v| v.as_u64().expect("a count"))
        .collect()
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
            let axis = u64s(axis);
            axis[0]..axis[1]
        })
        .collect()
}

fn whole(oracle: &Value) -> Vec<Range<u64>> {
    u64s(&oracle["shape"]).into_iter().map(|n| 0..n).collect()
}

fn arrays(oracle: &Value) -> Vec<(String, Value)> {
    let names: Vec<String> = oracle
        .as_object()
        .expect("arrays")
        .keys()
        .cloned()
        .collect();
    // Every case the builder writes, so a case dropped there is noticed here.
    assert_eq!(names, ["cube", "square", "twice", "wide"]);
    names
        .into_iter()
        .map(|name| {
            let case = oracle[&name].clone();
            (name, case)
        })
        .collect()
}

fn is_data(key: &str) -> bool {
    key.contains("/c/")
}

fn bytes_of<'a>(all: &'a [(String, Vec<u8>)], key: &str) -> &'a [u8] {
    &all.iter().find(|(k, _)| k == key).expect("a shard").1
}

/// A shard read whole decodes to the values zarr-python reads: the whole
/// array and each region. Before #1343 the transpose was dropped and every
/// array read back transposed (or, where the inner grid did not divide the
/// untransposed shard, was refused).
#[test]
fn a_transposed_shard_read_whole_reads_what_zarr_python_reads() {
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

/// The issue's case through the decoder alone: `decode_shard` hands back the
/// inner grid and each inner chunk in the array's own axis order, so the
/// inner chunk at grid position (0, 1) is rows 0-1, columns 2-3.
#[test]
fn decode_shard_puts_a_transposed_shard_back_in_the_arrays_order() {
    let all = files();
    let meta = std::str::from_utf8(bytes_of(&all, "square/zarr.json")).unwrap();
    let decoder = ChunkDecoder::from_v3_metadata(meta).expect("reads");
    let shard = decoder
        .decode_shard(bytes_of(&all, "square/c/0/0"))
        .expect("decodes");
    assert_eq!(shard.chunks_per_shard(), [2, 2]);
    let array: Vec<f64> = values(&oracle()["square"]["values"])
        .into_iter()
        .map(Option::unwrap)
        .collect();
    for position in 0..4 {
        let (row, col) = (position / 2 * 2, position % 2 * 2);
        let want: Vec<f64> = (row..row + 2)
            .flat_map(|r| (col..col + 2).map(move |c| (r, c)))
            .map(|(r, c)| array[r * 4 + c])
            .collect();
        let got = decoder
            .dtype()
            .read_values(shard.chunk(position).expect("written"))
            .unwrap();
        assert_eq!(got, want, "inner chunk {position}");
    }
}

/// Read by range: each region plans its shards' indexes, then exactly the
/// inner chunks it touches, at the places on the transposed grid the oracle
/// names, and reads from those ranges alone the values zarr-python reads.
#[test]
fn a_transposed_shard_read_by_range_plans_and_reads_its_inner_chunks() {
    let all = files();
    let oracle = oracle();
    for (name, case) in arrays(&oracle) {
        let location = match case["index_location"].as_str() {
            Some("start") => IndexLocation::Start,
            _ => IndexLocation::End,
        };
        let index_len = case["index_length"].as_u64().expect("length");
        for read in case["regions"].as_array().expect("regions") {
            let region = region(&read["region"]);
            let what = format!("{name} {region:?}");
            let mut objects = PrefetchedObjects::new();
            for (key, bytes) in &all {
                if !is_data(key) {
                    objects.insert(key.clone(), bytes.clone());
                }
            }

            // Step 1: the index of every shard the region touches.
            let store = ZarrStore::open(&objects).expect("opens on the metadata");
            let indexes = store
                .shard_indexes(&name, &region)
                .expect("plans")
                .expect("sharded");
            for index in &indexes {
                assert_eq!((index.location, index.len), (location, index_len), "{what}");
                let shard = bytes_of(&all, &index.key);
                let size = shard.len() as u64;
                let start = match index.location {
                    IndexLocation::End => size - index.len,
                    IndexLocation::Start => 0,
                };
                let at = usize::try_from(start).unwrap();
                let raw = shard[at..at + usize::try_from(index.len).unwrap()].to_vec();
                objects
                    .insert_range(index.key.clone(), size, start, raw)
                    .unwrap();
            }

            // Step 2: the inner chunks, where the transposed grid has them.
            let store = ZarrStore::open(&objects).expect("opens");
            let inner = store
                .inner_chunk_ranges(&name, &region)
                .expect("plans")
                .expect("every index is in hand");
            let mut got: Vec<Planned> = inner
                .iter()
                .map(|c| {
                    (
                        c.key.clone(),
                        c.index.clone(),
                        c.inner.clone(),
                        c.range.start,
                        c.range.len,
                    )
                })
                .collect();
            got.sort();
            let want: Vec<Planned> = read["inner_chunks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    (
                        c["key"].as_str().unwrap().to_string(),
                        u64s(&c["index"]),
                        u64s(&c["inner"]),
                        c["offset"].as_u64().unwrap(),
                        c["length"].as_u64().unwrap(),
                    )
                })
                .collect();
            assert_eq!(got, want, "{what}: inner chunks");

            for c in &inner {
                let shard = bytes_of(&all, &c.key);
                let at = usize::try_from(c.range.start).unwrap();
                let bytes = shard[at..at + usize::try_from(c.range.len).unwrap()].to_vec();
                objects
                    .insert_range(c.key.clone(), shard.len() as u64, c.range.start, bytes)
                    .unwrap();
            }
            let recorded = Recording::new(&objects);
            let store = ZarrStore::open(&recorded).expect("opens");
            assert!(store.unfetched_for(&name, &region).unwrap().is_empty());
            recorded.clear();
            assert_eq!(
                store.read_region(&name, &region).expect("reads"),
                values(&read["values"]),
                "{what}"
            );
            assert!(
                recorded.gets().iter().all(|k| !is_data(k)),
                "{what}: a shard read whole: {:?}",
                recorded.gets()
            );
        }
    }
}

/// A `zarr.json` for a 4x4 array in one shard of 2x2 inner chunks, behind
/// `before`.
fn sharded_behind(before: Value) -> String {
    let mut codecs = before.as_array().cloned().unwrap_or_default();
    codecs.push(serde_json::json!({
        "name": "sharding_indexed",
        "configuration": {
            "chunk_shape": [2, 2],
            "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}],
            "index_codecs": [
                {"name": "bytes", "configuration": {"endian": "little"}},
                {"name": "crc32c"}
            ]
        }
    }));
    serde_json::json!({
        "zarr_format": 3,
        "node_type": "array",
        "shape": [4, 4],
        "data_type": "float32",
        "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [4, 4]}},
        "chunk_key_encoding": {"name": "default"},
        "fill_value": 0.0,
        "codecs": codecs
    })
    .to_string()
}

/// Only `transpose` is read before the sharding codec. Anything else there is
/// refused when the metadata is read, by the decoder and by the planner, and
/// so is a transpose that is not a permutation of the shard's axes, rather
/// than a shard being read in the wrong order.
#[test]
fn a_codec_before_sharding_that_is_not_a_transpose_is_refused() {
    let gzip = sharded_behind(serde_json::json!([{"name": "gzip"}]));
    let err = ChunkDecoder::from_v3_metadata(&gzip).unwrap_err();
    assert!(
        matches!(&err, FieldglassError::UnsupportedSection(m) if m.contains("before `sharding_indexed`")),
        "{err:?}"
    );
    let store = ZarrStore::open(MemoryObjects::from_iter([(
        "zarr.json".to_string(),
        gzip.into_bytes(),
    )]))
    .expect("opens");
    let plan = store.shard_indexes("", &[0..4, 0..4]);
    assert!(
        matches!(plan, Err(FieldglassError::UnsupportedSection(_))),
        "{plan:?}"
    );

    for order in [serde_json::json!([0, 0]), serde_json::json!([1, 0, 2])] {
        let bad = sharded_behind(
            serde_json::json!([{"name": "transpose", "configuration": {"order": order}}]),
        );
        assert!(
            matches!(
                ChunkDecoder::from_v3_metadata(&bad),
                Err(FieldglassError::Parse(_))
            ),
            "{order}"
        );
    }

    // A transpose spelled out as the identity is no transpose at all.
    let identity = sharded_behind(
        serde_json::json!([{"name": "transpose", "configuration": {"order": [0, 1]}}]),
    );
    let decoder = ChunkDecoder::from_v3_metadata(&identity).expect("reads");
    assert_eq!(decoder.chain().shard_order(2).unwrap(), None);
}

/// The sharding codec's `chunk_shape` divides the shard it was handed, which
/// behind a transpose is the transposed one. Here a 2x4 `uint8` shard is
/// stored as 4x2 in inner chunks of 4x1, which do not divide 2x4.
///
/// zarr-python 3.3.0 encodes and decodes this layout the same way, but its
/// metadata check holds `chunk_shape` to the untransposed shard and refuses to
/// create such an array, so this shard is built by hand from the spec: the
/// codecs run in order, each handed the previous one's output.
#[test]
fn an_inner_grid_that_divides_only_the_transposed_shard_is_read() {
    let meta = serde_json::json!({
        "zarr_format": 3,
        "node_type": "array",
        "shape": [2, 4],
        "data_type": "uint8",
        "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [2, 4]}},
        "chunk_key_encoding": {"name": "default"},
        "fill_value": 0,
        "codecs": [
            {"name": "transpose", "configuration": {"order": [1, 0]}},
            {"name": "sharding_indexed", "configuration": {
                "chunk_shape": [4, 1],
                "codecs": [{"name": "bytes"}],
                "index_codecs": [
                    {"name": "bytes", "configuration": {"endian": "little"}},
                    {"name": "crc32c"}
                ]
            }}
        ]
    });
    // The array is [[0, 1, 2, 3], [4, 5, 6, 7]]; transposed it is 4x2, whose
    // columns, the two inner chunks, are the array's rows.
    let mut shard: Vec<u8> = (0..8).collect();
    let mut index = Vec::new();
    for (offset, length) in [(0u64, 4u64), (4, 4)] {
        index.extend_from_slice(&offset.to_le_bytes());
        index.extend_from_slice(&length.to_le_bytes());
    }
    index.extend_from_slice(&fieldglass_zarr::crc32c::checksum(&index).to_le_bytes());
    shard.extend_from_slice(&index);
    let store = ZarrStore::open(MemoryObjects::from_iter([
        ("zarr.json".to_string(), meta.to_string().into_bytes()),
        ("c/0/0".to_string(), shard),
    ]))
    .expect("opens");
    let all: Vec<Option<f64>> = (0..8).map(|v| Some(f64::from(v))).collect();
    assert_eq!(store.read_region("", &[0..2, 0..4]).expect("reads"), all);
    // The second row is the second inner chunk, at (0, 1) on the stored grid.
    let planned = store
        .inner_chunk_ranges("", &[1..2, 1..3])
        .expect("plans")
        .expect("held whole");
    assert_eq!(planned.len(), 1);
    assert_eq!(planned[0].inner, [0, 1]);
    assert_eq!(planned[0].range.start, 4);
}
