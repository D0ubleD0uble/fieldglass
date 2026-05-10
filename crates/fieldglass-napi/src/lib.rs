#![deny(clippy::all)]

use fieldglass_core::{Format, detect_from_bytes};
use fieldglass_grib1::{
    Grib1Reader,
    tables::{lookup_centre, lookup_parameter},
};
use fieldglass_grib2::{Grib2Reader, lookup_discipline};
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

/// Decode the grid values for one GRIB1 message. Returns one entry per grid
/// point in scan order: a number for present points, `null` for points that
/// are masked out by the message's Bit Map Section.
#[napi]
pub fn decode_grid(
    bytes: napi::bindgen_prelude::Buffer,
    message_index: u32,
) -> napi::Result<Vec<Option<f64>>> {
    let reader = Grib1Reader::from_bytes(bytes.to_vec())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    reader
        .decode_message_values(message_index as usize)
        .map_err(|e| napi::Error::from_reason(e.to_string()))
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
        });
    }
    Ok(result)
}

/// Parse a GRIB2 file from raw bytes and return Indicator-Section metadata
/// for each message. Phase 4.0 only populates IS-level fields (edition,
/// discipline, total length); other columns are placeholders until the
/// per-section parsers land.
#[napi]
pub fn open_grib2(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Vec<MessageMeta>> {
    let reader = Grib2Reader::from_bytes(bytes.to_vec())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;

    let mut result = Vec::with_capacity(reader.messages.len());
    for msg in &reader.messages {
        result.push(MessageMeta {
            message_index: msg.message_index as i32,
            offset_bytes: msg.byte_offset as i32,
            // Until per-section parsers land, surface the discipline string
            // in the existing `parameter_name` column so the table renderer
            // shows something meaningful per row without rendering-side
            // changes. The structured discipline is also exposed below.
            parameter_name: lookup_discipline(msg.is.discipline).to_string(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: "—".to_string(),
            level_type: "—".to_string(),
            reference_time: "—".to_string(),
            forecast_hours: 0,
            forecast_display: "—".to_string(),
            originating_centre: format!("{} bytes", msg.is.total_length),
            grid_type: None,
            grid_ni: None,
            grid_nj: None,
            lat_first: None,
            lon_first: None,
            lat_last: None,
            lon_last: None,
            format: "grib2".to_string(),
            edition: Some(i32::from(msg.is.edition)),
            discipline: Some(lookup_discipline(msg.is.discipline).to_string()),
            total_length_bytes: Some(msg.is.total_length as f64),
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
