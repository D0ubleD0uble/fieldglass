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
//! Section 4 layout + Templates 4.0 / 4.8 / 4.11. The templates that share
//! 4.0's layout through octet 34 (4.1, 4.2, 4.5, 4.6, 4.7, 4.9, 4.10, 4.12,
//! 4.15) parse as [`ProductTemplate::SharedLayout`], with that common block
//! read and the rest kept as written. The chemistry and aerosol templates
//! (4.40–4.48), which put a constituent type and size and wavelength ranges
//! between the parameter and the rest of that block, parse as
//! [`ProductTemplate::Constituent`]. Any other template number parses as
//! [`ProductTemplate::Unsupported`], with its octets, so callers still see
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
    /// The surface type (Code Table 4.5); 255 is the "missing" sentinel.
    pub surface_type: u8,
    /// Decimal scale factor, or `None` when the octet is the missing sentinel.
    pub scale_factor: Option<i8>,
    /// The scaled value, or `None` when the octets are the missing sentinel.
    /// [`FixedSurface::value`] combines it with the scale factor.
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
    /// End-of-interval year, four digits.
    pub end_year: u16,
    /// End-of-interval month, 1–12.
    pub end_month: u8,
    /// End-of-interval day of the month, 1–31.
    pub end_day: u8,
    /// End-of-interval hour, 0–23 UTC.
    pub end_hour: u8,
    /// End-of-interval minute, 0–59.
    pub end_minute: u8,
    /// End-of-interval second, 0–60.
    pub end_second: u8,
    /// Number of time-range specifications that follow.
    pub num_time_range_specs: u8,
    /// Total number of data values missing in the statistical process.
    pub num_values_missing: u32,
    /// The time-range specifications themselves, `num_time_range_specs` long.
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
    /// Background generating-process identifier, defined by the centre.
    pub background_process_id: u8,
    /// Forecast generating-process identifier, defined by the centre — the
    /// model that produced the field.
    pub forecast_process_id: u8,
    /// Hours after the reference time at which observations were cut off.
    pub obs_cutoff_hours: u16,
    /// Minutes of the observation cut-off, added to `obs_cutoff_hours`.
    pub obs_cutoff_minutes: u8,
    /// WMO Code Table 4.4 — unit of the forecast-time field below.
    pub forecast_time_unit: u8,
    /// Forecast time in units of `forecast_time_unit`. Sign-magnitude on the
    /// wire; widened to `i64` to hold the full encoded range cleanly.
    pub forecast_time: i64,
    /// The level, or the top of a layer.
    pub first_surface: FixedSurface,
    /// The bottom of a layer; missing for a single-level field.
    pub second_surface: FixedSurface,
}

/// Template 4.0 — analysis or forecast at a horizontal level or layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Template40 {
    /// The parameter, generating process, forecast time and surfaces.
    pub common: HorizontalProductCommon,
}

/// Template 4.8 — as 4.0, but statistically processed over a time interval.
#[derive(Debug, Clone, PartialEq)]
pub struct Template48 {
    /// The parameter, generating process, forecast time and surfaces.
    pub common: HorizontalProductCommon,
    /// The interval, and what was done over it.
    pub stats: StatisticalProcessing,
}

/// Template 4.11 — as 4.8, for one member of an ensemble.
#[derive(Debug, Clone, PartialEq)]
pub struct Template411 {
    /// The parameter, generating process, forecast time and surfaces.
    pub common: HorizontalProductCommon,
    /// WMO Code Table 4.6 — type of ensemble forecast.
    pub ensemble_type: u8,
    /// Which member of the ensemble this field is.
    pub perturbation_number: u8,
    /// How many members the ensemble has.
    pub num_forecasts_in_ensemble: u8,
    /// The interval, and what was done over it.
    pub stats: StatisticalProcessing,
}

/// Decoded template payload. Templates outside the supported set surface as
/// [`ProductTemplate::Unsupported`] so callers can still expose section-
/// header fields and a useful name without erroring.
#[derive(Debug, Clone, PartialEq)]
pub enum ProductTemplate {
    /// §4.0.
    HorizontalAnalysisForecast(Template40),
    /// §4.8.
    HorizontalTimeInterval(Template48),
    /// §4.11.
    EnsembleTimeInterval(Template411),
    /// A template this build has no type of its own for, whose octets 10–34
    /// are 4.0's horizontal common block (#959): 4.1, 4.2, 4.5, 4.6, 4.7, 4.9,
    /// 4.10, 4.12 and 4.15. Boxed, as these are rare, so the common case pays
    /// nothing for the variant.
    SharedLayout(Box<SharedLayoutTemplate>),
    /// A chemistry or aerosol template, 4.40 to 4.48 (#1130). Boxed for the
    /// reason `SharedLayout` is.
    Constituent(Box<ConstituentTemplate>),
    /// A template this build does not read, carrying its number and its
    /// octets from 10 to the end of the section as written.
    Unsupported(u16, Box<[u8]>),
}

/// One limit of a size or wavelength range: a scale factor and a scaled
/// value, both sign-magnitude, as a fixed surface's are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScaledLimit {
    /// Decimal scale factor, or `None` when the octet is the missing sentinel.
    pub scale_factor: Option<i8>,
    /// The scaled value, or `None` when the octets are the missing sentinel.
    pub scaled_value: Option<i64>,
}

impl ScaledLimit {
    /// The limit as `scaled_value × 10^-scale_factor`, in metres for a size or
    /// a wavelength. `None` if either component is missing.
    pub fn value(&self) -> Option<f64> {
        let scale = self.scale_factor?;
        let val = self.scaled_value?;
        Some((val as f64) * 10f64.powi(-(scale as i32)))
    }
}

/// A range of aerosol particle sizes or of wavelengths: the type of interval
/// (Code Table 4.91: below the first limit, between the two, …) and its two
/// limits. Eleven octets on the wire.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TypedInterval {
    /// Code Table 4.91; 255 is missing, and then neither limit means anything.
    pub interval_type: u8,
    /// The first limit.
    pub first: ScaledLimit,
    /// The second limit.
    pub second: ScaledLimit,
}

/// The ensemble block of a member forecast: Code Table 4.6 type, which
/// member, and how many there are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EnsembleBlock {
    /// WMO Code Table 4.6 — type of ensemble forecast.
    pub ensemble_type: u8,
    /// Which member of the ensemble this field is.
    pub perturbation_number: u8,
    /// How many members the ensemble has.
    pub num_forecasts_in_ensemble: u8,
}

/// A chemistry or aerosol product template (WMO FM 92 Code Table 4.0,
/// 4.40–4.48): the parameter, then what it is a quantity of, then 4.0's
/// generating process, forecast time and fixed surfaces, then for some an
/// ensemble block and 4.8's time interval. Per the template definitions:
///
/// | Template | After the parameter (octet 12 on) | Then |
/// |---|---|---|
/// | 4.40 | chemical constituent (Code Table 4.230) | — |
/// | 4.41 | chemical constituent | ensemble |
/// | 4.42 | chemical constituent | statistics |
/// | 4.43 | chemical constituent | ensemble, statistics |
/// | 4.44 | aerosol type (4.233), size range | — (a two-octet forecast time) |
/// | 4.45 | aerosol type, size range | ensemble |
/// | 4.46 | aerosol type, size range | statistics |
/// | 4.47 | generating process, then aerosol type, size range | ensemble, statistics |
/// | 4.48 | aerosol type, size range, wavelength range | — |
///
/// So the fixed surfaces are at octets 25–36 for 4.40–4.43, 34–45 for 4.44,
/// 36–47 for 4.45–4.47 and 47–58 for 4.48, not 4.0's 23–34.
///
/// Two layouts are read as the spec has them where a reference reader does
/// not, or not only:
///
/// * **4.47** puts the type of generating process at octet 12 and the aerosol
///   type at 13–14, unlike every other aerosol template (WMO's own table,
///   NCEPLIBS-g2c and wgrib2's `code_table_4_233_location`). eccodes (2.34.1,
///   2.45.0 and the 2.48 PyPI wheel) lays 4.47 out as 4.46 is, with the
///   aerosol type at 12–13 and the generating process at 25, so it reads a
///   conforming 4.47's constituent, size range and generating process from the
///   wrong octets, and a 4.47 it writes reads wrongly here. A known
///   divergence: ADR-0017 (`docs/decisions/0017-grib2-template-4-47-follows-wmo.md`)
///   and `tests/fixtures/NOTICE.md`.
/// * **4.44** states its forecast time in two unsigned octets (32–33), so the
///   template is 45 octets long. eccodes once wrote it in four signed octets
///   (its GRIB-29) and reads a 4.44 of any other length that way; so does this
///   reader, since the section length tells the two apart.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstituentTemplate {
    /// The template number, 40 to 48.
    pub number: u16,
    /// The parameter, generating process, forecast time and surfaces.
    pub common: HorizontalProductCommon,
    /// The atmospheric chemical constituent (Code Table 4.230, 4.40–4.43) or
    /// aerosol type (Code Table 4.233, 4.44–4.48). Both tables are WMO Common
    /// Code Table C-14.
    pub constituent_type: u16,
    /// The particle size range, 4.44–4.48. `None` for a chemical template.
    pub size: Option<TypedInterval>,
    /// The wavelength range, 4.48 only.
    pub wavelength: Option<TypedInterval>,
    /// The ensemble block of 4.41, 4.43, 4.45 and 4.47.
    pub ensemble: Option<EnsembleBlock>,
    /// The interval, and what was done over it, for 4.42, 4.43, 4.46 and 4.47.
    pub stats: Option<StatisticalProcessing>,
    /// The constituent type and the size and wavelength ranges, as written:
    /// two, 13 or 24 octets. What a caller that must tell two such fields apart
    /// compares, as [`ProductDefinitionSection::unread_octets`] is for a
    /// template read less far.
    pub qualifier: Box<[u8]>,
}

/// Where a chemistry or aerosol template keeps what it adds to 4.0's layout.
struct ConstituentLayout {
    /// Octets of constituent type, size range and wavelength range.
    qualifier: usize,
    /// Whether the generating process comes first (octet 12), as only 4.47's
    /// does.
    process_first: bool,
    /// Whether an ensemble block follows the fixed surfaces.
    ensemble: bool,
    /// Whether 4.8's time interval follows that.
    statistical: bool,
}

/// Octets of a constituent type.
const CONSTITUENT_LEN: usize = 2;

/// Octets of a size or wavelength range: Code Table 4.91 type, then two limits
/// of a scale factor and a scaled value each.
const INTERVAL_LEN: usize = 11;

/// The layout of chemistry or aerosol template `number`, or `None` for a
/// template that is not one.
fn constituent_layout(number: u16) -> Option<ConstituentLayout> {
    let chemical = CONSTITUENT_LEN;
    let aerosol = CONSTITUENT_LEN + INTERVAL_LEN;
    let (qualifier, process_first, ensemble, statistical) = match number {
        40 => (chemical, false, false, false),
        41 => (chemical, false, true, false),
        42 => (chemical, false, false, true),
        43 => (chemical, false, true, true),
        44 => (aerosol, false, false, false),
        45 => (aerosol, false, true, false),
        46 => (aerosol, false, false, true),
        47 => (aerosol, true, true, true),
        48 => (aerosol + INTERVAL_LEN, false, false, false),
        _ => return None,
    };
    Some(ConstituentLayout {
        qualifier,
        process_first,
        ensemble,
        statistical,
    })
}

/// A product template laid out as 4.0 up to octet 34, then a block of its
/// own, then for the statistical templates (4.9, 4.10, 4.12) the time-interval
/// block 4.8 has. Per WMO FM 92 Code Table 4.0 and the template definitions:
///
/// | Template | Block after octet 34 | Then |
/// |---|---|---|
/// | 4.1 | ensemble: type, perturbation number, members (3) | — |
/// | 4.2 | derived forecast and members (2) | — |
/// | 4.5 | probability: number, count, type, limits (13) | — |
/// | 4.6 | percentile (1) | — |
/// | 4.7 | none (4.0's layout, an analysis or forecast error) | — |
/// | 4.9 | probability, as 4.5 (13) | statistics |
/// | 4.10 | percentile (1) | statistics |
/// | 4.12 | derived forecast and members (2) | statistics |
/// | 4.15 | statistical process, spatial processing, points used (3) | — |
#[derive(Debug, Clone, PartialEq)]
pub struct SharedLayoutTemplate {
    /// The template number.
    pub number: u16,
    /// The parameter, generating process, forecast time and surfaces.
    pub common: HorizontalProductCommon,
    /// The template's own block after octet 34, as written; empty for 4.7.
    pub qualifier: Box<[u8]>,
    /// The interval, and what was done over it, for 4.9, 4.10 and 4.12.
    pub stats: Option<StatisticalProcessing>,
}

/// The length of the block a template sharing 4.0's layout has after octet 34,
/// and whether the statistical block follows it. `None` for a template that
/// does not share the layout.
fn shared_layout(number: u16) -> Option<(usize, bool)> {
    match number {
        1 => Some((3, false)),
        2 => Some((2, false)),
        5 => Some((13, false)),
        6 => Some((1, false)),
        7 => Some((0, false)),
        9 => Some((13, true)),
        10 => Some((1, true)),
        12 => Some((2, true)),
        15 => Some((3, false)),
        _ => None,
    }
}

/// Parsed contents of the Product Definition Section.
#[derive(Debug, Clone, PartialEq)]
pub struct ProductDefinitionSection {
    /// Length of the section in bytes, from its own 4-octet length prefix.
    pub section_length: u32,
    /// Number of coordinate values appended after the template payload
    /// (used by hybrid-coordinate vertical grids; usually 0).
    pub num_coordinate_values: u16,
    /// Product Definition Template number (Code Table 4.0), as the message
    /// states it — kept even for a template this build does not model.
    pub template_number: u16,
    /// The template, parsed, or [`ProductTemplate::Unsupported`].
    pub template: ProductTemplate,
}

impl ProductDefinitionSection {
    /// Short human-readable name of the template (`"horizontal"`,
    /// `"horizontal-interval"`, `"ensemble-interval"`, `"horizontal(4.N)"`,
    /// `"chemical(4.N)"`, `"aerosol(4.N)"`, `"unsupported(4.N)"`).
    pub fn template_name(&self) -> String {
        match &self.template {
            ProductTemplate::HorizontalAnalysisForecast(_) => "horizontal".to_string(),
            ProductTemplate::HorizontalTimeInterval(_) => "horizontal-interval".to_string(),
            ProductTemplate::EnsembleTimeInterval(_) => "ensemble-interval".to_string(),
            ProductTemplate::SharedLayout(t) => format!("horizontal(4.{})", t.number),
            ProductTemplate::Constituent(t) if t.size.is_some() => {
                format!("aerosol(4.{})", t.number)
            }
            ProductTemplate::Constituent(t) => format!("chemical(4.{})", t.number),
            ProductTemplate::Unsupported(n, _) => format!("unsupported(4.{n})"),
        }
    }

    /// The template's octets this crate does not read into fields, as written:
    /// for a template sharing 4.0's layout, its own block after octet 34 (see
    /// [`SharedLayoutTemplate`]); for one this crate does not read at all,
    /// every octet from 10 on. Empty for 4.0, 4.8, 4.11 and the chemistry and
    /// aerosol templates, which are read whole.
    ///
    /// What a caller that must tell two fields apart compares when it cannot
    /// name what these octets mean (#959).
    pub fn unread_octets(&self) -> &[u8] {
        match &self.template {
            ProductTemplate::SharedLayout(t) => &t.qualifier,
            ProductTemplate::Unsupported(_, octets) => octets,
            _ => &[],
        }
    }

    /// The ensemble a member forecast belongs to and which member it is:
    /// `(type of ensemble forecast, perturbation number)`, Code Table 4.6 and
    /// §4 octets 35-36 of templates 4.1 and 4.11, and the ensemble block of
    /// the chemistry and aerosol templates 4.41, 4.43, 4.45 and 4.47. `None`
    /// for every other template.
    pub fn ensemble_member(&self) -> Option<(u8, u8)> {
        match &self.template {
            ProductTemplate::EnsembleTimeInterval(t) => {
                Some((t.ensemble_type, t.perturbation_number))
            }
            ProductTemplate::Constituent(t) => {
                t.ensemble.map(|e| (e.ensemble_type, e.perturbation_number))
            }
            ProductTemplate::SharedLayout(t) if t.number == 1 => {
                Some((*t.qualifier.first()?, *t.qualifier.get(1)?))
            }
            _ => None,
        }
    }

    /// The derived forecast (Code Table 4.7, §4 octet 35) of templates 4.2 and
    /// 4.12: an unweighted mean, a spread, a probability-weighted mean… `None`
    /// for every other template.
    pub fn derived_forecast(&self) -> Option<u8> {
        match &self.template {
            ProductTemplate::SharedLayout(t) if matches!(t.number, 2 | 12) => {
                t.qualifier.first().copied()
            }
            _ => None,
        }
    }

    /// `(category, number)` parameter pair, when the template carries it.
    /// Every template this crate reads does; `None` for one it does not.
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
            ProductTemplate::SharedLayout(t) => Some(&t.common),
            ProductTemplate::Constituent(t) => Some(&t.common),
            ProductTemplate::Unsupported(..) => None,
        }
    }

    /// Borrow the time-statistics block, when the template has one.
    pub fn stats(&self) -> Option<&StatisticalProcessing> {
        match &self.template {
            ProductTemplate::HorizontalTimeInterval(t) => Some(&t.stats),
            ProductTemplate::EnsembleTimeInterval(t) => Some(&t.stats),
            ProductTemplate::SharedLayout(t) => t.stats.as_ref(),
            ProductTemplate::Constituent(t) => t.stats.as_ref(),
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
        // A template sharing 4.0's layout that is too short for it is kept as
        // unread octets rather than refused: before #959 every such template
        // opened, read or not.
        other => {
            if let Some(t) =
                shared_layout(other).and_then(|layout| parse_shared(other, layout, payload))
            {
                ProductTemplate::SharedLayout(Box::new(t))
            } else if let Some(t) = constituent_layout(other).and_then(|layout| {
                parse_constituent(other, &layout, payload, num_coordinate_values)
            }) {
                ProductTemplate::Constituent(Box::new(t))
            } else {
                // Too short for the layout it states, a chemistry or aerosol
                // template is kept as unread octets, as a shared one is.
                ProductTemplate::Unsupported(other, payload.into())
            }
        }
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
    let short = || {
        FieldglassError::Parse(format!(
            "PDS horizontal common block needs {HORIZONTAL_CORE_LEN} bytes, got {}",
            payload.len()
        ))
    };
    let (&[category, number, process], rest) = payload.split_first_chunk().ok_or_else(short)?;
    read_common(category, number, process, rest, ForecastTime::Signed32)
        .map(|(common, _)| common)
        .ok_or_else(short)
}

/// How a template writes its forecast time.
#[derive(Clone, Copy)]
enum ForecastTime {
    /// Four sign-magnitude octets, as every template but 4.44 does.
    Signed32,
    /// Two unsigned octets, as WMO's 4.44 does.
    Unsigned16,
}

/// 4.0's block from the background process (its octet 13) to the second fixed
/// surface, read from the start of `rest`, with the parameter and generating
/// process a template may keep elsewhere. Returns the block and how many
/// octets of `rest` it took; `None` if `rest` is shorter than that.
fn read_common(
    category: u8,
    number: u8,
    process: u8,
    rest: &[u8],
    forecast: ForecastTime,
) -> Option<(HorizontalProductCommon, usize)> {
    // Background and forecast process (2), cut-off hours and minutes (3) and
    // the unit (1) come before the forecast time.
    let at = 6;
    let (forecast_time, width) = match forecast {
        ForecastTime::Signed32 => (read_signed_i32(rest.get(at..at + 4)?), 4),
        ForecastTime::Unsigned16 => {
            let o = rest.get(at..at + 2)?;
            (i64::from(u16::from_be_bytes([o[0], o[1]])), 2)
        }
    };
    let first = at + width;
    let second = first + SURFACE_TRIPLE_LEN;
    let end = second + SURFACE_TRIPLE_LEN;
    let head = rest.get(..end)?;
    Some((
        HorizontalProductCommon {
            parameter_category: category,
            parameter_number: number,
            generating_process_type: process,
            background_process_id: head[0],
            forecast_process_id: head[1],
            obs_cutoff_hours: u16::from_be_bytes([head[2], head[3]]),
            obs_cutoff_minutes: head[4],
            forecast_time_unit: head[5],
            forecast_time,
            first_surface: read_fixed_surface(&head[first..second]),
            second_surface: read_fixed_surface(&head[second..end]),
        },
        end,
    ))
}

/// A size or wavelength range from the 11 octets at the start of `bytes`.
fn read_interval(bytes: &[u8]) -> Option<TypedInterval> {
    let b = bytes.get(..INTERVAL_LEN)?;
    Some(TypedInterval {
        interval_type: b[0],
        first: ScaledLimit {
            scale_factor: read_scale_factor(b[1]),
            scaled_value: read_scaled_value(&b[2..6]),
        },
        second: ScaledLimit {
            scale_factor: read_scale_factor(b[6]),
            scaled_value: read_scaled_value(&b[7..11]),
        },
    })
}

/// A chemistry or aerosol template, read as [`ConstituentTemplate`]; `None`
/// when the section is too short for the layout it states. `payload` runs from
/// octet 10 to the end of the section, coordinate values included, which
/// 4.44's length rule leaves out.
fn parse_constituent(
    number: u16,
    layout: &ConstituentLayout,
    payload: &[u8],
    num_coordinate_values: u16,
) -> Option<ConstituentTemplate> {
    let (&[category, parameter_number], rest) = payload.split_first_chunk()?;
    // 4.47 alone writes its generating process before the constituent.
    let (first_process, rest) = if layout.process_first {
        let (&process, rest) = rest.split_first()?;
        (Some(process), rest)
    } else {
        (None, rest)
    };
    let qualifier = rest.get(..layout.qualifier)?;
    let rest = &rest[layout.qualifier..];
    let (process, rest) = match first_process {
        Some(process) => (process, rest),
        None => {
            let (&process, rest) = rest.split_first()?;
            (process, rest)
        }
    };
    // WMO's 4.44 is 45 octets with a two-octet forecast time. eccodes reads a
    // 4.44 of any other length with a four-octet one, which it once wrote
    // (`template.4.forecast_time_44.def`, GRIB-29); so does this.
    let coordinates = 4 * usize::from(num_coordinate_values);
    let forecast = if number == 44 && payload.len().checked_sub(coordinates) == Some(45 - 9) {
        ForecastTime::Unsigned16
    } else {
        ForecastTime::Signed32
    };
    let (common, used) = read_common(category, parameter_number, process, rest, forecast)?;
    let mut rest = &rest[used..];
    let ensemble = if layout.ensemble {
        let (
            &[
                ensemble_type,
                perturbation_number,
                num_forecasts_in_ensemble,
            ],
            after,
        ) = rest.split_first_chunk()?;
        rest = after;
        Some(EnsembleBlock {
            ensemble_type,
            perturbation_number,
            num_forecasts_in_ensemble,
        })
    } else {
        None
    };
    let stats = if layout.statistical {
        Some(parse_statistics(rest).ok()?)
    } else {
        None
    };
    let (constituent, ranges) = qualifier.split_first_chunk::<CONSTITUENT_LEN>()?;
    Some(ConstituentTemplate {
        number,
        common,
        constituent_type: u16::from_be_bytes(*constituent),
        size: read_interval(ranges),
        wavelength: ranges.get(INTERVAL_LEN..).and_then(read_interval),
        ensemble,
        stats,
        qualifier: qualifier.into(),
    })
}

/// A template sharing 4.0's layout, read as [`SharedLayoutTemplate`]; `None`
/// when the section is too short for the layout it states.
fn parse_shared(
    number: u16,
    (block, statistical): (usize, bool),
    payload: &[u8],
) -> Option<SharedLayoutTemplate> {
    let common = parse_horizontal_common(payload).ok()?;
    let qualifier = payload.get(HORIZONTAL_CORE_LEN..HORIZONTAL_CORE_LEN + block)?;
    let stats = if statistical {
        Some(parse_statistics(payload.get(HORIZONTAL_CORE_LEN + block..)?).ok()?)
    } else {
        None
    };
    Some(SharedLayoutTemplate {
        number,
        common,
        qualifier: qualifier.into(),
        stats,
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
        // 0x80 is "−0" in sign-magnitude — normalize to +0 so it stays usable.
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
        assert_eq!(pds.ensemble_member(), Some((3, 5)));

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
        assert!(matches!(pds.template, ProductTemplate::Unsupported(99, _)));
        assert_eq!(pds.template_name(), "unsupported(4.99)");
        assert_eq!(pds.parameter(), None);
        assert!(pds.common().is_none());
        assert!(pds.stats().is_none());
        // The octets it holds are kept, so a caller can still tell two apart.
        assert_eq!(pds.unread_octets(), &bytes[9..]);
    }

    /// 4.0 with a different template number and `tail` appended.
    fn build_shared(number: u16, tail: &[u8]) -> Vec<u8> {
        let mut bytes = build_pds_4_0();
        bytes[7..9].copy_from_slice(&number.to_be_bytes());
        bytes.extend_from_slice(tail);
        let len = bytes.len() as u32;
        bytes[0..4].copy_from_slice(&len.to_be_bytes());
        bytes
    }

    /// A template sharing 4.0's layout is read through octet 34 as 4.0 is
    /// (#959): its parameter, surfaces and forecast time are the right
    /// octets, and its own block is kept as written.
    #[test]
    fn a_template_sharing_the_layout_reads_the_common_block() {
        let reference = parse_product_definition(&build_pds_4_0()).expect("4.0");
        // 4.1: ensemble type 3, member 7 of 20.
        let pds = parse_product_definition(&build_shared(1, &[3, 7, 20])).expect("4.1");
        assert!(matches!(pds.template, ProductTemplate::SharedLayout(_)));
        assert_eq!(pds.template_name(), "horizontal(4.1)");
        assert_eq!(pds.common(), reference.common());
        assert_eq!(pds.unread_octets(), &[3, 7, 20]);
        assert_eq!(pds.ensemble_member(), Some((3, 7)));
        assert_eq!(pds.derived_forecast(), None);
        assert!(pds.stats().is_none());
    }

    /// 4.12 carries 4.8's statistical block after its own two octets.
    #[test]
    fn a_statistical_template_sharing_the_layout_reads_its_interval() {
        let mut tail = vec![0, 20]; // derived forecast 0 (unweighted mean), 20 members
        push_be(&mut tail, 2026, 2);
        tail.extend_from_slice(&[1, 1, 6, 0, 0]); // end 2026-01-01T06:00:00
        tail.push(1); // one time range
        push_be(&mut tail, 0, 4); // no values missing
        tail.extend_from_slice(&[1, 2, 1]); // accumulation, successive, hours
        push_be(&mut tail, 6, 4);
        tail.push(1);
        push_be(&mut tail, 0, 4);
        let pds = parse_product_definition(&build_shared(12, &tail)).expect("4.12");
        assert_eq!(pds.unread_octets(), &[0, 20]);
        assert_eq!(pds.derived_forecast(), Some(0));
        assert_eq!(pds.ensemble_member(), None);
        let stats = pds.stats().expect("4.12 is statistical");
        assert_eq!(stats.end_time_iso8601(), "2026-01-01T06:00:00Z");
        assert_eq!(stats.specs.len(), 1);
        assert_eq!(
            (stats.specs[0].stat_process, stats.specs[0].stat_length),
            (1, 6)
        );
    }

    /// One too short for the layout it states still opens, as unread octets.
    #[test]
    fn a_shared_layout_template_too_short_for_it_is_kept_unread() {
        let pds = parse_product_definition(&build_shared(5, &[1, 2])).expect("opens");
        assert!(matches!(pds.template, ProductTemplate::Unsupported(5, _)));
        assert!(pds.common().is_none());
    }

    /// The constituent type every chemistry and aerosol test writes.
    const CONSTITUENT: [u8; 2] = [0xF2, 0x31]; // 62001, dust dry

    /// A size range: Code Table 4.91 type 2 (between), 2 × 10⁻⁷ to 2 × 10⁻⁶ m.
    const SIZE: [u8; 11] = [2, 7, 0, 0, 0, 2, 6, 0, 0, 0, 2];

    /// A wavelength range: type 7 (closed), 545 × 10⁻⁹ to 565 × 10⁻⁹ m, each
    /// value needing two octets of its four.
    const WAVELENGTH: [u8; 11] = [7, 9, 0, 0, 2, 0x21, 9, 0, 0, 2, 0x35];

    /// The ensemble block: type 3 (positively perturbed), member 7 of 20.
    const ENSEMBLE: [u8; 3] = [3, 7, 20];

    /// 4.0's octets 12-34 with a forecast time of `forecast` (four or two
    /// octets): process 2, background 3, model 96, cut-off 1 h 5 min, hours,
    /// then a layer from 500 hPa to 850 hPa in pascals.
    fn process_to_surfaces(forecast: &[u8]) -> Vec<u8> {
        let mut out = vec![2, 3, 96, 0, 1, 5, 1];
        out.extend_from_slice(forecast);
        out.extend_from_slice(&[100, 0]);
        push_be(&mut out, 50_000, 4);
        out.extend_from_slice(&[100, 0]);
        push_be(&mut out, 85_000, 4);
        out
    }

    /// 4.8's interval: one 6-hour accumulation ending 2026-01-01T06:00:00Z.
    fn interval() -> Vec<u8> {
        let mut out = Vec::new();
        push_be(&mut out, 2026, 2);
        out.extend_from_slice(&[1, 1, 6, 0, 0, 1]);
        push_be(&mut out, 0, 4);
        out.extend_from_slice(&[1, 2, 1]);
        push_be(&mut out, 6, 4);
        out.push(1);
        push_be(&mut out, 0, 4);
        out
    }

    /// A §4 of template `number` over `payload` (octets 10 on).
    fn section(number: u16, payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        push_be(&mut out, 9 + payload.len() as u64, 4);
        out.push(PDS_SECTION_NUMBER);
        push_be(&mut out, 0, 2);
        push_be(&mut out, u64::from(number), 2);
        out.extend_from_slice(payload);
        out
    }

    /// Template `number` laid out as WMO's Code Table 4.0 entry has it, with
    /// parameter 0.20.7 and the blocks above.
    fn build_constituent(number: u16) -> Vec<u8> {
        let forecast: &[u8] = if number == 44 { &[0, 9] } else { &[0, 0, 0, 9] };
        let mut payload = vec![20, 7];
        if number == 47 {
            // 4.47 alone writes its generating process at octet 12, before
            // the aerosol type, and 4.0's block resumes at the background
            // process.
            payload.push(2);
            payload.extend_from_slice(&CONSTITUENT);
            payload.extend_from_slice(&SIZE);
            payload.extend_from_slice(&process_to_surfaces(forecast)[1..]);
        } else {
            payload.extend_from_slice(&CONSTITUENT);
            if (44..=48).contains(&number) {
                payload.extend_from_slice(&SIZE);
            }
            if number == 48 {
                payload.extend_from_slice(&WAVELENGTH);
            }
            payload.extend_from_slice(&process_to_surfaces(forecast));
        }
        if matches!(number, 41 | 43 | 45 | 47) {
            payload.extend_from_slice(&ENSEMBLE);
        }
        if matches!(number, 42 | 43 | 46 | 47) {
            payload.extend_from_slice(&interval());
        }
        section(number, &payload)
    }

    /// The section length WMO's template states, for each of 4.40-4.48 with
    /// one time range: what the builder must produce, so it lays the
    /// templates out as the spec does rather than as it assumed.
    #[test]
    fn the_builder_lays_each_template_out_at_its_wmo_length() {
        for (number, len) in [
            (40, 36),
            (41, 39),
            (42, 60),
            (43, 63),
            (44, 45),
            (45, 50),
            (46, 71),
            (47, 74),
            (48, 58),
        ] {
            assert_eq!(build_constituent(number).len(), len, "4.{number}");
        }
    }

    /// Every chemistry and aerosol template reads its parameter, constituent,
    /// ranges, forecast time and both fixed surfaces from the spec's octets
    /// (#1130), and its ensemble block and interval where it has them.
    #[test]
    fn each_chemistry_and_aerosol_template_reads_its_own_layout() {
        for number in 40..=48u16 {
            let pds = parse_product_definition(&build_constituent(number))
                .unwrap_or_else(|e| panic!("4.{number}: {e}"));
            let ProductTemplate::Constituent(t) = &pds.template else {
                panic!("4.{number} parsed as {:?}", pds.template);
            };
            assert_eq!(t.number, number);
            assert_eq!(pds.parameter(), Some((20, 7)), "4.{number}");
            let c = pds.common().expect("a common block");
            assert_eq!(
                (c.generating_process_type, c.background_process_id),
                (2, 3),
                "4.{number}"
            );
            assert_eq!(c.forecast_process_id, 96, "4.{number}");
            assert_eq!((c.obs_cutoff_hours, c.obs_cutoff_minutes), (1, 5));
            assert_eq!(
                (c.forecast_time_unit, c.forecast_time),
                (1, 9),
                "4.{number}"
            );
            assert_eq!(c.first_surface.surface_type, 100, "4.{number}");
            assert_eq!(c.first_surface.value(), Some(50_000.0), "4.{number}");
            assert_eq!(c.second_surface.value(), Some(85_000.0), "4.{number}");
            assert_eq!(t.constituent_type, 62001, "4.{number}");

            let aerosol = (44..=48).contains(&number);
            assert_eq!(t.size.is_some(), aerosol, "4.{number}");
            if let Some(size) = t.size {
                assert_eq!(size.interval_type, 2);
                assert_eq!(size.first.scale_factor, Some(7));
                assert_eq!(size.first.scaled_value, Some(2));
                assert!((size.second.value().unwrap() - 2e-6).abs() < 1e-18);
            }
            match t.wavelength {
                Some(w) => {
                    assert_eq!(number, 48);
                    assert_eq!(w.interval_type, 7);
                    assert_eq!(w.first.scaled_value, Some(545));
                    assert_eq!(w.second.scaled_value, Some(565));
                    assert!((w.first.value().unwrap() - 545e-9).abs() < 1e-18);
                }
                None => assert_ne!(number, 48),
            }
            let qualifier_len = match number {
                40..=43 => 2,
                48 => 24,
                _ => 13,
            };
            assert_eq!(t.qualifier.len(), qualifier_len, "4.{number}");
            assert_eq!(&t.qualifier[..2], &CONSTITUENT);
            assert!(pds.unread_octets().is_empty(), "4.{number} is read whole");

            let member = matches!(number, 41 | 43 | 45 | 47);
            assert_eq!(
                pds.ensemble_member(),
                member.then_some((3, 7)),
                "4.{number}"
            );
            if member {
                assert_eq!(t.ensemble.map(|e| e.num_forecasts_in_ensemble), Some(20));
            }
            let statistical = matches!(number, 42 | 43 | 46 | 47);
            match pds.stats() {
                Some(s) => {
                    assert!(statistical, "4.{number} has no interval");
                    assert_eq!(s.end_time_iso8601(), "2026-01-01T06:00:00Z");
                    assert_eq!((s.specs[0].stat_process, s.specs[0].stat_length), (1, 6));
                }
                None => assert!(!statistical, "4.{number} lost its interval"),
            }
            let kind = if aerosol { "aerosol" } else { "chemical" };
            assert_eq!(pds.template_name(), format!("{kind}(4.{number})"));
        }
    }

    /// 4.47 read as 4.46 is laid out, as eccodes does, would take the
    /// generating process as the aerosol type's high octet. Read as the spec
    /// lays it out, the type is the same as every other template's.
    #[test]
    fn template_4_47_keeps_its_generating_process_at_octet_12() {
        let bytes = build_constituent(47);
        // §4 octet 12 is byte 11.
        assert_eq!(bytes[11], 2, "the generating process");
        assert_eq!(&bytes[12..14], &CONSTITUENT);
        let pds = parse_product_definition(&bytes).expect("4.47");
        let ProductTemplate::Constituent(t) = &pds.template else {
            panic!("4.47 parsed as {:?}", pds.template);
        };
        assert_eq!(t.constituent_type, 62001);
        assert_eq!(t.common.generating_process_type, 2);
        assert_eq!(t.size.map(|s| s.interval_type), Some(2));
    }

    /// WMO's 4.44 states its forecast time in two unsigned octets. One written
    /// with four, as eccodes once wrote it, is two octets longer, and is read
    /// as eccodes reads it.
    #[test]
    fn template_4_44_reads_two_or_four_octet_forecast_times() {
        let spec = build_constituent(44);
        // Octets 32-33 (bytes 31-32) are the forecast time, two unsigned
        // octets: 0xFFFF is 65535, not a sign-magnitude −32767.
        assert_eq!(&spec[31..33], &[0, 9]);
        let mut wide = spec.clone();
        wide[31..33].copy_from_slice(&[0xFF, 0xFF]);
        let pds = parse_product_definition(&wide).expect("4.44");
        assert_eq!(pds.common().map(|c| c.forecast_time), Some(65_535));

        // Widened to four sign-magnitude octets, −9.
        let mut legacy = spec[..31].to_vec();
        legacy.extend_from_slice(&[0x80, 0, 0, 9]);
        legacy.extend_from_slice(&spec[33..]);
        let len = legacy.len() as u32;
        legacy[0..4].copy_from_slice(&len.to_be_bytes());
        let pds = parse_product_definition(&legacy).expect("legacy 4.44");
        let c = pds.common().expect("a common block");
        assert_eq!(c.forecast_time, -9);
        assert_eq!(c.first_surface.value(), Some(50_000.0));
        assert_eq!(c.second_surface.value(), Some(85_000.0));
    }

    /// Coordinate values after a 4.44 do not make it the four-octet form: the
    /// length rule is the template's own, as eccodes' `section4Length - 4*NV`.
    #[test]
    fn coordinate_values_do_not_change_how_4_44_is_read() {
        let mut bytes = build_constituent(44);
        bytes[5..7].copy_from_slice(&2u16.to_be_bytes());
        bytes.extend_from_slice(&[0x41, 0x20, 0, 0, 0x41, 0xA0, 0, 0]);
        let len = bytes.len() as u32;
        bytes[0..4].copy_from_slice(&len.to_be_bytes());
        let pds = parse_product_definition(&bytes).expect("4.44 with NV = 2");
        assert_eq!(pds.common().map(|c| c.forecast_time), Some(9));
        assert_eq!(
            pds.common().map(|c| c.first_surface.value()),
            Some(Some(50_000.0))
        );
    }

    /// A chemistry or aerosol template cut short anywhere still opens, as
    /// unread octets, without reading past the section: the same as a
    /// template sharing 4.0's layout.
    #[test]
    fn a_truncated_constituent_template_is_kept_unread() {
        for number in 40..=48u16 {
            let full = build_constituent(number);
            for len in 9..full.len() {
                let mut bytes = full[..len].to_vec();
                bytes[0..4].copy_from_slice(&(len as u32).to_be_bytes());
                let pds = parse_product_definition(&bytes)
                    .unwrap_or_else(|e| panic!("4.{number} cut to {len}: {e}"));
                if !matches!(pds.template, ProductTemplate::Unsupported(n, _) if n == number) {
                    panic!("4.{number} cut to {len} parsed as {:?}", pds.template);
                }
                assert!(pds.common().is_none());
                assert_eq!(pds.unread_octets(), &bytes[9..]);
            }
        }
    }
}
