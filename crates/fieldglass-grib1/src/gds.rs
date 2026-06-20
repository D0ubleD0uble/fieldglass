use fieldglass_core::{
    FieldglassError, LambertParams, LambertProjector, PlanarGridProjector, PolarStereoParams,
    PolarStereoProjector, bits::ibm_float_to_f64,
};

// ---------------------------------------------------------------------------
// Flag bytes
// ---------------------------------------------------------------------------

/// Resolution and component flags — WMO ON388 Code Table 7 (GDS octet 17).
pub struct ResolutionFlags {
    /// True if Di/Dj increments are given in the GDS.
    pub increments_given: bool,
    /// True if earth is oblate spheroid; false = spherical (radius 6367.47 km).
    pub earth_oblate: bool,
    /// True if u/v vector components are resolved relative to the grid (i,j)
    /// rather than to geographic east/north.
    pub uv_relative_to_grid: bool,
}

impl ResolutionFlags {
    fn from_byte(b: u8) -> Self {
        Self {
            increments_given: b & 0x80 != 0,
            earth_oblate: b & 0x40 != 0,
            uv_relative_to_grid: b & 0x08 != 0,
        }
    }
}

/// Scanning mode flags — WMO ON388 Flag Table 8 (GDS octet 28).
pub struct ScanningMode {
    /// True = points scan in −i direction (east→west); false = west→east.
    pub i_negative: bool,
    /// True = points scan in +j direction (south→north); false = north→south.
    pub j_positive: bool,
    /// True = adjacent points are consecutive in j (column-major); false = row-major.
    pub j_consecutive: bool,
}

impl ScanningMode {
    fn from_byte(b: u8) -> Self {
        Self {
            i_negative: b & 0x80 != 0,
            j_positive: b & 0x40 != 0,
            j_consecutive: b & 0x20 != 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-projection structs
// ---------------------------------------------------------------------------

/// Grid type 0 — Latitude/Longitude (equidistant cylindrical / Plate Carrée).
pub struct LatLonGrid {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
    /// East-west increment in degrees.
    pub di: f64,
    /// North-south increment in degrees.
    pub dj: f64,
    pub resolution_flags: ResolutionFlags,
    pub scanning_mode: ScanningMode,
}

/// Grid type 10 — Rotated Latitude/Longitude.
///
/// A regular lat/lon grid expressed in a *rotated* coordinate frame whose south
/// pole sits at (`south_pole_lat`, `south_pole_lon`). The grid body is identical
/// to [`LatLonGrid`]; the rotated-pole position and rotation angle follow the
/// scanning-mode octet (after four reserved octets). The corner coordinates
/// (`lat_first`/`lon_first`/`lat_last`/`lon_last`) are in the rotated frame —
/// converting them to geographic is the reprojector's job, not the parser's.
pub struct RotatedLatLonGrid {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
    /// East-west increment in degrees.
    pub di: f64,
    /// North-south increment in degrees.
    pub dj: f64,
    /// Geographic latitude of the rotated grid's south pole (degrees).
    pub south_pole_lat: f64,
    /// Geographic longitude of the rotated grid's south pole (degrees).
    pub south_pole_lon: f64,
    /// Angle of rotation about the new polar axis (degrees).
    pub angle_of_rotation: f64,
    pub resolution_flags: ResolutionFlags,
    pub scanning_mode: ScanningMode,
}

/// Grid type 4 — Gaussian Latitude/Longitude.
pub struct GaussianGrid {
    pub ni: u32,
    pub nj: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
    /// East-west increment in degrees (may be absent; check resolution_flags).
    pub di: f64,
    /// Number of Gaussian latitudes between pole and equator.
    pub n_gaussians: u16,
    pub resolution_flags: ResolutionFlags,
    pub scanning_mode: ScanningMode,
}

/// Grid type 5 — Polar Stereographic.
pub struct PolarStereoGrid {
    pub nx: u32,
    pub ny: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    /// Orientation longitude — meridian parallel to y-axis (degrees).
    pub lov: f64,
    /// Grid spacing in x at the 60° parallel (metres).
    pub dx_m: u32,
    /// Grid spacing in y at the 60° parallel (metres).
    pub dy_m: u32,
    /// True = South Pole on projection plane; false = North Pole.
    pub south_pole: bool,
    pub resolution_flags: ResolutionFlags,
    pub scanning_mode: ScanningMode,
}

impl PolarStereoGrid {
    /// Geographic `(lat, lon)` of the last scanned grid point — the corner
    /// diagonally opposite the origin.
    ///
    /// GRIB1 polar-stereographic GDS encodes only the first point (La1/Lo1);
    /// unlike a lat/lon grid there is no La2/Lo2 to read. The opposite corner
    /// is recovered by forward-projecting the origin to plane metres, stepping
    /// `(Nx-1)·Dx` / `(Ny-1)·Dy`, and inverse-projecting back to lat/lon.
    fn last_point(&self) -> (f64, f64) {
        let projector = PolarStereoProjector::new(PolarStereoParams {
            ni: self.nx,
            nj: self.ny,
            lat_first: self.lat_first,
            lon_first: self.lon_first,
            lov: self.lov,
            // GRIB1 polar stereo fixes the latitude of true scale at ±60°
            // (there is no LaD field); the projector takes the magnitude.
            lad: 60.0,
            dx_metres: self.dx_m as f64,
            dy_metres: self.dy_m as f64,
            south_pole: self.south_pole,
        });
        let (lat, lon) = projector.last_grid_point_lonlat();
        // The inverse is `lov + atan2(..)` and can land outside [-180, 180]
        // (e.g. lov=247 yields ~328°); normalise so the reported corner is
        // consistent with the first point's longitude convention.
        (lat, normalise_longitude(lon))
    }
}

/// Grid type 3 — Lambert Conformal (conic or bi-polar).
pub struct LambertGrid {
    pub nx: u32,
    pub ny: u32,
    pub lat_first: f64,
    pub lon_first: f64,
    /// Orientation longitude (degrees).
    pub lov: f64,
    /// Grid spacing in x (metres).
    pub dx_m: u32,
    /// Grid spacing in y (metres).
    pub dy_m: u32,
    /// True = South Pole on projection plane; false = North Pole.
    pub south_pole: bool,
    /// First standard parallel (degrees).
    pub latin1: f64,
    /// Second standard parallel (degrees).
    pub latin2: f64,
    /// Southern pole latitude for oblique projection (degrees).
    pub lat_south_pole: f64,
    /// Southern pole longitude for oblique projection (degrees).
    pub lon_south_pole: f64,
    pub resolution_flags: ResolutionFlags,
    pub scanning_mode: ScanningMode,
}

impl LambertGrid {
    /// Geographic `(lat, lon)` of the last scanned grid point — the corner
    /// diagonally opposite the origin.
    ///
    /// Like polar stereographic, a GRIB1 Lambert GDS encodes only the first
    /// point; the opposite corner is recovered from the projection. `LaD`
    /// (latitude of true scale) is taken as the first standard parallel,
    /// matching how the warp path builds [`LambertParams`].
    fn last_point(&self) -> (f64, f64) {
        let projector = LambertProjector::new(LambertParams {
            ni: self.nx,
            nj: self.ny,
            lat_first: self.lat_first,
            lon_first: self.lon_first,
            lad: self.latin1,
            lov: self.lov,
            dx_metres: self.dx_m as f64,
            dy_metres: self.dy_m as f64,
            latin1: self.latin1,
            latin2: self.latin2,
        });
        let (lat, lon) = projector.last_grid_point_lonlat();
        (lat, normalise_longitude(lon))
    }
}

// ---------------------------------------------------------------------------
// Top-level enum
// ---------------------------------------------------------------------------

pub enum GridDescription {
    LatLon(LatLonGrid),
    RotatedLatLon(RotatedLatLonGrid),
    Gaussian(GaussianGrid),
    PolarStereographic(PolarStereoGrid),
    LambertConformal(LambertGrid),
    /// Grid type present but not yet supported by this parser.
    Unsupported {
        grid_type: u8,
    },
}

impl GridDescription {
    pub fn grid_type_name(&self) -> &'static str {
        match self {
            Self::LatLon(_) => "latlon",
            Self::RotatedLatLon(_) => "rotated_latlon",
            Self::Gaussian(_) => "gaussian",
            Self::PolarStereographic(_) => "polar_stereo",
            Self::LambertConformal(_) => "lambert",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// Ni × Nj grid dimensions, if available.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match self {
            Self::LatLon(g) => Some((g.ni, g.nj)),
            Self::RotatedLatLon(g) => Some((g.ni, g.nj)),
            Self::Gaussian(g) => Some((g.ni, g.nj)),
            Self::PolarStereographic(g) => Some((g.nx, g.ny)),
            Self::LambertConformal(g) => Some((g.nx, g.ny)),
            Self::Unsupported { .. } => None,
        }
    }

    /// Geographic bounds as (lat_first, lon_first, lat_last, lon_last), if available.
    ///
    /// For [`Self::RotatedLatLon`] these are the corner coordinates in the
    /// rotated frame, not geographic; unrotating them is the reprojector's job.
    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        match self {
            Self::LatLon(g) => Some((g.lat_first, g.lon_first, g.lat_last, g.lon_last)),
            Self::RotatedLatLon(g) => Some((g.lat_first, g.lon_first, g.lat_last, g.lon_last)),
            Self::Gaussian(g) => Some((g.lat_first, g.lon_first, g.lat_last, g.lon_last)),
            Self::PolarStereographic(g) => {
                let (lat_last, lon_last) = g.last_point();
                Some((g.lat_first, g.lon_first, lat_last, lon_last))
            }
            Self::LambertConformal(g) => {
                let (lat_last, lon_last) = g.last_point();
                Some((g.lat_first, g.lon_first, lat_last, lon_last))
            }
            Self::Unsupported { .. } => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// Parse the Grid Description Section starting at `bytes[0]`.
/// `bytes` should begin at the first byte of the GDS (the section-length octet).
pub fn parse_grid_description(bytes: &[u8]) -> Result<GridDescription, FieldglassError> {
    if bytes.len() < 6 {
        return Err(FieldglassError::Parse(format!(
            "GDS too short for header: {} bytes",
            bytes.len()
        )));
    }

    let section_len = read_u24(&bytes[0..3]) as usize;
    if bytes.len() < section_len {
        return Err(FieldglassError::Parse(format!(
            "GDS section_len {section_len} exceeds available bytes {}",
            bytes.len()
        )));
    }

    let grid_type = bytes[5];

    match grid_type {
        0 => Ok(GridDescription::LatLon(parse_latlon(
            &bytes[..section_len],
        )?)),
        3 => Ok(GridDescription::LambertConformal(parse_lambert(
            &bytes[..section_len],
        )?)),
        4 => Ok(GridDescription::Gaussian(parse_gaussian(
            &bytes[..section_len],
        )?)),
        10 => Ok(GridDescription::RotatedLatLon(parse_rotated_latlon(
            &bytes[..section_len],
        )?)),
        5 => Ok(GridDescription::PolarStereographic(parse_polar_stereo(
            &bytes[..section_len],
        )?)),
        _ => Ok(GridDescription::Unsupported { grid_type }),
    }
}

// ---------------------------------------------------------------------------
// Per-type parsers (all offsets are 0-indexed from start of GDS)
// ---------------------------------------------------------------------------

fn parse_latlon(b: &[u8]) -> Result<LatLonGrid, FieldglassError> {
    require_len(b, 28, "LatLon GDS")?;
    Ok(LatLonGrid {
        ni: u16::from_be_bytes([b[6], b[7]]) as u32,
        nj: u16::from_be_bytes([b[8], b[9]]) as u32,
        lat_first: read_signed_magnitude_24(&b[10..13]) as f64 / 1000.0,
        lon_first: read_signed_magnitude_24(&b[13..16]) as f64 / 1000.0,
        resolution_flags: ResolutionFlags::from_byte(b[16]),
        lat_last: read_signed_magnitude_24(&b[17..20]) as f64 / 1000.0,
        lon_last: read_signed_magnitude_24(&b[20..23]) as f64 / 1000.0,
        di: u16::from_be_bytes([b[23], b[24]]) as f64 / 1000.0,
        dj: u16::from_be_bytes([b[25], b[26]]) as f64 / 1000.0,
        scanning_mode: ScanningMode::from_byte(b[27]),
    })
}

fn parse_rotated_latlon(b: &[u8]) -> Result<RotatedLatLonGrid, FieldglassError> {
    // Octets 7-28 are the lat/lon body; 29-32 are reserved; 33-35 / 36-38 hold
    // the rotated south pole (sign-magnitude, /1000); 39-42 the rotation angle
    // (IBM single-precision float). 0-indexed, the angle ends at byte 42.
    require_len(b, 42, "Rotated LatLon GDS")?;
    Ok(RotatedLatLonGrid {
        ni: u16::from_be_bytes([b[6], b[7]]) as u32,
        nj: u16::from_be_bytes([b[8], b[9]]) as u32,
        lat_first: read_signed_magnitude_24(&b[10..13]) as f64 / 1000.0,
        lon_first: read_signed_magnitude_24(&b[13..16]) as f64 / 1000.0,
        resolution_flags: ResolutionFlags::from_byte(b[16]),
        lat_last: read_signed_magnitude_24(&b[17..20]) as f64 / 1000.0,
        lon_last: read_signed_magnitude_24(&b[20..23]) as f64 / 1000.0,
        di: u16::from_be_bytes([b[23], b[24]]) as f64 / 1000.0,
        dj: u16::from_be_bytes([b[25], b[26]]) as f64 / 1000.0,
        scanning_mode: ScanningMode::from_byte(b[27]),
        south_pole_lat: read_signed_magnitude_24(&b[32..35]) as f64 / 1000.0,
        south_pole_lon: read_signed_magnitude_24(&b[35..38]) as f64 / 1000.0,
        angle_of_rotation: ibm_float_to_f64(read_u32(&b[38..42])),
    })
}

fn parse_gaussian(b: &[u8]) -> Result<GaussianGrid, FieldglassError> {
    require_len(b, 28, "Gaussian GDS")?;
    Ok(GaussianGrid {
        ni: u16::from_be_bytes([b[6], b[7]]) as u32,
        nj: u16::from_be_bytes([b[8], b[9]]) as u32,
        lat_first: read_signed_magnitude_24(&b[10..13]) as f64 / 1000.0,
        lon_first: read_signed_magnitude_24(&b[13..16]) as f64 / 1000.0,
        resolution_flags: ResolutionFlags::from_byte(b[16]),
        lat_last: read_signed_magnitude_24(&b[17..20]) as f64 / 1000.0,
        lon_last: read_signed_magnitude_24(&b[20..23]) as f64 / 1000.0,
        di: u16::from_be_bytes([b[23], b[24]]) as f64 / 1000.0,
        n_gaussians: u16::from_be_bytes([b[25], b[26]]),
        scanning_mode: ScanningMode::from_byte(b[27]),
    })
}

fn parse_polar_stereo(b: &[u8]) -> Result<PolarStereoGrid, FieldglassError> {
    require_len(b, 28, "Polar Stereo GDS")?;
    Ok(PolarStereoGrid {
        nx: u16::from_be_bytes([b[6], b[7]]) as u32,
        ny: u16::from_be_bytes([b[8], b[9]]) as u32,
        lat_first: read_signed_magnitude_24(&b[10..13]) as f64 / 1000.0,
        lon_first: read_signed_magnitude_24(&b[13..16]) as f64 / 1000.0,
        resolution_flags: ResolutionFlags::from_byte(b[16]),
        lov: read_signed_magnitude_24(&b[17..20]) as f64 / 1000.0,
        dx_m: read_u24(&b[20..23]),
        dy_m: read_u24(&b[23..26]),
        south_pole: b[26] & 0x80 != 0,
        scanning_mode: ScanningMode::from_byte(b[27]),
    })
}

fn parse_lambert(b: &[u8]) -> Result<LambertGrid, FieldglassError> {
    require_len(b, 40, "Lambert GDS")?;
    Ok(LambertGrid {
        nx: u16::from_be_bytes([b[6], b[7]]) as u32,
        ny: u16::from_be_bytes([b[8], b[9]]) as u32,
        lat_first: read_signed_magnitude_24(&b[10..13]) as f64 / 1000.0,
        lon_first: read_signed_magnitude_24(&b[13..16]) as f64 / 1000.0,
        resolution_flags: ResolutionFlags::from_byte(b[16]),
        lov: read_signed_magnitude_24(&b[17..20]) as f64 / 1000.0,
        dx_m: read_u24(&b[20..23]),
        dy_m: read_u24(&b[23..26]),
        south_pole: b[26] & 0x80 != 0,
        scanning_mode: ScanningMode::from_byte(b[27]),
        latin1: read_signed_magnitude_24(&b[28..31]) as f64 / 1000.0,
        latin2: read_signed_magnitude_24(&b[31..34]) as f64 / 1000.0,
        lat_south_pole: read_signed_magnitude_24(&b[34..37]) as f64 / 1000.0,
        lon_south_pole: read_signed_magnitude_24(&b[37..40]) as f64 / 1000.0,
    })
}

// ---------------------------------------------------------------------------
// Byte-reading helpers
// ---------------------------------------------------------------------------

/// Read a 3-byte big-endian unsigned integer.
fn read_u24(b: &[u8]) -> u32 {
    u32::from_be_bytes([0, b[0], b[1], b[2]])
}

/// Read a 4-byte big-endian unsigned integer.
fn read_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// Read a 3-byte big-endian sign-and-magnitude integer.
/// GRIB1 latitude, longitude, and orientation values are encoded with bit 23
/// as the sign flag (1 = negative) and bits 22..0 as the unsigned magnitude —
/// this is NOT two's-complement. Decoding `0x815f90` (sign + 90000) as two's
/// complement yields a bogus `-8298608`; sign-magnitude yields the correct
/// `-90000`.
fn read_signed_magnitude_24(b: &[u8]) -> i32 {
    let raw = read_u24(b);
    let magnitude = (raw & 0x7f_ffff) as i32;
    if raw & 0x80_0000 != 0 {
        -magnitude
    } else {
        magnitude
    }
}

/// Wrap a longitude in degrees into the half-open range (-180, 180].
fn normalise_longitude(lon: f64) -> f64 {
    let wrapped = (lon + 180.0).rem_euclid(360.0) - 180.0;
    // rem_euclid maps exactly 180 to -180; prefer +180 as the upper bound.
    if wrapped == -180.0 { 180.0 } else { wrapped }
}

fn require_len(b: &[u8], min: usize, label: &str) -> Result<(), FieldglassError> {
    if b.len() < min {
        Err(FieldglassError::Parse(format!(
            "{label} requires {min} bytes, got {}",
            b.len()
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod sign_magnitude_tests {
    use super::*;

    #[test]
    fn positive_90_degrees() {
        // 90000 = 0x015f90.
        assert_eq!(read_signed_magnitude_24(&[0x01, 0x5f, 0x90]), 90_000);
    }

    #[test]
    fn negative_90_degrees() {
        // sign bit + 90000 = 0x80 | 0x01 0x5f 0x90 → 0x815f90.
        // Two's-complement decode would give -8298608 — make sure we don't.
        assert_eq!(read_signed_magnitude_24(&[0x81, 0x5f, 0x90]), -90_000);
    }

    #[test]
    fn negative_zero_decodes_to_zero() {
        assert_eq!(read_signed_magnitude_24(&[0x80, 0x00, 0x00]), 0);
    }
}

#[cfg(test)]
mod grid_variant_tests {
    //! Synthetic full-GDS parse tests for the projection types we claim to
    //! support. Each test hand-builds a byte array with known values and
    //! asserts the parser surfaces them on the right struct. Catches
    //! regressions where a byte offset or sign-magnitude conversion drifts
    //! without any real fixture being in hand.

    use super::*;

    /// Encode an i32 as a 3-byte sign-and-magnitude (the GRIB1 lat/lon
    /// convention; high bit = sign, low 23 bits = absolute value).
    fn sm24(v: i32) -> [u8; 3] {
        let mag = v.unsigned_abs();
        assert!(mag < 0x80_0000, "magnitude {mag} too large for 24-bit");
        let raw = if v < 0 { 0x80_0000 | mag } else { mag };
        [(raw >> 16) as u8, (raw >> 8) as u8, raw as u8]
    }

    fn u24(v: u32) -> [u8; 3] {
        assert!(v < 0x100_0000);
        [(v >> 16) as u8, (v >> 8) as u8, v as u8]
    }

    fn u16be(v: u16) -> [u8; 2] {
        v.to_be_bytes()
    }

    /// Build a GDS section byte array with a given grid_type, length, and
    /// per-type body bytes. Returns the whole section (length-prefixed).
    fn build_gds(grid_type: u8, body: &[u8]) -> Vec<u8> {
        let len = (6 + body.len()) as u32;
        let mut out = vec![
            (len >> 16) as u8,
            (len >> 8) as u8,
            len as u8,
            0, // NV
            0, // PV / PL
            grid_type,
        ];
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn parses_lambert_conformal_gds() {
        // Realistic continental-US Lambert grid: 601×401 points, origin
        // 38.5° N / 126.0° W, two standard parallels at 38.5°, 13.545 km
        // grid spacing, north pole projection.
        let mut body = Vec::new();
        body.extend(u16be(601)); // nx
        body.extend(u16be(401)); // ny
        body.extend(sm24(38_500)); // lat_first = 38.500°
        body.extend(sm24(-126_000)); // lon_first = -126.000°
        body.push(0xC0); // resolution flags: increments_given + earth_oblate
        body.extend(sm24(-95_000)); // lov = -95.000°
        body.extend(u24(13_545)); // dx_m = 13.545 km
        body.extend(u24(13_545)); // dy_m = 13.545 km
        body.push(0); // projection centre flag: north pole
        body.push(0x40); // scanning mode: j_positive
        body.extend(sm24(38_500)); // latin1
        body.extend(sm24(38_500)); // latin2
        body.extend(sm24(0)); // lat_south_pole
        body.extend(sm24(0)); // lon_south_pole

        let gds = build_gds(3, &body);
        let parsed = parse_grid_description(&gds).expect("Lambert GDS parses");
        let GridDescription::LambertConformal(g) = parsed else {
            panic!("expected LambertConformal");
        };
        assert_eq!(g.nx, 601);
        assert_eq!(g.ny, 401);
        assert_eq!(g.lat_first, 38.500);
        assert_eq!(g.lon_first, -126.000);
        assert_eq!(g.lov, -95.000);
        assert_eq!(g.dx_m, 13_545);
        assert_eq!(g.dy_m, 13_545);
        assert!(!g.south_pole);
        assert_eq!(g.latin1, 38.500);
        assert_eq!(g.latin2, 38.500);
        assert!(g.resolution_flags.increments_given);
        assert!(g.resolution_flags.earth_oblate);
        assert!(g.scanning_mode.j_positive);
    }

    #[test]
    fn lambert_bounds_compute_opposite_corner() {
        // Same CONUS Lambert grid as above. A GRIB1 Lambert GDS carries no
        // La2/Lo2, so `bounds()` must derive the last grid point from the
        // projection instead of returning the (0, 0) placeholder.
        let mut body = Vec::new();
        body.extend(u16be(601));
        body.extend(u16be(401));
        body.extend(sm24(38_500)); // lat_first
        body.extend(sm24(-126_000)); // lon_first
        body.push(0xC0);
        body.extend(sm24(-95_000)); // lov
        body.extend(u24(13_545)); // dx_m
        body.extend(u24(13_545)); // dy_m
        body.push(0); // north pole
        body.push(0x40);
        body.extend(sm24(38_500)); // latin1
        body.extend(sm24(38_500)); // latin2
        body.extend(sm24(0));
        body.extend(sm24(0));

        let parsed = parse_grid_description(&build_gds(3, &body)).expect("parses");
        let (la1, lo1, la2, lo2) = parsed.bounds().expect("Lambert has bounds");
        assert_eq!((la1, lo1), (38.500, -126.000), "first point unchanged");
        assert!(
            (la2, lo2) != (0.0, 0.0),
            "last point should be computed, got the placeholder"
        );
        // The grid is ~8000 km wide, so its opposite corner ≈ (57.248°N,
        // 15.284°E) — well east of the prime meridian, normalised to
        // (-180, 180]. The point is that it is a real corner, not (0, 0).
        assert!((la2 - 57.248).abs() < 1e-2, "lat_last: {la2}");
        assert!((lo2 - 15.284).abs() < 1e-2, "lon_last: {lo2}");

        // Round-trip: forward-projecting the corner reproduces the far grid
        // point's plane coordinates.
        let GridDescription::LambertConformal(g) = parsed else {
            unreachable!("parsed as Lambert above");
        };
        let projector = LambertProjector::new(LambertParams {
            ni: g.nx,
            nj: g.ny,
            lat_first: g.lat_first,
            lon_first: g.lon_first,
            lad: g.latin1,
            lov: g.lov,
            dx_metres: g.dx_m as f64,
            dy_metres: g.dy_m as f64,
            latin1: g.latin1,
            latin2: g.latin2,
        });
        let (ox, oy) = projector.origin();
        let (x, y) = projector.forward(la2, lo2);
        assert!((x - (ox + 600.0 * 13_545.0)).abs() < 1e-3, "x metres: {x}");
        assert!((y - (oy + 400.0 * 13_545.0)).abs() < 1e-3, "y metres: {y}");
    }

    #[test]
    fn parses_rotated_latlon_gds() {
        // A COSMO-style rotated lat/lon grid: 100×90 points, rotated south pole
        // at (-30°, 10°), 0.5° angle of rotation, 0.0625° spacing. The corner
        // coordinates are in the rotated frame.
        let mut body = Vec::new();
        body.extend(u16be(100)); // ni
        body.extend(u16be(90)); // nj
        body.extend(sm24(-18_000)); // lat_first = -18.000° (rotated frame)
        body.extend(sm24(-12_000)); // lon_first = -12.000°
        body.push(0x80); // resolution flags: increments_given
        body.extend(sm24(20_000)); // lat_last = 20.000°
        body.extend(sm24(15_000)); // lon_last = 15.000°
        body.extend(u16be(63)); // di = 0.063°
        body.extend(u16be(63)); // dj = 0.063°
        body.push(0x40); // scanning mode: j_positive
        body.extend([0, 0, 0, 0]); // 4 reserved octets
        body.extend(sm24(-30_000)); // latitudeOfSouthernPole = -30.000°
        body.extend(sm24(10_000)); // longitudeOfSouthernPole = 10.000°
        // angleOfRotation as an IBM single-precision float: 0x40800000 = 0.5.
        body.extend([0x40, 0x80, 0x00, 0x00]);

        let gds = build_gds(10, &body);
        let parsed = parse_grid_description(&gds).expect("rotated lat/lon GDS parses");
        assert_eq!(parsed.grid_type_name(), "rotated_latlon");
        assert_eq!(parsed.dimensions(), Some((100, 90)));
        assert_eq!(parsed.bounds(), Some((-18.0, -12.0, 20.0, 15.0)));
        let GridDescription::RotatedLatLon(g) = parsed else {
            panic!("expected RotatedLatLon");
        };
        assert_eq!(g.ni, 100);
        assert_eq!(g.nj, 90);
        assert_eq!(g.lat_first, -18.0);
        assert_eq!(g.lon_first, -12.0);
        assert_eq!(g.lat_last, 20.0);
        assert_eq!(g.lon_last, 15.0);
        assert_eq!(g.di, 0.063);
        assert_eq!(g.dj, 0.063);
        assert_eq!(g.south_pole_lat, -30.0);
        assert_eq!(g.south_pole_lon, 10.0);
        assert!((g.angle_of_rotation - 0.5).abs() < 1e-9);
        assert!(g.resolution_flags.increments_given);
        assert!(g.scanning_mode.j_positive);
    }

    #[test]
    fn rotated_latlon_too_short_yields_parse_error() {
        // grid_type 10 needs 42 bytes; give it a 32-byte lat/lon-sized body.
        let body = vec![0u8; 26];
        let gds = build_gds(10, &body);
        let Err(err) = parse_grid_description(&gds) else {
            panic!("short rotated GDS should error");
        };
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn parses_polar_stereographic_gds() {
        // 800×800 northern-hemisphere polar stereographic, origin at the
        // grid's south-east corner, 5 km resolution, orientation -80°.
        let mut body = Vec::new();
        body.extend(u16be(800)); // nx
        body.extend(u16be(800)); // ny
        body.extend(sm24(-20_826)); // lat_first
        body.extend(sm24(-145_000)); // lon_first
        body.push(0x88); // resolution + earth_oblate
        body.extend(sm24(-80_000)); // lov
        body.extend(u24(5_000)); // dx_m = 5 km
        body.extend(u24(5_000)); // dy_m = 5 km
        body.push(0x80); // projection centre: south pole on plane
        body.push(0x40); // scanning mode

        let gds = build_gds(5, &body);
        let parsed = parse_grid_description(&gds).expect("polar stereo GDS parses");
        let GridDescription::PolarStereographic(g) = parsed else {
            panic!("expected PolarStereographic");
        };
        assert_eq!(g.nx, 800);
        assert_eq!(g.ny, 800);
        assert_eq!(g.lat_first, -20.826);
        assert_eq!(g.lon_first, -145.000);
        assert_eq!(g.lov, -80.000);
        assert_eq!(g.dx_m, 5_000);
        assert_eq!(g.dy_m, 5_000);
        assert!(g.south_pole);
    }

    #[test]
    fn polar_stereo_bounds_compute_opposite_corner() {
        // GRIB1 polar-stereographic GDS carries no La2/Lo2, so `bounds()`
        // must derive the last grid point from the projection rather than
        // returning a (0, 0) placeholder. Verify the derived corner is a
        // real, distinct lat/lon and round-trips back to grid index
        // (nx-1, ny-1) through the same projector.
        let mut body = Vec::new();
        body.extend(u16be(800)); // nx
        body.extend(u16be(800)); // ny
        body.extend(sm24(-20_826)); // lat_first
        body.extend(sm24(-145_000)); // lon_first
        body.push(0x88);
        body.extend(sm24(-80_000)); // lov
        body.extend(u24(5_000)); // dx_m
        body.extend(u24(5_000)); // dy_m
        body.push(0x80); // south pole on plane
        body.push(0x40);

        let parsed = parse_grid_description(&build_gds(5, &body)).expect("parses");
        let (la1, lo1, la2, lo2) = parsed.bounds().expect("polar stereo has bounds");
        assert_eq!((la1, lo1), (-20.826, -145.000), "first point unchanged");
        // No longer the (0, 0) sentinel.
        assert!(
            (la2, lo2) != (0.0, 0.0),
            "last point should be computed, got the placeholder"
        );
        assert!(
            (-90.0..=0.0).contains(&la2),
            "south-polar lat in range: {la2}"
        );
        assert!((-180.0..=180.0).contains(&lo2), "lon in range: {lo2}");

        // Round-trip: forward-projecting the derived corner reproduces the
        // far grid point's plane coordinates, (nx-1)·Dx / (ny-1)·Dy from the
        // origin. (Going through `inverse()` instead would skim the index
        // upper bound and get rejected on a floating-point hair.)
        let GridDescription::PolarStereographic(g) = parsed else {
            unreachable!("parsed as polar stereo above");
        };
        let projector = PolarStereoProjector::new(PolarStereoParams {
            ni: g.nx,
            nj: g.ny,
            lat_first: g.lat_first,
            lon_first: g.lon_first,
            lov: g.lov,
            lad: 60.0,
            dx_metres: g.dx_m as f64,
            dy_metres: g.dy_m as f64,
            south_pole: g.south_pole,
        });
        let (ox, oy) = projector.origin();
        let (x, y) = projector.forward(la2, lo2);
        assert!((x - (ox + 799.0 * 5_000.0)).abs() < 1e-3, "x metres: {x}");
        assert!((y - (oy + 799.0 * 5_000.0)).abs() < 1e-3, "y metres: {y}");
    }

    #[test]
    fn unsupported_grid_type_surfaces_marker() {
        // grid_type 50 isn't one we implement; parser should return the
        // Unsupported variant carrying the offending byte rather than fail.
        // Body bytes are irrelevant for the unsupported branch, but the
        // section must still pass the length-prefix validation.
        let body = vec![0u8; 22];
        let gds = build_gds(50, &body);
        let parsed = parse_grid_description(&gds).expect("unsupported parses cleanly");
        let GridDescription::Unsupported { grid_type } = parsed else {
            panic!("expected Unsupported variant");
        };
        assert_eq!(grid_type, 50);
    }

    #[test]
    fn lambert_too_short_yields_parse_error() {
        // Lambert needs 40 bytes; give it 28 (the LatLon size).
        let body = vec![0u8; 22]; // 6 header + 22 body = 28 total
        let gds = build_gds(3, &body);
        let Err(err) = parse_grid_description(&gds) else {
            panic!("short Lambert should error");
        };
        assert!(matches!(err, FieldglassError::Parse(_)));
    }
}
