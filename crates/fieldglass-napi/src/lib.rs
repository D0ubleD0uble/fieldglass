#![deny(clippy::all)]

use fieldglass_core::{detect_from_bytes, Format};
use fieldglass_grib1::{
    tables::{lookup_centre, lookup_level_type, lookup_parameter},
    Grib1Reader,
};
use napi_derive::napi;

/// A single message's metadata, exposed to Node.js.
#[napi(object)]
pub struct MessageMeta {
    pub message_index: i32,
    pub offset_bytes: i32,
    pub parameter_name: String,
    pub parameter_units: String,
    pub parameter_abbreviation: String,
    pub level_type: String,
    pub level_value: f64,
    pub reference_time: String,
    pub forecast_hours: i32,
    pub originating_centre: String,
    pub grid_type: Option<String>,
    pub grid_ni: Option<i32>,
    pub grid_nj: Option<i32>,
    pub lat_first: Option<f64>,
    pub lon_first: Option<f64>,
    pub lat_last: Option<f64>,
    pub lon_last: Option<f64>,
    pub format: String,
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

        let (grid_type, grid_ni, grid_nj, lat_first, lon_first, lat_last, lon_last) =
            match &msg.gds {
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
            level_type: lookup_level_type(msg.pds.level_type).to_string(),
            level_value: fieldglass_grib1::level_value(&msg.pds),
            reference_time: fieldglass_grib1::reference_time(&msg.pds),
            forecast_hours: fieldglass_grib1::forecast_hours(&msg.pds),
            originating_centre: lookup_centre(msg.pds.originating_centre).to_string(),
            grid_type,
            grid_ni,
            grid_nj,
            lat_first,
            lon_first,
            lat_last,
            lon_last,
            format: "grib1".to_string(),
        });
    }
    Ok(result)
}
