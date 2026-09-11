//! Reading a Zarr store: the walk from its root to every group and array in it,
//! and a region read that fetches exactly the chunks it covers (#658).
//!
//! A Zarr store is not a file a reader parses. It is a layout — chunks under
//! keys, and the documents that say what the keys hold — so this is IO, and it
//! sits where the NetCDF readers do: behind
//! [`ArraySource`], reading through the
//! [`ObjectSource`] seam a host fills. A directory, a bucket and a browser's
//! map of fetched objects are the same store to it.
//!
//! # What opening costs
//!
//! One prefetch batch of the root documents, and then one of two walks:
//!
//! * **Consolidated.** v2's `.zmetadata` and v3's `consolidated_metadata` hold
//!   every document in the store, so opening is one read. zarr-python and
//!   xarray write it by default, and walking a store one document at a time is
//!   what it exists to avoid.
//! * **Not consolidated.** The store is listed once, its metadata keys are
//!   prefetched in one batch, and each is read. Listing a remote store is the
//!   expensive step either way, which is why a consolidated store is the one a
//!   host should expect.
//!
//! # What a read costs
//!
//! [`ArraySource::read_region`] resolves the chunks the region covers from the
//! grid, spells their keys from core, prefetches exactly those in one batch,
//! and then reads them. An absent chunk is the fill value — a sparse array is a
//! normal one — and a chunk at a ragged edge is stored full-size, so only the
//! part inside the region is copied out.
//!
//! # Where CF comes from
//!
//! Nowhere here: the region read is raw, and
//! [`ArraySource::read_region_physical`] applies core's one CF rule from the
//! array's attributes. Two things xarray does to those attributes are undone
//! on the way in, so that rule sees what xarray means:
//!
//! * **Zarr v2 keeps `_FillValue` as the array's `fill_value`**, not as an
//!   attribute. A v2 array's numeric fill value is presented as a `_FillValue`
//!   attribute when the store states none, which is how xarray reads it — and
//!   is also why v3 is left alone, where `fill_value` is only what an absent
//!   chunk holds and a stored zero that packs to 200 K is data.
//! * **Zarr v3 float `_FillValue`s are base64.** xarray writes the bytes of a
//!   little-endian float, because JSON has no NaN. They are decoded to the
//!   number, or a `-9999` sentinel would never match a stored `-9999`.
//!
//! # What fails, and how far
//!
//! An array whose codec this crate does not decode is listed like any other,
//! and only reading it fails. An array whose document does not parse, or that
//! gives a shared dimension a second length, is left out of the tree and named
//! in [`ZarrStore::problems`] — one bad array is not a reason to refuse the
//! store, which is the failure #550 fixed for HDF5.

use std::collections::BTreeMap;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{
    ArrayDescription, ArraySource, Attribute, AttributeValue, Dimension, ElementType, Group,
};
use fieldglass_core::bytes::ObjectSource;
use serde_json::{Map, Value};

use crate::dtype::{DType, ScalarKind};
use crate::metadata::ArrayMetadata;

/// The most elements one region read — or one chunk it decodes — will hold.
///
/// An array's shape and chunk shape are numbers out of a document somebody
/// else wrote, so they are allocation instructions an attacker controls. 64 M
/// elements is a gigabyte of `Option<f64>`, past any slice a viewer asks for
/// and the cap `fieldglass-grib1` puts on a grid for the same reason.
pub const MAX_REGION_ELEMENTS: u64 = 64 * 1024 * 1024;

/// The documents a store's root may hold, prefetched as one batch before the
/// first is read, so finding out which edition and layout a store has costs one
/// round trip rather than one per guess.
const ROOT_KEYS: [&str; 5] = ["zarr.json", ".zmetadata", ".zgroup", ".zarray", ".zattrs"];

/// A Zarr store, opened.
///
/// Holds the [`ObjectSource`] it reads from, the tree it found, and for each
/// readable array the metadata a region read needs. Read it through
/// [`ArraySource`], which is how a caller that also reads NetCDF holds either.
///
/// ```
/// use fieldglass_core::bytes::MemoryObjects;
/// use fieldglass_zarr::{ArraySource, ZarrStore};
///
/// let store = MemoryObjects::from_iter([
///     (".zgroup", br#"{"zarr_format": 2}"#.to_vec()),
///     ("t/.zarray", br#"{"zarr_format": 2, "shape": [2, 2], "chunks": [2, 2],
///         "dtype": "<i2", "fill_value": 0, "compressor": null,
///         "filters": null, "order": "C"}"#.to_vec()),
///     ("t/0.0", [1i16, 2, 3, 4].iter().flat_map(|v| v.to_le_bytes()).collect()),
/// ]);
/// let zarr = ZarrStore::open(&store)?;
/// assert_eq!(zarr.group().arrays_qualified()[0].0, "t");
/// assert_eq!(
///     zarr.read_region("t", &[0..2, 1..2])?,
///     vec![Some(2.0), Some(4.0)]
/// );
/// # Ok::<(), fieldglass_zarr::FieldglassError>(())
/// ```
#[derive(Debug)]
pub struct ZarrStore<O> {
    objects: O,
    root: Group,
    arrays: BTreeMap<String, StoredArray>,
    problems: Vec<(String, String)>,
}

/// What a region read needs of one array, beyond its description.
// Without `codecs` there is no region read to need it, but the walk still
// builds it, so a build with the feature and one without list the same arrays.
#[cfg_attr(not(feature = "codecs"), allow(dead_code))]
#[derive(Debug)]
struct StoredArray {
    /// The key prefix its chunks sit under: `temp/`, or empty for an array at
    /// the store root.
    prefix: String,
    meta: ArrayMetadata,
}

/// One node of the hierarchy as its documents state it, before the tree is
/// built from them.
#[derive(Debug)]
enum Node {
    Group {
        attributes: Map<String, Value>,
    },
    Array {
        document: Value,
        attributes: Map<String, Value>,
        edition: u8,
    },
}

impl<O: ObjectSource> ZarrStore<O> {
    /// Walk the store `objects` holds, either edition, consolidated or not.
    ///
    /// # Errors
    ///
    /// A root that holds none of `zarr.json`, `.zmetadata`, `.zgroup` or
    /// `.zarray`, a root document that is not JSON, or a failure of the
    /// source. A malformed *array* is not an error here: it is left out and
    /// named in [`Self::problems`].
    pub fn open(objects: O) -> Result<Self, FieldglassError> {
        objects.prefetch(&ROOT_KEYS)?;
        let nodes = if let Some(root) = document(&objects, "zarr.json")? {
            v3_nodes(&objects, root)?
        } else if let Some(consolidated) = document(&objects, ".zmetadata")? {
            v2_consolidated_nodes(&consolidated)?
        } else if objects.get(".zgroup")?.is_some() || objects.get(".zarray")?.is_some() {
            v2_listed_nodes(&objects)?
        } else {
            return Err(FieldglassError::WrongLayout(
                "this store holds no zarr.json, .zmetadata, .zgroup or .zarray at its root, \
                 so it is not a Zarr store"
                    .to_string(),
            ));
        };
        let (root, arrays, problems) = build(nodes);
        Ok(Self {
            objects,
            root,
            arrays,
            problems,
        })
    }

    /// The source the store reads from.
    pub fn objects(&self) -> &O {
        &self.objects
    }

    /// The arrays left out of [`ArraySource::group`], each with why: a
    /// document that does not parse, a type this crate does not read, or a
    /// dimension given two lengths.
    pub fn problems(&self) -> &[(String, String)] {
        &self.problems
    }
}

impl<O: ObjectSource> ArraySource for ZarrStore<O> {
    fn group(&self) -> &Group {
        &self.root
    }

    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let stored = self.arrays.get(array).ok_or_else(|| {
            let why = self
                .problems
                .iter()
                .find(|(name, _)| name == array)
                .map_or_else(String::new, |(_, why)| format!(": {why}"));
            FieldglassError::Parse(format!("this store holds no readable array {array:?}{why}"))
        })?;
        read(&self.objects, stored, region)
    }
}

/// Read and parse one JSON document, or `None` when the store does not hold
/// the key.
fn document<O: ObjectSource>(objects: &O, key: &str) -> Result<Option<Value>, FieldglassError> {
    objects
        .get(key)?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|e| FieldglassError::Parse(format!("{key} is not JSON: {e}")))
        })
        .transpose()
}

/// The object a JSON value holds, or an empty one.
fn object(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// One v3 node's `zarr.json`, as a [`Node`].
fn v3_node(document: Value) -> Node {
    let attributes = object(document.get("attributes"));
    match document.get("node_type").and_then(Value::as_str) {
        Some("array") => Node::Array {
            document,
            attributes,
            edition: 3,
        },
        // A group, or a node that says nothing. The latter is not a valid
        // v3 document, but reading it as an empty group lists the arrays
        // under it rather than refusing them for their parent's sake.
        _ => Node::Group { attributes },
    }
}

/// Every node of a v3 store: from the root's `consolidated_metadata` when it
/// has one, and by listing the store when it does not.
fn v3_nodes<O: ObjectSource>(
    objects: &O,
    root: Value,
) -> Result<BTreeMap<String, Node>, FieldglassError> {
    let mut nodes = BTreeMap::new();
    let consolidated = root
        .get("consolidated_metadata")
        .and_then(|c| c.get("metadata"))
        .and_then(Value::as_object)
        .cloned();
    nodes.insert(String::new(), v3_node(root));

    if let Some(entries) = consolidated {
        for (path, document) in entries {
            nodes.insert(path, v3_node(document));
        }
        return Ok(nodes);
    }

    let keys: Vec<String> = objects
        .list("")?
        .into_iter()
        .filter(|key| key.ends_with("/zarr.json"))
        .collect();
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    objects.prefetch(&refs)?;
    for key in &keys {
        let path = key.trim_end_matches("/zarr.json").to_string();
        if let Some(document) = document(objects, key)? {
            nodes.insert(path, v3_node(document));
        }
    }
    Ok(nodes)
}

/// A v2 node from the documents its directory holds.
fn v2_node(zarray: Option<Value>, zattrs: Option<&Value>) -> Node {
    let attributes = object(zattrs);
    match zarray {
        Some(document) => Node::Array {
            document,
            attributes,
            edition: 2,
        },
        None => Node::Group { attributes },
    }
}

/// The directory a v2 metadata key sits in, and which document it is.
fn v2_split(key: &str) -> Option<(&str, &str)> {
    let (dir, file) = match key.rsplit_once('/') {
        Some((dir, file)) => (dir, file),
        None => ("", key),
    };
    matches!(file, ".zgroup" | ".zarray" | ".zattrs").then_some((dir, file))
}

/// The three documents of each v2 directory, gathered from `(key, document)`
/// pairs.
fn v2_gather(documents: impl IntoIterator<Item = (String, Value)>) -> BTreeMap<String, Node> {
    let mut dirs: BTreeMap<String, [Option<Value>; 3]> = BTreeMap::new();
    for (key, value) in documents {
        let Some((dir, file)) = v2_split(&key) else {
            continue;
        };
        let slot = match file {
            ".zgroup" => 0,
            ".zarray" => 1,
            _ => 2,
        };
        dirs.entry(dir.to_string()).or_default()[slot] = Some(value);
    }
    dirs.into_iter()
        .map(|(dir, [_, zarray, zattrs])| (dir, v2_node(zarray, zattrs.as_ref())))
        .collect()
}

/// Every node of a consolidated v2 store, out of its `.zmetadata`.
fn v2_consolidated_nodes(zmetadata: &Value) -> Result<BTreeMap<String, Node>, FieldglassError> {
    let entries = zmetadata
        .get("metadata")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            FieldglassError::Parse(".zmetadata holds no `metadata` object".to_string())
        })?;
    Ok(v2_gather(entries.clone()))
}

/// Every node of an unconsolidated v2 store, found by listing it.
fn v2_listed_nodes<O: ObjectSource>(
    objects: &O,
) -> Result<BTreeMap<String, Node>, FieldglassError> {
    let keys: Vec<String> = objects
        .list("")?
        .into_iter()
        .filter(|key| v2_split(key).is_some())
        .collect();
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    objects.prefetch(&refs)?;
    let mut documents = Vec::with_capacity(keys.len());
    for key in keys {
        if let Some(value) = document(objects, &key)? {
            documents.push((key, value));
        }
    }
    Ok(v2_gather(documents))
}

/// The group a node sits in: `a/b` for `a/b/c`, the root for `a`.
fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// A node's own name: its last path segment.
fn leaf(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, leaf)| leaf)
}

/// What one group of the tree holds while it is being built.
#[derive(Debug, Default)]
struct GroupParts {
    attributes: Vec<Attribute>,
    dimensions: Vec<Dimension>,
    arrays: Vec<ArrayDescription>,
}

/// Make sure `path` and every group above it exist, so an array under a group
/// the store states no document for still has a place in the tree.
fn ensure_groups(groups: &mut BTreeMap<String, GroupParts>, path: &str) {
    let mut at = path;
    loop {
        groups.entry(at.to_string()).or_default();
        if at.is_empty() {
            return;
        }
        at = parent(at);
    }
}

/// The tree, the readable arrays, and the arrays left out, from the nodes.
fn build(
    nodes: BTreeMap<String, Node>,
) -> (Group, BTreeMap<String, StoredArray>, Vec<(String, String)>) {
    let mut groups: BTreeMap<String, GroupParts> = BTreeMap::new();
    let mut arrays = BTreeMap::new();
    let mut problems = Vec::new();
    ensure_groups(&mut groups, "");

    for (path, node) in &nodes {
        if let Node::Group { attributes } = node {
            ensure_groups(&mut groups, path);
            if let Some(parts) = groups.get_mut(path) {
                parts.attributes = attributes_of(attributes);
            }
        }
    }

    for (path, node) in nodes {
        let Node::Array {
            document,
            attributes,
            edition,
        } = node
        else {
            continue;
        };
        let home = parent(&path).to_string();
        ensure_groups(&mut groups, &home);
        let (description, meta) = match describe(&path, &document, &attributes, edition) {
            Ok(described) => described,
            Err(why) => {
                problems.push((path, why.to_string()));
                continue;
            }
        };

        // A dimension is shared by name across the group, so a second array
        // giving it another length is the one left out, not the group.
        let parts = groups.get_mut(&home).expect("ensured above");
        let shape = meta.grid().shape();
        let clash = description
            .dimensions
            .iter()
            .zip(shape)
            .find_map(|(name, length)| {
                parts
                    .dimensions
                    .iter()
                    .find(|d| &d.name == name && d.length != *length)
                    .map(|d| {
                        format!(
                            "dimension {name:?} has length {length} here and {} in \
                             another array of this group",
                            d.length
                        )
                    })
            });
        if let Some(why) = clash {
            problems.push((path, why));
            continue;
        }
        for (name, length) in description.dimensions.iter().zip(shape) {
            if !parts.dimensions.iter().any(|d| &d.name == name) {
                parts.dimensions.push(Dimension {
                    name: name.clone(),
                    length: *length,
                });
            }
        }
        parts.arrays.push(description);
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        arrays.insert(path, StoredArray { prefix, meta });
    }

    (assemble("", &mut groups), arrays, problems)
}

/// One group and everything under it, out of the flat map of parts.
fn assemble(path: &str, groups: &mut BTreeMap<String, GroupParts>) -> Group {
    let parts = groups.remove(path).unwrap_or_default();
    let children: Vec<String> = groups
        .keys()
        .filter(|candidate| !candidate.is_empty() && parent(candidate) == path)
        .cloned()
        .collect();
    Group {
        name: leaf(path).to_string(),
        attributes: parts.attributes,
        dimensions: parts.dimensions,
        arrays: parts.arrays,
        groups: children
            .iter()
            .map(|child| assemble(child, groups))
            .collect(),
    }
}

/// One array's description and the metadata a read needs, or why neither can
/// be had.
fn describe(
    path: &str,
    document: &Value,
    attributes: &Map<String, Value>,
    edition: u8,
) -> Result<(ArrayDescription, ArrayMetadata), FieldglassError> {
    let meta = ArrayMetadata::from_value(document, edition)?;
    let name = leaf(path).to_string();
    let rank = meta.grid().rank();

    // v3 states the names in the array document; v2 has none, and xarray's
    // convention is an `_ARRAY_DIMENSIONS` attribute. Either way a list of
    // the wrong length, or an axis left unnamed, falls back to a name that is
    // the array's own, so it is never shared by accident with another array
    // whose axis merely has the same position.
    let stated = match edition {
        3 => document.get("dimension_names"),
        _ => attributes.get("_ARRAY_DIMENSIONS"),
    }
    .and_then(Value::as_array)
    .filter(|names| names.len() == rank);
    let dimensions = (0..rank)
        .map(|axis| {
            stated
                .and_then(|names| names[axis].as_str())
                .map_or_else(|| unnamed_axis(&name, axis), str::to_string)
        })
        .collect();

    let mut attributes = attributes_of(attributes);
    present_fill_value(&mut attributes, &meta, edition);

    Ok((
        ArrayDescription {
            name,
            element_type: element_type(meta.dtype()),
            dimensions,
            attributes,
            chunk_grid: Some(meta.grid().clone()),
        },
        meta,
    ))
}

/// The name an axis the store leaves unnamed gets: `temp_dim1` for the second
/// axis of `temp`, `dim0` for the first axis of an array at the store root.
fn unnamed_axis(array: &str, axis: usize) -> String {
    if array.is_empty() {
        format!("dim{axis}")
    } else {
        format!("{array}_dim{axis}")
    }
}

/// The two ways xarray keeps `_FillValue` somewhere core's CF rule would not
/// look, undone — see the module doc.
fn present_fill_value(attributes: &mut Vec<Attribute>, meta: &ArrayMetadata, edition: u8) {
    let stated = attributes.iter().position(|a| a.name == "_FillValue");
    match (edition, stated) {
        (2, None) => {
            if let Some(fill) = meta.fill_value() {
                attributes.push(Attribute::number("_FillValue", fill));
            }
        }
        (3, Some(at)) if meta.dtype().kind == ScalarKind::Float => {
            let decoded = attributes[at]
                .value
                .text()
                .and_then(crate::base64::decode)
                .and_then(|bytes| match bytes.len() {
                    8 => Some(f64::from_le_bytes(bytes.try_into().ok()?)),
                    4 => Some(f64::from(f32::from_le_bytes(bytes.try_into().ok()?))),
                    _ => None,
                });
            if let Some(value) = decoded {
                attributes[at] = Attribute::number("_FillValue", value);
            }
        }
        _ => {}
    }
}

/// A Zarr attribute object as the model's attributes, numbers kept as numbers.
fn attributes_of(object: &Map<String, Value>) -> Vec<Attribute> {
    object
        .iter()
        .map(|(name, value)| {
            let value = match value {
                Value::String(text) => AttributeValue::Text(text.clone()),
                Value::Number(n) => match n.as_f64() {
                    Some(v) => AttributeValue::Numbers(vec![v]),
                    None => AttributeValue::Opaque(n.to_string()),
                },
                Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_number) => {
                    AttributeValue::Numbers(items.iter().filter_map(Value::as_f64).collect())
                }
                // A boolean, a list of strings, a nested object: kept as the
                // store's own JSON so a host can still show it.
                other => AttributeValue::Opaque(other.to_string()),
            };
            Attribute {
                name: name.clone(),
                value,
            }
        })
        .collect()
}

/// The model's element type for a Zarr dtype.
fn element_type(dtype: DType) -> ElementType {
    let bits = u8::try_from(dtype.size * 8).unwrap_or(u8::MAX);
    match dtype.kind {
        ScalarKind::Int => ElementType::Int(bits),
        ScalarKind::Uint => ElementType::Uint(bits),
        ScalarKind::Float => ElementType::Float(bits),
        ScalarKind::Bool => ElementType::Other("bool".to_string()),
    }
}

#[cfg(feature = "codecs")]
/// The product of some extents, refused past [`MAX_REGION_ELEMENTS`].
fn element_count(extents: &[u64]) -> Result<usize, FieldglassError> {
    extents
        .iter()
        .try_fold(1u64, |acc, &n| acc.checked_mul(n))
        .filter(|&count| count <= MAX_REGION_ELEMENTS)
        .and_then(|count| usize::try_from(count).ok())
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "{extents:?} is more than the {MAX_REGION_ELEMENTS} elements one read will hold"
            ))
        })
}

#[cfg(feature = "codecs")]
/// Row-major strides for a box of `extents`.
fn strides(extents: &[u64]) -> Vec<u64> {
    let mut out = vec![1u64; extents.len()];
    for axis in (0..extents.len().saturating_sub(1)).rev() {
        out[axis] = out[axis + 1] * extents[axis + 1];
    }
    out
}

#[cfg(feature = "codecs")]
/// Copy the part of one decoded block that falls inside `region` into `out`.
///
/// The block is `block_shape` elements with its first at `origin`; `out` is
/// the region, `lens` long on each axis. A block at a ragged edge is stored
/// full-size, and because the region never reaches past the array, the part
/// beyond the edge is never inside it — the intersection is the trim.
fn copy_block(
    block: &[Option<f64>],
    block_shape: &[u64],
    origin: &[u64],
    region: &[Range<u64>],
    lens: &[u64],
    out: &mut [Option<f64>],
) {
    let rank = region.len();
    if rank == 0 {
        if let (Some(slot), Some(value)) = (out.first_mut(), block.first()) {
            *slot = *value;
        }
        return;
    }
    let lo: Vec<u64> = (0..rank).map(|d| origin[d].max(region[d].start)).collect();
    let hi: Vec<u64> = (0..rank)
        .map(|d| (origin[d] + block_shape[d]).min(region[d].end))
        .collect();
    if (0..rank).any(|d| lo[d] >= hi[d]) {
        return;
    }
    let (from, to) = (strides(block_shape), strides(lens));
    let last = rank - 1;
    // `hi - lo` along the last axis is within one block, so it fits.
    let run = (hi[last] - lo[last]) as usize;
    let mut at = lo.clone();
    loop {
        let src: u64 = (0..rank).map(|d| (at[d] - origin[d]) * from[d]).sum();
        let dst: u64 = (0..rank).map(|d| (at[d] - region[d].start) * to[d]).sum();
        // Both are offsets into buffers whose lengths were checked against
        // `MAX_REGION_ELEMENTS`, so they fit a `usize`.
        let (src, dst) = (src as usize, dst as usize);
        out[dst..dst + run].copy_from_slice(&block[src..src + run]);
        let mut axis = last;
        loop {
            if axis == 0 {
                return;
            }
            axis -= 1;
            at[axis] += 1;
            if at[axis] < hi[axis] {
                break;
            }
            at[axis] = lo[axis];
        }
    }
}

#[cfg(feature = "codecs")]
fn read<O: ObjectSource>(
    objects: &O,
    array: &StoredArray,
    region: &[Range<u64>],
) -> Result<Vec<Option<f64>>, FieldglassError> {
    use crate::ChunkDecoder;

    let meta = &array.meta;
    let grid = meta.grid();
    // Rank and bounds are checked here, before anything is sized or fetched.
    let covering = grid.chunks_covering(region)?;
    let lens: Vec<u64> = region
        .iter()
        .map(|r| r.end.saturating_sub(r.start))
        .collect();
    let count = element_count(&lens)?;
    if covering.is_empty() {
        return Ok(Vec::new());
    }

    // Built per read rather than per array at open: an undecodable codec is
    // this array's failure, and a store full of arrays nobody reads should not
    // pay to build every chain.
    let decoder = ChunkDecoder::from_metadata(meta)?;
    let chunk_shape = grid.chunk_shape();
    let chunk_len = element_count(chunk_shape)?;
    let fill = meta.fill_value();

    let keys = covering
        .iter()
        .map(|index| Ok(format!("{}{}", array.prefix, meta.chunk_key(index)?)))
        .collect::<Result<Vec<String>, FieldglassError>>()?;
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    objects.prefetch(&refs)?;

    let mut out = vec![fill; count];
    for (index, key) in covering.iter().zip(&keys) {
        // Absent is the fill value, which `out` already holds.
        let Some(stored) = objects.get(key)? else {
            continue;
        };
        let block = if decoder.is_sharded() {
            shard_block(&decoder, &stored, chunk_shape, fill)?
        } else {
            decoder
                .decode_raw_values(&stored)?
                .into_iter()
                .map(Some)
                .collect()
        };
        if block.len() != chunk_len {
            return Err(FieldglassError::Parse(format!(
                "chunk {key:?} decoded to {} elements, not the {chunk_len} its shape holds",
                block.len()
            )));
        }
        let origin: Vec<u64> = index.iter().zip(chunk_shape).map(|(i, c)| i * c).collect();
        copy_block(&block, chunk_shape, &origin, region, &lens, &mut out);
    }
    Ok(out)
}

/// One shard, as the full block of values it covers: each inner chunk it
/// holds in its place, and the fill value where it holds none.
#[cfg(feature = "codecs")]
fn shard_block(
    decoder: &crate::ChunkDecoder,
    stored: &[u8],
    shard_shape: &[u64],
    fill: Option<f64>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let shard = decoder.decode_shard(stored)?;
    let inner: Vec<u64> = decoder
        .chain()
        .sharding()
        .map(|sharding| sharding.chunk_shape.iter().map(|&n| n as u64).collect())
        .ok_or_else(|| {
            FieldglassError::WrongLayout("this array's chunks are not shards".to_string())
        })?;
    let per_shard: Vec<u64> = shard.chunks_per_shard().iter().map(|&n| n as u64).collect();
    let whole: Vec<Range<u64>> = shard_shape.iter().map(|&n| 0..n).collect();
    let mut out = vec![fill; element_count(shard_shape)?];
    for position in 0..shard.len() {
        let Some(bytes) = shard.chunk(position) else {
            continue;
        };
        let values: Vec<Option<f64>> = decoder
            .dtype()
            .read_values(bytes)?
            .into_iter()
            .map(Some)
            .collect();
        // The inner chunks are in the C order of the inner grid.
        let mut rest = position as u64;
        let mut origin = vec![0u64; per_shard.len()];
        for axis in (0..per_shard.len()).rev() {
            origin[axis] = (rest % per_shard[axis]) * inner[axis];
            rest /= per_shard[axis];
        }
        copy_block(&values, &inner, &origin, &whole, shard_shape, &mut out);
    }
    Ok(out)
}

#[cfg(not(feature = "codecs"))]
fn read<O: ObjectSource>(
    _objects: &O,
    _array: &StoredArray,
    _region: &[Range<u64>],
) -> Result<Vec<Option<f64>>, FieldglassError> {
    Err(FieldglassError::UnsupportedSection(
        "this build of fieldglass-zarr has no `codecs` feature, so it reads metadata \
         and cannot decode a chunk"
            .to_string(),
    ))
}

#[cfg(all(test, feature = "codecs"))]
mod tests {
    use super::*;

    /// Copying a block into a region takes exactly the intersection, in the
    /// right places, for a block that straddles the region's corner.
    #[test]
    fn a_block_copies_only_its_intersection_with_the_region() {
        // A 2x3 block at (2, 3), values 0..6, into the region rows 1..3,
        // columns 4..7 of some larger array.
        let block: Vec<Option<f64>> = (0..6).map(|v| Some(f64::from(v))).collect();
        let region = [1..3, 4..7];
        let lens = [2, 3];
        let mut out = vec![None; 6];
        copy_block(&block, &[2, 3], &[2, 3], &region, &lens, &mut out);
        // Only row 2 of the array (the block's first row) is in the region,
        // and within it columns 4 and 5 (block columns 1 and 2).
        assert_eq!(out, vec![None, None, None, Some(1.0), Some(2.0), None]);
    }

    /// A rank-0 array is one value, and a region over it has no axes.
    #[test]
    fn a_zero_dimensional_block_is_its_one_value() {
        let mut out = vec![None];
        copy_block(&[Some(7.0)], &[], &[], &[], &[], &mut out);
        assert_eq!(out, vec![Some(7.0)]);
    }

    /// The cap applies to the product, checked, not to any one extent.
    #[test]
    fn a_region_past_the_cap_is_refused_before_it_is_allocated() {
        assert!(element_count(&[1 << 13, 1 << 13]).is_ok());
        assert!(element_count(&[1 << 14, 1 << 14]).is_err());
        assert!(element_count(&[u64::MAX, 2]).is_err());
        assert_eq!(element_count(&[]).unwrap(), 1);
    }
}
