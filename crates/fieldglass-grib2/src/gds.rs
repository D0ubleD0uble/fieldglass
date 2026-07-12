//! GRIB2 Grid Definition Section (§3).
//!
//! Implements the templates that cover the bulk of operational GRIB2 traffic:
//! 3.0 (regular latitude/longitude), 3.1 (rotated latitude/longitude), 3.10
//! (Mercator), 3.20 (polar stereographic), 3.30 (Lambert Conformal), 3.40
//! (Gaussian latitude/longitude — both regular and the reduced variant), and
//! 3.90 (space view perspective / geostationary).
//!
//! Spec reference: WMO Manual on Codes Vol I.2 (FM 92 GRIB Edition 2),
//! Section 3 layout + Templates 3.0 / 3.1 / 3.10 / 3.20 / 3.30 / 3.40 / 3.90.

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

/// Read a 4-byte big-endian IEEE-754 single-precision float — used by the
/// rotation sub-template's angle-of-rotation field.
fn read_ieee_f32(bytes: &[u8]) -> f32 {
    f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// Read a GRIB2 scale-factor / scaled-value pair into a physical quantity:
/// `value · 10^(-scale)`. The 1-byte scale factor is sign-magnitude (high bit
/// is the sign); the all-ones sentinel in either field means "not given" and
/// yields `None`.
fn read_scaled(scale_byte: u8, value_bytes: &[u8]) -> Option<f64> {
    let raw = u32::from_be_bytes([
        value_bytes[0],
        value_bytes[1],
        value_bytes[2],
        value_bytes[3],
    ]);
    if scale_byte == 0xFF || raw == U32_MISSING {
        return None;
    }
    // The scale factor is a 1-byte sign-magnitude integer — reuse the shared
    // GRIB decode of that convention.
    let scale = sign_magnitude_to_i64(scale_byte as u32, 8) as i32;
    Some(raw as f64 * 10f64.powi(-scale))
}

/// Resolve the GRIB2 §3 shape-of-earth group (the first 16 payload octets,
/// section octets 15-30) into `(r_eq, r_pol)` in **metres**. Handles WMO Code
/// Table 3.2: the fixed spheres/ellipsoids and the producer-specified radius /
/// axes codes (1, 3, 7) read from the scaled-value octets. Unknown or
/// unresolvable codes fall back to the WMO mean sphere so geolocation never
/// silently uses a zero radius.
fn resolve_earth_shape(p: &[u8]) -> (f64, f64) {
    const MEAN_SPHERE_M: f64 = 6_371_229.0;
    // Spherical-radius pair (octet 16 scale, 17-20 value) and the major/minor
    // axis pairs (octets 21 / 22-25 and 26 / 27-30).
    let spherical = || read_scaled(p[1], &p[2..6]).unwrap_or(MEAN_SPHERE_M);
    let major_m = || read_scaled(p[6], &p[7..11]);
    let minor_m = || read_scaled(p[11], &p[12..16]);
    let major_km = || major_m().map(|v| v * 1000.0);
    let minor_km = || minor_m().map(|v| v * 1000.0);
    match p[0] {
        0 => (6_367_470.0, 6_367_470.0),
        1 => {
            let r = spherical();
            (r, r)
        }
        2 => (6_378_160.0, 6_356_775.0), // IAU 1965
        3 => (
            major_km().unwrap_or(6_378_137.0),
            minor_km().unwrap_or(6_356_752.314),
        ),
        4 => (6_378_137.0, 6_356_752.314),     // IAG-GRS80
        5 => (6_378_137.0, 6_356_752.314_245), // WGS84
        6 => (MEAN_SPHERE_M, MEAN_SPHERE_M),
        7 => (
            major_m().unwrap_or(6_378_137.0),
            minor_m().unwrap_or(6_356_752.314),
        ),
        8 => (6_371_200.0, 6_371_200.0),
        9 => (6_377_563.396, 6_356_256.909), // OSGB 1936 / Airy 1830
        _ => (MEAN_SPHERE_M, MEAN_SPHERE_M),
    }
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

/// Template 3.1 — rotated latitude/longitude. Shares the 3.0 latitude/
/// longitude layout, then appends the projection's southern-pole position
/// and an IEEE angle of rotation (COSMO, DWD/ECMWF limited-area runs).
#[derive(Debug, Clone, Copy)]
pub struct RotatedLatLonTemplate {
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
    /// Latitude of the southern pole of projection (degrees).
    pub south_pole_lat: f64,
    /// Longitude of the southern pole of projection (degrees).
    pub south_pole_lon: f64,
    /// Angle of rotation of the projection about the new polar axis (degrees).
    pub angle_of_rotation: f64,
}

/// Template 3.10 — Mercator projection. Grid lengths Di/Dj are in metres at
/// the intersection latitude `lad` (occasionally seen in oceanographic
/// products).
#[derive(Debug, Clone, Copy)]
pub struct MercatorTemplate {
    pub shape_of_earth: u8,
    pub ni: u32,
    pub nj: u32,
    pub la1: f64,
    pub lo1: f64,
    /// Latitude at which the projection intersects the Earth — where Di and
    /// Dj are specified.
    pub lad: f64,
    pub la2: f64,
    pub lo2: f64,
    /// Orientation of the grid: angle between i-direction and the equator.
    pub orientation: f64,
    pub di_metres: f64,
    pub dj_metres: f64,
    pub resolution_flags: u8,
    pub scanning_mode: u8,
}

/// Template 3.20 — polar stereographic projection (NCEP NDGD analyses,
/// sea-ice products).
#[derive(Debug, Clone, Copy)]
pub struct PolarStereographicTemplate {
    pub shape_of_earth: u8,
    /// Radius of the sphere to project on, resolved from the earth-shape fields.
    pub earth_radius_m: f64,
    pub nx: u32,
    pub ny: u32,
    pub la1: f64,
    pub lo1: f64,
    /// Latitude where Dx and Dy are specified.
    pub lad: f64,
    /// Orientation of the grid — longitude of the meridian parallel to the
    /// y-axis (LoV).
    pub lov: f64,
    pub dx_metres: f64,
    pub dy_metres: f64,
    pub resolution_flags: u8,
    pub projection_centre: u8,
    /// `true` when the south pole is on the projection plane (projection-centre
    /// flag bit 1 set); `false` → north pole.
    pub south_pole: bool,
    pub scanning_mode: u8,
}

/// Template 3.90 — space view perspective / orthographic (geostationary
/// satellite imagery). Carries the sub-satellite point and camera geometry
/// rather than corner lat/lon, so it has no `bounds()`.
#[derive(Debug, Clone, Copy)]
pub struct SpaceViewTemplate {
    pub shape_of_earth: u8,
    /// Ellipsoid semi-major / semi-minor axes in metres, resolved from the
    /// shape-of-earth group. Geostationary geolocation is ellipsoidal (GOES
    /// uses GRS80, Meteosat WGS84), unlike the spherical projectors.
    pub r_eq: f64,
    pub r_pol: f64,
    pub nx: u32,
    pub ny: u32,
    /// Latitude of the sub-satellite point (degrees).
    pub lap: f64,
    /// Longitude of the sub-satellite point (degrees).
    pub lop: f64,
    /// Apparent diameter of the Earth in grid lengths, X- and Y-direction.
    pub dx: u32,
    pub dy: u32,
    /// X/Y coordinate of the sub-satellite point, in grid lengths (the raw
    /// 10⁻³-grid-length integers divided down to whole grid lengths).
    pub xp: f64,
    pub yp: f64,
    /// Orientation of the grid (degrees).
    pub orientation: f64,
    /// Altitude of the camera from the Earth's centre, in units of the
    /// Earth's radius × 10⁶; `None` for the all-ones missing sentinel.
    pub nr: Option<u32>,
    /// X/Y coordinate of the origin of the sector image, in grid lengths.
    pub xo: u32,
    pub yo: u32,
    pub resolution_flags: u8,
    pub scanning_mode: u8,
}

/// Template 3.30 — Lambert Conformal projection.
#[derive(Debug, Clone, Copy)]
pub struct LambertTemplate {
    pub shape_of_earth: u8,
    /// Radius of the sphere to project on, resolved from the earth-shape fields.
    pub earth_radius_m: f64,
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
    RotatedLatLon(RotatedLatLonTemplate),
    Mercator(MercatorTemplate),
    PolarStereographic(PolarStereographicTemplate),
    Lambert(LambertTemplate),
    Gaussian(GaussianTemplate),
    SpaceView(SpaceViewTemplate),
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
            GridTemplate::RotatedLatLon(t) => Some((t.ni, t.nj)),
            GridTemplate::Mercator(t) => Some((t.ni, t.nj)),
            GridTemplate::PolarStereographic(t) => Some((t.nx, t.ny)),
            GridTemplate::Lambert(t) => Some((t.nx, t.ny)),
            GridTemplate::Gaussian(t) => t.ni.map(|ni| (ni, t.nj)),
            GridTemplate::SpaceView(t) => Some((t.nx, t.ny)),
            GridTemplate::Unsupported(_) => None,
        }
    }

    /// `(la1, lo1, la2, lo2)` corner coordinates in degrees, when the
    /// template defines them. The projection grids that lack an explicit last
    /// grid point — Lambert and polar stereographic — return
    /// `(la1, lo1, lad, lov)`, the natural projection parameters that pair
    /// with the metadata the napi layer surfaces. Space view carries no
    /// corner coordinates (only a sub-satellite point) and returns `None`.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        match &self.template {
            GridTemplate::LatLon(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::RotatedLatLon(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::Mercator(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::PolarStereographic(t) => Some((t.la1, t.lo1, t.lad, t.lov)),
            GridTemplate::Lambert(t) => Some((t.la1, t.lo1, t.lad, t.lov)),
            GridTemplate::Gaussian(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::SpaceView(_) => None,
            GridTemplate::Unsupported(_) => None,
        }
    }

    /// Short human-readable name of the template (e.g. `"latlon"`,
    /// `"lambert"`, `"gaussian"`, `"unsupported(N)"`).
    pub fn template_name(&self) -> String {
        match &self.template {
            GridTemplate::LatLon(_) => "latlon".to_string(),
            GridTemplate::RotatedLatLon(_) => "rotated_latlon".to_string(),
            GridTemplate::Mercator(_) => "mercator".to_string(),
            GridTemplate::PolarStereographic(_) => "polar_stereo".to_string(),
            GridTemplate::Lambert(_) => "lambert".to_string(),
            GridTemplate::Gaussian(_) => "gaussian".to_string(),
            GridTemplate::SpaceView(_) => "space_view".to_string(),
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
        1 => GridTemplate::RotatedLatLon(parse_template_3_1(payload)?),
        10 => GridTemplate::Mercator(parse_template_3_10(payload)?),
        20 => GridTemplate::PolarStereographic(parse_template_3_20(payload)?),
        30 => GridTemplate::Lambert(parse_template_3_30(payload)?),
        40 => GridTemplate::Gaussian(parse_template_3_40(payload, optional_list_octet_size)?),
        90 => GridTemplate::SpaceView(parse_template_3_90(payload)?),
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

/// Radius of the sphere to project a planar grid on, in metres.
///
/// Derived from [`resolve_earth_shape`], so it inherits that function's handling
/// of the producer-specified shapes and of a missing scaled value. A spherical
/// shape has `a == b` and the mean is exactly the declared radius.
///
/// An oblate shape is an approximation: eccodes projects those on the true
/// spheroid, while these projections are spherical, so we take the spheroid's
/// mean radius `(2a + b) / 3` — within ~0.1 % of the true figure, and far closer
/// than ignoring the declared shape. True ellipsoidal projection is a follow-up.
fn earth_radius_from_shape(p: &[u8]) -> f64 {
    let (major, minor) = resolve_earth_shape(p);
    (2.0 * major + minor) / 3.0
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

/// Template 3.1 payload — the 58-byte 3.0 latitude/longitude block plus a
/// 12-byte rotation suffix (southern-pole lat/lon + IEEE angle of rotation).
/// Total payload length = 70 bytes (octets 15..=84 of the section).
fn parse_template_3_1(p: &[u8]) -> Result<RotatedLatLonTemplate, FieldglassError> {
    if p.len() < 70 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.1 needs 70 bytes of payload, got {}",
            p.len()
        )));
    }
    let base = parse_template_3_0(p)?;
    Ok(RotatedLatLonTemplate {
        shape_of_earth: base.shape_of_earth,
        ni: base.ni,
        nj: base.nj,
        la1: base.la1,
        lo1: base.lo1,
        la2: base.la2,
        lo2: base.lo2,
        di: base.di,
        dj: base.dj,
        resolution_flags: base.resolution_flags,
        scanning_mode: base.scanning_mode,
        south_pole_lat: read_lat_degrees(&p[58..62]),
        south_pole_lon: read_lon_degrees(&p[62..66]),
        angle_of_rotation: read_ieee_f32(&p[66..70]) as f64,
    })
}

/// Template 3.10 payload starts at GDS octet 15. Payload length = 58 bytes
/// (octets 15..=72 of the section). Unlike 3.0 there are no basic-angle /
/// subdivision fields: La1/Lo1 follow Ni/Nj directly.
fn parse_template_3_10(p: &[u8]) -> Result<MercatorTemplate, FieldglassError> {
    if p.len() < 58 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.10 needs 58 bytes of payload, got {}",
            p.len()
        )));
    }
    Ok(MercatorTemplate {
        shape_of_earth: p[0],
        ni: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        nj: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        la1: read_lat_degrees(&p[24..28]),
        lo1: read_lat_degrees(&p[28..32]),
        resolution_flags: p[32],
        lad: read_lat_degrees(&p[33..37]),
        la2: read_lat_degrees(&p[37..41]),
        lo2: read_lat_degrees(&p[41..45]),
        scanning_mode: p[45],
        orientation: read_lon_degrees(&p[46..50]),
        di_metres: read_metre_increment(&p[50..54]),
        dj_metres: read_metre_increment(&p[54..58]),
    })
}

/// Template 3.20 payload starts at GDS octet 15. Payload length = 51 bytes
/// (octets 15..=65 of the section).
fn parse_template_3_20(p: &[u8]) -> Result<PolarStereographicTemplate, FieldglassError> {
    if p.len() < 51 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.20 needs 51 bytes of payload, got {}",
            p.len()
        )));
    }
    let projection_centre = p[49];
    Ok(PolarStereographicTemplate {
        shape_of_earth: p[0],
        earth_radius_m: earth_radius_from_shape(p),
        nx: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        ny: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        la1: read_lat_degrees(&p[24..28]),
        lo1: read_lon_degrees(&p[28..32]),
        resolution_flags: p[32],
        lad: read_lat_degrees(&p[33..37]),
        lov: read_lat_degrees(&p[37..41]),
        dx_metres: read_metre_increment(&p[41..45]),
        dy_metres: read_metre_increment(&p[45..49]),
        projection_centre,
        // WMO bit 1 (most significant) of the projection-centre flag: set
        // means the south pole is on the projection plane.
        south_pole: projection_centre & 0x80 != 0,
        scanning_mode: p[50],
    })
}

/// Template 3.90 payload starts at GDS octet 15. Payload length = 66 bytes
/// (octets 15..=80 of the section).
fn parse_template_3_90(p: &[u8]) -> Result<SpaceViewTemplate, FieldglassError> {
    if p.len() < 66 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.90 needs 66 bytes of payload, got {}",
            p.len()
        )));
    }
    let (r_eq, r_pol) = resolve_earth_shape(p);
    Ok(SpaceViewTemplate {
        shape_of_earth: p[0],
        r_eq,
        r_pol,
        nx: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        ny: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        lap: read_lat_degrees(&p[24..28]),
        lop: read_lat_degrees(&p[28..32]),
        resolution_flags: p[32],
        dx: u32::from_be_bytes([p[33], p[34], p[35], p[36]]),
        dy: u32::from_be_bytes([p[37], p[38], p[39], p[40]]),
        // Xp/Yp are 10⁻³-grid-length integers; scale down to grid lengths.
        xp: u32::from_be_bytes([p[41], p[42], p[43], p[44]]) as f64 / 1.0e3,
        yp: u32::from_be_bytes([p[45], p[46], p[47], p[48]]) as f64 / 1.0e3,
        scanning_mode: p[49],
        orientation: read_lat_degrees(&p[50..54]),
        nr: read_u32_or_missing(&p[54..58]),
        xo: u32::from_be_bytes([p[58], p[59], p[60], p[61]]),
        yo: u32::from_be_bytes([p[62], p[63], p[64], p[65]]),
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
        earth_radius_m: earth_radius_from_shape(p),
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

    /// Append the 16-byte shape-of-earth block (sphere R = 6371229 m).
    fn push_shape_of_earth(buf: &mut Vec<u8>) {
        buf.push(6);
        buf.extend_from_slice(&[0u8; 15]);
    }

    /// Wrap a template payload in a minimal §3 header (source 0, the given
    /// number of data points, no optional list, the given template number).
    fn wrap_gds(template_number: u16, num_points: u32, payload: &[u8]) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        push_be(&mut buf, (14 + payload.len()) as u32, 4);
        buf.push(3);
        buf.push(0); // source
        push_be(&mut buf, num_points, 4);
        buf.push(0); // optional list octet size
        buf.push(0); // optional list interpretation
        push_be(&mut buf, template_number as u32, 2);
        buf.extend_from_slice(payload);
        buf
    }

    #[test]
    fn template_3_1_round_trips_rotation_suffix() {
        let mut p: Vec<u8> = Vec::new();
        push_shape_of_earth(&mut p);
        push_be(&mut p, 16, 4); // Ni
        push_be(&mut p, 31, 4); // Nj
        push_be(&mut p, 0, 4); // basic angle
        push_be(&mut p, 0, 4); // subdivisions
        p.extend_from_slice(&signed_lat_bytes(60_000_000)); // La1 = 60°
        push_be(&mut p, 0, 4); // Lo1 = 0°
        p.push(0x30); // resolution flags (i/j increments given)
        p.extend_from_slice(&signed_lat_bytes(0)); // La2 = 0°
        push_be(&mut p, 30_000_000, 4); // Lo2 = 30°
        push_be(&mut p, 2_000_000, 4); // Di = 2°
        push_be(&mut p, 2_000_000, 4); // Dj = 2°
        p.push(0); // scanning mode
        // Rotation suffix
        p.extend_from_slice(&signed_lat_bytes(-30_000_000)); // S-pole lat = -30°
        push_be(&mut p, 10_000_000, 4); // S-pole lon = 10°
        p.extend_from_slice(&15.0f32.to_be_bytes()); // angle of rotation = 15°
        assert_eq!(p.len(), 70);

        let bytes = wrap_gds(1, 16 * 31, &p);
        let gds = parse_grid_definition(&bytes).expect("parse 3.1");
        assert_eq!(gds.template_number, 1);
        let t = match gds.template {
            GridTemplate::RotatedLatLon(t) => t,
            other => panic!("expected RotatedLatLon, got {other:?}"),
        };
        assert_eq!((t.ni, t.nj), (16, 31));
        assert!((t.la1 - 60.0).abs() < 1e-9);
        assert!((t.lo2 - 30.0).abs() < 1e-9);
        assert_eq!(t.di, Some(2.0));
        assert_eq!(t.dj, Some(2.0));
        assert!((t.south_pole_lat - (-30.0)).abs() < 1e-9);
        assert!((t.south_pole_lon - 10.0).abs() < 1e-9);
        assert!((t.angle_of_rotation - 15.0).abs() < 1e-6);
        assert_eq!(gds.dimensions(), Some((16, 31)));
        assert_eq!(gds.bounds(), Some((60.0, 0.0, 0.0, 30.0)));
        assert_eq!(gds.template_name(), "rotated_latlon");
    }

    #[test]
    fn template_3_10_round_trips_mercator() {
        let mut p: Vec<u8> = Vec::new();
        push_shape_of_earth(&mut p);
        push_be(&mut p, 360, 4); // Ni
        push_be(&mut p, 181, 4); // Nj
        p.extend_from_slice(&signed_lat_bytes(-80_000_000)); // La1 = -80°
        push_be(&mut p, 0, 4); // Lo1 = 0°
        p.push(0x30); // resolution flags
        p.extend_from_slice(&signed_lat_bytes(20_000_000)); // LaD = 20°
        p.extend_from_slice(&signed_lat_bytes(80_000_000)); // La2 = 80°
        push_be(&mut p, 359_000_000, 4); // Lo2 = 359°
        p.push(64); // scanning mode (j scans positively)
        push_be(&mut p, 0, 4); // orientation = 0°
        push_be(&mut p, 25_000_000, 4); // Di = 25000 m
        push_be(&mut p, 25_000_000, 4); // Dj = 25000 m
        assert_eq!(p.len(), 58);

        let bytes = wrap_gds(10, 360 * 181, &p);
        let gds = parse_grid_definition(&bytes).expect("parse 3.10");
        let t = match gds.template {
            GridTemplate::Mercator(t) => t,
            other => panic!("expected Mercator, got {other:?}"),
        };
        assert_eq!((t.ni, t.nj), (360, 181));
        assert!((t.la1 - (-80.0)).abs() < 1e-9);
        assert!((t.lad - 20.0).abs() < 1e-9);
        assert!((t.la2 - 80.0).abs() < 1e-9);
        assert!((t.lo2 - 359.0).abs() < 1e-9);
        assert!((t.di_metres - 25_000.0).abs() < 1e-6);
        assert!((t.dj_metres - 25_000.0).abs() < 1e-6);
        assert_eq!(gds.dimensions(), Some((360, 181)));
        assert_eq!(gds.template_name(), "mercator");
    }

    #[test]
    fn template_3_20_round_trips_polar_stereo_south_pole_flag() {
        let mut p: Vec<u8> = Vec::new();
        push_shape_of_earth(&mut p);
        push_be(&mut p, 512, 4); // Nx
        push_be(&mut p, 512, 4); // Ny
        p.extend_from_slice(&signed_lat_bytes(-20_000_000)); // La1 = -20°
        push_be(&mut p, 225_000_000, 4); // Lo1 = 225°
        p.push(0x08); // resolution flags
        p.extend_from_slice(&signed_lat_bytes(-60_000_000)); // LaD = -60°
        push_be(&mut p, 100_000_000, 4); // LoV = 100°
        push_be(&mut p, 12_700_000, 4); // Dx = 12700 m
        push_be(&mut p, 12_700_000, 4); // Dy = 12700 m
        p.push(0x80); // projection centre: south pole on plane
        p.push(64); // scanning mode
        assert_eq!(p.len(), 51);

        let bytes = wrap_gds(20, 512 * 512, &p);
        let gds = parse_grid_definition(&bytes).expect("parse 3.20");
        let t = match gds.template {
            GridTemplate::PolarStereographic(t) => t,
            other => panic!("expected PolarStereographic, got {other:?}"),
        };
        assert_eq!((t.nx, t.ny), (512, 512));
        assert!((t.la1 - (-20.0)).abs() < 1e-9);
        assert!((t.lad - (-60.0)).abs() < 1e-9);
        assert!((t.lov - 100.0).abs() < 1e-9);
        assert!((t.dx_metres - 12_700.0).abs() < 1e-6);
        assert!(t.south_pole, "projection-centre bit 1 set → south pole");
        assert_eq!(gds.dimensions(), Some((512, 512)));
        // Polar stereo borrows Lambert's (la1, lo1, lad, lov) bounds shape.
        assert_eq!(gds.bounds(), Some((-20.0, 225.0, -60.0, 100.0)));
        assert_eq!(gds.template_name(), "polar_stereo");
    }

    #[test]
    fn template_3_90_round_trips_space_view() {
        let mut p: Vec<u8> = Vec::new();
        push_shape_of_earth(&mut p);
        push_be(&mut p, 3712, 4); // Nx
        push_be(&mut p, 3712, 4); // Ny
        p.extend_from_slice(&signed_lat_bytes(0)); // Lap = 0°
        push_be(&mut p, 0, 4); // Lop = 0°
        p.push(0); // resolution flags
        push_be(&mut p, 3622, 4); // dx (grid lengths)
        push_be(&mut p, 3622, 4); // dy (grid lengths)
        push_be(&mut p, 1_856_000, 4); // Xp = 1856.0 grid lengths
        push_be(&mut p, 1_856_000, 4); // Yp = 1856.0 grid lengths
        p.push(0); // scanning mode
        p.extend_from_slice(&signed_lat_bytes(180_000_000)); // orientation = 180°
        push_be(&mut p, 6_610_710, 4); // Nr
        push_be(&mut p, 0, 4); // Xo
        push_be(&mut p, 0, 4); // Yo
        assert_eq!(p.len(), 66);

        let bytes = wrap_gds(90, 3712 * 3712, &p);
        let gds = parse_grid_definition(&bytes).expect("parse 3.90");
        let t = match gds.template {
            GridTemplate::SpaceView(t) => t,
            other => panic!("expected SpaceView, got {other:?}"),
        };
        assert_eq!((t.nx, t.ny), (3712, 3712));
        assert_eq!(t.dx, 3622);
        assert!((t.xp - 1856.0).abs() < 1e-6);
        assert!((t.orientation - 180.0).abs() < 1e-9);
        assert_eq!(t.nr, Some(6_610_710));
        // Shape code 6 → WMO mean sphere (r_eq == r_pol).
        assert!((t.r_eq - 6_371_229.0).abs() < 1e-3);
        assert!((t.r_pol - 6_371_229.0).abs() < 1e-3);
        assert_eq!(gds.dimensions(), Some((3712, 3712)));
        // Space view carries only a sub-satellite point — no corner bounds.
        assert_eq!(gds.bounds(), None);
        assert_eq!(gds.template_name(), "space_view");
    }

    #[test]
    fn earth_radius_matches_the_declared_shape() {
        // The radius the planar projections actually use. Getting these wrong by
        // one part in 1700 misplaces a continental grid by kilometres (#271), so
        // pin each spherical shape to its exact declared value.
        let shape = |code: u8| {
            let mut p = vec![code];
            p.extend_from_slice(&[0xFFu8; 15]); // scaled values all "missing"
            earth_radius_from_shape(&p)
        };
        assert_eq!(shape(0), 6_367_470.0, "shape 0 sphere");
        assert_eq!(shape(6), 6_371_229.0, "shape 6 sphere (the WMO default)");
        assert_eq!(shape(8), 6_371_200.0, "shape 8 sphere");
        // An oblate shape collapses to the spheroid's mean radius, which must sit
        // between the two axes rather than outside them.
        let wgs84 = shape(5);
        assert!(
            (6_356_752.0..6_378_137.0).contains(&wgs84),
            "WGS84 mean radius {wgs84} is not between the axes"
        );
        // A producer-specified shape whose scaled value is *missing* must not
        // yield a nonsense radius — `resolve_earth_shape` falls back, and the
        // radius stays a plausible Earth.
        for code in [1u8, 3, 7] {
            let r = shape(code);
            assert!(
                (6_300_000.0..6_400_000.0).contains(&r),
                "shape {code} with a missing scaled value gave radius {r}"
            );
        }
    }

    #[test]
    fn resolve_earth_shape_handles_fixed_and_specified_codes() {
        // Code 6 → WMO mean sphere.
        let mut sphere = vec![6u8];
        sphere.extend_from_slice(&[0u8; 15]);
        assert_eq!(resolve_earth_shape(&sphere), (6_371_229.0, 6_371_229.0));

        // Code 5 → WGS84 ellipsoid (oblate).
        let mut wgs84 = vec![5u8];
        wgs84.extend_from_slice(&[0u8; 15]);
        let (a, b) = resolve_earth_shape(&wgs84);
        assert!((a - 6_378_137.0).abs() < 1e-3 && b < a);

        // Code 7 → oblate, axes specified in metres via the scaled-value octets.
        // major = 6378137 m (scale 0), minor = 6356752 m (scale 0).
        let mut p = vec![0u8; 16];
        p[0] = 7;
        p[6] = 0; // major scale factor
        p[7..11].copy_from_slice(&6_378_137u32.to_be_bytes());
        p[11] = 0; // minor scale factor
        p[12..16].copy_from_slice(&6_356_752u32.to_be_bytes());
        let (a, b) = resolve_earth_shape(&p);
        assert!((a - 6_378_137.0).abs() < 1e-3, "r_eq = {a}");
        assert!((b - 6_356_752.0).abs() < 1e-3, "r_pol = {b}");

        // Code 1 → spherical radius specified in metres (octets 16-20),
        // here with a scale factor of 1 (value 63712290 · 10⁻¹).
        let mut p1 = vec![0u8; 16];
        p1[0] = 1;
        p1[1] = 1; // scale factor
        p1[2..6].copy_from_slice(&63_712_290u32.to_be_bytes());
        let (a, b) = resolve_earth_shape(&p1);
        assert!((a - 6_371_229.0).abs() < 1e-3 && (a - b).abs() < 1e-9);
    }

    #[test]
    fn template_3_90_missing_nr_sentinel_yields_none() {
        // 16-byte shape block + 50 bytes of template fields (p[16..66]).
        let mut p: Vec<u8> = Vec::new();
        push_shape_of_earth(&mut p);
        p.extend_from_slice(&[0u8; 50]);
        assert_eq!(p.len(), 66);
        // Nr occupies p[54..58]; set it to the all-ones missing sentinel.
        p[54..58].copy_from_slice(&U32_MISSING.to_be_bytes());

        let bytes = wrap_gds(90, 0, &p);
        let gds = parse_grid_definition(&bytes).expect("parse 3.90");
        let t = match gds.template {
            GridTemplate::SpaceView(t) => t,
            _ => unreachable!(),
        };
        assert_eq!(t.nr, None);
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
