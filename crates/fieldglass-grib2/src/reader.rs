use crate::bms::{BMS_SECTION_NUMBER, parse_bit_map_with_header};
use crate::drs::{
    DRS_SECTION_NUMBER, DataRepresentationSection, parse_data_representation_with_header,
};
use crate::ds::{
    DS_SECTION_NUMBER, decode_values, parse_data_section_body, undo_second_order_boustrophedonic,
};
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
use crate::spectral::{
    BiFourierCoefficients, SpectralCoefficients, decode_bifourier, decode_spectral_complex,
    decode_spectral_simple,
};
use fieldglass_core::FieldglassError;

/// Hard cap on `ni · nj` for `decode_message_values`. Real grids top out
/// around 10⁷ points; this guards against pathological inputs that would
/// otherwise allocate gigabytes. Matches the GRIB1 reader's cap.
const MAX_GRID_POINTS: usize = 200_000_000;

/// Parsed metadata for a single GRIB2 message. Surfaces §0–§5 inline (the
/// fixed-size fields); §6 (BMS) and §7 (DS) live behind byte ranges so the
/// reader doesn't eagerly decode payloads.
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
    /// Parsed Data Representation Section (Section 5) — required by spec.
    pub drs: DataRepresentationSection,
    /// Byte range of the Bit-Map Section (Section 6) within the file.
    /// Required by spec; presence of an inline bitmap is signalled by §6's
    /// own indicator byte (0=inline, 255=none).
    pub bms_range: (usize, usize),
    /// Byte range of the Data Section (Section 7) within the file.
    pub ds_range: (usize, usize),
}

/// A decoded `grid_simple_matrix` field (template 5.1, `matrixBitmapsPresent =
/// 1`): an `NR × NC` matrix at every grid point. `values` is `ni·nj·nr·nc` long,
/// grid-point major (scan order) with each point's `nr·nc` matrix cells stored
/// consecutively; `None` marks a bitmap-masked cell or absent grid point.
/// Mirrors GRIB1's `MatrixField`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixField {
    /// Grid width `Ni`.
    pub ni: usize,
    /// Grid height `Nj`.
    pub nj: usize,
    /// First matrix dimension.
    pub nr: usize,
    /// Second matrix dimension.
    pub nc: usize,
    /// Flattened matrix values — see the type docs for the layout.
    pub values: Vec<Option<f64>>,
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

    /// Decode the grid values for one message, mirroring the GRIB1 reader's
    /// API. Returns one entry per grid point: `Some(value)` for present
    /// points, `None` for points masked out by the §6 bitmap or substituted
    /// as missing by §5 missing-value management.
    ///
    /// Currently supports DRS templates 5.0 (simple packing), 5.2 / 5.3
    /// (complex packing, with and without spatial differencing — both
    /// splitting methods, inline missing-value management 0/1/2), 5.4 (IEEE
    /// floating point), 5.40 (JPEG 2000 packing), 5.41 (PNG packing), 5.42
    /// (CCSDS / AEC packing), 5.61 (simple packing with logarithmic
    /// pre-processing), and 5.200 (run-length packing). Other packing templates
    /// return [`FieldglassError::UnsupportedSection`].
    pub fn decode_message_values(
        &self,
        message_index: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or(FieldglassError::OutOfRange)?;

        // Spherical-harmonic messages carry coefficients, not a grid, so they
        // have no dimensions and decode through `decode_spectral_message`.
        if msg.gds.spherical_harmonic().is_some() {
            return Err(FieldglassError::UnsupportedSection(
                "message holds spherical-harmonic coefficients (§3.50), which are not values \
                 on a grid — decode them with `Grib2Reader::decode_spectral_message`"
                    .to_string(),
            ));
        }

        // Bi-Fourier messages likewise carry coefficients, not a grid.
        if msg.gds.bifourier().is_some() {
            return Err(FieldglassError::UnsupportedSection(
                "message holds bi-Fourier spectral coefficients (§3.61/62/63), which are not \
                 values on a grid — decode them with `Grib2Reader::decode_bifourier_message`"
                    .to_string(),
            ));
        }

        let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
            FieldglassError::Parse(
                "grid template has no declared dimensions — reduced grids \
                 are not yet supported for decode"
                    .to_string(),
            )
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
        // The grid geometry (ni×nj) must agree with the point count the GDS
        // declares for itself (§3 octets 7–10). A mismatch means the grid
        // template and the section's own count disagree — a malformed message.
        // Without this, a corrupted ni/nj can name a hundred-million-point grid
        // (still under MAX_GRID_POINTS) whose constant-field decode then
        // allocates gigabytes, even though the file carries no such data — an
        // OOM found by the decode fuzz target. (Reduced grids return `None`
        // from dimensions() above and never reach here.)
        if expected_count != msg.gds.num_data_points as usize {
            return Err(FieldglassError::Parse(format!(
                "grid dimensions {ni}×{nj} = {expected_count} points disagree with the \
                 GDS-declared {} data points",
                msg.gds.num_data_points
            )));
        }

        // §6 BMS — decode the bitmap once (or skip it when indicator == 255).
        let (bms_start, bms_end) = msg.bms_range;
        let bms_header = parse_section_header(&self.data[bms_start..bms_end])?;
        let bms =
            parse_bit_map_with_header(&self.data[bms_start..bms_end], bms_header, expected_count)?;
        let bitmap = if bms.has_inline_bitmap() {
            Some(bms.bitmap.as_slice())
        } else {
            None
        };

        // §7 DS — strip the section header, hand the packed bytes to the
        // packing decoder selected by §5.
        let (ds_start, ds_end) = msg.ds_range;
        let ds_header = parse_section_header(&self.data[ds_start..ds_end])?;
        let ds_payload = parse_data_section_body(&self.data[ds_start..ds_end], ds_header)?;
        // The DRS template is small for every packing except run-length, whose
        // level table is heap-allocated; `decode_message_values` runs once per
        // message render (not per point), so the clone is not on any hot path.
        let mut values =
            decode_values(ds_payload, msg.drs.template.clone(), bitmap, expected_count)?;
        // Second-order packing (template 5.50002) may store alternating rows
        // right-to-left; undo it now that the grid width Ni is known. A no-op
        // for every other template and for 5.50001.
        undo_second_order_boustrophedonic(&mut values, &msg.drs.template, ni as usize);
        Ok(values)
    }

    /// Decode a `grid_simple_matrix` message (template 5.1) that carries an
    /// `NR × NC` matrix at every grid point (`matrixBitmapsPresent = 1`).
    ///
    /// Returns a [`MatrixField`] whose `values` is `Ni·Nj·NR·NC` long, grid-point
    /// major with each point's `NR·NC` matrix cells consecutive; `None` marks a
    /// bitmap-masked cell or absent grid point. Use
    /// [`decode_message_values`](Self::decode_message_values) for the flat
    /// `matrixBitmapsPresent = 0` form — this errors on it. eccodes cannot decode
    /// this variant (it crashes), so it follows the GRIBEX interpretation the
    /// GRIB1 matrix path uses; see [`crate::matrix`].
    pub fn decode_matrix_message(
        &self,
        message_index: usize,
    ) -> Result<MatrixField, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or(FieldglassError::OutOfRange)?;
        let t = msg.drs.matrix_simple().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "message {message_index} uses §5 packing {}, not grid_simple_matrix (5.1)",
                msg.drs.template_name()
            ))
        })?;
        if t.matrix_bitmaps_present == 0 {
            return Err(FieldglassError::UnsupportedSection(
                "grid_simple_matrix message has matrixBitmapsPresent = 0 (a scalar field); \
                 decode it with `decode_message_values`, not as a matrix."
                    .to_string(),
            ));
        }

        let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
            FieldglassError::Parse("matrix message has no declared grid dimensions".to_string())
        })?;
        let expected_count = (ni as usize).checked_mul(nj as usize).ok_or_else(|| {
            FieldglassError::Parse(format!("grid dimensions {ni}×{nj} overflow usize"))
        })?;
        if expected_count > MAX_GRID_POINTS {
            return Err(FieldglassError::Parse(format!(
                "grid {ni}×{nj} = {expected_count} points exceeds cap of {MAX_GRID_POINTS}"
            )));
        }
        // Keep the ni×nj geometry and the GDS-declared point count in agreement,
        // like the scalar path — a corrupted ni/nj naming a huge grid unbacked by
        // data is rejected here rather than driving a large allocation.
        if expected_count != msg.gds.num_data_points as usize {
            return Err(FieldglassError::Parse(format!(
                "grid dimensions {ni}×{nj} = {expected_count} points disagree with the \
                 GDS-declared {} data points",
                msg.gds.num_data_points
            )));
        }

        let (bms_start, bms_end) = msg.bms_range;
        let bms_header = parse_section_header(&self.data[bms_start..bms_end])?;
        let bms =
            parse_bit_map_with_header(&self.data[bms_start..bms_end], bms_header, expected_count)?;
        let bitmap = if bms.has_inline_bitmap() {
            Some(bms.bitmap.as_slice())
        } else {
            None
        };

        let (ds_start, ds_end) = msg.ds_range;
        let ds_header = parse_section_header(&self.data[ds_start..ds_end])?;
        let ds_payload = parse_data_section_body(&self.data[ds_start..ds_end], ds_header)?;
        let values = crate::matrix::decode_matrix_of_values(ds_payload, t, bitmap, expected_count)?;
        Ok(MatrixField {
            ni: ni as usize,
            nj: nj as usize,
            nr: t.nr as usize,
            nc: t.nc as usize,
            values,
        })
    }

    /// Decode a spherical-harmonic message (§3.50 + §5.50) into its spectral
    /// coefficients.
    ///
    /// A spectral message stores the field in wavenumber space, not on a grid,
    /// so it has no `Ni`/`Nj` and cannot go through
    /// [`Grib2Reader::decode_message_values`]. Turning the coefficients back
    /// into a grid needs an inverse spherical-harmonic transform, which is not
    /// implemented yet; what you get here is what eccodes' `grib_get_data`
    /// prints for the same message. Errors if the message is not
    /// spherical-harmonic, or its §5 packing is not one the spectral decoder
    /// supports (only `spectral_simple` / template 5.50 today).
    pub fn decode_spectral_message(
        &self,
        message_index: usize,
    ) -> Result<SpectralCoefficients, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or(FieldglassError::OutOfRange)?;

        let sh = msg.gds.spherical_harmonic().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "message {message_index} is a {} grid, not spherical-harmonic coefficients — \
                 use `decode_message_values`",
                msg.gds.template_name()
            ))
        })?;

        let (ds_start, ds_end) = msg.ds_range;
        let ds_header = parse_section_header(&self.data[ds_start..ds_end])?;
        let ds_payload = parse_data_section_body(&self.data[ds_start..ds_end], ds_header)?;

        if let Some(t) = msg.drs.spectral_simple() {
            decode_spectral_simple(ds_payload, t, sh.j, sh.k, sh.m)
        } else if let Some(t) = msg.drs.spectral_complex() {
            decode_spectral_complex(ds_payload, t, sh.j, sh.k, sh.m)
        } else {
            Err(FieldglassError::UnsupportedSection(format!(
                "spherical-harmonic message {message_index} uses §5 packing {} — only \
                 spectral_simple (5.50) and spectral_complex (5.51) decode today",
                msg.drs.template_name()
            )))
        }
    }

    /// Synthesize a spherical-harmonic message onto a regular lat/lon grid via
    /// the inverse spherical-harmonic transform.
    ///
    /// Decodes the coefficients (see
    /// [`decode_spectral_message`](Self::decode_spectral_message)) and evaluates
    /// the field at every `(latitude, longitude)` in `latitudes_deg` ×
    /// `longitudes_deg`, returning `latitudes_deg.len() · longitudes_deg.len()`
    /// values in latitude-major scan order. This is the transform no other tool
    /// in the ecosystem performs, letting a spectral message be turned back into
    /// a grid for rendering. The numerics are validated against ECMWF's
    /// definitive spectral definition (see [`fieldglass_core::sht`]).
    pub fn synthesize_spectral_message(
        &self,
        message_index: usize,
        latitudes_deg: &[f64],
        longitudes_deg: &[f64],
    ) -> Result<Vec<f64>, FieldglassError> {
        let coeffs = self.decode_spectral_message(message_index)?;
        fieldglass_core::sht::synthesize_spherical_harmonic(
            &coeffs.coefficients,
            coeffs.j,
            latitudes_deg,
            longitudes_deg,
        )
    }

    /// Decode a bi-Fourier message (§3.61/62/63 + §5.53) into its spectral
    /// coefficients.
    ///
    /// Like [`decode_spectral_message`](Self::decode_spectral_message), a
    /// bi-Fourier message stores the field as spectral coefficients (four per
    /// `(i, j)` wavenumber pair), not on a grid, so it has no `Ni`/`Nj` and
    /// cannot go through [`decode_message_values`](Self::decode_message_values).
    /// Recovering a grid needs an inverse bi-Fourier transform, which is not
    /// implemented yet; what you get here is what eccodes' `grib_get_data`
    /// prints for the same message. Errors if the message is not bi-Fourier, or
    /// its §5 packing is not `bifourier_complex` (template 5.53).
    pub fn decode_bifourier_message(
        &self,
        message_index: usize,
    ) -> Result<BiFourierCoefficients, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or(FieldglassError::OutOfRange)?;

        let bf = msg.gds.bifourier().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "message {message_index} is a {} grid, not bi-Fourier coefficients — \
                 use `decode_message_values`",
                msg.gds.template_name()
            ))
        })?;
        let drs = msg.drs.bifourier().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "bi-Fourier message {message_index} uses §5 packing {} — only \
                 bifourier_complex (5.53) decodes here",
                msg.drs.template_name()
            ))
        })?;

        let (ds_start, ds_end) = msg.ds_range;
        let ds_header = parse_section_header(&self.data[ds_start..ds_end])?;
        let ds_payload = parse_data_section_body(&self.data[ds_start..ds_end], ds_header)?;

        decode_bifourier(ds_payload, drs, bf, msg.drs.num_data_points as usize)
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

        // `total_length` is an attacker-controlled u64; a value near u64::MAX
        // would overflow `offset + total_length`. checked_add turns that into
        // the same "claims more than the buffer holds" error as a merely-too-big
        // length, instead of a panic under overflow checks.
        let msg_end_u64 = match (offset as u64).checked_add(is.total_length) {
            Some(end) if end <= data.len() as u64 => end,
            _ => {
                return Err(FieldglassError::Parse(format!(
                    "Message at offset {offset} claims length {} but only {} bytes remain",
                    is.total_length,
                    data.len() - offset
                )));
            }
        };
        let msg_end = msg_end_u64 as usize;

        // The "impossibly small length" guard above already implies
        // `msg_end >= offset + END_SECTION_LEN >= END_SECTION_LEN`, but assert it
        // locally so the subtraction below can't underflow even if that guard is
        // ever loosened.
        if msg_end < END_SECTION_LEN {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} ends before its trailing 7777 marker"
            )));
        }

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
        cursor += pds_header.length as usize;

        // §5 DRS — required by the WMO spec in every message.
        let drs_header = parse_section_header(&data[cursor..msg_end])?;
        if drs_header.number != DRS_SECTION_NUMBER {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: expected DRS (section {DRS_SECTION_NUMBER}), \
                 got section {}",
                drs_header.number
            )));
        }
        let drs = parse_data_representation_with_header(&data[cursor..msg_end], drs_header)?;
        cursor += drs_header.length as usize;

        // §6 BMS — required by spec (its "indicator" byte signals
        // bitmap-present vs no-bitmap; we just record the byte range here
        // and defer body parsing to decode time).
        let bms_header = parse_section_header(&data[cursor..msg_end])?;
        if bms_header.number != BMS_SECTION_NUMBER {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: expected BMS (section {BMS_SECTION_NUMBER}), \
                 got section {}",
                bms_header.number
            )));
        }
        // The other sections' parsers validate their declared length against
        // the bytes available; BMS/DS are recorded lazily, so do it here.
        // Without this an oversized BMS length pushes `cursor` past `msg_end`,
        // inverting the DS-header slice below, and an oversized DS length
        // records a range that over-reads `data` at decode time.
        if bms_header.length as usize > msg_end - cursor {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: BMS declares length {} but only {} bytes remain",
                bms_header.length,
                msg_end - cursor
            )));
        }
        let bms_end_in_file = cursor + bms_header.length as usize;
        let bms_range = (cursor, bms_end_in_file);
        cursor = bms_end_in_file;

        // §7 DS — required by spec. Same lazy treatment as §6: record the
        // byte range, decode on demand.
        let ds_header = parse_section_header(&data[cursor..msg_end])?;
        if ds_header.number != DS_SECTION_NUMBER {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: expected DS (section {DS_SECTION_NUMBER}), \
                 got section {}",
                ds_header.number
            )));
        }
        if ds_header.length as usize > msg_end - cursor {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset}: DS declares length {} but only {} bytes remain",
                ds_header.length,
                msg_end - cursor
            )));
        }
        let ds_end_in_file = cursor + ds_header.length as usize;
        let ds_range = (cursor, ds_end_in_file);

        messages.push(Grib2Message {
            message_index: messages.len(),
            byte_offset: offset,
            is,
            ids,
            lus_range,
            gds,
            pds,
            drs,
            bms_range,
            ds_range,
        });

        offset = msg_end; // advance to the next message
    }

    Ok(messages)
}
