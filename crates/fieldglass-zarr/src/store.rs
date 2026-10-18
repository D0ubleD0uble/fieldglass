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

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{
    ArrayDescription, ArraySource, Attribute, AttributeValue, Dimension, ElementType, Group,
    LeftOut, LeftOutCause, PhonyDimensions,
};
use fieldglass_core::bytes::ObjectSource;
use serde_json::{Map, Value};

use crate::dtype::{DType, ScalarKind};
use crate::metadata::ArrayMetadata;

/// The most elements one region read — or one chunk it decodes — will hold.
///
/// An alias of [`fieldglass_core::MAX_FIELD_POINTS`], which is where the number
/// and the reasoning live: an array's shape and chunk shape are numbers out of a
/// document somebody else wrote, so they are allocation instructions an attacker
/// controls, and one field of them is what a reader materialises. The name stays
/// because it is public API. `u64` because the products compared against it are.
pub const MAX_REGION_ELEMENTS: u64 = fieldglass_core::MAX_FIELD_POINTS as u64;

/// The documents a store's root may hold, prefetched as one batch before the
/// first is read, so finding out which edition and layout a store has costs one
/// round trip rather than one per guess.
const ROOT_KEYS: [&str; 5] = ["zarr.json", ".zmetadata", ".zgroup", ".zarray", ".zattrs"];

/// The v2 metadata documents one directory may hold, in the order
/// [`v2_gather`] wants them.
const V2_DOCS: [&str; 3] = [".zgroup", ".zarray", ".zattrs"];

/// How many directories an unconsolidated walk will list before giving up.
///
/// The walk descends into a directory only when it is not an array, so a
/// well-formed store costs one listing per *group* — a handful. A store whose
/// arrays are missing their `.zarray`, or that nests groups without end, would
/// otherwise cost one listing per directory it holds, and a listing is a
/// request. The number is a store's groups, not its arrays or its chunks, and
/// a hierarchy anywhere near it should be carrying consolidated metadata.
const MAX_LISTED_DIRECTORIES: usize = 4096;

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
    problems: Vec<LeftOut>,
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
    pub fn problems(&self) -> &[LeftOut] {
        &self.problems
    }

    /// The objects a region read of `array` fetches: each chunk's index on the
    /// chunk grid and the key it is stored under, from the store's root, in
    /// row-major order (#1083).
    ///
    /// The same keys [`ArraySource::read_region`] asks the source for, spelled
    /// by the same code, so a host that fetches exactly these and nothing else
    /// holds every object the read will want. A chunk the store never wrote is
    /// still listed: only fetching it can tell that it is absent, and an absent
    /// chunk is the fill value, not an error. For a sharded array the objects
    /// are its shards.
    ///
    /// Metadata only, so it answers without the `codecs` feature.
    ///
    /// # Errors
    ///
    /// An array this store does not read, a region of the wrong rank or past
    /// the array's shape, and a region touching more chunks than one plan may
    /// hold.
    pub fn chunk_keys(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<(Vec<u64>, String)>, FieldglassError> {
        let stored = self.stored(array)?;
        let covering = stored.meta.grid().chunks_covering(region)?;
        let keys = keys_of(stored, &covering)?;
        Ok(covering.into_iter().zip(keys).collect())
    }

    /// The array a read or a plan asks for, or why the store holds no readable
    /// one by that name.
    fn stored(&self, array: &str) -> Result<&StoredArray, FieldglassError> {
        self.arrays.get(array).ok_or_else(|| {
            let why = self
                .problems
                .iter()
                .find(|l| l.name == array)
                .map_or_else(String::new, |l| format!(": {}", l.reason));
            FieldglassError::Parse(format!("this store holds no readable array {array:?}{why}"))
        })
    }
}

/// The keys `covering`'s chunks are stored under, from the store's root.
///
/// The one spelling both [`ZarrStore::chunk_keys`] and the region read use, so
/// a plan and the read it plans for cannot name different objects.
fn keys_of(array: &StoredArray, covering: &[Vec<u64>]) -> Result<Vec<String>, FieldglassError> {
    covering
        .iter()
        .map(|index| Ok(format!("{}{}", array.prefix, array.meta.chunk_key(index)?)))
        .collect()
}

impl<O: ObjectSource> ArraySource for ZarrStore<O> {
    fn group(&self) -> &Group {
        &self.root
    }

    /// What [`Self::problems`] holds, in the shape every container states it
    /// (#709). The two used to be different types — a tuple here and a named
    /// struct in `fieldglass-netcdf` — which left a host reading both to guess
    /// which string was which.
    fn left_out(&self) -> Vec<LeftOut> {
        self.problems.clone()
    }

    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        read(&self.objects, self.stored(array)?, region)
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
/// has one, and by descending the store when it does not.
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

    // Unconsolidated: descend, rather than listing the store. See `Walk`.
    let mut arrays = BTreeSet::new();
    if matches!(nodes.get(""), Some(Node::Array { .. })) {
        arrays.insert(String::new());
    }
    let mut walk = Walk::new();
    while let Some(dirs) = walk.next_level(objects, &arrays)? {
        let keys: Vec<String> = dirs.iter().map(|dir| format!("{dir}zarr.json")).collect();
        prefetch_keys(objects, &keys)?;
        for (dir, key) in dirs.iter().zip(&keys) {
            let Some(document) = document(objects, key)? else {
                continue;
            };
            let path = dir.trim_end_matches('/').to_string();
            let node = v3_node(document);
            if matches!(node, Node::Array { .. }) {
                arrays.insert(path.clone());
            }
            nodes.insert(path, node);
        }
    }
    Ok(nodes)
}

/// One level of an unconsolidated walk at a time.
///
/// The shape both editions share: start at the root, and descend into a
/// directory only when what was found there is *not* an array. That is what
/// keeps a chunk key out of every listing — an array's directory is never
/// listed at all, so neither its chunks nor its chunk rows are ever named — and
/// it is why the walk gets each level's documents in one batch rather than one
/// request per directory.
///
/// A directory the store holds no document for is descended into anyway. v3 has
/// no implicit groups, but zarr-python will happily write an array under a
/// directory that has none, and the listing walk this replaced found it; a
/// directory with no metadata document also holds no chunks to enumerate.
struct Walk {
    frontier: Vec<String>,
    listed: usize,
    started: bool,
}

impl Walk {
    fn new() -> Self {
        Self {
            frontier: Vec::new(),
            listed: 0,
            started: false,
        }
    }

    /// The directories of the next level, or `None` when the walk is done.
    ///
    /// `arrays` holds the paths the levels so far turned out to be arrays,
    /// which is how the walk knows which directories not to descend into.
    fn next_level<O: ObjectSource>(
        &mut self,
        objects: &O,
        arrays: &BTreeSet<String>,
    ) -> Result<Option<Vec<String>>, FieldglassError> {
        let parents = if self.started {
            std::mem::take(&mut self.frontier)
        } else {
            self.started = true;
            vec![String::new()]
        };
        let mut next = Vec::new();
        for parent in parents {
            if arrays.contains(parent.trim_end_matches('/')) {
                continue;
            }
            self.listed += 1;
            if self.listed > MAX_LISTED_DIRECTORIES {
                return Err(FieldglassError::WrongLayout(format!(
                    "this store's hierarchy has more than {MAX_LISTED_DIRECTORIES} \
                     directories that are not arrays, so it is not being walked further"
                )));
            }
            next.extend(
                objects
                    .list_children(&parent)?
                    .into_iter()
                    .filter(|key| key.ends_with('/')),
            );
        }
        if next.is_empty() {
            return Ok(None);
        }
        self.frontier.clone_from(&next);
        Ok(Some(next))
    }
}

/// Prefetch owned keys, which is every batch this walk makes.
fn prefetch_keys<O: ObjectSource>(objects: &O, keys: &[String]) -> Result<(), FieldglassError> {
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    objects.prefetch(&refs)
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

/// Every node of an unconsolidated v2 store, found by descending it.
///
/// One level at a time through [`Walk`], so a directory that turned out to be
/// an array is never listed and its chunk keys are never enumerated. The three
/// documents a directory may hold are asked for rather than looked for: an
/// absent key is `Ok(None)` on this seam, so three speculative gets in one
/// batch cost a remote store less than a listing would.
fn v2_listed_nodes<O: ObjectSource>(
    objects: &O,
) -> Result<BTreeMap<String, Node>, FieldglassError> {
    let mut documents = Vec::new();
    let mut arrays = BTreeSet::new();
    // The root's own documents are already in the `ROOT_KEYS` batch `open`
    // made, so this level costs a remote store nothing.
    read_v2_level(objects, &[String::new()], &mut documents, &mut arrays)?;
    let mut walk = Walk::new();
    while let Some(dirs) = walk.next_level(objects, &arrays)? {
        read_v2_level(objects, &dirs, &mut documents, &mut arrays)?;
    }
    Ok(v2_gather(documents))
}

/// The metadata documents of every directory in one level, in one batch.
///
/// `arrays` gains the directories that hold a `.zarray`, so [`Walk`] knows
/// which of them not to descend into; the tree itself is built from
/// `documents` by [`v2_gather`], which is the one place that reads them.
fn read_v2_level<O: ObjectSource>(
    objects: &O,
    dirs: &[String],
    documents: &mut Vec<(String, Value)>,
    arrays: &mut BTreeSet<String>,
) -> Result<(), FieldglassError> {
    let keys: Vec<String> = dirs
        .iter()
        .flat_map(|dir| V2_DOCS.iter().map(move |doc| format!("{dir}{doc}")))
        .collect();
    prefetch_keys(objects, &keys)?;
    let mut owed = Vec::new();
    for (dir, chunk) in dirs.iter().zip(keys.chunks(V2_DOCS.len())) {
        let mut node = false;
        let mut no_zattrs = None;
        for key in chunk {
            let Some(value) = document(objects, key)? else {
                if key.ends_with(".zattrs") {
                    no_zattrs = Some(key.as_str());
                }
                continue;
            };
            if key.ends_with(".zarray") {
                arrays.insert(dir.trim_end_matches('/').to_string());
            }
            node |= key.ends_with(".zarray") || key.ends_with(".zgroup");
            documents.push((key.clone(), value));
        }
        // A node's `.zattrs` holds its dimension names and CF attributes
        // (`scale_factor`, `_FillValue`), so a source that was never given it
        // would open the node without them and show packed values unscaled.
        // Absent, it is a node with no attributes; never given, it is refused
        // (#1124). Asked only when the document did not come back, so a store
        // that holds it pays nothing here.
        if let (true, Some(zattrs)) = (node, no_zattrs) {
            owed.extend(objects.unfetched(&[zattrs]));
        }
    }
    if !owed.is_empty() {
        let named: Vec<String> = owed.iter().map(|k| format!("`{k}`")).collect();
        return Err(FieldglassError::Parse(format!(
            "this store's source was never given {}, which holds a node's dimension names and \
             CF attributes: fetch it, and give it as absent (a 404) when the store has none",
            named.join(", ")
        )));
    }
    Ok(())
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
fn build(nodes: BTreeMap<String, Node>) -> (Group, BTreeMap<String, StoredArray>, Vec<LeftOut>) {
    let mut groups: BTreeMap<String, GroupParts> = BTreeMap::new();
    let mut arrays = BTreeMap::new();
    let mut problems = Vec::new();
    // One allocator per group, so an unnamed axis is shared only with arrays
    // beside it — the scope a named dimension has here too.
    let mut phony: BTreeMap<String, PhonyDimensions> = BTreeMap::new();
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
        let allocator = phony.entry(home.clone()).or_default();
        let (description, meta) = match describe(&path, &document, &attributes, edition, allocator)
        {
            Ok(described) => described,
            Err(why) => {
                problems.push(LeftOut {
                    cause: cause_of(&why, &document, edition),
                    reason: why.to_string(),
                    name: path,
                });
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
                    .map(|d| LeftOut {
                        name: path.clone(),
                        reason: format!(
                            "dimension {name:?} has length {length} here and {} in \
                             another array of this group",
                            d.length
                        ),
                        cause: LeftOutCause::DimensionClash {
                            dimension: name.clone(),
                        },
                    })
            });
        if let Some(left_out) = clash {
            problems.push(left_out);
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
    phony: &mut PhonyDimensions,
) -> Result<(ArrayDescription, ArrayMetadata), FieldglassError> {
    let meta = ArrayMetadata::from_value(document, edition)?;
    let name = leaf(path).to_string();
    let rank = meta.grid().rank();

    // v3 states the names in the array document; v2 has none, and xarray's
    // convention is an `_ARRAY_DIMENSIONS` attribute. An axis left unnamed —
    // or every axis, when the list is the wrong length — is named the way
    // netCDF-C names an HDF5 dataset's anonymous axes (`phony_dim_N`, shared
    // by extent within the group), so a NetCDF file and a Zarr store of the
    // same shape present the same axes (#704).
    let stated: Vec<Option<String>> = {
        let names = match edition {
            3 => document.get("dimension_names"),
            _ => attributes.get("_ARRAY_DIMENSIONS"),
        }
        .and_then(Value::as_array)
        .filter(|names| names.len() == rank);
        (0..rank)
            .map(|axis| names.and_then(|n| n[axis].as_str()).map(str::to_string))
            .collect()
    };
    let unnamed: Vec<u64> = stated
        .iter()
        .zip(meta.grid().shape())
        .filter(|(name, _)| name.is_none())
        .map(|(_, length)| *length)
        .collect();
    let mut invented = phony.axes_for(&unnamed, &[]).into_iter();
    let dimensions = stated
        .into_iter()
        .map(|name| name.or_else(|| invented.next()).unwrap_or_default())
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

/// Why [`describe`] refused an array's document, as a code (#1129).
///
/// The metadata reader says which of its rules failed only in prose, so the
/// code is worked out here, and from the rule that refused the document
/// rather than from everything wrong with it. The reader checks in a fixed
/// order, so the element type is what refused it exactly when putting a type
/// it does decode in its place changes the answer. Then the type is named,
/// and text is told apart from the rest. Otherwise the refusal's own kind
/// decides: "not implemented" is something else this crate does not read,
/// and anything else is a document that does not parse.
fn cause_of(error: &FieldglassError, document: &Value, edition: u8) -> LeftOutCause {
    let key = if edition == 3 { "data_type" } else { "dtype" };
    if dtype_refused(error, document, edition, key) {
        return dtype_cause(document, edition, key);
    }
    if matches!(error, FieldglassError::UnsupportedSection(_)) {
        LeftOutCause::Unsupported
    } else {
        LeftOutCause::UnreadableMetadata
    }
}

/// Whether the element type is the rule that refused `document`: the same
/// document with a type this crate decodes in its place is refused
/// differently, or not at all.
fn dtype_refused(error: &FieldglassError, document: &Value, edition: u8, key: &str) -> bool {
    let Value::Object(fields) = document else {
        return false;
    };
    let mut replaced = fields.clone();
    let decoded = if edition == 3 { "float32" } else { "<f4" };
    replaced.insert(key.to_string(), Value::String(decoded.to_string()));
    match ArrayMetadata::from_value(&Value::Object(replaced), edition) {
        Ok(_) => true,
        Err(other) => other.to_string() != error.to_string(),
    }
}

/// The code for an element type that refused its document.
fn dtype_cause(document: &Value, edition: u8, key: &str) -> LeftOutCause {
    let named = |dtype: &str| {
        if is_text(dtype, edition, document) {
            LeftOutCause::Text
        } else {
            LeftOutCause::UnsupportedDtype {
                dtype: dtype.to_string(),
            }
        }
    };
    match document.get(key) {
        Some(Value::String(dtype)) => {
            let parsed = if edition == 3 {
                DType::parse_v3(dtype)
            } else {
                DType::parse_v2(dtype)
            };
            // NumPy spells an object array `|O`, with no width, so the grammar
            // refuses it as malformed; it is a real type, and with a string
            // codec it is text. Any other spelling the grammar cannot read is
            // a broken document, not a type this crate leaves alone.
            let object = edition == 2 && dtype.chars().nth(1) == Some('O');
            if object || matches!(parsed, Err(FieldglassError::UnsupportedSection(_))) {
                named(dtype)
            } else {
                LeftOutCause::UnreadableMetadata
            }
        }
        // A v2 structured dtype is a list of fields, and a v3 type with
        // parameters an object naming itself: both valid, neither a number.
        Some(Value::Array(_)) if edition == 2 => LeftOutCause::UnsupportedDtype {
            dtype: "structured".to_string(),
        },
        Some(Value::Object(spec)) if edition == 3 => named(
            spec.get("name")
                .and_then(Value::as_str)
                .unwrap_or("extension"),
        ),
        _ => LeftOutCause::UnreadableMetadata,
    }
}

/// The v3 data types that hold text or bytes: zarr-python 3's
/// `variable_length_utf8` (written `string` before 3.1),
/// `fixed_length_utf32`, `variable_length_bytes` and `null_terminated_bytes`.
/// `raw_bytes` is opaque bytes rather than text, and is left a type.
const V3_TEXT: &[&str] = &[
    "string",
    "variable_length_utf8",
    "fixed_length_utf32",
    "variable_length_bytes",
    "null_terminated_bytes",
];

/// The v2 object codecs that make an `|O` array strings, as zarr-python 2 and
/// xarray write them.
const V2_TEXT_CODECS: &[&str] = &["vlen-utf8", "vlen-bytes"];

/// Whether a dtype spelling names text: NumPy's `U` (unicode) and `S` / `a`
/// (bytes) kinds in v2, or its `O` kind with a string object codec, and
/// [`V3_TEXT`] in v3.
fn is_text(dtype: &str, edition: u8, document: &Value) -> bool {
    if edition == 3 {
        return V3_TEXT.contains(&dtype);
    }
    match dtype.chars().nth(1) {
        Some('U' | 'S' | 'a') => true,
        Some('O') => {
            let codec = |c: &Value| {
                c.get("id")
                    .and_then(Value::as_str)
                    .is_some_and(|id| V2_TEXT_CODECS.contains(&id))
            };
            document
                .get("filters")
                .and_then(Value::as_array)
                .is_some_and(|filters| filters.iter().any(codec))
                || document.get("compressor").is_some_and(codec)
        }
        _ => false,
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
    // Core's check, the one a NetCDF region read makes too, so both reach a
    // host as the same refusal (#942). `MAX_REGION_ELEMENTS` is its cap.
    Ok(fieldglass_core::array::field_element_count(extents)?)
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

    let keys = keys_of(array, &covering)?;
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    // A chunk the source was never given is not one the store left unwritten,
    // so it is not the fill value either (#1124). Named, all of them, so a host
    // fetches them in one round.
    let unfetched = objects.unfetched(&refs);
    if !unfetched.is_empty() {
        let named: Vec<String> = unfetched.iter().map(|k| format!("`{k}`")).collect();
        return Err(FieldglassError::Parse(format!(
            "this read needs {}, which the store's source was never given: fetch them, and \
             give one the store does not hold (a 404) as absent",
            named.join(", ")
        )));
    }
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
        fieldglass_core::array::copy_block(&block, chunk_shape, &origin, region, &mut out);
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
        fieldglass_core::array::copy_block(&values, &inner, &origin, &whole, &mut out);
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
    use fieldglass_core::array::copy_block;

    /// Copying a block into a region takes exactly the intersection, in the
    /// right places, for a block that straddles the region's corner.
    #[test]
    fn a_block_copies_only_its_intersection_with_the_region() {
        // A 2x3 block at (2, 3), values 0..6, into the region rows 1..3,
        // columns 4..7 of some larger array.
        let block: Vec<Option<f64>> = (0..6).map(|v| Some(f64::from(v))).collect();
        let region = [1..3, 4..7];
        let mut out = vec![None; 6];
        copy_block(&block, &[2, 3], &[2, 3], &region, &mut out);
        // Only row 2 of the array (the block's first row) is in the region,
        // and within it columns 4 and 5 (block columns 1 and 2).
        assert_eq!(out, vec![None, None, None, Some(1.0), Some(2.0), None]);
    }

    /// A rank-0 array is one value, and a region over it has no axes.
    #[test]
    fn a_zero_dimensional_block_is_its_one_value() {
        let mut out = vec![None];
        copy_block(&[Some(7.0)], &[], &[], &[], &mut out);
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
