//! See an array in your terminal.
//!
//! Draws a two-dimensional array as shaded blocks, light for low values and
//! dark for high ones, with the numbers beside it. No plotting library needed.
//!
//!     cargo run -p fieldglass-zarr --example see_it
//!
//! To see your own data, change the store and array names below.

use std::{error::Error, fs, path::Path};

use fieldglass_core::bytes::MemoryObjects;
use fieldglass_zarr::{ArraySource, ZarrStore};

// The store and the two-dimensional array to draw.
const STORE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/stores/cf_v3");
const ARRAY: &str = "t";

fn main() -> Result<(), Box<dyn Error>> {
    let store = ZarrStore::open(MemoryObjects::from_iter(read_folder(Path::new(STORE))?))?;
    let shape = store
        .array(ARRAY)
        .and_then(|a| a.chunk_grid.as_ref())
        .map(|g| g.shape().to_vec())
        .ok_or("no such two-dimensional array")?;
    let (rows, cols) = (shape[0], shape[1]);

    // Read every value, in real units.
    let values = store.read_region_physical(ARRAY, &[0..rows, 0..cols])?;
    let present: Vec<f64> = values.iter().flatten().copied().collect();
    let low = present.iter().copied().fold(f64::INFINITY, f64::min);
    let high = present.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    // Four shades from low to high; a dot where there is no data.
    let shades = ['░', '▒', '▓', '█'];
    println!("{ARRAY}: {rows} rows × {cols} columns, from {low} to {high}");
    for row in values.chunks(cols as usize) {
        let mut picture = String::new();
        let mut numbers = Vec::new();
        for value in row {
            match value {
                Some(v) => {
                    let step = if high > low {
                        (v - low) / (high - low)
                    } else {
                        0.0
                    };
                    let shade = shades[((step * 3.0).round() as usize).min(3)];
                    picture.push(shade);
                    picture.push(shade);
                    numbers.push(format!("{v:>7.2}"));
                }
                None => {
                    picture.push_str("··");
                    numbers.push("     --".into());
                }
            }
        }
        println!("  {picture}   {}", numbers.join(" "));
    }
    Ok(())
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
