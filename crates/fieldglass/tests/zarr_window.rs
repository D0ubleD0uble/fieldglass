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
//! Paths are relative for the `wasm32-wasip1` run's sandbox, which preopens the
//! package directory and its parent.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;

use fieldglass::{
    ChunkRef, DecodeOptions, Error, FieldglassError, MemoryObjects, ObjectSource, Session,
    WindowRequest,
};
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

/// The objects of `store` a host holds: its metadata, plus these keys.
fn holding(all: &[(String, Vec<u8>)], keys: &BTreeSet<String>) -> MemoryObjects {
    MemoryObjects::from_iter(
        all.iter()
            .filter(|(k, _)| is_metadata(k) || keys.contains(k))
            .cloned(),
    )
}

/// The recording store, kept reachable after the session has taken it, as in
/// `placement_is_memoised.rs`.
struct Shared(Rc<Recording<MemoryObjects>>);

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
    for store in ["window_v2", "window_v3"] {
        let case = &oracle[store];
        let all = objects(store);

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
    for store in ["window_v2", "window_v3"] {
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
