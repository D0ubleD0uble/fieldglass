//! CF axis detection, the renderable-array list, and the 2-D coordinate pairs
//! a slice can be placed by (moved from `fieldglass-netcdf`, #704).
//!
//! An array in a container of named arrays is routinely 3-D or 4-D
//! (`time × level × lat × lon`), and nothing but conventions says which of its
//! axes are horizontal. This module answers that from CF — the `units`,
//! `standard_name` and `axis` of a 1-D coordinate array, or the pair a
//! `coordinates` attribute names — and synthesises a regular lat/lon grid from
//! 1-D coordinates for the placement to use.

use crate::FieldglassError;
use crate::array::{
    ArrayDescription, Attribute, AttributeValue, Dimension, ElementType, Group, attribute,
};
use crate::bytes::checked_usize;

/// The horizontal axis a coordinate array represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AxisKind {
    /// The array is the grid's latitude / y axis.
    Latitude,
    /// The array is the grid's longitude / x axis.
    Longitude,
}

/// A text attribute. A number stored under the name is not text, however it
/// would print; the CF attributes this reads are text by definition.
fn text<'a>(array: &'a ArrayDescription, name: &str) -> Option<&'a str> {
    attribute(&array.attributes, name).and_then(AttributeValue::text)
}

/// Classify a coordinate array's axis by CF conventions, in priority order:
/// `units` → `standard_name` → `axis` → a name heuristic. Returns `None` for a
/// coordinate that matches none (e.g. a vertical or time axis).
pub fn detect_axis(array: &ArrayDescription) -> Option<AxisKind> {
    if let Some(units) = text(array, "units")
        && let Some(kind) = axis_from_units(units)
    {
        return Some(kind);
    }
    if let Some(std) = text(array, "standard_name") {
        match std.trim() {
            "latitude" => return Some(AxisKind::Latitude),
            "longitude" => return Some(AxisKind::Longitude),
            _ => {}
        }
    }
    match text(array, "axis").map(str::trim) {
        Some("Y") => return Some(AxisKind::Latitude),
        Some("X") => return Some(AxisKind::Longitude),
        _ => {}
    }
    axis_from_name(&array.name)
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

/// Whether an element type is one a decode turns into numbers.
fn is_numeric(element_type: &ElementType) -> bool {
    matches!(
        element_type,
        ElementType::Int(_) | ElementType::Uint(_) | ElementType::Float(_)
    )
}

/// Join a group path and a name the way [`Group::arrays_qualified`] does.
fn qualify(path: &str, name: &str) -> String {
    if path.is_empty() {
        name.to_string()
    } else {
        format!("{path}/{name}")
    }
}

/// One array as the CF rules see it: its qualified name, the group it sits
/// in, its description, and its axes with their lengths.
#[derive(Debug)]
pub(crate) struct Entry<'g> {
    /// Path-qualified, as [`Group::arrays_qualified`] spells it.
    pub(crate) name: String,
    /// The path of the group holding it, empty for the root.
    pub(crate) group: String,
    pub(crate) array: &'g ArrayDescription,
    /// Its axes in declared order, each named as its group qualifies it and
    /// sized from that group's dimensions (`0` for one the group does not
    /// declare, as the NetCDF view always did).
    pub(crate) dims: Vec<Dimension>,
}

impl Entry<'_> {
    /// A coordinate array is 1-D and shares its name with its single axis (a
    /// `lat(lat)` array).
    fn is_coordinate(&self) -> bool {
        self.dims.len() == 1 && self.dims[0].name == self.name
    }

    fn dim_names(&self) -> impl Iterator<Item = &str> {
        self.dims.iter().map(|d| d.name.as_str())
    }
}

/// Every array in a group tree, flattened, with the root group's attributes
/// — where WRF keeps its projection.
#[derive(Debug)]
pub(crate) struct Catalog<'g> {
    pub(crate) entries: Vec<Entry<'g>>,
    pub(crate) globals: &'g [Attribute],
}

impl<'g> Catalog<'g> {
    pub(crate) fn new(root: &'g Group) -> Self {
        fn walk<'g>(group: &'g Group, path: &str, out: &mut Vec<Entry<'g>>) {
            for array in &group.arrays {
                let dims = array
                    .dimensions
                    .iter()
                    .map(|name| Dimension {
                        name: qualify(path, name),
                        length: group
                            .dimensions
                            .iter()
                            .find(|d| &d.name == name)
                            .map_or(0, |d| d.length),
                    })
                    .collect();
                out.push(Entry {
                    name: qualify(path, &array.name),
                    group: path.to_string(),
                    array,
                    dims,
                });
            }
            for child in &group.groups {
                let child_path = if child.name.is_empty() {
                    path.to_string()
                } else {
                    qualify(path, &child.name)
                };
                walk(child, &child_path, out);
            }
        }
        let mut entries = Vec::new();
        walk(root, "", &mut entries);
        Self {
            entries,
            globals: &root.attributes,
        }
    }

    /// An array by its qualified name.
    pub(crate) fn find(&self, name: &str) -> Option<&Entry<'g>> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// An array an attribute names, looked up in the group of the array that
    /// named it.
    pub(crate) fn resolve(&self, group: &str, name: &str) -> Option<&Entry<'g>> {
        self.find(&qualify(group, name))
    }

    /// The coordinate array of an axis, if it has one: 1-D, and named for it.
    pub(crate) fn coordinate_of(&self, dim_name: &str) -> Option<&Entry<'g>> {
        self.entries
            .iter()
            .find(|e| e.is_coordinate() && e.name == dim_name)
    }

    /// Every axis with a coordinate array, and the horizontal kind that array
    /// is. Non-horizontal axes (time, level) are absent.
    fn axis_by_dim(&self) -> Vec<(&str, AxisKind)> {
        self.entries
            .iter()
            .filter(|e| e.is_coordinate())
            .filter_map(|e| detect_axis(e.array).map(|kind| (e.name.as_str(), kind)))
            .collect()
    }

    /// The 2-D lat/lon pair an array's CF `coordinates` attribute names, with
    /// the two axes they span.
    ///
    /// CF lets an array point at coordinates it is not indexed by
    /// (`coordinates = "Longitude Latitude Date"` is what RTOFS writes, and
    /// `Date` is a time), so every name is resolved and then kept only if it is
    /// a 2-D array over two of the array's *own* axes. What survives is
    /// classified by [`detect_axis`].
    ///
    /// `None` unless exactly one latitude and one longitude survive **and they
    /// agree on their axes**. Two latitudes, a lone longitude, or a pair laid
    /// out over different axes describe a grid this cannot place, and guessing
    /// would put the field somewhere wrong rather than leaving it in the source
    /// projection where the user can see it is unplaced.
    pub(crate) fn curvilinear(&self, entry: &Entry<'_>) -> Option<CurvilinearPair> {
        let named = text(entry.array, "coordinates")?;
        let (mut lat, mut lon) = (None, None);
        for name in named.split_whitespace() {
            let Some(candidate) = self.resolve(&entry.group, name) else {
                continue;
            };
            // Two axes, both the array's own: a `coordinates` list may name a
            // 1-D time axis alongside the spatial pair.
            let own: Vec<&str> = entry.dim_names().collect();
            if candidate.dims.len() != 2 || !candidate.dim_names().all(|d| own.contains(&d)) {
                continue;
            }
            let found = Some((candidate.name.clone(), candidate.dims.clone()));
            match detect_axis(candidate.array) {
                Some(AxisKind::Latitude) if lat.is_none() => lat = found,
                Some(AxisKind::Longitude) if lon.is_none() => lon = found,
                // A second array of a kind already found: the attribute names
                // two latitudes, and which one places the grid is not something
                // to pick by declaration order.
                Some(_) => return None,
                None => {}
            }
        }
        let (lat, lat_dims) = lat?;
        let (lon, lon_dims) = lon?;
        // The pair must be laid out the same way, or there is no single raster
        // for them to describe.
        if lat_dims != lon_dims {
            return None;
        }
        Some(CurvilinearPair {
            lat,
            lon,
            y_dim: lat_dims[0].name.clone(),
            x_dim: lat_dims[1].name.clone(),
        })
    }
}

/// The two 2-D auxiliary coordinate arrays a CF `coordinates` attribute
/// names, and the two axes they span (#445).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurvilinearPair {
    /// The qualified name of the 2-D latitude array.
    pub lat: String,
    /// The qualified name of the 2-D longitude array.
    pub lon: String,
    /// The axis the pair's rows run along — their first dimension.
    pub y_dim: String,
    /// The axis the pair's columns run along — their second dimension.
    pub x_dim: String,
}

/// The 2-D coordinate pair `array` names, if it names a usable one — see
/// [`CurvilinearPair`].
pub fn curvilinear_pair(root: &Group, array: &str) -> Option<CurvilinearPair> {
    let catalog = Catalog::new(root);
    catalog.curvilinear(catalog.find(array)?)
}

/// Which of `array`'s axes are its image axes when its position comes from 2-D
/// coordinates, as `(y, x)` positions in its own axes (#218).
///
/// An array placed by a 2-D pair has no 1-D coordinate to detect an axis from,
/// so a slice picker had nothing to pre-select and fell back to the first two
/// axes — for an ocean field shaped `(time, Y, X)` the length-1 time axis. The
/// pair already names the answer: it spans exactly the image's two axes, in
/// that order.
pub fn curvilinear_axes(root: &Group, array: &str) -> Option<(usize, usize)> {
    let catalog = Catalog::new(root);
    let entry = catalog.find(array)?;
    let pair = catalog.curvilinear(entry)?;
    let position = |name: &str| entry.dim_names().position(|d| d == name);
    Some((position(&pair.y_dim)?, position(&pair.x_dim)?))
}

/// An array a slice picker can draw, with its axes and the detected horizontal
/// axis positions (`None` when CF detection found no matching axis — the user
/// picks them by hand).
#[derive(Debug, Clone, PartialEq)]
pub struct RenderableArray {
    /// The array's qualified name.
    pub name: String,
    /// Its element type.
    pub element_type: ElementType,
    /// Its axes in declared (C) order — the order `detected_y_dim` and
    /// `detected_x_dim` index into — named as its group qualifies them.
    pub dims: Vec<Dimension>,
    /// Position of the latitude axis within `dims`.
    pub detected_y_dim: Option<usize>,
    /// Position of the longitude axis within `dims`.
    pub detected_x_dim: Option<usize>,
}

/// The arrays a slice picker can draw: numeric, at least 2-D, and neither a
/// coordinate array nor half of another array's 2-D coordinate pair. Each
/// carries the detected horizontal axis positions so the picker can pre-fill
/// its X / Y selectors. In the order the container lists them.
pub fn renderable_arrays(root: &Group) -> Vec<RenderableArray> {
    let catalog = Catalog::new(root);
    // Every array some *other* array names as its 2-D lat/lon pair. These are
    // coordinates, not fields, and a picker offering them puts a picture of
    // latitude in front of the user before anything else — RTOFS lists
    // `Latitude` first, so it was the default a file opened on (#218).
    let coordinate_planes: Vec<String> = catalog
        .entries
        .iter()
        .filter_map(|e| catalog.curvilinear(e))
        .flat_map(|pair| [pair.lat, pair.lon])
        .collect();
    let axes = catalog.axis_by_dim();
    let lat_dim = axes
        .iter()
        .find(|(_, k)| *k == AxisKind::Latitude)
        .map(|(n, _)| *n);
    let lon_dim = axes
        .iter()
        .find(|(_, k)| *k == AxisKind::Longitude)
        .map(|(n, _)| *n);

    catalog
        .entries
        .iter()
        .filter(|e| {
            is_numeric(&e.array.element_type)
                && e.dims.len() >= 2
                && !e.is_coordinate()
                && !coordinate_planes.contains(&e.name)
        })
        .map(|e| {
            let position = |dim: Option<&str>| dim.and_then(|d| e.dim_names().position(|n| n == d));
            // An array placed by a 2-D pair has no 1-D coordinate to detect, so
            // its axes come from the pair instead — otherwise the picker falls
            // back to the first two axes and lands on a time axis (#218).
            let curvilinear = catalog.curvilinear(e).and_then(|pair| {
                let at = |name: &str| e.dim_names().position(|d| d == name);
                Some((at(&pair.y_dim)?, at(&pair.x_dim)?))
            });
            RenderableArray {
                name: e.name.clone(),
                element_type: e.array.element_type.clone(),
                dims: e.dims.clone(),
                detected_y_dim: position(lat_dim).or(curvilinear.map(|(y, _)| y)),
                detected_x_dim: position(lon_dim).or(curvilinear.map(|(_, x)| x)),
            }
        })
        .collect()
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
/// array. `shape` is the array's axis lengths in declared order; `fixed` gives
/// the held index for every non-horizontal axis (its entry for `x_dim` /
/// `y_dim` is ignored). The output is row-major over the synthesised grid —
/// `nj` rows (one per `y_dim` index) of `ni` values (one per `x_dim` index) —
/// matching how the warp reads a `"latlon"` field. Works for any axis
/// positions, so an X-before-Y assignment transposes correctly.
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
    /// **The rule this exists for:** a container of named arrays carries no
    /// scanning mode, so this is what GRIB reads from flag 0x40 — the raster has
    /// to be flipped to face north-up when it is `true`. CF's common ordering
    /// is ascending, so that is the usual case (#286). Only meaningful when the
    /// slice's Y axis really is a latitude; a cross-section against level or
    /// time has no north to face and stays in storage order.
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
