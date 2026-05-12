use crate::ids::{IDS_SECTION_NUMBER, IdentificationSection, parse_identification_with_header};
use crate::is::{
    END_SECTION_LEN, GRIB2_EDITION, INDICATOR_SECTION_LEN, IndicatorSection, parse_indicator,
};
use crate::lus::{LUS_SECTION_NUMBER, parse_local_use_with_header};
use crate::section::parse_section_header;
use fieldglass_core::FieldglassError;

/// Parsed metadata for a single GRIB2 message. Currently surfaces §0–§2;
/// §3–§7 are populated as later issues land.
#[derive(Debug, Clone, Copy)]
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

        // §1 IDS — always immediately follows §0.
        let ids_offset = offset + INDICATOR_SECTION_LEN;
        if ids_offset >= msg_end {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} is too short to contain an IDS"
            )));
        }
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
        // claims to be section 2. Anything else (§3 GDS, §7 DS, …) is left
        // for later issues to walk.
        let lus_range = if after_ids + crate::section::SECTION_HEADER_LEN <= msg_end {
            let next = parse_section_header(&data[after_ids..msg_end])?;
            if next.number == LUS_SECTION_NUMBER {
                let lus = parse_local_use_with_header(&data[after_ids..msg_end], next)?;
                Some((after_ids, after_ids + lus.section_length as usize))
            } else {
                None
            }
        } else {
            None
        };

        messages.push(Grib2Message {
            message_index: messages.len(),
            byte_offset: offset,
            is,
            ids,
            lus_range,
        });

        offset = msg_end; // advance to the next message
    }

    Ok(messages)
}
