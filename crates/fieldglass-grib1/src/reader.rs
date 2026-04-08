use fieldglass_core::FieldglassError;
use crate::is::{parse_indicator, IndicatorSection};
use crate::pds::{parse_product_definition, ProductDefinition};

pub struct Grib1Message {
    pub message_index: usize,
    pub byte_offset: usize,
    pub is: IndicatorSection,
    pub pds: ProductDefinition,
}

pub struct Grib1Reader {
    data: Vec<u8>,
    pub messages: Vec<Grib1Message>,
}

impl Grib1Reader {
    /// Parse a GRIB1 file from raw bytes, scanning for all messages.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, FieldglassError> {
        let messages = scan_messages(&data)?;
        Ok(Self { data, messages })
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

/// Scan `data` for GRIB messages. Each message starts with the `GRIB` magic
/// bytes; the IS provides the total length so we can jump to the next message.
fn scan_messages(data: &[u8]) -> Result<Vec<Grib1Message>, FieldglassError> {
    let mut messages = Vec::new();
    let mut offset = 0usize;

    while offset + 8 <= data.len() {
        // Search forward for the next GRIB marker.
        if &data[offset..offset + 4] != b"GRIB" {
            offset += 1;
            continue;
        }

        let is = parse_indicator(&data[offset..])?;

        // Only handle GRIB edition 1 in this crate.
        if is.edition != 1 {
            offset += 1;
            continue;
        }

        let msg_end = offset + is.total_length as usize;
        if msg_end > data.len() {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} claims length {} but only {} bytes remain",
                is.total_length,
                data.len() - offset
            )));
        }

        // PDS starts immediately after the 8-byte IS.
        let pds_start = offset + 8;
        let pds = parse_product_definition(&data[pds_start..msg_end])?;

        messages.push(Grib1Message {
            message_index: messages.len(),
            byte_offset: offset,
            is,
            pds,
        });

        offset += msg_end - offset; // advance by total_length
    }

    Ok(messages)
}

/// Convert the PDS time unit + P1 to a number of forecast hours.
/// Uses WMO ON388 Table 4 time unit codes.
pub fn forecast_hours(pds: &ProductDefinition) -> i32 {
    let p1 = pds.p1 as i32;
    match pds.time_unit {
        0  => p1 / 60,           // minutes
        1  => p1,                // hours
        2  => p1 * 24,           // days
        10 => p1 * 3,            // 3-hour periods
        11 => p1 * 6,            // 6-hour periods
        12 => p1 * 12,           // 12-hour periods
        13 => (p1 as f64 / 3600.0).round() as i32, // seconds
        _  => p1,                // fall back to raw P1
    }
}

/// Format the PDS reference time as an ISO 8601 string.
pub fn reference_time(pds: &ProductDefinition) -> String {
    let year = (pds.century as i32 - 1) * 100 + pds.reference_year as i32;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:00Z",
        year,
        pds.reference_month,
        pds.reference_day,
        pds.reference_hour,
        pds.reference_minute,
    )
}

/// Combined 16-bit level value (level_value_1 << 8 | level_value_2).
/// Interpretation depends on level_type — see WMO ON388 Table 3.
pub fn level_value(pds: &ProductDefinition) -> f64 {
    ((pds.level_value_1 as u16) << 8 | pds.level_value_2 as u16) as f64
}
