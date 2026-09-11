//! One reader for an array's metadata document, in either edition.
//!
//! A `.zarray` (v2) and a `zarr.json` (v3) describe the same array in two
//! spellings, and two things want to read them: the codecs here, which need the
//! element type and the chain a chunk was written through, and a planner or a
//! store walker, which needs the shape, the chunk shape and how a chunk key is
//! spelled. Before #686 each read the document itself, with two `zarr_format`
//! checks, two `node_type` checks, two "is the chunk grid regular" checks and
//! two vocabularies for refusing the same thing.
//!
//! This is the one reader. It produces [`ArrayMetadata`]: the shared array
//! model from `fieldglass-core` plus what this crate adds on top of it.
//!
//! # It is not behind the `codecs` feature
//!
//! Reading a document is not decoding one. A consumer that wants to know what
//! an array *is* — its shape, its chunk grid, where a chunk lives, what type
//! its elements are — links no decompressor to find out, which is what makes
//! `fieldglass-fetchplan` able to take this crate at all (ADR-0010 decision 6).
//! [`ChunkDecoder`](crate::ChunkDecoder) is the part behind the feature.

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{ChunkGrid, ChunkKeyEncoding};
use serde_json::Value;

use crate::dtype::DType;

/// How the elements of a chunk were laid out before any filter saw them.
///
/// v2 states this as `order`; v3 has no equivalent and expresses the same thing
/// as a `transpose` codec, which is why this does not reach the v3 arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElementOrder {
    /// Row-major, the default.
    C,
    /// Column-major. Decoded into C order so a caller indexes every array the
    /// same way whichever order it was written in.
    Fortran,
}

/// The codec configuration, kept as the document stated it.
///
/// Held rather than parsed so that reading a document costs nothing when the
/// `codecs` feature is off: the chain is built from this on demand, by the half
/// of the crate that can decode.
#[derive(Debug, Clone, PartialEq)]
pub enum CodecSource {
    /// v2: an element order, a compressor and a filter list, each of which the
    /// document may state as `null`.
    V2 {
        /// `order`, already validated.
        order: ElementOrder,
        /// `compressor`, verbatim.
        compressor: Option<Value>,
        /// `filters`, verbatim.
        filters: Option<Value>,
    },
    /// v3: an ordered `codecs` list, verbatim.
    V3 {
        /// `codecs`, verbatim.
        codecs: Value,
    },
}

/// One Zarr array, as its metadata document describes it.
///
/// The chunk grid and the key encoding are `fieldglass-core`'s (#677), because
/// a store walker, a fetch planner and this crate all ask the same questions of
/// them. What is added here is what only a Zarr document states: which edition
/// wrote it, the element type, the fill value, and the codec configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayMetadata {
    zarr_format: u8,
    grid: ChunkGrid,
    key_encoding: ChunkKeyEncoding,
    separator: char,
    dtype: DType,
    fill_value: Option<f64>,
    codecs: CodecSource,
}

impl ArrayMetadata {
    /// Read either edition, deciding from the document's own `zarr_format`.
    ///
    /// A caller that walked a store knows which document it opened and can use
    /// [`from_v2`](Self::from_v2) or [`from_v3`](Self::from_v3) directly. A
    /// caller holding an inline value out of a reference document, where the
    /// key says `.zarray` but nothing enforces it, is better served asking the
    /// document what it is.
    pub fn parse(json: &str) -> Result<Self, FieldglassError> {
        let doc = document(json)?;
        match doc.get("zarr_format").and_then(Value::as_u64) {
            Some(2) => Self::from_value(&doc, 2),
            Some(3) => Self::from_value(&doc, 3),
            Some(other) => Err(FieldglassError::UnsupportedSection(format!(
                "Zarr metadata states zarr_format {other}; only 2 and 3 are read"
            ))),
            None => Err(FieldglassError::Parse(
                "Zarr metadata states no `zarr_format`".to_string(),
            )),
        }
    }

    /// Read a v2 `.zarray`.
    pub fn from_v2(json: &str) -> Result<Self, FieldglassError> {
        Self::from_value(&document(json)?, 2)
    }

    /// Read a v3 `zarr.json` for an array.
    pub fn from_v3(json: &str) -> Result<Self, FieldglassError> {
        Self::from_value(&document(json)?, 3)
    }

    /// Read an already-parsed document of a known edition. What the store
    /// walker uses: a consolidated store hands it every document at once, as
    /// JSON values, and serialising each back to text to reparse it would be
    /// work for nothing.
    pub(crate) fn from_value(doc: &Value, edition: u8) -> Result<Self, FieldglassError> {
        // Checked once, whichever entry point was used, so a document handed to
        // the wrong reader says so instead of failing on a field it does not
        // have.
        if let Some(stated) = doc.get("zarr_format").and_then(Value::as_u64)
            && stated != u64::from(edition)
        {
            return Err(FieldglassError::Parse(format!(
                "this document states zarr_format {stated}, not {edition}"
            )));
        }
        match edition {
            2 => Self::v2(doc),
            _ => Self::v3(doc),
        }
    }

    fn v2(doc: &Value) -> Result<Self, FieldglassError> {
        let shape = extents(doc.get("shape"), "Zarr v2 `shape`")?;
        let chunk_shape = extents(doc.get("chunks"), "Zarr v2 `chunks`")?;
        let dtype =
            DType::parse_v2(doc.get("dtype").and_then(Value::as_str).ok_or_else(|| {
                FieldglassError::Parse("Zarr v2 .zarray states no `dtype`".into())
            })?)?;

        // Absent in stores written before the key was standardised, and `.` is
        // what those meant.
        let separator = match doc.get("dimension_separator") {
            None | Some(Value::Null) => ChunkKeyEncoding::V2.default_separator(),
            Some(value) => separator(value)?,
        };

        let order = match doc.get("order").and_then(Value::as_str) {
            None | Some("C") => ElementOrder::C,
            Some("F") => ElementOrder::Fortran,
            Some(other) => {
                return Err(FieldglassError::Parse(format!(
                    "Zarr v2 .zarray states order {other:?}, which is neither C nor F"
                )));
            }
        };

        Ok(Self {
            zarr_format: 2,
            grid: ChunkGrid::new(shape, chunk_shape)?,
            key_encoding: ChunkKeyEncoding::V2,
            separator,
            dtype,
            fill_value: fill_number(doc.get("fill_value"), dtype),
            codecs: CodecSource::V2 {
                order,
                compressor: doc.get("compressor").cloned(),
                filters: doc.get("filters").cloned(),
            },
        })
    }

    fn v3(doc: &Value) -> Result<Self, FieldglassError> {
        // A group's `zarr.json` has the same file name and none of these
        // fields, so saying which node this is beats failing on a missing
        // `shape`.
        if let Some(node) = doc.get("node_type").and_then(Value::as_str)
            && node != "array"
        {
            return Err(FieldglassError::WrongLayout(format!(
                "this zarr.json describes a {node}, not an array"
            )));
        }

        let shape = extents(doc.get("shape"), "Zarr v3 `shape`")?;

        let grid = doc.get("chunk_grid").ok_or_else(|| {
            FieldglassError::Parse("Zarr v3 zarr.json states no `chunk_grid`".to_string())
        })?;
        match grid.get("name").and_then(Value::as_str) {
            Some("regular") => {}
            Some(other) => {
                return Err(FieldglassError::UnsupportedSection(format!(
                    "Zarr v3 chunk grid {other:?} is not read (only `regular` is)"
                )));
            }
            None => {
                return Err(FieldglassError::Parse(
                    "Zarr v3 `chunk_grid` states no name".to_string(),
                ));
            }
        }
        let chunk_shape = extents(
            grid.get("configuration").and_then(|c| c.get("chunk_shape")),
            "Zarr v3 `chunk_shape`",
        )?;

        // Absent means `default` with its own separator. Each encoding has its
        // own default, so an absent `configuration` is not one missing value
        // but two — see `ChunkKeyEncoding`.
        let (key_encoding, separator) = match doc.get("chunk_key_encoding") {
            None | Some(Value::Null) => (
                ChunkKeyEncoding::Default,
                ChunkKeyEncoding::Default.default_separator(),
            ),
            Some(value) => {
                let encoding = match value.get("name").and_then(Value::as_str) {
                    Some("default") => ChunkKeyEncoding::Default,
                    Some("v2") => ChunkKeyEncoding::V2,
                    Some(other) => {
                        return Err(FieldglassError::UnsupportedSection(format!(
                            "Zarr v3 chunk key encoding {other:?} is not read \
                             (only `default` and `v2` are)"
                        )));
                    }
                    None => {
                        return Err(FieldglassError::Parse(
                            "Zarr v3 `chunk_key_encoding` states no name".to_string(),
                        ));
                    }
                };
                let separator = match value.get("configuration").and_then(|c| c.get("separator")) {
                    None | Some(Value::Null) => encoding.default_separator(),
                    Some(value) => separator(value)?,
                };
                (encoding, separator)
            }
        };

        let dtype = DType::parse_v3(doc.get("data_type").and_then(Value::as_str).ok_or_else(
            || FieldglassError::Parse("Zarr v3 zarr.json states no `data_type`".to_string()),
        )?)?;

        let codecs = doc
            .get("codecs")
            .ok_or_else(|| {
                FieldglassError::Parse("Zarr v3 zarr.json states no `codecs`".to_string())
            })?
            .clone();

        Ok(Self {
            zarr_format: 3,
            grid: ChunkGrid::new(shape, chunk_shape)?,
            key_encoding,
            separator,
            dtype,
            fill_value: fill_number(doc.get("fill_value"), dtype),
            codecs: CodecSource::V3 { codecs },
        })
    }

    /// Which edition's metadata this was read from: 2 or 3.
    pub fn zarr_format(&self) -> u8 {
        self.zarr_format
    }

    /// The array's chunk grid — shape, chunk shape, and the arithmetic between
    /// an index and a chunk.
    pub fn grid(&self) -> &ChunkGrid {
        &self.grid
    }

    /// How this array's chunk keys are spelled.
    pub fn key_encoding(&self) -> ChunkKeyEncoding {
        self.key_encoding
    }

    /// The character joining a chunk key's parts.
    pub fn separator(&self) -> char {
        self.separator
    }

    /// The store key one chunk of the grid is written under, relative to the
    /// array. The array's own prefix is the caller's.
    pub fn chunk_key(&self, index: &[u64]) -> Result<String, FieldglassError> {
        Ok(self
            .grid
            .chunk_key(index, self.key_encoding, self.separator)?)
    }

    /// The element type.
    pub fn dtype(&self) -> DType {
        self.dtype
    }

    /// The fill value an absent chunk reads as, when the document states one
    /// this crate can express as a number.
    pub fn fill_value(&self) -> Option<f64> {
        self.fill_value
    }

    /// The codec configuration, as the document stated it.
    pub fn codecs(&self) -> &CodecSource {
        &self.codecs
    }
}

/// A `fill_value` as a number, in any of the spellings the two editions allow.
///
/// JSON has no NaN or infinity, so both editions spell them as the strings
/// `"NaN"`, `"Infinity"` and `"-Infinity"`, and v3 also allows a float's exact
/// bit pattern as a hex string (`"0x7fc00000"`). Reading only JSON numbers
/// turned every NaN-filled array's absent chunks into holes rather than NaN,
/// which is not what zarr-python reads there (#658). `None` is left for `null`
/// and for what no numeric type can express — a structured dtype's base64 blob.
fn fill_number(value: Option<&Value>, dtype: DType) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => match s.as_str() {
            "NaN" => Some(f64::NAN),
            "Infinity" => Some(f64::INFINITY),
            "-Infinity" => Some(f64::NEG_INFINITY),
            hex => {
                let bits = u64::from_str_radix(hex.strip_prefix("0x")?, 16).ok()?;
                match dtype.size {
                    4 => Some(f64::from(f32::from_bits(u32::try_from(bits).ok()?))),
                    8 => Some(f64::from_bits(bits)),
                    _ => None,
                }
            }
        },
        _ => None,
    }
}

fn document(json: &str) -> Result<Value, FieldglassError> {
    serde_json::from_str(json)
        .map_err(|e| FieldglassError::Parse(format!("Zarr metadata is not JSON: {e}")))
}

/// One `shape`-shaped field: a list of non-negative lengths.
///
/// **An empty list is a zero-dimensional array, not an error.** Zarr allows
/// one, it holds exactly one chunk, and both editions have a spelling for that
/// chunk's key. The reader this replaced refused it (`names no axes`) while the
/// planner's accepted it, which is the kind of disagreement one parser exists
/// to end.
fn extents(value: Option<&Value>, what: &str) -> Result<Vec<u64>, FieldglassError> {
    let list = value
        .and_then(Value::as_array)
        .ok_or_else(|| FieldglassError::Parse(format!("{what} is missing or is not a list")))?;
    list.iter()
        .map(|v| {
            v.as_u64().ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "{what} holds a value that is not a non-negative length"
                ))
            })
        })
        .collect()
}

fn separator(value: &Value) -> Result<char, FieldglassError> {
    Ok(ChunkKeyEncoding::separator_from_str(
        value.as_str().unwrap_or_default(),
    )?)
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
        "fill_value": 0.0, "codecs": [{"name": "bytes", "configuration": {"endian": "little"}}]
    }"#;

    /// The two editions describe one array, and the reader answers the same
    /// questions of both — which is the whole point of there being one of it.
    #[test]
    fn the_two_editions_read_to_one_model() {
        let v2 = ArrayMetadata::parse(V2).unwrap();
        let v3 = ArrayMetadata::parse(V3).unwrap();

        assert_eq!(v2.zarr_format(), 2);
        assert_eq!(v3.zarr_format(), 3);
        assert_eq!(v2.grid().shape(), v3.grid().shape());
        assert_eq!(v2.grid().chunk_shape(), v3.grid().chunk_shape());
        assert_eq!(v2.grid().grid_shape(), v3.grid().grid_shape());

        // They spell the same chunk differently, and both spellings are what
        // the committed stores use as file names.
        assert_eq!(v2.chunk_key(&[1, 1]).unwrap(), "1.1");
        assert_eq!(v3.chunk_key(&[1, 1]).unwrap(), "c/1/1");
    }

    /// A zero-dimensional array is legal and has one chunk. The codec-side
    /// reader used to refuse it and the planner's accepted it; one parser means
    /// one answer, and the specification's answer is that it is fine.
    #[test]
    fn a_zero_dimensional_array_is_read_rather_than_refused() {
        let v2 = ArrayMetadata::parse(
            r#"{"zarr_format": 2, "shape": [], "chunks": [], "dtype": "<f4"}"#,
        )
        .unwrap();
        assert_eq!(v2.grid().rank(), 0);
        assert_eq!(v2.chunk_key(&[]).unwrap(), "0");

        let v3 = ArrayMetadata::parse(
            r#"{"zarr_format": 3, "node_type": "array", "shape": [],
                "data_type": "uint8",
                "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": []}},
                "codecs": []}"#,
        )
        .unwrap();
        assert_eq!(v3.chunk_key(&[]).unwrap(), "c");
    }

    /// v3's `v2` key encoding takes `.` even though the `default` encoding it
    /// sits beside would have meant `/`. The default is the encoding's.
    #[test]
    fn the_v2_key_encoding_under_v3_takes_its_own_default_separator() {
        let text = V3.replace(
            r#""chunk_key_encoding": {"name": "default", "configuration": {"separator": "/"}}"#,
            r#""chunk_key_encoding": {"name": "v2"}"#,
        );
        let meta = ArrayMetadata::parse(&text).unwrap();
        assert_eq!(meta.key_encoding(), ChunkKeyEncoding::V2);
        assert_eq!(meta.separator(), '.');
        assert_eq!(meta.chunk_key(&[1, 0]).unwrap(), "1.0");
    }

    /// Each unsupported feature is refused in one place, so the two readers
    /// that used to do this separately cannot drift about which is which.
    #[test]
    fn what_is_not_read_is_refused_once_and_by_name() {
        let cases = [
            (
                r#"{"zarr_format": 3, "node_type": "array", "shape": [4],
                    "data_type": "uint8", "codecs": [],
                    "chunk_grid": {"name": "rectilinear"}}"#,
                "rectilinear",
            ),
            (
                r#"{"zarr_format": 3, "node_type": "array", "shape": [4],
                    "data_type": "uint8", "codecs": [],
                    "chunk_grid": {"name": "regular", "configuration": {"chunk_shape": [2]}},
                    "chunk_key_encoding": {"name": "nested"}}"#,
                "nested",
            ),
            (
                r#"{"zarr_format": 4, "shape": [4], "chunks": [2], "dtype": "<f4"}"#,
                "zarr_format 4",
            ),
        ];
        for (json, expected) in cases {
            let err = ArrayMetadata::parse(json).unwrap_err();
            assert!(
                err.to_string().contains(expected),
                "{json} gave {err}, which does not name {expected:?}"
            );
        }

        // A group's document, handed over where an array's was expected.
        let group = r#"{"zarr_format": 3, "node_type": "group", "attributes": {}}"#;
        assert!(matches!(
            ArrayMetadata::parse(group),
            Err(FieldglassError::WrongLayout(_))
        ));
    }

    /// The refusals the shared array model owns arrive through it, so a rank
    /// mismatch reads the same here as anywhere else.
    #[test]
    fn the_models_own_refusals_come_through_it() {
        let ranks = r#"{"zarr_format": 2, "shape": [4, 6], "chunks": [2], "dtype": "<f4"}"#;
        assert!(matches!(
            ArrayMetadata::parse(ranks),
            Err(FieldglassError::Array(
                fieldglass_core::array::ArrayError::RankMismatch { .. }
            ))
        ));

        let zero = r#"{"zarr_format": 2, "shape": [4], "chunks": [0], "dtype": "<f4"}"#;
        assert!(matches!(
            ArrayMetadata::parse(zero),
            Err(FieldglassError::Array(
                fieldglass_core::array::ArrayError::ZeroChunkExtent { axis: 0 }
            ))
        ));

        let separator = r#"{"zarr_format": 2, "shape": [4], "chunks": [2],
            "dtype": "<f4", "dimension_separator": "-"}"#;
        assert!(matches!(
            ArrayMetadata::parse(separator),
            Err(FieldglassError::Array(
                fieldglass_core::array::ArrayError::BadSeparator { .. }
            ))
        ));
    }

    /// An extent that is not a count is not a length. `as u64` would turn -1
    /// into a very large array.
    #[test]
    fn an_extent_that_is_not_a_count_is_refused() {
        for shape in ["[-1, 6]", "[4.5, 6]", r#"["4", 6]"#, "4"] {
            let json = format!(
                r#"{{"zarr_format": 2, "shape": {shape}, "chunks": [2, 3], "dtype": "<f4"}}"#
            );
            assert!(
                ArrayMetadata::parse(&json).is_err(),
                "{shape} should not parse as a shape"
            );
        }
    }

    /// The codec configuration is carried verbatim, because building the chain
    /// is the other half of the crate's job and may not be compiled at all.
    #[test]
    fn the_codec_configuration_is_carried_as_stated() {
        let meta = ArrayMetadata::parse(V2).unwrap();
        assert!(matches!(
            meta.codecs(),
            CodecSource::V2 {
                order: ElementOrder::C,
                ..
            }
        ));

        let fortran =
            ArrayMetadata::parse(&V2.replace(r#""order": "C""#, r#""order": "F""#)).unwrap();
        assert!(matches!(
            fortran.codecs(),
            CodecSource::V2 {
                order: ElementOrder::Fortran,
                ..
            }
        ));

        assert!(matches!(
            ArrayMetadata::parse(V3).unwrap().codecs(),
            CodecSource::V3 { .. }
        ));

        // An order that is neither is refused where it is read, not where it
        // would have been applied.
        let bad = V2.replace(r#""order": "C""#, r#""order": "Z""#);
        assert!(ArrayMetadata::parse(&bad).is_err());
    }
}
