//! Kerchunk reference documents: a chunk key in, a byte range out.
//!
//! A kerchunk reference document is what makes an archive that was never a Zarr
//! store readable as one. It maps every key a Zarr client would ask for onto
//! either the bytes themselves — the metadata documents, small enough to write
//! inline — or a `[url, offset, length]` triple pointing into some object that
//! already exists. The archive is untouched; the reference document is a few
//! hundred kilobytes beside it.
//!
//! That triple is this crate's output type already, which is why this belongs
//! beside the `.idx` and `.index` dialects rather than inside the codec crate:
//! it is a third spelling of "where are the bytes", not a fourth thing.
//!
//! [`KerchunkRefs`] carries a worked example.
//!
//! # Why this is not a [`Manifest`](crate::Manifest)
//!
//! The trait the two GRIB dialects implement promises one object key per
//! manifest and answers a [`Query`](crate::Query) written in parameters, levels
//! and forecast steps. A reference document has neither: it addresses as many
//! objects as it likes — that is the point of `templates` — and the only thing
//! it can be asked is which chunk of which array you want, in indices. Making
//! it implement the trait would mean `key()` returning one of the URLs and a
//! parameter query that never matches, so the two stay separate types.
//!
//! # What is read, and what is refused by name
//!
//! Version 1 only, and of it the `refs` map and simple `{{name}}` substitution
//! against `templates`. A `gen` block generates references from jinja2
//! expressions over a dimension, which is a template language rather than a
//! manifest grammar; it is refused, naming itself, rather than half-read by
//! planning the `refs` beside it and silently omitting everything `gen` would
//! have produced.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::error::{Dialect, FetchPlanError};
use crate::plan::{Expect, PlanItem, PlanRange};
use crate::zarr::ZarrArrayMeta;

/// The metadata document names a Zarr v2 array is stored under.
const V2_ARRAY_METADATA: &str = ".zarray";

/// And v3's, which is also a group's, so a match on it is checked by parsing.
const V3_METADATA: &str = "zarr.json";

/// One entry of a reference document.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    /// The bytes, written into the document itself. The metadata documents
    /// arrive this way, which is what lets a client read the store's shape
    /// without fetching anything at all.
    Inline(Vec<u8>),
    /// A byte range of an object.
    Range {
        url: String,
        offset: u64,
        length: u64,
    },
    /// A whole object, which kerchunk writes as a one-element array.
    Whole { url: String },
}

/// A parsed kerchunk reference document.
///
/// Holds every key the document declares, with templates already substituted,
/// so nothing here reads the clock or the network and a resolved item carries a
/// URL a host can fetch as-is.
///
/// ```
/// use fieldglass_fetchplan::KerchunkRefs;
///
/// let doc = r#"{
///   "version": 1,
///   "templates": {"u": "s3://bucket/archive.nc"},
///   "refs": {
///     "temp/.zarray": "{\"zarr_format\":2,\"shape\":[4,6],\"chunks\":[2,3]}",
///     "temp/1.1": ["{{u}}", 4096, 24]
///   }
/// }"#;
/// let refs = KerchunkRefs::parse(doc)?;
/// let meta = refs.array("temp")?;
///
/// let item = refs.chunk_at("temp", &meta, &[1, 1])?.expect("a planned chunk");
/// assert_eq!(item.key, "s3://bucket/archive.nc");
/// assert_eq!(item.range.http_range_header().as_deref(), Some("bytes=4096-4119"));
/// # Ok::<(), fieldglass_fetchplan::FetchPlanError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KerchunkRefs {
    entries: BTreeMap<String, Entry>,
}

impl KerchunkRefs {
    /// Read a version 1 reference document.
    pub fn parse(text: &str) -> Result<Self, FetchPlanError> {
        let doc: Value = serde_json::from_str(text).map_err(|e| FetchPlanError::Document {
            dialect: Dialect::Kerchunk,
            detail: e.to_string(),
        })?;

        // A version 0 document is a bare map of references with no wrapper at
        // all, so an absent `version` is a different document rather than a
        // malformed one, and saying so beats "required key missing".
        let Some(version) = doc.get("version") else {
            return Err(FetchPlanError::UnsupportedFeature {
                dialect: Dialect::Kerchunk,
                feature: "a document with no `version`, which is the version 0 \
                          layout; only version 1 is read",
            });
        };
        if version.as_u64() != Some(1) {
            return Err(FetchPlanError::UnsupportedKerchunkVersion {
                found: version.to_string(),
            });
        }

        // Refused rather than skipped. `gen` is the half of the document that
        // is a jinja2 program over a dimension, and reading the `refs` beside
        // it would return a plan that is short by every reference `gen` would
        // have produced — with nothing to say so.
        if doc
            .get("gen")
            .and_then(Value::as_array)
            .is_some_and(|entries| !entries.is_empty())
        {
            return Err(FetchPlanError::UnsupportedFeature {
                dialect: Dialect::Kerchunk,
                feature: "a `gen` block, which generates references from jinja2 \
                          expressions rather than stating them",
            });
        }

        let templates = match doc.get("templates") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return Err(FetchPlanError::BadFieldType {
                    dialect: Dialect::Kerchunk,
                    key: "templates",
                    expected: "an object mapping a name to a URL",
                });
            }
        };

        let refs = match doc.get("refs") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return Err(FetchPlanError::BadFieldType {
                    dialect: Dialect::Kerchunk,
                    key: "refs",
                    expected: "an object mapping a store key to data or a range",
                });
            }
        };

        let mut entries = BTreeMap::new();
        for (key, value) in &refs {
            entries.insert(key.clone(), entry(key, value, &templates)?);
        }
        Ok(Self { entries })
    }

    /// Every key the document declares, in sorted order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// The bytes of a key written into the document, if it was written inline.
    ///
    /// The metadata documents are what this answers in practice, and answering
    /// them without a fetch is the reason a reference document exists.
    pub fn inline(&self, key: &str) -> Option<&[u8]> {
        match self.entries.get(key) {
            Some(Entry::Inline(bytes)) => Some(bytes),
            _ => None,
        }
    }

    /// The fetch for one key, if the document addresses it as a range.
    ///
    /// `None` covers both "no such key" and "that key is inline": neither is a
    /// fetch, and a caller reaching for bytes should ask
    /// [`inline`](Self::inline) first.
    pub fn range_of(&self, key: &str) -> Option<PlanItem> {
        let (url, range) = match self.entries.get(key)? {
            Entry::Inline(_) => return None,
            Entry::Range {
                url,
                offset,
                length,
            } => (
                url,
                PlanRange::Exact {
                    offset: *offset,
                    length: *length,
                },
            ),
            Entry::Whole { url } => (url, PlanRange::Whole),
        };
        Some(PlanItem {
            key: url.clone(),
            range,
            sub_index: None,
            // A reference document promises nothing about the bytes beyond
            // where they are: no parameter, no level, and no GRIB envelope to
            // check, because what is in the range is a Zarr chunk.
            expect: Expect::default(),
        })
    }

    /// Every array the document carries metadata for, in sorted order.
    ///
    /// An array is a key ending in `.zarray` or `zarr.json`; the name is what
    /// precedes it. The root array of a store — metadata at `.zarray` with no
    /// prefix — comes back as the empty string, which is the name it has.
    pub fn arrays(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry, Entry::Inline(_)))
            .filter_map(|(key, _)| array_name(key))
            .filter(|name| self.array(name).is_ok())
            .map(str::to_string)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Read one array's metadata for addressing.
    ///
    /// Both editions are looked for under `name`, so a caller need not know
    /// which one wrote the store.
    pub fn array(&self, name: &str) -> Result<ZarrArrayMeta, FetchPlanError> {
        for document in [V2_ARRAY_METADATA, V3_METADATA] {
            let key = join(name, document);
            if let Some(bytes) = self.inline(&key) {
                let text = std::str::from_utf8(bytes).map_err(|e| FetchPlanError::Document {
                    dialect: Dialect::ZarrMetadata,
                    detail: format!("{key}: {e}"),
                })?;
                return ZarrArrayMeta::from_metadata(text);
            }
        }
        Err(FetchPlanError::NoSuchArray {
            name: name.to_string(),
        })
    }

    /// The fetch for one chunk of one array.
    ///
    /// The call a host makes: it has an array's metadata and the index of the
    /// chunk it wants, and this spells the key the way that array's edition
    /// does, prefixes it with the array's name, and looks it up.
    ///
    /// `Ok(None)` is a chunk the document does not address, which for a Zarr
    /// array is not an error: an absent chunk is the fill value, and a sparse
    /// array is a normal one.
    pub fn chunk_at(
        &self,
        array: &str,
        meta: &ZarrArrayMeta,
        index: &[u64],
    ) -> Result<Option<PlanItem>, FetchPlanError> {
        Ok(self.range_of(&join(array, &meta.chunk_key(index)?)))
    }

    /// The fetches for every chunk a region of an array touches, in row-major
    /// order, skipping the ones the document does not address.
    pub fn chunks_covering(
        &self,
        array: &str,
        meta: &ZarrArrayMeta,
        region: &[std::ops::Range<u64>],
    ) -> Result<Vec<PlanItem>, FetchPlanError> {
        let mut out = Vec::new();
        for index in meta.chunks_covering(region)? {
            if let Some(item) = self.chunk_at(array, meta, &index)? {
                out.push(item);
            }
        }
        Ok(out)
    }
}

/// Join an array name to a key below it, tolerating the unnamed root.
fn join(name: &str, key: &str) -> String {
    if name.is_empty() {
        key.to_string()
    } else {
        format!("{name}/{key}")
    }
}

/// The array a metadata key belongs to, or `None` if the key is not one.
fn array_name(key: &str) -> Option<&str> {
    for document in [V2_ARRAY_METADATA, V3_METADATA] {
        if key == document {
            return Some("");
        }
        if let Some(prefix) = key.strip_suffix(document)
            && let Some(name) = prefix.strip_suffix('/')
        {
            return Some(name);
        }
    }
    None
}

/// One `refs` value, in the three shapes the spec allows.
fn entry(
    key: &str,
    value: &Value,
    templates: &Map<String, Value>,
) -> Result<Entry, FetchPlanError> {
    match value {
        Value::String(text) => match text.strip_prefix("base64:") {
            Some(encoded) => decode_base64(encoded).map(Entry::Inline).ok_or_else(|| {
                FetchPlanError::BadReference {
                    key: key.to_string(),
                    detail: "the `base64:` value is not valid standard base64".to_string(),
                }
            }),
            // Not UTF-8 validated on the way in: an inline value is data, and
            // the metadata documents that arrive this way are checked when they
            // are read as JSON. A caller getting bytes back is the honest shape.
            None => Ok(Entry::Inline(text.clone().into_bytes())),
        },
        Value::Array(parts) => match parts.as_slice() {
            [url] => Ok(Entry::Whole {
                url: resolve(key, url, templates)?,
            }),
            [url, offset, length] => Ok(Entry::Range {
                url: resolve(key, url, templates)?,
                offset: count(key, offset, "offset")?,
                length: count(key, length, "length")?,
            }),
            other => Err(FetchPlanError::BadReference {
                key: key.to_string(),
                detail: format!(
                    "a reference array has one element (a whole object) or three \
                     (url, offset, length), found {}",
                    other.len()
                ),
            }),
        },
        // The spec allows an object, meaning "this key's data is this JSON".
        // Rendering it back to text is the one reading that does not invent
        // anything, and it is how a `.zattrs` sometimes arrives.
        Value::Object(_) => Ok(Entry::Inline(value.to_string().into_bytes())),
        other => Err(FetchPlanError::BadReference {
            key: key.to_string(),
            detail: format!(
                "a reference is data, a one-element array or a three-element \
                 array, found {other}"
            ),
        }),
    }
}

/// A `refs` offset or length: a non-negative integer.
///
/// Kerchunk writes these as JSON numbers, and a document produced by a tool
/// that stringified them is common enough to accept — but only if the string is
/// itself an integer, which is a parse rather than a cast.
fn count(key: &str, value: &Value, field: &'static str) -> Result<u64, FetchPlanError> {
    let parsed = match value {
        Value::Number(_) => value.as_u64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    };
    parsed.ok_or_else(|| FetchPlanError::BadReference {
        key: key.to_string(),
        detail: format!("{field} is not a non-negative integer: {value}"),
    })
}

/// Substitute `{{name}}` against the document's templates.
///
/// Only a bare name is read. The spec's templates are jinja2, so a value may in
/// principle be an expression or a call with arguments; those are refused by
/// name rather than approximated, because a URL built by guessing at a template
/// language fetches the wrong object and reports success.
fn resolve(
    key: &str,
    url: &Value,
    templates: &Map<String, Value>,
) -> Result<String, FetchPlanError> {
    let url = url.as_str().ok_or_else(|| FetchPlanError::BadReference {
        key: key.to_string(),
        detail: format!("a reference's URL must be a string, found {url}"),
    })?;

    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let close = after
            .find("}}")
            .ok_or_else(|| FetchPlanError::BadReference {
                key: key.to_string(),
                detail: format!("the URL {url:?} has a `{{{{` with no closing `}}}}`"),
            })?;
        let name = after[..close].trim();

        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            || name.is_empty()
        {
            return Err(FetchPlanError::UnsupportedTemplate {
                expression: name.to_string(),
            });
        }
        let value = templates.get(name).and_then(Value::as_str).ok_or_else(|| {
            FetchPlanError::UnknownTemplate {
                name: name.to_string(),
            }
        })?;
        out.push_str(value);
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Decode standard base64, strictly.
///
/// Hand-rolled rather than taken as a dependency: this crate declares three,
/// each explained in its manifest, and the alternative to forty lines of
/// well-specified arithmetic is a fourth entry in every downstream consumer's
/// licence scan. Strict on purpose — no whitespace, no alternative alphabet, no
/// missing padding — because a lenient decoder turns a corrupt document into
/// plausible bytes, and the fuzz target drives this.
fn decode_base64(text: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u32> {
        Some(match byte {
            b'A'..=b'Z' => u32::from(byte - b'A'),
            b'a'..=b'z' => u32::from(byte - b'a') + 26,
            b'0'..=b'9' => u32::from(byte - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }

    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (block, quad) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let last = block == bytes.len() / 4 - 1;
        // Padding is only ever the last one or two characters of the last
        // quad: `=` anywhere else is a corrupt document, not a short one.
        let padding = if last {
            quad.iter().filter(|b| **b == b'=').count()
        } else {
            0
        };
        if padding > 2 || quad[..4 - padding].contains(&b'=') {
            return None;
        }
        let mut packed = 0u32;
        for byte in &quad[..4 - padding] {
            packed = (packed << 6) | sextet(*byte)?;
        }
        // The bits a padded quad does not carry must be zero, or two distinct
        // encodings would decode to the same bytes.
        packed <<= 6 * padding;
        let decoded = packed.to_be_bytes();
        if padding > 0 && decoded[4 - padding..].iter().any(|b| *b != 0) {
            return None;
        }
        out.extend_from_slice(&decoded[1..4 - padding]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZARRAY: &str =
        r#"{\"zarr_format\":2,\"shape\":[4,6],\"chunks\":[2,3],\"dtype\":\"<f4\"}"#;

    fn document(body: &str) -> String {
        format!(r#"{{"version": 1, "refs": {{"temp/.zarray": "{ZARRAY}", {body}}}}}"#)
    }

    /// The three shapes a `refs` value takes, each reaching the right one of
    /// the two answers a caller can get: bytes, or a fetch.
    #[test]
    fn the_three_reference_shapes_each_resolve() {
        let refs = KerchunkRefs::parse(&document(
            r#""temp/0.0": ["s3://b/o.bin", 10, 24],
               "temp/0.1": ["s3://b/whole.bin"],
               "temp/1.0": "raw bytes""#,
        ))
        .unwrap();

        let ranged = refs.range_of("temp/0.0").unwrap();
        assert_eq!(ranged.key, "s3://b/o.bin");
        assert_eq!(
            ranged.range,
            PlanRange::Exact {
                offset: 10,
                length: 24
            }
        );

        let whole = refs.range_of("temp/0.1").unwrap();
        assert_eq!(whole.key, "s3://b/whole.bin");
        assert_eq!(whole.range, PlanRange::Whole);
        // A whole-object range sends no `Range` header at all.
        assert_eq!(whole.range.http_range_header(), None);

        // Inline data is bytes, and is not a fetch.
        assert_eq!(refs.inline("temp/1.0").unwrap(), b"raw bytes");
        assert!(refs.range_of("temp/1.0").is_none());

        // A key the document does not carry is not a fetch either.
        assert!(refs.range_of("temp/9.9").is_none());
        assert!(refs.inline("temp/9.9").is_none());
    }

    /// A template is substituted, so the item a host receives carries a URL it
    /// can fetch rather than one it has to finish.
    #[test]
    fn a_template_is_substituted_into_the_url() {
        let doc = r#"{
            "version": 1,
            "templates": {"u": "s3://bucket/archive.nc"},
            "refs": {"temp/0.0": ["{{u}}", 0, 24], "temp/0.1": ["{{ u }}/part", 0, 24]}
        }"#;
        let refs = KerchunkRefs::parse(doc).unwrap();
        assert_eq!(
            refs.range_of("temp/0.0").unwrap().key,
            "s3://bucket/archive.nc"
        );
        // Substitution is textual, so a template inside a longer URL works and
        // the surrounding text survives.
        assert_eq!(
            refs.range_of("temp/0.1").unwrap().key,
            "s3://bucket/archive.nc/part"
        );
    }

    /// Everything the crate will not read is refused by name, and nothing is
    /// half-read: a `gen` document does not come back holding only its `refs`.
    #[test]
    fn what_is_not_read_is_refused_by_name() {
        let generated = r#"{"version": 1, "refs": {"a": "b"},
            "gen": [{"key": "t/{{i}}", "url": "{{u}}", "dimensions": {"i": {"stop": 2}}}]}"#;
        assert!(matches!(
            KerchunkRefs::parse(generated).unwrap_err(),
            FetchPlanError::UnsupportedFeature { .. }
        ));

        // An empty `gen` generates nothing, so there is nothing to refuse.
        assert!(KerchunkRefs::parse(r#"{"version": 1, "gen": [], "refs": {}}"#).is_ok());

        let v0 = r#"{"a": "b", "c": ["s3://bucket/o", 0, 4]}"#;
        assert!(matches!(
            KerchunkRefs::parse(v0).unwrap_err(),
            FetchPlanError::UnsupportedFeature { .. }
        ));

        let future = r#"{"version": 2, "refs": {}}"#;
        assert!(matches!(
            KerchunkRefs::parse(future).unwrap_err(),
            FetchPlanError::UnsupportedKerchunkVersion { .. }
        ));

        // A jinja2 expression is not a name, and is refused rather than
        // substituted as if it were one.
        let expression = r#"{"version": 1, "templates": {"u": "s3://b/o"},
            "refs": {"a": ["{{u}}_{{i * 2}}", 0, 4]}}"#;
        assert!(matches!(
            KerchunkRefs::parse(expression).unwrap_err(),
            FetchPlanError::UnsupportedTemplate { .. }
        ));

        let unknown = r#"{"version": 1, "refs": {"a": ["{{nope}}", 0, 4]}}"#;
        assert!(matches!(
            KerchunkRefs::parse(unknown).unwrap_err(),
            FetchPlanError::UnknownTemplate { name } if name == "nope"
        ));
    }

    /// A malformed reference is refused with the key that carried it, because
    /// a document has thousands and "an offset is not an integer" alone does
    /// not say which.
    #[test]
    fn a_malformed_reference_names_its_key() {
        for body in [
            r#""temp/0.0": ["s3://b/o", -1, 24]"#,
            r#""temp/0.0": ["s3://b/o", 0, 24, 7]"#,
            r#""temp/0.0": ["s3://b/o", 1.5, 24]"#,
            r#""temp/0.0": ["s3://b/o", 0]"#,
            r#""temp/0.0": [7, 0, 24]"#,
            r#""temp/0.0": 42"#,
        ] {
            let err = KerchunkRefs::parse(&document(body)).unwrap_err();
            assert!(
                matches!(&err, FetchPlanError::BadReference { key, .. } if key == "temp/0.0"),
                "{body} gave {err:?}"
            );
        }
    }

    /// The seam this type exists for, without a store: metadata in, a chunk
    /// key spelled by the array's own edition, a range out.
    #[test]
    fn an_array_index_becomes_the_range_holding_that_chunk() {
        let refs = KerchunkRefs::parse(&document(
            r#""temp/0.0": ["s3://b/o", 0, 24],
               "temp/0.1": ["s3://b/o", 40, 24],
               "temp/1.0": ["s3://b/o", 80, 24],
               "temp/1.1": ["s3://b/o", 120, 24]"#,
        ))
        .unwrap();

        assert_eq!(refs.arrays(), vec!["temp".to_string()]);
        let meta = refs.array("temp").unwrap();

        let item = refs.chunk_at("temp", &meta, &[1, 0]).unwrap().unwrap();
        assert_eq!(
            item.range,
            PlanRange::Exact {
                offset: 80,
                length: 24
            }
        );

        // The region walk goes through the same spelling, and asks for exactly
        // the chunks the region touches.
        let plan = refs.chunks_covering("temp", &meta, &[0..4, 0..3]).unwrap();
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].range.offset(), 0);
        assert_eq!(plan[1].range.offset(), 80);

        // An unaddressed chunk is a sparse array's fill value, not an error.
        let sparse = KerchunkRefs::parse(&document(r#""temp/0.0": ["s3://b/o", 0, 24]"#)).unwrap();
        assert!(sparse.chunk_at("temp", &meta, &[1, 1]).unwrap().is_none());

        // An index off the grid is a caller error and is refused.
        assert!(refs.chunk_at("temp", &meta, &[2, 0]).is_err());
    }

    /// A base64 value decodes to bytes, and a corrupt one is refused rather
    /// than truncated into plausible data.
    #[test]
    fn base64_values_decode_strictly() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8h").unwrap(), b"hello!");
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("TQ==").unwrap(), b"M");

        // Unpadded, mispadded, out of alphabet, and non-zero trailing bits.
        assert!(decode_base64("aGVsbG8").is_none());
        assert!(decode_base64("a=VsbG8=").is_none());
        assert!(decode_base64("aGVs bG8=").is_none());
        assert!(decode_base64("aGVsbG8*").is_none());
        assert!(decode_base64("TR==").is_none());

        let refs = KerchunkRefs::parse(&document(r#""temp/0.0": "base64:aGVsbG8=""#)).unwrap();
        assert_eq!(refs.inline("temp/0.0").unwrap(), b"hello");

        let bad = KerchunkRefs::parse(&document(r#""temp/0.0": "base64:!!!!""#)).unwrap_err();
        assert!(matches!(bad, FetchPlanError::BadReference { key, .. } if key == "temp/0.0"));
    }

    /// The root array of a store has no name, and asking for one that is not
    /// there says so rather than answering an empty plan.
    #[test]
    fn the_unnamed_root_array_is_addressable() {
        let doc = format!(
            r#"{{"version": 1, "refs": {{".zarray": "{ZARRAY}", "0.0": ["s3://b/o", 0, 24]}}}}"#
        );
        let refs = KerchunkRefs::parse(&doc).unwrap();
        assert_eq!(refs.arrays(), vec![String::new()]);

        let meta = refs.array("").unwrap();
        let item = refs.chunk_at("", &meta, &[0, 0]).unwrap().unwrap();
        assert_eq!(item.range.offset(), 0);

        assert!(matches!(
            refs.array("nope").unwrap_err(),
            FetchPlanError::NoSuchArray { name } if name == "nope"
        ));
    }
}
