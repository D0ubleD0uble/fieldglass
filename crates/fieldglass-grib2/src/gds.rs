//! GRIB2 Grid Definition Section (§3).
//!
//! Implements the three templates that cover most operational GRIB2 traffic:
//! 3.0 (regular latitude/longitude), 3.30 (Lambert Conformal), and 3.40
//! (Gaussian latitude/longitude — both regular and the reduced variant).
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 3 layout + Templates 3.0 / 3.30 / 3.40.

use crate::section::{SectionHeader, parse_section_header};
use fieldglass_core::{FieldglassError, bits::sign_magnitude_to_i64};

/// Section number for the Grid Definition Section.
pub const GDS_SECTION_NUMBER: u8 = 3;

/// Sentinel value used by GRIB2 to mark a 4-byte unsigned field as "missing".
pub const U32_MISSING: u32 = 0xFFFF_FFFF;

/// Convert a 4-byte signed-magnitude latitude (μdegrees) → degrees.
fn read_lat_degrees(bytes: &[u8]) -> f64 {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    sign_magnitude_to_i64(raw, 32) as f64 / 1.0e6
}

/// Convert a 4-byte unsigned longitude (μdegrees, 0..=360e6) → degrees.
fn read_lon_degrees(bytes: &[u8]) -> f64 {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    raw as f64 / 1.0e6
}

/// Convert a 4-byte unsigned angular increment (μdegrees) → degrees, with
/// the GRIB2 "all-ones" sentinel mapped to `None`.
fn read_increment_degrees(bytes: &[u8]) -> Option<f64> {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if raw == U32_MISSING {
        None
    } else {
        Some(raw as f64 / 1.0e6)
    }
}

/// Convert a 4-byte unsigned linear increment (10⁻³ m, used by projection
/// grids like Lambert) → metres.
fn read_metre_increment(bytes: &[u8]) -> f64 {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    raw as f64 / 1.0e3
}

fn read_u32_or_missing(bytes: &[u8]) -> Option<u32> {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if raw == U32_MISSING { None } else { Some(raw) }
}

/// Template 3.0 — regular latitude/longitude (equidistant cylindrical).
#[derive(Debug, Clone, Copy)]
pub struct LatLonTemplate {
    pub shape_of_earth: u8,
    pub ni: u32,
    pub nj: u32,
    pub la1: f64,
    pub lo1: f64,
    pub la2: f64,
    pub lo2: f64,
    pub di: Option<f64>,
    pub dj: Option<f64>,
    pub resolution_flags: u8,
    pub scanning_mode: u8,
}

/// Template 3.30 — Lambert Conformal projection.
#[derive(Debug, Clone, Copy)]
pub struct LambertTemplate {
    pub shape_of_earth: u8,
    pub nx: u32,
    pub ny: u32,
    pub la1: f64,
    pub lo1: f64,
    /// Latitude where Dx and Dy are specified.
    pub lad: f64,
    /// Longitude of meridian parallel to y-axis.
    pub lov: f64,
    pub dx_metres: f64,
    pub dy_metres: f64,
    pub latin1: f64,
    pub latin2: f64,
    pub resolution_flags: u8,
    pub projection_centre: u8,
    pub scanning_mode: u8,
}

/// Template 3.40 — Gaussian latitude/longitude (regular or reduced).
#[derive(Debug, Clone, Copy)]
pub struct GaussianTemplate {
    pub shape_of_earth: u8,
    /// `None` for reduced grids — the row width varies and lives in the
    /// optional list of numbers at the end of the section.
    pub ni: Option<u32>,
    pub nj: u32,
    pub la1: f64,
    pub lo1: f64,
    pub la2: f64,
    pub lo2: f64,
    /// `None` for reduced grids (no constant Di).
    pub di: Option<f64>,
    /// Number of parallels between a pole and the equator.
    pub n_parallels: u32,
    pub resolution_flags: u8,
    pub scanning_mode: u8,
    /// True if the section carries a non-empty optional list of numbers,
    /// indicating a reduced (per-row) grid.
    pub is_reduced: bool,
}

/// Parsed template payload. Templates outside the supported set surface as
/// `Unsupported` so callers can still expose section-header fields and a
/// useful name without erroring out.
#[derive(Debug, Clone, Copy)]
pub enum GridTemplate {
    LatLon(LatLonTemplate),
    Lambert(LambertTemplate),
    Gaussian(GaussianTemplate),
    Unsupported(u16),
}

/// Parsed contents of the Grid Definition Section.
#[derive(Debug, Clone, Copy)]
pub struct GridDefinitionSection {
    pub section_length: u32,
    pub source: u8,
    pub num_data_points: u32,
    pub optional_list_octet_size: u8,
    pub optional_list_interp: u8,
    pub template_number: u16,
    pub template: GridTemplate,
}

impl GridDefinitionSection {
    /// `(ni, nj)` if the template carries explicit dimensions. Reduced
    /// Gaussian grids return `None` because Ni varies per row.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match &self.template {
            GridTemplate::LatLon(t) => Some((t.ni, t.nj)),
            GridTemplate::Lambert(t) => Some((t.nx, t.ny)),
            GridTemplate::Gaussian(t) => t.ni.map(|ni| (ni, t.nj)),
            GridTemplate::Unsupported(_) => None,
        }
    }

    /// `(la1, lo1, la2, lo2)` corner coordinates in degrees, when the
    /// template defines them. Lambert returns `(la1, lo1, lad, lov)` —
    /// the natural projection corners that pair with the Lambert metadata
    /// the napi layer surfaces.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        match &self.template {
            GridTemplate::LatLon(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::Lambert(t) => Some((t.la1, t.lo1, t.lad, t.lov)),
            GridTemplate::Gaussian(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::Unsupported(_) => None,
        }
    }

    /// Short human-readable name of the template (e.g. `"latlon"`,
    /// `"lambert"`, `"gaussian"`, `"unsupported(N)"`).
    pub fn template_name(&self) -> String {
        match &self.template {
            GridTemplate::LatLon(_) => "latlon".to_string(),
            GridTemplate::Lambert(_) => "lambert".to_string(),
            GridTemplate::Gaussian(_) => "gaussian".to_string(),
            GridTemplate::Unsupported(n) => format!("unsupported(3.{n})"),
        }
    }
}

/// Parse the Grid Definition Section starting at `bytes[0]`.
pub fn parse_grid_definition(bytes: &[u8]) -> Result<GridDefinitionSection, FieldglassError> {
    let header = parse_section_header(bytes)?;
    parse_grid_definition_with_header(bytes, header)
}

/// Variant for callers that have already read the section header.
pub fn parse_grid_definition_with_header(
    bytes: &[u8],
    header: SectionHeader,
) -> Result<GridDefinitionSection, FieldglassError> {
    if header.number != GDS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "expected GDS (section {GDS_SECTION_NUMBER}), got section {}",
            header.number
        )));
    }
    let len = header.length as usize;
    // The shortest possible §3 has 14 fixed octets (header through template
    // number) before any template payload — short of that we can't read the
    // template number safely.
    if len < 14 {
        return Err(FieldglassError::Parse(format!(
            "GDS section length {len} is below the 14-byte minimum"
        )));
    }
    if bytes.len() < len {
        return Err(FieldglassError::Parse(format!(
            "GDS declares length {len} but only {} bytes available",
            bytes.len()
        )));
    }

    let source = bytes[5];
    let num_data_points = u32::from_be_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
    let optional_list_octet_size = bytes[10];
    let optional_list_interp = bytes[11];
    let template_number = u16::from_be_bytes([bytes[12], bytes[13]]);

    // Template payload starts at octet 15 (= byte index 14).
    let payload = &bytes[14..len];
    let template = match template_number {
        0 => GridTemplate::LatLon(parse_template_3_0(payload)?),
        30 => GridTemplate::Lambert(parse_template_3_30(payload)?),
        40 => GridTemplate::Gaussian(parse_template_3_40(payload, optional_list_octet_size)?),
        other => GridTemplate::Unsupported(other),
    };

    Ok(GridDefinitionSection {
        section_length: header.length,
        source,
        num_data_points,
        optional_list_octet_size,
        optional_list_interp,
        template_number,
        template,
    })
}

/// Template 3.0 payload starts at GDS octet 15 (= `payload[0]`).
/// Total payload length = 58 bytes (octets 15..=72 of the section).
fn parse_template_3_0(p: &[u8]) -> Result<LatLonTemplate, FieldglassError> {
    if p.len() < 58 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.0 needs 58 bytes of payload, got {}",
            p.len()
        )));
    }
    Ok(LatLonTemplate {
        shape_of_earth: p[0],
        ni: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        nj: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        la1: read_lat_degrees(&p[32..36]),
        lo1: read_lon_degrees(&p[36..40]),
        resolution_flags: p[40],
        la2: read_lat_degrees(&p[41..45]),
        lo2: read_lon_degrees(&p[45..49]),
        di: read_increment_degrees(&p[49..53]),
        dj: read_increment_degrees(&p[53..57]),
        scanning_mode: p[57],
    })
}

/// Template 3.30 payload starts at GDS octet 15. Payload length = 67 bytes
/// (octets 15..=81 of the section).
fn parse_template_3_30(p: &[u8]) -> Result<LambertTemplate, FieldglassError> {
    if p.len() < 67 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.30 needs 67 bytes of payload, got {}",
            p.len()
        )));
    }
    Ok(LambertTemplate {
        shape_of_earth: p[0],
        nx: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        ny: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        la1: read_lat_degrees(&p[24..28]),
        lo1: read_lon_degrees(&p[28..32]),
        resolution_flags: p[32],
        lad: read_lat_degrees(&p[33..37]),
        lov: read_lon_degrees(&p[37..41]),
        dx_metres: read_metre_increment(&p[41..45]),
        dy_metres: read_metre_increment(&p[45..49]),
        projection_centre: p[49],
        scanning_mode: p[50],
        latin1: read_lat_degrees(&p[51..55]),
        latin2: read_lat_degrees(&p[55..59]),
        // Octets 74..=81 of the section (= payload[59..=66]) carry the
        // southern-pole lat/lon for the projection — surfaced via the
        // raw payload length but not needed for grid rendering.
    })
}

/// Template 3.40 payload starts at GDS octet 15. Payload length = 58 bytes
/// (octets 15..=72 of the section).
fn parse_template_3_40(
    p: &[u8],
    optional_list_octet_size: u8,
) -> Result<GaussianTemplate, FieldglassError> {
    if p.len() < 58 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.40 needs 58 bytes of payload, got {}",
            p.len()
        )));
    }
    let is_reduced = optional_list_octet_size > 0;
    Ok(GaussianTemplate {
        shape_of_earth: p[0],
        ni: read_u32_or_missing(&p[16..20]),
        nj: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        la1: read_lat_degrees(&p[32..36]),
        lo1: read_lon_degrees(&p[36..40]),
        resolution_flags: p[40],
        la2: read_lat_degrees(&p[41..45]),
        lo2: read_lon_degrees(&p[45..49]),
        di: read_increment_degrees(&p[49..53]),
        n_parallels: u32::from_be_bytes([p[53], p[54], p[55], p[56]]),
        scanning_mode: p[57],
        is_reduced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Append `n` unsigned big-endian bytes encoding `v` into `buf`.
    fn push_be(buf: &mut Vec<u8>, v: u32, width: usize) {
        let bytes = v.to_be_bytes();
        buf.extend_from_slice(&bytes[(4 - width)..]);
    }

    fn signed_lat_bytes(lat_micro: i32) -> [u8; 4] {
        let mag = lat_micro.unsigned_abs();
        let raw = if lat_micro < 0 {
            mag | 0x8000_0000
        } else {
            mag
        };
        raw.to_be_bytes()
    }

    /// Build a minimum §3 with template 3.0, lat/lon corners, and Di/Dj.
    fn build_gds_3_0() -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        // Section header: length placeholder + section number 3
        push_be(&mut buf, 72, 4);
        buf.push(3);
        // source, num_data_points, optional list size/interp, template
        buf.push(0);
        push_be(&mut buf, 144 * 73, 4);
        buf.push(0);
        buf.push(0);
        push_be(&mut buf, 0, 2); // template number 0
        // Payload (58 bytes)
        buf.push(6); // shape of earth = sphere R = 6371229 m
        buf.extend_from_slice(&[0u8; 15]); // earth-shape parameters (ignored)
        push_be(&mut buf, 144, 4); // Ni
        push_be(&mut buf, 73, 4); // Nj
        push_be(&mut buf, 0, 4); // basic angle
        push_be(&mut buf, 0, 4); // subdivisions
        buf.extend_from_slice(&signed_lat_bytes(90_000_000)); // La1 = 90°
        push_be(&mut buf, 0, 4); // Lo1 = 0°
        buf.push(0); // resolution flags
        buf.extend_from_slice(&signed_lat_bytes(-90_000_000)); // La2 = -90°
        push_be(&mut buf, 357_500_000, 4); // Lo2 = 357.5°
        push_be(&mut buf, 2_500_000, 4); // Di = 2.5°
        push_be(&mut buf, 2_500_000, 4); // Dj = 2.5°
        buf.push(0); // scanning mode
        assert_eq!(buf.len(), 72);
        buf
    }

    #[test]
    fn template_3_0_round_trips_synthesized_payload() {
        let bytes = build_gds_3_0();
        let gds = parse_grid_definition(&bytes).expect("parse 3.0");
        assert_eq!(gds.template_number, 0);
        assert_eq!(gds.num_data_points, 144 * 73);
        let t = match gds.template {
            GridTemplate::LatLon(t) => t,
            _ => panic!("expected LatLon"),
        };
        assert_eq!(t.ni, 144);
        assert_eq!(t.nj, 73);
        assert!((t.la1 - 90.0).abs() < 1e-9);
        assert!((t.la2 - (-90.0)).abs() < 1e-9);
        assert!((t.lo2 - 357.5).abs() < 1e-9);
        assert_eq!(t.di, Some(2.5));
        assert_eq!(t.dj, Some(2.5));
        assert_eq!(gds.dimensions(), Some((144, 73)));
        assert_eq!(gds.template_name(), "latlon");
    }

    #[test]
    fn template_3_0_handles_negative_latitude_via_sign_magnitude() {
        let mut bytes = build_gds_3_0();
        // La1 is at section octets 47–50 = bytes 46–49.
        let neg = 0x8000_0000u32 | 45_000_000;
        bytes[46..50].copy_from_slice(&neg.to_be_bytes());
        let gds = parse_grid_definition(&bytes).expect("parse");
        let t = match gds.template {
            GridTemplate::LatLon(t) => t,
            _ => unreachable!(),
        };
        assert!((t.la1 - (-45.0)).abs() < 1e-9);
    }

    #[test]
    fn template_3_0_increment_missing_sentinel_yields_none() {
        let mut bytes = build_gds_3_0();
        // Di is at section octets 64–67 = bytes 63–66.
        bytes[63..67].copy_from_slice(&U32_MISSING.to_be_bytes());
        let gds = parse_grid_definition(&bytes).expect("parse");
        let t = match gds.template {
            GridTemplate::LatLon(t) => t,
            _ => unreachable!(),
        };
        assert_eq!(t.di, None);
    }

    #[test]
    fn rejects_short_buffer() {
        let bytes = [0u8; 10];
        assert!(parse_grid_definition(&bytes).is_err());
    }

    #[test]
    fn rejects_wrong_section_number() {
        let mut bytes = build_gds_3_0();
        bytes[4] = 4; // claim §4
        assert!(parse_grid_definition(&bytes).is_err());
    }

    #[test]
    fn unsupported_template_round_trips_with_label() {
        let mut bytes = build_gds_3_0();
        // Template number lives at section octets 13–14 = bytes 12–13.
        bytes[12..14].copy_from_slice(&99u16.to_be_bytes());
        let gds = parse_grid_definition(&bytes).expect("parse");
        assert!(matches!(gds.template, GridTemplate::Unsupported(99)));
        assert_eq!(gds.template_name(), "unsupported(3.99)");
        assert_eq!(gds.dimensions(), None);
        assert_eq!(gds.bounds(), None);
    }
}
