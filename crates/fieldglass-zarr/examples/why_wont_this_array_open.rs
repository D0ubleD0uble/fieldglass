//! Why won't this array open?
//!
//! Opens a store that holds a mix of arrays this crate can and cannot read,
//! and says what happened to each one. Nothing is ever misread: an array is
//! either read correctly or refused with a reason.
//!
//!     cargo run -p fieldglass-zarr --example why_wont_this_array_open
//!
//! To check your own store, pass its folder:
//!
//!     cargo run -p fieldglass-zarr --example why_wont_this_array_open -- path/to/my.zarr

use std::{error::Error, fs, path::Path};

use fieldglass_core::bytes::MemoryObjects;
use fieldglass_zarr::{ArraySource, ZarrStore};

const TEST_STORE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/stores/v2_problems"
);

fn main() -> Result<(), Box<dyn Error>> {
    let folder = std::env::args().nth(1).unwrap_or(TEST_STORE.into());
    let store = ZarrStore::open(MemoryObjects::from_iter(read_folder(Path::new(&folder))?))?;

    // Arrays listed by the store: try to read each one in full.
    for (name, array) in store.group().arrays_qualified() {
        let whole: Vec<_> = array
            .chunk_grid
            .as_ref()
            .map(|grid| grid.shape().iter().map(|&len| 0..len).collect())
            .unwrap_or_default();
        match store.read_region(&name, &whole) {
            Ok(values) => println!("{name}: read, {} values", values.len()),
            Err(error) => println!("{name}: listed, but reading it failed: {error}"),
        }
    }

    // Arrays the store holds but leaves out of the list, and why.
    for left_out in store.left_out() {
        println!("{}: left out: {}", left_out.name, left_out.reason);
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
