//! The generated corpus: `corpus/generate.py`'s output directory, read back.

use std::path::{Path, PathBuf};

use fieldglass_core::bytes::MemoryObjects;
use serde_json::Value;

/// A generated corpus directory and its `corpus.json`.
#[derive(Debug)]
pub struct Corpus {
    dir: PathBuf,
    doc: Value,
}

impl Corpus {
    /// Read the corpus at `dir`.
    ///
    /// # Errors
    ///
    /// When `dir` holds no `corpus.json` or it does not parse. A missing corpus
    /// is an error rather than an empty catalogue: a harness that measured
    /// nothing must not report that nothing regressed.
    pub fn open(dir: &Path) -> Result<Self, String> {
        let index = dir.join("corpus.json");
        let text = std::fs::read_to_string(&index).map_err(|e| {
            format!(
                "no corpus at {}: {e}\n  generate one with: python3 crates/fieldglass-perf/corpus/generate.py {}",
                index.display(),
                dir.display()
            )
        })?;
        let doc: Value =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", index.display()))?;
        if doc["inputs"]
            .as_object()
            .is_none_or(|inputs| inputs.is_empty())
        {
            return Err(format!("{}: the corpus lists no inputs", index.display()));
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            doc,
        })
    }

    /// The directory named by `FIELDGLASS_PERF_CORPUS`, opened.
    ///
    /// # Errors
    ///
    /// When the variable is unset or the corpus does not open.
    pub fn from_env() -> Result<Self, String> {
        let dir = std::env::var_os("FIELDGLASS_PERF_CORPUS")
            .ok_or("FIELDGLASS_PERF_CORPUS is not set; run crates/fieldglass-perf/run.sh")?;
        Self::open(Path::new(&dir))
    }

    /// The SHA-256 over every generated file, as the generator printed it.
    pub fn digest(&self) -> &str {
        self.doc["digest"].as_str().unwrap_or_default()
    }

    /// Every input, in name order.
    pub fn input_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.doc["inputs"]
            .as_object()
            .map(|inputs| inputs.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    fn input(&self, name: &str) -> &Value {
        let input = &self.doc["inputs"][name];
        assert!(input.is_object(), "the corpus has no input {name:?}");
        input
    }

    fn uint(&self, name: &str, key: &str) -> u64 {
        self.input(name)[key]
            .as_u64()
            .unwrap_or_else(|| panic!("corpus input {name:?} has no integer {key:?}"))
    }

    fn usize(&self, name: &str, key: &str) -> usize {
        usize::try_from(self.uint(name, key)).expect("a corpus fact fits usize")
    }

    /// `grib1`, `grib2`, `netcdf` or `zarr`.
    pub fn format(&self, name: &str) -> &str {
        self.input(name)["format"].as_str().unwrap_or_default()
    }

    /// The array variable a slice decodes.
    pub fn variable(&self, name: &str) -> &str {
        self.input(name)["variable"].as_str().unwrap_or_default()
    }

    /// Cells in one field or plane.
    pub fn cells(&self, name: &str) -> usize {
        self.usize(name, "ni") * self.usize(name, "nj")
    }

    /// Time steps in an array variable; 1 for a message.
    pub fn planes(&self, name: &str) -> u64 {
        self.input(name)["nt"].as_u64().unwrap_or(1)
    }

    /// A single-file input's bytes.
    pub fn bytes(&self, name: &str) -> Vec<u8> {
        let path = self
            .dir
            .join(self.input(name)["file"].as_str().unwrap_or_default());
        std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    /// A store input's objects, keyed as the store names them.
    pub fn objects(&self, name: &str) -> MemoryObjects {
        let root = self
            .dir
            .join(self.input(name)["dir"].as_str().unwrap_or_default());
        let mut objects = MemoryObjects::new();
        for (key, _) in self.object_sizes(name) {
            let path = root.join(&key);
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            objects.insert(key, bytes);
        }
        objects
    }

    /// Every object in a store input and its size, as the generator listed them.
    pub fn object_sizes(&self, name: &str) -> Vec<(String, u64)> {
        self.input(name)["objects"]
            .as_object()
            .map(|objects| {
                objects
                    .iter()
                    .map(|(key, size)| (key.clone(), size.as_u64().unwrap_or_default()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Size of a single-file input, or the sum of a store's objects.
    pub fn total_bytes(&self, name: &str) -> u64 {
        match self.input(name)["file"].as_str() {
            Some(file) => std::fs::metadata(self.dir.join(file)).map_or(0, |m| m.len()),
            None => self.object_sizes(name).iter().map(|(_, size)| size).sum(),
        }
    }

    /// A message's length and its data section, `(offset, length)`.
    pub fn data_section(&self, name: &str) -> (usize, usize) {
        (
            self.usize(name, "data_offset"),
            self.usize(name, "data_length"),
        )
    }

    /// A GRIB message's total length.
    pub fn message_length(&self, name: &str) -> u64 {
        self.uint(name, "message_length")
    }

    /// The byte extents holding `plane` of a single-file array input.
    pub fn plane_extents(&self, name: &str, plane: u32) -> Vec<(u64, u64)> {
        self.input(name)["planes"][plane as usize]
            .as_array()
            .map(|extents| {
                extents
                    .iter()
                    .map(|e| (e[0].as_u64().unwrap_or(0), e[1].as_u64().unwrap_or(0)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The single extent of `plane` of a one-chunk-per-plane file.
    pub fn plane_extent(&self, name: &str, plane: u32) -> (usize, usize) {
        let extents = self.plane_extents(name, plane);
        assert_eq!(extents.len(), 1, "{name} plane {plane} is not one extent");
        let (offset, length) = extents[0];
        (offset as usize, length as usize)
    }

    /// The object keys holding `plane` of a store input.
    pub fn plane_keys(&self, name: &str, plane: u32) -> Vec<String> {
        self.input(name)["planes"][plane as usize]
            .as_array()
            .map(|keys| {
                keys.iter()
                    .filter_map(|k| k.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The bytes of the one object holding `plane`.
    pub fn plane_object(&self, name: &str, plane: u32) -> Vec<u8> {
        let keys = self.plane_keys(name, plane);
        assert_eq!(keys.len(), 1, "{name} plane {plane} is not one object");
        let root = self
            .dir
            .join(self.input(name)["dir"].as_str().unwrap_or_default());
        std::fs::read(root.join(&keys[0])).expect("a listed object is readable")
    }

    /// `[bits per sample, block size, reference sample interval, GRIB2 flags]`.
    pub fn aec(&self, name: &str) -> [u32; 4] {
        let aec = &self.input(name)["aec"];
        ["bits_per_sample", "block_size", "rsi", "flags"].map(|key| {
            aec[key]
                .as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or_else(|| panic!("{name} has no AEC parameter {key}"))
        })
    }
}
