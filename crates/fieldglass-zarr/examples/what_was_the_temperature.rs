//! What was the temperature at one place?
//!
//! Finds the grid cell nearest a latitude and longitude and reads the one
//! value stored there. It also shows the difference between the number as it
//! is stored and the real temperature it stands for.
//!
//!     cargo run -p fieldglass-zarr --example what_was_the_temperature
//!
//! To ask about your own store, change the names and the place below.

use std::{error::Error, fs, path::Path};

use fieldglass_core::bytes::MemoryObjects;
use fieldglass_zarr::{ArraySource, ZarrStore};

// The store, the array to read, the names of its coordinate arrays, and the
// place to look. Change these to ask your own question.
const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/stores/cf_v3");
const VALUES: &str = "t";
const LATITUDES: &str = "lat";
const LONGITUDES: &str = "lon";
const WANT_LAT: f64 = 12.0;
const WANT_LON: f64 = 1.0;

fn main() -> Result<(), Box<dyn Error>> {
    let store = ZarrStore::open(MemoryObjects::from_iter(read_folder(Path::new(STORE))?))?;

    // Read the whole latitude and longitude axes, then find the closest point.
    let row = nearest(&read_all(&store, LATITUDES)?, WANT_LAT);
    let col = nearest(&read_all(&store, LONGITUDES)?, WANT_LON);

    // Read just that one cell: one row, one column.
    let cell = [row..row + 1, col..col + 1];
    let stored = store.read_region(VALUES, &cell)?[0];
    let real = store.read_region_physical(VALUES, &cell)?[0];

    println!("Nearest grid point to {WANT_LAT}°N, {WANT_LON}°E: row {row}, column {col}");
    println!("Stored as:   {}", show(stored));
    println!("Real value:  {}", show(real));
    println!("(The store packs each value into a small integer; reading it in");
    println!(" physical units applies the scale and offset saved with the array.)");
    Ok(())
}

/// Every value of a one-dimensional array, such as a list of latitudes.
fn read_all(store: &ZarrStore<MemoryObjects>, name: &str) -> Result<Vec<f64>, Box<dyn Error>> {
    let length = store
        .array(name)
        .and_then(|a| a.chunk_grid.as_ref())
        .map_or(0, |g| g.shape()[0]);
    // One range per axis; this array has a single axis.
    #[allow(clippy::single_range_in_vec_init)]
    let values = store.read_region_physical(name, &[0..length])?;
    Ok(values.into_iter().map(|v| v.unwrap_or(f64::NAN)).collect())
}

/// The position of the value closest to `want`.
fn nearest(axis: &[f64], want: f64) -> u64 {
    let distance = |i: &usize| (axis[*i] - want).abs();
    (0..axis.len())
        .min_by(|a, b| distance(a).total_cmp(&distance(b)))
        .unwrap_or(0) as u64
}

/// A value, or "no data" for a missing one.
fn show(value: Option<f64>) -> String {
    value.map_or("no data".into(), |v| v.to_string())
}

/// Every file in a folder and its subfolders, named by its path inside the folder.
fn read_folder(root: &Path) -> std::io::Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let path = entry?.path();
            if path.is_dir() {
                folders.push(path);
            } else if let Ok(name) = path.strip_prefix(root) {
                files.push((name.to_string_lossy().replace('\\', "/"), fs::read(&path)?));
            }
        }
    }
    Ok(files)
}
