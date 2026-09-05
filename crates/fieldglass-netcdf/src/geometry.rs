//! NetCDF 2-D slice geometry: CF axis detection, renderable-variable selection,
//! and synthesis of a regular lat/lon grid from 1-D coordinate variables
//! (decision 0002).
//!
//! A NetCDF variable is routinely 3-D or 4-D (`time × level × lat × lon`), and
//! the file carries no GRIB-style projection metadata. To reach the existing
//! warp pipeline this module answers two questions the GRIB path never had to:
//!
//! 1. **Which dimensions are the horizontal (lat / lon) axes** — detected from
//!    CF conventions on the 1-D coordinate variables, not dimension order.
//! 2. **What grid geometry** to synthesise — corner coordinates read from the
//!    coordinate arrays, mapped onto a regular `"latlon"` grid.
//!
//! The logic is backing-agnostic: it operates on a neutral [`DatasetView`] so the
//! classic and NetCDF-4 / HDF5 backings share one implementation (built by
//! [`DatasetView::from_classic`] / [`DatasetView::from_hdf5`]).
//! The first pass handles **regular 1-D lat/lon grids only**; curvilinear (2-D
//! coordinate) and projected grids are tracked separately (decision 0002,
//! *Out of scope*).

use crate::classic::{ClassicHeader, NcType};
use crate::hdf5::dimensions::{Hdf5Metadata, UnsupportedVariable};
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::checked_usize;

/// The horizontal axis a coordinate variable represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    /// The variable is the grid's latitude / y axis.
    Latitude,
    /// The variable is the grid's longitude / x axis.
    Longitude,
}

/// One dimension in the neutral view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimView {
    /// The dimension's name, as both backings report it.
    pub name: String,
    /// The dimension's runtime length; the record dimension resolves to its
    /// count rather than the on-disk zero.
    pub length: u64,
}

/// The value stored in the neutral view for an attribute, preserving full `f64`
/// precision for a **scalar numeric** attribute. The human-facing display string
/// rounds small magnitudes lossily (a GOES `scale_factor` ≈ 6.7e-7 prints as
/// `"0.000001"`), which would mis-scale the `x`/`y` scan-angle coordinates the
/// geostationary resolver reads. Multi-valued (comma-bearing) and string
/// attributes keep their display string. `format!("{n}")` is the shortest
/// round-trippable representation.
fn attr_value(display: &str, first_value: Option<f64>) -> String {
    match first_value {
        Some(n) if !display.contains(',') => format!("{n}"),
        _ => display.to_string(),
    }
}

/// One variable in the neutral view, carrying just what axis detection and the
/// slice picker need. `decode_index` is the index
/// [`crate::NetcdfReader::decode_variable_values`] uses, so a chosen variable
/// maps straight back to its data.
#[derive(Debug, Clone, PartialEq)]
pub struct VarView {
    /// Index [`crate::NetcdfReader::decode_variable_values`] takes, so a
    /// chosen variable maps straight back to its data.
    pub decode_index: usize,
    /// The variable's name.
    pub name: String,
    /// The variable's element type, mapped onto the classic vocabulary for
    /// both backings.
    pub nc_type: NcType,
    /// Ordered dimension names.
    pub dim_names: Vec<String>,
    /// Attributes as `(name, display_value)`. Axis detection reads the CF axis
    /// attributes (`units`, `standard_name`, `axis`); [`VarView::unpack`] reads
    /// the mask-and-scale ones (`scale_factor`, `add_offset`, `valid_range`,
    /// `valid_min`, `valid_max`) back out of the same strings, so the values
    /// here have to stay numerically faithful — see [`DatasetView::from_hdf5`].
    pub attrs: Vec<(String, String)>,
}

impl VarView {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// A coordinate variable is 1-D and shares its name with its single
    /// dimension (a `lat(lat)` variable).
    fn is_coordinate(&self) -> bool {
        self.dim_names.len() == 1 && self.dim_names[0] == self.name
    }

    fn is_numeric(&self) -> bool {
        self.nc_type != NcType::Char
    }

    /// Apply the CF mask-and-scale this variable's own attributes call for to
    /// values already decoded for it — [`crate::unpack_cf_data`] with
    /// [`VarView::attrs`], which is the only correct attribute set for them.
    ///
    /// The decode ([`crate::NetcdfReader::decode_variable_values`]) returns raw
    /// on-disk codes with only the fill / missing sentinels masked; this is the
    /// second stage that turns them into physical units. Callers that decode and
    /// unpack in one go want [`crate::NetcdfReader::decode_variable_physical`]
    /// or [`crate::NetcdfReader::decode_plane`] instead; this method is for a
    /// host that caches the raw decode per variable and re-slices it.
    pub fn unpack(&self, raw: &[Option<f64>]) -> Vec<Option<f64>> {
        crate::projection::unpack_cf_data(raw, &self.attrs)
    }
}

/// A neutral, backing-agnostic view of a dataset's dimensions and variables.
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
    /// Every dimension in the dataset, in declared order.
    pub dims: Vec<DimView>,
    /// Every variable in the dataset, in decode order — coordinate variables
    /// included, unlike [`DatasetView::renderable_variables`].
    pub vars: Vec<VarView>,
    /// Global (root-group) attributes as `(name, display_value)`. Carries the
    /// non-CF projection metadata WRF stores at the file level (`MAP_PROJ`,
    /// `TRUELAT1`, …); see [`crate::projection`].
    pub global_attrs: Vec<(String, String)>,
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
        let dims: Vec<DimView> = header
            .dimensions
            .iter()
            .map(|d| DimView {
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
                name: v.name.clone(),
                nc_type: v.nc_type,
                dim_names: v
                    .dim_ids
                    .iter()
                    .map(|&id| {
                        dims.get(id as usize)
                            .map(|d| d.name.clone())
                            .unwrap_or_else(|| format!("dim#{id}"))
                    })
                    .collect(),
                attrs: v
                    .attributes
                    .iter()
                    .map(|a| (a.name.clone(), attr_value(&a.value, a.first_value)))
                    .collect(),
            })
            .collect();
        let global_attrs = header
            .global_attributes
            .iter()
            .map(|a| (a.name.clone(), attr_value(&a.value, a.first_value)))
            .collect();
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
    /// Attribute values are taken as display strings, but they are **not** only
    /// read for display: [`VarView::unpack`] parses `scale_factor`,
    /// `add_offset` and `valid_range` back out of them, which is why a scalar
    /// numeric attribute keeps its full `f64` precision here (see `attr_value`)
    /// rather than the rounded form the metadata panel shows. Making that
    /// formatter lossier silently mis-scales every packed field on this
    /// backing.
    pub fn from_hdf5(meta: &Hdf5Metadata) -> Self {
        let dims = meta
            .dimensions
            .iter()
            .map(|d| DimView {
                name: d.name.clone(),
                length: d.length,
            })
            .collect();
        let vars = meta
            .variables
            .iter()
            .map(|v| VarView {
                decode_index: v.decode_index,
                name: v.name.clone(),
                nc_type: v.nc_type,
                dim_names: v.dimensions.clone(),
                attrs: v
                    .attributes
                    .iter()
                    .map(|a| (a.name.clone(), attr_value(&a.value, a.first_value)))
                    .collect(),
            })
            .collect();
        let global_attrs = meta
            .global_attributes
            .iter()
            .map(|a| (a.name.clone(), attr_value(&a.value, a.first_value)))
            .collect();
        Self {
            dims,
            vars,
            global_attrs,
            unsupported: meta.unsupported.clone(),
        }
    }

    fn dim_length(&self, name: &str) -> Option<u64> {
        self.dims.iter().find(|d| d.name == name).map(|d| d.length)
    }

    /// The variable carrying `decode_index`, if the view has one. The index is
    /// the decode order, not a position in [`DatasetView::vars`] — the HDF5
    /// backing skips pure-dimension datasets, so the two differ there. Gives a
    /// host holding only a [`RenderableVariable`] (which carries no attributes)
    /// its way back to them, for [`VarView::unpack`].
    pub fn var(&self, decode_index: usize) -> Option<&VarView> {
        self.vars.iter().find(|v| v.decode_index == decode_index)
    }

    /// The decode index of a dimension's coordinate variable, if one exists (a
    /// 1-D variable whose name equals the dimension name). The render path reads
    /// it through [`crate::NetcdfReader::decode_variable_values`] to derive the
    /// grid corners.
    pub fn coordinate_index(&self, dim_name: &str) -> Option<usize> {
        self.vars
            .iter()
            .find(|v| v.is_coordinate() && v.name == dim_name)
            .map(|v| v.decode_index)
    }

    /// Map every dimension that has a coordinate variable to its detected axis
    /// kind. Only latitude / longitude are reported; non-horizontal axes (time,
    /// level) are simply absent from the map.
    fn axis_by_dim(&self) -> Vec<(String, AxisKind)> {
        self.vars
            .iter()
            .filter(|v| v.is_coordinate())
            .filter_map(|v| detect_axis(v).map(|kind| (v.name.clone(), kind)))
            .collect()
    }

    /// The renderable variables (decision 0002, Q2): numeric, at least 2-D, and
    /// not a coordinate variable. Each carries the detected horizontal axis
    /// positions so the picker can pre-fill the X / Y selectors.
    pub fn renderable_variables(&self) -> Vec<RenderableVariable> {
        // Every variable some *other* variable names as its 2-D lat/lon pair.
        // These are coordinates, not fields, and a picker offering them puts a
        // picture of latitude in front of the user before anything else — RTOFS
        // lists `Latitude` first, so it was the default a file opened on.
        //
        // The 1-D case is already excluded by `is_coordinate`, which requires a
        // variable be named for its own single dimension. A 2-D coordinate is
        // not named for a dimension at all, so it needs its own rule (#218).
        let coordinate_planes: Vec<usize> = self
            .vars
            .iter()
            .filter_map(|v| self.resolve_curvilinear(v))
            .flat_map(|(coords, _, _)| [coords.lat_index, coords.lon_index])
            .collect();
        let axes = self.axis_by_dim();
        let lat_dim = axes
            .iter()
            .find(|(_, k)| *k == AxisKind::Latitude)
            .map(|(n, _)| n.as_str());
        let lon_dim = axes
            .iter()
            .find(|(_, k)| *k == AxisKind::Longitude)
            .map(|(n, _)| n.as_str());

        self.vars
            .iter()
            .filter(|v| {
                v.is_numeric()
                    && v.dim_names.len() >= 2
                    && !v.is_coordinate()
                    && !coordinate_planes.contains(&v.decode_index)
            })
            .map(|v| {
                let position =
                    |dim: Option<&str>| dim.and_then(|d| v.dim_names.iter().position(|n| n == d));
                let curvilinear = self.curvilinear_axes(v);
                RenderableVariable {
                    decode_index: v.decode_index,
                    name: v.name.clone(),
                    nc_type: v.nc_type,
                    dims: v
                        .dim_names
                        .iter()
                        .map(|n| DimView {
                            name: n.clone(),
                            length: self.dim_length(n).unwrap_or(0),
                        })
                        .collect(),
                    // A curvilinear variable has no 1-D coordinate variable
                    // to detect, so its axes come from the 2-D pair instead —
                    // otherwise the picker falls back to the first two
                    // dimensions and lands on a time axis (#218).
                    detected_y_dim: position(lat_dim).or(curvilinear.map(|(y, _)| y)),
                    detected_x_dim: position(lon_dim).or(curvilinear.map(|(_, x)| x)),
                }
            })
            .collect()
    }

    /// The 2-D auxiliary lat/lon coordinates a data variable names, if it names
    /// a usable pair (#445).
    ///
    /// CF lets a variable point at coordinates it is not indexed by
    /// (`coordinates = "Longitude Latitude Date"` is what RTOFS writes, and
    /// `Date` is a time), so every name is resolved and then kept only if it is
    /// a 2-D variable over exactly the two dimensions being drawn, in that
    /// order. What survives is classified by [`detect_axis`] — which reads
    /// `units` and `standard_name` and has always been able to recognise these;
    /// it was never offered them, because `Self::axis_by_dim` only offers
    /// 1-D coordinate variables.
    ///
    /// `None` unless exactly one latitude and one longitude survive. Two
    /// latitudes, or a lone longitude, describe a grid this cannot place, and
    /// guessing at it would put the field somewhere wrong rather than leaving
    /// it in the source projection where the user can see it is unplaced.
    pub fn curvilinear_coords(
        &self,
        var: &VarView,
        y_dim: &str,
        x_dim: &str,
    ) -> Option<CurvilinearCoords> {
        let (coords, found_y, found_x) = self.resolve_curvilinear(var)?;
        (found_y == y_dim && found_x == x_dim).then_some(coords)
    }

    /// Which of `var`'s dimensions are its image axes, when its position comes
    /// from 2-D coordinates — as `(y, x)` positions in `var.dim_names` (#218).
    ///
    /// A curvilinear variable has no 1-D coordinate variable to detect an axis
    /// from, so the slice picker had nothing to pre-select and fell back to the
    /// variable's first two dimensions. For a swath that is right by luck; for
    /// an ocean field shaped `(time, Y, X)` it picks the length-1 time axis and
    /// renders a one-pixel sliver of the wrong plane, ungeolocated.
    ///
    /// The 2-D coordinate arrays already name the answer: they span exactly the
    /// two dimensions that are the image, in that order.
    pub fn curvilinear_axes(&self, var: &VarView) -> Option<(usize, usize)> {
        let (_, y_dim, x_dim) = self.resolve_curvilinear(var)?;
        let position = |name: &str| var.dim_names.iter().position(|d| d == name);
        Some((position(&y_dim)?, position(&x_dim)?))
    }

    /// The 2-D lat/lon pair a variable's CF `coordinates` attribute names, with
    /// the two dimension names they span.
    ///
    /// CF lets a variable point at coordinates it is not indexed by
    /// (`coordinates = "Longitude Latitude Date"` is what RTOFS writes, and
    /// `Date` is a time), so every name is resolved and then kept only if it is
    /// a 2-D variable over two of the variable's *own* dimensions. What
    /// survives is classified by [`detect_axis`] — which reads `units` and
    /// `standard_name` and has always been able to recognise these; it was
    /// never offered them, because [`Self::axis_by_dim`] only offers 1-D
    /// coordinate variables.
    ///
    /// `None` unless exactly one latitude and one longitude survive **and they
    /// agree on their dimensions**. Two latitudes, a lone longitude, or a pair
    /// laid out over different axes describe a grid this cannot place, and
    /// guessing would put the field somewhere wrong rather than leaving it in
    /// the source projection where the user can see it is unplaced.
    fn resolve_curvilinear(&self, var: &VarView) -> Option<(CurvilinearCoords, String, String)> {
        let named = var.attr("coordinates")?;
        let (mut lat, mut lon) = (None, None);
        for name in named.split_whitespace() {
            let Some(candidate) = self.vars.iter().find(|v| v.name == name) else {
                continue;
            };
            // Two dimensions, both the variable's own: a `coordinates` list may
            // name a 1-D time axis alongside the spatial pair.
            if candidate.dim_names.len() != 2
                || !candidate
                    .dim_names
                    .iter()
                    .all(|d| var.dim_names.contains(d))
            {
                continue;
            }
            let entry = Some((candidate.decode_index, candidate.dim_names.clone()));
            match detect_axis(candidate) {
                Some(AxisKind::Latitude) if lat.is_none() => lat = entry,
                Some(AxisKind::Longitude) if lon.is_none() => lon = entry,
                // A second variable of a kind already found: the attribute
                // names two latitudes, and which one places the grid is not
                // something to pick by declaration order.
                Some(_) => return None,
                None => {}
            }
        }
        let (lat_index, lat_dims) = lat?;
        let (lon_index, lon_dims) = lon?;
        // The pair must be laid out the same way, or there is no single raster
        // for them to describe.
        if lat_dims != lon_dims {
            return None;
        }
        Some((
            CurvilinearCoords {
                lat_index,
                lon_index,
            },
            lat_dims[0].clone(),
            lat_dims[1].clone(),
        ))
    }
}

/// The two 2-D auxiliary coordinate variables a CF `coordinates` attribute
/// names, resolved against the data variable's own dimensions (#445).
///
/// Carries decode indices rather than the arrays themselves: a `DatasetView` is
/// metadata, and these are two full planes — 52,000 doubles for the smallest
/// real grid in the corpus — that only the render seam wants and only once per
/// slice. `planned/03-composition.md` draws them as `lat2d` / `lon2d` `Vec`s;
/// the shape is the same, the ownership is not.
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
    /// Index [`crate::NetcdfReader::decode_variable_values`] takes.
    pub decode_index: usize,
    /// The variable's name.
    pub name: String,
    /// The variable's element type.
    pub nc_type: NcType,
    /// The variable's axes in declared (C) order — the order `detected_y_dim`
    /// and `detected_x_dim` index into.
    pub dims: Vec<DimView>,
    /// Position (axis index) of the latitude dimension within `dims`.
    pub detected_y_dim: Option<usize>,
    /// Position (axis index) of the longitude dimension within `dims`.
    pub detected_x_dim: Option<usize>,
}

/// Classify a coordinate variable's axis by CF conventions, in priority order:
/// `units` → `standard_name` → `axis` → a name heuristic. Returns `None` for a
/// coordinate variable that matches none (e.g. a vertical or time axis).
pub fn detect_axis(var: &VarView) -> Option<AxisKind> {
    if let Some(units) = var.attr("units")
        && let Some(kind) = axis_from_units(units)
    {
        return Some(kind);
    }
    if let Some(std) = var.attr("standard_name") {
        match std.trim() {
            "latitude" => return Some(AxisKind::Latitude),
            "longitude" => return Some(AxisKind::Longitude),
            _ => {}
        }
    }
    match var.attr("axis").map(str::trim) {
        Some("Y") => return Some(AxisKind::Latitude),
        Some("X") => return Some(AxisKind::Longitude),
        _ => {}
    }
    axis_from_name(&var.name)
}

/// CF latitude/longitude `units` test. Accepts the canonical `degrees_north` /
/// `degrees_east` family and the spelling variants CF permits
/// (`degree_north`, `degreesN`, `degree_N`, …). Case-insensitive on the
/// direction token; a leading `degree`/`degrees` (singular or plural) is
/// required so a bare `"north"` does not match.
fn axis_from_units(units: &str) -> Option<AxisKind> {
    let u = units.trim();
    let rest = u
        .strip_prefix("degrees")
        .or_else(|| u.strip_prefix("degree"))?;
    // Allow an optional separator between the degree token and the direction.
    let dir = rest.trim_start_matches(['_', ' ']);
    match dir.to_ascii_lowercase().as_str() {
        "north" | "n" => Some(AxisKind::Latitude),
        "east" | "e" => Some(AxisKind::Longitude),
        _ => None,
    }
}

/// Last-resort name heuristic when CF metadata is absent. Recognises the common
/// `lat`/`latitude`/`y` and `lon`/`longitude`/`x` spellings.
fn axis_from_name(name: &str) -> Option<AxisKind> {
    match name.to_ascii_lowercase().as_str() {
        "lat" | "latitude" | "y" | "nav_lat" | "yc" => Some(AxisKind::Latitude),
        "lon" | "long" | "longitude" | "x" | "nav_lon" | "xc" => Some(AxisKind::Longitude),
        _ => None,
    }
}

/// First and last value of a coordinate array plus whether its spacing is
/// regular (uniform deltas within tolerance). The synthesised `"latlon"`
/// geometry assumes uniform spacing; an irregular axis (a Gaussian latitude
/// row, say) still renders via the corner mapping but the panel flags it as
/// approximate. A constant or single-point axis is treated as regular.
pub fn corner_and_regularity(coord: &[f64]) -> Option<(f64, f64, bool)> {
    let first = *coord.first()?;
    let last = *coord.last()?;
    if coord.len() < 3 {
        return Some((first, last, true));
    }
    let mean_delta = (last - first) / (coord.len() as f64 - 1.0);
    if mean_delta == 0.0 {
        return Some((first, last, true));
    }
    // Tolerate a small fraction of the mean step; floating-point coordinate
    // arrays rarely have bit-identical deltas even when uniform.
    let tol = mean_delta.abs() * 1e-3;
    let regular = coord
        .windows(2)
        .all(|w| ((w[1] - w[0]) - mean_delta).abs() <= tol);
    Some((first, last, regular))
}

/// Extract one 2-D plane (`y_dim × x_dim`) from a row-major (C-order) N-D
/// variable. `shape` is the variable's dimension lengths in declared order;
/// `fixed` gives the held index for every non-horizontal dimension (its entry
/// for `x_dim` / `y_dim` is ignored). The output is row-major over the
/// synthesised grid — `nj` rows (one per `y_dim` index) of `ni` values (one per
/// `x_dim` index) — matching how the warp reads a `"latlon"` field. Works for
/// any axis positions, so an X-before-Y assignment transposes correctly.
pub fn extract_plane(
    values: &[Option<f64>],
    shape: &[u64],
    y_dim: usize,
    x_dim: usize,
    fixed: &[usize],
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let rank = shape.len();
    if y_dim >= rank || x_dim >= rank || y_dim == x_dim {
        return Err(FieldglassError::Parse(format!(
            "invalid axis assignment y_dim={y_dim} x_dim={x_dim} for rank {rank}"
        )));
    }
    if fixed.len() != rank {
        return Err(FieldglassError::Parse(format!(
            "fixed index vector length {} does not match rank {rank}",
            fixed.len()
        )));
    }
    // C-order strides: stride[d] = product of shape[d+1..].
    let mut strides = vec![1usize; rank];
    for d in (0..rank.saturating_sub(1)).rev() {
        strides[d] = strides[d + 1]
            .checked_mul(checked_usize(shape[d + 1], "NetCDF dimension length")?)
            .ok_or_else(|| FieldglassError::Parse("variable shape overflows usize".into()))?;
    }
    // Base offset from the held (non-horizontal) indices.
    let mut base = 0usize;
    for d in 0..rank {
        if d == x_dim || d == y_dim {
            continue;
        }
        if fixed[d] >= checked_usize(shape[d], "NetCDF dimension length")? {
            return Err(FieldglassError::Parse(format!(
                "slice index {} out of range for dimension {d} (length {})",
                fixed[d], shape[d]
            )));
        }
        base += fixed[d] * strides[d];
    }

    let nj = checked_usize(shape[y_dim], "NetCDF dimension length")?;
    let ni = checked_usize(shape[x_dim], "NetCDF dimension length")?;
    let mut out = Vec::with_capacity(nj * ni);
    for j in 0..nj {
        let row = base + j * strides[y_dim];
        for i in 0..ni {
            let idx = row + i * strides[x_dim];
            out.push(values.get(idx).copied().flatten());
        }
    }
    Ok(out)
}

/// The synthesised geometry of a 2-D slice — a regular `"latlon"` grid plus a
/// flag for the picker when the coordinate spacing is irregular (so geolocation
/// is approximate).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceGeometry {
    /// Columns in the slice — the length of its longitude axis.
    pub ni: u32,
    /// Rows in the slice — the length of its latitude axis.
    pub nj: u32,
    /// Latitude of the first row, degrees.
    pub lat_first: f64,
    /// Latitude of the last row, degrees.
    pub lat_last: f64,
    /// Longitude of the first column, degrees.
    pub lon_first: f64,
    /// Longitude of the last column, degrees.
    pub lon_last: f64,
    /// `true` when either coordinate axis has non-uniform spacing.
    pub irregular: bool,
    /// `true` when the longitude axis is monotonically decreasing
    /// (east-to-west). The west-to-east inverse map would misread such an
    /// axis as an antimeridian wrap, so the render seam keeps these files in
    /// the source projection. (A wrapped-storage axis that jumps back across
    /// 0° — 180°..359.75°, 0°..179.75° — is not monotonic and stays `false`;
    /// its descending corner pair really is a wrap.) Descending *latitude*
    /// axes are common and handled; this flags longitude only.
    ///
    /// **The rule this exists for:** a slice is reprojectable exactly when
    /// `!lon_descending`. That mirrors the GRIB scanning-mode gate, and a host
    /// that offers reprojection on a descending-longitude slice draws the
    /// field mirrored. Read it, don't re-derive it from the corner pair —
    /// `lon_first > lon_last` is also true of a genuine wrap, which does
    /// reproject.
    pub lon_descending: bool,
    /// `true` when the latitude axis runs south to north, i.e. row 0 is the
    /// *southern* edge. This is the corner comparison `lat_first < lat_last`,
    /// deliberately not the strict monotonicity [`Self::lon_descending`] uses:
    /// latitude has no wrap to be confused with, so the corners settle the row
    /// order on their own.
    ///
    /// **The rule this exists for:** a NetCDF file carries no scanning mode, so
    /// this is what GRIB reads from flag 0x40 — the raster has to be flipped to
    /// face north-up when it is `true`. CF's common ordering is ascending, so
    /// that is the usual case (#286). Only meaningful when the slice's Y axis
    /// really is a latitude; a cross-section against level or time has no north
    /// to face and stays in storage order.
    pub lat_ascending: bool,
}

/// Synthesise the grid geometry from the decoded latitude and longitude
/// coordinate arrays. `ni = lon.len()`, `nj = lat.len()`; corners are the first
/// and last of each. Errors if either array is empty.
pub fn synthesize_geometry(lat: &[f64], lon: &[f64]) -> Result<SliceGeometry, FieldglassError> {
    let (lat_first, lat_last, lat_regular) = corner_and_regularity(lat)
        .ok_or_else(|| FieldglassError::Parse("empty latitude coordinate array".into()))?;
    let (lon_first, lon_last, lon_regular) = corner_and_regularity(lon)
        .ok_or_else(|| FieldglassError::Parse("empty longitude coordinate array".into()))?;
    let lon_descending = lon.len() >= 2 && lon.windows(2).all(|w| w[1] < w[0]);
    // `SliceGeometry` counts points in `u32`, the width every grid type in the
    // workspace uses. A coordinate array longer than that is not a grid this
    // renderer can describe, so say so rather than wrapping the count.
    let (Ok(ni), Ok(nj)) = (u32::try_from(lon.len()), u32::try_from(lat.len())) else {
        return Err(FieldglassError::Parse(format!(
            "coordinate arrays {}×{} exceed the u32 grid dimensions",
            lon.len(),
            lat.len()
        )));
    };
    Ok(SliceGeometry {
        ni,
        nj,
        lat_first,
        lat_last,
        lon_first,
        lon_last,
        irregular: !(lat_regular && lon_regular),
        lon_descending,
        lat_ascending: lat_first < lat_last,
    })
}
