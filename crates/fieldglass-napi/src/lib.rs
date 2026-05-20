#![deny(clippy::all)]

use fieldglass_core::{
    Format, GaussianParams, GaussianProjector, LambertParams, LambertProjector, LatLonParams,
    PolarStereoParams, PolarStereoProjector, Resampling, SourceGrid, TargetRaster,
    colormap::{min_max_ignoring_mask, paint_grid_rgba},
    detect_from_bytes, latlon_inverse,
    projection::GridIndex,
    warp::warp_to_equirectangular,
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
    pub resampling: String,
    pub flip_y: bool,
    /// Manual range override. When either is `None` the renderer uses the
    /// computed min/max over the present cells.
    pub range_min: Option<f64>,
    pub range_max: Option<f64>,
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
    /// Human-readable summary of the source→target projection chain,
    /// e.g. `"lambert → equirectangular (nearest)"`.
    pub projection_summary: String,
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
    projection: TargetProjection,
    resampling: Resampling,
    flip_y: bool,
    range_min: Option<f64>,
    range_max: Option<f64>,
}

#[cfg_attr(test, derive(Debug))]
enum TargetProjection {
    Source,
    Equirectangular,
}

impl ResolvedOptions {
    fn parse(options: &RenderOptions) -> napi::Result<Self> {
        let projection = match options.projection.as_str() {
            "source" => TargetProjection::Source,
            "equirectangular" => TargetProjection::Equirectangular,
            other => {
                return Err(napi::Error::from_reason(format!(
                    "unknown projection {other:?} (expected \"source\" or \"equirectangular\")"
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
        })
    }
}

fn render_with_options(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    options: &RenderOptions,
) -> napi::Result<RenderedGrid> {
    let resolved = ResolvedOptions::parse(options)?;
    let (values, mask, width, height, summary) = match resolved.projection {
        TargetProjection::Source => paint_source(meta, raw)?,
        TargetProjection::Equirectangular => warp_message(meta, raw, resolved.resampling)?,
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

    Ok(RenderedGrid {
        rgba: rgba.into(),
        width: width as i32,
        height: height as i32,
        used_min,
        used_max,
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
fn paint_source(
    meta: &MessageMeta,
    raw: &[Option<f64>],
) -> napi::Result<(Vec<f64>, Vec<u8>, u32, u32, String)> {
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
    Ok((values, mask, ni, nj, summary))
}

/// Dispatch warp setup to the right per-template helper. Each helper
/// builds the inverse closure (using a projector with precomputed state)
/// and the equirectangular target bounds; the shared finisher then runs
/// the warp itself.
#[allow(clippy::type_complexity)]
fn warp_message(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    resampling: Resampling,
) -> napi::Result<(Vec<f64>, Vec<u8>, u32, u32, String)> {
    let kind = meta.grid_type.as_deref().unwrap_or("");
    let ni = grid_ni(meta)?;
    let nj = grid_nj(meta)?;

    let sample = |i: usize, j: usize| -> Option<f64> {
        let k = j * ni as usize + i;
        raw.get(k).copied().flatten()
    };
    let sample_ref: &dyn Fn(usize, usize) -> Option<f64> = &sample;

    let (target, inverse_boxed) = match kind {
        "latlon" => latlon_warp_setup(meta, ni, nj)?,
        "gaussian" => gaussian_warp_setup(meta, ni, nj)?,
        "lambert" => lambert_warp_setup(meta, ni, nj)?,
        "polar_stereo" => polar_stereo_warp_setup(meta, ni, nj)?,
        other => {
            return Err(napi::Error::from_reason(format!(
                "reprojection not yet supported for grid type {other:?}"
            )));
        }
    };

    let inverse_ref: &dyn Fn(f64, f64) -> Option<GridIndex> = inverse_boxed.as_ref();
    let source = SourceGrid {
        ni,
        nj,
        sample: sample_ref,
        inverse_at: inverse_ref,
    };
    let warped = warp_to_equirectangular(&source, &target, resampling);
    let resample_label = match resampling {
        Resampling::Nearest => "nearest",
        Resampling::Bilinear => "bilinear",
    };
    let summary = format!(
        "{} → equirectangular ({resample_label})",
        source_projection_summary(meta),
    );
    Ok((
        warped.values,
        warped.mask,
        warped.width,
        warped.height,
        summary,
    ))
}

// --- Per-template warp-setup helpers ---------------------------------------

type WarpSetup = (TargetRaster, Box<dyn Fn(f64, f64) -> Option<GridIndex>>);

fn latlon_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = LatLonParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lat_last: require_f64(meta.lat_last, "latLast")?,
        lon_last: require_f64(meta.lon_last, "lonLast")?,
    };
    let target = TargetRaster {
        width: ni,
        height: nj,
        lat_max: p.lat_first.max(p.lat_last),
        lat_min: p.lat_first.min(p.lat_last),
        lon_min: p.lon_first.min(p.lon_last),
        lon_max: p.lon_first.max(p.lon_last),
    };
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| latlon_inverse(&p, lat, lon));
    Ok((target, inverse))
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
    let target = TargetRaster {
        width: ni,
        height: nj,
        lat_max: p.lat_first.max(p.lat_last),
        lat_min: p.lat_first.min(p.lat_last),
        lon_min: p.lon_first.min(p.lon_last),
        lon_max: p.lon_first.max(p.lon_last),
    };
    // `GaussianProjector` caches the row-ordered Gauss-Legendre lats
    // once; the inverse closure reuses it for every pixel.
    let projector = GaussianProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    Ok((target, inverse))
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

    // `LambertProjector` precomputes the cone constants + origin once.
    let projector = LambertProjector::new(p);
    // Target bounds = bounding box of the four forward-projected grid
    // corners.
    let origin = projector.origin();
    let corners = [
        origin,
        (origin.0 + (ni as f64 - 1.0) * p.dx_metres, origin.1),
        (origin.0, origin.1 + (nj as f64 - 1.0) * p.dy_metres),
        (
            origin.0 + (ni as f64 - 1.0) * p.dx_metres,
            origin.1 + (nj as f64 - 1.0) * p.dy_metres,
        ),
    ];
    let (mut lat_min, mut lat_max, mut lon_min, mut lon_max) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in corners {
        let (lat, lon) = projector.inverse_xy(x, y);
        lat_min = lat_min.min(lat);
        lat_max = lat_max.max(lat);
        lon_min = lon_min.min(lon);
        lon_max = lon_max.max(lon);
    }
    let target = TargetRaster {
        width: ni,
        height: nj,
        lat_max,
        lat_min,
        lon_min,
        lon_max,
    };
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    Ok((target, inverse))
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
    let pole_inside = projector.pole_inside_grid();
    // Compute the equirectangular bbox by inverse-projecting the four
    // forward-projected corners (same shape as `lambert_warp_setup`). When
    // the projection pole falls inside the grid, every meridian is
    // represented and the four-corner bbox is wrong: we override longitude
    // to the full 360° and clamp latitude to the relevant pole.
    let origin = projector.origin();
    let corners = [
        origin,
        (origin.0 + (ni as f64 - 1.0) * p.dx_metres, origin.1),
        (origin.0, origin.1 + (nj as f64 - 1.0) * p.dy_metres),
        (
            origin.0 + (ni as f64 - 1.0) * p.dx_metres,
            origin.1 + (nj as f64 - 1.0) * p.dy_metres,
        ),
    ];
    let (mut lat_min, mut lat_max, mut lon_min, mut lon_max) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for (x, y) in corners {
        let (lat, lon) = projector.inverse_xy(x, y);
        lat_min = lat_min.min(lat);
        lat_max = lat_max.max(lat);
        lon_min = lon_min.min(lon);
        lon_max = lon_max.max(lon);
    }
    if pole_inside {
        lon_min = -180.0;
        lon_max = 180.0;
        if p.south_pole {
            lat_min = -90.0;
        } else {
            lat_max = 90.0;
        }
    }
    let target = TargetRaster {
        width: ni,
        height: nj,
        lat_max,
        lat_min,
        lon_min,
        lon_max,
    };
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    Ok((target, inverse))
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
            resampling: resampling.to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
        }
    }

    #[test]
    fn parses_valid_combinations() {
        let r = ResolvedOptions::parse(&opts("source", "nearest")).expect("source/nearest");
        assert!(matches!(r.projection, TargetProjection::Source));
        assert_eq!(r.resampling, Resampling::Nearest);

        let r = ResolvedOptions::parse(&opts("equirectangular", "bilinear")).expect("eqr/bilinear");
        assert!(matches!(r.projection, TargetProjection::Equirectangular));
        assert_eq!(r.resampling, Resampling::Bilinear);
    }

    #[test]
    fn rejects_unknown_projection() {
        let err = ResolvedOptions::parse(&opts("orthographic", "nearest"))
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
    fn warps_polar_stereo_to_equirectangular() {
        let meta = cmc_polar_meta();
        // Synthetic uniform field — we're testing the warp geometry, not
        // value transport. Every present output pixel should read back
        // exactly 1.0.
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let (values, mask, w, h, summary) =
            warp_message(&meta, &raw, Resampling::Nearest).expect("warp");

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
    fn polar_stereo_warp_errors_without_required_fields() {
        // Missing polarStereoLov — warp setup must surface a clear error
        // naming the field rather than silently falling back to a default.
        let bad = MessageMeta {
            polar_stereo_lov: None,
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let err =
            warp_message(&bad, &raw, Resampling::Nearest).expect_err("missing lov must error");
        assert!(
            err.to_string().contains("polarStereoLov"),
            "error should name the missing field, got: {err}"
        );
    }
}
