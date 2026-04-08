#![deny(clippy::all)]

use fieldglass_core::{detect_format, Format};
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
    pub format: String,
}

/// Detect the format of a meteorological data file from its extension.
/// Returns "grib1" | "grib2" | "netcdf" | "unknown".
#[napi]
pub fn detect(file_path: String) -> String {
    match detect_format(&file_path) {
        Format::Grib1 => "grib1".to_string(),
        Format::Grib2 => "grib2".to_string(),
        Format::NetCdf => "netcdf".to_string(),
        Format::Unknown => "unknown".to_string(),
    }
}

/// Open a meteorological data file and return its message metadata.
/// Format is auto-detected from extension. Returns empty vec until parsers are implemented.
#[napi]
pub fn open(file_path: String) -> napi::Result<Vec<MessageMeta>> {
    let _ = file_path;
    Ok(vec![])
}
