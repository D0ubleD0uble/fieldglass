use crate::gds::{GDS_SECTION_NUMBER, GridDefinitionSection, parse_grid_definition_with_header};
use crate::ids::{IDS_SECTION_NUMBER, IdentificationSection, parse_identification_with_header};
use crate::is::{
    END_SECTION_LEN, GRIB2_EDITION, INDICATOR_SECTION_LEN, IndicatorSection, parse_indicator,
};
use crate::lus::{LUS_SECTION_NUMBER, parse_local_use_with_header};
use crate::pds::{
    PDS_SECTION_NUMBER, ProductDefinitionSection, parse_product_definition_with_header,
};
use crate::section::parse_section_header;
use fieldglass_core::FieldglassError;

/// Parsed metadata for a single GRIB2 message. Currently surfaces §0–§4;
/// §5–§7 are populated as later issues land.
#[derive(Debug, Clone)]
pub struct Grib2Message {
    /// Zero-based index of this message within the parent file.
    pub message_index: usize,
    /// Byte offset of the start of this message ("GRIB" magic) within the file.
    pub byte_offset: usize,
    /// Parsed Indicator Section (Section 0).
    pub is: IndicatorSection,
    /// Parsed Identification Section (Section 1) — required in every message.
    pub ids: IdentificationSection,
    /// Byte range of the Local Use Section (Section 2) within the file, if
    /// present. The section is optional per WMO spec.
    pub lus_range: Option<(usize, usize)>,
    /// Parsed Grid Definition Section (Section 3) — required by spec.
    pub gds: GridDefinitionSection,
    /// Parsed Product Definition Section (Section 4) — required by spec.
    pub pds: ProductDefinitionSection,
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

        // §1 IDS — always immediately follows §0. The earlier "impossibly
        // small length" guard ensures at least END_SECTION_LEN bytes follow
        // the IS, so a malformed-but-non-empty section header here will
        // surface from parse_section_header with a coherent error.
        let ids_offset = offset + INDICATOR_SECTION_LEN;
        let ids_header = parse_section_header(&data[ids_offset..msg_end])?;
        if ids_header.number != IDS_SECTION_NUMBER {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: expected IDS (section {IDS_SECTION_NUMBER}) \
                 immediately after IS, got section {}",
                ids_header.number
            )));
        }
        let ids = parse_identification_with_header(&data[ids_offset..msg_end], ids_header)?;
        let after_ids = ids_offset + ids_header.length as usize;

        // §2 LUS is optional; peek the next header and consume it only if it
        // claims to be section 2. Anything else (typically §3 GDS) is left
        // for the GDS step below.
        let mut cursor = after_ids;
        let lus_range = {
            let next = parse_section_header(&data[cursor..msg_end])?;
            if next.number == LUS_SECTION_NUMBER {
                let lus = parse_local_use_with_header(&data[cursor..msg_end], next)?;
                let end = cursor + lus.section_length as usize;
                let range = (cursor, end);
                cursor = end;
                Some(range)
            } else {
                None
            }
        };

        // §3 GDS — required by the WMO spec in every message.
        let gds_header = parse_section_header(&data[cursor..msg_end])?;
        if gds_header.number != GDS_SECTION_NUMBER {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: expected GDS (section {GDS_SECTION_NUMBER}), \
                 got section {}",
                gds_header.number
            )));
        }
        let gds = parse_grid_definition_with_header(&data[cursor..msg_end], gds_header)?;
        cursor += gds_header.length as usize;

        // §4 PDS — required by the WMO spec in every message.
        let pds_header = parse_section_header(&data[cursor..msg_end])?;
        if pds_header.number != PDS_SECTION_NUMBER {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: expected PDS (section {PDS_SECTION_NUMBER}), \
                 got section {}",
                pds_header.number
            )));
        }
        let pds = parse_product_definition_with_header(&data[cursor..msg_end], pds_header)?;

        messages.push(Grib2Message {
            message_index: messages.len(),
            byte_offset: offset,
            is,
            ids,
            lus_range,
            gds,
            pds,
        });

        offset = msg_end; // advance to the next message
    }

    Ok(messages)
}
