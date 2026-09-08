//! Which chunk holds a region of a Zarr array, and what that chunk's key is
//! called.
//!
//! This is addressing and nothing else. A Zarr array is a grid of chunks, each
//! its own object in a store, and reaching one is two questions: *which* chunk
//! covers the values you want, and *what key* it is stored under. Both are
//! answered by the array's metadata document — its shape, its chunk shape, and
//! the convention it spells keys with — and neither needs a single byte of the
//! chunk itself.
//!
//! Turning those bytes into numbers is [`fieldglass-zarr`]'s question, and the
//! split is deliberate: the codecs are a decoder's concern and the grid
//! arithmetic is a planner's, so a host that only wants to know which object to
//! fetch does not link a decompressor to find out.
//!
//! # The two editions spell a key differently
//!
//! v2 writes the chunk grid index joined by `dimension_separator`, which
//! defaults to `.`; v3 has a `chunk_key_encoding`, whose `default` form
//! prefixes a `c` and joins with `/`, and whose `v2` form is v2's. The
//! zero-dimensional cases are the ones worth writing down, because they are not
//! what the pattern suggests: `default` spells one as `c`, and `v2` spells it
//! `0`.
//!
//! [`ZarrArrayMeta`] carries a worked example.
//!
//! [`fieldglass-zarr`]: https://docs.rs/fieldglass-zarr

use std::ops::Range;

use fieldglass_core::array::{ChunkGrid, ChunkKeyEncoding};

use serde_json::Value;

use crate::error::{Dialect, FetchPlanError};

/// A Zarr array's metadata, read for addressing alone.
///
/// Parsed from a v2 `.zarray` or a v3 `zarr.json`. Everything a *decoder* needs
/// — the data type, the codec chain, the fill value, the memory order — is
/// deliberately not read here, because none of it changes which bytes to fetch.
///
/// ```
/// use fieldglass_fetchplan::ZarrArrayMeta;
///
/// let zarray = r#"{
///     "zarr_format": 2, "shape": [4, 6], "chunks": [2, 3],
///     "dtype": "<f4", "compressor": null, "fill_value": 0.0,
///     "order": "C", "filters": null
/// }"#;
/// let meta = ZarrArrayMeta::from_metadata(zarray)?;
///
/// // Six values across, three to a chunk: the value at row 3, column 4 is in
/// // chunk (1, 1), and that chunk is stored under `1.1`.
/// assert_eq!(meta.chunk_grid(), &[2, 2]);
/// assert_eq!(meta.chunk_containing(&[3, 4])?, vec![1, 1]);
/// assert_eq!(meta.chunk_key(&[1, 1])?, "1.1");
/// # Ok::<(), fieldglass_fetchplan::FetchPlanError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZarrArrayMeta {
    grid: ChunkGrid,
    encoding: ChunkKeyEncoding,
    separator: char,
    zarr_format: u8,
}

impl ZarrArrayMeta {
    /// Read either edition, deciding from the document's own `zarr_format`.
    ///
    /// A caller that has walked a store knows which document it opened and can
    /// use [`from_v2_metadata`](Self::from_v2_metadata) or
    /// [`from_v3_metadata`](Self::from_v3_metadata) directly; a caller holding
    /// an inline value out of a kerchunk reference document, where the key says
    /// `.zarray` but nothing enforces it, is better served by asking the
    /// document what it is.
    pub fn from_metadata(text: &str) -> Result<Self, FetchPlanError> {
        let doc = parse_document(text)?;
        match field(&doc, "zarr_format")?.as_u64() {
            Some(2) => Self::from_v2_value(&doc),
            Some(3) => Self::from_v3_value(&doc),
            _ => Err(FetchPlanError::UnsupportedZarrFormat {
                found: field(&doc, "zarr_format")?.to_string(),
            }),
        }
    }

    /// Read a v2 `.zarray`.
    pub fn from_v2_metadata(text: &str) -> Result<Self, FetchPlanError> {
        Self::from_v2_value(&parse_document(text)?)
    }

    /// Read a v3 `zarr.json`.
    pub fn from_v3_metadata(text: &str) -> Result<Self, FetchPlanError> {
        Self::from_v3_value(&parse_document(text)?)
    }

    fn from_v2_value(doc: &Value) -> Result<Self, FetchPlanError> {
        let shape = extents(doc, "shape")?;
        let chunk_shape = extents(doc, "chunks")?;
        // Absent in most stores written before the key was standardised, and
        // `.` is what those meant. zarr-python still writes it out.
        let separator = match doc.get("dimension_separator") {
            None | Some(Value::Null) => '.',
            Some(value) => separator_of(value)?,
        };
        Self::new(shape, chunk_shape, ChunkKeyEncoding::V2, separator, 2)
    }

    fn from_v3_value(doc: &Value) -> Result<Self, FetchPlanError> {
        // A group's `zarr.json` has the same name and none of the fields, so
        // saying which node this is beats failing on a missing `shape`.
        if let Some(node) = doc.get("node_type").and_then(Value::as_str)
            && node != "array"
        {
            return Err(FetchPlanError::NotAnArray {
                node_type: node.to_string(),
            });
        }

        let shape = extents(doc, "shape")?;

        let grid = field(doc, "chunk_grid")?;
        let grid_name = grid.get("name").and_then(Value::as_str).unwrap_or_default();
        if grid_name != "regular" {
            return Err(FetchPlanError::UnsupportedChunkGrid {
                name: grid_name.to_string(),
            });
        }
        let configuration = grid
            .get("configuration")
            .ok_or(FetchPlanError::MissingField {
                dialect: Dialect::ZarrMetadata,
                key: "chunk_grid.configuration",
            })?;
        let chunk_shape = extents(configuration, "chunk_shape")?;

        // Absent means `default` with `/`, which is what the v3 core spec's
        // own defaults say and what zarr-python writes when asked for neither.
        let (encoding, separator) = match doc.get("chunk_key_encoding") {
            None | Some(Value::Null) => (ChunkKeyEncoding::Default, '/'),
            Some(value) => {
                let name = value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let encoding = match name {
                    "default" => ChunkKeyEncoding::Default,
                    "v2" => ChunkKeyEncoding::V2,
                    other => {
                        return Err(FetchPlanError::UnsupportedChunkKeyEncoding {
                            name: other.to_string(),
                        });
                    }
                };
                // Each form has its own default separator, so an absent
                // `configuration` is not one missing value but two. The
                // per-encoding default is the model's to state, not this
                // reader's — see `ChunkKeyEncoding::default_separator`.
                let separator = match value.get("configuration").and_then(|c| c.get("separator")) {
                    None | Some(Value::Null) => encoding.default_separator(),
                    Some(separator) => separator_of(separator)?,
                };
                (encoding, separator)
            }
        };

        Self::new(shape, chunk_shape, encoding, separator, 3)
    }

    fn new(
        shape: Vec<u64>,
        chunk_shape: Vec<u64>,
        encoding: ChunkKeyEncoding,
        separator: char,
        zarr_format: u8,
    ) -> Result<Self, FetchPlanError> {
        // The rank check and the zero-extent refusal are the shared model's
        // (#677): a document read here and a store walked elsewhere describe
        // the same thing and must refuse the same shapes.
        Ok(Self {
            grid: ChunkGrid::new(shape, chunk_shape)?,
            encoding,
            separator,
            zarr_format,
        })
    }

    /// The array's shape, in elements.
    pub fn shape(&self) -> &[u64] {
        self.grid.shape()
    }

    /// One chunk's shape, in elements. Every chunk has this shape, including
    /// the ones at a ragged edge, which are stored full-size and trimmed on
    /// read — by the decoder, not here.
    pub fn chunk_shape(&self) -> &[u64] {
        self.grid.chunk_shape()
    }

    /// How many dimensions the array has.
    pub fn rank(&self) -> usize {
        self.grid.rank()
    }

    /// Which edition's metadata this was read from: 2 or 3.
    pub fn zarr_format(&self) -> u8 {
        self.zarr_format
    }

    /// How this array's chunk keys are spelled.
    pub fn chunk_key_encoding(&self) -> ChunkKeyEncoding {
        self.encoding
    }

    /// The character joining a chunk key's parts.
    pub fn separator(&self) -> char {
        self.separator
    }

    /// How many chunks there are along each axis.
    ///
    /// The ceiling of the shape over the chunk shape: an axis of 7 in chunks of
    /// 3 has three chunks, the last holding two values and two of padding.
    pub fn chunk_grid(&self) -> Vec<u64> {
        self.grid.grid_shape()
    }

    /// The store key one chunk of the grid is written under.
    ///
    /// The key is relative to the array, which is where a store puts it: an
    /// array at `temp` in a group holds this chunk at `temp/` + this key. The
    /// prefix is the caller's because this crate never invents a key.
    pub fn chunk_key(&self, index: &[u64]) -> Result<String, FetchPlanError> {
        Ok(self.grid.chunk_key(index, self.encoding, self.separator)?)
    }

    /// Which chunk holds one element of the array.
    pub fn chunk_containing(&self, point: &[u64]) -> Result<Vec<u64>, FetchPlanError> {
        Ok(self.grid.chunk_containing(point)?)
    }

    /// Every chunk a region of the array touches, in row-major order.
    ///
    /// What a host asks before fetching a slice: a request for
    /// `[0..1, 2..5]` of a `[2, 3]`-chunked array needs two chunks, not one and
    /// not six. The half-open ranges are element indices; an empty range on any
    /// axis selects nothing and yields no chunks, which is the honest answer
    /// rather than an error.
    ///
    /// A region touching more than 1,048,576 chunks is refused rather than
    /// built, because the metadata document that sets the array's shape arrives
    /// over the network and a plan is not a place to trust it. The bound comes
    /// back on
    /// [`ArrayError::RegionTooLarge`](fieldglass_core::array::ArrayError::RegionTooLarge),
    /// so a caller reports it rather than restating it.
    pub fn chunks_covering(&self, region: &[Range<u64>]) -> Result<Vec<Vec<u64>>, FetchPlanError> {
        Ok(self.grid.chunks_covering(region)?)
    }
}

fn parse_document(text: &str) -> Result<Value, FetchPlanError> {
    serde_json::from_str(text).map_err(|e| FetchPlanError::Document {
        dialect: Dialect::ZarrMetadata,
        detail: e.to_string(),
    })
}

fn field<'a>(doc: &'a Value, key: &'static str) -> Result<&'a Value, FetchPlanError> {
    doc.get(key).ok_or(FetchPlanError::MissingField {
        dialect: Dialect::ZarrMetadata,
        key,
    })
}

/// One `shape`-shaped field: an array of non-negative integers.
fn extents(doc: &Value, key: &'static str) -> Result<Vec<u64>, FetchPlanError> {
    let list = field(doc, key)?
        .as_array()
        .ok_or(FetchPlanError::BadFieldType {
            dialect: Dialect::ZarrMetadata,
            key,
            expected: "an array of non-negative integers",
        })?;
    list.iter()
        .map(|extent| {
            extent.as_u64().ok_or(FetchPlanError::BadFieldType {
                dialect: Dialect::ZarrMetadata,
                key,
                expected: "an array of non-negative integers",
            })
        })
        .collect()
}

/// One `dimension_separator` / `separator` field, validated by the shared
/// array model so every reader of an array's metadata refuses the same set.
fn separator_of(value: &Value) -> Result<char, FetchPlanError> {
    Ok(ChunkKeyEncoding::separator_from_str(
        value.as_str().unwrap_or_default(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass_core::array::ArrayError;

    const V2: &str = r#"{
        "zarr_format": 2, "shape": [4, 6], "chunks": [2, 3],
        "dtype": "<f4", "compressor": null, "fill_value": 0.0,
        "order": "C", "filters": null, "dimension_separator": "."
    }"#;

    const V3: &str = r#"{
        "zarr_format": 3, "node_type": "array", "shape": [4, 6],
        "data_type": "float32",
        "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [2, 3]}},
        "chunk_key_encoding": {"name": "default", "configuration": {"separator": "/"}},
        "fill_value": 0.0, "codecs": []
    }"#;

    /// The two editions describe the same grid, and differ only in how they
    /// spell it. Pinned against `zarr-python`'s own encoders, which is where
    /// these forms were read from rather than from prose.
    #[test]
    fn the_two_editions_spell_one_grid_two_ways() {
        let v2 = ZarrArrayMeta::from_metadata(V2).unwrap();
        let v3 = ZarrArrayMeta::from_metadata(V3).unwrap();

        assert_eq!(v2.shape(), v3.shape());
        assert_eq!(v2.chunk_shape(), v3.chunk_shape());
        assert_eq!(v2.chunk_grid(), vec![2, 2]);
        assert_eq!(v3.chunk_grid(), vec![2, 2]);

        assert_eq!(v2.chunk_key(&[1, 0]).unwrap(), "1.0");
        assert_eq!(v3.chunk_key(&[1, 0]).unwrap(), "c/1/0");
    }

    /// v3's `v2` encoding is v2's, which is the whole reason it exists — a
    /// store migrated to v3 keeps the object names it already had.
    #[test]
    fn the_v2_key_encoding_under_v3_spells_a_v2_key() {
        let text = V3.replace(
            r#""chunk_key_encoding": {"name": "default", "configuration": {"separator": "/"}}"#,
            r#""chunk_key_encoding": {"name": "v2"}"#,
        );
        let meta = ZarrArrayMeta::from_metadata(&text).unwrap();
        assert_eq!(meta.chunk_key_encoding(), ChunkKeyEncoding::V2);
        // The separator defaults per *encoding*, not per document: `v2` means
        // `.` even though the `default` encoding beside it would have meant `/`.
        assert_eq!(meta.separator(), '.');
        assert_eq!(meta.chunk_key(&[1, 0]).unwrap(), "1.0");
    }

    /// The zero-dimensional cases, which are not what the pattern suggests and
    /// are the reason this is a table rather than a join.
    #[test]
    fn a_zero_dimensional_array_has_one_chunk_with_a_name_of_its_own() {
        let v2 = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 2, "shape": [], "chunks": [], "dtype": "<f4"}"#,
        )
        .unwrap();
        assert_eq!(v2.chunk_key(&[]).unwrap(), "0");

        let v3 = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 3, "node_type": "array", "shape": [],
                "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": []}},
                "chunk_key_encoding": {"name": "default"}}"#,
        )
        .unwrap();
        assert_eq!(v3.chunk_key(&[]).unwrap(), "c");
    }

    /// A chunk extent of zero is a division by zero, and arrives from a
    /// document rather than from a caller.
    #[test]
    fn a_zero_chunk_extent_is_refused_at_parse() {
        let err = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 2, "shape": [4, 6], "chunks": [2, 0], "dtype": "<f4"}"#,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            FetchPlanError::Array(ArrayError::ZeroChunkExtent { axis: 1 })
        ));
    }

    /// Everything the crate cannot address is refused by name, so a host
    /// reports what is unsupported rather than a missing key three levels down.
    #[test]
    fn what_is_not_read_is_refused_by_name() {
        let rectilinear = r#"{"zarr_format": 3, "node_type": "array", "shape": [4],
            "chunk_grid": {"name": "rectilinear", "configuration": {"chunk_shape": [2]}}}"#;
        assert!(matches!(
            ZarrArrayMeta::from_metadata(rectilinear).unwrap_err(),
            FetchPlanError::UnsupportedChunkGrid { name } if name == "rectilinear"
        ));

        let group = r#"{"zarr_format": 3, "node_type": "group", "attributes": {}}"#;
        assert!(matches!(
            ZarrArrayMeta::from_metadata(group).unwrap_err(),
            FetchPlanError::NotAnArray { node_type } if node_type == "group"
        ));

        let future = r#"{"zarr_format": 4, "shape": [4], "chunks": [2]}"#;
        assert!(matches!(
            ZarrArrayMeta::from_metadata(future).unwrap_err(),
            FetchPlanError::UnsupportedZarrFormat { .. }
        ));

        let separator = r#"{"zarr_format": 2, "shape": [4], "chunks": [2],
            "dimension_separator": "-"}"#;
        assert!(matches!(
            ZarrArrayMeta::from_metadata(separator).unwrap_err(),
            FetchPlanError::Array(ArrayError::BadSeparator { found }) if found == "-"
        ));

        let ranks = r#"{"zarr_format": 2, "shape": [4, 6], "chunks": [2]}"#;
        assert!(matches!(
            ZarrArrayMeta::from_metadata(ranks).unwrap_err(),
            FetchPlanError::Array(ArrayError::RankMismatch {
                shape: 2,
                chunks: 1
            })
        ));
    }

    /// A negative or fractional extent is not a length. `serde_json` would
    /// happily hand back an `f64`, and `as u64` would turn -1 into a very large
    /// array.
    #[test]
    fn an_extent_that_is_not_a_count_is_refused() {
        for shape in ["[-1, 6]", "[4.5, 6]", r#"["4", 6]"#, "4"] {
            let text = format!(r#"{{"zarr_format": 2, "shape": {shape}, "chunks": [2, 3]}}"#);
            assert!(
                matches!(
                    ZarrArrayMeta::from_metadata(&text),
                    Err(FetchPlanError::BadFieldType { .. })
                ),
                "{shape} should not parse as a shape"
            );
        }
    }
}
