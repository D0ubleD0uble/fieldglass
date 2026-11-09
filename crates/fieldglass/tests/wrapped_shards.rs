//! A Zarr v3 array whose shards are wrapped in a bytes→bytes codec,
//! `[sharding_indexed, gzip]`, reads through `Session` to the values
//! zarr-python reads (#1303).
//!
//! Before #1303 every such shard was reported as a corrupt gzip stream. The
//! store and its oracle are `tools/build_zarr_fixtures.py
//! --wrapped-shards-only`'s. Paths are relative for the `wasm32-wasip1` run's
//! sandbox.

use std::path::Path;

use fieldglass::{DecodeOptions, Session};
use fieldglass_core::bytes::MemoryObjects;
use serde_json::Value;

const FIXTURES: &str = "../fieldglass-zarr/tests/fixtures";

/// A fixture store directory as the objects a host would hand over.
fn load(dir: &str) -> MemoryObjects {
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
    MemoryObjects::from_iter(entries)
}

#[test]
fn a_gzipped_shard_decodes_through_the_session() {
    let oracle: Value = serde_json::from_str(
        &std::fs::read_to_string(format!("{FIXTURES}/wrapped_shards_oracle.json")).expect("oracle"),
    )
    .expect("JSON");
    let session =
        Session::open_store(load(&format!("{FIXTURES}/wrapped_shards"))).expect("the store opens");
    let vars = session.variables();
    for name in [
        "gzip",
        "crc32c_zstd",
        "transposed",
        "inner_gzip",
        "index_start",
        "plain_index",
    ] {
        let case = &oracle[name];
        let v = vars
            .iter()
            .find(|v| v.name.trim_start_matches('/') == name)
            .unwrap_or_else(|| panic!("{name} not listed"));
        let field = session
            .decode_slice(
                v.index,
                v.default_y_dim,
                v.default_x_dim,
                &[0, 0],
                &DecodeOptions::default(),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let shape: Vec<u64> = case["shape"]
            .as_array()
            .expect("a shape")
            .iter()
            .map(|n| n.as_u64().expect("an extent"))
            .collect();
        assert_eq!(
            (u64::from(field.nj), u64::from(field.ni)),
            (shape[0], shape[1]),
            "{name}"
        );
        let expected: Vec<f64> = case["values"]
            .as_array()
            .expect("values")
            .iter()
            .map(|v| v.as_f64().expect("a number"))
            .collect();
        let values: Vec<f64> = match (field.values.as_f32(), field.values.as_f64()) {
            (Some(v), _) => v.iter().map(|&x| f64::from(x)).collect(),
            (_, Some(v)) => v.to_vec(),
            _ => unreachable!("one precision or the other"),
        };
        assert_eq!(values.len(), expected.len(), "{name}");
        for (cell, &want) in expected.iter().enumerate() {
            // A masked cell can only be the fill value, -1, which is where
            // the store left an inner chunk unwritten; whether a fill reads
            // as missing is the session's rule, not this test's.
            if field.mask[cell] == 0 {
                assert_eq!(want, -1.0, "{name} cell {cell}");
            } else {
                assert_eq!(values[cell], want, "{name} cell {cell}");
            }
        }
        // `gzip` holds an unwritten inner chunk, and the others none.
        let fills = expected.iter().filter(|&&v| v == -1.0).count();
        assert_eq!(fills, if name == "gzip" { 8 } else { 0 }, "{name}");
    }
}
