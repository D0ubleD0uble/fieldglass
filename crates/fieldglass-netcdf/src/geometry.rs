//! NetCDF 2-D slice geometry: the dataset view, and this crate's entry points
//! to CF axis detection and renderable-variable selection (decision 0002).
//!
//! A NetCDF variable is routinely 3-D or 4-D (`time × level × lat × lon`), and
//! the file carries no GRIB-style projection metadata. To reach the existing
//! warp pipeline two questions need answering that the GRIB path never had:
//! which dimensions are the horizontal axes, and what grid geometry to
//! synthesise from the coordinate arrays.
//!
//! **The answers are CF's, and since #704 they live in
//! [`fieldglass_core::cf`]**, written once over the core array model so a Zarr
//! store placed by the same attributes gets the same answers. What stays here
//! is the part only this crate has: the [`DatasetView`] both backings build,
//! and the decode indices that take a chosen variable back to its data. The
//! functions below keep their signatures and ask core; the moved helpers are
//! re-exported so every path this crate has published still resolves.

use crate::classic::{ClassicHeader, NcType};
use crate::hdf5::dimensions::{Hdf5Metadata, UnsupportedVariable};
use fieldglass_core::array::{
    ArrayDescription, Attribute, AttributeValue, Dimension, Group, attribute,
};

pub use fieldglass_core::cf::{
    AxisKind, SliceGeometry, corner_and_regularity, extract_plane, synthesize_geometry,
};

/// One attribute in core's model, from what either backing decoded.
///
/// Text stays text, and a numeric attribute keeps every element as the `f64`
/// its reader decoded, so nothing downstream reads a number back out of a
/// display string (#678). That round trip was a real hazard rather than a
/// theoretical one: a GOES `scale_factor` near 6.7e-7 prints as `0.000001`
/// under any rounding format, and the display text of a `float` is its 32-bit
/// shortest form, which is not the value the data is compared against once
/// widened.
fn model_attribute(name: &str, nc_type: NcType, text: &str, values: &[f64]) -> Attribute {
    if nc_type == NcType::Char {
        Attribute::text(name, text)
    } else {
        Attribute::numbers(name, values.to_vec())
    }
}

/// One variable in the view: core's description of it, and the index that
/// reaches its data.
///
/// The description is what every container of named arrays has — a name, an
/// element type, axes, attributes — and it is all that axis detection, the
/// slice picker and the CF unpacking read. `decode_index` is the part only
/// this reader has. It sits beside the description rather than in a table of
/// its own, so a variable is never handed over without its way back to its
/// data.
#[derive(Debug, Clone, PartialEq)]
pub struct VarView {
    /// Index [`crate::NetcdfReader::decode_variable_raw`] takes, so a
    /// chosen variable maps straight back to its data.
    pub decode_index: usize,
    /// The variable in core's array model. Its element type is the NetCDF type
    /// as [`NcType::element_type`] maps it, and its
    /// [`chunk_grid`](ArrayDescription::chunk_grid) is `None`: the view
    /// describes names and axes, and the storage layout is read when the
    /// variable is decoded.
    pub array: ArrayDescription,
}

impl VarView {
    /// The variable's name, path-qualified when it lives in a nested group.
    pub fn name(&self) -> &str {
        &self.array.name
    }

    /// One of the variable's own attributes, by name.
    pub fn attribute(&self, name: &str) -> Option<&AttributeValue> {
        attribute(&self.array.attributes, name)
    }

    /// The CF `units`, as the file spells them, when the variable declares any.
    /// A number stored under the name is not text, however it would print.
    pub fn units(&self) -> Option<&str> {
        self.attribute("units").and_then(AttributeValue::text)
    }

    /// The variable's NetCDF type. `None` only for a description built by hand
    /// with an element type NetCDF has no name for; neither backing makes one.
    pub fn nc_type(&self) -> Option<NcType> {
        NcType::from_element_type(&self.array.element_type)
    }

    /// A coordinate variable is 1-D and shares its name with its single
    /// dimension (a `lat(lat)` variable).
    fn is_coordinate(&self) -> bool {
        let dims = &self.array.dimensions;
        dims.len() == 1 && dims[0] == self.array.name
    }

    /// Apply the CF mask-and-scale this variable's own attributes call for to
    /// values already decoded for it — [`crate::unpack_cf_data`] with this
    /// variable's attributes, which are the only correct set for them.
    ///
    /// The decode ([`crate::NetcdfReader::decode_variable_raw`]) returns raw
    /// on-disk codes with only the fill / missing sentinels masked; this is the
    /// second stage that turns them into physical units. Callers that decode and
    /// unpack in one go want [`crate::NetcdfReader::decode_variable_physical`]
    /// or [`crate::NetcdfReader::decode_plane`] instead; this method is for a
    /// host that caches the raw decode per variable and re-slices it.
    pub fn unpack(&self, raw: &[Option<f64>]) -> Vec<Option<f64>> {
        crate::projection::unpack_cf_data(raw, &self.array.attributes)
    }
}

/// A neutral, backing-agnostic view of a dataset's dimensions and variables,
/// made of `fieldglass-core`'s array model: [`Dimension`]s, [`Attribute`]s and,
/// per variable, an [`ArrayDescription`] (#684). [`Self::group`] hands the same
/// thing over as the [`Group`] a host walks for any container of named arrays.
///
/// [`Default`] is the empty view — no dimensions, no variables, no global
/// attributes. It is what a host falls back to when
/// [`crate::NetcdfReader::view`] fails on an HDF5 layout outside the decoded
/// subset, so the file still opens on its format-level metadata alone. That is
/// the whole-file failure only: a single dataset whose *datatype* is outside
/// the subset no longer costs the view, and lands in
/// [`DatasetView::unsupported`] instead (#550).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DatasetView {
    /// Every dimension in the dataset, in declared order. The record dimension
    /// reports its record count rather than the on-disk zero.
    pub dims: Vec<Dimension>,
    /// Every variable in the dataset, in decode order — coordinate variables
    /// included, unlike [`DatasetView::renderable_variables`].
    pub vars: Vec<VarView>,
    /// Global (root-group) attributes. Carries the non-CF projection metadata
    /// WRF stores at the file level (`MAP_PROJ`, `TRUELAT1`, …); see
    /// [`crate::projection`].
    pub global_attrs: Vec<Attribute>,
    /// The datasets left out of `vars` because their datatype is outside the
    /// decoded subset, and why (#550). Always empty for a classic backing,
    /// which has no such types. A host that lists variables should say these
    /// exist: everything else in the view is complete, and this is the
    /// difference between it and the file.
    pub unsupported: Vec<UnsupportedVariable>,
}

impl DatasetView {
    /// Build the view from a classic (CDF-1/2/5) header. The record dimension's
    /// runtime length is taken from `numrecs`; all variables (coordinate
    /// variables included) keep their header order, which is the decode order.
    pub fn from_classic(header: &ClassicHeader) -> Self {
        let attribute = |a: &crate::classic::Attribute| {
            model_attribute(&a.name, a.nc_type, &a.value, &a.values)
        };
        let dims: Vec<Dimension> = header
            .dimensions
            .iter()
            .map(|d| Dimension {
                name: d.name.clone(),
                length: if d.is_record {
                    header.numrecs.unwrap_or(0)
                } else {
                    d.length
                },
            })
            .collect();
        let vars = header
            .variables
            .iter()
            .enumerate()
            .map(|(i, v)| VarView {
                decode_index: i,
                array: ArrayDescription {
                    name: v.name.clone(),
                    element_type: v.nc_type.element_type(),
                    dimensions: v
                        .dim_ids
                        .iter()
                        .map(|&id| {
                            dims.get(id as usize)
                                .map(|d| d.name.clone())
                                .unwrap_or_else(|| format!("dim#{id}"))
                        })
                        .collect(),
                    attributes: v.attributes.iter().map(attribute).collect(),
                    chunk_grid: None,
                },
            })
            .collect();
        let global_attrs = header.global_attributes.iter().map(attribute).collect();
        Self {
            dims,
            vars,
            global_attrs,
            // A classic file has no datatype outside the decoded subset.
            unsupported: Vec::new(),
        }
    }

    /// Build the view from resolved NetCDF-4 / HDF5 metadata (decision 0003).
    /// Dimensions and variables carry the dimension-scale names; each variable's
    /// [`VarView::decode_index`] is the metadata's own
    /// [`crate::hdf5::dimensions::VariableInfo::decode_index`], which already
    /// accounts for the pure-dimension datasets the classic backing never has.
    ///
    /// A numeric attribute carries the numbers the attribute reader decoded,
    /// not its display text: [`VarView::unpack`] reads `scale_factor`,
    /// `add_offset` and `valid_range` from them, and the geostationary
    /// resolver reads a GOES scale factor small enough that any rounded
    /// rendering of it would mis-scale the whole grid.
    pub fn from_hdf5(meta: &Hdf5Metadata) -> Self {
        let attribute = |a: &crate::hdf5::attribute::Hdf5Attribute| {
            model_attribute(&a.name, a.datatype.nc_type, &a.value, &a.values)
        };
        let dims = meta
            .dimensions
            .iter()
            .map(|d| Dimension {
                name: d.name.clone(),
                length: d.length,
            })
            .collect();
        let vars = meta
            .variables
            .iter()
            .map(|v| VarView {
                decode_index: v.decode_index,
                array: ArrayDescription {
                    name: v.name.clone(),
                    element_type: v.nc_type.element_type(),
                    dimensions: v.dimensions.clone(),
                    attributes: v.attributes.iter().map(attribute).collect(),
                    chunk_grid: None,
                },
            })
            .collect();
        let global_attrs = meta.global_attributes.iter().map(attribute).collect();
        Self {
            dims,
            vars,
            global_attrs,
            unsupported: meta.unsupported.clone(),
        }
    }

    /// The dataset as core's [`Group`]: the tree a host walks the same way for
    /// every container of named arrays (ADR-0010 decision 3).
    ///
    /// One root group, holding every dimension and variable under the names
    /// both backings already resolve — a variable in a nested NetCDF-4 group
    /// keeps its HDF5 path, `/PRODUCT/latitude` — so
    /// [`Group::arrays_qualified`] and [`Group::dimensions_qualified`] give back
    /// exactly the names in this view. A nested group's own attributes are not
    /// in it, because neither backing reads them. [`crate::NetcdfArrays`] is
    /// the same tree as an `ArraySource`, with the leading `/` dropped so a
    /// nested name reads the way a Zarr store's does.
    pub fn group(&self) -> Group {
        Group {
            name: String::new(),
            attributes: self.global_attrs.clone(),
            dimensions: self.dims.clone(),
            arrays: self.vars.iter().map(|v| v.array.clone()).collect(),
            groups: Vec::new(),
        }
    }

    /// The variable carrying `decode_index`, if the view has one. The index is
    /// the decode order, not a position in [`DatasetView::vars`] — the HDF5
    /// backing skips pure-dimension datasets, so the two differ there. Gives a
    /// host holding only a [`RenderableVariable`] (which carries no attributes)
    /// its way back to them, for [`VarView::unpack`].
    pub fn var(&self, decode_index: usize) -> Option<&VarView> {
        self.vars.iter().find(|v| v.decode_index == decode_index)
    }

    /// The variable of a given name.
    fn var_named(&self, name: &str) -> Option<&VarView> {
        self.vars.iter().find(|v| v.array.name == name)
    }

    /// The decode index of a dimension's coordinate variable, if one exists (a
    /// 1-D variable whose name equals the dimension name). The render path reads
    /// it through [`crate::NetcdfReader::decode_variable_raw`] to derive the
    /// grid corners.
    pub fn coordinate_index(&self, dim_name: &str) -> Option<usize> {
        self.vars
            .iter()
            .find(|v| v.is_coordinate() && v.array.name == dim_name)
            .map(|v| v.decode_index)
    }

    /// The renderable variables (decision 0002, Q2): numeric, at least 2-D, and
    /// neither a coordinate variable nor half of another's 2-D coordinate pair.
    /// Each carries the detected horizontal axis positions so the picker can
    /// pre-fill the X / Y selectors — see [`fieldglass_core::cf::renderable_arrays`],
    /// which this asks.
    pub fn renderable_variables(&self) -> Vec<RenderableVariable> {
        fieldglass_core::cf::renderable_arrays(&self.group())
            .into_iter()
            .filter_map(|array| {
                let var = self.var_named(&array.name)?;
                Some(RenderableVariable {
                    decode_index: var.decode_index,
                    name: array.name,
                    // Numeric, by the renderable rule, so NetCDF has a name for it.
                    nc_type: NcType::from_element_type(&array.element_type)?,
                    dims: array.dims,
                    detected_y_dim: array.detected_y_dim,
                    detected_x_dim: array.detected_x_dim,
                })
            })
            .collect()
    }

    /// The 2-D auxiliary lat/lon coordinates a data variable names, if it names
    /// a usable pair spanning exactly `y_dim` then `x_dim` (#445) — see
    /// [`fieldglass_core::cf::curvilinear_pair`] for what "usable" means.
    pub fn curvilinear_coords(
        &self,
        var: &VarView,
        y_dim: &str,
        x_dim: &str,
    ) -> Option<CurvilinearCoords> {
        let pair = fieldglass_core::cf::curvilinear_pair(&self.group(), var.name())?;
        if pair.y_dim != y_dim || pair.x_dim != x_dim {
            return None;
        }
        Some(CurvilinearCoords {
            lat_index: self.var_named(&pair.lat)?.decode_index,
            lon_index: self.var_named(&pair.lon)?.decode_index,
        })
    }

    /// Which of `var`'s dimensions are its image axes, when its position comes
    /// from 2-D coordinates — as `(y, x)` positions in the variable's own
    /// dimensions (#218). See [`fieldglass_core::cf::curvilinear_axes`].
    pub fn curvilinear_axes(&self, var: &VarView) -> Option<(usize, usize)> {
        fieldglass_core::cf::curvilinear_axes(&self.group(), var.name())
    }
}

/// The two 2-D auxiliary coordinate variables a CF `coordinates` attribute
/// names, resolved against the data variable's own dimensions (#445).
///
/// Carries decode indices rather than the arrays themselves: a `DatasetView` is
/// metadata, and these are two full planes — 52,000 doubles for the smallest
/// real grid in the corpus — that only the render seam wants and only once per
/// slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurvilinearCoords {
    /// `decode_index` of the 2-D latitude variable.
    pub lat_index: usize,
    /// `decode_index` of the 2-D longitude variable.
    pub lon_index: usize,
}

/// A variable the slice picker can draw, with its dimensions and the detected
/// horizontal-axis positions (`None` when CF detection found no matching axis —
/// the user picks them by hand).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderableVariable {
    /// Index [`crate::NetcdfReader::decode_variable_raw`] takes.
    pub decode_index: usize,
    /// The variable's name.
    pub name: String,
    /// The variable's element type.
    pub nc_type: NcType,
    /// The variable's axes in declared (C) order — the order `detected_y_dim`
    /// and `detected_x_dim` index into.
    pub dims: Vec<Dimension>,
    /// Position (axis index) of the latitude dimension within `dims`.
    pub detected_y_dim: Option<usize>,
    /// Position (axis index) of the longitude dimension within `dims`.
    pub detected_x_dim: Option<usize>,
}

/// Classify a coordinate variable's axis by CF conventions — `units` →
/// `standard_name` → `axis` → a name heuristic. [`fieldglass_core::cf::detect_axis`]
/// over the variable's description.
pub fn detect_axis(var: &VarView) -> Option<AxisKind> {
    fieldglass_core::cf::detect_axis(&var.array)
}
