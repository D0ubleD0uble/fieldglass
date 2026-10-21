//! A lat/lon box and a time index, planned into the chunks of a Zarr store and
//! decoded from those chunks alone (#1083).
//!
//! The flow a browser runs against a remote store, through `Session` over a
//! map of objects that grows as the host fetches: the metadata documents, then
//! the coordinate chunks `coordinate_chunks` names, then the variable's chunks
//! `plan_window` names, and nothing else. The expected rows, columns, chunks
//! and values come from `windows_oracle.json`, which
//! `tools/build_zarr_fixtures.py --windows-only` computes from the coordinate
//! values and reads back through xarray; nothing here is recorded from the
//! code under test.
//!
//! The host's objects are a `PrefetchedObjects`, as the browser's are: a key it
//! fetched is bytes, or absent when the store does not hold it (a 404), and
//! any other key is one it has not fetched (#1124, #1163).
//!
//! Paths are relative for the `wasm32-wasip1` run's sandbox, which preopens the
//! package directory and its parent.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

use fieldglass::{
    ChunkRef, DecodeOptions, Error, FieldglassError, InnerChunkRef, MemoryObjects, ObjectSource,
    PrefetchedObjects, Session, ShardIndexRef, WindowRequest,
};
use fieldglass_core::bytes::ByteRange;
use fieldglass_core::testing::Recording;
use serde_json::Value;

const FIXTURES: &str = "../fieldglass-zarr/tests/fixtures";

/// The metadata documents a store holds, by the names both editions give them.
const METADATA: [&str; 5] = ["zarr.json", ".zmetadata", ".zgroup", ".zarray", ".zattrs"];

/// Every object of a fixture store, by its key from the store's root.
fn objects(store: &str) -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
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
    let root = format!("{FIXTURES}/windows/{store}");
    let root = Path::new(&root);
    let mut out = Vec::new();
    walk(root, root, &mut out);
    assert!(
        !out.is_empty(),
        "{store}: no objects; is the fixture missing?"
    );
    out
}

fn is_metadata(key: &str) -> bool {
    METADATA
        .iter()
        .any(|doc| key == *doc || key.ends_with(&format!("/{doc}")))
}

/// The objects of `store` a host holds after fetching its metadata and these
/// keys: each key's bytes, or absent for one the store does not hold, as a
/// fetch that came back 404 is given.
fn holding(all: &[(String, Vec<u8>)], keys: &BTreeSet<String>) -> PrefetchedObjects {
    let mut held = PrefetchedObjects::new();
    for (key, bytes) in all {
        if is_metadata(key) || keys.contains(key) {
            held.insert(key.clone(), bytes.clone());
        }
    }
    for key in keys {
        if !all.iter().any(|(k, _)| k == key) {
            held.insert_absent(key.clone());
        }
    }
    held
}

/// The window stores, the oracle's keys: north to south and south to north,
/// each in both editions.
const STORES: [&str; 4] = ["window_v2", "window_v3", "window_s2n_v2", "window_s2n_v3"];

/// The recording store, kept reachable after the session has taken it, as in
/// `placement_is_memoised.rs`.
struct Shared(Rc<Recording<PrefetchedObjects>>);

impl ObjectSource for Shared {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        self.0
            .get(key)
            .map(|v| v.map(|v| Cow::Owned(v.into_owned())))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        self.0.list(prefix)
    }

    fn list_children(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        self.0.list_children(prefix)
    }

    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        self.0.prefetch(keys)
    }

    fn unfetched(&self, keys: &[&str]) -> Vec<String> {
        self.0.unfetched(keys)
    }

    fn object_size(&self, key: &str) -> Result<Option<u64>, FieldglassError> {
        self.0.object_size(key)
    }

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        self.0
            .get_range(key, range)
            .map(|v| v.map(|v| Cow::Owned(v.into_owned())))
    }

    fn unfetched_ranges(&self, wanted: &[(&str, ByteRange)]) -> Vec<(String, ByteRange)> {
        self.0.unfetched_ranges(wanted)
    }
}

fn oracle() -> Value {
    let text = std::fs::read_to_string(format!("{FIXTURES}/windows_oracle.json"))
        .expect("windows_oracle.json");
    serde_json::from_str(&text).expect("the oracle is JSON")
}

fn refs(list: &Value) -> Vec<ChunkRef> {
    serde_json::from_value(list.clone()).expect("chunk refs")
}

fn keys(chunks: &[ChunkRef]) -> BTreeSet<String> {
    chunks.iter().map(|c| c.key.clone()).collect()
}

fn u64s(v: &Value) -> Vec<u64> {
    v.as_array()
        .expect("an array")
        .iter()
        .map(|n| n.as_u64().expect("an index"))
        .collect()
}

fn variable(session: &Session, name: &str) -> u32 {
    session
        .variables()
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("no variable {name}"))
        .index
}

/// A request over `t2m(time, latitude, longitude)` at one time step.
fn request(variable: u32, bounds: [f64; 4], time: u32) -> WindowRequest {
    WindowRequest::new(variable, 1, 2, bounds, vec![time, 0, 0])
}

fn bounds(v: &Value) -> [f64; 4] {
    let b: Vec<f64> = v
        .as_array()
        .expect("bounds")
        .iter()
        .map(|n| n.as_f64().expect("a number"))
        .collect();
    b.try_into().expect("four edges")
}

#[test]
fn a_box_and_a_time_index_plan_exactly_the_chunks_they_touch() {
    let oracle = oracle();
    for store in STORES {
        let case = &oracle[store];
        let all = objects(store);
        // The coordinate chunks zarr-python left unwritten are the ones the
        // fixture lacks, so the fetches below see them as 404s.
        let unwritten: BTreeSet<String> = refs(&case["coordinate_chunks"])
            .into_iter()
            .map(|c| c.key)
            .filter(|k| !all.iter().any(|(key, _)| key == k))
            .collect();
        let recorded: BTreeSet<String> = case["unwritten_coordinate_chunks"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|k| k.as_str().expect("a key").to_string())
            .collect();
        assert_eq!(unwritten, recorded, "{store}: unwritten coordinate chunks");

        // 1. The metadata alone names the coordinate chunks.
        let bare = Session::open_store(holding(&all, &BTreeSet::new())).expect("opens");
        let t2m = variable(&bare, "t2m");
        let coordinates = bare
            .coordinate_chunks(t2m)
            .expect("plans")
            .expect("a store is keyed");
        assert_eq!(coordinates, refs(&case["coordinate_chunks"]), "{store}");

        // 2. With those, every window plans the chunks the oracle names.
        let session = Session::open_store(holding(&all, &keys(&coordinates))).expect("opens");
        for window in case["windows"].as_array().expect("windows") {
            let name = format!("{store}/{}", window["name"]);
            let time = u32::try_from(window["time"].as_u64().expect("time")).expect("small");
            let plan = session
                .plan_window(&request(t2m, bounds(&window["bounds"]), time))
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            let rows = u64s(&window["rows"]);
            assert_eq!(plan.rows.to_vec(), rows, "{name}: rows");
            let columns: Vec<Vec<u64>> = window["columns"]
                .as_array()
                .expect("runs")
                .iter()
                .map(u64s)
                .collect();
            let planned: Vec<Vec<u64>> = plan.columns.iter().map(|r| r.to_vec()).collect();
            assert_eq!(planned, columns, "{name}: columns");
            assert_eq!(
                u64::from(plan.nj),
                rows[1] - rows[0],
                "{name}: nj is the rows"
            );
            assert_eq!(
                u64::from(plan.ni),
                columns.iter().map(|r| r[1] - r[0]).sum::<u64>(),
                "{name}: ni is the runs"
            );
            assert_eq!(
                plan.chunks.expect("a store is keyed"),
                refs(&window["chunks"]),
                "{name}: chunks"
            );
        }
    }
}

#[test]
fn a_window_decodes_from_its_planned_chunks_and_nothing_else() {
    let oracle = oracle();
    for store in STORES {
        let case = &oracle[store];
        let all = objects(store);
        let coordinates = keys(&refs(&case["coordinate_chunks"]));
        let planner = Session::open_store(holding(&all, &coordinates)).expect("opens");
        let t2m = variable(&planner, "t2m");
        // The whole store, for the slice a window is cut from.
        let whole = Session::open_store(MemoryObjects::from_iter(all.clone())).expect("opens");

        for window in case["windows"].as_array().expect("windows") {
            let name = format!("{store}/{}", window["name"]);
            let time = u32::try_from(window["time"].as_u64().expect("time")).expect("small");
            let asked = request(t2m, bounds(&window["bounds"]), time);
            let plan = planner.plan_window(&asked).expect("plans");
            let planned = keys(&plan.chunks.expect("keyed"));

            // A host holding the metadata, the coordinates and the planned
            // chunks, and not one other object of the store.
            let supplied: BTreeSet<String> = coordinates.union(&planned).cloned().collect();
            let recorded = Rc::new(Recording::new(holding(&all, &supplied)));
            let session = Session::open_store(Shared(Rc::clone(&recorded))).expect("opens");
            recorded.clear();
            let field = session
                .decode_window(&asked, &DecodeOptions::default())
                .unwrap_or_else(|e| panic!("{name}: {e}"));

            let asked_for: BTreeSet<String> = recorded.gets().into_iter().collect();
            let data: BTreeSet<String> = asked_for
                .iter()
                .filter(|k| k.starts_with("t2m/"))
                .cloned()
                .collect();
            assert_eq!(data, planned, "{name}: the chunks the decode read");
            assert!(
                asked_for.is_subset(&supplied),
                "{name}: the decode asked for {:?}, which the host never fetched",
                asked_for.difference(&supplied).collect::<Vec<_>>()
            );

            // The values, as xarray reads them back.
            let want: Vec<f64> = window["values"]
                .as_array()
                .expect("values")
                .iter()
                .map(|v| v.as_f64().expect("a value"))
                .collect();
            assert_eq!(field.values.to_f64(), want, "{name}: values");
            assert!(field.mask.iter().all(|&m| m == 1), "{name}: nothing masked");
            assert_eq!(field.ni as usize * field.nj as usize, want.len(), "{name}");
            assert_eq!(field.parameter.as_deref(), Some("t2m"));
            assert_eq!(field.units.as_deref(), Some("K"));

            // Placed at its own corners: the first and last cell's centres.
            let lats = window["latitude"].as_array().expect("latitudes");
            let lons = window["longitude"].as_array().expect("longitudes");
            let placed = field.georef.geometry.forward_at();
            let (ni, nj) = (field.ni, field.nj);
            for (i, j, lat, lon) in [
                (0, 0, &lats[0], &lons[0]),
                (ni - 1, nj - 1, &lats[lats.len() - 1], &lons[lons.len() - 1]),
            ] {
                let (got_lat, got_lon) = placed(i, j).expect("a placed corner");
                let (lat, lon) = (lat.as_f64().expect("lat"), lon.as_f64().expect("lon"));
                assert!(
                    (got_lat - lat).abs() < 1e-9,
                    "{name}: lat {got_lat} vs {lat}"
                );
                assert!(
                    ((got_lon - lon).rem_euclid(360.0)).min((lon - got_lon).rem_euclid(360.0))
                        < 1e-9,
                    "{name}: lon {got_lon} vs {lon}"
                );
            }

            // And cut from the slice: every cell is the whole plane's cell at
            // the same row and column.
            let plane = whole
                .decode_slice(t2m, 1, 2, &[time, 0, 0], &DecodeOptions::default())
                .expect("the whole plane");
            let plane_values = plane.values.to_f64();
            let rows = plan.rows;
            let mut at = 0;
            for j in rows[0]..rows[1] {
                for run in &plan.columns {
                    for i in run[0]..run[1] {
                        let cell = (j * u64::from(plane.ni) + i) as usize;
                        let window_cell = field.values.to_f64()[at];
                        assert_eq!(window_cell, plane_values[cell], "{name}: ({i}, {j})");
                        at += 1;
                    }
                }
            }
        }
    }
}

/// A sharded variable planned in two steps and decoded from byte ranges
/// (#1123): the plan names each shard's index, then, with the indexes in
/// hand, exactly the inner chunks the window intersects; the window decodes
/// from those ranges and the indexes, and never reads a shard whole. The
/// shards, index ranges and inner-chunk ranges are the oracle's, read from the
/// shard files with Python's `struct`, and the values are xarray's.
#[test]
fn a_sharded_window_plans_and_decodes_exactly_the_inner_chunks_it_intersects() {
    let oracle = oracle();
    let case = &oracle["window_sharded_v3"];
    let all = objects("window_sharded_v3");
    let coordinates = keys(&refs(&case["coordinate_chunks"]));
    let bytes_of = |key: &str| -> &[u8] {
        &all.iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("no shard {key}"))
            .1
    };
    let range_of = |key: &str, start: u64, len: u64| -> Vec<u8> {
        let at = usize::try_from(start).expect("small");
        bytes_of(key)[at..at + usize::try_from(len).expect("small")].to_vec()
    };
    for name in ["t2m", "t2m_start"] {
        for window in case["windows"][name].as_array().expect("windows") {
            let label = format!("{name}/{}", window["name"]);
            let time = u32::try_from(window["time"].as_u64().expect("time")).expect("small");

            // 1. The metadata and the coordinates: the shards and their
            // indexes, and no inner chunk yet.
            let mut held = holding(&all, &coordinates);
            let session = Session::open_store(holding(&all, &coordinates)).expect("opens");
            let asked = request(variable(&session, name), bounds(&window["bounds"]), time);
            let plan = session.plan_window(&asked).expect("plans");
            assert_eq!(plan.chunks, Some(refs(&window["chunks"])), "{label}");
            let indexes: Vec<ShardIndexRef> =
                serde_json::from_value(window["shard_indexes"].clone()).expect("index refs");
            assert_eq!(plan.shard_indexes.as_ref(), Some(&indexes), "{label}");
            assert_eq!(plan.inner_chunks, None, "{label}: no index is in hand");

            // The host fetches each index by range, learning the size.
            for index in &indexes {
                let size = window["shard_sizes"][&index.key].as_u64().expect("size");
                let start = match index.location {
                    fieldglass::IndexLocation::End => size - index.length,
                    _ => 0,
                };
                held.insert_range(
                    index.key.clone(),
                    size,
                    start,
                    range_of(&index.key, start, index.length),
                )
                .unwrap();
            }

            // 2. Exactly the inner chunks the window intersects.
            let session = Session::open_store(held).expect("opens");
            let plan = session.plan_window(&asked).expect("plans");
            let inner: Vec<InnerChunkRef> =
                serde_json::from_value(window["inner_chunks"].clone()).expect("inner refs");
            assert_eq!(plan.inner_chunks.as_ref(), Some(&inner), "{label}");

            // Decoding before they are fetched names each one's range.
            let err = session
                .decode_window(&asked, &DecodeOptions::default())
                .expect_err("the inner chunks are owed");
            assert_eq!(err.code(), "invalid_option", "{label}: {err}");
            for c in &inner {
                let range = format!("bytes {}..{} of `{}`", c.offset, c.offset + c.length, c.key);
                assert!(err.message().contains(&range), "{label}: {err}");
            }

            // 3. Fetched: decoded from those ranges and the indexes alone.
            let mut held = holding(&all, &coordinates);
            for (start, len, key) in indexes
                .iter()
                .map(|i| {
                    let size = window["shard_sizes"][&i.key].as_u64().expect("size");
                    let start = match i.location {
                        fieldglass::IndexLocation::End => size - i.length,
                        _ => 0,
                    };
                    (start, i.length, i.key.clone())
                })
                .chain(inner.iter().map(|c| (c.offset, c.length, c.key.clone())))
            {
                let size = bytes_of(&key).len() as u64;
                held.insert_range(key.clone(), size, start, range_of(&key, start, len))
                    .unwrap();
            }
            let recorded = Rc::new(Recording::new(held));
            let session = Session::open_store(Shared(Rc::clone(&recorded))).expect("opens");
            recorded.clear();
            let field = session
                .decode_window(&asked, &DecodeOptions::default())
                .unwrap_or_else(|e| panic!("{label}: {e}"));
            let data = format!("{name}/c/");
            assert!(
                recorded.gets().iter().all(|k| !k.starts_with(&data)),
                "{label}: a shard was read whole: {:?}",
                recorded.gets()
            );
            for (key, range) in recorded.range_gets() {
                let planned = indexes.iter().any(|i| {
                    i.key == key
                        && range.len == i.length
                        && (range.start == 0 || range.end() == Some(bytes_of(&key).len() as u64))
                }) || inner
                    .iter()
                    .any(|c| c.key == key && c.offset == range.start && c.length == range.len);
                assert!(
                    planned,
                    "{label}: read {range:?} of {key}, which no plan named"
                );
            }

            // The values, as xarray reads them back; the unwritten inner
            // chunk's cells are masked.
            let values = field.values.to_f64();
            let want = window["values"].as_array().expect("values");
            assert_eq!(values.len(), want.len(), "{label}");
            for (at, (want, (&mask, &got))) in want
                .iter()
                .zip(field.mask.iter().zip(values.iter()))
                .enumerate()
            {
                match want.as_f64() {
                    Some(want) => assert_eq!((mask, got), (1, want), "{label}: cell {at}"),
                    None => assert_eq!(mask, 0, "{label}: cell {at} is fill"),
                }
            }
        }
    }
}

/// A host that fetches a sharded variable's shards whole, as before #1123,
/// still decodes the window, and its plan still names the inner chunks, read
/// from the indexes inside the whole shards.
#[test]
fn whole_shards_still_decode_and_plan_their_inner_chunks() {
    let oracle = oracle();
    let case = &oracle["window_sharded_v3"];
    let all = objects("window_sharded_v3");
    let coordinates = keys(&refs(&case["coordinate_chunks"]));
    let window = &case["windows"]["t2m"][0];
    let time = u32::try_from(window["time"].as_u64().expect("time")).expect("small");
    let shards = keys(&refs(&window["chunks"]));
    let given: BTreeSet<String> = coordinates.union(&shards).cloned().collect();
    let session = Session::open_store(holding(&all, &given)).expect("opens");
    let asked = request(variable(&session, "t2m"), bounds(&window["bounds"]), time);
    let plan = session.plan_window(&asked).expect("plans");
    let inner: Vec<InnerChunkRef> =
        serde_json::from_value(window["inner_chunks"].clone()).expect("inner refs");
    assert_eq!(plan.inner_chunks, Some(inner));
    let field = session
        .decode_window(&asked, &DecodeOptions::default())
        .expect("decodes");
    let want = window["values"].as_array().expect("values");
    for (at, (want, (&mask, &got))) in want
        .iter()
        .zip(field.mask.iter().zip(field.values.to_f64().iter()))
        .enumerate()
    {
        match want.as_f64() {
            Some(want) => assert_eq!((mask, got), (1, want), "cell {at}"),
            None => assert_eq!(mask, 0, "cell {at} is fill"),
        }
    }
}

/// The sharded fixture with its index ranges given for `window`'s shards, and
/// `edit` applied to each object first.
fn sharded_with_indexes(
    window: &Value,
    coordinates: &BTreeSet<String>,
    edit: impl Fn(&str, Vec<u8>) -> Vec<u8>,
) -> PrefetchedObjects {
    let all: Vec<(String, Vec<u8>)> = objects("window_sharded_v3")
        .into_iter()
        .map(|(k, v)| {
            let v = edit(&k, v);
            (k, v)
        })
        .collect();
    let mut held = holding(&all, coordinates);
    let indexes: Vec<ShardIndexRef> =
        serde_json::from_value(window["shard_indexes"].clone()).expect("index refs");
    for index in indexes {
        let shard = &all.iter().find(|(k, _)| *k == index.key).expect("shard").1;
        let size = shard.len() as u64;
        let start = size - index.length;
        held.insert_range(
            index.key.clone(),
            size,
            start,
            shard[usize::try_from(start).expect("small")..].to_vec(),
        )
        .unwrap();
    }
    held
}

/// A plan names a sharded variable's shards in `chunks` whatever else
/// happens, so one that cannot be planned by range leaves the shard fields
/// `null` and the host fetches the shards whole, rather than the plan failing
/// (#1123 review). Each case failed `plan_window` outright before.
#[test]
fn a_sharded_plan_that_cannot_go_by_range_still_names_its_shards() {
    let oracle = oracle();
    let case = &oracle["window_sharded_v3"];
    let coordinates = keys(&refs(&case["coordinate_chunks"]));
    let window = &case["windows"]["t2m"][0];
    let time = u32::try_from(window["time"].as_u64().expect("time")).expect("small");
    let inner: Vec<InnerChunkRef> =
        serde_json::from_value(window["inner_chunks"].clone()).expect("inner refs");

    // An inner codec this build does not decode: locating the inner chunks
    // reads only the index, so the plan is the full one.
    let unknown = |key: &str, bytes: Vec<u8>| {
        if key == "zarr.json" || key == "t2m/zarr.json" {
            String::from_utf8(bytes)
                .expect("JSON")
                .replace("\"name\": \"blosc\"", "\"name\": \"made_up\"")
                .into_bytes()
        } else {
            bytes
        }
    };
    let session =
        Session::open_store(sharded_with_indexes(window, &coordinates, unknown)).expect("opens");
    let asked = request(variable(&session, "t2m"), bounds(&window["bounds"]), time);
    let plan = session.plan_window(&asked).expect("plans");
    assert_eq!(plan.chunks, Some(refs(&window["chunks"])));
    assert_eq!(
        plan.inner_chunks,
        Some(inner.clone()),
        "unknown inner codec"
    );

    // A whole shard whose index fails its checksum: the shards, and no inner
    // chunks; the decode says what is wrong.
    let all = objects("window_sharded_v3");
    let shards = keys(&refs(&window["chunks"]));
    let broken: Vec<(String, Vec<u8>)> = all
        .into_iter()
        .map(|(k, mut v)| {
            if shards.contains(&k) {
                let at = v.len() - 30;
                v[at] ^= 0xff;
            }
            (k, v)
        })
        .collect();
    let given: BTreeSet<String> = coordinates.union(&shards).cloned().collect();
    let session = Session::open_store(holding(&broken, &given)).expect("opens");
    let plan = session.plan_window(&asked).expect("plans");
    assert_eq!(plan.chunks, Some(refs(&window["chunks"])), "corrupt index");
    assert!(plan.shard_indexes.is_some(), "the indexes are metadata");
    assert_eq!(plan.inner_chunks, None, "a corrupt index locates nothing");
    let err = session
        .decode_window(&asked, &DecodeOptions::default())
        .expect_err("the shard is corrupt");
    assert_eq!(err.code(), "decode", "{err}");
}

/// A sharding codec with another codec after it does not make the stored
/// object a shard: it is a compressed one, whose index no range can reach. So
/// the plan treats the variable as unsharded, the rule `CodecChain::sharding`
/// follows (#1123 review).
#[test]
fn a_shard_behind_another_codec_plans_as_unsharded() {
    let oracle = oracle();
    let case = &oracle["window_sharded_v3"];
    let coordinates = keys(&refs(&case["coordinate_chunks"]));
    let window = &case["windows"]["t2m"][0];
    let time = u32::try_from(window["time"].as_u64().expect("time")).expect("small");
    let gzip_after = |key: &str, bytes: Vec<u8>| {
        let codecs = |doc: &mut Value| {
            doc["codecs"]
                .as_array_mut()
                .expect("codecs")
                .push(serde_json::json!({"name": "gzip", "configuration": {"level": 5}}));
        };
        let mut doc: Value = match key {
            "zarr.json" | "t2m/zarr.json" => serde_json::from_slice(&bytes).expect("JSON"),
            _ => return bytes,
        };
        if key == "zarr.json" {
            codecs(&mut doc["consolidated_metadata"]["metadata"]["t2m"]);
        } else {
            codecs(&mut doc);
        }
        serde_json::to_vec(&doc).expect("JSON")
    };
    let session =
        Session::open_store(sharded_with_indexes(window, &coordinates, gzip_after)).expect("opens");
    let asked = request(variable(&session, "t2m"), bounds(&window["bounds"]), time);
    let plan = session.plan_window(&asked).expect("plans");
    assert_eq!(plan.chunks, Some(refs(&window["chunks"])));
    assert_eq!(
        plan.shard_indexes, None,
        "a gzipped shard has no index to range"
    );
    assert_eq!(plan.inner_chunks, None);
}

/// `t(lat, lon)` in shards of `shard_rows` x 1024 cells, as one-cell inner
/// chunks, over a regular global grid of `rows` x 1024: metadata and
/// coordinates only.
fn one_cell_inner_chunks(rows: usize, shard_rows: usize) -> MemoryObjects {
    let lat: Vec<f64> = (0..rows)
        .map(|j| 90.0 - (j as f64 + 0.5) * 180.0 / rows as f64)
        .collect();
    let lon: Vec<f64> = (0..1024).map(|i| f64::from(i) * 360.0 / 1024.0).collect();
    let t = format!(
        "{{\"zarr_format\":3,\"node_type\":\"array\",\"shape\":[{rows},1024],\
         \"data_type\":\"float32\",\"chunk_grid\":{{\"name\":\"regular\",\
         \"configuration\":{{\"chunk_shape\":[{shard_rows},1024]}}}},\
         \"chunk_key_encoding\":{{\"name\":\"default\",\"configuration\":{{\"separator\":\"/\"}}}},\
         \"fill_value\":\"NaN\",\"codecs\":[{{\"name\":\"sharding_indexed\",\"configuration\":{{\
         \"chunk_shape\":[1,1],\
         \"codecs\":[{{\"name\":\"bytes\",\"configuration\":{{\"endian\":\"little\"}}}}],\
         \"index_codecs\":[{{\"name\":\"bytes\",\"configuration\":{{\"endian\":\"little\"}}}},\
         {{\"name\":\"crc32c\"}}]}}}}],\
         \"dimension_names\":[\"lat\",\"lon\"],\"attributes\":{{}}}}"
    );
    MemoryObjects::from_iter([
        ("zarr.json", V3_GROUP.to_vec()),
        (
            "lat/zarr.json",
            v3_array(&format!("[{rows}]"), &format!("[{rows}]"), "[\"lat\"]"),
        ),
        ("lat/c/0", f8(&lat)),
        ("lon/zarr.json", v3_array("[1024]", "[1024]", "[\"lon\"]")),
        ("lon/c/0", f8(&lon)),
        ("t/zarr.json", t.into_bytes()),
    ])
}

/// Metadata cannot make a plan name a vast index or a vast list of inner
/// chunks: a shard over the inner-chunk cap has no index plan, and a window
/// touching more inner chunks than one plan holds has no inner-chunk plan.
/// Both still name the shards (#1123 review).
#[test]
fn a_sharded_plan_past_its_caps_names_its_shards_and_nothing_finer() {
    let whole_globe = [-90.0, 90.0, -180.0, 180.0];

    // 1024 x 1024 one-cell inner chunks per shard is the cap exactly; two
    // shards' worth is twice what one plan may name.
    let session = Session::open_store(one_cell_inner_chunks(2048, 1024)).expect("opens");
    let t = variable(&session, "t");
    let plan = session
        .plan_window(&WindowRequest::new(t, 0, 1, whole_globe, vec![0, 0]))
        .expect("plans");
    assert_eq!(plan.chunks.as_ref().map(Vec::len), Some(2));
    let indexes = plan.shard_indexes.expect("within the cap");
    assert_eq!(indexes[0].length, (1 << 20) * 16 + 4);
    assert_eq!(plan.inner_chunks, None, "two million inner chunks");

    // One row of inner chunks more per shard is past the cap: no index plan.
    let session = Session::open_store(one_cell_inner_chunks(1025, 1025)).expect("opens");
    let t = variable(&session, "t");
    let plan = session
        .plan_window(&WindowRequest::new(t, 0, 1, whole_globe, vec![0, 0]))
        .expect("plans");
    assert_eq!(plan.chunks.as_ref().map(Vec::len), Some(1));
    assert_eq!(plan.shard_indexes, None, "an index past the cap");
    assert_eq!(plan.inner_chunks, None);
}

#[test]
fn a_box_that_holds_no_cell_is_an_empty_plan_and_no_window() {
    let all = objects("window_v3");
    let session = Session::open_store(MemoryObjects::from_iter(all)).expect("opens");
    let t2m = variable(&session, "t2m");
    // North of the northernmost row (70).
    let off = request(t2m, [75.0, 89.0, 0.0, 90.0], 0);
    let plan = session.plan_window(&off).expect("an empty plan is a plan");
    assert_eq!((plan.ni, plan.nj), (0, 0));
    assert!(plan.columns.is_empty());
    assert_eq!(plan.rows, [0, 0]);
    assert_eq!(plan.chunks, Some(Vec::new()));
    let err = session
        .decode_window(&off, &DecodeOptions::default())
        .expect_err("nothing to decode");
    assert_eq!(err.code(), "invalid_option", "{err}");
    // Between two columns (0 and 30).
    let between = request(t2m, [0.0, 70.0, 5.0, 25.0], 0);
    assert_eq!(session.plan_window(&between).expect("plans").ni, 0);
}

#[test]
fn a_request_that_does_not_describe_a_window_is_refused() {
    let all = objects("window_v2");
    let session = Session::open_store(MemoryObjects::from_iter(all)).expect("opens");
    let t2m = variable(&session, "t2m");
    let good = [0.0, 40.0, 0.0, 90.0];
    let refused = |r: WindowRequest, code: &str, what: &str| {
        let plan = session.plan_window(&r).expect_err(what);
        assert_eq!(plan.code(), code, "{what}: {plan}");
        let decode = session
            .decode_window(&r, &DecodeOptions::default())
            .expect_err(what);
        assert_eq!(decode.code(), code, "{what}: {decode}");
    };
    refused(
        WindowRequest::new(t2m, 1, 2, good, vec![0, 0]),
        "invalid_option",
        "too few slice indices",
    );
    refused(
        WindowRequest::new(t2m, 1, 1, good, vec![0, 0, 0]),
        "invalid_option",
        "the same axis twice",
    );
    refused(
        WindowRequest::new(t2m, 1, 3, good, vec![0, 0, 0]),
        "invalid_option",
        "an axis past the rank",
    );
    refused(
        request(t2m, good, 3),
        "invalid_option",
        "a time step past the end",
    );
    refused(
        request(t2m, [40.0, 0.0, 0.0, 90.0], 0),
        "invalid_option",
        "latitudes upside down",
    );
    refused(
        request(t2m, [0.0, 40.0, 90.0, 0.0], 0),
        "invalid_option",
        "a west edge east of the east edge",
    );
    refused(
        request(t2m, [0.0, f64::NAN, 0.0, 90.0], 0),
        "invalid_option",
        "a NaN edge",
    );
    refused(
        request(99, good, 0),
        "no_such_message",
        "a variable past the list",
    );
    // Time against longitude is a cross-section, not a map: no lat/lon
    // placement, so no window.
    refused(
        WindowRequest::new(t2m, 0, 2, good, vec![0, 0, 0]),
        "unsupported",
        "a slice nothing places as lat/lon",
    );
}

/// A time step past the end is worded as `decode_slice` and `decode_line` word
/// it: one check, one message (#1301).
#[test]
fn a_time_step_past_the_end_names_the_axis() {
    let session =
        Session::open_store(MemoryObjects::from_iter(objects("window_v2"))).expect("opens");
    let t2m = variable(&session, "t2m");
    let err = session
        .plan_window(&request(t2m, [0.0, 40.0, 0.0, 90.0], 3))
        .expect_err("past the end");
    assert_eq!(err.code(), "invalid_option");
    assert!(err.to_string().contains("past the end of `"), "{err}");
}

#[test]
fn a_store_with_no_coordinates_is_not_windowed() {
    // `temp(y, x)` carries no coordinate arrays, so its slice is placed in its
    // own source projection and a lat/lon box means nothing over it.
    let dir = format!("{FIXTURES}/stores/v3_nested");
    let mut entries = Vec::new();
    for key in [
        "zarr.json",
        "temp/zarr.json",
        "sub/zarr.json",
        "sub/inner/zarr.json",
    ] {
        entries.push((
            key.to_string(),
            std::fs::read(format!("{dir}/{key}")).expect("read"),
        ));
    }
    let session = Session::open_store(MemoryObjects::from_iter(entries)).expect("opens");
    let temp = variable(&session, "temp");
    assert_eq!(
        session.coordinate_chunks(temp).expect("plans"),
        Some(Vec::new()),
        "no axis has a coordinate array"
    );
    let err = session
        .plan_window(&WindowRequest::new(
            temp,
            0,
            1,
            [0.0, 1.0, 0.0, 1.0],
            vec![0, 0],
        ))
        .expect_err("not windowed");
    assert_eq!(err.code(), "unsupported", "{err}");
}

#[test]
fn a_netcdf_file_decodes_a_window_and_plans_no_keys() {
    let twin = Session::open(std::fs::read(format!("{FIXTURES}/cf_twin.nc")).expect("twin"))
        .expect("opens");
    let t = variable(&twin, "t");
    assert_eq!(twin.coordinate_chunks(t).expect("asks"), None);
    // lat 10, 20, 30 and lon 0..3: rows 1..3, columns 1..4.
    let asked = WindowRequest::new(t, 0, 1, [15.0, 30.0, 0.5, 3.0], vec![0, 0]);
    let plan = twin.plan_window(&asked).expect("plans");
    assert_eq!(plan.rows, [1, 3]);
    assert_eq!(plan.columns, vec![[1, 4]]);
    assert_eq!(
        plan.chunks, None,
        "a NetCDF file is read by range, not by key"
    );
    let field = twin
        .decode_window(&asked, &DecodeOptions::default())
        .expect("decodes");
    let plane = twin
        .decode_slice(t, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("the plane");
    let (values, plane_values) = (field.values.to_f64(), plane.values.to_f64());
    let mut at = 0;
    for j in 1..3 {
        for i in 1..4 {
            let cell = j * plane.ni as usize + i;
            assert_eq!(field.mask[at], plane.mask[cell], "({i}, {j}) mask");
            if field.mask[at] == 1 {
                assert_eq!(values[at], plane_values[cell], "({i}, {j})");
            }
            at += 1;
        }
    }
    // The packed int16 arrives unpacked once, as the plane does: row 1 of
    // `t` is 260.25, 270, 280, 290.5 K, and the window starts at its column 1.
    assert_eq!(values[0], 270.0);
}

#[test]
fn a_window_past_the_field_cap_is_refused_before_it_is_allocated() {
    // A 70,000 x 70,000 global grid in one unwritten chunk: the coordinates
    // are real, the data is 4.9 billion cells of fill. Planning it is cheap;
    // decoding the whole of it must be refused, not attempted.
    let n = 70_000u32;
    let axis = |first: f64, step: f64| -> Vec<u8> {
        (0..n)
            .flat_map(|i| (first + step * f64::from(i)).to_le_bytes())
            .collect()
    };
    let zarray = |shape: &str, dtype: &str| {
        format!(
            "{{\"zarr_format\":2,\"shape\":{shape},\"chunks\":{shape},\"dtype\":\"{dtype}\",\
             \"fill_value\":0,\"compressor\":null,\"filters\":null,\"order\":\"C\"}}"
        )
        .into_bytes()
    };
    let dims = |names: &str| format!("{{\"_ARRAY_DIMENSIONS\":{names}}}").into_bytes();
    let store = MemoryObjects::from_iter([
        (".zgroup", b"{\"zarr_format\":2}".to_vec()),
        ("lat/.zarray", zarray(&format!("[{n}]"), "<f8")),
        ("lat/.zattrs", dims("[\"lat\"]")),
        ("lat/0", axis(89.99, -179.98 / f64::from(n - 1))),
        ("lon/.zarray", zarray(&format!("[{n}]"), "<f8")),
        ("lon/.zattrs", dims("[\"lon\"]")),
        ("lon/0", axis(0.0, 360.0 / f64::from(n))),
        ("t/.zarray", zarray(&format!("[{n},{n}]"), "<f4")),
        ("t/.zattrs", dims("[\"lat\",\"lon\"]")),
    ]);
    let session = Session::open_store(store).expect("opens");
    let t = variable(&session, "t");
    let whole = WindowRequest::new(t, 0, 1, [-90.0, 90.0, -180.0, 180.0], vec![0, 0]);
    let plan = session.plan_window(&whole).expect("plans");
    assert_eq!((plan.ni, plan.nj), (n, n));
    assert_eq!(plan.chunks.map(|c| c.len()), Some(1));
    let err = session
        .decode_window(&whole, &DecodeOptions::default())
        .expect_err("too many cells for one field");
    assert_eq!(err.code(), "unsupported", "{err}");
}

#[test]
fn a_variable_stored_longitude_first_windows_as_rows_of_latitude() {
    // `t(lon, lat)`: the X axis precedes the Y axis in storage, so each region
    // comes back columns outer and has to be turned into rows. Raw `<f8` in
    // 2 x 3 chunks, `t(i, j) = 10 i + j`, on a global 90-degree grid.
    let zarray = |shape: &str, chunks: &str| {
        format!(
            "{{\"zarr_format\":2,\"shape\":{shape},\"chunks\":{chunks},\"dtype\":\"<f8\",\
             \"fill_value\":null,\"compressor\":null,\"filters\":null,\"order\":\"C\"}}"
        )
        .into_bytes()
    };
    let f8 = |values: &[f64]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
    let dims = |names: &str| format!("{{\"_ARRAY_DIMENSIONS\":{names}}}").into_bytes();
    let store = MemoryObjects::from_iter([
        (".zgroup", b"{\"zarr_format\":2}".to_vec()),
        ("lat/.zarray", zarray("[3]", "[3]")),
        ("lat/.zattrs", dims("[\"lat\"]")),
        ("lat/0", f8(&[10.0, 20.0, 30.0])),
        ("lon/.zarray", zarray("[4]", "[4]")),
        ("lon/.zattrs", dims("[\"lon\"]")),
        ("lon/0", f8(&[0.0, 90.0, 180.0, 270.0])),
        ("t/.zarray", zarray("[4,3]", "[2,3]")),
        ("t/.zattrs", dims("[\"lon\",\"lat\"]")),
        ("t/0.0", f8(&[0.0, 1.0, 2.0, 10.0, 11.0, 12.0])),
        ("t/1.0", f8(&[20.0, 21.0, 22.0, 30.0, 31.0, 32.0])),
    ]);
    let session = Session::open_store(store).expect("opens");
    let t = variable(&session, "t");
    // Latitudes 20 and 30; longitudes 270, 0 and 90, across the seam.
    let asked = WindowRequest::new(t, 1, 0, [15.0, 30.0, 260.0, 460.0], vec![0, 0]);
    let plan = session.plan_window(&asked).expect("plans");
    assert_eq!(plan.rows, [1, 3]);
    assert_eq!(plan.columns, vec![[3, 4], [0, 2]]);
    let keys: Vec<&str> = plan
        .chunks
        .as_ref()
        .expect("keyed")
        .iter()
        .map(|c| c.key.as_str())
        .collect();
    assert_eq!(keys, ["t/0.0", "t/1.0"]);
    let field = session
        .decode_window(&asked, &DecodeOptions::default())
        .expect("decodes");
    assert_eq!((field.ni, field.nj), (3, 2));
    assert_eq!(
        field.values.to_f64().to_vec(),
        [31.0, 1.0, 11.0, 32.0, 2.0, 12.0],
        "rows of latitude, columns east from 270"
    );
}

/// A v3 array's `zarr.json`, raw little-endian `float64` through the `bytes`
/// codec, for the in-memory stores below.
fn v3_array(shape: &str, chunks: &str, dims: &str) -> Vec<u8> {
    format!(
        "{{\"zarr_format\":3,\"node_type\":\"array\",\"shape\":{shape},\"data_type\":\"float64\",\
         \"chunk_grid\":{{\"name\":\"regular\",\"configuration\":{{\"chunk_shape\":{chunks}}}}},\
         \"chunk_key_encoding\":{{\"name\":\"default\",\"configuration\":{{\"separator\":\"/\"}}}},\
         \"fill_value\":\"NaN\",\"codecs\":[{{\"name\":\"bytes\",\"configuration\":{{\"endian\":\"little\"}}}}],\
         \"dimension_names\":{dims},\"attributes\":{{}}}}"
    )
    .into_bytes()
}

fn f8(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

const V3_GROUP: &[u8] = b"{\"zarr_format\":3,\"node_type\":\"group\",\"attributes\":{}}";

/// `grp/t(lat, lon)` with `grp/lat` and `grp/lon` beside it: what a store
/// written from a nested xarray `DataTree` holds.
fn sub_group_objects(with_coordinates: bool) -> Vec<(&'static str, Vec<u8>)> {
    let mut objects = vec![
        ("zarr.json", V3_GROUP.to_vec()),
        ("grp/zarr.json", V3_GROUP.to_vec()),
        ("grp/lat/zarr.json", v3_array("[3]", "[3]", "[\"lat\"]")),
        ("grp/lon/zarr.json", v3_array("[4]", "[4]", "[\"lon\"]")),
        (
            "grp/t/zarr.json",
            v3_array("[3,4]", "[3,2]", "[\"lat\",\"lon\"]"),
        ),
        ("grp/t/c/0/0", f8(&[0.0, 1.0, 10.0, 11.0, 20.0, 21.0])),
        ("grp/t/c/0/1", f8(&[2.0, 3.0, 12.0, 13.0, 22.0, 23.0])),
    ];
    if with_coordinates {
        objects.push(("grp/lat/c/0", f8(&[10.0, 20.0, 30.0])));
        objects.push(("grp/lon/c/0", f8(&[0.0, 90.0, 180.0, 270.0])));
    }
    objects
}

fn sub_group_store(with_coordinates: bool) -> MemoryObjects {
    MemoryObjects::from_iter(sub_group_objects(with_coordinates))
}

/// The same objects, as a host that has fetched exactly these and no others.
fn prefetched(objects: Vec<(&'static str, Vec<u8>)>) -> PrefetchedObjects {
    let mut held = PrefetchedObjects::new();
    for (key, bytes) in objects {
        held.insert(key, bytes);
    }
    held
}

#[test]
fn a_variable_in_a_sub_group_names_its_own_groups_coordinates() {
    let session = Session::open_store(sub_group_store(true)).expect("opens");
    let t = variable(&session, "grp/t");
    let keys: Vec<(String, String)> = session
        .coordinate_chunks(t)
        .expect("plans")
        .expect("keyed")
        .into_iter()
        .map(|c| (c.array, c.key))
        .collect();
    assert_eq!(
        keys,
        [
            ("grp/lat".to_string(), "grp/lat/c/0".to_string()),
            ("grp/lon".to_string(), "grp/lon/c/0".to_string()),
        ]
    );
    let asked = WindowRequest::new(t, 0, 1, [15.0, 30.0, 170.0, 280.0], vec![0, 0]);
    let plan = session.plan_window(&asked).expect("plans");
    assert_eq!((plan.rows, plan.columns.clone()), ([1, 3], vec![[2, 4]]));
    let planned: Vec<String> = plan
        .chunks
        .expect("keyed")
        .into_iter()
        .map(|c| c.key)
        .collect();
    assert_eq!(planned, ["grp/t/c/0/1"]);
    let field = session
        .decode_window(&asked, &DecodeOptions::default())
        .expect("decodes");
    assert_eq!(field.values.to_f64().to_vec(), [12.0, 13.0, 22.0, 23.0]);
}

#[test]
fn planning_before_the_coordinates_are_fetched_names_them() {
    // A host that has fetched the metadata and the data but not the
    // coordinates is told which keys to fetch.
    let session = Session::open_store(prefetched(sub_group_objects(false))).expect("opens");
    let t = variable(&session, "grp/t");
    let asked = WindowRequest::new(t, 0, 1, [15.0, 30.0, 170.0, 280.0], vec![0, 0]);
    for err in [
        session.plan_window(&asked).expect_err("no coordinates"),
        session
            .decode_window(&asked, &DecodeOptions::default())
            .expect_err("no coordinates"),
    ] {
        assert_eq!(err.code(), "invalid_option", "{err}");
        let message = err.message();
        assert!(
            message.contains("grp/lat/c/0") && message.contains("grp/lon/c/0"),
            "the missing objects are named: {message}"
        );
    }
}

#[test]
fn a_coordinate_chunk_the_store_never_wrote_is_its_fill_value() {
    // zarr-python skipped `latitude/c/2` of the south-to-north v3 store: it
    // holds only the equator, which is the array's fill value (0.0).
    let all = objects("window_s2n_v3");
    assert!(
        !all.iter().any(|(k, _)| k == "latitude/c/2"),
        "the fixture leaves the chunk unwritten"
    );
    let oracle = oracle();
    let case = &oracle["window_s2n_v3"];
    let equator = case["windows"]
        .as_array()
        .expect("windows")
        .iter()
        .find(|w| w["name"] == "the_unwritten_coordinate_chunk")
        .expect("the equator window");
    let coordinates = keys(&refs(&case["coordinate_chunks"]));

    // Given as absent, the 404 a fetch of it returns: it reads as 0.0, the
    // equator, and the box around the equator plans the oracle's rows.
    let session = Session::open_store(holding(&all, &coordinates)).expect("opens");
    let t2m = variable(&session, "t2m");
    let asked = request(t2m, bounds(&equator["bounds"]), 1);
    let plan = session.plan_window(&asked).expect("plans");
    assert_eq!(plan.rows.to_vec(), u64s(&equator["rows"]));
    let latitudes = session.axis_values(t2m, 1).expect("latitudes");
    assert_eq!(
        latitudes.coordinates,
        Some(vec![-60.0, -50.0, -40.0, -30.0, -20.0, -10.0, 0.0])
    );

    // A whole store that lacks the object says the same.
    let whole = Session::open_store(MemoryObjects::from_iter(all.clone())).expect("opens");
    assert_eq!(whole.plan_window(&asked).expect("plans"), plan);

    // Not given at all, it is a fetch the host still owes, and named.
    let mut owed = coordinates.clone();
    owed.remove("latitude/c/2");
    let session = Session::open_store(holding(&all, &owed)).expect("opens");
    let err = session.plan_window(&asked).expect_err("not fetched");
    assert_eq!(err.code(), "invalid_option", "{err}");
    assert!(err.message().contains("`latitude/c/2`"), "{err}");
}

#[test]
fn a_coordinate_chunk_absent_from_the_store_that_places_nothing_is_named() {
    // `latitude/c/0` is every latitude of the north-to-south v3 store, and its
    // fill value is NaN: given as absent, the coordinate is all fill, which
    // places no grid. The refusal says which chunk did it.
    let all: Vec<(String, Vec<u8>)> = objects("window_v3")
        .into_iter()
        .filter(|(k, _)| k != "latitude/c/0")
        .collect();
    let coordinates = keys(&refs(&oracle()["window_v3"]["coordinate_chunks"]));
    let session = Session::open_store(holding(&all, &coordinates)).expect("opens");
    let t2m = variable(&session, "t2m");
    let asked = request(t2m, [5.0, 45.0, 50.0, 130.0], 0);
    for err in [
        session.plan_window(&asked).expect_err("no latitudes"),
        session
            .decode_window(&asked, &DecodeOptions::default())
            .expect_err("no latitudes"),
    ] {
        assert_eq!(err.code(), "unsupported", "{err}");
        let message = err.message();
        assert!(
            message.contains("`latitude/c/0`") && message.contains("fill value"),
            "the unwritten chunk is named: {message}"
        );
    }
}

#[test]
fn a_planned_chunk_the_host_did_not_fetch_is_named_not_filled() {
    // #1124: a planned chunk left out of the objects is a fetch the host
    // forgot, not a chunk the store never wrote.
    let oracle = oracle();
    let case = &oracle["window_v3"];
    let window = &case["windows"][0];
    assert_eq!(window["name"], "crosses_chunk_edges");
    let all = objects("window_v3");
    let coordinates = keys(&refs(&case["coordinate_chunks"]));
    let planned = refs(&window["chunks"]);
    let (forgotten, kept) = planned.split_first().expect("chunks");
    let mut given: BTreeSet<String> = coordinates.union(&keys(kept)).cloned().collect();
    let time = u32::try_from(window["time"].as_u64().expect("time")).expect("small");

    let session = Session::open_store(holding(&all, &given)).expect("opens");
    let t2m = variable(&session, "t2m");
    let asked = request(t2m, bounds(&window["bounds"]), time);
    let err = session
        .decode_window(&asked, &DecodeOptions::default())
        .expect_err("a planned chunk is missing");
    assert_eq!(err.code(), "invalid_option", "{err}");
    assert!(
        err.message().contains(&format!("`{}`", forgotten.key)),
        "{err}"
    );
    // Any other read that needs it is refused too, by the reader.
    let err = session
        .decode_slice(t2m, 1, 2, &[time, 0, 0], &DecodeOptions::default())
        .expect_err("the plane needs it");
    assert_eq!(err.code(), "decode", "{err}");
    assert!(
        err.message().contains(&format!("`{}`", forgotten.key)),
        "{err}"
    );

    // Given as absent, it is the fill value, NaN: its cells are masked and
    // every other cell is xarray's.
    let without: Vec<(String, Vec<u8>)> = all
        .iter()
        .filter(|(k, _)| *k != forgotten.key)
        .cloned()
        .collect();
    given.insert(forgotten.key.clone());
    let session = Session::open_store(holding(&without, &given)).expect("opens");
    let field = session
        .decode_window(&asked, &DecodeOptions::default())
        .expect("decodes");
    let want: Vec<f64> = window["values"]
        .as_array()
        .expect("values")
        .iter()
        .map(|v| v.as_f64().expect("a value"))
        .collect();
    let rows = u64s(&window["rows"]);
    let columns: Vec<u64> = window["columns"]
        .as_array()
        .expect("runs")
        .iter()
        .flat_map(|r| {
            let r = u64s(r);
            r[0]..r[1]
        })
        .collect();
    let values = field.values.to_f64();
    let mut masked = 0;
    for (at, (&mask, &value)) in field.mask.iter().zip(values.iter()).enumerate() {
        let j = rows[0] + (at / columns.len()) as u64;
        let i = columns[at % columns.len()];
        if [j / 3, i / 4] == [forgotten.index[1], forgotten.index[2]] {
            assert_eq!(mask, 0, "({i}, {j}) is fill");
            masked += 1;
        } else {
            assert_eq!((mask, value), (1, want[at]), "({i}, {j})");
        }
    }
    assert!(masked > 0, "the absent chunk holds window cells");
}

#[test]
fn a_left_out_coordinate_is_named_with_why() {
    // `lat` is stored as complex numbers, which this build does not read, so
    // the store leaves it out and `t(lat, lon)` has no latitudes to place it.
    let lat = String::from_utf8(v3_array("[3]", "[3]", "[\"lat\"]"))
        .expect("text")
        .replace("\"float64\"", "\"complex128\"");
    let session = Session::open_store(MemoryObjects::from_iter([
        ("zarr.json", V3_GROUP.to_vec()),
        ("lat/zarr.json", lat.into_bytes()),
        ("lon/zarr.json", v3_array("[4]", "[4]", "[\"lon\"]")),
        ("lon/c/0", f8(&[0.0, 90.0, 180.0, 270.0])),
        (
            "t/zarr.json",
            v3_array("[3,4]", "[3,4]", "[\"lat\",\"lon\"]"),
        ),
    ]))
    .expect("opens");
    let left_out = session.left_out();
    let why = left_out
        .iter()
        .find(|l| l.name == "lat")
        .unwrap_or_else(|| panic!("lat is left out: {left_out:?}"));
    let t = variable(&session, "t");
    let err = session
        .plan_window(&WindowRequest::new(
            t,
            0,
            1,
            [0.0, 90.0, 0.0, 90.0],
            vec![0, 0],
        ))
        .expect_err("no latitudes");
    assert_eq!(err.code(), "unsupported", "{err}");
    let message = err.message();
    assert!(
        message.contains("`lat`") && message.contains(&why.reason),
        "names the coordinate and why ({}): {message}",
        why.reason
    );
}

#[test]
fn a_box_edge_on_a_centre_holds_it_whatever_the_rounding() {
    // A 0.1-degree global row of 3,600 columns: column 1234 is placed at
    // `1234 * 0.1`, which is 123.39999999999999, one ulp short of 123.4. A box
    // whose edges are that centre's own longitude holds it.
    let n = 3600u32;
    let lon: Vec<f64> = (0..n).map(|i| f64::from(i) / 10.0).collect();
    let objects = vec![
        ("zarr.json", V3_GROUP.to_vec()),
        ("lat/zarr.json", v3_array("[2]", "[2]", "[\"lat\"]")),
        ("lat/c/0", f8(&[0.0, 0.1])),
        (
            "lon/zarr.json",
            v3_array(&format!("[{n}]"), &format!("[{n}]"), "[\"lon\"]"),
        ),
        ("lon/c/0", f8(&lon)),
        (
            "t/zarr.json",
            v3_array(&format!("[2,{n}]"), "[2,100]", "[\"lat\",\"lon\"]"),
        ),
    ];
    let session = Session::open_store(MemoryObjects::from_iter(objects)).expect("opens");
    let t = variable(&session, "t");
    for (bounds, columns) in [
        ([0.0, 0.1, 123.4, 123.5], vec![[1234, 1236]]),
        ([0.0, 0.1, 123.3, 123.4], vec![[1233, 1235]]),
        ([0.1, 0.1, 359.9, 360.0], vec![[3599, 3600], [0, 1]]),
    ] {
        let plan = session
            .plan_window(&WindowRequest::new(t, 0, 1, bounds, vec![0, 0]))
            .expect("plans");
        assert_eq!(plan.columns, columns, "{bounds:?}");
    }
}

#[test]
fn a_message_stream_has_no_windows() {
    let bytes = std::fs::read("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2")
        .expect("a GRIB2 fixture");
    let session = Session::open(bytes).expect("opens");
    let asked = WindowRequest::new(0, 0, 1, [0.0, 1.0, 0.0, 1.0], vec![0, 0]);
    for err in [
        session.plan_window(&asked).expect_err("messages"),
        session
            .decode_window(&asked, &DecodeOptions::default())
            .expect_err("messages"),
        session.coordinate_chunks(0).expect_err("messages"),
    ] {
        assert!(matches!(err, Error::WrongAddressing { .. }), "{err}");
    }
}
