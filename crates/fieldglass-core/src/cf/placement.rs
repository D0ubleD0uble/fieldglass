//! The resolver of resolvers: which geometry one array's 2-D slice has
//! (moved from `fieldglass-netcdf`, #704).
//!
//! [`super::resolvers`] ships a resolver for every family a container of named
//! arrays can state — `synthesize_geometry` for a 1-D lat/lon pair,
//! `resolve_wrf_*` for WRF's `MAP_PROJ` domains, `resolve_cf_geostationary`
//! for a CF geostationary CRS, a spatial index over 2-D coordinates — and this
//! chooses between them. Until #549 the precedence lived in `fieldglass-napi`;
//! until #704 it lived in `fieldglass-netcdf`, where a Zarr store could not
//! reach it.
//!
//! # The precedence, and why it is this order
//!
//! 1. **2-D coordinates (curvilinear).** They give the position of every cell
//!    outright. No formula below could reconstruct them, so nothing may take
//!    priority over them.
//! 2. **WRF `MAP_PROJ`.** A WRF domain's `x`/`y` are projected metres. It is
//!    checked before CF because WRF files carry both, and the global attributes
//!    are the more specific statement.
//! 3. **CF `grid_mapping`.** `geostationary` resolves; `latitude_longitude`
//!    falls through to (4); **anything else stops here** — see the guard below.
//! 4. **1-D lat/lon coordinate arrays.** Reached only when no projection
//!    resolved, which CF makes safe: a projected CRS is *required* to name a
//!    `grid_mapping`, so its absence implies geographic coordinates. Only when
//!    the Y axis's coordinate is a latitude and the X axis's a longitude,
//!    though: a cross-section picks any two axes (#171), and a time or level
//!    coordinate read as degrees placed a time–latitude plane on the map.
//! 5. **Nothing.** [`GridGeometry::Unsupported`], the source-only fallback.
//!
//! # The guard that matters
//!
//! An unrecognised *projected* `grid_mapping` must never fall through to (4).
//! Its `x`/`y` coordinate arrays hold metres, and the 1-D path would read them
//! as degrees — a Lambert domain over the United States rendered onto the
//! equator off West Africa, silently, with a plausible-looking picture. So an
//! unclassified mapping returns `Unsupported` rather than continuing, and that
//! is what [`CfMapping::Unsupported`] exists to express.
//!
//! # Reading coordinates
//!
//! Every coordinate is read whole through [`ArraySource::read_region`], raw,
//! and scaled by its own `scale_factor` / `add_offset`. Nothing here memoises:
//! a caller placing many slices of one container holds the source, and a
//! decode cache is the host's to own.

use std::ops::Range;

use super::geometry::{AxisKind, Catalog, Entry, detect_axis, synthesize_geometry};
use super::resolvers::{
    WrfMapProj, cf_scale_offset, resolve_cf_geostationary, resolve_wrf_lambert, resolve_wrf_latlon,
    resolve_wrf_mercator, resolve_wrf_polar_stereo, wrf_map_proj,
};
use crate::FieldglassError;
use crate::array::{ArraySource, Attribute, AttributeValue, attribute};
use crate::projection::{GridGeometry, LatLonParams, Scan};
use crate::spatial_index::SpatialIndex;

/// How an array's CF `grid_mapping_name` routes through the precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfMapping {
    /// `geostationary` — resolve through the geostationary projector.
    Geostationary,
    /// `latitude_longitude` — a plain geographic CRS; use the coordinate arrays.
    LatLon,
    /// Any other mapping. **Projected until proven otherwise**, so the caller
    /// must fall back to source-only rather than reading `x`/`y` as degrees.
    Unsupported,
}

/// Classify a CF `grid_mapping` array's attributes.
///
/// A missing `grid_mapping_name` is [`CfMapping::LatLon`]: a data array can
/// point at a mapping array that omits the name, and for an unprojected file
/// the coordinate arrays are the right answer. That default is safe precisely
/// because CF requires a projected CRS to state its name — a file that omits it
/// is asserting it has nothing to state.
#[must_use]
pub fn classify_grid_mapping(attrs: &[Attribute]) -> CfMapping {
    match attribute(attrs, "grid_mapping_name")
        .and_then(AttributeValue::text)
        .map(str::trim)
    {
        Some("geostationary") => CfMapping::Geostationary,
        Some("latitude_longitude") | None => CfMapping::LatLon,
        Some(_) => CfMapping::Unsupported,
    }
}

/// A resolved slice: where its cells are, and the order they are stored in.
///
/// Two things rather than one because they answer different questions and only
/// one of them is a geometry. The scan is *not* derivable from the geometry it
/// travels with: a longitude axis counts as descending only when **every** step
/// runs east to west, and a grid whose corners happen to decrease across a
/// non-monotonic axis is not the same thing. Deriving the flag from the corners
/// would quietly reproject those files the wrong way round.
///
/// A container of named arrays states no scanning mode of its own — this is its
/// coordinate order read as the flags GRIB carries explicitly, which is what
/// lets `core` apply one rule to every format.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicePlacement {
    /// Where the cells are.
    pub geometry: GridGeometry,
    /// The storage order the coordinates imply, or `None` when the container
    /// states none.
    ///
    /// Only the 1-D lat/lon path reads an order out of a file at all. A WRF or
    /// CF projected domain has absorbed its direction into signed spacings and
    /// corner pinning, and a cell list has no axis to read an order from — for
    /// those this is `None`, which is a different answer from "north-down" and
    /// the reason it is an `Option`. A host reporting the row order should say
    /// nothing rather than guess, even though both spellings drive the same
    /// north-down default downstream.
    pub scan: Option<Scan>,
}

/// The label [`GridGeometry::Unsupported`] carries when nothing placed the grid.
///
/// Named rather than inlined at each of the several returns: a host prints it,
/// and three spellings of "we could not place this" would read as three
/// different outcomes.
pub const SOURCE_ONLY: &str = "source";

/// A placement for a family that reads no order out of the file.
fn unordered(geometry: GridGeometry) -> SlicePlacement {
    SlicePlacement {
        geometry,
        scan: None,
    }
}

fn source_only() -> GridGeometry {
    GridGeometry::Unsupported {
        label: SOURCE_ONLY.to_string(),
    }
}

fn no_such_array(array: &str) -> FieldglassError {
    FieldglassError::Parse(format!("this container holds no array {array:?}"))
}

/// Which geometry the 2-D slice of `array` on axes `(y_dim, x_dim)` has, and
/// the storage order beside it.
///
/// The precedence and its guard are described on [the module](self). `y_dim`
/// and `x_dim` are positions in the array's own axes.
///
/// Returns [`GridGeometry::Unsupported`] with the label [`SOURCE_ONLY`]
/// whenever no family could place the grid. That is a real answer, not an
/// error: the raster is still renderable in its own source projection, and it
/// is the *safe* answer for a projected CRS this build cannot resolve.
///
/// # Errors
///
/// An array the source does not hold, axes that are equal or out of range, a
/// coordinate that fails to read, and a masked value where a position has to
/// be — a 1-D axis, or a WRF corner — since shifting past it would move the
/// grid.
pub fn slice_placement(
    source: &dyn ArraySource,
    array: &str,
    y_dim: usize,
    x_dim: usize,
) -> Result<SlicePlacement, FieldglassError> {
    if y_dim == x_dim {
        return Err(FieldglassError::WrongLayout(
            "the X and Y axes must be different dimensions".to_string(),
        ));
    }
    let catalog = Catalog::new(source.group());
    let entry = catalog.find(array).ok_or_else(|| no_such_array(array))?;
    let (Some(y_axis), Some(x_axis)) = (entry.dims.get(y_dim), entry.dims.get(x_dim)) else {
        return Err(FieldglassError::out_of_range(
            y_dim.max(x_dim),
            entry.dims.len(),
        ));
    };
    let ni = u32::try_from(x_axis.length).unwrap_or(u32::MAX);
    let nj = u32::try_from(y_axis.length).unwrap_or(u32::MAX);

    // (1) 2-D coordinates.
    if let Some(lookup) = lookup(source, &catalog, entry, y_dim, x_dim)? {
        return Ok(unordered(lookup));
    }

    // (2) WRF `MAP_PROJ`.
    if let Some(geometry) = wrf_geometry(
        source,
        &catalog,
        &entry.group,
        &y_axis.name,
        &x_axis.name,
        ni,
        nj,
    )? {
        return Ok(unordered(geometry));
    }

    // (3) CF `grid_mapping`.
    if let Some(gm_attrs) = grid_mapping_attrs(&catalog, entry) {
        match classify_grid_mapping(gm_attrs) {
            CfMapping::Geostationary => {
                let x = coordinate_values_for_dim(source, &catalog, &x_axis.name)?;
                let y = coordinate_values_for_dim(source, &catalog, &y_axis.name)?;
                return Ok(unordered(match (x, y) {
                    (Some(x), Some(y)) => resolve_cf_geostationary(gm_attrs, &x, &y)
                        .as_ref()
                        .map_or_else(source_only, GridGeometry::from),
                    _ => source_only(),
                }));
            }
            // Fall through to the coordinate arrays.
            CfMapping::LatLon => {}
            // **The guard.** Projected until proven otherwise.
            CfMapping::Unsupported => return Ok(unordered(source_only())),
        }
    }

    // (4) 1-D lat/lon coordinate arrays: latitude down the rows and longitude
    // across the columns, and nothing else. A cross-section's axes are a time,
    // a level or a transposed pair (#171); none of those is a map, and reading
    // their coordinates as degrees would draw one anyway.
    let (Some(lat), Some(lon)) = (
        catalog
            .coordinate_of(&y_axis.name)
            .filter(|e| detect_axis(e.array) == Some(AxisKind::Latitude)),
        catalog
            .coordinate_of(&x_axis.name)
            .filter(|e| detect_axis(e.array) == Some(AxisKind::Longitude)),
    ) else {
        return Ok(unordered(source_only()));
    };
    let lat = coordinate_values(source, lat)?;
    let lon = coordinate_values(source, lon)?;
    let g = synthesize_geometry(&lat, &lon)?;
    Ok(SlicePlacement {
        geometry: GridGeometry::LatLon(LatLonParams {
            ni: g.ni,
            nj: g.nj,
            lat_first: g.lat_first,
            lon_first: g.lon_first,
            lat_last: g.lat_last,
            lon_last: g.lon_last,
        }),
        // The only file-derived scan there is. `lon_descending` is the whole
        // axis running east to west, not merely its corners decreasing —
        // see [`SlicePlacement`].
        scan: Some(Scan::new(g.lon_descending, g.lat_ascending, false)),
    })
}

/// The spatial index over an array's 2-D coordinate pair, as a
/// [`GridGeometry::Lookup`], or `None` when it names no usable pair for these
/// axes.
///
/// Public because building it walks and copies every cell, so a caller placing
/// the same array repeatedly — a probe runs per mouse move — wants to hold the
/// result rather than have [`slice_placement`] rebuild it.
///
/// # Errors
///
/// An array the source does not hold, and a coordinate that fails to read.
pub fn curvilinear_lookup(
    source: &dyn ArraySource,
    array: &str,
    y_dim: usize,
    x_dim: usize,
) -> Result<Option<GridGeometry>, FieldglassError> {
    let catalog = Catalog::new(source.group());
    let entry = catalog.find(array).ok_or_else(|| no_such_array(array))?;
    lookup(source, &catalog, entry, y_dim, x_dim)
}

fn lookup(
    source: &dyn ArraySource,
    catalog: &Catalog<'_>,
    entry: &Entry<'_>,
    y_dim: usize,
    x_dim: usize,
) -> Result<Option<GridGeometry>, FieldglassError> {
    let (Some(y_axis), Some(x_axis)) = (entry.dims.get(y_dim), entry.dims.get(x_dim)) else {
        return Ok(None);
    };
    let Some(pair) = catalog
        .curvilinear(entry)
        .filter(|pair| pair.y_dim == y_axis.name && pair.x_dim == x_axis.name)
    else {
        return Ok(None);
    };
    let (Some(lat), Some(lon)) = (catalog.find(&pair.lat), catalog.find(&pair.lon)) else {
        return Ok(None);
    };
    let lats = plane(source, lat)?;
    let lons = plane(source, lon)?;
    Ok(SpatialIndex::new(
        u32::try_from(x_axis.length).unwrap_or(u32::MAX),
        u32::try_from(y_axis.length).unwrap_or(u32::MAX),
        &lats,
        &lons,
    )
    .map(GridGeometry::Lookup))
}

/// A 2-D auxiliary coordinate array, whole and CF-scaled, with a masked cell
/// carried through as `NaN` rather than refused.
///
/// A 1-D axis is refused on a masked cell (see [`slice_placement`]); a 2-D
/// coordinate array is a field of positions and may legitimately be masked
/// outside a swath. Public so a host checking where a swath's cells landed
/// reads the same array the index was built from.
///
/// # Errors
///
/// An array the source does not hold, and a failure reading it.
pub fn coordinate_plane(
    source: &dyn ArraySource,
    array: &str,
) -> Result<Vec<f64>, FieldglassError> {
    let catalog = Catalog::new(source.group());
    let entry = catalog.find(array).ok_or_else(|| no_such_array(array))?;
    plane(source, entry)
}

/// Every stored value of an array, raw.
///
/// The whole array rather than a plane cut with the *data* array's axis
/// positions: a rank-3 data array's `(1, 2)` are not a rank-2 coordinate's
/// axes, and a coordinate that carries a leading `Time` has its first step
/// first in C order, which is the step being placed.
fn whole(source: &dyn ArraySource, entry: &Entry<'_>) -> Result<Vec<Option<f64>>, FieldglassError> {
    let region: Vec<Range<u64>> = entry.dims.iter().map(|d| 0..d.length).collect();
    source.read_region(&entry.name, &region)
}

fn plane(source: &dyn ArraySource, entry: &Entry<'_>) -> Result<Vec<f64>, FieldglassError> {
    let raw: Vec<f64> = whole(source, entry)?
        .into_iter()
        .map(|v| v.unwrap_or(f64::NAN))
        .collect();
    Ok(super::resolvers::apply_scale_offset(
        &raw,
        &entry.array.attributes,
    ))
}

/// A 1-D coordinate axis, CF-scaled.
///
/// **A fill value here is a hard error**, and that asymmetry with a 2-D
/// coordinate is the point: an axis is a monotonic run of positions, so a hole
/// in it means the corners and the spacing derived from it are not what the
/// file says.
fn coordinate_values(
    source: &dyn ArraySource,
    entry: &Entry<'_>,
) -> Result<Vec<f64>, FieldglassError> {
    let raw: Vec<f64> = whole(source, entry)?
        .into_iter()
        .map(|v| {
            v.ok_or_else(|| {
                FieldglassError::Parse("coordinate variable contains a fill value".to_string())
            })
        })
        .collect::<Result<_, _>>()?;
    Ok(super::resolvers::apply_scale_offset(
        &raw,
        &entry.array.attributes,
    ))
}

/// The 1-D coordinate axis of a dimension, or `None` when it has none.
fn coordinate_values_for_dim(
    source: &dyn ArraySource,
    catalog: &Catalog<'_>,
    dim_name: &str,
) -> Result<Option<Vec<f64>>, FieldglassError> {
    match catalog.coordinate_of(dim_name) {
        Some(entry) => Ok(Some(coordinate_values(source, entry)?)),
        None => Ok(None),
    }
}

/// The attributes of the `grid_mapping` array a data array points at.
fn grid_mapping_attrs<'g>(catalog: &Catalog<'g>, entry: &Entry<'_>) -> Option<&'g [Attribute]> {
    let name = attribute(&entry.array.attributes, "grid_mapping")?.text()?;
    catalog
        .resolve(&entry.group, name)
        .map(|gm| gm.array.attributes.as_slice())
}

/// Whether an array's last two axes are `y` then `x`.
///
/// WRF's `XLAT` is `(Time, south_north, west_east)`, so the leading axes are
/// ignored and only the trailing pair has to match the slice being placed.
fn dims_end_with(entry: &Entry<'_>, y: &str, x: &str) -> bool {
    matches!(entry.dims.as_slice(), [.., dy, dx] if dy.name == y && dx.name == x)
}

/// One cell of a 2-D coordinate field at a flat C-order index, CF-scaled.
///
/// **A masked corner is a hard error.** Shifting to the next present cell
/// would move the origin and mis-georeference the whole domain, which is the
/// failure this guard exists to prevent rather than to survive.
fn corner_value(
    source: &dyn ArraySource,
    entry: &Entry<'_>,
    name: &str,
    flat_index: usize,
) -> Result<f64, FieldglassError> {
    let raw = whole(source, entry)?
        .get(flat_index)
        .copied()
        .flatten()
        .ok_or_else(|| {
            FieldglassError::Parse(format!("{name}[{flat_index}] is missing or masked"))
        })?;
    let (scale, offset) = cf_scale_offset(&entry.array.attributes);
    Ok(raw * scale + offset)
}

/// A WRF projected grid, when the container carries the `MAP_PROJ` globals and
/// the 2-D `XLAT`/`XLONG` whose `(0, 0)` cell fixes the origin.
fn wrf_geometry(
    source: &dyn ArraySource,
    catalog: &Catalog<'_>,
    group: &str,
    y_name: &str,
    x_name: &str,
    ni: u32,
    nj: u32,
) -> Result<Option<GridGeometry>, FieldglassError> {
    let (Some(xlat), Some(xlong)) = (
        catalog.resolve(group, "XLAT"),
        catalog.resolve(group, "XLONG"),
    ) else {
        return Ok(None);
    };
    // The corner reads below index `(0, 0)` of a plane whose last two axes
    // must be the ones being placed; a file whose `XLAT` is shaped differently
    // is not the domain this array lives on.
    if !dims_end_with(xlat, y_name, x_name) || !dims_end_with(xlong, y_name, x_name) {
        return Ok(None);
    }
    // `MAP_PROJ` is matched *before* the corner cells are read: a file whose
    // projection this build does not resolve keeps its source-only fallback
    // even when a corner is masked, and pays no coordinate decode. A masked
    // corner on a projection that *does* resolve stays a hard error rather
    // than silently mis-georeferencing.
    let Some(map_proj) = wrf_map_proj(catalog.globals) else {
        return Ok(None);
    };
    let lat_first = corner_value(source, xlat, "XLAT", 0)?;
    let lon_first = corner_value(source, xlong, "XLONG", 0)?;
    let globals = catalog.globals;
    // The `(lat, lon)` of the far corner — `[nj - 1, ni - 1]` in C order.
    // Mercator and unrotated lat/lon are corner-pinned, so they alone need it.
    let far_corner = || -> Result<(f64, f64), FieldglassError> {
        let flat = (nj.saturating_sub(1) as usize) * ni as usize + (ni.saturating_sub(1) as usize);
        Ok((
            corner_value(source, xlat, "XLAT", flat)?,
            corner_value(source, xlong, "XLONG", flat)?,
        ))
    };
    Ok(match map_proj {
        WrfMapProj::Lambert => resolve_wrf_lambert(globals, lat_first, lon_first, ni, nj)
            .as_ref()
            .map(GridGeometry::from),
        WrfMapProj::PolarStereo => resolve_wrf_polar_stereo(globals, lat_first, lon_first, ni, nj)
            .as_ref()
            .map(GridGeometry::from),
        WrfMapProj::Mercator => {
            let (lat_last, lon_last) = far_corner()?;
            resolve_wrf_mercator(globals, lat_first, lon_first, lat_last, lon_last, ni, nj)
                .as_ref()
                .map(GridGeometry::from)
        }
        WrfMapProj::LatLon => {
            let (lat_last, lon_last) = far_corner()?;
            resolve_wrf_latlon(globals, lat_first, lon_first, lat_last, lon_last, ni, nj).map(|g| {
                GridGeometry::LatLon(LatLonParams {
                    ni: g.ni,
                    nj: g.nj,
                    lat_first: g.lat_first,
                    lon_first: g.lon_first,
                    lat_last: g.lat_last,
                    lon_last: g.lon_last,
                })
            })
        }
    })
}
