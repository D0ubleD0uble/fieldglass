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
use fieldglass_core::FieldglassError;

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
    pub name: String,
    pub length: u64,
    /// `true` for the unlimited (`H5S_UNLIMITED`) dimension — the record axis.
    pub is_unlimited: bool,
}

/// One resolved NetCDF-4 variable: a coordinate variable or a plain data
/// variable. Pure dimensions (placeholder scales with no values) are *not*
/// variables and appear only in [`Hdf5Metadata::dimensions`].
#[derive(Debug, Clone, PartialEq)]
pub struct VariableInfo {
    pub name: String,
    pub nc_type: NcType,
    /// Ordered dimension names, resolved from `DIMENSION_LIST` (or the scale's
    /// own name for a coordinate variable).
    pub dimensions: Vec<String>,
    /// User attributes, with the NetCDF-4 machinery attributes filtered out.
    pub attributes: Vec<Hdf5Attribute>,
    /// `true` when the variable is also a dimension scale (a coordinate variable).
    pub is_coordinate: bool,
    /// Index into [`crate::NetcdfReader::decode_variable_values`] — the variable's
    /// position in the whole-file depth-first dataset list, *pure dimensions
    /// included*. This is a different index space from this variable's position in
    /// [`Hdf5Metadata::variables`] (which excludes pure dimensions), so the render
    /// path must use this field, not the `variables` index.
    pub decode_index: usize,
}

/// The fully resolved metadata for a NetCDF-4 / HDF5 file — every group,
/// descended depth-first — the HDF5 analogue of the classic header, ready for
/// the napi `DatasetMeta`. Objects in nested groups carry a path-qualified name.
///
/// `variables` is sorted by name and **excludes** pure dimensions (placeholder
/// scales with no coordinate values). This is a different index space from
/// [`crate::NetcdfReader::decode_variable_values`], which indexes *all* datasets
/// across the file, pure dimensions included; each [`VariableInfo`] therefore
/// carries its own [`VariableInfo::decode_index`], and the render path must use
/// that rather than the variable's position in this list.
#[derive(Debug, Clone, PartialEq)]
pub struct Hdf5Metadata {
    pub dimensions: Vec<DimensionInfo>,
    pub global_attributes: Vec<Hdf5Attribute>,
    pub variables: Vec<VariableInfo>,
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
pub fn resolve(bytes: &[u8], probe: &Hdf5Probe) -> Result<Hdf5Metadata, FieldglassError> {
    let datasets: Vec<DatasetInfo> = group::all_children(bytes, probe)?
        .iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .map(|child| describe(bytes, probe, child.clone()))
        .collect::<Result<_, _>>()?;

    // Pass 1: a table from each scale's object-header address to its name, plus
    // the ordered dimension list.
    let mut name_by_address: HashMap<u64, String> = HashMap::new();
    for d in &datasets {
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
    let mut by_name: Vec<usize> = (0..datasets.len()).collect();
    by_name.sort_by(|&a, &b| datasets[a].name.cmp(&datasets[b].name));
    let mut phony = PhonyDimensions::default();
    let mut dimension_names: Vec<Vec<String>> = vec![Vec::new(); datasets.len()];
    for index in by_name {
        dimension_names[index] = resolve_variable_dimensions(
            bytes,
            probe,
            &datasets[index],
            &name_by_address,
            &mut phony,
        )?;
    }

    let dimensions = build_dimensions(&datasets, &phony);

    // Pass 3: classify each dataset. `datasets` is
    // in the same whole-file depth-first order `decode_variable_values` indexes
    // (`hdf5_dataset_address` walks the identical `list_all_children` filter), so
    // the enumerate position is the variable's decode index — recorded now because
    // it survives the by-name sort below, where the vector position no longer does.
    let mut variables = Vec::new();
    for (decode_index, d) in datasets.iter().enumerate() {
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
        bytes,
        root_group_address(bytes, probe)?,
        probe,
    )?);

    Ok(Hdf5Metadata {
        dimensions,
        global_attributes,
        variables,
    })
}

/// Gather one dataset's name, element type, rank, attributes, and — if it is a
/// dimension scale — its scale entry, from a single header walk.
fn describe(
    bytes: &[u8],
    probe: &Hdf5Probe,
    child: group::GroupChild,
) -> Result<DatasetInfo, FieldglassError> {
    let shape = dataset::describe(bytes, child.object_header_address, probe)?;
    let attributes = attribute::list_attributes(bytes, child.object_header_address, probe)?;
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
                .and_then(|a| a.first_value)
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

/// Allocates the anonymous dimensions a bare HDF5 dataset needs, following
/// netCDF-C's rule so that `ncdump -h` and Fieldglass name the same axes.
///
/// Measured against netCDF-C (through netCDF4-python) on a file whose datasets
/// deliberately repeat, differ and transpose their shapes:
///
/// ```text
/// a_8x8 → (phony_dim_0=8, phony_dim_1=8)    b_8x8 → (phony_dim_0, phony_dim_1)
/// c_4x6 → (phony_dim_2=4, phony_dim_3=6)    d_6x4 → (phony_dim_3, phony_dim_2)
/// e_1d7 → (phony_dim_4=7)
/// ```
///
/// So the rule is per *axis*, not per shape: reuse the lowest-numbered existing
/// anonymous dimension of that extent which this dataset is not already using,
/// and otherwise allocate the next number. `a_8x8` is the case that rules out
/// deduplicating by length alone — both its axes are 8 long and it still gets
/// two dimensions — and `d_6x4` is the case that rules out matching whole
/// shapes, since it reuses `c_4x6`'s pair transposed.
#[derive(Default)]
struct PhonyDimensions {
    /// The length of `phony_dim_N`, indexed by `N`.
    lengths: Vec<u64>,
    /// Whether `phony_dim_N` is extensible. netCDF-C carries `H5S_UNLIMITED`
    /// through to the dimension it invents — `hdf5_ea_chunk_index.h5` reads back
    /// as `phony_dim_0 = 600 (unlimited)` — so this does too.
    unlimited: Vec<bool>,
}

impl PhonyDimensions {
    /// The ordered dimension names for a dataset of these extents, allocating
    /// as it goes.
    fn axes_for(&mut self, extents: &[u64], unlimited_axes: &[bool]) -> Vec<String> {
        let mut taken: Vec<usize> = Vec::with_capacity(extents.len());
        for (axis, &length) in extents.iter().enumerate() {
            let extensible = unlimited_axes.get(axis).copied().unwrap_or(false);
            let existing =
                (0..self.lengths.len()).find(|i| self.lengths[*i] == length && !taken.contains(i));
            let index = match existing {
                Some(i) => i,
                None => {
                    self.lengths.push(length);
                    self.unlimited.push(false);
                    self.lengths.len() - 1
                }
            };
            // Shared by extent, so two datasets can disagree about whether the
            // axis grows. One writer saying it does is enough: the dimension
            // describes what the file permits, not what any one dataset uses.
            self.unlimited[index] |= extensible;
            taken.push(index);
        }
        taken.iter().map(|i| format!("phony_dim_{i}")).collect()
    }

    /// Dimension-table entries for everything allocated.
    fn dimensions(&self) -> Vec<DimensionInfo> {
        self.lengths
            .iter()
            .enumerate()
            .map(|(index, &length)| DimensionInfo {
                name: format!("phony_dim_{index}"),
                length,
                is_unlimited: self.unlimited[index],
            })
            .collect()
    }
}

/// Build the dimension list, ordered by `_Netcdf4Dimid` (falling back to
/// discovery order for scales an older writer left without one), then the
/// anonymous dimensions invented for datasets that declared none.
fn build_dimensions(datasets: &[DatasetInfo], phony: &PhonyDimensions) -> Vec<DimensionInfo> {
    let mut scales: Vec<(i64, DimensionInfo)> = datasets
        .iter()
        .filter_map(|d| d.scale.as_ref())
        .enumerate()
        .map(|(discovery, s)| {
            (
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
    dimensions.extend(phony.dimensions());
    dimensions
}

/// Resolve a variable's ordered dimension names. A coordinate variable's single
/// dimension is itself; any other variable reads its `DIMENSION_LIST`. A dataset
/// with neither (a bare HDF5 dataset, not written by netCDF) falls back to
/// anonymous `phony_dim_N` axes sized from its dataspace by [`PhonyDimensions`],
/// which is what lets such a file be rendered at all: an axis whose name is not
/// in the dimension table resolves to length 0.
fn resolve_variable_dimensions(
    bytes: &[u8],
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
    match attribute::raw_attribute(bytes, d.address, probe, "DIMENSION_LIST")? {
        Some(raw) => decode_dimension_list(bytes, probe, &raw, d.extents.len(), name_by_address),
        None => Ok(phony.axes_for(&d.extents, &d.unlimited_axes)),
    }
}

/// Decode a `DIMENSION_LIST` attribute (a vlen of object references) into the
/// ordered names of the dimensions each axis is attached to. `rank` is the
/// variable's own rank; the attribute must carry one axis per dimension.
fn decode_dimension_list(
    bytes: &[u8],
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
        raw.dataspace.dims.first().copied().unwrap_or(0) as usize
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
            global_heap::read_object(bytes, collection_addr, object_index, probe.length_size)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The anonymous-dimension rule, pinned against netCDF-C.
    ///
    /// These are the exact names netCDF-C gives a scale-less file whose datasets
    /// repeat, differ and transpose their shapes — read back through
    /// netCDF4-python and reproduced here so the rule cannot drift silently.
    /// `build_hdf5_fixtures.py` writes that file as `hdf5_phony_dims.h5` and
    /// `hdf5_phony_dims.rs` checks the same expectations end to end.
    #[test]
    fn anonymous_dimensions_are_numbered_the_way_netcdf_c_numbers_them() {
        let mut phony = PhonyDimensions::default();

        // Two axes of the same length still get two dimensions: an anonymous
        // dimension is per axis, not per distinct length.
        assert_eq!(phony.axes_for(&[8, 8], &[]), ["phony_dim_0", "phony_dim_1"]);
        // An identical shape reuses them rather than allocating more.
        assert_eq!(phony.axes_for(&[8, 8], &[]), ["phony_dim_0", "phony_dim_1"]);
        // New extents allocate.
        assert_eq!(phony.axes_for(&[4, 6], &[]), ["phony_dim_2", "phony_dim_3"]);
        // A transposed shape reuses the same pair the other way round, which is
        // what rules out matching whole shapes instead of individual extents.
        assert_eq!(phony.axes_for(&[6, 4], &[]), ["phony_dim_3", "phony_dim_2"]);
        assert_eq!(phony.axes_for(&[7], &[]), ["phony_dim_4"]);

        assert_eq!(
            phony
                .dimensions()
                .iter()
                .map(|d| (d.name.as_str(), d.length, d.is_unlimited))
                .collect::<Vec<_>>(),
            [
                ("phony_dim_0", 8, false),
                ("phony_dim_1", 8, false),
                ("phony_dim_2", 4, false),
                ("phony_dim_3", 6, false),
                ("phony_dim_4", 7, false),
            ]
        );
    }

    /// Three axes of one length need three dimensions, not two — the `taken`
    /// check has to exclude everything this dataset already holds, not just the
    /// one it matched last.
    #[test]
    fn a_dataset_never_reuses_a_dimension_within_itself() {
        let mut phony = PhonyDimensions::default();
        assert_eq!(
            phony.axes_for(&[5, 5, 5], &[]),
            ["phony_dim_0", "phony_dim_1", "phony_dim_2"]
        );
        assert_eq!(phony.lengths, [5, 5, 5]);
    }

    /// A dimension is shared by extent, so two datasets can disagree about
    /// whether that axis grows. One writer saying it does is enough — the
    /// dimension describes what the file permits.
    #[test]
    fn a_shared_dimension_is_unlimited_if_any_user_says_so() {
        let mut phony = PhonyDimensions::default();
        // Bounded first, then the same extent declared extensible.
        assert_eq!(phony.axes_for(&[4], &[false]), ["phony_dim_0"]);
        assert_eq!(phony.axes_for(&[4], &[true]), ["phony_dim_0"]);
        assert!(phony.dimensions()[0].is_unlimited);

        // And it does not leak the other way: an untouched dimension stays bounded.
        let mut bounded = PhonyDimensions::default();
        bounded.axes_for(&[4, 9], &[true, false]);
        assert_eq!(
            bounded
                .dimensions()
                .iter()
                .map(|d| d.is_unlimited)
                .collect::<Vec<_>>(),
            [true, false]
        );
    }

    /// A scalar dataset has no axes, and must not invent one.
    #[test]
    fn a_scalar_dataset_allocates_nothing() {
        let mut phony = PhonyDimensions::default();
        assert!(phony.axes_for(&[], &[]).is_empty());
        assert!(phony.dimensions().is_empty());
    }
}
