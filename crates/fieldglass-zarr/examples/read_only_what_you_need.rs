//! Read only what you need.
//!
//! Zarr splits a big array into many small pieces so a program can read part
//! of it without downloading the rest. This example reads a small corner of an
//! array and counts how much of the store it actually touched. It then lists
//! the exact files you would fetch to read that corner from a web server or a
//! cloud bucket.
//!
//!     cargo run -p fieldglass-zarr --example read_only_what_you_need

use std::{borrow::Cow, cell::Cell, error::Error, fs, path::Path};

use fieldglass_core::bytes::MemoryObjects;
use fieldglass_zarr::{ArraySource, ByteRange, FieldglassError, ObjectSource, ZarrStore};

// A 5 × 7 array split into shards, each holding a few smaller chunks.
const STORE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/stores/v3_sharded"
);
const ARRAY: &str = "s";

fn main() -> Result<(), Box<dyn Error>> {
    let files = read_folder(Path::new(STORE))?;
    let total_bytes: usize = files.iter().map(|(_, bytes)| bytes.len()).sum();
    let total_files = files.len();

    let counter = Counting::new(MemoryObjects::from_iter(files));
    let store = ZarrStore::open(&counter)?;
    counter.reset(); // count only the read below, not opening the store

    // The top-left corner: rows 0 and 1, columns 0 and 1.
    let corner = [0..2, 0..2];
    let values = store.read_region(ARRAY, &corner)?;
    println!(
        "Read {} values from the top-left corner of {ARRAY}.",
        values.len()
    );
    println!(
        "That needed {} read(s) and {} bytes, out of {total_files} files and {total_bytes} bytes in the store.",
        counter.reads.get(),
        counter.bytes.get(),
    );

    // The files a program would fetch over the network to read that corner.
    println!("To read it from a server or bucket, fetch:");
    for (_, key) in store.chunk_keys(ARRAY, &corner)? {
        println!("  {key}");
    }
    Ok(())
}

/// A store that counts how much is read from it. You don't need to follow
/// this part to use the crate: it wraps the real store and keeps a tally.
struct Counting {
    inner: MemoryObjects,
    reads: Cell<usize>,
    bytes: Cell<usize>,
}

impl Counting {
    fn new(inner: MemoryObjects) -> Self {
        Self {
            inner,
            reads: Cell::new(0),
            bytes: Cell::new(0),
        }
    }

    fn reset(&self) {
        self.reads.set(0);
        self.bytes.set(0);
    }

    fn tally(&self, found: &Option<Cow<'_, [u8]>>) {
        self.reads.set(self.reads.get() + 1);
        self.bytes
            .set(self.bytes.get() + found.as_ref().map_or(0, |b| b.len()));
    }
}

impl ObjectSource for Counting {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        let found = self.inner.get(key)?;
        self.tally(&found);
        Ok(found)
    }

    fn get_range(
        &self,
        key: &str,
        range: ByteRange,
    ) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        let found = self.inner.get_range(key, range)?;
        self.tally(&found);
        Ok(found)
    }

    fn object_size(&self, key: &str) -> Result<Option<u64>, FieldglassError> {
        self.inner.object_size(key)
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        self.inner.list(prefix)
    }
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
