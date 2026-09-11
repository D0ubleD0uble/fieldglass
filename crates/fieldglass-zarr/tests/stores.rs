//! Whole Zarr stores, walked and read through `ArraySource`, against the
//! reference implementation (#658).
//!
//! `real_stores.rs` proves a chunk decodes. This proves the store around it:
//! that the walker sees the groups, arrays, dimensions and attributes
//! zarr-python sees, that a region read returns the values zarr-python reads,
//! that the physical read is xarray's, and that the reads it makes to get
//! there are the ones it should. Every expectation comes from
//! `tests/fixtures/stores_oracle.json`, which `tools/build_zarr_fixtures.py`
//! records from zarr-python and xarray — never from this crate.
//!
//! Everything goes through `&dyn ArraySource`, the way a caller that also reads
//! NetCDF will hold a store.
//!
//! Paths are relative for the reason `real_stores.rs` gives: the suite also
//! runs under `wasmtime --dir=. --dir=..`.

// A region over a one-dimensional array is a slice of one range — `&[0..4]` is
// exactly the value meant, not a mistyped `(0..4).collect()`.
#![allow(clippy::single_range_in_vec_init)]

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;

use fieldglass_core::array::{ArraySource, AttributeValue, ElementType, Group, PhonyDimensions};
use fieldglass_core::bytes::{MemoryObjects, ObjectSource};
use fieldglass_core::testing::Recording;
use fieldglass_zarr::ZarrStore;
use serde_json::{Value, json};

/// A fixture directory as the objects a host would hand over: every file,
/// keyed by its path under the store root with `/` separators.
///
/// Wrapped in a recorder, because what several of these tests say is what the
/// walk *asked the store for* and not only what it returned.
fn load(dir: &str) -> Recording<MemoryObjects> {
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
    let root = Path::new(dir);
    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    Recording::new(MemoryObjects::from_iter(entries))
}

fn oracle(name: &str) -> Value {
    let path = format!("tests/fixtures/{name}");
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .expect("oracle is JSON")
}

/// A number as the oracle spells it, NaN and the infinities included.
fn number(value: &Value) -> f64 {
    match value {
        Value::String(s) if s == "NaN" => f64::NAN,
        Value::String(s) if s == "Infinity" => f64::INFINITY,
        Value::String(s) if s == "-Infinity" => f64::NEG_INFINITY,
        other => other
            .as_f64()
            .unwrap_or_else(|| panic!("{other} is not a number")),
    }
}

/// The oracle's spelling of a number, so a model value can be compared as
/// JSON with a NaN in it.
fn spelled(value: f64) -> Value {
    if value.is_nan() {
        json!("NaN")
    } else if value.is_infinite() {
        json!(if value > 0.0 { "Infinity" } else { "-Infinity" })
    } else {
        json!(value)
    }
}

fn cells(list: &Value) -> Vec<Option<f64>> {
    list.as_array()
        .expect("a list of cells")
        .iter()
        .map(|v| (!v.is_null()).then(|| number(v)))
        .collect()
}

fn assert_cells(got: &[Option<f64>], want: &[Option<f64>], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let same = match (g, w) {
            (Some(g), Some(w)) => g == w || (g.is_nan() && w.is_nan()),
            (None, None) => true,
            _ => false,
        };
        assert!(same, "{what}: cell {i} is {g:?}, expected {w:?}");
    }
}

fn attribute_json(value: &AttributeValue) -> Value {
    match value {
        AttributeValue::Numbers(values) => {
            json!({ "numbers": values.iter().map(|v| spelled(*v)).collect::<Vec<_>>() })
        }
        AttributeValue::Text(text) => json!({ "text": text }),
        AttributeValue::Opaque(text) => json!({ "opaque": text }),
        other => panic!("an attribute kind the oracle does not spell: {other:?}"),
    }
}

fn element_json(element: &ElementType) -> Value {
    match element {
        ElementType::Float(bits) => json!({ "kind": "float", "bits": bits }),
        ElementType::Int(bits) => json!({ "kind": "int", "bits": bits }),
        ElementType::Uint(bits) => json!({ "kind": "uint", "bits": bits }),
        other => json!({ "kind": format!("{other:?}") }),
    }
}

/// Every group in the tree, by the path `arrays_qualified` uses for them.
fn groups_by_path(group: &Group) -> BTreeMap<String, &Group> {
    fn walk<'a>(group: &'a Group, path: String, out: &mut BTreeMap<String, &'a Group>) {
        for child in &group.groups {
            let child_path = if path.is_empty() {
                child.name.clone()
            } else {
                format!("{path}/{}", child.name)
            };
            walk(child, child_path, out);
        }
        out.insert(path, group);
    }
    let mut out = BTreeMap::new();
    walk(group, String::new(), &mut out);
    out
}

fn ranges(list: &Value) -> Vec<Range<u64>> {
    list.as_array()
        .expect("ranges")
        .iter()
        .map(|pair| pair[0].as_u64().unwrap()..pair[1].as_u64().unwrap())
        .collect()
}

fn whole(shape: &Value) -> Vec<Range<u64>> {
    shape
        .as_array()
        .expect("shape")
        .iter()
        .map(|n| 0..n.as_u64().unwrap())
        .collect()
}

/// Each committed store, opened, with its oracle entry.
fn stores() -> Vec<(String, Recording<MemoryObjects>, Value)> {
    let oracle = oracle("stores_oracle.json");
    oracle
        .as_object()
        .expect("an object of stores")
        .iter()
        .map(|(name, want)| {
            (
                name.clone(),
                load(&format!("tests/fixtures/stores/{name}")),
                want.clone(),
            )
        })
        .collect()
}

/// The structure: groups and their attributes, and for every array its
/// element type, dimensions, chunk grid and attributes — and exactly the
/// arrays zarr-python lists, less the ones this crate leaves out and says so.
#[test]
fn every_store_lists_what_zarr_python_lists() {
    let mut arrays_checked = 0;
    for (name, objects, want) in stores() {
        let store = ZarrStore::open(&objects).unwrap_or_else(|e| panic!("{name}: {e}"));
        let source: &dyn ArraySource = &store;

        let groups = groups_by_path(source.group());
        let want_groups = want["groups"].as_object().unwrap();
        assert_eq!(
            groups.keys().cloned().collect::<BTreeSet<_>>(),
            want_groups.keys().cloned().collect::<BTreeSet<_>>(),
            "{name}: groups"
        );
        for (path, group) in &groups {
            let got: serde_json::Map<String, Value> = group
                .attributes
                .iter()
                .map(|a| (a.name.clone(), attribute_json(&a.value)))
                .collect();
            assert_eq!(
                Value::Object(got),
                want_groups[path],
                "{name}: group {path:?} attributes"
            );
        }

        let listed: BTreeMap<String, _> = source.group().arrays_qualified().into_iter().collect();
        let want_arrays = want["arrays"].as_object().unwrap();
        assert_eq!(
            listed.keys().cloned().collect::<BTreeSet<_>>(),
            want_arrays.keys().cloned().collect::<BTreeSet<_>>(),
            "{name}: arrays"
        );
        // The walker names an unnamed axis by netCDF-C's phony rule, one
        // allocator per group, in listing order — replayed here with core's
        // allocator, so the expectation is the rule and not the walker's copy
        // of it.
        let mut phony: BTreeMap<String, PhonyDimensions> = BTreeMap::new();
        for (path, array) in &listed {
            let expected = &want_arrays[path];
            let grid = array.chunk_grid.as_ref().expect("a Zarr array is chunked");
            assert_eq!(
                json!(grid.shape()),
                expected["shape"],
                "{name}/{path}: shape"
            );
            assert_eq!(
                json!(grid.chunk_shape()),
                expected["chunks"],
                "{name}/{path}: chunks"
            );
            assert_eq!(
                element_json(&array.element_type),
                expected["element"],
                "{name}/{path}: element type"
            );
            let stated: Vec<Option<String>> = (0..grid.rank())
                .map(|axis| {
                    expected["dimensions"]
                        .get(axis)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect();
            let unnamed: Vec<u64> = stated
                .iter()
                .zip(grid.shape())
                .filter(|(name, _)| name.is_none())
                .map(|(_, length)| *length)
                .collect();
            let group = path.rsplit_once('/').map_or("", |(g, _)| g).to_string();
            let mut invented = phony
                .entry(group)
                .or_default()
                .axes_for(&unnamed, &[])
                .into_iter();
            let want_dims: Vec<String> = stated
                .into_iter()
                .map(|name| name.or_else(|| invented.next()).unwrap_or_default())
                .collect();
            assert_eq!(array.dimensions, want_dims, "{name}/{path}: dimensions");
            let got: serde_json::Map<String, Value> = array
                .attributes
                .iter()
                .map(|a| (a.name.clone(), attribute_json(&a.value)))
                .collect();
            assert_eq!(
                Value::Object(got),
                expected["attributes"],
                "{name}/{path}: attributes"
            );
            arrays_checked += 1;
        }

        let left_out: BTreeSet<String> = store.problems().iter().map(|(p, _)| p.clone()).collect();
        let want_left_out: BTreeSet<String> = want["left_out"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(left_out, want_left_out, "{name}: arrays left out");
    }
    assert!(
        arrays_checked >= 15,
        "only {arrays_checked} arrays compared"
    );
}

/// The values: every array's full read and a region crossing chunk
/// boundaries, as zarr-python reads them — absent chunks as the fill value,
/// ragged edges trimmed, shards unpacked.
#[test]
fn every_array_reads_what_zarr_python_reads() {
    let mut reads = 0;
    for (name, objects, want) in stores() {
        let store = ZarrStore::open(&objects).unwrap_or_else(|e| panic!("{name}: {e}"));
        let source: &dyn ArraySource = &store;
        let unreadable: BTreeSet<&str> = want["unreadable"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for (path, expected) in want["arrays"].as_object().unwrap() {
            if unreadable.contains(path.as_str()) {
                continue;
            }
            let full = source
                .read_region(path, &whole(&expected["shape"]))
                .unwrap_or_else(|e| panic!("{name}/{path}: {e}"));
            assert_cells(
                &full,
                &cells(&expected["values"]),
                &format!("{name}/{path} full"),
            );
            let region = ranges(&expected["region"]["ranges"]);
            let part = source
                .read_region(path, &region)
                .unwrap_or_else(|e| panic!("{name}/{path} {region:?}: {e}"));
            assert_cells(
                &part,
                &cells(&expected["region"]["values"]),
                &format!("{name}/{path} {region:?}"),
            );
            reads += 2;
        }
    }
    assert!(reads >= 30, "only {reads} reads compared");
}

/// The physical values of xarray's CF encoding, in both editions, equal what
/// xarray decodes — through core's one CF rule and the two presentation rules
/// the walker applies to `_FillValue`.
#[test]
fn physical_reads_match_xarray() {
    let mut compared = 0;
    for (name, objects, want) in stores() {
        let store = ZarrStore::open(&objects).unwrap_or_else(|e| panic!("{name}: {e}"));
        let source: &dyn ArraySource = &store;
        for (path, expected) in want["arrays"].as_object().unwrap() {
            let Some(physical) = expected.get("physical") else {
                continue;
            };
            let got = source
                .read_region_physical(path, &whole(&expected["shape"]))
                .unwrap_or_else(|e| panic!("{name}/{path}: {e}"));
            assert_cells(&got, &cells(physical), &format!("{name}/{path} physical"));
            compared += 1;
        }
    }
    // `t`, `a`, `lat`, `lon`, in each of two editions.
    assert_eq!(compared, 8, "physical arrays compared");
}

/// The region cap is core's field cap, and it is one element wide.
///
/// Three crates used to state this number and two of them disagreed while their
/// doc comments claimed to match, so a field between 67 M and 200 M points was
/// accepted by one reader and refused by another (#707). What is checked here is
/// that this reader's threshold *is* `fieldglass_core::MAX_FIELD_POINTS` — both
/// as a constant and in the number the refusal names, since a caller reads the
/// message and not the constant.
///
/// A region of exactly the cap is not refused, and that follows from the
/// comparison rather than from a second fixture: the guard is `count <= CAP`, so
/// naming `CAP` as the bound it exceeded proves `CAP` itself passes. Running it
/// would mean letting the decode allocate a gigabyte.
#[test]
fn the_region_cap_is_the_core_field_cap() {
    const CAP: u64 = fieldglass_core::MAX_FIELD_POINTS as u64;
    assert_eq!(
        fieldglass_zarr::store::MAX_REGION_ELEMENTS,
        CAP,
        "this crate's cap must be core's, not a second copy of the number"
    );

    let over = CAP + 1;
    // One chunk spanning the whole array, so the chunk-count cap cannot be what
    // refuses this and the element count is what is under test.
    let zarray = format!(
        r#"{{"zarr_format": 2, "shape": [{over}], "chunks": [{over}],
            "dtype": "<f8", "fill_value": 0, "compressor": null, "filters": null,
            "order": "C"}}"#
    );
    let objects = MemoryObjects::from_iter([
        (".zgroup".to_string(), br#"{"zarr_format": 2}"#.to_vec()),
        ("wide/.zarray".to_string(), zarray.into_bytes()),
    ]);
    let store = ZarrStore::open(&objects).expect("describing it costs nothing");
    let source: &dyn ArraySource = &store;

    let err = source
        .read_region("wide", &[0..over])
        .expect_err("one element past the cap");
    let msg = err.to_string();
    assert!(
        msg.contains(&CAP.to_string()),
        "the refusal must name the cap it enforces: {msg}"
    );
    assert!(
        msg.contains(&over.to_string()) || msg.contains("elements"),
        "and what was asked for: {msg}"
    );
}

/// An array whose codec the crate does not decode is listed and fails only
/// when read, naming the codec; the array beside it reads.
#[test]
fn an_undecodable_codec_fails_only_its_array() {
    let objects = load("tests/fixtures/stores/v2_problems");
    let store = ZarrStore::open(&objects).expect("the store opens");
    let source: &dyn ArraySource = &store;
    assert!(source.array("bz").is_some(), "listed");
    let err = source
        .read_region("bz", &[0..4])
        .expect_err("bz2 is refused");
    assert!(err.to_string().contains("bz2"), "{err}");
    assert!(source.read_region("ok", &[0..4]).is_ok());
    // The two left out say why.
    let why: BTreeMap<&str, &str> = store
        .problems()
        .iter()
        .map(|(p, w)| (p.as_str(), w.as_str()))
        .collect();
    assert!(why["x2"].contains("dimension \"x\""), "{}", why["x2"]);
    assert!(!why["names"].is_empty());
    let err = source.read_region("x2", &[0..5]).expect_err("left out");
    assert!(err.to_string().contains("dimension"), "{err}");
}

/// Opening a consolidated store reads its root documents and nothing else, in
/// one prefetch batch; a region read prefetches exactly the chunk keys it
/// covers, once, and reads each of them — absent ones included, since asking is
/// how absence is learned.
#[test]
fn reads_are_what_they_should_be() {
    let root_batch = vec![
        "zarr.json".to_string(),
        ".zmetadata".to_string(),
        ".zgroup".to_string(),
        ".zarray".to_string(),
        ".zattrs".to_string(),
    ];

    let v3 = load("tests/fixtures/stores/v3_nested");
    let store = ZarrStore::open(&v3).expect("opens");
    assert_eq!(v3.key_prefetches(), vec![root_batch.clone()]);
    assert_eq!(v3.gets(), vec!["zarr.json".to_string()]);

    v3.clear();
    let source: &dyn ArraySource = &store;
    // Rows 1..3 and columns 2..5 of a 2x3-chunked 5x7 array touch chunks
    // (0,0), (0,1), (1,0) and (1,1).
    source.read_region("temp", &[1..3, 2..5]).expect("reads");
    let keys: Vec<String> = ["c/0/0", "c/0/1", "c/1/0", "c/1/1"]
        .iter()
        .map(|k| format!("temp/{k}"))
        .collect();
    assert_eq!(v3.key_prefetches(), vec![keys.clone()]);
    assert_eq!(v3.gets(), keys);

    let v2 = load("tests/fixtures/stores/v2_nested");
    ZarrStore::open(&v2).expect("opens");
    assert_eq!(v2.key_prefetches(), vec![root_batch]);
    assert_eq!(
        v2.gets(),
        vec!["zarr.json".to_string(), ".zmetadata".to_string()]
    );

    // Without consolidated metadata the store is descended, one batch of
    // metadata documents per level — not one request per document, and not a
    // listing of the whole store.
    let listed = load("tests/fixtures/stores/v2_slash");
    ZarrStore::open(&listed).expect("opens");
    let batches = listed.key_prefetches();
    assert_eq!(
        batches.len(),
        4,
        "the root batch, then one per level of the hierarchy: {batches:?}"
    );
    assert!(
        batches[1..]
            .iter()
            .flatten()
            .all(|k| V2_DOCS.iter().any(|doc| k.ends_with(doc))),
        "every batch after the root holds only metadata documents: {batches:?}"
    );
}

/// The v2 metadata documents, which is all an unconsolidated walk ever fetches.
const V2_DOCS: [&str; 3] = [".zgroup", ".zarray", ".zattrs"];

/// An unconsolidated walk never lists a chunk key.
///
/// The property that decides whether opening a store on a bucket is a handful
/// of requests or a paged walk of every chunk in it. `list("")` — what this
/// walk used to do — names every chunk of every array; descending through
/// `list_children` names a directory once and never looks inside one that
/// turned out to be an array.
///
/// `v2_slash` is the fixture for it because it has both shapes of chunk key:
/// `temp/` uses `/` as its dimension separator, so its chunks are two levels
/// down, and `sub/inner/` stores its chunks directly beside its own documents.
/// A walk that listed either directory would name them.
#[test]
fn an_unconsolidated_walk_lists_no_chunk_key() {
    let listed = load("tests/fixtures/stores/v2_slash");
    let store = ZarrStore::open(&listed).expect("opens");
    // The fixture really does hold chunks, or this proves nothing.
    let chunks: Vec<String> = listed
        .inner()
        .list("")
        .expect("list")
        .into_iter()
        .filter(|k| !V2_DOCS.iter().any(|doc| k.ends_with(doc)))
        .collect();
    assert_eq!(chunks.len(), 6, "{chunks:?}");

    for key in listed.listed_keys() {
        assert!(
            key.ends_with('/') || V2_DOCS.iter().any(|doc| key.ends_with(doc)),
            "the walk listed {key:?}, which is neither a directory nor a metadata document"
        );
    }
    // It descended: the root, and the one group under it. Not the two arrays.
    assert_eq!(
        listed
            .listings()
            .iter()
            .map(|(prefix, _)| prefix.clone())
            .collect::<Vec<_>>(),
        ["", "sub/"]
    );

    // And it found the whole hierarchy anyway.
    let source: &dyn ArraySource = &store;
    let mut names: Vec<String> = source
        .group()
        .arrays_qualified()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    names.sort();
    assert_eq!(names, ["sub/inner", "temp"]);
}

/// Every non-sharded store of the codec corpus (#657) — one root array per
/// store — reads through the walker to the array it was written from. The
/// codec tests prove each chunk decodes; this proves the walk finds them,
/// spells their keys and assembles them, over every codec.
#[test]
fn every_codec_store_reads_through_the_walker() {
    let oracle = oracle("oracle.json");
    let mut compared = 0;
    for (case, entry) in oracle.as_object().unwrap() {
        if entry["sharded"].as_bool().unwrap_or(false) {
            continue;
        }
        let objects = load(&format!("tests/fixtures/{case}"));
        let store = ZarrStore::open(&objects).unwrap_or_else(|e| panic!("{case}: {e}"));
        let source: &dyn ArraySource = &store;
        let (name, array) = source
            .group()
            .arrays_qualified()
            .into_iter()
            .next()
            .expect("one array");
        let grid = array.chunk_grid.clone().expect("chunked");
        let (shape, chunk) = (grid.shape().to_vec(), grid.chunk_shape().to_vec());

        // The expected array, assembled from the oracle's chunk records by a
        // loop of this test's own rather than the walker's copy.
        let total: u64 = shape.iter().product();
        let mut expected = vec![None; total as usize];
        for record in entry["chunks"].as_array().unwrap() {
            let index: Vec<u64> = record["index"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap())
                .collect();
            let values: Vec<f64> = if let Some(ramp) = record.get("ramp") {
                let (start, step) = (number(&ramp["start"]), number(&ramp["step"]));
                (0..ramp["count"].as_u64().unwrap())
                    .map(|i| start + step * i as f64)
                    .collect()
            } else {
                record["values"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(number)
                    .collect()
            };
            for (local, value) in values.iter().enumerate() {
                let mut rest = local as u64;
                let mut point = vec![0u64; chunk.len()];
                for axis in (0..chunk.len()).rev() {
                    point[axis] = index[axis] * chunk[axis] + rest % chunk[axis];
                    rest /= chunk[axis];
                }
                if point.iter().zip(&shape).all(|(p, n)| p < n) {
                    let flat = point
                        .iter()
                        .zip(&shape)
                        .fold(0u64, |acc, (p, n)| acc * n + p);
                    expected[flat as usize] = Some(*value);
                }
            }
        }

        let region: Vec<Range<u64>> = shape.iter().map(|&n| 0..n).collect();
        let got = source
            .read_region(&name, &region)
            .unwrap_or_else(|e| panic!("{case}: {e}"));
        assert_cells(&got, &expected, case);
        compared += 1;
    }
    assert!(compared >= 30, "only {compared} codec stores read");
}

/// An array's shape and chunk shape are numbers out of a document somebody
/// else wrote. Opening a store that declares a trillion elements allocates
/// nothing, and reading it — a whole array, or one cell of a chunk that large —
/// is refused by the cap rather than attempted.
#[test]
fn a_hostile_shape_is_refused_rather_than_allocated() {
    let objects = MemoryObjects::from_iter([
        (".zgroup", br#"{"zarr_format": 2}"#.to_vec()),
        (
            "huge/.zarray",
            br#"{"zarr_format": 2, "shape": [1000000, 1000000], "chunks": [1000000, 1000000],
                "dtype": "<f8", "fill_value": 0, "compressor": null, "filters": null,
                "order": "C"}"#
                .to_vec(),
        ),
        ("huge/0.0", vec![0u8; 16]),
    ]);
    let store = ZarrStore::open(&objects).expect("describing it costs nothing");
    let source: &dyn ArraySource = &store;
    assert!(source.array("huge").is_some());

    let err = source
        .read_region("huge", &[0..1_000_000, 0..1_000_000])
        .expect_err("a trillion cells");
    assert!(err.to_string().contains("elements"), "{err}");

    // One cell, but of a chunk no read may decode.
    let err = source
        .read_region("huge", &[0..1, 0..1])
        .expect_err("the chunk is the problem");
    assert!(err.to_string().contains("elements"), "{err}");

    // And a region past the shape is the array model's refusal, not a panic.
    assert!(source.read_region("huge", &[0..1_000_001, 0..1]).is_err());
}
