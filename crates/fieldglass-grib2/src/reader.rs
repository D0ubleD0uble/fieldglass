use crate::is::{
    END_SECTION_LEN, GRIB2_EDITION, INDICATOR_SECTION_LEN, IndicatorSection, parse_indicator,
};
use fieldglass_core::FieldglassError;

/// Parsed metadata for a single GRIB2 message. For Phase 4.0 this only
/// surfaces the Indicator Section fields; later issues will populate
/// per-section parses (IDS, GDS, PDS, …).
#[derive(Debug, Clone, Copy)]
pub struct Grib2Message {
    /// Zero-based index of this message within the parent file.
    pub message_index: usize,
    /// Byte offset of the start of this message ("GRIB" magic) within the file.
    pub byte_offset: usize,
    /// Parsed Indicator Section (Section 0).
    pub is: IndicatorSection,
}

/// Top-level reader for a GRIB2 file. Owns the underlying bytes and a
/// per-message metadata vector populated by [`Grib2Reader::from_bytes`].
pub struct Grib2Reader {
    #[allow(dead_code)]
    data: Vec<u8>,
    pub messages: Vec<Grib2Message>,
}

impl Grib2Reader {
    /// Parse a GRIB2 file from raw bytes, scanning for all messages by
    /// walking IS total-length offsets. Mirrors the GRIB1 reader's
    /// boundary-walking shape; non-GRIB2 leading garbage is skipped one
    /// byte at a time until a `GRIB`-edition-2 marker is found.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, FieldglassError> {
        let messages = scan_messages(&data)?;
        Ok(Self { data, messages })
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }
}

fn scan_messages(data: &[u8]) -> Result<Vec<Grib2Message>, FieldglassError> {
    let mut messages = Vec::new();
    let mut offset = 0usize;

    while offset + INDICATOR_SECTION_LEN <= data.len() {
        // Search forward for the next GRIB marker.
        if &data[offset..offset + 4] != b"GRIB" {
            offset += 1;
            continue;
        }

        // Peek at the edition byte before fully parsing — a GRIB1 message
        // sharing the same magic shouldn't be a hard error here, just skipped.
        if data[offset + 7] != GRIB2_EDITION {
            offset += 1;
            continue;
        }

        let is = parse_indicator(&data[offset..])?;

        if is.total_length < INDICATOR_SECTION_LEN as u64 + END_SECTION_LEN as u64 {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} declares an impossibly small length {}",
                is.total_length
            )));
        }

        let msg_end_u64 = offset as u64 + is.total_length;
        if msg_end_u64 > data.len() as u64 {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} claims length {} but only {} bytes remain",
                is.total_length,
                data.len() - offset
            )));
        }
        let msg_end = msg_end_u64 as usize;

        // Trailing 4-byte End Section "7777".
        if &data[msg_end - END_SECTION_LEN..msg_end] != b"7777" {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} is missing trailing 7777 marker"
            )));
        }

        messages.push(Grib2Message {
            message_index: messages.len(),
            byte_offset: offset,
            is,
        });

        offset = msg_end; // advance to the next message
    }

    Ok(messages)
}
