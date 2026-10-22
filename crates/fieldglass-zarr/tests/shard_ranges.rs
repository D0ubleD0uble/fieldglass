//! A sharded array read by range (#1123): the shard indexes first, then the
//! inner chunks a region touches, and nothing else of any shard.
//!
//! Over `windows/window_sharded_v3`, written by zarr-python, whose oracle
//! (`windows_oracle.json`) states each window's shards, their index ranges and
//! the byte range of every inner chunk the window touches, read from the shard
//! files with Python's `struct` rather than through this crate.
#![cfg(feature = "codecs")]

use std::ops::Range;
use std::path::Path;

use fieldglass_core::array::ArraySource;
use fieldglass_core::bytes::{ByteRange, MemoryObjects, PrefetchedObjects};
use fieldglass_core::testing::Recording;
use fieldglass_zarr::{IndexLocation, ZarrStore};
use serde_json::Value;

const STORE: &str = "tests/fixtures/windows/window_sharded_v3";

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
    let text = std::fs::read_to_string("tests/fixtures/windows_oracle.json").expect("oracle");
    let all: Value = serde_json::from_str(&text).expect("JSON");
    all["window_sharded_v3"].clone()
}

fn u64s(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .expect("a list")
        .iter()
        .map(|v| v.as_u64().expect("a count"))
        .collect()
}

/// The regions a window reads, one per column run, in the variable's axis
/// order (time, latitude, longitude).
fn regions(window: &Value) -> Vec<Vec<Range<u64>>> {
    let step = window["time"].as_u64().expect("time");
    let rows = u64s(&window["rows"]);
    window["columns"]
        .as_array()
        .expect("runs")
        .iter()
        .map(|run| {
            let run = u64s(run);
            vec![step..step + 1, rows[0]..rows[1], run[0]..run[1]]
        })
        .collect()
}

fn is_data(key: &str, variable: &str) -> bool {
    key.starts_with(&format!("{variable}/c/"))
}

/// Everything but the variable's shards: the metadata and the coordinates.
fn without_shards(all: &[(String, Vec<u8>)], variable: &str) -> PrefetchedObjects {
    let mut objects = PrefetchedObjects::new();
    for (key, bytes) in all {
        if !is_data(key, variable) {
            objects.insert(key.clone(), bytes.clone());
        }
    }
    objects
}

fn bytes_of<'a>(all: &'a [(String, Vec<u8>)], key: &str) -> &'a [u8] {
    &all.iter().find(|(k, _)| k == key).expect("a shard").1
}

/// Each window plans its shards' indexes, then exactly the inner chunks it
/// intersects as byte ranges, and reads from those ranges alone the values a
/// read of the whole shards reads.
#[test]
fn a_window_plans_and_reads_exactly_the_inner_chunks_it_intersects() {
    let all = files();
    let expected = oracle();
    let whole = ZarrStore::open(MemoryObjects::from_iter(all.clone())).expect("opens");
    let index_len = expected["index_length"].as_u64().expect("length");
    for variable in ["t2m", "t2m_start"] {
        let location = if variable == "t2m" {
            IndexLocation::End
        } else {
            IndexLocation::Start
        };
        for window in expected["windows"][variable].as_array().expect("windows") {
            let name = format!("{variable}/{}", window["name"]);
            let regions = regions(window);

            // Step 1: the index of every shard the window touches.
            let mut objects = without_shards(&all, variable);
            let store = ZarrStore::open(&objects).expect("opens on the metadata");
            let mut indexes = Vec::new();
            for region in &regions {
                let planned = store
                    .shard_indexes(variable, region)
                    .expect("plans")
                    .expect("sharded");
                assert!(
                    planned
                        .iter()
                        .all(|i| i.location == location && i.len == index_len)
                );
                // Not yet: no index is in hand.
                assert_eq!(store.inner_chunk_ranges(variable, region).unwrap(), None);
                indexes.extend(planned);
            }
            indexes.sort_by(|a, b| a.index.cmp(&b.index));
            indexes.dedup_by(|a, b| a.index == b.index);
            let keys: Vec<&str> = indexes.iter().map(|i| i.key.as_str()).collect();
            let want: Vec<&str> = window["chunks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["key"].as_str().unwrap())
                .collect();
            assert_eq!(keys, want, "{name}: shards");

            // The host fetches each index as a range, and learns the size.
            for index in &indexes {
                let shard = bytes_of(&all, &index.key);
                let size = shard.len() as u64;
                assert_eq!(
                    window["shard_sizes"][&index.key].as_u64(),
                    Some(size),
                    "{name}"
                );
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

            // Step 2: the inner chunks, as byte ranges.
            let store = ZarrStore::open(&objects).expect("opens");
            let mut inner = Vec::new();
            for region in &regions {
                inner.extend(
                    store
                        .inner_chunk_ranges(variable, region)
                        .expect("plans")
                        .expect("every index is in hand"),
                );
            }
            inner.sort_by(|a, b| (&a.index, &a.inner).cmp(&(&b.index, &b.inner)));
            inner.dedup();
            let got: Vec<(String, Vec<u64>, u64, u64)> = inner
                .iter()
                .map(|c| (c.key.clone(), c.inner.clone(), c.range.start, c.range.len))
                .collect();
            let want: Vec<(String, Vec<u64>, u64, u64)> = window["inner_chunks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    (
                        c["key"].as_str().unwrap().to_string(),
                        u64s(&c["inner"]),
                        c["offset"].as_u64().unwrap(),
                        c["length"].as_u64().unwrap(),
                    )
                })
                .collect();
            assert_eq!(got, want, "{name}: inner chunks");

            // Before the inner chunks are fetched, a read names each one.
            for region in &regions {
                let owed = store.unfetched_for(variable, region).unwrap();
                let err = store.read_region(variable, region);
                if owed.is_empty() {
                    // A region whose touched inner chunks are all absent.
                    assert!(err.is_ok(), "{name}");
                    continue;
                }
                let message = err.expect_err("owes the inner chunks").to_string();
                for item in &owed {
                    assert!(message.contains(item), "{name}: {message} lacks {item}");
                }
            }

            // Fetch them, and read: only those ranges, never a whole shard.
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
            for region in &regions {
                assert!(store.unfetched_for(variable, region).unwrap().is_empty());
                recorded.clear();
                let got = store.read_region(variable, region).expect("reads");
                // Debug-formatted, so a NaN fill equals a NaN fill.
                assert_eq!(
                    format!("{got:?}"),
                    format!("{:?}", whole.read_region(variable, region).unwrap()),
                    "{name}: {region:?}"
                );
                assert!(
                    recorded.gets().iter().all(|k| !is_data(k, variable)),
                    "{name}: a shard read whole: {:?}",
                    recorded.gets()
                );
                let index_ranges: Vec<(String, ByteRange)> = indexes
                    .iter()
                    .map(|i| {
                        let size = bytes_of(&all, &i.key).len() as u64;
                        let start = match i.location {
                            IndexLocation::End => size - i.len,
                            IndexLocation::Start => 0,
                        };
                        (i.key.clone(), ByteRange::new(start, i.len))
                    })
                    .collect();
                for read in recorded.range_gets() {
                    assert!(
                        index_ranges.contains(&read)
                            || inner.iter().any(|c| c.key == read.0 && c.range == read.1),
                        "{name}: read {read:?}, which was not planned"
                    );
                }
            }
        }
    }
}

/// A shard the store does not hold (a 404) is the fill value, and owes
/// nothing; one given whole reads as it always has, beside ones given by
/// range.
#[test]
fn an_absent_shard_is_fill_and_a_whole_one_reads_whole() {
    let all = files();
    let whole = ZarrStore::open(MemoryObjects::from_iter(all.clone())).expect("opens");
    let region = [0..1, 0..8, 0..12];
    let mut objects = without_shards(&all, "t2m");
    objects.insert_absent("t2m/c/0/0/0");
    objects.insert("t2m/c/0/0/1", bytes_of(&all, "t2m/c/0/0/1").to_vec());
    // The other two by range: their indexes, then what the region touches.
    for key in ["t2m/c/0/1/0", "t2m/c/0/1/1"] {
        let shard = bytes_of(&all, key);
        let size = shard.len() as u64;
        objects
            .insert_range(key, size, size - 100, shard[shard.len() - 100..].to_vec())
            .unwrap();
    }
    let store = ZarrStore::open(&objects).expect("opens");
    for c in store
        .inner_chunk_ranges("t2m", &region)
        .unwrap()
        .expect("every index is in hand")
    {
        assert_ne!(c.key, "t2m/c/0/0/0", "an absent shard has no inner chunks");
        let shard = bytes_of(&all, &c.key);
        let at = usize::try_from(c.range.start).unwrap();
        let bytes = shard[at..at + usize::try_from(c.range.len).unwrap()].to_vec();
        if c.key != "t2m/c/0/0/1" {
            objects
                .insert_range(c.key, shard.len() as u64, c.range.start, bytes)
                .unwrap();
        }
    }
    let store = ZarrStore::open(&objects).expect("opens");
    let got = store.read_region("t2m", &region).expect("reads");
    let full = whole.read_region("t2m", &region).unwrap();
    for (row, (g, f)) in got.chunks(12).zip(full.chunks(12)).enumerate() {
        for (col, (g, f)) in g.iter().zip(f).enumerate() {
            if row < 6 && col < 8 {
                // Fill: NaN, which the raw read gives as the fill value.
                assert!(g.is_some_and(f64::is_nan), "({row}, {col}) {g:?}");
            } else {
                assert_eq!(format!("{g:?}"), format!("{f:?}"), "({row}, {col})");
            }
        }
    }
}

/// A shard whose index the host fetched but whose bytes it altered on the way
/// is refused when planned and when read, never sliced at the offsets it
/// states.
#[test]
fn a_tampered_index_is_refused_by_plan_and_read() {
    let all = files();
    let key = "t2m/c/0/0/0";
    let shard = bytes_of(&all, key);
    let size = shard.len() as u64;
    let mut raw = shard[shard.len() - 100..].to_vec();
    // The first entry's offset, pointed past the shard; the checksum then
    // fails before the offset is believed.
    raw[7] = 0x7f;
    let mut objects = without_shards(&all, "t2m");
    objects.insert_range(key, size, size - 100, raw).unwrap();
    let store = ZarrStore::open(&objects).expect("opens");
    let region = [0..1, 0..2, 0..4];
    assert!(store.inner_chunk_ranges("t2m", &region).is_err());
    assert!(store.read_region("t2m", &region).is_err());
}
