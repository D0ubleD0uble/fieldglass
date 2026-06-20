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
//! classic path (here) and the future HDF5 path (#169) share one implementation.
//! The first pass handles **regular 1-D lat/lon grids only**; curvilinear (2-D
//! coordinate) and projected grids are tracked separately (decision 0002,
//! *Out of scope*).

use crate::classic::{ClassicHeader, NcType};
use fieldglass_core::FieldglassError;

/// The horizontal axis a coordinate variable represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    Latitude,
    Longitude,
}

/// One dimension in the neutral view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimView {
    pub name: String,
    pub length: u64,
}

/// One variable in the neutral view, carrying just what axis detection and the
/// slice picker need. `decode_index` is the index
/// [`crate::NetcdfReader::decode_variable_values`] uses, so a chosen variable
/// maps straight back to its data.
#[derive(Debug, Clone, PartialEq)]
pub struct VarView {
    pub decode_index: usize,
    pub name: String,
    pub nc_type: NcType,
    /// Ordered dimension names.
    pub dim_names: Vec<String>,
    /// Attributes as `(name, display_value)`; only the CF axis attributes
    /// (`units`, `standard_name`, `axis`) are consulted here.
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
}

/// A neutral, backing-agnostic view of a dataset's dimensions and variables.
#[derive(Debug, Clone, PartialEq)]
pub struct DatasetView {
    pub dims: Vec<DimView>,
    pub vars: Vec<VarView>,
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
                    .map(|a| (a.name.clone(), a.value.clone()))
                    .collect(),
            })
            .collect();
        Self { dims, vars }
    }

    fn dim_length(&self, name: &str) -> Option<u64> {
        self.dims.iter().find(|d| d.name == name).map(|d| d.length)
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
            .filter(|v| v.is_numeric() && v.dim_names.len() >= 2 && !v.is_coordinate())
            .map(|v| {
                let position =
                    |dim: Option<&str>| dim.and_then(|d| v.dim_names.iter().position(|n| n == d));
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
                    detected_y_dim: position(lat_dim),
                    detected_x_dim: position(lon_dim),
                }
            })
            .collect()
    }
}

/// A variable the slice picker can draw, with its dimensions and the detected
/// horizontal-axis positions (`None` when CF detection found no matching axis —
/// the user picks them by hand).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderableVariable {
    pub decode_index: usize,
    pub name: String,
    pub nc_type: NcType,
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
            .checked_mul(shape[d + 1] as usize)
            .ok_or_else(|| FieldglassError::Parse("variable shape overflows usize".into()))?;
    }
    // Base offset from the held (non-horizontal) indices.
    let mut base = 0usize;
    for d in 0..rank {
        if d == x_dim || d == y_dim {
            continue;
        }
        if fixed[d] >= shape[d] as usize {
            return Err(FieldglassError::Parse(format!(
                "slice index {} out of range for dimension {d} (length {})",
                fixed[d], shape[d]
            )));
        }
        base += fixed[d] * strides[d];
    }

    let nj = shape[y_dim] as usize;
    let ni = shape[x_dim] as usize;
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
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lat_last: f64,
    pub lon_first: f64,
    pub lon_last: f64,
    /// `true` when either coordinate axis has non-uniform spacing.
    pub irregular: bool,
}

/// Synthesise the grid geometry from the decoded latitude and longitude
/// coordinate arrays. `ni = lon.len()`, `nj = lat.len()`; corners are the first
/// and last of each. Errors if either array is empty.
pub fn synthesize_geometry(lat: &[f64], lon: &[f64]) -> Result<SliceGeometry, FieldglassError> {
    let (lat_first, lat_last, lat_regular) = corner_and_regularity(lat)
        .ok_or_else(|| FieldglassError::Parse("empty latitude coordinate array".into()))?;
    let (lon_first, lon_last, lon_regular) = corner_and_regularity(lon)
        .ok_or_else(|| FieldglassError::Parse("empty longitude coordinate array".into()))?;
    Ok(SliceGeometry {
        ni: lon.len() as u32,
        nj: lat.len() as u32,
        lat_first,
        lat_last,
        lon_first,
        lon_last,
        irregular: !(lat_regular && lon_regular),
    })
}
