//! GRIB2 Product Definition Section (§4).
//!
//! Implements the three product-definition templates that cover most
//! operational GRIB2 traffic:
//!
//! - **4.0** — analysis or forecast at a horizontal level / layer at a point in time
//! - **4.8** — average, accumulation, or extreme values at a horizontal level over
//!   a time interval (e.g. total-precipitation accumulation)
//! - **4.11** — individual ensemble forecast at a horizontal level over a time interval
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 4 layout + Templates 4.0 / 4.8 / 4.11. Unsupported template
//! numbers parse as [`ProductTemplate::Unsupported`] so callers still see
//! the section header without erroring out.

use crate::section::{SectionHeader, parse_section_header};
use fieldglass_core::{FieldglassError, bits::sign_magnitude_to_i64};

/// Section number for the Product Definition Section.
pub const PDS_SECTION_NUMBER: u8 = 4;

/// Sentinel marking a 4-byte unsigned PDS field as "missing" (all ones).
pub const U32_MISSING: u32 = 0xFFFF_FFFF;

/// Sentinel marking a 1-byte surface-type/code field as "missing".
pub const U8_MISSING: u8 = 0xFF;

/// Minimum byte length of a PDS — header (5) + NV (2) + template number (2).
/// Real templates push this much higher; this is just the "can we read the
/// template number safely" floor.
const PDS_MIN_LEN: usize = 9;

/// Payload length for the horizontal-level core (templates 4.0 / 4.8 / 4.11
/// share octets 10..=34 = 25 octets).
const HORIZONTAL_CORE_LEN: usize = 25;

/// Length of the time-stats header (end timestamp + n + missing count) that
/// precedes the per-spec table in 4.8 / 4.11.
const STATS_HEADER_LEN: usize = 12;

/// Length of each per-spec entry inside the time-stats block.
const STATS_SPEC_LEN: usize = 12;

/// 6-octet "fixed surface" triple: type + scale factor + scaled value.
const SURFACE_TRIPLE_LEN: usize = 6;

/// Decode a 1-octet sign-magnitude scale factor (bit 7 = sign).
fn read_scale_factor(b: u8) -> Option<i8> {
    if b == U8_MISSING {
        return None;
    }
    let mag = (b & 0x7F) as i8;
    if b & 0x80 != 0 { Some(-mag) } else { Some(mag) }
}

/// Decode a 4-octet sign-magnitude scaled value, mapping the all-ones
/// sentinel to `None`.
fn read_scaled_value(bytes: &[u8]) -> Option<i64> {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if raw == U32_MISSING {
        None
    } else {
        Some(sign_magnitude_to_i64(raw, 32))
    }
}

/// Decode a 4-octet sign-magnitude signed integer (used for forecast time).
fn read_signed_i32(bytes: &[u8]) -> i64 {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    sign_magnitude_to_i64(raw, 32)
}

/// Read the 6-octet fixed-surface triple at `bytes[0..6]`.
fn read_fixed_surface(bytes: &[u8]) -> FixedSurface {
    FixedSurface {
        surface_type: bytes[0],
        scale_factor: read_scale_factor(bytes[1]),
        scaled_value: read_scaled_value(&bytes[2..6]),
    }
}

/// A fixed-surface descriptor (type code + scale factor + scaled value).
///
/// "Missing" surfaces are common in PDS — for a single-level field the
/// second surface is type 255 (missing) and both scale fields are 0xFF.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixedSurface {
    pub surface_type: u8,
    pub scale_factor: Option<i8>,
    pub scaled_value: Option<i64>,
}

impl FixedSurface {
    /// `true` when the surface type itself is the WMO "missing" sentinel.
    pub fn is_missing(&self) -> bool {
        self.surface_type == U8_MISSING
    }

    /// Decoded surface value as `scaled_value × 10^-scale_factor`. Returns
    /// `None` if either component is missing.
    pub fn value(&self) -> Option<f64> {
        let scale = self.scale_factor?;
        let val = self.scaled_value?;
        // Scale exponent is small (−9..=9 in practice); `powi` handles the sign.
        Some((val as f64) * 10f64.powi(-(scale as i32)))
    }
}

/// One row of the time-range-specification table that follows the end
/// timestamp in templates 4.8 / 4.11.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRangeSpec {
    /// WMO Code Table 4.10 — statistical process (average, accumulation, …).
    pub stat_process: u8,
    /// WMO Code Table 4.11 — type of time increment between successive fields.
    pub time_increment_type: u8,
    /// WMO Code Table 4.4 — unit of the statistical-processing length below.
    pub stat_length_unit: u8,
    /// Length of the time range over which statistical processing is done,
    /// in units of `stat_length_unit`.
    pub stat_length: u32,
    /// WMO Code Table 4.4 — unit of the increment below.
    pub increment_unit: u8,
    /// Time increment between successive fields used in statistical process.
    pub increment: u32,
}

/// Time-statistical-processing block shared by templates 4.8 and 4.11.
#[derive(Debug, Clone, PartialEq)]
pub struct StatisticalProcessing {
    pub end_year: u16,
    pub end_month: u8,
    pub end_day: u8,
    pub end_hour: u8,
    pub end_minute: u8,
    pub end_second: u8,
    /// Number of time-range specifications that follow.
    pub num_time_range_specs: u8,
    /// Total number of data values missing in the statistical process.
    pub num_values_missing: u32,
    pub specs: Vec<TimeRangeSpec>,
}

impl StatisticalProcessing {
    /// Render the end-of-interval timestamp as ISO-8601 with no calendar
    /// validation — mirrors [`crate::ids::IdentificationSection::reference_time_iso8601`].
    pub fn end_time_iso8601(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            self.end_year,
            self.end_month,
            self.end_day,
            self.end_hour,
            self.end_minute,
            self.end_second
        )
    }
}

/// Fields shared by every "horizontal level/layer" template (4.0 / 4.8 / 4.11):
/// the parameter triple, generating-process metadata, forecast time, and the
/// two fixed surfaces.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HorizontalProductCommon {
    /// WMO Code Table 4.1 (keyed by discipline).
    pub parameter_category: u8,
    /// WMO Code Table 4.2 (keyed by discipline + category).
    pub parameter_number: u8,
    /// WMO Code Table 4.3.
    pub generating_process_type: u8,
    pub background_process_id: u8,
    pub forecast_process_id: u8,
    pub obs_cutoff_hours: u16,
    pub obs_cutoff_minutes: u8,
    /// WMO Code Table 4.4 — unit of the forecast-time field below.
    pub forecast_time_unit: u8,
    /// Forecast time in units of `forecast_time_unit`. Sign-magnitude on the
    /// wire; widened to `i64` to hold the full encoded range cleanly.
    pub forecast_time: i64,
    pub first_surface: FixedSurface,
    pub second_surface: FixedSurface,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Template40 {
    pub common: HorizontalProductCommon,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template48 {
    pub common: HorizontalProductCommon,
    pub stats: StatisticalProcessing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Template411 {
    pub common: HorizontalProductCommon,
    /// WMO Code Table 4.6 — type of ensemble forecast.
    pub ensemble_type: u8,
    pub perturbation_number: u8,
    pub num_forecasts_in_ensemble: u8,
    pub stats: StatisticalProcessing,
}

/// Decoded template payload. Templates outside the supported set surface as
/// [`ProductTemplate::Unsupported`] so callers can still expose section-
/// header fields and a useful name without erroring.
#[derive(Debug, Clone, PartialEq)]
pub enum ProductTemplate {
    HorizontalAnalysisForecast(Template40),
    HorizontalTimeInterval(Template48),
    EnsembleTimeInterval(Template411),
    Unsupported(u16),
}

/// Parsed contents of the Product Definition Section.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductDefinitionSection {
    pub section_length: u32,
    /// Number of coordinate values appended after the template payload
    /// (used by hybrid-coordinate vertical grids; usually 0).
    pub num_coordinate_values: u16,
    pub template_number: u16,
    pub template: ProductTemplate,
}

impl ProductDefinitionSection {
    /// Short human-readable name of the template (`"horizontal"`,
    /// `"horizontal-interval"`, `"ensemble-interval"`, `"unsupported(4.N)"`).
    pub fn template_name(&self) -> String {
        match &self.template {
            ProductTemplate::HorizontalAnalysisForecast(_) => "horizontal".to_string(),
            ProductTemplate::HorizontalTimeInterval(_) => "horizontal-interval".to_string(),
            ProductTemplate::EnsembleTimeInterval(_) => "ensemble-interval".to_string(),
            ProductTemplate::Unsupported(n) => format!("unsupported(4.{n})"),
        }
    }

    /// `(category, number)` parameter pair, when the template carries it.
    /// All three currently-supported templates do; `None` is reserved for
    /// future non-horizontal templates routed through `Unsupported`.
    pub fn parameter(&self) -> Option<(u8, u8)> {
        self.common()
            .map(|c| (c.parameter_category, c.parameter_number))
    }

    /// Borrow the horizontal-level common block if the template has one.
    pub fn common(&self) -> Option<&HorizontalProductCommon> {
        match &self.template {
            ProductTemplate::HorizontalAnalysisForecast(t) => Some(&t.common),
            ProductTemplate::HorizontalTimeInterval(t) => Some(&t.common),
            ProductTemplate::EnsembleTimeInterval(t) => Some(&t.common),
            ProductTemplate::Unsupported(_) => None,
        }
    }

    /// Borrow the time-statistics block, when the template has one.
    pub fn stats(&self) -> Option<&StatisticalProcessing> {
        match &self.template {
            ProductTemplate::HorizontalTimeInterval(t) => Some(&t.stats),
            ProductTemplate::EnsembleTimeInterval(t) => Some(&t.stats),
            _ => None,
        }
    }
}

/// Parse the Product Definition Section starting at `bytes[0]`.
pub fn parse_product_definition(bytes: &[u8]) -> Result<ProductDefinitionSection, FieldglassError> {
    let header = parse_section_header(bytes)?;
    parse_product_definition_with_header(bytes, header)
}

/// Variant for callers that have already read the section header.
pub fn parse_product_definition_with_header(
    bytes: &[u8],
    header: SectionHeader,
) -> Result<ProductDefinitionSection, FieldglassError> {
    if header.number != PDS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "expected PDS (section {PDS_SECTION_NUMBER}), got section {}",
            header.number
        )));
    }
    let len = header.length as usize;
    if len < PDS_MIN_LEN {
        return Err(FieldglassError::Parse(format!(
            "PDS section length {len} is below the {PDS_MIN_LEN}-byte minimum"
        )));
    }
    if bytes.len() < len {
        return Err(FieldglassError::Parse(format!(
            "PDS declares length {len} but only {} bytes available",
            bytes.len()
        )));
    }

    let num_coordinate_values = u16::from_be_bytes([bytes[5], bytes[6]]);
    let template_number = u16::from_be_bytes([bytes[7], bytes[8]]);

    // Template payload starts at section octet 10 (= byte index 9).
    let payload = &bytes[9..len];
    let template = match template_number {
        0 => ProductTemplate::HorizontalAnalysisForecast(parse_template_4_0(payload)?),
        8 => ProductTemplate::HorizontalTimeInterval(parse_template_4_8(payload)?),
        11 => ProductTemplate::EnsembleTimeInterval(parse_template_4_11(payload)?),
        other => ProductTemplate::Unsupported(other),
    };

    Ok(ProductDefinitionSection {
        section_length: header.length,
        num_coordinate_values,
        template_number,
        template,
    })
}

/// Octets 10..=34 of the section (= `payload[0..25]`) carry the horizontal
/// product common block used by 4.0/4.8/4.11.
fn parse_horizontal_common(payload: &[u8]) -> Result<HorizontalProductCommon, FieldglassError> {
    if payload.len() < HORIZONTAL_CORE_LEN {
        return Err(FieldglassError::Parse(format!(
            "PDS horizontal common block needs {HORIZONTAL_CORE_LEN} bytes, got {}",
            payload.len()
        )));
    }
    Ok(HorizontalProductCommon {
        parameter_category: payload[0],
        parameter_number: payload[1],
        generating_process_type: payload[2],
        background_process_id: payload[3],
        forecast_process_id: payload[4],
        obs_cutoff_hours: u16::from_be_bytes([payload[5], payload[6]]),
        obs_cutoff_minutes: payload[7],
        forecast_time_unit: payload[8],
        forecast_time: read_signed_i32(&payload[9..13]),
        first_surface: read_fixed_surface(&payload[13..13 + SURFACE_TRIPLE_LEN]),
        second_surface: read_fixed_surface(&payload[19..19 + SURFACE_TRIPLE_LEN]),
    })
}

fn parse_template_4_0(payload: &[u8]) -> Result<Template40, FieldglassError> {
    Ok(Template40 {
        common: parse_horizontal_common(payload)?,
    })
}

fn parse_template_4_8(payload: &[u8]) -> Result<Template48, FieldglassError> {
    let common = parse_horizontal_common(payload)?;
    // Stats block immediately follows the 25-byte horizontal core.
    let stats = parse_statistics(&payload[HORIZONTAL_CORE_LEN..])?;
    Ok(Template48 { common, stats })
}

fn parse_template_4_11(payload: &[u8]) -> Result<Template411, FieldglassError> {
    let common = parse_horizontal_common(payload)?;
    // Ensemble triple precedes the stats block in 4.11.
    let ens_offset = HORIZONTAL_CORE_LEN;
    if payload.len() < ens_offset + 3 {
        return Err(FieldglassError::Parse(format!(
            "PDS template 4.11 needs at least {} ensemble bytes after the common block, got {}",
            3,
            payload.len().saturating_sub(ens_offset),
        )));
    }
    let ensemble_type = payload[ens_offset];
    let perturbation_number = payload[ens_offset + 1];
    let num_forecasts_in_ensemble = payload[ens_offset + 2];
    let stats = parse_statistics(&payload[ens_offset + 3..])?;
    Ok(Template411 {
        common,
        ensemble_type,
        perturbation_number,
        num_forecasts_in_ensemble,
        stats,
    })
}

fn parse_statistics(bytes: &[u8]) -> Result<StatisticalProcessing, FieldglassError> {
    if bytes.len() < STATS_HEADER_LEN {
        return Err(FieldglassError::Parse(format!(
            "PDS stats block needs {STATS_HEADER_LEN} bytes of header, got {}",
            bytes.len()
        )));
    }
    let end_year = u16::from_be_bytes([bytes[0], bytes[1]]);
    let end_month = bytes[2];
    let end_day = bytes[3];
    let end_hour = bytes[4];
    let end_minute = bytes[5];
    let end_second = bytes[6];
    let num_time_range_specs = bytes[7];
    let num_values_missing = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);

    let n = num_time_range_specs as usize;
    let needed = STATS_HEADER_LEN
        .checked_add(n.checked_mul(STATS_SPEC_LEN).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "PDS stats: spec count {n} overflows when sized at {STATS_SPEC_LEN} bytes each"
            ))
        })?)
        .ok_or_else(|| FieldglassError::Parse("PDS stats: total length overflow".to_string()))?;
    if bytes.len() < needed {
        return Err(FieldglassError::Parse(format!(
            "PDS stats: need {needed} bytes for {n} spec(s), got {}",
            bytes.len()
        )));
    }

    let mut specs = Vec::with_capacity(n);
    for i in 0..n {
        let off = STATS_HEADER_LEN + i * STATS_SPEC_LEN;
        let s = &bytes[off..off + STATS_SPEC_LEN];
        specs.push(TimeRangeSpec {
            stat_process: s[0],
            time_increment_type: s[1],
            stat_length_unit: s[2],
            stat_length: u32::from_be_bytes([s[3], s[4], s[5], s[6]]),
            increment_unit: s[7],
            increment: u32::from_be_bytes([s[8], s[9], s[10], s[11]]),
        });
    }

    Ok(StatisticalProcessing {
        end_year,
        end_month,
        end_day,
        end_hour,
        end_minute,
        end_second,
        num_time_range_specs,
        num_values_missing,
        specs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append `n` unsigned big-endian bytes encoding `v` into `buf`.
    fn push_be(buf: &mut Vec<u8>, v: u64, width: usize) {
        let bytes = v.to_be_bytes();
        buf.extend_from_slice(&bytes[(8 - width)..]);
    }

    /// Build a minimal §4 with template 4.0:
    /// - parameter 0/0/0 (temperature in WMO discipline 0)
    /// - generating process 2 (forecast), forecast time 24 hours
    /// - first surface = isobaric (type 100, scale 0, value 50000 Pa = 500 hPa)
    /// - second surface = missing (single-level field)
    fn build_pds_4_0() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let payload_len = HORIZONTAL_CORE_LEN as u32;
        let section_len = 9 + payload_len; // header(5) + NV(2) + tpl(2) + payload
        push_be(&mut buf, section_len as u64, 4);
        buf.push(PDS_SECTION_NUMBER);
        push_be(&mut buf, 0, 2); // NV
        push_be(&mut buf, 0, 2); // template 4.0

        // Horizontal common block (25 bytes)
        buf.push(0); // parameter category
        buf.push(0); // parameter number
        buf.push(2); // generating process type = forecast
        buf.push(0); // background process id
        buf.push(96); // forecast process id (NCEP GFS)
        push_be(&mut buf, 0, 2); // obs cutoff hours
        buf.push(0); // obs cutoff minutes
        buf.push(1); // forecast time unit = hour
        push_be(&mut buf, 24, 4); // forecast time = 24 (sign-magnitude positive)
        // First surface: isobaric (100), scale 0, value 50000
        buf.push(100);
        buf.push(0);
        push_be(&mut buf, 50000, 4);
        // Second surface: missing
        buf.push(U8_MISSING);
        buf.push(U8_MISSING);
        push_be(&mut buf, U32_MISSING as u64, 4);
        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_4_0_round_trips_synthesized_payload() {
        let bytes = build_pds_4_0();
        let pds = parse_product_definition(&bytes).expect("parse 4.0");
        assert_eq!(pds.template_number, 0);
        assert_eq!(pds.num_coordinate_values, 0);

        let common = pds.common().expect("4.0 has common block");
        assert_eq!(common.parameter_category, 0);
        assert_eq!(common.parameter_number, 0);
        assert_eq!(common.generating_process_type, 2);
        assert_eq!(common.forecast_process_id, 96);
        assert_eq!(common.forecast_time_unit, 1);
        assert_eq!(common.forecast_time, 24);

        assert_eq!(common.first_surface.surface_type, 100);
        assert_eq!(common.first_surface.scale_factor, Some(0));
        assert_eq!(common.first_surface.scaled_value, Some(50_000));
        assert_eq!(common.first_surface.value(), Some(50_000.0));

        assert!(common.second_surface.is_missing());
        assert_eq!(common.second_surface.scale_factor, None);
        assert_eq!(common.second_surface.scaled_value, None);
        assert_eq!(common.second_surface.value(), None);

        assert_eq!(pds.parameter(), Some((0, 0)));
        assert_eq!(pds.template_name(), "horizontal");
        assert!(pds.stats().is_none());
    }

    #[test]
    fn negative_forecast_time_decodes_via_sign_magnitude() {
        let mut bytes = build_pds_4_0();
        // Forecast time is at section octets 19..=22 = bytes 18..=21.
        // Set sign bit + magnitude 6.
        let neg = 0x8000_0000u32 | 6;
        bytes[18..22].copy_from_slice(&neg.to_be_bytes());
        let pds = parse_product_definition(&bytes).expect("parse");
        let common = pds.common().unwrap();
        assert_eq!(common.forecast_time, -6);
    }

    #[test]
    fn scale_factor_sign_magnitude() {
        assert_eq!(read_scale_factor(0x02), Some(2));
        assert_eq!(read_scale_factor(0x82), Some(-2));
        assert_eq!(read_scale_factor(0xFF), None);
        // 0x80 is "−0" in sign-magnitude — normalise to +0 so it stays usable.
        assert_eq!(read_scale_factor(0x80), Some(0));
    }

    #[test]
    fn fixed_surface_value_applies_scale() {
        // 1.5 m above ground = type 103, scale -1, value 15 → 15 * 10^1 = 150.
        // Wait — scale factor of -1 means value × 10^-(-1) = value × 10 = 150.
        // Conversely scale factor of 1 means value × 10^-1 = 1.5 (e.g. 15 mm precip).
        // Verify the convention.
        let s = FixedSurface {
            surface_type: 103,
            scale_factor: Some(1),
            scaled_value: Some(15),
        };
        assert!((s.value().unwrap() - 1.5).abs() < 1e-9);
    }

    /// Build a §4 PDS with template 4.8: 6-hour precipitation accumulation
    /// at the surface (parameter 0/1/8 = APCP), single time-range spec
    /// (accumulation over 6 hours, no increment).
    fn build_pds_4_8() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let payload_len = (HORIZONTAL_CORE_LEN + STATS_HEADER_LEN + STATS_SPEC_LEN) as u32;
        let section_len = 9 + payload_len;
        push_be(&mut buf, section_len as u64, 4);
        buf.push(PDS_SECTION_NUMBER);
        push_be(&mut buf, 0, 2); // NV
        push_be(&mut buf, 8, 2); // template 4.8

        // Horizontal common: APCP (0/1/8), forecast generating process,
        // forecast time = end of interval (hour 6).
        buf.push(1); // parameter category = moisture
        buf.push(8); // parameter number = total precipitation (APCP)
        buf.push(2); // generating process type = forecast
        buf.push(0);
        buf.push(96);
        push_be(&mut buf, 0, 2);
        buf.push(0);
        buf.push(1); // forecast time unit = hour
        push_be(&mut buf, 6, 4); // start of accumulation, hours
        // First surface: ground (type 1), scale/val missing
        buf.push(1);
        buf.push(U8_MISSING);
        push_be(&mut buf, U32_MISSING as u64, 4);
        // Second surface: missing
        buf.push(U8_MISSING);
        buf.push(U8_MISSING);
        push_be(&mut buf, U32_MISSING as u64, 4);

        // Stats header (12 bytes): end of interval 2024-01-01T12:00:00Z,
        // 1 spec, 0 missing values.
        push_be(&mut buf, 2024, 2); // year
        buf.push(1); // month
        buf.push(1); // day
        buf.push(12); // hour
        buf.push(0); // minute
        buf.push(0); // second
        buf.push(1); // num specs
        push_be(&mut buf, 0, 4); // missing count

        // Spec (12 bytes): accumulation (0/1), unit hour (1), length 6,
        // no increment.
        buf.push(1); // stat process = accumulation
        buf.push(2); // time-increment type = same forecast time, increment of ref time
        buf.push(1); // stat length unit = hour
        push_be(&mut buf, 6, 4); // stat length = 6
        buf.push(255); // increment unit = missing
        push_be(&mut buf, 0, 4); // increment = 0

        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_4_8_decodes_accumulation_spec() {
        let bytes = build_pds_4_8();
        let pds = parse_product_definition(&bytes).expect("parse 4.8");
        assert_eq!(pds.template_number, 8);
        let common = pds.common().expect("4.8 has common block");
        assert_eq!(common.parameter_category, 1);
        assert_eq!(common.parameter_number, 8);

        let stats = pds.stats().expect("4.8 has stats");
        assert_eq!(stats.end_year, 2024);
        assert_eq!(stats.num_time_range_specs, 1);
        assert_eq!(stats.specs.len(), 1);
        assert_eq!(stats.specs[0].stat_process, 1); // accumulation
        assert_eq!(stats.specs[0].stat_length_unit, 1); // hour
        assert_eq!(stats.specs[0].stat_length, 6);
        assert_eq!(stats.end_time_iso8601(), "2024-01-01T12:00:00Z");
        assert_eq!(pds.template_name(), "horizontal-interval");
    }

    /// Build a §4 PDS with template 4.11: ensemble member 5/20 control
    /// run, 6-hour mean of 2 m temperature.
    fn build_pds_4_11() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        let payload_len = (HORIZONTAL_CORE_LEN + 3 + STATS_HEADER_LEN + STATS_SPEC_LEN) as u32;
        let section_len = 9 + payload_len;
        push_be(&mut buf, section_len as u64, 4);
        buf.push(PDS_SECTION_NUMBER);
        push_be(&mut buf, 0, 2); // NV
        push_be(&mut buf, 11, 2); // template 4.11

        // Horizontal common: 2 m temperature (0/0/0 at 103/2.0)
        buf.push(0);
        buf.push(0);
        buf.push(4); // generating process type = ensemble forecast
        buf.push(0);
        buf.push(96);
        push_be(&mut buf, 0, 2);
        buf.push(0);
        buf.push(1); // hour
        push_be(&mut buf, 6, 4); // forecast time = 6
        // First surface: height-above-ground (103), 2 m
        buf.push(103);
        buf.push(0);
        push_be(&mut buf, 2, 4);
        // Second surface: missing
        buf.push(U8_MISSING);
        buf.push(U8_MISSING);
        push_be(&mut buf, U32_MISSING as u64, 4);

        // Ensemble triple
        buf.push(3); // type = positively perturbed
        buf.push(5); // perturbation number
        buf.push(20); // members in ensemble

        // Stats header: end of interval 2024-01-01T18:00:00Z, 1 spec
        push_be(&mut buf, 2024, 2);
        buf.push(1);
        buf.push(1);
        buf.push(18);
        buf.push(0);
        buf.push(0);
        buf.push(1);
        push_be(&mut buf, 0, 4);
        // Spec: average over 6 hours
        buf.push(0); // average
        buf.push(2);
        buf.push(1);
        push_be(&mut buf, 6, 4);
        buf.push(255);
        push_be(&mut buf, 0, 4);

        assert_eq!(buf.len() as u32, section_len);
        buf
    }

    #[test]
    fn template_4_11_decodes_ensemble_and_stats() {
        let bytes = build_pds_4_11();
        let pds = parse_product_definition(&bytes).expect("parse 4.11");
        assert_eq!(pds.template_number, 11);

        let common = pds.common().unwrap();
        assert_eq!(common.parameter_category, 0);
        assert_eq!(common.parameter_number, 0);
        assert_eq!(common.first_surface.surface_type, 103);
        assert_eq!(common.first_surface.value(), Some(2.0));

        let t = match &pds.template {
            ProductTemplate::EnsembleTimeInterval(t) => t,
            other => panic!("expected EnsembleTimeInterval, got {other:?}"),
        };
        assert_eq!(t.ensemble_type, 3);
        assert_eq!(t.perturbation_number, 5);
        assert_eq!(t.num_forecasts_in_ensemble, 20);

        let stats = pds.stats().unwrap();
        assert_eq!(stats.end_hour, 18);
        assert_eq!(stats.specs.len(), 1);
        assert_eq!(stats.specs[0].stat_process, 0); // average
        assert_eq!(pds.template_name(), "ensemble-interval");
    }

    #[test]
    fn rejects_wrong_section_number() {
        let mut bytes = build_pds_4_0();
        bytes[4] = 3; // claim §3
        assert!(parse_product_definition(&bytes).is_err());
    }

    #[test]
    fn rejects_short_section() {
        let bytes = [0u8; 8];
        assert!(parse_product_definition(&bytes).is_err());
    }

    #[test]
    fn rejects_length_below_minimum() {
        let mut bytes = build_pds_4_0();
        bytes[0..4].copy_from_slice(&8u32.to_be_bytes());
        assert!(parse_product_definition(&bytes).is_err());
    }

    #[test]
    fn rejects_length_exceeding_buffer() {
        let mut bytes = build_pds_4_0();
        bytes[0..4].copy_from_slice(&1000u32.to_be_bytes());
        assert!(parse_product_definition(&bytes).is_err());
    }

    #[test]
    fn rejects_4_8_when_stats_truncated() {
        let mut bytes = build_pds_4_8();
        // Drop the trailing spec — declared length still says full.
        // Forge a length that fits in-buffer but leaves the spec count > 0.
        let new_len = bytes.len() - STATS_SPEC_LEN;
        bytes[0..4].copy_from_slice(&(new_len as u32).to_be_bytes());
        bytes.truncate(new_len);
        assert!(parse_product_definition(&bytes).is_err());
    }

    #[test]
    fn rejects_4_11_when_ensemble_triple_missing() {
        // Build a 4.11 with only the horizontal core — no ensemble bytes.
        let mut buf: Vec<u8> = Vec::new();
        let payload_len = HORIZONTAL_CORE_LEN as u32;
        let section_len = 9 + payload_len;
        push_be(&mut buf, section_len as u64, 4);
        buf.push(PDS_SECTION_NUMBER);
        push_be(&mut buf, 0, 2);
        push_be(&mut buf, 11, 2);
        buf.extend_from_slice(&[0u8; HORIZONTAL_CORE_LEN]);
        assert!(parse_product_definition(&buf).is_err());
    }

    #[test]
    fn rejects_4_0_when_horizontal_common_truncated() {
        // §4 with a declared section length that just covers the header (9
        // bytes) plus 10 of the 25 octets in the horizontal-common block.
        // `parse_horizontal_common` must reject the short payload.
        let payload_len = 10usize;
        let section_len = 9 + payload_len;
        let mut buf: Vec<u8> = Vec::new();
        push_be(&mut buf, section_len as u64, 4);
        buf.push(PDS_SECTION_NUMBER);
        push_be(&mut buf, 0, 2); // NV
        push_be(&mut buf, 0, 2); // template 4.0
        buf.extend_from_slice(&[0u8; 10]); // ten bytes of "common", far short of 25
        let err = parse_product_definition(&buf).expect_err("must reject");
        let s = err.to_string();
        assert!(
            s.contains("horizontal common block needs"),
            "error names horizontal-common shortfall, got: {s}",
        );
    }

    #[test]
    fn rejects_4_8_when_stats_header_missing() {
        // §4 template 4.8 with declared section length covering only the
        // horizontal-common block — no room for the 12-byte stats header.
        // `parse_statistics` must reject before reading the end-year.
        let payload_len = HORIZONTAL_CORE_LEN; // common only, no stats
        let section_len = 9 + payload_len;
        let mut buf: Vec<u8> = Vec::new();
        push_be(&mut buf, section_len as u64, 4);
        buf.push(PDS_SECTION_NUMBER);
        push_be(&mut buf, 0, 2); // NV
        push_be(&mut buf, 8, 2); // template 4.8
        buf.extend_from_slice(&[0u8; HORIZONTAL_CORE_LEN]);
        let err = parse_product_definition(&buf).expect_err("must reject");
        let s = err.to_string();
        assert!(
            s.contains("stats block needs"),
            "error names stats-header shortfall, got: {s}",
        );
    }

    #[test]
    fn unsupported_template_round_trips_with_label() {
        let mut bytes = build_pds_4_0();
        // Template number lives at section octets 8–9 = bytes 7–8.
        bytes[7..9].copy_from_slice(&99u16.to_be_bytes());
        let pds = parse_product_definition(&bytes).expect("parse");
        assert!(matches!(pds.template, ProductTemplate::Unsupported(99)));
        assert_eq!(pds.template_name(), "unsupported(4.99)");
        assert_eq!(pds.parameter(), None);
        assert!(pds.common().is_none());
        assert!(pds.stats().is_none());
    }
}
