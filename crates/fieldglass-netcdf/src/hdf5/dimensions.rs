//! NetCDF-4 dimension-scale resolution — the semantic layer that turns the raw
//! HDF5 object model into named, shared dimensions and per-variable ordered
//! dimension lists (#174, under #33; decision 0003).
//!
//! NetCDF-4 represents a shared dimension as an HDF5 **dimension scale**: a
//! dataset carrying `CLASS = "DIMENSION_SCALE"`, a `NAME`, and a `_Netcdf4Dimid`.
//! A dimension that also has coordinate values is a *coordinate variable* (its
//! `NAME` is the variable name); a dimension with no coordinate variable is a
//! char placeholder whose `NAME` begins with
//! `"This is a netCDF dimension but not a netCDF variable."`. Every *variable*
//! dataset carries a `DIMENSION_LIST` — a variable-length array of object
//! references, one per axis — that names its dimensions in order.
//!
//! This module reads that convention over the **whole file** — the root group
//! and every nested group, descended depth-first (#219) — and exposes it as
//! [`Hdf5Metadata`], shaped so the napi layer can build the same
//! dimensions / variables / attributes tables the classic backing produces.
//! Objects in nested groups carry a path-qualified name (`/PRODUCT/qa_value`);
//! a `DIMENSION_LIST` reference resolves to the referenced scale wherever it
//! lives in the tree. Layouts outside the decoded subset (a `DIMENSION_LIST`
//! that isn't a vlen of object references) return a clear error rather than a
//! silent misread, matching the rest of the HDF5 reader.
//!
//! A single *dataset* whose datatype is outside the decoded subset is the one
//! thing that does not fail the file: it is skipped and listed in
//! [`Hdf5Metadata::unsupported`], so a station-record file or a TROPOMI granule
//! resolves everything but that variable (#550). So is a dataset whose string
//! attributes pass the text one listing reads: it is listed there with the
//! attributes it lacks, rather than described without them (#1556).
//!
//! Reference: HDF5 file format specification version 3; NetCDF User's Guide,
//! "NetCDF-4 File Format"; Unidata, "NetCDF-4 use of dimension scales".

use std::collections::{BTreeMap, HashMap};

use super::attribute::{self, Hdf5Attribute, HeapText};
use super::dataset;
use super::datatype::{self, VlenBase};
use super::global_heap;
use super::group::{self, ChildKind};
use super::object_header::read_uint_le;
use super::source::ClaimedRanges;
use super::{Hdf5Probe, root_group_address};
use crate::classic::NcType;
// The netCDF-C naming rule for anonymous axes, in core since #704 so the Zarr
// walker names an unnamed axis the same way.
use fieldglass_core::FieldglassError;
use fieldglass_core::array::{LeftOutCause, PhonyDimensions};
use fieldglass_core::bytes::{ByteSource, checked_usize};

/// The `NAME` prefix netCDF-4 writes on a dimension scale that has **no**
/// coordinate variable. The real attribute appends padding and the dimension
/// length (e.g. `"…not a netCDF variable.         2"`), so match by prefix.
const PURE_DIMENSION_NAME_PREFIX: &str = "This is a netCDF dimension but not a netCDF variable.";

/// NetCDF-4 / HDF5 attributes that are machinery, not user metadata. Hidden from
/// the variable / global attribute tables so the view matches `ncdump -h`.
const HIDDEN_ATTRIBUTES: &[&str] = &[
    "CLASS",
    "NAME",
    "DIMENSION_LIST",
    "REFERENCE_LIST",
    "_Netcdf4Dimid",
    "_Netcdf4Coordinates",
    "_NCProperties",
];

/// One resolved NetCDF-4 dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimensionInfo {
    /// The dimension's name, path-qualified when it lives in a nested group.
    pub name: String,
    /// The dimension's length — for the unlimited axis, its current extent.
    pub length: u64,
    /// `true` for the unlimited (`H5S_UNLIMITED`) dimension — the record axis.
    pub is_unlimited: bool,
}

/// One resolved NetCDF-4 variable: a coordinate variable or a plain data
/// variable. Pure dimensions (placeholder scales with no values) are *not*
/// variables and appear only in [`Hdf5Metadata::dimensions`].
#[derive(Debug, Clone, PartialEq)]
pub struct VariableInfo {
    /// The variable's name, path-qualified when it lives in a nested group.
    pub name: String,
    /// The variable's element type, mapped onto the classic vocabulary.
    pub nc_type: NcType,
    /// Ordered dimension names, resolved from `DIMENSION_LIST` (or the scale's
    /// own name for a coordinate variable).
    pub dimensions: Vec<String>,
    /// User attributes, with the NetCDF-4 machinery attributes filtered out.
    pub attributes: Vec<Hdf5Attribute>,
    /// `true` when the variable is also a dimension scale (a coordinate variable).
    pub is_coordinate: bool,
    /// Index into [`crate::NetcdfReader::decode_variable_raw`] — the variable's
    /// position in the whole-file depth-first dataset list, *pure dimensions
    /// included*. This is a different index space from this variable's position in
    /// [`Hdf5Metadata::variables`] (which excludes pure dimensions), so the render
    /// path must use this field, not the `variables` index.
    pub decode_index: usize,
}

/// A dataset the reader could not describe, and why. It would have been a
/// variable: a station-record file or an OMI / TROPOMI granule stores one in an
/// HDF5 compound or variable-length datatype, which is outside the subset
/// [`super::datatype::decode`] maps to an [`NcType`].
///
/// Reported rather than fatal (#550). One such dataset used to fail metadata
/// resolution for the whole file, so a host showed nothing instead of
/// everything but the one variable. Only a dataset that fails with
/// [`FieldglassError::UnsupportedSection`] lands here; a genuine parse failure
/// still fails the file, so "not implemented" and "corrupt" stay
/// distinguishable.
///
/// A dataset whose string attributes a listing cap left out lands here too
/// (#1556): its description is incomplete, and a `calendar` or `units` read
/// as absent would be read wrong, so it is not offered as a variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedVariable {
    /// The dataset's name, path-qualified when it lives in a nested group —
    /// the same spelling a decodable variable would carry.
    pub name: String,
    /// Why it was skipped, as the decoder phrased it (e.g.
    /// `"unsupported section: HDF5 datatype class 6 (compound)"`).
    pub reason: String,
    /// Why, as a code a host can word for itself (#1129): the datatype, when
    /// that is what was declined, and [`LeftOutCause::Unsupported`] for
    /// anything else.
    pub cause: LeftOutCause,
}

/// The fully resolved metadata for a NetCDF-4 / HDF5 file — every group,
/// descended depth-first — the HDF5 analog of the classic header, ready for
/// the napi `DatasetMeta`. Objects in nested groups carry a path-qualified name.
///
/// `variables` is sorted by name and **excludes** pure dimensions (placeholder
/// scales with no coordinate values). This is a different index space from
/// [`crate::NetcdfReader::decode_variable_raw`], which indexes *all* datasets
/// across the file, pure dimensions included; each [`VariableInfo`] therefore
/// carries its own [`VariableInfo::decode_index`], and the render path must use
/// that rather than the variable's position in this list.
#[derive(Debug, Clone, PartialEq)]
pub struct Hdf5Metadata {
    /// Every dimension in the file, pure dimensions included.
    pub dimensions: Vec<DimensionInfo>,
    /// The root group's user attributes.
    pub global_attributes: Vec<Hdf5Attribute>,
    /// Every variable in the file, sorted by name and excluding pure
    /// dimensions — see the type doc for why the index space differs.
    pub variables: Vec<VariableInfo>,
    /// The datasets that were skipped because their datatype is outside the
    /// decoded subset, or because a listing cap left some of their attributes
    /// out (#1556), in the order they were met. Empty for a file every
    /// dataset of which resolved. A host should surface these: the file's
    /// remaining metadata is complete, but this list is the difference between
    /// it and the file.
    pub unsupported: Vec<UnsupportedVariable>,
}

/// The attributes a listing could not read but whose objects it still
/// describes, each named with why (#1604).
///
/// A NetCDF-4 string attribute can be lost to a damaged global-heap entry, to
/// the 16 MiB of string text one listing keeps, or to the cap on one heap
/// object (#1556, #1569). A variable that loses anything that changes what is
/// drawn is left out, in [`Hdf5Metadata::unsupported`]. One that loses only
/// descriptive text, such as `long_name`, is kept without it, and so are the
/// global attributes that did fit. This is where those losses are named, so
/// a host can say what it shows the file without.
///
/// Beside [`Hdf5Metadata`] and [`crate::DatasetView`] rather than a field of
/// each, because both are exhaustive structs a caller can build, and a new
/// field would break that caller in a patch release. Both describe the same
/// names, so one of these answers for either.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct AttributesNotRead {
    /// Which global attributes were not read, and why, or `None` when every
    /// one was. The file opens with the rest.
    pub global: Option<String>,
    /// Each variable listed without some of its attributes, by the name the
    /// metadata and the view give it (`/PRODUCT/x` for a nested one): which
    /// attributes, and why. A variable not in it lost none.
    pub variables: BTreeMap<String, String>,
}

impl AttributesNotRead {
    /// Why some of `variable`'s attributes are missing, by the name the view
    /// gives it.
    pub fn variable(&self, variable: &str) -> Option<&str> {
        self.variables.get(variable).map(String::as_str)
    }

    /// Whether every attribute of everything described was read.
    pub fn is_empty(&self) -> bool {
        self.global.is_none() && self.variables.is_empty()
    }
}

/// A dimension scale, keyed in the table by its object-header address so a
/// `DIMENSION_LIST` reference can resolve back to its name.
#[derive(Clone)]
struct DimScale {
    name: String,
    length: u64,
    is_unlimited: bool,
    /// `_Netcdf4Dimid`, if present; else `None` (assigned by discovery order).
    dimid: Option<i64>,
    /// `false` for the pure-dimension placeholder (no coordinate variable).
    has_coordinate_values: bool,
}

/// Per-dataset facts gathered in one header walk, reused across both passes.
#[derive(Clone)]
struct DatasetInfo {
    address: u64,
    name: String,
    nc_type: NcType,
    /// The dataspace extents, one per axis. Kept whole rather than reduced to a
    /// rank because a dataset with no dimension scales has nowhere else to get
    /// its axis lengths from ([`PhonyDimensions`]).
    extents: Vec<u64>,
    /// Which of those axes the writer declared `H5S_UNLIMITED`, per axis.
    unlimited_axes: Vec<bool>,
    attributes: Vec<Hdf5Attribute>,
    scale: Option<DimScale>,
    /// The raw `DIMENSION_LIST`, read with the other attributes, for a dataset
    /// that is not itself a scale.
    dimension_list: Option<attribute::RawAttribute>,
}

/// Resolve the root group's dimensions, variables, and global attributes.
///
/// A dataset whose datatype is outside the decoded subset (compound, enum,
/// variable-length, opaque, array) is **skipped and recorded** in
/// [`Hdf5Metadata::unsupported`] rather than failing the file (#550) — and it
/// keeps its slot in the dataset list, because
/// [`crate::NetcdfReader::decode_variable_raw`] indexes that same list and
/// closing the gap would silently renumber every variable after it. So is a
/// dataset some of whose attributes were not read: string text past the
/// listing's budget (#1556), a damaged global-heap reference to it, or one
/// attribute message past the cap on one heap object (#1569). Its dimension
/// scale still names its dimension. A dataset that lost only descriptive
/// text (`long_name`, a note: see `attribute::Unread::leaves_out_the_object`)
/// is kept instead, without it, since nothing drawn changes. The global
/// attributes are read before any dataset, and the file opens with the ones
/// that fit. [`resolve_with_unread`] names what both of those lost (#1604);
/// this drops that report. Any other
/// failure still propagates: a file that does not parse is a different thing
/// from a file carrying a type this build does not implement.
pub fn resolve<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<Hdf5Metadata, FieldglassError> {
    resolve_with_unread(source, probe).map(|(meta, _)| meta)
}

/// [`resolve`], with the attributes it could not read on the objects it
/// still describes: the global attributes that did not fit or were damaged,
/// and each kept variable's lost descriptive text (#1604). The same walk, so
/// asking costs nothing more.
pub fn resolve_with_unread<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<(Hdf5Metadata, AttributesNotRead), FieldglassError> {
    let mut unsupported = Vec::new();
    // The file ranges the listing has read: every object's dense attribute
    // storage is claimed in it, so objects sharing one store are refused
    // rather than each reading all of it (#921).
    let mut file = ClaimedRanges::default();
    // Every object's string attributes are read through one reader, so a
    // collection they share is scanned once and their text is held to one
    // budget (#1549).
    let mut text = HeapText::new(source, probe);
    // An object reached by two hard links is one object, described once and
    // listed under each name. Described again, its attribute storage would
    // be claimed twice.
    let mut seen: HashMap<u64, Result<usize, String>> = HashMap::new();
    // The code beside each declined object's reason (#1129). A map of its own
    // rather than a wider `seen` entry, so a file that declines nothing pays
    // nothing for it: `seen` is sized for every object, and this stays empty.
    let mut declined: HashMap<u64, LeftOutCause> = HashMap::new();
    // Why each object whose attributes a listing cap or damage left unread
    // is left out (#1556, #1569), a map of its own for the reason `declined`
    // is.
    let mut unread: HashMap<u64, String> = HashMap::new();
    // What each object kept without some descriptive attributes lost, and
    // why (#1604): named on the variable rather than leaving it out.
    let mut kept_without: HashMap<u64, String> = HashMap::new();
    let mut datasets: Vec<Option<DatasetInfo>> = Vec::new();
    let children = group::all_children(source, probe)?;
    // The global attributes first, so they are charged to the text budget
    // before any variable is. One the budget leaves out is dropped, named in
    // the report beside the metadata (#1604), and the file opens with the
    // rest; a variable past the budget is still left out. Read last, a file
    // whose variables had used the budget up would lose even a short
    // `Conventions`.
    let root = root_group_address(source, probe)?;
    let globals = attribute::message_bodies_in(source, root, probe, &mut file)?.parse(&mut text)?;
    let global_attributes = visible_attributes(&globals.attributes);
    let global_unread = globals.unread.as_ref().map(attribute::Unread::reason);
    for child in children.iter().filter(|c| c.kind == ChildKind::Dataset) {
        let described = match seen.get(&child.object_header_address) {
            Some(Ok(first)) => datasets[*first]
                .clone()
                .map(|info| info.renamed(&child.name)),
            Some(Err(reason)) => {
                unsupported.push(UnsupportedVariable {
                    name: child.name.clone(),
                    reason: reason.clone(),
                    cause: declined
                        .get(&child.object_header_address)
                        .cloned()
                        .unwrap_or(LeftOutCause::Unsupported),
                });
                None
            }
            None => match describe(source, probe, child.clone(), (&mut file, &mut text)) {
                Ok((info, cut)) => {
                    seen.insert(child.object_header_address, Ok(datasets.len()));
                    if let Some(cut) = cut {
                        let into = if cut.leaves_out_the_object() {
                            &mut unread
                        } else {
                            &mut kept_without
                        };
                        into.insert(child.object_header_address, cut.reason());
                    }
                    Some(info)
                }
                Err(e @ FieldglassError::UnsupportedSection(_)) => {
                    let reason = e.to_string();
                    let cause =
                        dataset::unsupported_cause(source, child.object_header_address, probe, &e)
                            .unwrap_or(LeftOutCause::Unsupported);
                    seen.insert(child.object_header_address, Err(reason.clone()));
                    declined.insert(child.object_header_address, cause.clone());
                    unsupported.push(UnsupportedVariable {
                        name: child.name.clone(),
                        reason,
                        cause,
                    });
                    None
                }
                Err(other) => return Err(other),
            },
        };
        // A dataset whose attributes a cap cut is left out under every name
        // it is reached by, its slot kept as a declined one's is. Its scale
        // is kept, so the dimension it names, and the variables along it,
        // still resolve. A pure-dimension placeholder is a dimension, not a
        // variable, and none of its attributes are shown, so a cut there
        // loses nothing and is not reported as a variable left out.
        if let Some(reason) = unread
            .get(&child.object_header_address)
            .filter(|_| described.as_ref().is_some_and(DatasetInfo::is_variable))
        {
            unsupported.push(UnsupportedVariable {
                name: child.name.clone(),
                reason: reason.clone(),
                cause: LeftOutCause::Unsupported,
            });
        }
        datasets.push(described);
    }

    // Pass 1: a table from each scale's object-header address to its name, plus
    // the ordered dimension list.
    let mut name_by_address: HashMap<u64, String> = HashMap::new();
    for d in datasets.iter().flatten() {
        if let Some(scale) = &d.scale {
            name_by_address.insert(d.address, scale.name.clone());
        }
    }
    // Pass 2: resolve every dataset's ordered dimension names, walking in *name*
    // order rather than the depth-first order `datasets` arrives in. Only the
    // anonymous fallback cares, but it cares exactly: netCDF-C numbers the
    // dimensions it invents in name order, and matching that is what keeps
    // `ncdump -h` and Fieldglass calling the same axis `phony_dim_0`. Results are
    // stored back against each dataset's own position, so the decode order below
    // is untouched.
    let mut by_name: Vec<(usize, &DatasetInfo)> = datasets
        .iter()
        .enumerate()
        .filter_map(|(index, d)| d.as_ref().map(|d| (index, d)))
        .collect();
    by_name.sort_by(|(_, a), (_, b)| a.name.cmp(&b.name));
    let mut phony = PhonyDimensions::default();
    let mut dimension_names: Vec<Vec<String>> = vec![Vec::new(); datasets.len()];
    // One scan of each global-heap collection, however many variables refer
    // into it (#921).
    let mut collections = global_heap::Collections::default();
    for (index, d) in by_name {
        dimension_names[index] = resolve_variable_dimensions(
            source,
            probe,
            d,
            &name_by_address,
            (&mut phony, &mut collections),
        )?;
    }

    let dimensions = build_dimensions(&datasets, &phony);

    // Pass 3: classify each dataset. `datasets` is
    // in the same whole-file depth-first order `decode_variable_raw` indexes
    // (`hdf5_dataset_address` walks the identical `list_all_children` filter), so
    // the enumerate position is the variable's decode index — recorded now because
    // it survives the by-name sort below, where the vector position no longer does.
    let mut variables = Vec::new();
    let mut not_read = AttributesNotRead {
        global: global_unread,
        variables: BTreeMap::new(),
    };
    for (decode_index, d) in datasets.iter().enumerate() {
        // A skipped dataset holds its slot so the indices after it don't move.
        let Some(d) = d else { continue };
        // The pure-dimension placeholder is a dimension, not a variable, and
        // one whose attributes were cut is in `unsupported`.
        if !d.is_variable() || unread.contains_key(&d.address) {
            continue;
        }
        let dims = std::mem::take(&mut dimension_names[decode_index]);
        // Under every name it is listed by, as a left-out one is.
        if let Some(reason) = kept_without.get(&d.address) {
            not_read.variables.insert(d.name.clone(), reason.clone());
        }
        variables.push(VariableInfo {
            name: d.name.clone(),
            nc_type: d.nc_type,
            dimensions: dims,
            attributes: visible_attributes(&d.attributes),
            is_coordinate: d.scale.is_some(),
            decode_index,
        });
    }
    variables.sort_by(|a, b| a.name.cmp(&b.name));

    Ok((
        Hdf5Metadata {
            dimensions,
            global_attributes,
            variables,
            unsupported,
        },
        not_read,
    ))
}

/// Gather one dataset's name, element type, rank, attributes, and — if it is a
/// dimension scale — its scale entry, from a single header walk.
///
/// The attributes are read once, dense storage claimed in `file`, string text
/// read through the listing's `text`, and the raw `DIMENSION_LIST` is taken
/// from the same read. Beside the facts, the attributes a listing cap left
/// out, if any (#1556).
fn describe<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
    child: group::GroupChild,
    (file, text): (&mut ClaimedRanges, &mut HeapText<'_, S>),
) -> Result<(DatasetInfo, Option<attribute::Unread>), FieldglassError> {
    let shape = dataset::describe(source, child.object_header_address, probe)?;
    let bodies = attribute::message_bodies_in(source, child.object_header_address, probe, file)?;
    let attribute::Listed { attributes, unread } = bodies.parse(text)?;
    let attr = |name: &str| attributes.iter().find(|a| a.name == name);

    let is_scale = attr("CLASS").is_some_and(|a| a.value == "DIMENSION_SCALE");
    let scale = is_scale.then(|| {
        let placeholder =
            attr("NAME").is_some_and(|a| a.value.starts_with(PURE_DIMENSION_NAME_PREFIX));
        DimScale {
            name: child.name.clone(),
            // A dimension scale is 1-D; fall back to 0 for a malformed scalar.
            length: shape.dataspace.dims.first().copied().unwrap_or(0),
            is_unlimited: shape.dataspace.max_dims.iter().any(Option::is_none),
            dimid: attr("_Netcdf4Dimid")
                .and_then(|a| a.first_value())
                .map(|v| v as i64),
            has_coordinate_values: !placeholder,
        }
    });
    // A coordinate variable's axis is itself; any other reads its list.
    let dimension_list = if scale.is_none() {
        attribute::raw_in_bodies(&bodies.messages, "DIMENSION_LIST", probe.length_size)?
    } else {
        None
    };

    let info = DatasetInfo {
        address: child.object_header_address,
        name: child.name,
        nc_type: shape.datatype.nc_type,
        // A missing max-dims block means no axis is extensible, so an absent
        // entry reads as bounded rather than defaulting the other way.
        unlimited_axes: (0..shape.dataspace.dims.len())
            .map(|axis| {
                shape
                    .dataspace
                    .max_dims
                    .get(axis)
                    .is_some_and(Option::is_none)
            })
            .collect(),
        extents: shape.dataspace.dims,
        attributes,
        scale,
        dimension_list,
    };
    Ok((info, unread))
}

impl DatasetInfo {
    /// Whether this is a variable: anything but the pure-dimension
    /// placeholder, which is a dimension only.
    fn is_variable(&self) -> bool {
        !matches!(&self.scale, Some(s) if !s.has_coordinate_values)
    }

    /// The same dataset under another of its names: a second hard link to it.
    /// A scale's dimension is named by the link it is reached through, as
    /// describing it again under that name did.
    fn renamed(mut self, name: &str) -> Self {
        name.clone_into(&mut self.name);
        if let Some(scale) = &mut self.scale {
            name.clone_into(&mut scale.name);
        }
        self
    }
}

/// Build the dimension list, ordered by `_Netcdf4Dimid` (falling back to
/// discovery order for scales an older writer left without one), then the
/// anonymous dimensions invented for datasets that declared none.
fn build_dimensions(
    datasets: &[Option<DatasetInfo>],
    phony: &PhonyDimensions,
) -> Vec<DimensionInfo> {
    let mut scales: Vec<(i64, DimensionInfo)> = datasets
        .iter()
        .flatten()
        .filter_map(|d| d.scale.as_ref())
        .enumerate()
        .map(|(discovery, s)| {
            (
                // Sort key only, and `discovery` counts dimension scales
                // already held in memory, so the narrowing cannot wrap.
                s.dimid.unwrap_or(discovery as i64),
                DimensionInfo {
                    name: s.name.clone(),
                    length: s.length,
                    is_unlimited: s.is_unlimited,
                },
            )
        })
        .collect();
    scales.sort_by_key(|(dimid, _)| *dimid);
    // Declared dimensions first: a `_Netcdf4Dimid` orders only its own writer's
    // dimensions, and says nothing about where an invented one belongs.
    let mut dimensions: Vec<DimensionInfo> = scales.into_iter().map(|(_, dim)| dim).collect();
    dimensions.extend(phony.dimensions().into_iter().map(|d| DimensionInfo {
        name: d.name,
        length: d.length,
        is_unlimited: d.unlimited,
    }));
    dimensions
}

/// Resolve a variable's ordered dimension names. A coordinate variable's single
/// dimension is itself; any other variable reads its `DIMENSION_LIST`. A dataset
/// with neither (a bare HDF5 dataset, not written by netCDF) falls back to
/// anonymous `phony_dim_N` axes sized from its dataspace by [`PhonyDimensions`],
/// which is what lets such a file be rendered at all: an axis whose name is not
/// in the dimension table resolves to length 0.
fn resolve_variable_dimensions<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
    d: &DatasetInfo,
    name_by_address: &HashMap<u64, String>,
    (phony, collections): (&mut PhonyDimensions, &mut global_heap::Collections),
) -> Result<Vec<String>, FieldglassError> {
    if d.scale.is_some() {
        // A coordinate variable carries no DIMENSION_LIST; its axis is its own
        // dimension.
        return Ok(vec![d.name.clone()]);
    }
    match &d.dimension_list {
        Some(raw) => decode_dimension_list(
            source,
            probe,
            raw,
            (d.extents.len(), name_by_address),
            collections,
        ),
        None => Ok(phony.axes_for(&d.extents, &d.unlimited_axes)),
    }
}

/// Decode a `DIMENSION_LIST` attribute (a vlen of object references) into the
/// ordered names of the dimensions each axis is attached to. `rank` is the
/// variable's own rank; the attribute must carry one axis per dimension.
fn decode_dimension_list<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
    raw: &attribute::RawAttribute,
    (rank, name_by_address): (usize, &HashMap<u64, String>),
    collections: &mut global_heap::Collections,
) -> Result<Vec<String>, FieldglassError> {
    let vlen = datatype::decode_vlen(&raw.datatype_bytes)?;
    let reference = match vlen.base {
        VlenBase::Reference(reference) if vlen.is_sequence => reference,
        _ => {
            return Err(FieldglassError::Parse(
                "DIMENSION_LIST is not a variable-length array of object references".into(),
            ));
        }
    };

    let o = probe.offset_size as usize;
    // One axis per dataspace element; each on-disk vlen element is
    // length(4) + global-heap collection address(offset_size) + object index(4).
    let axes = if raw.dataspace.is_scalar {
        1
    } else {
        checked_usize(
            raw.dataspace.dims.first().copied().unwrap_or(0),
            "HDF5 dimension-list length",
        )?
    };
    if axes != rank {
        return Err(FieldglassError::Parse(format!(
            "DIMENSION_LIST has {axes} axes but the variable has rank {rank}"
        )));
    }
    let elem_width = 4 + o + 4;

    let mut names = Vec::with_capacity(axes);
    for axis in 0..axes {
        let base = axis * elem_width;
        let count = read_uint_le(&raw.data, base, 4)?;
        if count == 0 {
            return Err(FieldglassError::Parse(
                "DIMENSION_LIST axis references no dimension".into(),
            ));
        }
        let collection_addr = read_uint_le(&raw.data, base + 4, o)?;
        // The on-disk object index is 4 bytes; the global heap stores it as a
        // u16, so a value that wouldn't fit is a malformed ID, not a silent wrap.
        let object_index =
            u16::try_from(read_uint_le(&raw.data, base + 4 + o, 4)?).map_err(|_| {
                FieldglassError::Parse("DIMENSION_LIST global-heap object index exceeds u16".into())
            })?;
        let object =
            collections.locate(source, collection_addr, object_index, probe.length_size)?;
        // The object is the axis's `count` references, each the reference
        // datatype's size: 8 bytes as libhdf5 writes one, whatever the size
        // of offsets, the address padded to it. A count either side of the
        // object's size is a malformed ID, refused rather than read short or
        // past (#1557), as libhdf5 refuses it ("Expected global heap object
        // size does not match"). Checked before the cap, as a string's
        // length is, so damage is reported as damage.
        if count.checked_mul(u64::from(reference.size)) != Some(object.size()) {
            return Err(FieldglassError::Parse(
                "DIMENSION_LIST axis length does not match its global-heap object".into(),
            ));
        }
        // Only the first reference is read below, so this read is not what
        // the cap guards. It stays so a global-heap object past 16 MiB is
        // refused as too large wherever it is named (ADR-0016 decision 5).
        object.checked_size()?;
        // netCDF-4 attaches exactly one scale per axis; take the first
        // reference, and read only that: many variables may name one large
        // object, and each would read all of it (#1569).
        let referenced = read_uint_le(&object.read_prefix(source, o)?, 0, o)?;
        let name = name_by_address.get(&referenced).ok_or_else(|| {
            FieldglassError::Parse(
                "DIMENSION_LIST references an object that is not a known dimension scale".into(),
            )
        })?;
        names.push(name.clone());
    }
    Ok(names)
}

/// Drop the NetCDF-4 machinery attributes, leaving user-facing metadata.
fn visible_attributes(attrs: &[Hdf5Attribute]) -> Vec<Hdf5Attribute> {
    attrs
        .iter()
        .filter(|a| !HIDDEN_ATTRIBUTES.contains(&a.name.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdf5::dataspace::Dataspace;

    /// Where the collection starts in [`file`], and the address of the one
    /// dimension scale.
    const COLLECTION: u64 = 16;
    const SCALE: u64 = 0x40;

    /// A file holding, at [`COLLECTION`], a global-heap collection whose
    /// object 1 is `object`, then the free-space object.
    fn file(object: &[u8]) -> Vec<u8> {
        let mut file = vec![0; COLLECTION as usize];
        file.extend_from_slice(b"GCOL");
        file.extend_from_slice(&[1, 0, 0, 0]); // version + reserved
        let size_at = file.len();
        file.extend_from_slice(&0u64.to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&[1, 0, 0, 0, 0, 0]); // reference count + reserved
        file.extend_from_slice(&(object.len() as u64).to_le_bytes());
        file.extend_from_slice(object);
        file.resize(file.len().next_multiple_of(8), 0);
        file.extend_from_slice(&[0; 16]); // free-space object
        let size = (file.len() as u64 - COLLECTION).to_le_bytes();
        file[size_at..size_at + 8].copy_from_slice(&size);
        file
    }

    /// The axes `decode_dimension_list` reads from a rank-1 `DIMENSION_LIST`
    /// whose one heap ID claims `count` references in object 1, in a file
    /// with 4-byte offsets, as `hdf5_sizes_o4_l8.h5` has.
    fn axes(count: u32, object: &[u8]) -> Result<Vec<String>, FieldglassError> {
        axes_in(&file(object), count)
    }

    /// The axes as [`axes`] reads them, from `source`, a [`file`].
    fn axes_in<S: ByteSource + ?Sized>(
        source: &S,
        count: u32,
    ) -> Result<Vec<String>, FieldglassError> {
        let mut reference = vec![0x17, 0, 0, 0]; // class 7, version 1, object
        reference.extend_from_slice(&8u32.to_le_bytes());
        let mut datatype = vec![0x19, 0, 0, 0]; // class 9, version 1, sequence
        datatype.extend_from_slice(&12u32.to_le_bytes());
        datatype.extend_from_slice(&reference);
        let mut data = count.to_le_bytes().to_vec();
        data.extend_from_slice(&(COLLECTION as u32).to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        let raw = attribute::RawAttribute {
            name: "DIMENSION_LIST".into(),
            datatype_bytes: datatype,
            dataspace: Dataspace {
                is_scalar: false,
                dims: vec![1],
                max_dims: vec![Some(1)],
            },
            data,
        };
        let names = HashMap::from([(SCALE, "x".to_string())]);
        decode_dimension_list(
            source,
            &Hdf5Probe::new(0, 4, 8),
            &raw,
            (1, &names),
            &mut global_heap::Collections::default(),
        )
    }

    /// A reference is the reference datatype's size, eight bytes, even where
    /// offsets are four: the address is padded to it. So the object of one
    /// reference is eight bytes, and one of four, the size of an offset, is
    /// refused, as is a count either side of the object's size (#1557).
    #[test]
    fn an_axis_object_is_its_count_of_references_of_the_datatype_size() {
        const WHY: &str = "DIMENSION_LIST axis length does not match its global-heap object";
        let padded = [&SCALE.to_le_bytes()[..4], &[0; 4]].concat();
        assert_eq!(axes(1, &padded).unwrap(), ["x"]);
        let refused = |got: Result<Vec<String>, FieldglassError>| matches!(&got, Err(FieldglassError::Parse(m)) if m == WHY);
        assert!(refused(axes(1, &padded[..4])), "an offset's size");
        assert!(refused(axes(2, &padded)), "a count past the object");
        assert!(refused(axes(1, &padded.repeat(2))), "a count short of it");
        // Damage is reported as damage even on an object past the cap.
        let huge = vec![0; (16 << 20) + 8];
        assert!(refused(axes(1, &huge)), "a count short of a huge object");
    }

    /// Only an axis's first reference is read, not its whole object: many
    /// variables may name one large object, and each read all of it (#1569).
    #[test]
    fn an_axis_reads_only_its_first_reference() {
        const COUNT: usize = 1 << 17;
        let padded = [&SCALE.to_le_bytes()[..4], &[0; 4]].concat();
        let file = file(&padded.repeat(COUNT));
        let source = fieldglass_core::testing::Recording::new(file.as_slice());
        assert_eq!(axes_in(&source, COUNT as u32).unwrap(), ["x"]);
        let largest = source.reads().iter().map(|r| r.len).max().unwrap();
        assert!(largest < 1 << 16, "a {largest}-byte read of a 1 MiB object");
    }
}
