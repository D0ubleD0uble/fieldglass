//! A coordinate chunk the store never wrote is its fill value, and the
//! variable list says so (#1305).
//!
//! The Zarr spec reads a chunk the store holds no object for as the array's
//! `fill_value`, and zarr-python leaves a chunk that is all fill unwritten on
//! purpose. So a longitude chunk with fill 0 is placed at 0°, which is right
//! when the coordinate is 0 there and a stripe at the prime meridian when it
//! is not. Placement can't tell those apart, so it places the chunk, and the
//! list of variables names it beside the variable, so the fill is never
//! silent.
//!
//! Every case goes through `Session`, the path both hosts take: a store built
//! here in memory, and the committed zarr-python store whose `latitude/c/2`
//! holds only the equator and was never written. Paths are relative for the
//! `wasm32-wasip1` run's sandbox.

use std::path::Path;

use fieldglass::{
    DecodeOptions, MemoryObjects, PrefetchedObjects, Session, VariableInfo, WindowRequest,
};

const V3_GROUP: &[u8] = b"{\"zarr_format\":3,\"node_type\":\"group\",\"attributes\":{}}";
const STORES: &str = "../fieldglass-zarr/tests/fixtures/windows";

/// A float64 v3 array chunked by `chunks`, with fill value `fill` (JSON).
fn v3(shape: &str, chunks: &str, fill: &str, dims: &str, attrs: &str) -> Vec<u8> {
    format!(
        "{{\"zarr_format\":3,\"node_type\":\"array\",\"shape\":{shape},\"data_type\":\"float64\",\
         \"chunk_grid\":{{\"name\":\"regular\",\"configuration\":{{\"chunk_shape\":{chunks}}}}},\
         \"chunk_key_encoding\":{{\"name\":\"default\",\"configuration\":{{\"separator\":\"/\"}}}},\
         \"fill_value\":{fill},\"codecs\":[{{\"name\":\"bytes\",\"configuration\":{{\"endian\":\"little\"}}}}],\
         \"dimension_names\":{dims},\"attributes\":{attrs}}}"
    )
    .into_bytes()
}

fn f8(values: &[f64]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// `t(lat, lon)`, 3 × 4, over the latitudes 10, 20, 30 and the longitudes
/// 0, 90, 180, 270, with `lon` stored one value to a chunk under fill 0.
/// `lon/c/0`, which holds 0, is written only when `written`.
fn store(written: bool) -> Session {
    let mut objects = vec![
        ("zarr.json".to_string(), V3_GROUP.to_vec()),
        (
            "lat/zarr.json".to_string(),
            v3(
                "[3]",
                "[3]",
                "\"NaN\"",
                "[\"lat\"]",
                "{\"units\":\"degrees_north\"}",
            ),
        ),
        ("lat/c/0".to_string(), f8(&[10.0, 20.0, 30.0])),
        (
            "lon/zarr.json".to_string(),
            v3(
                "[4]",
                "[1]",
                "0.0",
                "[\"lon\"]",
                "{\"units\":\"degrees_east\"}",
            ),
        ),
        ("lon/c/1".to_string(), f8(&[90.0])),
        ("lon/c/2".to_string(), f8(&[180.0])),
        ("lon/c/3".to_string(), f8(&[270.0])),
        (
            "t/zarr.json".to_string(),
            v3("[3,4]", "[3,4]", "\"NaN\"", "[\"lat\",\"lon\"]", "{}"),
        ),
        (
            "t/c/0/0".to_string(),
            f8(&(0..12).map(f64::from).collect::<Vec<_>>()),
        ),
    ];
    if written {
        objects.push(("lon/c/0".to_string(), f8(&[0.0])));
    }
    Session::open_store(MemoryObjects::from_iter(objects)).expect("opens")
}

fn variable<'v>(listed: &'v [VariableInfo], name: &str) -> &'v VariableInfo {
    listed
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("lists {name}"))
}

#[test]
fn an_unwritten_longitude_chunk_of_fill_zero_places_at_zero_and_is_named() {
    let session = store(false);
    // Placed at 0°, as the Zarr spec reads the chunk: the first column of
    // the grid is the prime meridian.
    let field = session
        .decode_slice(0, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("places");
    assert_eq!(field.georef.kind, "latlon", "{:?}", field.georef);
    assert_eq!(
        field.georef.corners,
        Some([10.0, 0.0, 30.0, 270.0]),
        "{:?}",
        field.georef
    );
    let plan = session
        .plan_window(&WindowRequest::new(
            0,
            0,
            1,
            [-90.0, 90.0, -1.0, 1.0],
            vec![0, 0],
        ))
        .expect("plans");
    assert_eq!(plan.columns.first().map(|c| c[0]), Some(0), "{plan:?}");

    // And the list says so beside the variable.
    let listed = session.variables();
    let t = variable(&listed, "t");
    assert_eq!(
        t.coordinates_not_written.as_deref(),
        Some(r#"the store holds no "lon/c/0", so those values of "lon" are its fill value, 0"#),
    );
    // On the wire, as every host hands it on.
    let wire = serde_json::to_value(t).expect("serializes");
    assert_eq!(
        wire["coordinatesNotWritten"],
        serde_json::json!(t.coordinates_not_written),
    );
}

#[test]
fn a_store_that_wrote_every_coordinate_chunk_has_no_note() {
    let session = store(true);
    let listed = session.variables();
    let t = variable(&listed, "t");
    assert_eq!(t.coordinates_not_written, None, "{t:?}");
    let wire = serde_json::to_value(t).expect("serializes");
    assert!(wire["coordinatesNotWritten"].is_null(), "{wire}");
}

/// Every object of a committed window store, by its key from its root.
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
    let root = format!("{STORES}/{store}");
    let root = Path::new(&root);
    let mut out = Vec::new();
    walk(root, root, &mut out);
    assert!(!out.is_empty(), "{store}: no objects");
    out
}

#[test]
fn a_zarr_python_store_names_the_latitude_chunk_it_never_wrote() {
    // zarr-python skipped `latitude/c/2` of this store: it holds only the
    // equator, the array's fill value (0.0).
    let all = objects("window_s2n_v3");
    assert!(
        !all.iter().any(|(k, _)| k == "latitude/c/2"),
        "the fixture leaves the chunk unwritten"
    );
    let note =
        r#"the store holds no "latitude/c/2", so those values of "latitude" are its fill value, 0"#;

    // The whole store, as a folder on disk is read.
    let whole = Session::open_store(MemoryObjects::from_iter(all.clone())).expect("opens");
    let listed = whole.variables();
    let t2m = variable(&listed, "t2m");
    assert_eq!(t2m.coordinates_not_written.as_deref(), Some(note));
    // Placed: the equator is a real latitude here.
    let field = whole
        .decode_slice(t2m.index, 1, 2, &[0, 0, 0], &DecodeOptions::default())
        .expect("places");
    assert_eq!(field.georef.kind, "latlon", "{:?}", field.georef);

    // A browser host's map, before it fetched the coordinate chunks: none
    // is known to be unwritten, so none is named.
    let metadata = || {
        let mut held = PrefetchedObjects::new();
        for (key, bytes) in &all {
            if key.ends_with("zarr.json") {
                held.insert(key.clone(), bytes.clone());
            }
        }
        held
    };
    let early = Session::open_store(metadata()).expect("opens");
    assert_eq!(
        variable(&early.variables(), "t2m").coordinates_not_written,
        None
    );

    // After it fetched them, with the 404 given as absent: named.
    let mut held = metadata();
    for chunk in early
        .coordinate_chunks(t2m.index)
        .expect("chunks")
        .expect("a store")
    {
        match all.iter().find(|(k, _)| *k == chunk.key) {
            Some((key, bytes)) => held.insert(key.clone(), bytes.clone()),
            None => held.insert_absent(chunk.key.clone()),
        }
    }
    let fetched = Session::open_store(held).expect("opens");
    assert_eq!(
        variable(&fetched.variables(), "t2m")
            .coordinates_not_written
            .as_deref(),
        Some(note)
    );

    // The north-to-south store wrote every chunk.
    let written =
        Session::open_store(MemoryObjects::from_iter(objects("window_v3"))).expect("opens");
    assert_eq!(
        variable(&written.variables(), "t2m").coordinates_not_written,
        None
    );
}
