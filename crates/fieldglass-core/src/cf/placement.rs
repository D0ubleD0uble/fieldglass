//! Functions that decide where one array's 2-D slice sits on the Earth.
//!
//! A file can say where its grid is in several ways. [`super::resolvers`]
//! reads each one, and [`slice_placement`] decides which applies, trying them
//! in a fixed order. The result is a [`SlicePlacement`]: where the cells are,
//! as a [`GridGeometry`], and the order they are stored in.
//!
//! # The order, and why
//!
//! 1. 2-D coordinates. A pair of 2-D latitude and longitude arrays gives the
//!    position of every cell outright, and no formula could do better, so they
//!    come first. [`curvilinear_lookup`] builds a spatial index over them, and
//!    [`coordinate_plane`] reads each array. A pair whose latitudes run past
//!    the poles places nothing, and the precedence stops there. A latitude
//!    cell the array's own attributes call missing, a fill value or a value
//!    outside `valid_range`, `valid_min` or `valid_max`, has no position and
//!    is not checked. A pair with no cell left, although some hold a value,
//!    places nothing either (#1727).
//! 2. WRF's `MAP_PROJ`. A WRF domain's `x` and `y` are in projected meters.
//!    WRF files carry CF attributes as well, and WRF's own global attributes
//!    are the more specific, so they are checked first.
//! 3. CF's `grid_mapping`. [`classify_grid_mapping`] sorts the mapping into a
//!    [`CfMapping`]. A `geostationary` mapping is placed, or refused when its
//!    numbers describe no satellite view. It is placed only when the X axis's
//!    coordinate is the `x` scan angle and the Y axis's is `y`. Swapped, the
//!    formula would place a real but wrong grid, and when nothing says which
//!    is which, the slice isn't placed. A `latitude_longitude` mapping falls
//!    through to step 4. Any other mapping stops here; see the guard below.
//! 4. 1-D latitude and longitude arrays. Reached only when no projection
//!    applied. CF requires a projected grid to name a `grid_mapping`, so
//!    without one the coordinates are geographic. The Y axis's coordinate has
//!    to be a latitude and the X axis's a longitude: a cross-section can pick
//!    any two axes, and reading a time or level axis as degrees would put a
//!    time–latitude plane on the map. Their values have to be possible
//!    degrees too, however the axes were read, or the slice is not placed: a
//!    latitude inside -90 to 90, and a longitude axis running at most twice
//!    around the globe and staying inside -720 to 720. A projected `y` and
//!    `x` in meters that lost the attributes saying so are read as latitude
//!    and longitude from their names, and usually fail one of these (#1719).
//! 5. Nothing. [`GridGeometry::Unsupported`], which shows the slice without a
//!    map, labeled [`SOURCE_ONLY`].
//!
//! # The guard that matters
//!
//! A projected `grid_mapping` this code doesn't recognize must never fall
//! through to step 4. Its `x` and `y` arrays hold meters, and step 4 would read
//! them as degrees. A Lambert grid over the United States would be drawn on
//! the equator off West Africa, silently, as a plausible-looking picture. So
//! an unrecognized mapping stops at [`CfMapping::Unsupported`].
//!
//! # Reading coordinates
//!
//! Each coordinate is read whole through [`ArraySource::read_region`], and
//! scaled by its own `scale_factor` and `add_offset`. Nothing here caches: a
//! host placing many slices of one file keeps its own cache.
//!
//! A value at a fill value the coordinate states, its own `_FillValue` or
//! `missing_value`, is masked wherever it sits, whichever container read it
//! (#1305). A fill value the container only presents, as a Zarr store
//! presents a v2 `fill_value` ([`ArraySource::presents_attribute`]), masks
//! nothing. A latitude within 0.001° past a pole is read as that pole,
//! exactly, so an `f32` 90 is a row a box up to the pole holds.

use std::ops::Range;

use super::geometry::{
    AxisKind, Catalog, Entry, detect_axis, pole_snapped, snap_to_poles, synthesize_geometry,
    values_out_of_range,
};
use super::resolvers::{
    WrfMapProj, cf_scale_offset, resolve_cf_geostationary, resolve_wrf_lambert, resolve_wrf_latlon,
    resolve_wrf_mercator, resolve_wrf_polar_stereo, wrf_map_proj,
};
use crate::FieldglassError;
use crate::array::{
    ArrayDescription, ArraySource, Attribute, AttributeValue, CfUnpacking, attribute,
};
use crate::projection::{GridFingerprint, GridGeometry, LatLonParams, Scan};
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
        declared: None,
    }
}

/// A CF `geostationary` mapping whose numbers describe no camera, declined
/// under the family the file states.
///
/// `space_view` rather than [`SOURCE_ONLY`], because the file did state a
/// family this build models: a GRIB2 §3.90 with the same defect is declined as
/// `space_view`, and the refusals then say its geometry could not be built
/// rather than that nothing placed it (#961). That covers a required attribute
/// that is missing as well as one that is out of range, since both are the
/// mapping's own numbers building no grid. A mapping whose slice has no `x`/`y`
/// coordinate arrays stays [`SOURCE_ONLY`]: nothing in the file says where its
/// cells are, which is the case that label exists for. So do axes whose units
/// are no angle or length: the mapping is stated, and only its coordinates
/// cannot be read as scan angles (#966).
///
/// The fingerprint folds in what the resolver reads, in a fixed order, and both
/// axes, so two slices that state the same mapping still combine and two that
/// do not are refused (#962). Only those: a container lists attributes in its
/// own order and carries others (`long_name`, a fill value) that do not move
/// a cell, so the same mapping written as classic NetCDF, NetCDF-4 or Zarr has
/// to fingerprint alike. A numeric attribute is read as its first number and a
/// missing one is recorded as absent, as the resolver reads them; the sweep
/// axis is folded as the resolver interprets it, since an absent one is `x`.
/// Each axis's units are folded beside its values, trimmed, as the resolver
/// reads them, since the same numbers in metres and in radians are different
/// grids.
fn declined_space_view(gm_attrs: &[Attribute], x: &Axis, y: &Axis) -> GridGeometry {
    let number = |f: GridFingerprint, value: Option<f64>| match value {
        // One NaN for every payload: a container may write any of them.
        Some(v) if v.is_nan() => f.with_bytes(&[2]),
        Some(v) => f.with_bytes(&[1]).with_bytes(&v.to_bits().to_le_bytes()),
        None => f.with_bytes(&[0]),
    };
    let text = |name| attribute(gm_attrs, name).and_then(AttributeValue::text);
    let mut f = GridFingerprint::EMPTY;
    let name = text("grid_mapping_name").unwrap_or_default().trim();
    f = f
        .with_bytes(&(name.len() as u64).to_le_bytes())
        .with_bytes(name.as_bytes());
    for name in [
        "perspective_point_height",
        "semi_major_axis",
        "semi_minor_axis",
        "longitude_of_projection_origin",
    ] {
        f = number(
            f,
            attribute(gm_attrs, name).and_then(AttributeValue::number),
        );
    }
    let sweep_x = text("sweep_angle_axis").is_none_or(|s| s.trim() != "y");
    f = f.with_bytes(&[u8::from(sweep_x)]);
    for axis in [x, y] {
        f = match axis.units() {
            Some(units) => f
                .with_bytes(&[1])
                .with_bytes(&(units.len() as u64).to_le_bytes())
                .with_bytes(units.as_bytes()),
            None => f.with_bytes(&[0]),
        };
        f = f.with_bytes(&(axis.values.len() as u64).to_le_bytes());
        f = axis.values.iter().fold(f, |f, v| number(f, Some(*v)));
    }
    GridGeometry::Unsupported {
        // `GridGeometry::Geostationary`'s kind, which `declined_family` reads.
        label: "space_view".to_string(),
        declared: Some(f),
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
/// is the *safe* answer for a projected CRS this build cannot resolve. The one
/// exception is a `geostationary` mapping whose own numbers describe no
/// camera, which is declined as `space_view`, the family it states (#961).
///
/// # Errors
///
/// An array the source does not hold, axes that are equal or out of range, a
/// coordinate that fails to read, and a masked or non-finite value where a
/// position has to be — a 1-D axis, or a WRF corner — since shifting past it
/// would move the grid (#1172).
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

    // (1) 2-D coordinates. A pair whose latitudes run past the poles stops
    // here rather than falling through: `Session`'s memo keyed on the pair
    // answers for every field that names it, so the answer must not depend
    // on the field.
    match lookup(source, &catalog, entry, y_dim, x_dim)? {
        Pair::Placed(lookup) => return Ok(unordered(lookup)),
        Pair::NoDegrees => return Ok(unordered(source_only())),
        Pair::None => {}
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
                // The scan angles in their own order: `x` across the columns
                // and `y` down the rows. Picked the other way round, the
                // formula places a real but wrong grid (#1387), so a pair not
                // named in that order is drawn source-only, as the other
                // branches treat a transposed pair.
                let in_order = |dim: &str, want: AxisKind| {
                    catalog
                        .coordinate_of(dim)
                        .is_some_and(|e| projection_axis(e.array) == Some(want))
                };
                if !in_order(&x_axis.name, AxisKind::Longitude)
                    || !in_order(&y_axis.name, AxisKind::Latitude)
                {
                    return Ok(unordered(source_only()));
                }
                let x = axis_for_dim(source, &catalog, &x_axis.name)?;
                let y = axis_for_dim(source, &catalog, &y_axis.name)?;
                return Ok(unordered(match (x, y) {
                    (Some(x), Some(y)) => resolve_cf_geostationary(
                        gm_attrs,
                        &x.values,
                        x.units(),
                        &y.values,
                        y.units(),
                    )
                    .as_ref()
                    .map_or_else(|| declined_space_view(gm_attrs, &x, &y), GridGeometry::from),
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
    let mut lat = coordinate_values(source, lat)?;
    let lon = coordinate_values(source, lon)?;
    // A pole stored a hair past 90 is the pole (#1305); one further out is
    // judged below.
    snap_to_poles(&mut lat);
    let g = synthesize_geometry(&lat, &lon)?;
    // Values no latitude or longitude can hold, such as a projected `y` and
    // `x` in meters read by their names (#1719), place nothing. The order
    // the axes give the rows and columns still stands.
    let geometry = if values_out_of_range(AxisKind::Latitude, &lat).is_some()
        || values_out_of_range(AxisKind::Longitude, &lon).is_some()
    {
        source_only()
    } else {
        GridGeometry::LatLon(LatLonParams {
            ni: g.ni,
            nj: g.nj,
            lat_first: g.lat_first,
            lon_first: g.lon_first,
            lat_last: g.lat_last,
            lon_last: g.lon_last,
        })
    };
    Ok(SlicePlacement {
        geometry,
        // The only file-derived scan there is. `lon_descending` is the whole
        // axis running east to west, not merely its corners decreasing —
        // see [`SlicePlacement`].
        scan: Some(Scan::new(g.lon_descending, g.lat_ascending, false)),
    })
}

/// The spatial index over an array's 2-D coordinate pair, as a
/// [`GridGeometry::Lookup`], or `None` when it names no usable pair for these
/// axes. A pair whose latitudes run past the poles, by
/// [`values_out_of_range`], is not usable (#1719). A cell the latitude array's
/// own fill values or valid bounds call missing has no position and is not
/// counted. So when every latitude cell holding a value is outside those
/// bounds, every cell is missing, no usable pair is left, and the result is
/// `None` (#1727).
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
    Ok(match lookup(source, &catalog, entry, y_dim, x_dim)? {
        Pair::Placed(geometry) => Some(geometry),
        Pair::NoDegrees | Pair::None => None,
    })
}

/// What a slice's 2-D coordinate pair says.
enum Pair {
    /// No usable pair spans these axes.
    None,
    /// The pair spans them, and its latitudes run past the poles (#1719), or
    /// every cell holding a value is outside the bounds the array states
    /// (#1727).
    NoDegrees,
    /// The pair's spatial index.
    Placed(GridGeometry),
}

fn lookup(
    source: &dyn ArraySource,
    catalog: &Catalog<'_>,
    entry: &Entry<'_>,
    y_dim: usize,
    x_dim: usize,
) -> Result<Pair, FieldglassError> {
    let (Some(y_axis), Some(x_axis)) = (entry.dims.get(y_dim), entry.dims.get(x_dim)) else {
        return Ok(Pair::None);
    };
    let Some(pair) = catalog
        .curvilinear(entry)
        .filter(|pair| pair.y_dim == y_axis.name && pair.x_dim == x_axis.name)
    else {
        return Ok(Pair::None);
    };
    let (Some(lat), Some(lon)) = (catalog.find(&pair.lat), catalog.find(&pair.lon)) else {
        return Ok(Pair::None);
    };
    let Some(lats) = pair_latitudes(source, lat)? else {
        return Ok(Pair::NoDegrees);
    };
    let lons = pair_degrees(source, lon, AxisKind::Longitude)?.values;
    Ok(SpatialIndex::new(
        u32::try_from(x_axis.length).unwrap_or(u32::MAX),
        u32::try_from(y_axis.length).unwrap_or(u32::MAX),
        &lats,
        &lons,
    )
    .map_or(Pair::None, |index| {
        Pair::Placed(GridGeometry::Lookup(index))
    }))
}

/// The latitudes of a 2-D pair, whole and in degrees, with `NaN` for a cell
/// that has no position, or `None` when they are no latitudes and the pair
/// places nothing (#1719).
///
/// A cell the array's own attributes call missing, a stated fill value or a
/// value outside `valid_range`, `valid_min` or `valid_max`, has no position,
/// as CF says, and is not checked (#1727). Entered at its stored value, a
/// latitude of 95 outside `valid_range = -90, 90` would sit at 85 on the far
/// side of the pole. A pole stored a hair past 90 is the pole (#1305). The
/// pair is refused when a cell left runs past the poles, by
/// [`values_out_of_range`], and when no cell is left although some hold a
/// stored value: every one of them is outside the bounds the array states,
/// and an index over none would not say why.
///
/// A fill the container only presents is read as [`pair_degrees`] says.
fn pair_latitudes(
    source: &dyn ArraySource,
    lat: &Entry<'_>,
) -> Result<Option<Vec<f64>>, FieldglassError> {
    let lats = pair_degrees(source, lat, AxisKind::Latitude)?;
    let past = lats
        .range
        .is_some_and(|(lo, hi)| values_out_of_range(AxisKind::Latitude, &[lo, hi]).is_some());
    Ok((!past && (lats.left || !lats.stored)).then_some(lats.values))
}

/// One array of a 2-D pair in degrees, as [`pair_degrees`] reads it.
struct PairDegrees {
    /// Every cell, `NaN` where it has no position.
    values: Vec<f64>,
    /// Whether any cell holds a finite stored value the array's own
    /// attributes do not call missing by its fill.
    stored: bool,
    /// Whether any cell is left with a finite position.
    left: bool,
    /// The lowest and highest position left.
    range: Option<(f64, f64)>,
}

/// One array of a 2-D pair, whole and in degrees, with `NaN` for a cell that
/// has no position.
///
/// A cell at a fill value the array states has no position. A latitude's
/// `valid_range`, `valid_min` and `valid_max` are read as well (see
/// [`pair_latitudes`]); a longitude's are not, as a 1-D longitude axis does
/// not read them either. A latitude a hair past a pole is the pole.
///
/// A fill the container only presents, a Zarr v2 `fill_value` the
/// attributes do not state (see [`masked`]), masks a cell only when the
/// cell at it is no latitude, or no longitude, by [`values_out_of_range`].
/// xarray's v2 writer keeps a coordinate's fill there alone, so a swath's
/// -999 cells are its missing ones and were skipped in 0.6.1; read as
/// latitudes they refused the whole pair, and a -999 longitude beside a good
/// latitude sat at 81°E. A presented fill that is a latitude or longitude,
/// zarr-python's default 0 among them, is a position. Like any fill, it is a
/// stored value: it is compared with the cell before the scale and offset,
/// and is no degree when it is none either stored or unpacked, so a packed
/// latitude's -999 is missing although -999 hundredths of a degree would be
/// one (#1727).
///
/// One pass and one array of degrees, judged as it is built: a copy of the
/// cells left, made only to judge them, cost 8 bytes a cell at RTOFS size
/// (#1727). The degrees are a new array beside the stored cells rather than
/// collected in their place, since a `Vec<f64>` collected from a
/// `Vec<Option<f64>>` reuses its allocation and so holds 16 bytes a cell,
/// not 8, for as long as the index is being built.
fn pair_degrees(
    source: &dyn ArraySource,
    entry: &Entry<'_>,
    kind: AxisKind,
) -> Result<PairDegrees, FieldglassError> {
    let rule = stated_rule(source, entry);
    let (scale, offset) = cf_scale_offset(&entry.array.attributes);
    let unpacked = |v: f64| match kind {
        AxisKind::Latitude => rule.value(v),
        AxisKind::Longitude => Some(v * scale + offset),
    };
    let presented = |name: &str| match attribute(&entry.array.attributes, name) {
        Some(value) if source.presents_attribute(&entry.name, name) => value.numbers(),
        _ => &[],
    };
    let (fill, missing) = (presented("_FillValue"), presented("missing_value"));
    let no_degree = |v: f64| values_out_of_range(kind, &[v]).is_some();
    let (mut stored, mut left) = (false, false);
    let mut range: Option<(f64, f64)> = None;
    let values: Vec<f64> = masked(source, entry)?
        .iter()
        .map(|&cell| {
            let at_fill = cell.is_some_and(|v| fill.contains(&v) || missing.contains(&v));
            // At a presented fill, and no degree as stored: missing.
            if at_fill && cell.is_some_and(no_degree) {
                return f64::NAN;
            }
            let Some(v) = cell.and_then(unpacked) else {
                stored |= cell.is_some_and(f64::is_finite);
                return f64::NAN;
            };
            // Or no degree once unpacked.
            if at_fill && no_degree(v) {
                return f64::NAN;
            }
            let v = match kind {
                AxisKind::Latitude => pole_snapped(v).unwrap_or(v),
                AxisKind::Longitude => v,
            };
            stored |= cell.is_some_and(f64::is_finite);
            if v.is_finite() {
                left = true;
                range = Some(range.map_or((v, v), |(lo, hi)| (lo.min(v), hi.max(v))));
            }
            v
        })
        .collect();
    Ok(PairDegrees {
        values,
        stored,
        left,
        range,
    })
}

/// A 2-D auxiliary coordinate array, whole and CF-scaled, with a masked cell
/// carried through as `NaN` rather than refused.
///
/// A 1-D axis is refused on a masked cell (see [`slice_placement`]); a 2-D
/// coordinate array is a field of positions and may legitimately be masked
/// outside a swath. Public so a host checking where a swath's cells landed
/// reads the same array the index was built from.
///
/// So a 2-D latitude or longitude is read as [`curvilinear_lookup`] reads it
/// (#1727): a cell the index gives no position, such as one at a Zarr v2
/// `fill_value` that is no degree or a latitude outside its `valid_range`,
/// is `NaN` here too, and a latitude a hair past a pole is the pole. Any
/// other array is read with only the fill values it states masked.
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
    match detect_axis(entry.array) {
        Some(kind) if entry.dims.len() == 2 => Ok(pair_degrees(source, entry, kind)?.values),
        _ => plane(source, entry),
    }
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
    Ok(scaled(&masked(source, entry)?, entry))
}

/// [`whole`], with every value at a fill value the coordinate states masked
/// (#1305).
///
/// A coordinate's own `_FillValue` and `missing_value` say which stored
/// values are no position, wherever they sit, every entry of a multi-valued
/// `missing_value` included. A NetCDF reader masks the `_FillValue` and only
/// the first `missing_value` entry, as libnetcdf does; a Zarr store masks
/// none, so a latitude of -999 was placed as one. A fill value the container only presents, a Zarr v2
/// `fill_value` ([`ArraySource::presents_attribute`]), masks nothing: it is
/// written for every array, often as 0, a real longitude. `valid_range`,
/// `valid_min` and `valid_max` are left to the callers that read them.
fn masked(
    source: &dyn ArraySource,
    entry: &Entry<'_>,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let mut values = whole(source, entry)?;
    // Read off the attributes in place: a coordinate is read for every slice
    // placed, and most state no fill, so this allocates nothing (#743).
    let stated = |name: &str| match attribute(&entry.array.attributes, name) {
        Some(value) if !source.presents_attribute(&entry.name, name) => value.numbers(),
        _ => &[],
    };
    let (fill, missing) = (stated("_FillValue"), stated("missing_value"));
    if fill.is_empty() && missing.is_empty() {
        return Ok(values);
    }
    for v in &mut values {
        if v.is_some_and(|v| fill.contains(&v) || missing.contains(&v)) {
            *v = None;
        }
    }
    Ok(values)
}

/// The CF rule of the attributes a coordinate states, leaving out any the
/// container only presents (see [`masked`]).
fn stated_rule(source: &dyn ArraySource, entry: &Entry<'_>) -> CfUnpacking {
    let attributes = &entry.array.attributes;
    if !attributes
        .iter()
        .any(|a| source.presents_attribute(&entry.name, &a.name))
    {
        return CfUnpacking::from_attributes(attributes);
    }
    let stated: Vec<Attribute> = attributes
        .iter()
        .filter(|a| !source.presents_attribute(&entry.name, &a.name))
        .cloned()
        .collect();
    CfUnpacking::from_attributes(&stated)
}

/// Stored values scaled by the array's `scale_factor` and `add_offset`, with
/// an absent cell as `NaN`.
///
/// In one pass and one allocation: a 2-D coordinate is read whole, and a
/// second copy to scale it cost 8 bytes a cell (#1727).
fn scaled(stored: &[Option<f64>], entry: &Entry<'_>) -> Vec<f64> {
    let (scale, offset) = cf_scale_offset(&entry.array.attributes);
    let identity = scale == 1.0 && offset == 0.0;
    stored
        .iter()
        .map(|v| match v {
            Some(v) if identity => *v,
            Some(v) => v * scale + offset,
            None => f64::NAN,
        })
        .collect()
}

/// A 1-D coordinate axis, CF-scaled.
///
/// **A fill value here is a hard error**, and that asymmetry with a 2-D
/// coordinate is the point: an axis is a monotonic run of positions, so a hole
/// in it means the corners and the spacing derived from it are not what the
/// file says. CF says the same: a coordinate variable is strictly monotonic
/// and may hold no missing value (CF §1.3, after the NUG).
///
/// **So is a value that is not finite** (#1172). A NaN is the hole a masked
/// value is, without the mask: a Zarr coordinate whose fill value is NaN reads
/// as NaN wherever the store never wrote a chunk, and a file can write one
/// outright. Placed anyway, an all-NaN latitude gave a lat/lon grid with NaN
/// corners, and a NaN between two finite ends was placed from those ends as if
/// the axis were whole. An infinity places no cell either. Checked after the
/// scale and offset, which is where a position is read.
fn coordinate_values(
    source: &dyn ArraySource,
    entry: &Entry<'_>,
) -> Result<Vec<f64>, FieldglassError> {
    let raw: Vec<f64> = masked(source, entry)?
        .into_iter()
        .enumerate()
        .map(|(at, v)| v.ok_or_else(|| no_position(&entry.name, at, "a fill value")))
        .collect::<Result<_, _>>()?;
    let values = super::resolvers::apply_scale_offset(&raw, &entry.array.attributes);
    if let Some((at, v)) = values.iter().enumerate().find(|(_, v)| !v.is_finite()) {
        return Err(no_position(&entry.name, at, &v.to_string()));
    }
    Ok(values)
}

/// The refusal for a 1-D coordinate value that is no position: `what` sits at
/// `at` in the array `name`.
fn no_position(name: &str, at: usize, what: &str) -> FieldglassError {
    FieldglassError::Parse(format!(
        "coordinate {name:?} holds {what} at index {at}, so the slices it spans cannot be \
         placed: a coordinate's values must all be present and finite"
    ))
}

/// A 1-D coordinate axis: its scaled values and the attributes beside them.
struct Axis<'g> {
    values: Vec<f64>,
    attributes: &'g [Attribute],
}

impl Axis<'_> {
    /// The axis's `units`, trimmed, when it states them as text.
    fn units(&self) -> Option<&str> {
        attribute(self.attributes, "units")
            .and_then(AttributeValue::text)
            .map(str::trim)
    }
}

/// The 1-D coordinate axis of a dimension, or `None` when it has none.
fn axis_for_dim<'g>(
    source: &dyn ArraySource,
    catalog: &Catalog<'g>,
    dim_name: &str,
) -> Result<Option<Axis<'g>>, FieldglassError> {
    match catalog.coordinate_of(dim_name) {
        Some(entry) => Ok(Some(Axis {
            values: coordinate_values(source, entry)?,
            attributes: &entry.array.attributes,
        })),
        None => Ok(None),
    }
}

/// Which horizontal axis of a projected grid a coordinate is, with
/// [`AxisKind::Longitude`] standing for `x` and [`AxisKind::Latitude`] for `y`,
/// as that enum documents.
///
/// Its CF `standard_name` first: `projection_x_coordinate`, or the
/// `projection_x_angular_coordinate` CF gives a geostationary scan angle
/// (Appendix F), and the same for `y`. Otherwise whatever [`detect_axis`]
/// reads from its `axis` attribute or its name.
fn projection_axis(array: &ArrayDescription) -> Option<AxisKind> {
    match attribute(&array.attributes, "standard_name")
        .and_then(AttributeValue::text)
        .map(str::trim)
    {
        Some("projection_x_coordinate" | "projection_x_angular_coordinate") => {
            Some(AxisKind::Longitude)
        }
        Some("projection_y_coordinate" | "projection_y_angular_coordinate") => {
            Some(AxisKind::Latitude)
        }
        _ => detect_axis(array),
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
/// **A masked corner is a hard error**, and so is one that is not finite.
/// Shifting to the next present cell would move the origin and
/// mis-georeference the whole domain, which is the failure this guard exists
/// to prevent rather than to survive.
fn corner_value(
    source: &dyn ArraySource,
    entry: &Entry<'_>,
    name: &str,
    flat_index: usize,
) -> Result<f64, FieldglassError> {
    let raw = masked(source, entry)?
        .get(flat_index)
        .copied()
        .flatten()
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "coordinate {name:?} has no value at flat index {flat_index}, so the grid \
                 cannot be placed from its corner: a coordinate's values must all be present \
                 and finite"
            ))
        })?;
    let (scale, offset) = cf_scale_offset(&entry.array.attributes);
    let value = raw * scale + offset;
    // A NaN or an infinity is no origin either, and a resolver handed one
    // builds a grid with it (#1172).
    if !value.is_finite() {
        return Err(FieldglassError::Parse(format!(
            "coordinate {name:?} is {value} at flat index {flat_index}, so the grid cannot be \
             placed from its corner: a coordinate's values must all be present and finite"
        )));
    }
    Ok(value)
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
    // A corner latitude a hair past a pole is the pole. One further out
    // resolves no domain, as a projection the resolvers decline does, and
    // the slice is placed by what follows, as a 1-D latitude past the poles
    // is not placed (#1305).
    let Some(lat_first) = pole_snapped(corner_value(source, xlat, "XLAT", 0)?) else {
        return Ok(None);
    };
    let lon_first = corner_value(source, xlong, "XLONG", 0)?;
    let globals = catalog.globals;
    // The `(lat, lon)` of the far corner — `[nj - 1, ni - 1]` in C order.
    // Mercator and unrotated lat/lon are corner-pinned, so they alone need it.
    let far_corner = || -> Result<(Option<f64>, f64), FieldglassError> {
        // In `u64`: two file-stated dimensions overflow a 32-bit `usize`, and
        // a wrapped index would land inside the array and read the wrong cell
        // (#845). One `usize` cannot hold is past the end of any array.
        let flat =
            u64::from(nj.saturating_sub(1)) * u64::from(ni) + u64::from(ni.saturating_sub(1));
        let flat = usize::try_from(flat).map_err(|_| {
            FieldglassError::Parse(format!(
                "far-corner index {flat} is past what this target can index"
            ))
        })?;
        Ok((
            pole_snapped(corner_value(source, xlat, "XLAT", flat)?),
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
            let (Some(lat_last), lon_last) = far_corner()? else {
                return Ok(None);
            };
            resolve_wrf_mercator(globals, lat_first, lon_first, lat_last, lon_last, ni, nj)
                .as_ref()
                .map(GridGeometry::from)
        }
        WrfMapProj::LatLon => {
            let (Some(lat_last), lon_last) = far_corner()? else {
                return Ok(None);
            };
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
