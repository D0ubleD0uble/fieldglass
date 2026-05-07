use fieldglass_core::FieldglassError;
use crate::bds::{decode_values, parse_bds_header};
use crate::bms::parse_bitmap;
use crate::gds::{parse_grid_description, GridDescription};
use crate::is::{parse_indicator, IndicatorSection};
use crate::pds::{parse_product_definition, ProductDefinition};

pub struct Grib1Message {
    pub message_index: usize,
    pub byte_offset: usize,
    pub is: IndicatorSection,
    pub pds: ProductDefinition,
    pub gds: Option<GridDescription>,
    /// Byte range of the Bit Map Section within the file, if one is present.
    pub bms_range: Option<(usize, usize)>,
    /// Byte range of the Binary Data Section within the file.
    pub bds_range: (usize, usize),
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

    /// Decode the grid values for one message. Returns one entry per grid
    /// point: `Some(value)` for present points, `None` for points masked out
    /// by a Bit Map Section.
    pub fn decode_message_values(
        &self,
        message_index: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let msg = self.messages.get(message_index)
            .ok_or(FieldglassError::OutOfRange)?;

        // GDS dimensions are required to know how many points to expect.
        let gds = msg.gds.as_ref().ok_or_else(|| FieldglassError::Parse(
            "message has no GDS — predefined grids are not supported".to_string()
        ))?;
        let (ni, nj) = gds.dimensions().ok_or_else(|| FieldglassError::Parse(
            "grid type has no declared dimensions".to_string()
        ))?;
        let expected_count = ni as usize * nj as usize;

        let bitmap = match msg.bms_range {
            Some((start, end)) => Some(parse_bitmap(&self.data[start..end], expected_count)?),
            None => None,
        };
        let bitmap_bits = bitmap.as_ref().map(|b| b.bits.as_slice());

        let (bds_start, bds_end) = msg.bds_range;
        let bds_bytes = &self.data[bds_start..bds_end];
        let header = parse_bds_header(bds_bytes)?;
        decode_values(
            bds_bytes,
            &header,
            msg.pds.decimal_scale_factor,
            bitmap_bits,
            expected_count,
        )
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

        // Trailing 4-byte End Section "7777".
        if &data[msg_end - 4..msg_end] != b"7777" {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} is missing trailing 7777 marker"
            )));
        }

        // PDS starts immediately after the 8-byte IS.
        let pds_start = offset + 8;
        let pds = parse_product_definition(&data[pds_start..msg_end])?;

        // GDS immediately follows the PDS when the has_gds flag is set.
        let mut cursor = pds_start + pds.section_len as usize;
        let gds = if pds.has_gds {
            if cursor >= msg_end {
                return Err(FieldglassError::Parse(
                    "PDS claims a GDS follows but no bytes remain".to_string()
                ));
            }
            let gds = parse_grid_description(&data[cursor..msg_end])?;
            // Advance the cursor by the GDS length.
            let gds_len = u32::from_be_bytes([0, data[cursor], data[cursor + 1], data[cursor + 2]]) as usize;
            cursor += gds_len;
            Some(gds)
        } else {
            None
        };

        // BMS, if present, immediately follows the GDS.
        let bms_range = if pds.has_bms {
            if cursor >= msg_end {
                return Err(FieldglassError::Parse(
                    "PDS claims a BMS follows but no bytes remain".to_string()
                ));
            }
            let bms_len = u32::from_be_bytes([0, data[cursor], data[cursor + 1], data[cursor + 2]]) as usize;
            let bms_end = cursor + bms_len;
            if bms_end > msg_end {
                return Err(FieldglassError::Parse(
                    "BMS extends past end of message".to_string()
                ));
            }
            let range = (cursor, bms_end);
            cursor = bms_end;
            Some(range)
        } else {
            None
        };

        // BDS occupies everything from `cursor` up to the End Section.
        let bds_end = msg_end - 4;
        if cursor >= bds_end {
            return Err(FieldglassError::Parse(format!(
                "No BDS bytes between section cursor {cursor} and ES at {bds_end}"
            )));
        }
        let bds_range = (cursor, bds_end);

        messages.push(Grib1Message {
            message_index: messages.len(),
            byte_offset: offset,
            is,
            pds,
            gds,
            bms_range,
            bds_range,
        });

        offset = msg_end; // advance to the next message
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
