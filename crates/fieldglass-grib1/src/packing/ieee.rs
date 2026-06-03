//! IEEE 754 raw float packing — `grid_ieee`.
//!
//! Selected by BDS octet-4 flag bits `complexPacking = 0`,
//! `integerPointValues = 1`, `additionalFlagPresent = 1` (see
//! [`crate::packing::decoder_for`]). The WMO Guide to GRIB (WMO No. 306
//! Vol I.2, FM 92) reserved IEEE storage "for a later edition"; in practice it
//! is the ECMWF octet-4-bit-4 extension implemented by eccodes'
//! `grib1/data.grid_ieee.def` + `DataRawPacking`.
//!
//! Layout after the standard 11-octet BDS header: octet 11 (`bytes[10]`) is
//! `bitsPerValue`, octet 12 (`bytes[11]`) is the `precision` code-table value
//! (1 = 32-bit, 2 = 64-bit, 3 = 128-bit), and the value stream begins at
//! octet 13 (`bytes[12]`). Values are stored verbatim as big-endian IEEE
//! floats with **no** reference / binary-scale / decimal-scale transform —
//! eccodes applies only the bitmap, never the simple-packing affine map. We
//! match eccodes and reject precision 3 (128-bit), which `DataRawPacking`
//! itself returns `GRIB_NOT_IMPLEMENTED` for.

use fieldglass_core::FieldglassError;

use crate::bds::BdsHeader;

use super::{Grib1Packing, interleave_with_bitmap, present_count};

/// Byte offset (within the BDS) of the `precision` code-table octet.
const PRECISION_OFFSET: usize = 11;
/// Byte offset (within the BDS) at which the raw IEEE value stream begins.
const IEEE_DATA_OFFSET: usize = 12;

pub struct IeeePacking;

impl Grib1Packing for IeeePacking {
    fn decode(
        &self,
        bds: &[u8],
        header: &BdsHeader,
        _decimal_scale: i16,
        bitmap: Option<&[bool]>,
        expected_count: usize,
        _cols: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let section_len = header.section_len as usize;
        if bds.len() < section_len || section_len < IEEE_DATA_OFFSET {
            return Err(FieldglassError::Parse(format!(
                "grid_ieee BDS shorter than declared section_len {section_len}"
            )));
        }

        let precision = bds[PRECISION_OFFSET];
        let width = match precision {
            1 => 4, // IEEE 32-bit
            2 => 8, // IEEE 64-bit
            other => {
                return Err(FieldglassError::UnsupportedSection(format!(
                    "BDS uses grid_ieee raw packing with precision {other} \
                     (code-table 5.7); only 32-bit (1) and 64-bit (2) are \
                     supported — 128-bit is unimplemented in eccodes too."
                )));
            }
        };

        let data = &bds[IEEE_DATA_OFFSET..section_len];
        let stored_count = data.len() / width;
        let present = present_count(bitmap, expected_count);
        if stored_count < present {
            return Err(FieldglassError::Parse(format!(
                "grid_ieee holds {stored_count} values but {present} are required"
            )));
        }

        let mut decoded = Vec::with_capacity(present);
        for chunk in data.chunks_exact(width).take(present) {
            let value = match width {
                4 => f32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as f64,
                _ => f64::from_be_bytes([
                    chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
                ]),
            };
            decoded.push(value);
        }

        Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
    }
}
