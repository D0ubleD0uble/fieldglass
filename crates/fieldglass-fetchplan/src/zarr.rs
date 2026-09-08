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

use std::fmt::Write as _;
use std::ops::Range;

/// The most chunk indices [`ZarrArrayMeta::chunks_covering`] will build a list
/// of.
///
/// Without a bound, a metadata document is an allocation instruction: `shape`
/// of `[1000000000000]` in chunks of one is sixty bytes of JSON and a list of a
/// trillion indices, which is thirty-two terabytes before anything is fetched.
/// The document is fetched over the network, so its numbers are as untrusted as
/// a sidecar's offsets — the same reasoning that put checked arithmetic behind
/// the other two dialects (#652).
///
/// A million is far past any real plan. A host issuing a million range requests
/// has a different problem than this cap, and a viewer asking for a region that
/// touches a million chunks has asked for the whole archive.
const MAX_PLANNED_CHUNKS: u64 = 1 << 20;

use serde_json::Value;

use crate::error::{Dialect, FetchPlanError};

/// How a chunk grid index is spelled as a key in the store.
///
/// The names are Zarr v3's, and v2 has only the one convention, which v3 also
/// offers so that a v2 store can be migrated without rewriting every object's
/// name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChunkKeyEncoding {
    /// v3's `default`: a `c` prefix, then the index, all joined by the
    /// separator — `c/1/0`. A zero-dimensional array's chunk is `c`.
    Default,
    /// v2's, and v3's `v2`: the index joined by the separator, with no prefix —
    /// `1.0`. A zero-dimensional array's chunk is `0`, not the empty string.
    V2,
}

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
    shape: Vec<u64>,
    chunk_shape: Vec<u64>,
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
                // `configuration` is not one missing value but two.
                let separator = match value.get("configuration").and_then(|c| c.get("separator")) {
                    None | Some(Value::Null) => match encoding {
                        ChunkKeyEncoding::Default => '/',
                        ChunkKeyEncoding::V2 => '.',
                    },
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
        if shape.len() != chunk_shape.len() {
            return Err(FetchPlanError::RankMismatch {
                shape: shape.len(),
                chunks: chunk_shape.len(),
            });
        }
        // Every chunk extent divides a shape extent to give the grid, so a zero
        // is a division by zero rather than an empty array — and an array with
        // a zero *shape* extent is legal and has no chunks along it.
        if let Some(axis) = chunk_shape.iter().position(|extent| *extent == 0) {
            return Err(FetchPlanError::ZeroChunkExtent { axis });
        }
        Ok(Self {
            shape,
            chunk_shape,
            encoding,
            separator,
            zarr_format,
        })
    }

    /// The array's shape, in elements.
    pub fn shape(&self) -> &[u64] {
        &self.shape
    }

    /// One chunk's shape, in elements. Every chunk has this shape, including
    /// the ones at a ragged edge, which are stored full-size and trimmed on
    /// read — by the decoder, not here.
    pub fn chunk_shape(&self) -> &[u64] {
        &self.chunk_shape
    }

    /// How many dimensions the array has.
    pub fn rank(&self) -> usize {
        self.shape.len()
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
        self.shape
            .iter()
            .zip(&self.chunk_shape)
            .map(|(extent, chunk)| extent.div_ceil(*chunk))
            .collect()
    }

    /// The store key one chunk of the grid is written under.
    ///
    /// The key is relative to the array, which is where a store puts it: an
    /// array at `temp` in a group holds this chunk at `temp/` + this key. The
    /// prefix is the caller's because this crate never invents a key.
    pub fn chunk_key(&self, index: &[u64]) -> Result<String, FetchPlanError> {
        self.check_index(index)?;

        let mut key = String::new();
        if self.encoding == ChunkKeyEncoding::Default {
            key.push('c');
        }
        // A zero-dimensional array has one chunk and no index to write, and the
        // two encodings disagree about what to call it: `c` under `default`,
        // and `0` — not the empty string — under `v2`.
        if index.is_empty() {
            if self.encoding == ChunkKeyEncoding::V2 {
                key.push('0');
            }
            return Ok(key);
        }
        for (axis, position) in index.iter().enumerate() {
            if axis > 0 || self.encoding == ChunkKeyEncoding::Default {
                key.push(self.separator);
            }
            write!(key, "{position}").expect("writing to a String cannot fail");
        }
        Ok(key)
    }

    /// Which chunk holds one element of the array.
    pub fn chunk_containing(&self, point: &[u64]) -> Result<Vec<u64>, FetchPlanError> {
        if point.len() != self.rank() {
            return Err(FetchPlanError::WrongRank {
                expected: self.rank(),
                found: point.len(),
            });
        }
        point
            .iter()
            .zip(&self.shape)
            .zip(&self.chunk_shape)
            .enumerate()
            .map(|(axis, ((position, extent), chunk))| {
                if position >= extent {
                    return Err(FetchPlanError::PointOutOfRange {
                        axis,
                        index: *position,
                        extent: *extent,
                    });
                }
                Ok(position / chunk)
            })
            .collect()
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
    /// [`FetchPlanError::RegionTooLarge`](crate::FetchPlanError::RegionTooLarge),
    /// so a caller reports it rather than restating it.
    pub fn chunks_covering(&self, region: &[Range<u64>]) -> Result<Vec<Vec<u64>>, FetchPlanError> {
        if region.len() != self.rank() {
            return Err(FetchPlanError::WrongRank {
                expected: self.rank(),
                found: region.len(),
            });
        }

        let mut spans = Vec::with_capacity(self.rank());
        for (axis, (range, (extent, chunk))) in region
            .iter()
            .zip(self.shape.iter().zip(&self.chunk_shape))
            .enumerate()
        {
            if range.end > *extent {
                return Err(FetchPlanError::PointOutOfRange {
                    axis,
                    index: range.end,
                    extent: *extent,
                });
            }
            if range.start >= range.end {
                return Ok(Vec::new());
            }
            // Inclusive of the chunk holding the last element, exclusive of the
            // one after it: `end` is one past the region, so the last element
            // is `end - 1` and a region ending exactly on a chunk boundary must
            // not pull in the chunk beyond it.
            spans.push(range.start / chunk..(range.end - 1) / chunk + 1);
        }

        // Counted before anything is allocated, and with `checked_mul`, because
        // the product of the spans is exactly what the walk below would try to
        // hold and a rank-6 array overflows a `u64` long before it runs out of
        // memory.
        let mut planned: u64 = 1;
        for span in &spans {
            planned = planned
                .checked_mul(span.end - span.start)
                .filter(|count| *count <= MAX_PLANNED_CHUNKS)
                .ok_or(FetchPlanError::RegionTooLarge {
                    limit: MAX_PLANNED_CHUNKS,
                })?;
        }

        let mut out = vec![Vec::with_capacity(self.rank())];
        for span in spans {
            let width = usize::try_from(span.end - span.start).unwrap_or(usize::MAX);
            let mut next = Vec::with_capacity(out.len().saturating_mul(width));
            for prefix in &out {
                for position in span.clone() {
                    let mut index = prefix.clone();
                    index.push(position);
                    next.push(index);
                }
            }
            out = next;
        }
        Ok(out)
    }

    fn check_index(&self, index: &[u64]) -> Result<(), FetchPlanError> {
        if index.len() != self.rank() {
            return Err(FetchPlanError::WrongRank {
                expected: self.rank(),
                found: index.len(),
            });
        }
        for (axis, (position, extent)) in index.iter().zip(self.chunk_grid()).enumerate() {
            if *position >= extent {
                return Err(FetchPlanError::ChunkIndexOutOfRange {
                    axis,
                    index: *position,
                    extent,
                });
            }
        }
        Ok(())
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

/// A separator is one character, and only the two the conventions use.
///
/// Refused rather than passed through: a key built with an arbitrary string
/// fetches nothing, and this crate's whole output is keys.
fn separator_of(value: &Value) -> Result<char, FetchPlanError> {
    let text = value.as_str().unwrap_or_default();
    match text {
        "." => Ok('.'),
        "/" => Ok('/'),
        other => Err(FetchPlanError::BadSeparator {
            found: other.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A ragged edge rounds up: the grid has to cover the array, and the last
    /// chunk is stored full-size regardless.
    #[test]
    fn the_grid_covers_a_shape_that_is_not_a_multiple_of_the_chunk() {
        let meta = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 2, "shape": [7, 10], "chunks": [3, 4], "dtype": "<f4"}"#,
        )
        .unwrap();
        assert_eq!(meta.chunk_grid(), vec![3, 3]);
        // The last element is in the last chunk, and there is no chunk past it.
        assert_eq!(meta.chunk_containing(&[6, 9]).unwrap(), vec![2, 2]);
        assert!(meta.chunk_key(&[3, 0]).is_err());
    }

    /// A region ending exactly on a chunk boundary must not pull in the chunk
    /// after it — the off-by-one that fetches twice the bytes it needs.
    #[test]
    fn a_region_covers_the_chunks_it_touches_and_no_more() {
        let meta = ZarrArrayMeta::from_metadata(V2).unwrap();

        // Exactly the first chunk: rows 0..2, columns 0..3.
        assert_eq!(
            meta.chunks_covering(&[0..2, 0..3]).unwrap(),
            vec![vec![0, 0]]
        );
        // One column further and the neighbour is needed too.
        assert_eq!(
            meta.chunks_covering(&[0..2, 0..4]).unwrap(),
            vec![vec![0, 0], vec![0, 1]]
        );
        // The whole array is every chunk, in row-major order.
        assert_eq!(
            meta.chunks_covering(&[0..4, 0..6]).unwrap(),
            vec![vec![0, 0], vec![0, 1], vec![1, 0], vec![1, 1]]
        );
        // An empty range selects nothing.
        assert!(meta.chunks_covering(&[0..0, 0..6]).unwrap().is_empty());
        // Past the end is refused rather than clamped.
        assert!(meta.chunks_covering(&[0..5, 0..6]).is_err());
    }

    /// A chunk extent of zero is a division by zero, and arrives from a
    /// document rather than from a caller.
    #[test]
    fn a_zero_chunk_extent_is_refused_at_parse() {
        let err = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 2, "shape": [4, 6], "chunks": [2, 0], "dtype": "<f4"}"#,
        )
        .unwrap_err();
        assert!(matches!(err, FetchPlanError::ZeroChunkExtent { axis: 1 }));
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
            FetchPlanError::BadSeparator { found } if found == "-"
        ));

        let ranks = r#"{"zarr_format": 2, "shape": [4, 6], "chunks": [2]}"#;
        assert!(matches!(
            ZarrArrayMeta::from_metadata(ranks).unwrap_err(),
            FetchPlanError::RankMismatch {
                shape: 2,
                chunks: 1
            }
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

    /// A region is counted before it is built. The shape comes out of a fetched
    /// document, so "how many chunks is that" is an untrusted number, and the
    /// list this would otherwise allocate is thirty-two terabytes.
    #[test]
    fn an_enormous_region_is_refused_rather_than_allocated() {
        let meta = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 2, "shape": [1000000, 1000000], "chunks": [1, 1]}"#,
        )
        .unwrap();
        assert_eq!(meta.chunk_grid(), vec![1_000_000, 1_000_000]);

        let err = meta
            .chunks_covering(&[0..1_000_000, 0..1_000_000])
            .unwrap_err();
        assert!(
            matches!(err, FetchPlanError::RegionTooLarge { .. }),
            "{err:?}"
        );
        // Neither axis alone is over the cap, so a check that looked at one
        // span at a time would have let this through.
        assert!(meta.chunks_covering(&[0..1_000_000, 0..1]).is_ok());

        // The overflow path too: the product of the spans wraps a `u64` long
        // before it exhausts memory, so the count is checked and not merely
        // compared.
        let wide = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 2,
                "shape": [18446744073709551615, 18446744073709551615, 18446744073709551615],
                "chunks": [1, 1, 1]}"#,
        )
        .unwrap();
        assert!(matches!(
            wide.chunks_covering(&[
                0..18_446_744_073_709_551_615,
                0..18_446_744_073_709_551_615,
                0..18_446_744_073_709_551_615
            ])
            .unwrap_err(),
            FetchPlanError::RegionTooLarge { .. }
        ));

        // A region inside the cap is still planned, so the bound refuses the
        // pathological case and nothing else.
        let ordinary = ZarrArrayMeta::from_metadata(
            r#"{"zarr_format": 2, "shape": [64, 64], "chunks": [1, 1]}"#,
        )
        .unwrap();
        assert_eq!(
            ordinary.chunks_covering(&[0..64, 0..64]).unwrap().len(),
            4096
        );
    }
}
