#![deny(clippy::all)]

use fieldglass_core::{
    Format, GaussianParams, GaussianProjector, LambertParams, LambertProjector, LatLonParams,
    Orthographic, PlanarGridProjector, PolarStereoParams, PolarStereoProjector, PolarStereographic,
    ProjectedPolylines, Resampling, SourceGrid, SourceOverlayTarget, TargetRaster, WebMercator,
    colormap::{min_max_ignoring_mask, paint_grid_rgba},
    detect_from_bytes, latlon_inverse, project_polylines,
    projection::GridIndex,
    warp::{TargetProjection, WarpedRaster, warp},
};
use fieldglass_grib1::{
    Grib1Reader,
    tables::{lookup_centre, lookup_parameter},
};
use fieldglass_grib2::{
    Grib2Reader, HorizontalProductCommon, ProductDefinitionSection,
    lookup_centre as lookup_grib2_centre, lookup_discipline, lookup_fixed_surface,
    lookup_parameter as lookup_grib2_parameter, lookup_production_status, lookup_time_range_unit,
};
use fieldglass_netcdf::{NetcdfBacking, NetcdfReader};
use napi_derive::napi;
use std::sync::Mutex;

/// A single message's metadata, exposed to Node.js.
#[napi(object)]
pub struct MessageMeta {
    pub message_index: i32,
    pub offset_bytes: i32,
    pub parameter_name: String,
    pub parameter_units: String,
    pub parameter_abbreviation: String,
    pub level: String,
    pub level_type: String,
    pub reference_time: String,
    pub forecast_hours: i32,
    pub forecast_display: String,
    pub originating_centre: String,
    pub grid_type: Option<String>,
    pub grid_ni: Option<i32>,
    pub grid_nj: Option<i32>,
    pub lat_first: Option<f64>,
    pub lon_first: Option<f64>,
    pub lat_last: Option<f64>,
    pub lon_last: Option<f64>,
    pub format: String,
    /// GRIB edition number (1 or 2). Optional so older callers reading
    /// historical fields stay source-compatible.
    pub edition: Option<i32>,
    /// GRIB2 discipline name (WMO Code Table 0.0). `None` for non-GRIB2.
    pub discipline: Option<String>,
    /// Total length of the message in bytes, surfaced for GRIB2 where the
    /// 64-bit length is part of the IS metadata.
    pub total_length_bytes: Option<f64>,
    /// Human-readable production status (WMO Code Table 1.3). `None` for
    /// formats that don't carry the field.
    pub production_status: Option<String>,
    /// Human-readable processed-data type (WMO Code Table 1.4). `None` for
    /// formats that don't carry the field.
    pub data_type: Option<String>,
    // -------------------------------------------------------------------
    // Lambert Conformal projection parameters (only populated for Lambert
    // grids; `None` for every other grid type). The renderer uses these to
    // run an inverse-projection warp from output (lat, lon) → source grid
    // sample. Naming matches WMO §3.30 / GRIB1 GDS conventions.
    // -------------------------------------------------------------------
    /// Latitude at which Dx and Dy are specified, in degrees. GRIB2 §3.30
    /// carries this explicitly; for GRIB1 it is mirrored from `latin1` (the
    /// historical convention).
    pub lambert_lad: Option<f64>,
    /// Orientation longitude (the meridian parallel to the y-axis), degrees.
    pub lambert_lov: Option<f64>,
    /// Grid spacing in metres along x at the latitude of true scale.
    pub lambert_dx_metres: Option<f64>,
    /// Grid spacing in metres along y at the latitude of true scale.
    pub lambert_dy_metres: Option<f64>,
    /// First standard parallel, degrees.
    pub lambert_latin1: Option<f64>,
    /// Second standard parallel, degrees.
    pub lambert_latin2: Option<f64>,
    // -------------------------------------------------------------------
    // Gaussian projection parameter (only populated for Gaussian grids).
    // -------------------------------------------------------------------
    /// Number of parallels between a pole and the equator (the "N" in the
    /// Gaussian grid spec). Needed to reconstruct row latitudes via the
    /// Gauss–Legendre quadrature nodes during reprojection.
    pub gaussian_n_parallels: Option<i32>,
    // -------------------------------------------------------------------
    // Polar stereographic projection parameters (only populated for polar
    // stereographic grids; `None` for every other grid type). GRIB1 fixes
    // the latitude of true scale at 60° implicitly — see `PolarStereoParams`
    // — so the only on-the-wire fields are `lov`, the grid spacing in metres
    // along x and y, and the pole-orientation flag.
    // -------------------------------------------------------------------
    /// Orientation longitude (`LoV`) — meridian parallel to the y-axis,
    /// degrees.
    pub polar_stereo_lov: Option<f64>,
    /// Grid spacing in metres along x at the 60° latitude of true scale.
    pub polar_stereo_dx_metres: Option<f64>,
    /// Grid spacing in metres along y at the 60° latitude of true scale.
    pub polar_stereo_dy_metres: Option<f64>,
    /// `true` ⇒ south-pole projection, `false` ⇒ north-pole.
    pub polar_stereo_south_pole: Option<bool>,
}

/// Detect the format of a file from its raw bytes.
/// Returns "grib1" | "grib2" | "netcdf" | "unknown".
#[napi]
pub fn detect_bytes(bytes: napi::bindgen_prelude::Buffer) -> String {
    match detect_from_bytes(&bytes) {
        Format::Grib1 => "grib1".to_string(),
        Format::Grib2 => "grib2".to_string(),
        Format::NetCdf => "netcdf".to_string(),
        Format::Unknown => "unknown".to_string(),
    }
}

/// Build the `MessageMeta` payload for a single GRIB1 message. Used by
/// [`Grib1Handle::messages`].
fn build_grib1_message_meta(msg: &fieldglass_grib1::Grib1Message) -> MessageMeta {
    let param = lookup_parameter(msg.pds.parameter_id, msg.pds.table_version);
    let (grid_type, grid_ni, grid_nj, lat_first, lon_first, lat_last, lon_last) = match &msg.gds {
        Some(gds) => {
            let dims = gds.dimensions();
            let bounds = gds.bounds();
            (
                Some(gds.grid_type_name().to_string()),
                dims.map(|(ni, _)| ni as i32),
                dims.map(|(_, nj)| nj as i32),
                bounds.map(|(la1, _, _, _)| la1),
                bounds.map(|(_, lo1, _, _)| lo1),
                bounds.map(|(_, _, la2, _)| la2),
                bounds.map(|(_, _, _, lo2)| lo2),
            )
        }
        None => (None, None, None, None, None, None, None),
    };
    let lambert = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::LambertConformal(g)) => Some(g),
        _ => None,
    };
    let lambert_lad = lambert.map(|g| g.latin1);
    let lambert_lov = lambert.map(|g| g.lov);
    let lambert_dx_metres = lambert.map(|g| g.dx_m as f64);
    let lambert_dy_metres = lambert.map(|g| g.dy_m as f64);
    let lambert_latin1 = lambert.map(|g| g.latin1);
    let lambert_latin2 = lambert.map(|g| g.latin2);
    let gaussian_n_parallels = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::Gaussian(g)) => Some(g.n_gaussians as i32),
        _ => None,
    };
    let polar_stereo = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::PolarStereographic(g)) => Some(g),
        _ => None,
    };
    let polar_stereo_lov = polar_stereo.map(|g| g.lov);
    let polar_stereo_dx_metres = polar_stereo.map(|g| g.dx_m as f64);
    let polar_stereo_dy_metres = polar_stereo.map(|g| g.dy_m as f64);
    let polar_stereo_south_pole = polar_stereo.map(|g| g.south_pole);
    MessageMeta {
        message_index: msg.message_index as i32,
        offset_bytes: msg.byte_offset as i32,
        parameter_name: param.name.to_string(),
        parameter_units: param.units.to_string(),
        parameter_abbreviation: param.abbreviation.to_string(),
        level: fieldglass_grib1::level_value_str(&msg.pds),
        level_type: fieldglass_grib1::level_type_str(&msg.pds),
        reference_time: fieldglass_grib1::reference_time(&msg.pds),
        forecast_hours: fieldglass_grib1::forecast_hours(&msg.pds),
        forecast_display: fieldglass_grib1::forecast_display(&msg.pds),
        originating_centre: lookup_centre(msg.pds.originating_centre).to_string(),
        grid_type,
        grid_ni,
        grid_nj,
        lat_first,
        lon_first,
        lat_last,
        lon_last,
        format: "grib1".to_string(),
        edition: Some(1),
        discipline: None,
        total_length_bytes: Some(msg.is.total_length as f64),
        production_status: None,
        data_type: None,
        lambert_lad,
        lambert_lov,
        lambert_dx_metres,
        lambert_dy_metres,
        lambert_latin1,
        lambert_latin2,
        gaussian_n_parallels,
        polar_stereo_lov,
        polar_stereo_dx_metres,
        polar_stereo_dy_metres,
        polar_stereo_south_pole,
    }
}

/// Render the §4 product fields into the flat (`parameter_*`, `level`,
/// `forecast_*`) shape the JS layer consumes. Splitting the rendering out
/// of [`open_grib2`] keeps the loop body short and lets future templates
/// reuse the same projection.
fn grib2_product_fields(discipline: u8, pds: &ProductDefinitionSection) -> Grib2ProductFields {
    let Some(common) = pds.common() else {
        return Grib2ProductFields::placeholder();
    };
    let (abbreviation, name, units) = match lookup_grib2_parameter(
        discipline,
        common.parameter_category,
        common.parameter_number,
    ) {
        Some((abbr, long, units)) => (abbr.to_string(), long.to_string(), units.to_string()),
        None => (
            String::new(),
            format!(
                "Parameter {}/{}/{}",
                discipline, common.parameter_category, common.parameter_number
            ),
            String::new(),
        ),
    };

    let level_type = lookup_fixed_surface(common.first_surface.surface_type).to_string();
    let level = render_level(common);

    let (forecast_hours, forecast_display) = render_forecast(common);

    Grib2ProductFields {
        parameter_name: name,
        parameter_units: units,
        parameter_abbreviation: abbreviation,
        level,
        level_type,
        forecast_hours,
        forecast_display,
    }
}

struct Grib2ProductFields {
    parameter_name: String,
    parameter_units: String,
    parameter_abbreviation: String,
    level: String,
    level_type: String,
    forecast_hours: i32,
    forecast_display: String,
}

impl Grib2ProductFields {
    fn placeholder() -> Self {
        Self {
            parameter_name: String::new(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: "—".to_string(),
            level_type: "—".to_string(),
            forecast_hours: 0,
            forecast_display: "—".to_string(),
        }
    }
}

/// Render the first fixed surface as a human-readable level string. Falls
/// back to `"—"` when the surface is the WMO "missing" sentinel; otherwise
/// shows the decoded float with the surface label as a unit hint.
fn render_level(common: &HorizontalProductCommon) -> String {
    let surface = &common.first_surface;
    if surface.is_missing() {
        return "—".to_string();
    }
    match surface.value() {
        Some(v) => format!("{v}"),
        None => lookup_fixed_surface(surface.surface_type).to_string(),
    }
}

/// Render forecast time as `(hours_as_i32, display_string)`. Hours are
/// normalised for the common units (minute / hour / day) so the existing
/// `forecast_hours` column stays meaningful for sorting; the display string
/// preserves the original unit when the value can't be coerced.
fn render_forecast(common: &HorizontalProductCommon) -> (i32, String) {
    let unit_label = lookup_time_range_unit(common.forecast_time_unit);
    let raw = common.forecast_time;
    let hours = match common.forecast_time_unit {
        0 => Some(raw / 60), // minute
        1 => Some(raw),      // hour
        2 => Some(raw * 24), // day
        10 => Some(raw * 3),
        11 => Some(raw * 6),
        12 => Some(raw * 12),
        13 => Some(raw / 3600), // second
        _ => None,
    };
    let display = match (hours, common.forecast_time_unit) {
        (Some(h), 1) => format!("+{h}h"),
        (Some(_), _) => format!("+{raw} {unit_label}"),
        (None, _) => format!("+{raw} {unit_label}"),
    };
    let hours_i32 = hours.and_then(|h| i32::try_from(h).ok()).unwrap_or(0);
    (hours_i32, display)
}

/// Parse a GRIB2 file from raw bytes and return per-message metadata.
/// Surfaces §0 + §1 + §3 + §4 fields (edition, discipline, centre, ref-time,
/// parameter triple, level + level type, forecast time, production status,
/// data type, grid template, dimensions, corner coords); §5+ columns remain
/// placeholders until the data-representation / data parsers land.
///
/// Shared by [`open_grib2`] and the new [`Grib2Handle::messages`] method.
fn build_grib2_message_meta(msg: &fieldglass_grib2::Grib2Message) -> MessageMeta {
    let centre = lookup_grib2_centre(msg.ids.centre)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Centre {}", msg.ids.centre));
    let dims = msg.gds.dimensions();
    let bounds = msg.gds.bounds();
    let product = grib2_product_fields(msg.is.discipline, &msg.pds);

    let lambert = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::Lambert(t) => Some(t),
        _ => None,
    };
    let lambert_lad = lambert.map(|t| t.lad);
    let lambert_lov = lambert.map(|t| t.lov);
    let lambert_dx_metres = lambert.map(|t| t.dx_metres);
    let lambert_dy_metres = lambert.map(|t| t.dy_metres);
    let lambert_latin1 = lambert.map(|t| t.latin1);
    let lambert_latin2 = lambert.map(|t| t.latin2);
    let gaussian_n_parallels = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::Gaussian(t) => Some(t.n_parallels as i32),
        _ => None,
    };

    MessageMeta {
        message_index: msg.message_index as i32,
        offset_bytes: msg.byte_offset as i32,
        parameter_name: product.parameter_name,
        parameter_units: product.parameter_units,
        parameter_abbreviation: product.parameter_abbreviation,
        level: product.level,
        level_type: product.level_type,
        reference_time: msg.ids.reference_time_iso8601(),
        forecast_hours: product.forecast_hours,
        forecast_display: product.forecast_display,
        originating_centre: centre,
        grid_type: Some(msg.gds.template_name()),
        grid_ni: dims.map(|(ni, _)| ni as i32),
        grid_nj: dims.map(|(_, nj)| nj as i32),
        lat_first: bounds.map(|(la1, _, _, _)| la1),
        lon_first: bounds.map(|(_, lo1, _, _)| lo1),
        lat_last: bounds.map(|(_, _, la2, _)| la2),
        lon_last: bounds.map(|(_, _, _, lo2)| lo2),
        format: "grib2".to_string(),
        edition: Some(i32::from(msg.is.edition)),
        discipline: Some(lookup_discipline(msg.is.discipline).to_string()),
        total_length_bytes: Some(msg.is.total_length as f64),
        production_status: Some(lookup_production_status(msg.ids.production_status).to_string()),
        data_type: Some(fieldglass_grib2::lookup_data_type(msg.ids.data_type).to_string()),
        lambert_lad,
        lambert_lov,
        lambert_dx_metres,
        lambert_dy_metres,
        lambert_latin1,
        lambert_latin2,
        gaussian_n_parallels,
        // GRIB2 §3.20 polar stereo template parsing is tracked under #70 —
        // once it lands these get populated from the parsed template.
        polar_stereo_lov: None,
        polar_stereo_dx_metres: None,
        polar_stereo_dy_metres: None,
        polar_stereo_south_pole: None,
    }
}

// ---------------------------------------------------------------------------
// NetCDF
// ---------------------------------------------------------------------------

/// One NetCDF dimension, flattened for the JS boundary.
#[napi(object)]
pub struct DimensionMeta {
    pub name: String,
    /// Length of the dimension. `0` is a valid display value for the
    /// unlimited / record dimension (with `is_record == true`).
    pub length: f64,
    pub is_record: bool,
}

/// One NetCDF attribute (global or per-variable). `value` is already a
/// human-readable string — UTF-8 for Char attributes, comma-separated decimal
/// for numeric ones — so the provider can render it as-is.
#[napi(object)]
pub struct AttributeMeta {
    pub name: String,
    pub nc_type: String,
    pub value: String,
}

/// One NetCDF variable, flattened for the JS boundary. `dimensions` lists
/// resolved dimension names (in declared order) so the provider doesn't need
/// to cross-reference dim ids itself.
#[napi(object)]
pub struct VariableMeta {
    pub name: String,
    pub nc_type: String,
    pub dimensions: Vec<String>,
    pub attributes: Vec<AttributeMeta>,
}

/// Top-level NetCDF dataset metadata. Covers what's exposable from the
/// header alone; per-variable values are a separate decode step (out of
/// scope for issue #29).
#[napi(object)]
pub struct DatasetMeta {
    /// `"classic"` (CDF-1/2/5) or `"hdf5"` (NetCDF-4).
    pub backing: String,
    /// Human-readable label, e.g. `"NetCDF classic (CDF-1)"` or
    /// `"NetCDF-4 / HDF5"`.
    pub backing_label: String,
    /// `true` if dimensions / variables / attributes are populated;
    /// `false` for HDF5 today (deep parsing is a follow-up).
    pub fully_parsed: bool,
    /// Free-form note for the provider to surface when `fully_parsed` is
    /// false — e.g. "NetCDF-4 / HDF5 deep parsing not yet implemented".
    pub note: Option<String>,
    pub dimensions: Vec<DimensionMeta>,
    pub global_attributes: Vec<AttributeMeta>,
    pub variables: Vec<VariableMeta>,
    /// HDF5 superblock version, when applicable. `None` for classic files.
    pub hdf5_superblock_version: Option<i32>,
}

/// Parse a NetCDF file from raw bytes and return the top-level dataset
/// metadata. Errors only for files that are neither classic CDF nor HDF5;
/// HDF5 files succeed with `fully_parsed = false`.
#[napi]
pub fn open_netcdf(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<DatasetMeta> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(dataset_meta_from(reader))
}

fn dataset_meta_from(reader: NetcdfReader) -> DatasetMeta {
    let label = reader.backing.label().to_string();
    match reader.backing {
        NetcdfBacking::Classic(h) => {
            let dim_names: Vec<String> = h.dimensions.iter().map(|d| d.name.clone()).collect();
            let dimensions = h
                .dimensions
                .iter()
                .map(|d| DimensionMeta {
                    name: d.name.clone(),
                    length: d.length as f64,
                    is_record: d.is_record,
                })
                .collect();
            let global_attributes = h
                .global_attributes
                .iter()
                .map(|a| AttributeMeta {
                    name: a.name.clone(),
                    nc_type: a.nc_type.name().to_string(),
                    value: a.value.clone(),
                })
                .collect();
            let variables = h
                .variables
                .iter()
                .map(|v| VariableMeta {
                    name: v.name.clone(),
                    nc_type: v.nc_type.name().to_string(),
                    dimensions: v
                        .dim_ids
                        .iter()
                        .map(|&id| {
                            dim_names
                                .get(id as usize)
                                .cloned()
                                .unwrap_or_else(|| format!("dim#{id}"))
                        })
                        .collect(),
                    attributes: v
                        .attributes
                        .iter()
                        .map(|a| AttributeMeta {
                            name: a.name.clone(),
                            nc_type: a.nc_type.name().to_string(),
                            value: a.value.clone(),
                        })
                        .collect(),
                })
                .collect();
            DatasetMeta {
                backing: "classic".to_string(),
                backing_label: label,
                fully_parsed: true,
                note: None,
                dimensions,
                global_attributes,
                variables,
                hdf5_superblock_version: None,
            }
        }
        NetcdfBacking::Hdf5(probe) => DatasetMeta {
            backing: "hdf5".to_string(),
            backing_label: label,
            fully_parsed: false,
            note: Some(
                "NetCDF-4 / HDF5 deep parsing is not yet implemented; \
                 only the superblock has been validated. Classic NetCDF \
                 (CDF-1 / CDF-2 / CDF-5) renders fully."
                    .to_string(),
            ),
            dimensions: Vec::new(),
            global_attributes: Vec::new(),
            variables: Vec::new(),
            hdf5_superblock_version: Some(probe.superblock_version as i32),
        },
    }
}

// ---------------------------------------------------------------------------
// Reader handles + render entry  (closes #41 + the Rust-side render rewrite
// from #45)
// ---------------------------------------------------------------------------

/// Picker-driven render options posted from the webview. `projection` is
/// either `"source"` (paint the source grid unchanged) or
/// `"equirectangular"` (inverse-warp into a north-up lat/lon canvas).
#[napi(object)]
pub struct RenderOptions {
    pub projection: String,
    /// Preset selector for the parameterised targets. `"orthographic"` reads
    /// a centre preset (`"atlantic"` (0°N 0°E, default), `"indian"` (0°N 90°E),
    /// `"pacific"` (0°N 180°E), `"americas"` (0°N 270°E), `"north_pole"`,
    /// `"south_pole"`); `"polar_stereographic"` reads a
    /// hemisphere preset (`"north"` (default), `"south"`). Ignored by the
    /// lat/lon-box targets. `None`/unknown falls back to the default.
    pub projection_preset: Option<String>,
    pub resampling: String,
    pub flip_y: bool,
    /// Manual range override. When either is `None` the renderer uses the
    /// computed min/max over the present cells.
    pub range_min: Option<f64>,
    pub range_max: Option<f64>,
    /// Manual equirectangular extent override (degrees). Only consulted for
    /// the `"equirectangular"` target. When all four are `Some` and form a
    /// valid box (`lat_max > lat_min`, `lon_max > lon_min`) the warp renders
    /// that window; otherwise it falls back to the computed source-grid bounds
    /// (echoed back in [`RenderedGrid`] so a UI can pre-fill these inputs).
    /// `lon_min`/`lon_max` may lie outside [-180, 180] to describe a window
    /// that crosses the antimeridian.
    pub bounds_lat_min: Option<f64>,
    pub bounds_lat_max: Option<f64>,
    pub bounds_lon_min: Option<f64>,
    pub bounds_lon_max: Option<f64>,
}

/// Output of [`Grib1Handle::render_grid`] / [`Grib2Handle::render_grid`].
/// `rgba` is a paint-ready buffer the webview blits straight to canvas
/// via `putImageData`.
#[napi(object)]
pub struct RenderedGrid {
    /// RGBA bytes, `width * height * 4` long.
    pub rgba: napi::bindgen_prelude::Buffer,
    pub width: i32,
    pub height: i32,
    /// The min/max range actually used to paint, echoed back so the
    /// webview can pre-fill the manual-range inputs when the user
    /// switches to manual mode.
    pub used_min: f64,
    pub used_max: f64,
    /// Equirectangular extent actually rendered (degrees), echoed back so the
    /// webview can pre-fill the manual-bounds inputs. `None` for the
    /// source-projection target, which has no geographic extent. `lonMin`/
    /// `lonMax` may fall outside [-180, 180] for an antimeridian-spanning
    /// window — pass them back verbatim to reproduce the same view.
    pub used_lat_min: Option<f64>,
    pub used_lat_max: Option<f64>,
    pub used_lon_min: Option<f64>,
    pub used_lon_max: Option<f64>,
    /// Human-readable summary of the source→target projection chain,
    /// e.g. `"lambert → equirectangular (nearest)"`.
    pub projection_summary: String,
}

/// Projected overlay geometry returned by `project_overlay` — coastline /
/// graticule / user-shape polylines mapped into the warped raster's pixel
/// space, ready for the webview to stroke on its overlay canvas.
///
/// `xy` is flat `[x0, y0, x1, y1, …]` in output pixel coordinates (post-flipY,
/// identical to the rendered raster); `seg_lengths` gives the vertex count of
/// each visible run after clipping/splitting, so
/// `seg_lengths.iter().sum::<u32>() * 2 == xy.len()`. May be empty when no run
/// survives clipping (every vertex projects off the visible domain).
#[napi(object)]
pub struct ProjectedOverlay {
    pub xy: napi::bindgen_prelude::Float64Array,
    pub seg_lengths: napi::bindgen_prelude::Uint32Array,
}

impl ProjectedOverlay {
    fn from_polylines(p: ProjectedPolylines) -> Self {
        Self {
            xy: p.xy.into(),
            seg_lengths: p.seg_lengths.into(),
        }
    }
}

/// Decoded grid values + presence mask returned by the handle-based
/// `decode_grid`. `values[k]` is the decoded value at scan-order index
/// `k`; `mask[k]` is `1` when present and `0` when bitmap-masked.
///
/// Not consumed by the render panel today — `render_grid` returns
/// paint-ready RGBA instead. Kept on the napi surface for future
/// "export raw values" consumers (e.g. a notebook-style data probe);
/// the JS code path is dormant.
#[napi(object)]
pub struct DecodedGrid {
    pub values: napi::bindgen_prelude::Float64Array,
    pub mask: napi::bindgen_prelude::Buffer,
    pub width: i32,
    pub height: i32,
}

/// Persistent GRIB1 reader handle held across napi calls. Replaces the
/// `open_grib1` → buffer-clone → re-parse round-trip per call.
///
/// `bytes` is kept around because [`Self::set_p1`] hands a freshly-edited
/// buffer back to the JS caller; [`Grib2Handle`] doesn't need this because
/// GRIB2 has no edit path yet (general PDS-field editing is the metadata-
/// editing track, separate from this PR). The asymmetry is intentional —
/// not an oversight.
///
/// `decoded` caches per-message decode output (raw `Vec<Option<f64>>`)
/// so repeat `render_grid` calls with different `RenderOptions` skip the
/// bit-unpack + bitmap-merge step every time the user wiggles a picker.
///
/// The cache is wrapped in a `Mutex` not because we expect contention
/// — napi-rs runs class methods on the Node main thread — but because
/// `#[napi]` requires the class to be `Send`. `RefCell` would be the
/// natural single-threaded fit but its `!Send` rules it out. Lock /
/// unlock overhead at zero contention is negligible (single atomic
/// CAS per render).
#[napi]
pub struct Grib1Handle {
    bytes: Vec<u8>,
    reader: Grib1Reader,
    decoded: Mutex<std::collections::HashMap<u32, std::sync::Arc<Vec<Option<f64>>>>>,
}

#[napi]
impl Grib1Handle {
    /// Parse the supplied buffer once; the handle keeps the parsed
    /// reader alive for the lifetime of the JS object.
    #[napi(factory)]
    pub fn from_bytes(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Self> {
        let owned = bytes.to_vec();
        let reader = Grib1Reader::from_bytes(owned.clone())
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Self {
            bytes: owned,
            reader,
            decoded: Mutex::new(std::collections::HashMap::new()),
        })
    }

    #[napi]
    pub fn messages(&self) -> Vec<MessageMeta> {
        self.reader
            .messages
            .iter()
            .map(build_grib1_message_meta)
            .collect()
    }

    /// Decode one message into a `(values, mask)` typed-array pair. NaN
    /// is reserved for masked cells in `values`; callers should consult
    /// `mask[k] == 0` rather than checking for NaN.
    #[napi]
    pub fn decode_grid(&self, message_index: u32) -> napi::Result<DecodedGrid> {
        let raw = self.cached_decode(message_index)?;
        let (width, height) = grib1_dimensions(&self.reader, message_index as usize)?;
        Ok(decoded_grid_from(&raw, width, height))
    }

    /// Patch the PDS `p1` (forecast period) octet of one message and
    /// return a fresh byte buffer. Callers reconstruct a new
    /// [`Grib1Handle`] from those bytes — handle state is immutable
    /// across edits.
    #[napi]
    pub fn set_p1(
        &self,
        message_index: u32,
        value: u32,
    ) -> napi::Result<napi::bindgen_prelude::Buffer> {
        if value > u8::MAX as u32 {
            return Err(napi::Error::from_reason(format!(
                "p1 must fit in a u8 (0..=255), got {value}"
            )));
        }
        let msg = self
            .reader
            .messages
            .get(message_index as usize)
            .ok_or_else(|| {
                napi::Error::from_reason(format!(
                    "message index {message_index} out of range (have {})",
                    self.reader.messages.len()
                ))
            })?;
        let mut out = self.bytes.clone();
        let off = msg.pds_p1_offset();
        out[off] = value as u8;
        Ok(out.into())
    }

    /// Compose decode + reprojection warp + viridis colormap into a
    /// paint-ready RGBA buffer.
    #[napi]
    pub fn render_grid(
        &self,
        message_index: u32,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let meta = self
            .reader
            .messages
            .get(message_index as usize)
            .map(build_grib1_message_meta)
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })?;
        let raw = self.cached_decode(message_index)?;
        render_with_options(&meta, raw.as_ref(), &options)
    }

    /// Project geographic polylines (coastline / graticule / user shapes)
    /// onto the same raster `render_grid` produces for these `options`,
    /// returning pixel-space runs for the overlay layer (#72). Geometry-only:
    /// it never decodes values, so toggling the overlay never re-decodes.
    #[napi]
    pub fn project_overlay(
        &self,
        message_index: u32,
        options: RenderOptions,
        latlon: napi::bindgen_prelude::Float64Array,
        ring_lengths: napi::bindgen_prelude::Uint32Array,
    ) -> napi::Result<ProjectedOverlay> {
        let meta = self
            .reader
            .messages
            .get(message_index as usize)
            .map(build_grib1_message_meta)
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })?;
        project_overlay_impl(&meta, &options, latlon.as_ref(), ring_lengths.as_ref())
            .map(ProjectedOverlay::from_polylines)
    }
}

impl Grib1Handle {
    /// Get-or-build the decoded values for `message_index`. The cache is
    /// invalidated implicitly when the handle is dropped (which happens
    /// in `provider.ts` as soon as the document bytes change via setP1
    /// or the document closes), so we never have to worry about it
    /// going stale.
    fn cached_decode(&self, message_index: u32) -> napi::Result<std::sync::Arc<Vec<Option<f64>>>> {
        if let Some(hit) = self
            .decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .get(&message_index)
        {
            return Ok(std::sync::Arc::clone(hit));
        }
        let raw = self
            .reader
            .decode_message_values(message_index as usize)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let arc = std::sync::Arc::new(raw);
        self.decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .insert(message_index, std::sync::Arc::clone(&arc));
        Ok(arc)
    }
}

/// Persistent GRIB2 reader handle, sibling to [`Grib1Handle`].
///
/// Unlike `Grib1Handle`, this struct doesn't store the original bytes —
/// GRIB2 has no in-place edit path yet (the metadata-editing track is a
/// separate workstream). The `decoded` cache mirrors `Grib1Handle`'s
/// rationale: subsequent `render_grid` calls with different
/// `RenderOptions` re-paint without re-running the bit-unpack +
/// bitmap-merge step.
#[napi]
pub struct Grib2Handle {
    reader: Grib2Reader,
    decoded: Mutex<std::collections::HashMap<u32, std::sync::Arc<Vec<Option<f64>>>>>,
}

#[napi]
impl Grib2Handle {
    #[napi(factory)]
    pub fn from_bytes(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Self> {
        let reader = Grib2Reader::from_bytes(bytes.to_vec())
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Self {
            reader,
            decoded: Mutex::new(std::collections::HashMap::new()),
        })
    }

    #[napi]
    pub fn messages(&self) -> Vec<MessageMeta> {
        self.reader
            .messages
            .iter()
            .map(build_grib2_message_meta)
            .collect()
    }

    #[napi]
    pub fn decode_grid(&self, message_index: u32) -> napi::Result<DecodedGrid> {
        let raw = self.cached_decode(message_index)?;
        let msg = self
            .reader
            .messages
            .get(message_index as usize)
            .ok_or_else(|| napi::Error::from_reason("message index out of range".to_string()))?;
        let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
            napi::Error::from_reason("grid has no declared dimensions".to_string())
        })?;
        Ok(decoded_grid_from(&raw, ni, nj))
    }

    #[napi]
    pub fn render_grid(
        &self,
        message_index: u32,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let meta = self
            .reader
            .messages
            .get(message_index as usize)
            .map(build_grib2_message_meta)
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })?;
        let raw = self.cached_decode(message_index)?;
        render_with_options(&meta, raw.as_ref(), &options)
    }

    /// Project geographic polylines onto the same raster `render_grid`
    /// produces for these `options` (#72). Sibling to [`Grib1Handle::project_overlay`].
    #[napi]
    pub fn project_overlay(
        &self,
        message_index: u32,
        options: RenderOptions,
        latlon: napi::bindgen_prelude::Float64Array,
        ring_lengths: napi::bindgen_prelude::Uint32Array,
    ) -> napi::Result<ProjectedOverlay> {
        let meta = self
            .reader
            .messages
            .get(message_index as usize)
            .map(build_grib2_message_meta)
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })?;
        project_overlay_impl(&meta, &options, latlon.as_ref(), ring_lengths.as_ref())
            .map(ProjectedOverlay::from_polylines)
    }
}

impl Grib2Handle {
    fn cached_decode(&self, message_index: u32) -> napi::Result<std::sync::Arc<Vec<Option<f64>>>> {
        if let Some(hit) = self
            .decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .get(&message_index)
        {
            return Ok(std::sync::Arc::clone(hit));
        }
        let raw = self
            .reader
            .decode_message_values(message_index as usize)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let arc = std::sync::Arc::new(raw);
        self.decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .insert(message_index, std::sync::Arc::clone(&arc));
        Ok(arc)
    }
}

fn grib1_dimensions(reader: &Grib1Reader, message_index: usize) -> napi::Result<(u32, u32)> {
    let msg = reader
        .messages
        .get(message_index)
        .ok_or_else(|| napi::Error::from_reason("message index out of range".to_string()))?;
    let gds = msg.gds.as_ref().ok_or_else(|| {
        napi::Error::from_reason("message has no GDS — predefined grids unsupported".to_string())
    })?;
    gds.dimensions()
        .ok_or_else(|| napi::Error::from_reason("grid type has no declared dimensions".to_string()))
}

/// Repack the decoder's `Vec<Option<f64>>` into the typed-array pair
/// the JS side expects. NaN encodes missing in `values`; the explicit
/// `mask` byte is the source of truth for "present vs absent".
fn decoded_grid_from(raw: &[Option<f64>], width: u32, height: u32) -> DecodedGrid {
    let n = raw.len();
    let mut values = vec![0.0f64; n];
    let mut mask = vec![0u8; n];
    for (i, v) in raw.iter().enumerate() {
        match v {
            Some(x) => {
                values[i] = *x;
                mask[i] = 1;
            }
            None => {
                values[i] = f64::NAN;
                mask[i] = 0;
            }
        }
    }
    DecodedGrid {
        values: napi::bindgen_prelude::Float64Array::new(values),
        mask: mask.into(),
        width: width as i32,
        height: height as i32,
    }
}

/// Drive the warp + colormap pipeline for a single message. Returns the
/// paint-ready RGBA buffer plus the (min, max) actually used + a
/// human-readable summary of the source→target projection chain.
/// Validated picker state. Lifts `RenderOptions`'s loose strings into a
/// closed enum so the rest of the pipeline can pattern-match without
/// silently falling to defaults on a typo.
#[cfg_attr(test, derive(Debug))]
struct ResolvedOptions {
    projection: TargetKind,
    resampling: Resampling,
    flip_y: bool,
    range_min: Option<f64>,
    range_max: Option<f64>,
    bounds: Option<RenderBounds>,
}

/// What the picker's `projection` string resolved to. Named `TargetKind`
/// rather than `TargetProjection` to avoid colliding with the core
/// `fieldglass_core::TargetProjection` *trait* — this is the napi-side
/// dispatch enum, not the per-target math.
#[cfg_attr(test, derive(Debug))]
enum TargetKind {
    /// Paint the source grid unchanged (no warp).
    Source,
    /// Inverse-warp into one of the geographic targets.
    Warp(WarpTarget),
}

/// A validated equirectangular render window. Only constructed when the
/// caller supplied all four edges and they form a non-degenerate box.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct RenderBounds {
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
}

impl RenderBounds {
    /// Build from the four optional `RenderOptions` fields. Returns `None`
    /// unless every edge is present and the box is non-degenerate — a
    /// partially-filled or inverted box silently falls back to the computed
    /// bounds, mirroring the manual-range behaviour.
    fn from_options(o: &RenderOptions) -> Option<Self> {
        let (lat_min, lat_max, lon_min, lon_max) = (
            o.bounds_lat_min?,
            o.bounds_lat_max?,
            o.bounds_lon_min?,
            o.bounds_lon_max?,
        );
        if lat_max > lat_min && lon_max > lon_min {
            Some(Self {
                lat_min,
                lat_max,
                lon_min,
                lon_max,
            })
        } else {
            None
        }
    }
}

impl ResolvedOptions {
    fn parse(options: &RenderOptions) -> napi::Result<Self> {
        let preset = options.projection_preset.as_deref();
        let projection = match options.projection.as_str() {
            "source" => TargetKind::Source,
            "equirectangular" => TargetKind::Warp(WarpTarget::Equirectangular),
            "web_mercator" => TargetKind::Warp(WarpTarget::WebMercator),
            "orthographic" => TargetKind::Warp(orthographic_from_preset(preset)),
            "polar_stereographic" => TargetKind::Warp(polar_stereographic_from_preset(preset)),
            other => {
                return Err(napi::Error::from_reason(format!(
                    "unknown projection {other:?} (expected \"source\", \"equirectangular\", \
                     \"web_mercator\", \"orthographic\", or \"polar_stereographic\")"
                )));
            }
        };
        let resampling = match options.resampling.as_str() {
            "nearest" => Resampling::Nearest,
            "bilinear" => Resampling::Bilinear,
            other => {
                return Err(napi::Error::from_reason(format!(
                    "unknown resampling {other:?} (expected \"nearest\" or \"bilinear\")"
                )));
            }
        };
        Ok(Self {
            projection,
            resampling,
            flip_y: options.flip_y,
            range_min: options.range_min,
            range_max: options.range_max,
            bounds: RenderBounds::from_options(options),
        })
    }
}

/// Resolve an orthographic centre preset into `(lat0, lon0)`. The issue's
/// non-goals call for a small preset list rather than free-form lat0/lon0
/// inputs; unknown/`None` defaults to the Atlantic view (0°N 0°E).
fn orthographic_from_preset(preset: Option<&str>) -> WarpTarget {
    let (lat0, lon0) = match preset {
        Some("indian") => (0.0, 90.0),
        Some("pacific") => (0.0, 180.0),
        Some("americas") => (0.0, 270.0),
        Some("north_pole") => (90.0, 0.0),
        Some("south_pole") => (-90.0, 0.0),
        // "atlantic" / None / unknown
        _ => (0.0, 0.0),
    };
    WarpTarget::Orthographic { lat0, lon0 }
}

/// Resolve a polar-stereographic hemisphere preset. Defaults to the
/// north-pole aspect; `lon0` (orientation toward the bottom edge) stays at
/// 0° — the free-form orientation knob is a non-goal for #71.
fn polar_stereographic_from_preset(preset: Option<&str>) -> WarpTarget {
    let south_pole = matches!(preset, Some("south"));
    WarpTarget::PolarStereographic {
        south_pole,
        lon0: 0.0,
    }
}

/// Output of a projection stage (`paint_source` / `warp_message`):
/// `(values, mask, width, height, geographic_bounds, summary)`. The bounds
/// are `(lat_min, lat_max, lon_min, lon_max)` for the equirectangular target,
/// or `None` for source projection (no geographic extent).
type ProjectionStage = (
    Vec<f64>,
    Vec<u8>,
    u32,
    u32,
    Option<(f64, f64, f64, f64)>,
    String,
);

fn render_with_options(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    options: &RenderOptions,
) -> napi::Result<RenderedGrid> {
    let resolved = ResolvedOptions::parse(options)?;
    let (values, mask, width, height, used_bounds, summary) = match resolved.projection {
        TargetKind::Source => paint_source(meta, raw)?,
        TargetKind::Warp(target) => {
            warp_message(meta, raw, target, resolved.resampling, resolved.bounds)?
        }
    };

    let (used_min, used_max) = match (resolved.range_min, resolved.range_max) {
        (Some(min), Some(max)) if max > min => (min, max),
        _ => min_max_ignoring_mask(values.iter().enumerate().map(|(i, &v)| {
            if mask.get(i).copied().unwrap_or(0) == 0 {
                None
            } else {
                Some(v)
            }
        }))
        .unwrap_or((0.0, 1.0)),
    };

    let rgba = paint_grid_rgba(
        &values,
        Some(&mask),
        width,
        height,
        used_min,
        used_max,
        resolved.flip_y,
    );

    let (used_lat_min, used_lat_max, used_lon_min, used_lon_max) = match used_bounds {
        Some((la_min, la_max, lo_min, lo_max)) => {
            (Some(la_min), Some(la_max), Some(lo_min), Some(lo_max))
        }
        None => (None, None, None, None),
    };

    Ok(RenderedGrid {
        rgba: rgba.into(),
        width: width as i32,
        height: height as i32,
        used_min,
        used_max,
        used_lat_min,
        used_lat_max,
        used_lon_min,
        used_lon_max,
        projection_summary: summary,
    })
}

fn source_projection_summary(meta: &MessageMeta) -> String {
    let kind = meta.grid_type.as_deref().unwrap_or("unknown");
    let dims = match (meta.grid_ni, meta.grid_nj) {
        (Some(ni), Some(nj)) => format!("{ni}×{nj}"),
        _ => "?".to_string(),
    };
    format!("source: {kind} {dims}")
}

/// Source-projection paint: paint the decoded values into a buffer the
/// same shape as the source grid. NaN encodes masked cells.
#[allow(clippy::type_complexity)]
fn paint_source(meta: &MessageMeta, raw: &[Option<f64>]) -> napi::Result<ProjectionStage> {
    let ni = meta
        .grid_ni
        .ok_or_else(|| napi::Error::from_reason("grid has no Ni".to_string()))? as u32;
    let nj = meta
        .grid_nj
        .ok_or_else(|| napi::Error::from_reason("grid has no Nj".to_string()))? as u32;
    let n = (ni as usize) * (nj as usize);
    let mut values = vec![0.0f64; n];
    let mut mask = vec![0u8; n];
    for (i, v) in raw.iter().enumerate().take(n) {
        if let Some(x) = v {
            values[i] = *x;
            mask[i] = 1;
        } else {
            values[i] = f64::NAN;
        }
    }
    // Right-hand side names the actual source projection (e.g. "latlon",
    // "lambert", "polar_stereo") so the picker caption reads
    // `source: latlon 240×121 → latlon (no reprojection)`, mirroring the
    // equirectangular shape but making it explicit *what* the source
    // projection is rather than just labelling it "source projection".
    let kind = meta.grid_type.as_deref().unwrap_or("unknown");
    let summary = format!(
        "{} → {kind} (no reprojection)",
        source_projection_summary(meta),
    );
    // No geographic extent for the source-projection target.
    Ok((values, mask, ni, nj, None, summary))
}

/// Dispatch warp setup to the right per-template helper. Each helper
/// builds the inverse closure (using a projector with precomputed state)
/// and the equirectangular target bounds; the shared finisher then runs
/// the warp itself.
///
/// `bounds_override`, when present, replaces the computed source-grid extent
/// so the caller can render an arbitrary equirectangular window. The bounds
/// actually used are returned (last tuple element) for echo-back.
///
/// LIMITATION (warp output resolution): every setup sizes the output raster to
/// the source dims (`width = ni`, `height = nj`) and the warp samples it
/// uniformly in degrees across the lat/lon bbox. A *projected* source grid's
/// degrees-per-cell varies across the tile — meridians converge poleward — so
/// no single uniform output resolution matches it everywhere: the equator side
/// of a polar grid is downsampled (nearest aliases; bilinear blurs) while the
/// pole side is upsampled (one source cell magnified across several pixels).
/// Aspect isn't preserved either, so degrees-per-pixel differs between axes.
/// This is a deliberate fidelity/perf tradeoff — it keeps the RGBA buffer at a
/// predictable size and spends roughly one output pixel per source cell. The
/// manual-bounds window doubles as a zoom (the fixed pixel budget over a
/// smaller extent recovers detail down to the source resolution), which covers
/// the common "I need detail in this region" case without another knob.
/// Revisit — derive output dims from the extent and a target degrees-per-pixel
/// (capped to bound memory) — only if a real fixture shows aliasing or stretch
/// that's objectionable at pixel scale.
/// Which lat/lon → pixel target projection the warp paints into. Both
/// targets share the same source inverse map and computed bbox; they
/// differ only in how output rows are distributed (linear in latitude vs
/// linear in Mercator Y).
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
enum WarpTarget {
    /// Lat/lon-box targets: rows linear in latitude (equirectangular) or in
    /// Mercator Y (web mercator). These honour a manual lat/lon window and
    /// echo their geographic extent back.
    Equirectangular,
    WebMercator,
    /// Azimuthal targets parameterised by a centre / hemisphere preset. They
    /// fit a disc to the raster and have no lat/lon-box extent to echo.
    Orthographic {
        lat0: f64,
        lon0: f64,
    },
    PolarStereographic {
        south_pole: bool,
        lon0: f64,
    },
}

impl WarpTarget {
    fn label(self) -> &'static str {
        match self {
            WarpTarget::Equirectangular => "equirectangular",
            WarpTarget::WebMercator => "web mercator",
            WarpTarget::Orthographic { .. } => "orthographic",
            WarpTarget::PolarStereographic { .. } => "polar stereographic",
        }
    }
}

#[allow(clippy::type_complexity)]
fn warp_message(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    target_kind: WarpTarget,
    resampling: Resampling,
    bounds_override: Option<RenderBounds>,
) -> napi::Result<ProjectionStage> {
    let ni = grid_ni(meta)?;
    let nj = grid_nj(meta)?;

    let sample = |i: usize, j: usize| -> Option<f64> {
        let k = j * ni as usize + i;
        raw.get(k).copied().flatten()
    };
    let sample_ref: &dyn Fn(usize, usize) -> Option<f64> = &sample;

    let (inverse_boxed, bbox_thunk) = warp_setup_for(meta, ni, nj)?;

    let inverse_ref: &dyn Fn(f64, f64) -> Option<GridIndex> = inverse_boxed.as_ref();
    let source = SourceGrid {
        ni,
        nj,
        sample: sample_ref,
        inverse_at: inverse_ref,
    };
    // Construct the concrete target (shared with the overlay-projection path so
    // both paint into byte-identical geometry), then warp the source into it.
    let (built, used_bounds) = build_warp_target(target_kind, ni, nj, bbox_thunk, bounds_override);
    let warped = built.warp(&source, resampling);
    let resample_label = match resampling {
        Resampling::Nearest => "nearest",
        Resampling::Bilinear => "bilinear",
    };
    let summary = format!(
        "{} → {} ({resample_label})",
        source_projection_summary(meta),
        target_kind.label(),
    );
    Ok((
        warped.values,
        warped.mask,
        warped.width,
        warped.height,
        used_bounds,
        summary,
    ))
}

/// A concrete, constructed render target — the same value the warp paints
/// into and the overlay projects polylines onto. Centralising construction
/// here guarantees `render_grid` and `project_overlay` agree on the exact
/// raster (dims, clamped Mercator band, azimuthal disc side) pixel-for-pixel.
enum BuiltTarget {
    Equirect(TargetRaster),
    Mercator(WebMercator),
    Ortho(Orthographic),
    Polar(PolarStereographic),
}

impl BuiltTarget {
    fn dims(&self) -> (u32, u32) {
        match self {
            BuiltTarget::Equirect(t) => t.dims(),
            BuiltTarget::Mercator(t) => t.dims(),
            BuiltTarget::Ortho(t) => t.dims(),
            BuiltTarget::Polar(t) => t.dims(),
        }
    }

    fn warp(&self, source: &SourceGrid<'_>, resampling: Resampling) -> WarpedRaster {
        match self {
            BuiltTarget::Equirect(t) => warp(source, t, resampling),
            BuiltTarget::Mercator(t) => warp(source, t, resampling),
            BuiltTarget::Ortho(t) => warp(source, t, resampling),
            BuiltTarget::Polar(t) => warp(source, t, resampling),
        }
    }

    /// Project geographic `(lat, lon)` rings onto this target's pixel space,
    /// applying `flip_y` to match a vertically-flipped render. The lat/lon-box
    /// targets split runs at the antimeridian seam; the azimuthal targets have
    /// no seam (`wraps_antimeridian = false`) and break only off the disc.
    fn project(&self, flip_y: bool, latlon: &[f64], ring_lengths: &[u32]) -> ProjectedPolylines {
        let (w, h) = self.dims();
        match self {
            BuiltTarget::Equirect(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, true, latlon, ring_lengths)
            }
            BuiltTarget::Mercator(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, true, latlon, ring_lengths)
            }
            BuiltTarget::Ortho(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, false, latlon, ring_lengths)
            }
            BuiltTarget::Polar(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, false, latlon, ring_lengths)
            }
        }
    }
}

/// Build the concrete [`BuiltTarget`] for a warp, returning the geographic
/// extent actually used for the lat/lon-box targets (echoed back to the UI)
/// or `None` for the azimuthal targets.
///
/// The lat/lon-box targets resolve a geographic extent (the lazy `bbox_thunk`,
/// possibly replaced by a manual window); the azimuthal targets fit a disc to
/// the raster, so they never invoke the thunk — skipping the perimeter-walk
/// bbox for planar sources — and report no box extent. Output dims size to the
/// source grid for the box targets; the azimuthal discs use a square raster
/// (`side = max(ni, nj)`) so the globe stays circular rather than elliptical.
fn build_warp_target(
    target_kind: WarpTarget,
    ni: u32,
    nj: u32,
    bbox_thunk: BboxThunk,
    bounds_override: Option<RenderBounds>,
) -> (BuiltTarget, Option<(f64, f64, f64, f64)>) {
    match target_kind {
        WarpTarget::Equirectangular => {
            let (lat_min, lat_max, lon_min, lon_max) =
                resolve_box_extent(bbox_thunk, bounds_override);
            let target = TargetRaster {
                width: ni,
                height: nj,
                lat_max,
                lat_min,
                lon_min,
                lon_max,
            };
            (
                BuiltTarget::Equirect(target),
                Some((lat_min, lat_max, lon_min, lon_max)),
            )
        }
        WarpTarget::WebMercator => {
            let (lat_min, lat_max, lon_min, lon_max) =
                resolve_box_extent(bbox_thunk, bounds_override);
            let merc = WebMercator::new(ni, nj, lat_min, lat_max, lon_min, lon_max);
            let used = merc.extent();
            (BuiltTarget::Mercator(merc), Some(used))
        }
        WarpTarget::Orthographic { lat0, lon0 } => {
            let side = ni.max(nj);
            (
                BuiltTarget::Ortho(Orthographic::new(side, side, lat0, lon0)),
                None,
            )
        }
        WarpTarget::PolarStereographic { south_pole, lon0 } => {
            let side = ni.max(nj);
            (
                BuiltTarget::Polar(PolarStereographic::new(side, side, south_pole, lon0)),
                None,
            )
        }
    }
}

/// Dispatch to the per-grid-type warp setup, returning the source inverse map
/// and the lazy lat/lon-box extent thunk. Shared by the warp (`warp_message`)
/// and the overlay projection (`project_overlay`) so both derive identical
/// target geometry from the same source parameters.
fn warp_setup_for(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    match meta.grid_type.as_deref().unwrap_or("") {
        "latlon" => latlon_warp_setup(meta, ni, nj),
        "gaussian" => gaussian_warp_setup(meta, ni, nj),
        "lambert" => lambert_warp_setup(meta, ni, nj),
        "polar_stereo" => polar_stereo_warp_setup(meta, ni, nj),
        other => Err(napi::Error::from_reason(format!(
            "reprojection not yet supported for grid type {other:?}"
        ))),
    }
}

/// Project geographic `(lat, lon)` polylines onto the warped raster for a
/// message, producing pixel-space runs for the render panel's overlay layer
/// (#72). Geometry-only: it rebuilds the *same* target the warp paints into
/// (via [`build_warp_target`]) but never decodes or samples values.
///
/// `latlon` is flat `[lat, lon, …]`; `ring_lengths[k]` is the vertex count of
/// input ring `k`. See [`fieldglass_core::project_polylines`] for the
/// run-splitting (visibility / antimeridian) rules.
///
/// The `"source"` projection paints grid point `(i, j)` at pixel `(i, j)`, so
/// the warp's own inverse map (lat/lon → fractional grid index) doubles as its
/// forward pixel map — the overlay projects straight through it
/// ([`SourceOverlayTarget`]), no separate geographic forward projection needed.
fn project_overlay_impl(
    meta: &MessageMeta,
    options: &RenderOptions,
    latlon: &[f64],
    ring_lengths: &[u32],
) -> napi::Result<ProjectedPolylines> {
    let resolved = ResolvedOptions::parse(options)?;
    let ni = grid_ni(meta)?;
    let nj = grid_nj(meta)?;
    let (inverse, bbox_thunk) = warp_setup_for(meta, ni, nj)?;
    match resolved.projection {
        // A source grid can wrap longitude (a global grid's seam, or the cut
        // meridian of a projected grid), so split at a raster-width jump like
        // the box targets; on a regional grid, out-of-coverage vertices invert
        // to `None` and break runs there instead.
        TargetKind::Source => Ok(project_polylines(
            &SourceOverlayTarget::new(inverse.as_ref()),
            ni,
            nj,
            resolved.flip_y,
            true,
            latlon,
            ring_lengths,
        )),
        TargetKind::Warp(target_kind) => {
            let (built, _used_bounds) =
                build_warp_target(target_kind, ni, nj, bbox_thunk, resolved.bounds);
            Ok(built.project(resolved.flip_y, latlon, ring_lengths))
        }
    }
}

// --- Per-template warp-setup helpers ---------------------------------------

/// Lazily computes the source grid's `(lat_min, lat_max, lon_min, lon_max)`
/// extent. Deferred behind a thunk because the planar sources (Lambert,
/// polar stereographic) derive it from a 512-point-per-edge perimeter walk
/// that the azimuthal targets don't need — only the lat/lon-box targets
/// invoke it.
type BboxThunk = Box<dyn FnOnce() -> (f64, f64, f64, f64)>;

/// `(inverse map, lazy lat/lon-box extent)`. The inverse closure owns a
/// projector with its constants precomputed once; the bbox thunk builds its
/// own throwaway projector only if a lat/lon-box target calls it.
type WarpSetup = (Box<dyn Fn(f64, f64) -> Option<GridIndex>>, BboxThunk);

/// Resolve the lat/lon-box extent for a box target: the source bbox from the
/// thunk, replaced wholesale by the caller's manual window when present.
fn resolve_box_extent(
    bbox: BboxThunk,
    bounds_override: Option<RenderBounds>,
) -> (f64, f64, f64, f64) {
    match bounds_override {
        Some(b) => (b.lat_min, b.lat_max, b.lon_min, b.lon_max),
        None => bbox(),
    }
}

fn latlon_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = LatLonParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lat_last: require_f64(meta.lat_last, "latLast")?,
        lon_last: require_f64(meta.lon_last, "lonLast")?,
    };
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| latlon_inverse(&p, lat, lon));
    let bbox: BboxThunk = Box::new(move || {
        (
            p.lat_first.min(p.lat_last),
            p.lat_first.max(p.lat_last),
            p.lon_first.min(p.lon_last),
            p.lon_first.max(p.lon_last),
        )
    });
    Ok((inverse, bbox))
}

fn gaussian_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let n_parallels = meta
        .gaussian_n_parallels
        .ok_or_else(|| napi::Error::from_reason("missing gaussianNParallels".to_string()))?
        as u32;
    let p = GaussianParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lat_last: require_f64(meta.lat_last, "latLast")?,
        lon_last: require_f64(meta.lon_last, "lonLast")?,
        n_parallels,
    };
    // `GaussianProjector` caches the row-ordered Gauss-Legendre lats
    // once; the inverse closure reuses it for every pixel.
    let projector = GaussianProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    let bbox: BboxThunk = Box::new(move || {
        (
            p.lat_first.min(p.lat_last),
            p.lat_first.max(p.lat_last),
            p.lon_first.min(p.lon_last),
            p.lon_first.max(p.lon_last),
        )
    });
    Ok((inverse, bbox))
}

fn lambert_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = LambertParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lad: require_f64(meta.lambert_lad, "lambertLad")?,
        lov: require_f64(meta.lambert_lov, "lambertLov")?,
        dx_metres: require_f64(meta.lambert_dx_metres, "lambertDxMetres")?,
        dy_metres: require_f64(meta.lambert_dy_metres, "lambertDyMetres")?,
        latin1: require_f64(meta.lambert_latin1, "lambertLatin1")?,
        latin2: require_f64(meta.lambert_latin2, "lambertLatin2")?,
    };

    // `LambertProjector` precomputes the cone constants + origin once for the
    // inverse closure. The bbox thunk builds its own throwaway projector only
    // when a box target needs the antimeridian-aware perimeter bbox (see
    // `PlanarGridProjector::lonlat_bbox`).
    let projector = LambertProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    let bbox: BboxThunk = Box::new(move || LambertProjector::new(p).lonlat_bbox());
    Ok((inverse, bbox))
}

fn polar_stereo_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = PolarStereoParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lov: require_f64(meta.polar_stereo_lov, "polarStereoLov")?,
        dx_metres: require_f64(meta.polar_stereo_dx_metres, "polarStereoDxMetres")?,
        dy_metres: require_f64(meta.polar_stereo_dy_metres, "polarStereoDyMetres")?,
        south_pole: meta
            .polar_stereo_south_pole
            .ok_or_else(|| napi::Error::from_reason("missing polarStereoSouthPole".to_string()))?,
    };

    let projector = PolarStereoProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    // Antimeridian-aware bbox of the four grid corners (same shape as
    // `lambert_warp_setup`). When the projection pole falls inside the grid,
    // every meridian is represented and the four-corner bbox is wrong: override
    // longitude to the full 360° and clamp latitude to the relevant pole. Built
    // lazily — only a box target invokes it.
    let bbox: BboxThunk = Box::new(move || {
        let projector = PolarStereoProjector::new(p);
        let pole_inside = projector.pole_inside_grid();
        let (mut lat_min, mut lat_max, mut lon_min, mut lon_max) = projector.lonlat_bbox();
        if pole_inside {
            lon_min = -180.0;
            lon_max = 180.0;
            if p.south_pole {
                lat_min = -90.0;
            } else {
                lat_max = 90.0;
            }
        }
        (lat_min, lat_max, lon_min, lon_max)
    });
    Ok((inverse, bbox))
}

fn grid_ni(meta: &MessageMeta) -> napi::Result<u32> {
    meta.grid_ni
        .ok_or_else(|| napi::Error::from_reason("grid has no Ni".to_string()))
        .map(|n| n as u32)
}

fn grid_nj(meta: &MessageMeta) -> napi::Result<u32> {
    meta.grid_nj
        .ok_or_else(|| napi::Error::from_reason("grid has no Nj".to_string()))
        .map(|n| n as u32)
}

fn require_f64(value: Option<f64>, name: &str) -> napi::Result<f64> {
    value.ok_or_else(|| napi::Error::from_reason(format!("missing {name}")))
}

#[cfg(test)]
mod resolved_options_tests {
    use super::*;

    fn opts(projection: &str, resampling: &str) -> RenderOptions {
        RenderOptions {
            projection: projection.to_string(),
            projection_preset: None,
            resampling: resampling.to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
        }
    }

    #[test]
    fn bounds_parse_requires_all_four_edges_and_valid_box() {
        // Complete, valid box → Some.
        let mut o = opts("equirectangular", "nearest");
        o.bounds_lat_min = Some(10.0);
        o.bounds_lat_max = Some(50.0);
        o.bounds_lon_min = Some(-120.0);
        o.bounds_lon_max = Some(-40.0);
        assert_eq!(
            ResolvedOptions::parse(&o).unwrap().bounds,
            Some(RenderBounds {
                lat_min: 10.0,
                lat_max: 50.0,
                lon_min: -120.0,
                lon_max: -40.0,
            })
        );

        // Antimeridian window: lon_min < -180 is allowed (lon_max > lon_min).
        o.bounds_lon_min = Some(-183.0);
        o.bounds_lon_max = Some(-32.0);
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_some());

        // Partial box → None (silent fallback to computed bounds).
        o.bounds_lon_max = None;
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_none());

        // Inverted box → None.
        o.bounds_lon_min = Some(50.0);
        o.bounds_lon_max = Some(-50.0);
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_none());
    }

    #[test]
    fn parses_valid_combinations() {
        let r = ResolvedOptions::parse(&opts("source", "nearest")).expect("source/nearest");
        assert!(matches!(r.projection, TargetKind::Source));
        assert_eq!(r.resampling, Resampling::Nearest);

        let r = ResolvedOptions::parse(&opts("equirectangular", "bilinear")).expect("eqr/bilinear");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::Equirectangular)
        ));
        assert_eq!(r.resampling, Resampling::Bilinear);

        let r = ResolvedOptions::parse(&opts("web_mercator", "nearest")).expect("merc/nearest");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::WebMercator)
        ));

        // Azimuthal targets resolve their preset into concrete parameters.
        let r = ResolvedOptions::parse(&opts("orthographic", "nearest")).expect("ortho/nearest");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 0.0
        ));
        let r =
            ResolvedOptions::parse(&opts("polar_stereographic", "nearest")).expect("polar/nearest");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::PolarStereographic {
                south_pole: false,
                ..
            })
        ));
    }

    #[test]
    fn orthographic_preset_selects_centre() {
        let mut o = opts("orthographic", "nearest");
        o.projection_preset = Some("pacific".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 180.0
        ));
        o.projection_preset = Some("indian".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 90.0
        ));
        o.projection_preset = Some("americas".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 270.0
        ));
        o.projection_preset = Some("north_pole".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, .. }) if lat0 == 90.0
        ));
        // Unknown preset falls back to the Atlantic default.
        o.projection_preset = Some("nonsense".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 0.0
        ));
    }

    #[test]
    fn polar_stereographic_preset_selects_hemisphere() {
        let mut o = opts("polar_stereographic", "nearest");
        o.projection_preset = Some("south".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::PolarStereographic {
                south_pole: true,
                ..
            })
        ));
    }

    #[test]
    fn rejects_unknown_projection() {
        let err = ResolvedOptions::parse(&opts("mollweide", "nearest"))
            .expect_err("unknown projection must error");
        assert!(
            err.to_string().contains("unknown projection"),
            "error names the field, got: {err}"
        );
    }

    #[test]
    fn rejects_unknown_resampling() {
        let err = ResolvedOptions::parse(&opts("source", "bicubic"))
            .expect_err("unknown resampling must error");
        assert!(
            err.to_string().contains("unknown resampling"),
            "error names the field, got: {err}"
        );
    }
}

#[cfg(test)]
mod polar_stereo_warp_tests {
    use super::*;

    /// MessageMeta mirroring the `cmc_wind_300_2010052400_p012.grib`
    /// fixture: 135×95 polar-stereographic, 60 km at 60°N, north-polar.
    /// Only the fields warp_message actually consults are filled — every
    /// other Option-typed slot is left `None` to keep the synthetic
    /// minimal.
    fn cmc_polar_meta() -> MessageMeta {
        MessageMeta {
            message_index: 0,
            offset_bytes: 0,
            parameter_name: String::new(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: String::new(),
            level_type: String::new(),
            reference_time: String::new(),
            forecast_hours: 0,
            forecast_display: String::new(),
            originating_centre: String::new(),
            grid_type: Some("polar_stereo".to_string()),
            grid_ni: Some(135),
            grid_nj: Some(95),
            lat_first: Some(11.43),
            lon_first: Some(-110.27),
            lat_last: None,
            lon_last: None,
            format: "grib1".to_string(),
            edition: Some(1),
            discipline: None,
            total_length_bytes: None,
            production_status: None,
            data_type: None,
            lambert_lad: None,
            lambert_lov: None,
            lambert_dx_metres: None,
            lambert_dy_metres: None,
            lambert_latin1: None,
            lambert_latin2: None,
            gaussian_n_parallels: None,
            polar_stereo_lov: Some(247.0),
            polar_stereo_dx_metres: Some(60_000.0),
            polar_stereo_dy_metres: Some(60_000.0),
            polar_stereo_south_pole: Some(false),
        }
    }

    #[test]
    fn source_overlay_projects_onto_a_polar_stereo_grid() {
        // #72: the source-projection overlay must work for *projected* grids,
        // not just regular lat/lon — it reuses the grid's own inverse map. A
        // short polyline over North America (inside the CMC polar grid)
        // projects to a non-empty, shape-consistent run.
        let opts = RenderOptions {
            projection: "source".to_string(),
            projection_preset: None,
            resampling: "nearest".to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
        };
        let latlon = [40.0, -100.0, 41.0, -99.0, 42.0, -98.0];
        let out = project_overlay_impl(&cmc_polar_meta(), &opts, &latlon, &[3])
            .expect("source overlay on polar grid");
        let total: u32 = out.seg_lengths.iter().copied().sum();
        assert_eq!(total as usize * 2, out.xy.len(), "shape invariant");
        assert!(!out.xy.is_empty(), "polyline over the grid should project");
    }

    #[test]
    fn warps_polar_stereo_to_equirectangular() {
        let meta = cmc_polar_meta();
        // Synthetic uniform field — we're testing the warp geometry, not
        // value transport. Every present output pixel should read back
        // exactly 1.0.
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let (values, mask, w, h, _bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("warp");

        assert_eq!(w, 135);
        assert_eq!(h, 95);
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count > 0,
            "polar-stereo warp produced an entirely empty mask — \
             either the inverse map rejects every pixel or warp_setup \
             returned bad bounds"
        );
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(
            summary.contains("polar_stereo") && summary.contains("equirectangular"),
            "summary should name source kind + target, got: {summary}"
        );
    }

    #[test]
    fn warps_polar_stereo_to_web_mercator() {
        // The Web Mercator target shares the polar-stereo source inverse map
        // and bbox; it just distributes rows in Mercator Y. Verify the path
        // produces a non-empty mask, transports values, clamps the latitude
        // extent into the Mercator band, and names the target in the summary.
        let meta = cmc_polar_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let (values, mask, w, h, bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::WebMercator,
            Resampling::Nearest,
            None,
        )
        .expect("mercator warp");

        assert_eq!(w, 135);
        assert_eq!(h, 95);
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(present_count > 0, "mercator warp produced an empty mask");
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        let (lat_min, lat_max, _, _) = bounds.expect("web mercator has bounds");
        assert!(
            lat_min >= -85.06 && lat_max <= 85.06,
            "lat extent must be clamped to the Mercator band, got {lat_min}..{lat_max}"
        );
        assert!(
            summary.contains("polar_stereo") && summary.contains("web mercator"),
            "summary should name source kind + target, got: {summary}"
        );
    }

    #[test]
    fn warps_polar_stereo_to_azimuthal_targets() {
        // The orthographic and polar-stereographic *targets* fit a disc to the
        // raster: they share the source inverse map but report no lat/lon-box
        // extent. Verify both produce a non-empty mask and the right summary,
        // and that bounds come back `None`.
        let meta = cmc_polar_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];

        for (target, name) in [
            (
                WarpTarget::Orthographic {
                    lat0: 90.0,
                    lon0: 0.0,
                },
                "orthographic",
            ),
            (
                WarpTarget::PolarStereographic {
                    south_pole: false,
                    lon0: 0.0,
                },
                "polar stereographic",
            ),
        ] {
            let (values, mask, w, h, bounds, summary) =
                warp_message(&meta, &raw, target, Resampling::Nearest, None)
                    .unwrap_or_else(|e| panic!("{name} warp failed: {e}"));
            // Azimuthal discs render into a square raster (side = the larger
            // source axis) so the globe stays circular rather than stretching
            // into an ellipse on the 135×95 source.
            assert_eq!((w, h), (135, 135));
            let present = mask.iter().filter(|&&m| m == 1).count();
            assert!(present > 0, "{name} target produced an empty mask");
            for (i, &m) in mask.iter().enumerate() {
                if m == 1 {
                    assert_eq!(values[i], 1.0, "{name} present pixel {i} should be 1.0");
                }
            }
            assert!(bounds.is_none(), "{name} target has no lat/lon-box extent");
            assert!(
                summary.contains("polar_stereo") && summary.contains(name),
                "{name} summary should name source + target, got: {summary}"
            );
        }
    }

    #[test]
    fn warps_south_polar_stereo_to_equirectangular() {
        // Mirror the CMC tile into the southern hemisphere: south-pole
        // projection, negative lat_first. Exercises the `sign = -1` branch
        // through the full warp_message → polar_stereo_warp_setup path,
        // not just the projection-level round-trip tests.
        let meta = MessageMeta {
            lat_first: Some(-11.43),
            polar_stereo_south_pole: Some(true),
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let (values, mask, w, h, _bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("south-polar warp");

        assert_eq!(w, 135);
        assert_eq!(h, 95);
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count > 0,
            "south-polar warp produced an entirely empty mask"
        );
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(summary.contains("polar_stereo") && summary.contains("equirectangular"));
    }

    #[test]
    fn warps_hemispheric_grid_with_pole_inside() {
        // A synthetic hemispheric grid whose projected extent surrounds the
        // pole (same geometry as the projection-level
        // `polar_stereo_pole_inside_grid_detection` test). Two things this
        // pins that the regional CMC test does not:
        //   1. negative `dy_metres` (south-scanning) is handled by the warp
        //      setup, not just the projector unit test;
        //   2. the 360°-longitude / pole-clamp override path in
        //      polar_stereo_warp_setup is reachable through warp_message
        //      and produces a fully-covered raster instead of a thin
        //      four-corner sliver.
        let meta = MessageMeta {
            grid_ni: Some(4),
            grid_nj: Some(4),
            lat_first: Some(50.8),
            lon_first: Some(-135.0),
            polar_stereo_lov: Some(0.0),
            polar_stereo_dx_metres: Some(2_000_000.0),
            polar_stereo_dy_metres: Some(-2_000_000.0),
            polar_stereo_south_pole: Some(false),
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 4 * 4];
        let (_values, mask, w, h, _bounds, _summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("hemispheric warp");

        assert_eq!((w, h), (4, 4));
        // With the pole inside the grid the target spans the full hemisphere,
        // so a clear majority of the 4×4 output pixels resolve to a source
        // sample rather than the handful a four-corner bbox would cover.
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count >= 8,
            "pole-inside grid should fill most of the raster, got {present_count}/16 present"
        );
    }

    #[test]
    fn warps_grid_with_pole_exactly_on_origin() {
        // lat_first = 90° puts the first scanned point at the pole, so the
        // projected grid origin is exactly (0, 0). `pole_inside_grid` uses
        // inclusive bounds, so this edge case must still take the
        // 360°-longitude override and warp without panicking.
        let meta = MessageMeta {
            grid_ni: Some(4),
            grid_nj: Some(4),
            lat_first: Some(90.0),
            lon_first: Some(0.0),
            polar_stereo_lov: Some(0.0),
            polar_stereo_dx_metres: Some(2_000_000.0),
            polar_stereo_dy_metres: Some(2_000_000.0),
            polar_stereo_south_pole: Some(false),
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 4 * 4];
        let (_values, mask, w, h, _bounds, _summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("pole-on-origin warp");
        assert_eq!((w, h), (4, 4));
        assert!(
            mask.contains(&1),
            "pole-on-origin grid should still resolve some pixels"
        );
    }

    #[test]
    fn polar_stereo_warp_errors_without_required_fields() {
        // Missing polarStereoLov — warp setup must surface a clear error
        // naming the field rather than silently falling back to a default.
        let bad = MessageMeta {
            polar_stereo_lov: None,
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let err = warp_message(
            &bad,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect_err("missing lov must error");
        assert!(
            err.to_string().contains("polarStereoLov"),
            "error should name the missing field, got: {err}"
        );
    }

    #[test]
    fn bounds_override_replaces_computed_extent_and_echoes_back() {
        let meta = cmc_polar_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];

        // Default: no override → echoed bounds are the computed source extent.
        let (.., default_bounds, _) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("default warp");
        let default_bounds = default_bounds.expect("equirectangular has bounds");

        // Explicit window → that window is rendered and echoed back verbatim.
        let window = RenderBounds {
            lat_min: 30.0,
            lat_max: 60.0,
            lon_min: -140.0,
            lon_max: -60.0,
        };
        let (_v, _m, _w, _h, used, _s) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            Some(window),
        )
        .expect("windowed warp");
        assert_eq!(used, Some((30.0, 60.0, -140.0, -60.0)));
        assert_ne!(
            used.unwrap(),
            default_bounds,
            "override should differ from the computed default"
        );
    }
}

#[cfg(test)]
mod overlay_projection_tests {
    use super::*;

    /// A global 1°-resolution lat/lon grid (361×181, lat 90..-90, lon
    /// -180..180). Under the equirectangular target a vertex projects to a
    /// predictable pixel: `px = lon + 180`, `py = 90 - lat`.
    fn global_latlon_meta() -> MessageMeta {
        MessageMeta {
            message_index: 0,
            offset_bytes: 0,
            parameter_name: String::new(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: String::new(),
            level_type: String::new(),
            reference_time: String::new(),
            forecast_hours: 0,
            forecast_display: String::new(),
            originating_centre: String::new(),
            grid_type: Some("latlon".to_string()),
            grid_ni: Some(361),
            grid_nj: Some(181),
            lat_first: Some(90.0),
            lon_first: Some(-180.0),
            lat_last: Some(-90.0),
            lon_last: Some(180.0),
            format: "grib1".to_string(),
            edition: Some(1),
            discipline: None,
            total_length_bytes: None,
            production_status: None,
            data_type: None,
            lambert_lad: None,
            lambert_lov: None,
            lambert_dx_metres: None,
            lambert_dy_metres: None,
            lambert_latin1: None,
            lambert_latin2: None,
            gaussian_n_parallels: None,
            polar_stereo_lov: None,
            polar_stereo_dx_metres: None,
            polar_stereo_dy_metres: None,
            polar_stereo_south_pole: None,
        }
    }

    fn overlay_opts(projection: &str) -> RenderOptions {
        RenderOptions {
            projection: projection.to_string(),
            projection_preset: None,
            resampling: "nearest".to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
        }
    }

    #[test]
    fn source_projection_projects_through_the_inverse_map() {
        // The source raster paints grid point (i, j) at pixel (i, j), so the
        // overlay projects through the grid's own inverse map. For this global
        // 1° lat/lon grid that lands at px = lon + 180, py = 90 - lat —
        // matching the equirectangular target's pixels.
        let out = project_overlay_impl(
            &global_latlon_meta(),
            &overlay_opts("source"),
            &[45.0, 0.0, 45.0, 10.0],
            &[2],
        )
        .expect("source overlay");
        assert_eq!(out.seg_lengths, vec![2]);
        assert!((out.xy[0] - 180.0).abs() < 1e-6, "lon 0 → px {}", out.xy[0]);
        assert!((out.xy[1] - 45.0).abs() < 1e-6, "lat 45 → py {}", out.xy[1]);
        assert!(
            (out.xy[2] - 190.0).abs() < 1e-6,
            "lon 10 → px {}",
            out.xy[2]
        );
    }

    #[test]
    fn equirectangular_projects_to_predictable_pixels() {
        let out = project_overlay_impl(
            &global_latlon_meta(),
            &overlay_opts("equirectangular"),
            &[45.0, 0.0, 45.0, 10.0], // (lat 45, lon 0), (lat 45, lon 10)
            &[2],
        )
        .expect("equirect overlay");
        assert_eq!(out.seg_lengths, vec![2]);
        assert_eq!(out.xy.len(), 4);
        // px = lon + 180, py = 90 - lat.
        assert!((out.xy[0] - 180.0).abs() < 1e-6, "lon 0 → px {}", out.xy[0]);
        assert!((out.xy[1] - 45.0).abs() < 1e-6, "lat 45 → py {}", out.xy[1]);
        assert!(
            (out.xy[2] - 190.0).abs() < 1e-6,
            "lon 10 → px {}",
            out.xy[2]
        );
    }

    #[test]
    fn flip_y_mirrors_overlay_rows() {
        let mut o = overlay_opts("equirectangular");
        o.flip_y = true;
        let out = project_overlay_impl(&global_latlon_meta(), &o, &[45.0, 0.0, 45.0, 10.0], &[2])
            .expect("flipped overlay");
        // height = nj = 181 → py flips to (181 - 1) - 45 = 135.
        assert!((out.xy[1] - 135.0).abs() < 1e-6, "flipped py {}", out.xy[1]);
        assert!((out.xy[0] - 180.0).abs() < 1e-6, "x unaffected by flip");
    }

    #[test]
    fn output_shape_invariant_holds() {
        // sum(seg_lengths) * 2 == xy.len() for every projection.
        for projection in [
            "source",
            "equirectangular",
            "web_mercator",
            "orthographic",
            "polar_stereographic",
        ] {
            let out = project_overlay_impl(
                &global_latlon_meta(),
                &overlay_opts(projection),
                &[80.0, 0.0, 81.0, 1.0, 82.0, 2.0],
                &[3],
            )
            .unwrap_or_else(|e| panic!("{projection} overlay: {e}"));
            let total: u32 = out.seg_lengths.iter().copied().sum();
            assert_eq!(
                total as usize * 2,
                out.xy.len(),
                "{projection}: seg_lengths must account for every xy pair"
            );
        }
    }
}
