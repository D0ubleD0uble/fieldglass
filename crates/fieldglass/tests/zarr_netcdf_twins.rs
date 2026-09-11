//! One xarray dataset, written as a Zarr store and as a NetCDF-4 file, reads
//! the same through `Session` (#704).
//!
//! The CF rules that list a container's variables, find its horizontal axes
//! and place a slice on the Earth were NetCDF's alone until #704. They are
//! core's now, over the `ArraySource` each reader presents, and `Session` has
//! one arm for every container of named arrays. The proof that the rules really
//! are shared rather than duplicated is two containers holding the same data
//! answering identically: the same variables, dimensions, types and detected
//! axes, and for every slice the same values, mask, geometry and statistics.
//!
//! The two Zarr stores (one per edition) and the NetCDF twin are written by
//! `tools/build_zarr_fixtures.py` from one dataset; `--twin-only` writes the
//! NetCDF file. Paths are relative for the `wasm32-wasip1` run's sandbox.

use std::path::Path;

use fieldglass::{DecodeOptions, Session, SourceFormat, VariableInfo};
use fieldglass_core::bytes::MemoryObjects;

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

/// A variable's listing, less the index: the two containers list their
/// variables in their own orders, so they are matched by name.
fn listing(v: &VariableInfo) -> String {
    format!(
        "{:?} dtype={} units={:?} y={:?} x={:?}",
        v.dims, v.dtype, v.units, v.detected_y_dim, v.detected_x_dim
    )
}

/// Everything about a decoded slice except the name it carries.
fn slice(session: &Session, v: &VariableInfo) -> String {
    let (y, x) = (
        v.detected_y_dim
            .expect("a lat/lon dataset detects its axes"),
        v.detected_x_dim
            .expect("a lat/lon dataset detects its axes"),
    );
    let zeros = vec![0u32; v.dims.len()];
    let f = session
        .decode_slice(v.index, y, x, &zeros, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("{}: {e}", v.name));
    format!(
        "{:?}",
        (f.values, f.mask, f.ni, f.nj, f.georef, f.stats, f.units)
    )
}

#[test]
fn a_cf_zarr_store_and_its_netcdf_twin_read_the_same() {
    let twin = Session::open(std::fs::read(format!("{FIXTURES}/cf_twin.nc")).expect("twin"))
        .expect("the twin opens");
    assert_eq!(twin.format(), SourceFormat::NetCdf);
    let twin_vars = twin.variables();

    for store in ["cf_v2", "cf_v3"] {
        let zarr = Session::open_store(load(&format!("{FIXTURES}/stores/{store}")))
            .unwrap_or_else(|e| panic!("{store}: {e}"));
        assert_eq!(zarr.format(), SourceFormat::Zarr, "{store}");
        assert_eq!(zarr.addressing(), twin.addressing(), "{store}");
        assert_eq!(zarr.dimensions(), twin.dimensions(), "{store}: dimensions");

        let zarr_vars = zarr.variables();
        let mut names: Vec<&str> = zarr_vars.iter().map(|v| v.name.as_str()).collect();
        let mut twin_names: Vec<&str> = twin_vars.iter().map(|v| v.name.as_str()).collect();
        names.sort_unstable();
        twin_names.sort_unstable();
        assert_eq!(names, twin_names, "{store}: renderable variables");
        assert_eq!(
            names,
            ["a", "t"],
            "{store}: the two data variables, not lat/lon"
        );

        for v in &zarr_vars {
            let w = twin_vars
                .iter()
                .find(|w| w.name == v.name)
                .expect("matched");
            assert_eq!(listing(v), listing(w), "{store}/{}: listing", v.name);
            assert_eq!(
                slice(&zarr, v),
                slice(&twin, w),
                "{store}/{}: the decoded slice",
                v.name
            );
        }
    }
}

/// The slice itself is right, not only the same: the packed `t` decodes to the
/// physical values the dataset was written from, with its NaN masked, on the
/// lat/lon grid its coordinates state.
#[test]
fn the_twins_decode_to_the_dataset_they_were_written_from() {
    let zarr = Session::open_store(load(&format!("{FIXTURES}/stores/cf_v3"))).expect("opens");
    let vars = zarr.variables();
    let t = vars.iter().find(|v| v.name == "t").expect("t");
    let f = zarr
        .decode_slice(t.index, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("decodes");
    assert_eq!((f.ni, f.nj), (4, 3));
    // Row-major, lat 10 first; the third cell of the first row was NaN.
    let expected = [
        250.0, 251.5, 0.0, 253.0, 260.25, 270.0, 280.0, 290.5, 240.0, 241.0, 242.0, 243.0,
    ];
    assert_eq!(f.mask, [1, 1, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
    let got = format!("{:?}", f.values);
    for value in expected.iter().filter(|v| **v != 0.0) {
        assert!(
            got.contains(&format!("{value:?}")),
            "{value} missing from {got}"
        );
    }
    assert_eq!(f.georef.kind, "latlon");
}
