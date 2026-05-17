#![deny(clippy::all)]

use fieldglass_core::{Format, detect_from_bytes};
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

/// Patch the PDS `p1` (forecast period) octet of one message and return a new
/// buffer containing the modified file bytes. Length is preserved.
#[napi]
pub fn set_p1(
    bytes: napi::bindgen_prelude::Buffer,
    message_index: u32,
    value: u32,
) -> napi::Result<napi::bindgen_prelude::Buffer> {
    if value > u8::MAX as u32 {
        return Err(napi::Error::from_reason(format!(
            "p1 must fit in a u8 (0..=255), got {value}"
        )));
    }
    let mut out = bytes.to_vec();
    let reader = Grib1Reader::from_bytes(out.clone())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    let msg = reader.messages.get(message_index as usize).ok_or_else(|| {
        napi::Error::from_reason(format!(
            "message index {message_index} out of range (have {})",
            reader.messages.len()
        ))
    })?;
    let off = msg.pds_p1_offset();
    out[off] = value as u8;
    Ok(out.into())
}

/// Decode the grid values for one GRIB message. Returns one entry per grid
/// point in scan order: a number for present points, `null` for points that
/// are masked out by the message's Bit Map Section.
///
/// Format dispatch is by magic-byte detection: GRIB1 vs GRIB2 readers run
/// the same shape end-to-end, so the JS render pipeline doesn't need to
/// branch on edition.
//
// TODO(perf): every call here (and in set_p1, open_grib1) clones the full
// file buffer and re-parses every message. Hold a reader across napi
// calls via a handle table; also return Float64Array + Uint8Array directly
// instead of Vec<Option<f64>> to avoid the boxed-Array round trip.
#[napi]
pub fn decode_grid(
    bytes: napi::bindgen_prelude::Buffer,
    message_index: u32,
) -> napi::Result<Vec<Option<f64>>> {
    let raw = bytes.to_vec();
    match detect_from_bytes(&raw) {
        Format::Grib1 => {
            let reader = Grib1Reader::from_bytes(raw)
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            reader
                .decode_message_values(message_index as usize)
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }
        Format::Grib2 => {
            let reader = Grib2Reader::from_bytes(raw)
                .map_err(|e| napi::Error::from_reason(e.to_string()))?;
            reader
                .decode_message_values(message_index as usize)
                .map_err(|e| napi::Error::from_reason(e.to_string()))
        }
        Format::NetCdf => Err(napi::Error::from_reason(
            "decode_grid only supports GRIB1 / GRIB2, got netcdf".to_string(),
        )),
        Format::Unknown => Err(napi::Error::from_reason(
            "decode_grid: unrecognised format".to_string(),
        )),
    }
}

/// Parse a GRIB1 file from raw bytes and return metadata for each message.
#[napi]
pub fn open_grib1(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Vec<MessageMeta>> {
    let reader = Grib1Reader::from_bytes(bytes.to_vec())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;

    let mut result = Vec::with_capacity(reader.messages.len());
    for msg in &reader.messages {
        let param = lookup_parameter(msg.pds.parameter_id, msg.pds.table_version);

        let (grid_type, grid_ni, grid_nj, lat_first, lon_first, lat_last, lon_last) = match &msg.gds
        {
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

        // Surface the projection parameters needed by the renderer's
        // reprojection warp. Only populated for the matching grid types;
        // other shapes pass through as `None`.
        let lambert = match &msg.gds {
            Some(fieldglass_grib1::GridDescription::LambertConformal(g)) => Some(g),
            _ => None,
        };
        let lambert_lad = lambert.map(|g| g.latin1); // GRIB1 has no separate LaD; convention mirrors latin1
        let lambert_lov = lambert.map(|g| g.lov);
        let lambert_dx_metres = lambert.map(|g| g.dx_m as f64);
        let lambert_dy_metres = lambert.map(|g| g.dy_m as f64);
        let lambert_latin1 = lambert.map(|g| g.latin1);
        let lambert_latin2 = lambert.map(|g| g.latin2);
        let gaussian_n_parallels = match &msg.gds {
            Some(fieldglass_grib1::GridDescription::Gaussian(g)) => Some(g.n_gaussians as i32),
            _ => None,
        };

        result.push(MessageMeta {
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
        });
    }
    Ok(result)
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
#[napi]
pub fn open_grib2(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Vec<MessageMeta>> {
    let reader = Grib2Reader::from_bytes(bytes.to_vec())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;

    let mut result = Vec::with_capacity(reader.messages.len());
    for msg in &reader.messages {
        let centre = lookup_grib2_centre(msg.ids.centre)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Centre {}", msg.ids.centre));
        let dims = msg.gds.dimensions();
        let bounds = msg.gds.bounds();
        let product = grib2_product_fields(msg.is.discipline, &msg.pds);

        // Surface the projection parameters needed by the renderer's
        // reprojection warp. Only populated for the matching templates.
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

        result.push(MessageMeta {
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
            production_status: Some(
                lookup_production_status(msg.ids.production_status).to_string(),
            ),
            data_type: Some(fieldglass_grib2::lookup_data_type(msg.ids.data_type).to_string()),
            lambert_lad,
            lambert_lov,
            lambert_dx_metres,
            lambert_dy_metres,
            lambert_latin1,
            lambert_latin2,
            gaussian_n_parallels,
        });
    }
    Ok(result)
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
