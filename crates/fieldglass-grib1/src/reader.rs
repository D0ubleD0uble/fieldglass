use crate::bds::{BdsHeader, decode_values, parse_bds_header};
use crate::bms::{Bitmap, parse_bitmap};
use crate::gds::{GridDescription, parse_grid_description};
use crate::is::{IndicatorSection, parse_indicator};
use crate::packing::matrix::decode_matrix_of_values;
use crate::pds::{ProductDefinition, parse_product_definition};
use fieldglass_core::FieldglassError;

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

/// A decoded matrix-of-values field (`grid_simple_matrix` with
/// `matrixOfValues = 1`): an `nr × nc` matrix at every grid point rather than a
/// single value. `values` is `ni · nj · nr · nc` long, grid-point major in scan
/// order with each point's `nr·nc` matrix cells stored consecutively; `None`
/// marks a bitmap-masked cell or grid point. Such a field is not a single
/// renderable 2-D panel, which is why it has its own decode entry rather than
/// flowing through [`Grib1Reader::decode_message_values`].
pub struct MatrixField {
    /// Grid columns (points per row).
    pub ni: usize,
    /// Grid rows.
    pub nj: usize,
    /// First matrix dimension.
    pub nr: usize,
    /// Second matrix dimension.
    pub nc: usize,
    /// Flattened values, length `ni·nj·nr·nc`.
    pub values: Vec<Option<f64>>,
}

/// Byte index of the PDS `p1` (forecast period) octet within a GRIB1 message,
/// counted from the start of the IS. The IS is 8 bytes; `p1` sits at PDS
/// offset 18 (octet 19 in 1-indexed WMO terminology).
pub const PDS_P1_OFFSET_FROM_MESSAGE_START: usize = 8 + 18;

impl Grib1Message {
    /// Absolute file-byte offset of the PDS `p1` octet for this message.
    pub fn pds_p1_offset(&self) -> usize {
        self.byte_offset + PDS_P1_OFFSET_FROM_MESSAGE_START
    }
}

/// Hard cap on `ni * nj` for `decode_message_values`. Real grids top out
/// around 25M points; the cap bounds the worst-case `Vec<Option<f64>>`
/// allocation at ~1 GB (16 bytes/element).
pub const MAX_GRID_POINTS: usize = 64 * 1024 * 1024;

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

    /// eccodes-style `packingType` label for one message's BDS, or `None` if
    /// the index is out of range or the BDS header can't be parsed. This is
    /// metadata only — it parses the 11-byte BDS header and never decodes
    /// values, so it stays cheap to call for every message in a file.
    pub fn packing_label(&self, message_index: usize) -> Option<&'static str> {
        let (start, end) = self.messages.get(message_index)?.bds_range;
        let bytes = self.data.get(start..end)?;
        parse_bds_header(bytes)
            .ok()
            .map(|header| header.packing_type_label())
    }

    /// Shared decode preamble for [`Self::decode_message_values`] and
    /// [`Self::decode_matrix_message`]: resolve the message, its grid
    /// dimensions and point count (overflow- and `MAX_GRID_POINTS`-checked),
    /// parse the BMS bitmap if present, and parse the BDS header. Returns the
    /// borrowed BDS bytes alongside the owned bitmap so the caller can hand a
    /// `&[bool]` slice to the packing decoder.
    fn decode_inputs(&self, message_index: usize) -> Result<DecodeInputs<'_>, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or(FieldglassError::OutOfRange)?;

        let gds = msg.gds.as_ref().ok_or_else(|| {
            FieldglassError::Parse(
                "message has no GDS — predefined grids are not supported".to_string(),
            )
        })?;
        let (ni, nj) = gds.dimensions().ok_or_else(|| {
            FieldglassError::Parse("grid type has no declared dimensions".to_string())
        })?;
        // checked_mul guards 32-bit usize overflow; MAX_GRID_POINTS guards OOM.
        let expected_count = (ni as usize).checked_mul(nj as usize).ok_or_else(|| {
            FieldglassError::Parse(format!("grid dimensions {ni}×{nj} overflow usize"))
        })?;
        if expected_count > MAX_GRID_POINTS {
            return Err(FieldglassError::Parse(format!(
                "grid {ni}×{nj} = {expected_count} points exceeds cap of {MAX_GRID_POINTS}"
            )));
        }

        let bitmap = match msg.bms_range {
            Some((start, end)) => Some(parse_bitmap(&self.data[start..end], expected_count)?),
            None => None,
        };

        let (bds_start, bds_end) = msg.bds_range;
        let bds_bytes = &self.data[bds_start..bds_end];
        let header = parse_bds_header(bds_bytes)?;

        Ok(DecodeInputs {
            ni: ni as usize,
            nj: nj as usize,
            expected_count,
            decimal_scale: msg.pds.decimal_scale_factor,
            bitmap,
            bds_bytes,
            header,
        })
    }

    /// Decode the grid values for one message. Returns one entry per grid
    /// point: `Some(value)` for present points, `None` for points masked out
    /// by a Bit Map Section.
    pub fn decode_message_values(
        &self,
        message_index: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let inputs = self.decode_inputs(message_index)?;
        decode_values(
            inputs.bds_bytes,
            &inputs.header,
            inputs.decimal_scale,
            inputs.bitmap_bits(),
            inputs.expected_count,
            inputs.ni,
        )
    }

    /// Decode a `grid_simple_matrix` message that carries an `nr × nc` matrix at
    /// every grid point (`matrixOfValues = 1`). Returns a [`MatrixField`] whose
    /// `values` is `ni·nj·nr·nc` long. Use [`Grib1Reader::decode_message_values`]
    /// for scalar fields — this errors if the message is not a true
    /// matrix-of-values field.
    pub fn decode_matrix_message(
        &self,
        message_index: usize,
    ) -> Result<MatrixField, FieldglassError> {
        let inputs = self.decode_inputs(message_index)?;
        let matrix = decode_matrix_of_values(
            inputs.bds_bytes,
            &inputs.header,
            inputs.decimal_scale,
            inputs.bitmap_bits(),
            inputs.expected_count,
        )?;
        Ok(MatrixField {
            ni: inputs.ni,
            nj: inputs.nj,
            nr: matrix.nr,
            nc: matrix.nc,
            values: matrix.values,
        })
    }
}

/// Inputs resolved by [`Grib1Reader::decode_inputs`] and shared by both decode
/// entry points. Borrows the BDS bytes from the reader; owns the parsed bitmap
/// and BDS header.
struct DecodeInputs<'a> {
    ni: usize,
    nj: usize,
    expected_count: usize,
    decimal_scale: i16,
    bitmap: Option<Bitmap>,
    bds_bytes: &'a [u8],
    header: BdsHeader,
}

impl DecodeInputs<'_> {
    /// The per-point presence bits, if a Bit Map Section was present.
    fn bitmap_bits(&self) -> Option<&[bool]> {
        self.bitmap.as_ref().map(|b| b.bits.as_slice())
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
                    "PDS claims a GDS follows but no bytes remain".to_string(),
                ));
            }
            let gds = parse_grid_description(&data[cursor..msg_end])?;
            // Advance the cursor by the GDS length.
            let gds_len =
                u32::from_be_bytes([0, data[cursor], data[cursor + 1], data[cursor + 2]]) as usize;
            cursor += gds_len;
            Some(gds)
        } else {
            None
        };

        // BMS, if present, immediately follows the GDS.
        let bms_range = if pds.has_bms {
            if cursor >= msg_end {
                return Err(FieldglassError::Parse(
                    "PDS claims a BMS follows but no bytes remain".to_string(),
                ));
            }
            let bms_len =
                u32::from_be_bytes([0, data[cursor], data[cursor + 1], data[cursor + 2]]) as usize;
            let bms_end = cursor + bms_len;
            if bms_end > msg_end {
                return Err(FieldglassError::Parse(
                    "BMS extends past end of message".to_string(),
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
/// Uses WMO ON388 Table 4 time unit codes. Ignores `time_range` — for a
/// user-facing string that handles ranges/accumulations use [`forecast_display`].
pub fn forecast_hours(pds: &ProductDefinition) -> i32 {
    p1_to_hours(pds.time_unit, pds.p1 as i32)
}

fn p1_to_hours(time_unit: u8, p: i32) -> i32 {
    match time_unit {
        0 => p / 60,
        1 => p,
        2 => p * 24,
        10 => p * 3,
        11 => p * 6,
        12 => p * 12,
        13 => (p as f64 / 3600.0).round() as i32,
        _ => p,
    }
}

/// Format the PDS time information for display, branching on the time-range
/// indicator (WMO ON388 Table 5) so accumulations and ranges show both bounds
/// rather than collapsing to P1 only.
pub fn forecast_display(pds: &ProductDefinition) -> String {
    let p1 = pds.p1 as i32;
    let p2 = pds.p2 as i32;
    let h1 = p1_to_hours(pds.time_unit, p1);
    let h2 = p1_to_hours(pds.time_unit, p2);

    match pds.time_range {
        0 => format!("+{h1}h"),
        1 => "analysis".to_string(),
        2 => format!("{h1}–{h2}h valid"),
        3 => format!("{h1}–{h2}h average"),
        4 => format!("{h1}–{h2}h accum"),
        5 => format!("{h1}–{h2}h diff"),
        6 => format!("−{h1} to −{h2}h average"),
        7 => format!("−{h1} to +{h2}h average"),
        // P1 occupies both P1 and P2 octets as a single 16-bit value.
        10 => {
            let combined = ((pds.p1 as u16) << 8 | pds.p2 as u16) as i32;
            format!("+{}h", p1_to_hours(pds.time_unit, combined))
        }
        51 => "climatological mean".to_string(),
        _ => format!("+{h1}h"),
    }
}

/// Format the PDS reference time as an ISO 8601 string.
pub fn reference_time(pds: &ProductDefinition) -> String {
    let year = (pds.century as i32 - 1) * 100 + pds.reference_year as i32;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:00Z",
        year, pds.reference_month, pds.reference_day, pds.reference_hour, pds.reference_minute,
    )
}

/// Combined 16-bit level value (level_value_1 << 8 | level_value_2).
/// Only meaningful for level types whose value is encoded as a single 16-bit
/// integer (e.g. 100 isobaric). For layer types (101, 104, 106, …) the two
/// bytes are independent bounds and this function returns nonsense — callers
/// that want a user-facing string should use [`level_value_str`].
pub fn level_value(pds: &ProductDefinition) -> f64 {
    ((pds.level_value_1 as u16) << 8 | pds.level_value_2 as u16) as f64
}

/// Unit string for a given WMO ON388 Table 3 level type, if one applies to
/// the level value encoded in the PDS. Returns `None` for surface / fixed
/// levels where the value byte is meaningless and for level types whose
/// "value" is a dimensionless index (model level, NAM level).
pub fn level_unit(level_type: u8) -> Option<&'static str> {
    match level_type {
        // Single-value types with direct units.
        100 | 115 | 116 | 121 | 141 => Some("hPa"),
        103 | 105 | 160 => Some("m"),
        111 | 112 | 125 => Some("cm"),
        113 | 114 => Some("K"),
        126 => Some("Pa"),
        117 => Some("PVU"),
        107 | 108 | 128 => Some("σ"),
        // Layer types whose bounds are in their own units.
        101 => Some("kPa"),
        104 | 106 => Some("hm"),
        // Dimensionless / surface / index types.
        _ => None,
    }
}

/// Format the PDS level value (without unit) for display. Returns `"—"` for
/// fixed-surface / whole-column types where the value is meaningless, a
/// `"<lo> – <hi>"` range for layer types, and a scalar otherwise. The unit
/// belongs in the level-type column — see [`level_unit`].
pub fn level_value_str(pds: &ProductDefinition) -> String {
    let lv1 = pds.level_value_1 as i32;
    let lv2 = pds.level_value_2 as i32;
    let combined = ((pds.level_value_1 as u16) << 8 | pds.level_value_2 as u16) as i32;

    match pds.level_type {
        // Fixed surfaces / whole-column types: value byte is meaningless.
        1
        | 2
        | 3
        | 4
        | 5
        | 6
        | 7
        | 8
        | 9
        | 102
        | 200
        | 201
        | 204
        | 205
        | 209
        | 210..=221
        | 241
        | 242 => "—".to_string(),

        // Single 16-bit value, integer.
        100 | 103 | 105 | 111 | 113 | 115 | 126 | 160 => format!("{combined}"),

        // Single value with scaling.
        125 => format!("{:.2}", combined as f64 / 100.0),
        107 => format!("{:.4}", combined as f64 / 10000.0),
        117 => format!("{:.3}", combined as f64 / 1000.0),

        // Index-only level numbers.
        109 => format!("{combined}"),
        119 => format!("{combined}"),

        // Layer types: lv1 / lv2 are independent bounds.
        101 | 104 | 106 | 110 | 112 | 116 | 120 => format!("{lv1} – {lv2}"),
        108 => format!("{:.2} – {:.2}", lv1 as f64 / 100.0, lv2 as f64 / 100.0),
        114 => format!("{} – {}", 475 - lv1, 475 - lv2),
        121 => format!("{} – {}", 1100 - lv1, 1100 - lv2),
        128 => format!(
            "{:.3} – {:.3}",
            1.1 - lv1 as f64 * 0.001,
            1.1 - lv2 as f64 * 0.001
        ),
        141 => format!("{lv1} – {}", 1100 - lv2),

        _ => format!("{combined}"),
    }
}

/// Format the level type as `"(<unit>) <name>"` when a unit applies, or just
/// `"<name>"` for surface / dimensionless types. The unit prefix lets the row
/// read naturally with the value column to its left, e.g. `200 (hPa) Isobaric
/// level`.
pub fn level_type_str(pds: &ProductDefinition) -> String {
    let name = crate::tables::lookup_level_type(pds.level_type);
    match level_unit(pds.level_type) {
        Some(unit) => format!("({unit}) {name}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod level_display_tests {
    use super::*;

    fn pds(level_type: u8, lv1: u8, lv2: u8) -> ProductDefinition {
        ProductDefinition {
            section_len: 28,
            table_version: 2,
            originating_centre: 98,
            generating_process: 0,
            grid_number: 255,
            has_gds: true,
            has_bms: false,
            parameter_id: 0,
            level_type,
            level_value_1: lv1,
            level_value_2: lv2,
            reference_year: 0,
            reference_month: 1,
            reference_day: 1,
            reference_hour: 0,
            reference_minute: 0,
            time_unit: 1,
            p1: 0,
            p2: 0,
            time_range: 0,
            century: 21,
            sub_centre: 0,
            decimal_scale_factor: 0,
        }
    }

    #[test]
    fn isobaric_300_value_only() {
        // 300 = 1*256 + 44
        let p = pds(100, 1, 44);
        assert_eq!(level_value_str(&p), "300");
        assert_eq!(level_type_str(&p), "(hPa) Isobaric level");
    }

    #[test]
    fn isobaric_50_low_byte() {
        let p = pds(100, 0, 50);
        assert_eq!(level_value_str(&p), "50");
        assert_eq!(level_type_str(&p), "(hPa) Isobaric level");
    }

    #[test]
    fn cloud_base_level_has_no_value_and_no_unit() {
        let p = pds(1, 0, 0);
        assert_eq!(level_value_str(&p), "—");
        assert_eq!(level_type_str(&p), "Cloud base level");
    }

    #[test]
    fn height_above_ground_2m() {
        let p = pds(105, 0, 2);
        assert_eq!(level_value_str(&p), "2");
        assert_eq!(level_type_str(&p), "(m) Specified height above ground");
    }

    #[test]
    fn isobaric_layer_uses_two_bounds() {
        // Layer between 100 kPa and 85 kPa (1000 hPa – 850 hPa).
        let p = pds(101, 100, 85);
        assert_eq!(level_value_str(&p), "100 – 85");
        assert_eq!(
            level_type_str(&p),
            "(kPa) Layer between two isobaric levels"
        );
    }

    #[test]
    fn potential_vorticity_2_pvu() {
        let p = pds(117, 7, 208);
        assert_eq!(level_value_str(&p), "2.000");
        assert_eq!(level_type_str(&p), "(PVU) Potential vorticity surface");
    }

    #[test]
    fn unknown_level_type_falls_back_to_raw_with_no_unit() {
        let p = pds(250, 1, 0);
        assert_eq!(level_value_str(&p), "256");
        assert_eq!(level_type_str(&p), "Unknown level type");
    }
}

#[cfg(test)]
mod forecast_display_tests {
    use super::*;

    fn pds_time(time_unit: u8, time_range: u8, p1: u8, p2: u8) -> ProductDefinition {
        ProductDefinition {
            section_len: 28,
            table_version: 2,
            originating_centre: 98,
            generating_process: 0,
            grid_number: 255,
            has_gds: true,
            has_bms: false,
            parameter_id: 0,
            level_type: 1,
            level_value_1: 0,
            level_value_2: 0,
            reference_year: 0,
            reference_month: 1,
            reference_day: 1,
            reference_hour: 0,
            reference_minute: 0,
            time_unit,
            p1,
            p2,
            time_range,
            century: 21,
            sub_centre: 0,
            decimal_scale_factor: 0,
        }
    }

    #[test]
    fn plain_forecast_at_p1() {
        // hours unit, tr=0 (forecast at ref+P1), p1=24
        assert_eq!(forecast_display(&pds_time(1, 0, 24, 0)), "+24h");
    }

    #[test]
    fn analysis_renders_word() {
        assert_eq!(forecast_display(&pds_time(1, 1, 0, 0)), "analysis");
    }

    #[test]
    fn accumulation_shows_both_bounds() {
        // tr=4 accumulation, p1=0 to p2=24h
        assert_eq!(forecast_display(&pds_time(1, 4, 0, 24)), "0–24h accum");
    }

    #[test]
    fn average_range_shows_both_bounds() {
        assert_eq!(forecast_display(&pds_time(1, 3, 6, 12)), "6–12h average");
    }

    #[test]
    fn time_unit_days_converts_to_hours() {
        // unit=2 (days), p1=1 → 24h
        assert_eq!(forecast_display(&pds_time(2, 0, 1, 0)), "+24h");
    }
}
