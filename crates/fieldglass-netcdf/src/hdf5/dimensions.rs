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
//! resolves everything but that variable (#550).
//!
//! Reference: HDF5 file format specification version 3; NetCDF User's Guide,
//! "NetCDF-4 File Format"; Unidata, "NetCDF-4 use of dimension scales".

use std::collections::HashMap;

use super::attribute::{self, Hdf5Attribute};
use super::dataset;
use super::datatype::{self, VlenBase};
use super::global_heap;
use super::group::{self, ChildKind};
use super::object_header::read_uint_le;
use super::{Hdf5Probe, root_group_address};
use crate::classic::NcType;
// The netCDF-C naming rule for anonymous axes, in core since #704 so the Zarr
// walker names an unnamed axis the same way.
use fieldglass_core::FieldglassError;
use fieldglass_core::array::PhonyDimensions;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedVariable {
    /// The dataset's name, path-qualified when it lives in a nested group —
    /// the same spelling a decodable variable would carry.
    pub name: String,
    /// Why it was skipped, as the decoder phrased it (e.g.
    /// `"unsupported section: HDF5 datatype class 6 (compound)"`).
    pub reason: String,
}

/// The fully resolved metadata for a NetCDF-4 / HDF5 file — every group,
/// descended depth-first — the HDF5 analogue of the classic header, ready for
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
    /// decoded subset, in the order they were met. Empty for a file every
    /// dataset of which resolved. A host should surface these: the file's
    /// remaining metadata is complete, but this list is the difference between
    /// it and the file.
    pub unsupported: Vec<UnsupportedVariable>,
}

/// A dimension scale, keyed in the table by its object-header address so a
/// `DIMENSION_LIST` reference can resolve back to its name.
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
}

/// Resolve the root group's dimensions, variables, and global attributes.
///
/// A dataset whose datatype is outside the decoded subset (compound, enum,
/// variable-length, opaque, array) is **skipped and recorded** in
/// [`Hdf5Metadata::unsupported`] rather than failing the file (#550) — and it
/// keeps its slot in the dataset list, because
/// [`crate::NetcdfReader::decode_variable_raw`] indexes that same list and
/// closing the gap would silently renumber every variable after it. Any other
/// failure still propagates: a file that does not parse is a different thing
/// from a file carrying a type this build does not implement.
pub fn resolve<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
) -> Result<Hdf5Metadata, FieldglassError> {
    let mut unsupported = Vec::new();
    let datasets: Vec<Option<DatasetInfo>> = group::all_children(source, probe)?
        .iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .map(|child| match describe(source, probe, child.clone()) {
            Ok(info) => Ok(Some(info)),
            Err(e @ FieldglassError::UnsupportedSection(_)) => {
                unsupported.push(UnsupportedVariable {
                    name: child.name.clone(),
                    reason: e.to_string(),
                });
                Ok(None)
            }
            Err(other) => Err(other),
        })
        .collect::<Result<_, _>>()?;

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
    for (index, d) in by_name {
        dimension_names[index] =
            resolve_variable_dimensions(source, probe, d, &name_by_address, &mut phony)?;
    }

    let dimensions = build_dimensions(&datasets, &phony);

    // Pass 3: classify each dataset. `datasets` is
    // in the same whole-file depth-first order `decode_variable_raw` indexes
    // (`hdf5_dataset_address` walks the identical `list_all_children` filter), so
    // the enumerate position is the variable's decode index — recorded now because
    // it survives the by-name sort below, where the vector position no longer does.
    let mut variables = Vec::new();
    for (decode_index, d) in datasets.iter().enumerate() {
        // A skipped dataset holds its slot so the indices after it don't move.
        let Some(d) = d else { continue };
        // The pure-dimension placeholder is a dimension, not a variable.
        if matches!(&d.scale, Some(s) if !s.has_coordinate_values) {
            continue;
        }
        let dims = std::mem::take(&mut dimension_names[decode_index]);
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

    let global_attributes = visible_attributes(&attribute::list_attributes(
        source,
        root_group_address(source, probe)?,
        probe,
    )?);

    Ok(Hdf5Metadata {
        dimensions,
        global_attributes,
        variables,
        unsupported,
    })
}

/// Gather one dataset's name, element type, rank, attributes, and — if it is a
/// dimension scale — its scale entry, from a single header walk.
fn describe<S: ByteSource + ?Sized>(
    source: &S,
    probe: &Hdf5Probe,
    child: group::GroupChild,
) -> Result<DatasetInfo, FieldglassError> {
    let shape = dataset::describe(source, child.object_header_address, probe)?;
    let attributes = attribute::list_attributes(source, child.object_header_address, probe)?;
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

    Ok(DatasetInfo {
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
    })
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
    phony: &mut PhonyDimensions,
) -> Result<Vec<String>, FieldglassError> {
    if d.scale.is_some() {
        // A coordinate variable carries no DIMENSION_LIST; its axis is its own
        // dimension.
        return Ok(vec![d.name.clone()]);
    }
    match attribute::raw_attribute(source, d.address, probe, "DIMENSION_LIST")? {
        Some(raw) => decode_dimension_list(source, probe, &raw, d.extents.len(), name_by_address),
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
    rank: usize,
    name_by_address: &HashMap<u64, String>,
) -> Result<Vec<String>, FieldglassError> {
    let vlen = datatype::decode_vlen(&raw.datatype_bytes)?;
    if !vlen.is_sequence || !matches!(vlen.base, VlenBase::Reference(_)) {
        return Err(FieldglassError::Parse(
            "DIMENSION_LIST is not a variable-length array of object references".into(),
        ));
    }

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
            global_heap::read_object(source, collection_addr, object_index, probe.length_size)?;
        // netCDF-4 attaches exactly one scale per axis; take the first reference.
        let referenced = read_uint_le(&object, 0, o)?;
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
