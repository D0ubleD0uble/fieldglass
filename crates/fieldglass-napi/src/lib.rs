#![deny(clippy::all)]

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

/// Open a meteorological data file and return its message metadata.
/// Format is auto-detected from magic bytes and extension.
#[napi]
pub fn open(file_path: String) -> napi::Result<Vec<MessageMeta>> {
    // TODO: dispatch to correct format reader via fieldglass_core::detect_format
    // and return parsed metadata for all messages.
    let _ = file_path;
    Ok(vec![])
}
