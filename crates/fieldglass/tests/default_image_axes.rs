//! The axes a variable with no detected latitude and longitude opens on
//! (#1397): the last two dimensions, skipping a detected time axis, rather
//! than the first two. The answer is `VariableInfo::default_y_dim` and
//! `default_x_dim`, so the extension and the browser host open the same
//! plane. Paths are relative for the `wasm32-wasip1` run's sandbox.

#![cfg(all(feature = "netcdf", feature = "zarr"))]

use std::path::Path;

use fieldglass::{Session, VariableInfo};
use fieldglass_core::bytes::MemoryObjects;

const GROUPED: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc");
const ERSST: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc");
const SHARDS: &str = "../fieldglass-zarr/tests/fixtures/transposed_shards";

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

fn named<'a>(vars: &'a [VariableInfo], name: &str) -> &'a VariableInfo {
    vars.iter()
        .find(|v| v.name.trim_start_matches('/') == name)
        .unwrap_or_else(|| panic!("{name} not listed"))
}

/// `cube` and `twice` are 2 × 3 × 4 with unnamed dimensions: they open on the
/// 3 × 4 plane, with the stepper on the first dimension.
#[test]
fn an_unnamed_cube_opens_on_its_last_two_dimensions() {
    let session = Session::open_store(load(SHARDS)).expect("the store opens");
    let vars = session.variables();
    for name in ["cube", "twice"] {
        let v = named(&vars, name);
        let lengths: Vec<u64> = v.dims.iter().map(|d| d.length).collect();
        assert_eq!(lengths, [2, 3, 4], "{name}: the fixture's shape");
        assert_eq!((v.detected_y_dim, v.detected_x_dim), (None, None), "{name}");
        assert_eq!((v.default_y_dim, v.default_x_dim), (1, 2), "{name}");
    }
    // A 2-D array is unchanged: its only two dimensions, in order.
    let square = named(&vars, "square");
    assert_eq!((square.default_y_dim, square.default_x_dim), (0, 1));
}

/// `PRODUCT/qa_value` is time × scanline × ground_pixel: it opens on the
/// swath, not on time × scanline.
#[test]
fn a_swath_opens_on_scanline_by_ground_pixel() {
    let session = Session::open(GROUPED.to_vec()).expect("the file opens");
    let vars = session.variables();
    let qa = named(&vars, "PRODUCT/qa_value");
    let dims: Vec<&str> = qa.dims.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(dims.len(), 3, "{dims:?}");
    assert!(dims[0].ends_with("time"), "{dims:?}");
    assert_eq!((qa.detected_y_dim, qa.detected_x_dim), (None, None));
    assert_eq!((qa.default_y_dim, qa.default_x_dim), (1, 2), "{dims:?}");
}

/// A variable whose axes the file names keeps them: the default is the
/// detected pair, whatever its position. ERSST's `sst` is time × lev × lat ×
/// lon.
#[test]
fn detected_axes_are_the_default() {
    let session = Session::open(ERSST.to_vec()).expect("the file opens");
    let mut checked = 0;
    for v in session.variables() {
        if let (Some(y), Some(x)) = (v.detected_y_dim, v.detected_x_dim) {
            assert_eq!((v.default_y_dim, v.default_x_dim), (y, x), "{}", v.name);
            checked += 1;
        }
    }
    assert!(checked > 0, "the fixture has a variable with detected axes");
}
