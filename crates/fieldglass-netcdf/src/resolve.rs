//! The resolver of resolvers: which geometry one variable's 2-D slice has.
//!
//! This crate ships a resolver for every family it can read — `synthesize_geometry`
//! for a 1-D lat/lon pair, `resolve_wrf_*` for WRF's `MAP_PROJ` domains,
//! `resolve_cf_geostationary` for a CF geostationary CRS, `curvilinear_coords`
//! for 2-D coordinate arrays — and, until #549, nothing that *chose between
//! them*. The precedence lived in `fieldglass-napi`, which meant a consumer
//! calling `synthesize_geometry` directly on a Lambert-CRS file placed the
//! field in the Gulf of Guinea, and the guards that stop that were reachable
//! only through the Node addon.
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
//!    `grid_mapping`, so its absence implies geographic coordinates.
//! 5. **Nothing.** [`GridGeometry::Unsupported`], the source-only fallback.
//!
//! # The guard that matters
//!
//! An unrecognised *projected* `grid_mapping` must never fall through to (4).
//! Its `x`/`y` coordinate variables hold metres, and the 1-D path would read
//! them as degrees — a Lambert domain over the United States rendered onto the
//! equator off West Africa, silently, with a plausible-looking picture. So an
//! unclassified mapping returns `Unsupported` rather than continuing, and that
//! is what [`CfMapping::Unsupported`] exists to express.

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{Attribute, AttributeValue, attribute};
use fieldglass_core::projection::{GridGeometry, LatLonParams, Scan};
use fieldglass_core::spatial_index::SpatialIndex;

use crate::geometry::{DatasetView, RenderableVariable, VarView, synthesize_geometry};
use crate::projection::{
    WrfMapProj, resolve_cf_geostationary, resolve_wrf_lambert, resolve_wrf_latlon,
    resolve_wrf_mercator, resolve_wrf_polar_stereo, wrf_map_proj,
};
use crate::reader::NetcdfReader;

/// How a data variable's CF `grid_mapping_name` routes through the precedence.
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

/// Classify a CF `grid_mapping` variable's attributes.
///
/// A missing `grid_mapping_name` is [`CfMapping::LatLon`]: a data variable can
/// point at a mapping variable that omits the name, and for an unprojected file
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
/// NetCDF states no scanning mode of its own — this is the file's coordinate
/// order read as the flags GRIB carries explicitly, which is what lets `core`
/// apply one rule to both formats.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicePlacement {
    /// Where the cells are.
    pub geometry: GridGeometry,
    /// The storage order the coordinates imply, or `None` when the file states
    /// none.
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

impl NetcdfReader {
    /// Which geometry the 2-D slice of `var` on axes `(y_dim, x_dim)` has.
    ///
    /// The precedence and its guard are described on [the module](self). The
    /// `view` is passed in rather than rebuilt because resolving it walks the
    /// whole file, and a caller placing many slices holds one already.
    ///
    /// Returns [`GridGeometry::Unsupported`] with the label [`SOURCE_ONLY`]
    /// whenever no family could place the grid. That is a real answer, not an
    /// error: the raster is still renderable in its own source projection, and
    /// it is the *safe* answer for a projected CRS this build cannot resolve.
    pub fn slice_geometry(
        &self,
        view: &DatasetView,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
    ) -> Result<GridGeometry, FieldglassError> {
        Ok(self.slice_placement(view, var, y_dim, x_dim)?.geometry)
    }

    /// [`Self::slice_geometry`] and the storage order beside it.
    ///
    /// What a host that renders wants: placing the cells and knowing which way
    /// the rows and columns run are the same question asked of the same
    /// coordinate arrays, and reading them twice to answer it separately would
    /// be both slower and a chance for the two answers to disagree.
    pub fn slice_placement(
        &self,
        view: &DatasetView,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
    ) -> Result<SlicePlacement, FieldglassError> {
        if y_dim == x_dim {
            return Err(FieldglassError::WrongLayout(
                "the X and Y axes must be different dimensions".to_string(),
            ));
        }
        let (Some(y_axis), Some(x_axis)) = (var.dims.get(y_dim), var.dims.get(x_dim)) else {
            return Err(FieldglassError::out_of_range(
                y_dim.max(x_dim),
                var.dims.len(),
            ));
        };
        let ni = u32::try_from(x_axis.length).unwrap_or(u32::MAX);
        let nj = u32::try_from(y_axis.length).unwrap_or(u32::MAX);

        // (1) 2-D coordinates.
        if let Some(lookup) = self.curvilinear_index(view, var, y_dim, x_dim)? {
            return Ok(unordered(lookup));
        }

        // (2) WRF `MAP_PROJ`.
        if let Some(geometry) = self.wrf_geometry(view, &y_axis.name, &x_axis.name, ni, nj)? {
            return Ok(unordered(geometry));
        }

        // (3) CF `grid_mapping`.
        if let Some(gm_attrs) = grid_mapping_attrs(view, var) {
            match classify_grid_mapping(gm_attrs) {
                CfMapping::Geostationary => {
                    let x = self.coordinate_values_for_dim(view, &x_axis.name)?;
                    let y = self.coordinate_values_for_dim(view, &y_axis.name)?;
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

        // (4) 1-D lat/lon coordinate arrays.
        let (Some(lat_i), Some(lon_i)) = (
            view.coordinate_index(&y_axis.name),
            view.coordinate_index(&x_axis.name),
        ) else {
            return Ok(unordered(source_only()));
        };
        let lat = self.coordinate_values(view, lat_i)?;
        let lon = self.coordinate_values(view, lon_i)?;
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

    /// The spatial index over a variable's 2-D coordinate arrays, or `None`
    /// when it declares none.
    ///
    /// Public because building it walks and copies every cell, so a caller
    /// placing the same variable repeatedly — a probe runs per mouse move —
    /// wants to hold the result rather than have [`Self::slice_geometry`]
    /// rebuild it. The coordinate variables' indices are the identity to cache
    /// against: two data variables sharing one `XLAT`/`XLONG` pair share the
    /// index, which is the common case for a satellite product.
    ///
    /// Returns the wrapped [`GridGeometry::Lookup`] rather than the bare index,
    /// so a caller caches the same type [`Self::slice_geometry`] hands back and
    /// `fieldglass-core`'s `spatial_index` stays off this crate's public
    /// signatures.
    pub fn curvilinear_index(
        &self,
        view: &DatasetView,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
    ) -> Result<Option<GridGeometry>, FieldglassError> {
        let (Some(y_axis), Some(x_axis)) = (var.dims.get(y_dim), var.dims.get(x_dim)) else {
            return Ok(None);
        };
        let Some(source) = view
            .vars
            .iter()
            .find(|v| v.decode_index == var.decode_index)
        else {
            return Ok(None);
        };
        let Some(coords) = view.curvilinear_coords(source, &y_axis.name, &x_axis.name) else {
            return Ok(None);
        };
        let lats = self.coordinate_plane(view, coords.lat_index)?;
        let lons = self.coordinate_plane(view, coords.lon_index)?;
        Ok(SpatialIndex::new(
            u32::try_from(x_axis.length).unwrap_or(u32::MAX),
            u32::try_from(y_axis.length).unwrap_or(u32::MAX),
            &lats,
            &lons,
        )
        .map(GridGeometry::Lookup))
    }

    /// A WRF projected grid, when the file carries the `MAP_PROJ` globals and
    /// the 2-D `XLAT`/`XLONG` whose `(0, 0)` cell fixes the origin.
    fn wrf_geometry(
        &self,
        view: &DatasetView,
        y_name: &str,
        x_name: &str,
        ni: u32,
        nj: u32,
    ) -> Result<Option<GridGeometry>, FieldglassError> {
        let (Some(xlat), Some(xlong)) = (var_named(view, "XLAT"), var_named(view, "XLONG")) else {
            return Ok(None);
        };
        // The corner reads below index `(0, 0)` of a plane whose last two axes
        // must be the ones being placed; a file whose `XLAT` is shaped
        // differently is not the domain this variable lives on.
        if !dims_end_with(&xlat.array.dimensions, y_name, x_name)
            || !dims_end_with(&xlong.array.dimensions, y_name, x_name)
        {
            return Ok(None);
        }
        // `MAP_PROJ` is matched *before* the corner cells are read: a file whose
        // projection this build does not resolve keeps its source-only fallback
        // even when a corner is masked, and pays no coordinate decode. A masked
        // corner on a projection that *does* resolve stays a hard error rather
        // than silently mis-georeferencing.
        let Some(map_proj) = wrf_map_proj(&view.global_attrs) else {
            return Ok(None);
        };
        let lat_first = self.corner_value(view, xlat, "XLAT", 0)?;
        let lon_first = self.corner_value(view, xlong, "XLONG", 0)?;
        let globals = &view.global_attrs;
        Ok(match map_proj {
            WrfMapProj::Lambert => resolve_wrf_lambert(globals, lat_first, lon_first, ni, nj)
                .as_ref()
                .map(GridGeometry::from),
            WrfMapProj::PolarStereo => {
                resolve_wrf_polar_stereo(globals, lat_first, lon_first, ni, nj)
                    .as_ref()
                    .map(GridGeometry::from)
            }
            // Mercator and unrotated lat/lon are corner-pinned, so they alone
            // also need the far corner.
            WrfMapProj::Mercator => {
                let (lat_last, lon_last) = self.far_corner(view, xlat, xlong, ni, nj)?;
                resolve_wrf_mercator(globals, lat_first, lon_first, lat_last, lon_last, ni, nj)
                    .as_ref()
                    .map(GridGeometry::from)
            }
            WrfMapProj::LatLon => {
                let (lat_last, lon_last) = self.far_corner(view, xlat, xlong, ni, nj)?;
                resolve_wrf_latlon(globals, lat_first, lon_first, lat_last, lon_last, ni, nj).map(
                    |g| {
                        GridGeometry::LatLon(LatLonParams {
                            ni: g.ni,
                            nj: g.nj,
                            lat_first: g.lat_first,
                            lon_first: g.lon_first,
                            lat_last: g.lat_last,
                            lon_last: g.lon_last,
                        })
                    },
                )
            }
        })
    }
}

/// The attributes of the `grid_mapping` variable a data variable points at.
fn grid_mapping_attrs<'a>(
    view: &'a DatasetView,
    var: &RenderableVariable,
) -> Option<&'a [Attribute]> {
    let gm_name = view
        .var(var.decode_index)?
        .attribute("grid_mapping")?
        .text()?;
    var_named(view, gm_name).map(|gm| gm.array.attributes.as_slice())
}

/// Any variable by name — including the 2-D `XLAT`/`XLONG` and the scalar
/// `grid_mapping` carriers, which are not renderable.
fn var_named<'a>(view: &'a DatasetView, name: &str) -> Option<&'a VarView> {
    view.vars.iter().find(|v| v.array.name == name)
}

/// Whether a variable's last two dimensions are `y` then `x`.
///
/// WRF's `XLAT` is `(Time, south_north, west_east)`, so the leading axes are
/// ignored and only the trailing pair has to match the slice being placed.
fn dims_end_with(dims: &[String], y: &str, x: &str) -> bool {
    matches!(dims, [.., dy, dx] if dy == y && dx == x)
}

/// The coordinate reads the precedence needs.
///
/// Each decodes a whole coordinate variable. They are deliberately *not*
/// memoised here: a caller that places many slices of one file already holds
/// the [`DatasetView`], and a decode cache is the host's to own — which is what
/// `fieldglass-napi` does today and why these take an index rather than a
/// cached plane.
impl NetcdfReader {
    /// A 1-D coordinate axis, CF-scaled.
    ///
    /// **A fill value here is a hard error**, and that asymmetry with
    /// [`Self::coordinate_plane`] is the point: an axis is a monotonic run of
    /// positions, so a hole in it means the corners and the spacing derived
    /// from it are not what the file says. A 2-D coordinate array is a field of
    /// positions and may legitimately be masked outside a swath.
    fn coordinate_values(
        &self,
        view: &DatasetView,
        index: usize,
    ) -> Result<Vec<f64>, FieldglassError> {
        let raw: Vec<f64> = self
            .decode_variable_raw(index)?
            .into_iter()
            .map(|v| {
                v.ok_or_else(|| {
                    FieldglassError::Parse("coordinate variable contains a fill value".to_string())
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(crate::projection::apply_scale_offset(
            &raw,
            attrs_of(view, index),
        ))
    }

    /// The 1-D coordinate axis of a dimension, or `None` when it has none.
    fn coordinate_values_for_dim(
        &self,
        view: &DatasetView,
        dim_name: &str,
    ) -> Result<Option<Vec<f64>>, FieldglassError> {
        match view.coordinate_index(dim_name) {
            Some(i) => Ok(Some(self.coordinate_values(view, i)?)),
            None => Ok(None),
        }
    }

    /// A 2-D auxiliary coordinate array, CF-scaled, with a masked cell carried
    /// through as `NaN` rather than refused — see `coordinate_values`.
    ///
    /// Public as the sibling of [`Self::curvilinear_index`]: a host checking
    /// where a swath's cells actually landed reads the same array the index was
    /// built from, and reading it a second way would not be a check.
    pub fn coordinate_plane(
        &self,
        view: &DatasetView,
        index: usize,
    ) -> Result<Vec<f64>, FieldglassError> {
        // The whole variable, not a plane extracted with the *data* variable's
        // axis indices: a rank-3 data variable's `(1, 2)` are not a rank-2
        // coordinate's axes, and a coordinate that does carry a leading `Time`
        // has its first step first in C order, which is the step being placed.
        let raw: Vec<f64> = self
            .decode_variable_raw(index)?
            .into_iter()
            .map(|v| v.unwrap_or(f64::NAN))
            .collect();
        Ok(crate::projection::apply_scale_offset(
            &raw,
            attrs_of(view, index),
        ))
    }

    /// One cell of a 2-D coordinate field at `[j, i]`, CF-scaled.
    ///
    /// **A masked corner is a hard error.** Shifting to the next present cell
    /// would move the origin and mis-georeference the whole domain, which is
    /// the failure this guard exists to prevent rather than to survive.
    fn corner_value(
        &self,
        view: &DatasetView,
        var: &VarView,
        name: &str,
        flat_index: usize,
    ) -> Result<f64, FieldglassError> {
        let raw = self
            .decode_variable_raw(var.decode_index)?
            .get(flat_index)
            .copied()
            .flatten()
            .ok_or_else(|| {
                FieldglassError::Parse(format!("{name}[{flat_index}] is missing or masked"))
            })?;
        let (scale, offset) = crate::projection::cf_scale_offset(attrs_of(view, var.decode_index));
        Ok(raw * scale + offset)
    }

    /// The `(lat, lon)` of the far corner — `[nj - 1, ni - 1]` in C order.
    fn far_corner(
        &self,
        view: &DatasetView,
        xlat: &VarView,
        xlong: &VarView,
        ni: u32,
        nj: u32,
    ) -> Result<(f64, f64), FieldglassError> {
        let flat = (nj.saturating_sub(1) as usize) * ni as usize + (ni.saturating_sub(1) as usize);
        Ok((
            self.corner_value(view, xlat, "XLAT", flat)?,
            self.corner_value(view, xlong, "XLONG", flat)?,
        ))
    }
}

/// A variable's attributes by decode index, empty when it is not in the view.
fn attrs_of(view: &DatasetView, index: usize) -> &[Attribute] {
    view.var(index)
        .map_or(&[], |v| v.array.attributes.as_slice())
}
