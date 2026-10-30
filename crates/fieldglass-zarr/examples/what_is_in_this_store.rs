//! What is in this Zarr store?
//!
//! Lists every array in a store in plain words: its size, its axes, its units.
//! Start here if you have never opened a Zarr store before.
//!
//!     cargo run -p fieldglass-zarr --example what_is_in_this_store
//!
//! To look at your own store, name its folder in `FIELDGLASS_STORE`:
//!
//!     FIELDGLASS_STORE=path/to/my.zarr cargo run -p fieldglass-zarr --example what_is_in_this_store
//!
//! The path comes from the environment, not the command line: reading
//! `std::env::args()` trips semgrep's `rust.lang.security.args.args`, and this
//! repo keeps zero suppressions (as `bench_decode` and the AEC `oracle` do).

use std::{error::Error, fs, path::Path};

use fieldglass_core::array::{AttributeValue, attribute};
use fieldglass_core::bytes::MemoryObjects;
use fieldglass_zarr::{ArraySource, ZarrStore};

const TEST_STORE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/stores/v3_nested"
);

fn main() -> Result<(), Box<dyn Error>> {
    // The folder to read: yours if you named one, otherwise a small test
    // store committed in the repository.
    let folder = std::env::var("FIELDGLASS_STORE").unwrap_or(TEST_STORE.into());

    // A Zarr store is a folder of small files. Read them all into memory and
    // hand them to the store reader.
    let store = ZarrStore::open(MemoryObjects::from_iter(read_folder(Path::new(&folder))?))?;
    let root = store.group();

    let store_name = Path::new(&folder)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    println!("Store: {store_name}");
    if let Some(AttributeValue::Text(title)) = attribute(&root.attributes, "title") {
        println!("Title: {title}");
    }

    for (name, array) in root.arrays_qualified() {
        // How many values along each axis, e.g. "5 × 7".
        let sizes: Vec<String> = array
            .chunk_grid
            .as_ref()
            .map(|grid| grid.shape().iter().map(u64::to_string).collect())
            .unwrap_or_default();
        let units = match attribute(&array.attributes, "units") {
            Some(AttributeValue::Text(units)) => format!(", in {units}"),
            _ => String::new(),
        };
        println!(
            "  {name}: {} values over {}{units}",
            sizes.join(" × "),
            array.dimensions.join(", "),
        );
    }

    // Arrays the store holds that this crate will not read, and why.
    for left_out in store.left_out() {
        println!("  {} (not read: {})", left_out.name, left_out.reason);
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
