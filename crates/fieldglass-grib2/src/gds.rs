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
use fieldglass_core::{
    FieldglassError, LambertAzimuthalParams, LambertAzimuthalProjector, LambertParams,
    LambertProjector, PlanarGridProjector, PolarStereoParams, PolarStereoProjector,
    bits::sign_magnitude_to_i64, normalise_lon, signed_grid_increments,
};

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

/// Convert a 4-byte sign-magnitude projection coordinate in units of 10^-2 m
/// (§3.12's `XR`, `YR`, `X1`, `Y1`, `X2`, `Y2`) to metres.
///
/// Two things differ from [`read_metre_increment`], and both bite: the divisor
/// is a hundred rather than a thousand, and the field is signed. A real UKV
/// message carries `X1 = 0x816b28c0`, which read as unsigned is +2.17e7 m
/// rather than -238 000 m — a grid placed a third of the way round the planet.
fn read_signed_centimetres(bytes: &[u8]) -> f64 {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    sign_magnitude_to_i64(raw, 32) as f64 / 1.0e2
}

/// Convert a 4-byte unsigned grid increment in units of 10^-2 m to metres.
fn read_centimetre_increment(bytes: &[u8]) -> f64 {
    let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    raw as f64 / 1.0e2
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
    /// True when the `PL` list marks this an **octahedral** reduced grid
    /// (ECMWF's `O1280`) rather than a classic one (`N320`). Always false for
    /// a regular Gaussian grid, which carries no list.
    ///
    /// Classified at parse time from the row widths, which
    /// [`GridDefinitionSection::row_widths`] now keeps as well — the decode
    /// path (#503) is the consumer that took that trade, and the section is no
    /// longer `Copy` for it. The flag stays because the classification rule is
    /// eccodes' and belongs next to the parse, not at every call site.
    pub is_octahedral: bool,
}

/// Template 3.50 — spherical harmonic coefficients.
///
/// Not a grid: the message carries spectral coefficients truncated at the
/// pentagonal resolution `(J, K, M)`, so there are no `Ni`/`Nj` and no corner
/// coordinates. Only the triangular truncation `J = K = M` is defined for the
/// coefficient traversal the decoder uses. Rendering needs an inverse
/// spherical-harmonic transform (tracked separately); this template exists so
/// spectral messages parse their truncation metadata and decode to
/// coefficients rather than surfacing as an unsupported grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphericalHarmonicTemplate {
    /// `J` — pentagonal resolution parameter (octets 15–18).
    pub j: u32,
    /// `K` — pentagonal resolution parameter (octets 19–22).
    pub k: u32,
    /// `M` — pentagonal resolution parameter (octets 23–26).
    pub m: u32,
    /// Spectral data representation type (octet 27) — Code Table 3.6
    /// (`1` = the associated Legendre functions of the first kind).
    pub spectral_type: u8,
    /// Spectral data representation mode (octet 28) — Code Table 3.7
    /// (`1` = the complex coefficients stored for `m ≥ 0`).
    pub spectral_mode: u8,
}

/// Templates 3.61 / 3.62 / 3.63 — bi-Fourier spectral coefficients for
/// limited-area (ACCORD / ALADIN / AROME) spectral models, on a Mercator
/// (3.61), polar-stereographic (3.62), or Lambert (3.63) modelling subdomain.
///
/// Not a grid: like spherical harmonics (3.50) the message carries spectral
/// coefficients — here 4-tuples per bi-Fourier `(i, j)` wavenumber pair —
/// truncated at the resolution `(N, M)`, so there is no `Ni`/`Nj` and no corner
/// coordinates. Only the four leading fields shared by all three templates (the
/// `template.3.bf.def` head) are parsed; the projection tail that follows is not
/// needed to decode the coefficients (§5.53), which is the only thing this
/// template exists to support today (there is no inverse bi-Fourier transform to
/// render the field). The three template numbers differ only in that discarded
/// projection tail, so they share one parsed representation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BiFourierTemplate {
    /// Spectral data representation type (octet 15) — Code Table 3.6.
    pub spectral_type: u8,
    /// `N` — bi-Fourier resolution parameter in the `i` (zonal) direction
    /// (octets 16–19), the `bif_i` of eccodes' `DataG2BifourierPacking`.
    pub bif_i: u32,
    /// `M` — bi-Fourier resolution parameter in the `j` (meridional) direction
    /// (octets 20–23), the `bif_j` of eccodes' `DataG2BifourierPacking`.
    pub bif_j: u32,
    /// Type of bi-Fourier truncation (octet 24) — Code Table 3.25
    /// (`77` = rectangle, `88` = ellipse, `99` = diamond). Drives the
    /// coefficient traversal shape.
    pub truncation_type: u8,
}

/// Template 3.12 — transverse Mercator. The UTM / British National Grid
/// construction: the projection cylinder wraps along a meridian rather than
/// the equator, with a scale factor at that meridian and false easting and
/// northing offsets.
///
/// Three things make it unlike the other projected templates in this module:
///
/// * The grid corners are **projection coordinates in metres** (`X1`..`Y2`),
///   not latitudes and longitudes, so [`GridDefinitionSection::bounds`]
///   reports the reference point and its offsets instead.
/// * Every linear field is in units of 10^-2 m, where 3.10/3.20/3.30 use
///   10^-3 m, and all but `Di`/`Dj` are signed.
/// * The scale factor is an IEEE 32-bit float rather than a scaled integer,
///   so it is the message, not this parser, that rounds `0.9996012717` to
///   `0.99960124`.
#[derive(Debug, Clone, Copy)]
pub struct TransverseMercatorTemplate {
    pub shape_of_earth: u8,
    /// Semi-major and semi-minor axes in metres, from `resolve_earth_shape`.
    /// Both are carried rather than a single mean radius: the Krüger series
    /// this feeds is exact on the spheroid and degenerates to the spherical
    /// formulae when they are equal, so there is nothing to gain by averaging
    /// them first — and a UKV grid on Airy 1830 lands about 2.8 km out if you
    /// do.
    pub earth_major_m: f64,
    pub earth_minor_m: f64,
    pub ni: u32,
    pub nj: u32,
    /// `LaR` / `LoR` — the reference point the cylinder is tangent at, in
    /// degrees. Both are sign-magnitude, longitude included.
    pub lat_ref: f64,
    pub lon_ref: f64,
    pub resolution_flags: u8,
    /// `m` — scale factor at the reference meridian.
    pub scale_factor: f64,
    /// `XR` / `YR` — false easting and northing in metres.
    pub false_easting_m: f64,
    pub false_northing_m: f64,
    pub scanning_mode: u8,
    /// `Di` / `Dj` — grid spacing in metres.
    pub di_metres: f64,
    pub dj_metres: f64,
    /// `X1`..`Y2` — first and last grid point in projection metres.
    pub x1_metres: f64,
    pub y1_metres: f64,
    pub x2_metres: f64,
    pub y2_metres: f64,
}

/// Template 3.140 — Lambert azimuthal equal-area. The plane is tangent at one
/// point and area is preserved exactly, which is why Europe's statistical grids
/// (ETRS89-LAEA, EPSG:3035) and the CEMS/EFAS flood archive use it, along with
/// EUMETSAT OSI SAF sea-ice products.
///
/// Unlike §3.12 the corners are geographic, so `La1`/`Lo1` are a real first grid
/// point. Shared with §3.12, though, is that every angular field is signed
/// sign-magnitude — longitude included. The grid lengths here are millimetres,
/// the same as 3.10/3.20/3.30 and *not* the same as 3.12.
#[derive(Debug, Clone, Copy)]
pub struct LambertAzimuthalTemplate {
    pub shape_of_earth: u8,
    /// Semi-major and semi-minor axes in metres. Both are carried, not a mean
    /// radius: eccodes projects an oblate §3.140 on the true spheroid, and the
    /// mean-radius approximation is 13.5 km out over the EFAS domain.
    pub earth_major_m: f64,
    pub earth_minor_m: f64,
    pub nx: u32,
    pub ny: u32,
    pub la1: f64,
    pub lo1: f64,
    /// The tangent point: `standardParallel` is its latitude, `centralLongitude`
    /// its longitude. Both in degrees.
    pub standard_parallel: f64,
    pub central_longitude: f64,
    pub resolution_flags: u8,
    pub dx_metres: f64,
    pub dy_metres: f64,
    pub scanning_mode: u8,
}

/// GRIB2 §3.150 — the HEALPix grid.
///
/// Unlike every other gridded template this is not a raster: the field is a
/// single list of `12·Nside²` equal-area pixels with no `(ni, nj)`, so
/// [`GridDefinitionSection::dimensions`] reports `None` and the layout is the
/// pixel count alone. Placing a pixel is `fieldglass_core::healpix`'s job.
#[derive(Debug, Clone, Copy)]
pub struct HealpixTemplate {
    pub shape_of_earth: u8,
    pub resolution_flags: u8,
    /// Pixels along one side of each of the twelve base pixels. The field has
    /// `12·Nside²` points.
    pub nside: u32,
    /// Longitude of the centre line of the first rhomboid, degrees. The
    /// standard fixes this at 45; a message stating anything else is describing
    /// a grid this decoder has no oracle for, so it is carried rather than
    /// assumed.
    pub lon_first: f64,
    /// Code table 3.8 — where in its cell a value sits. 4 is "at the centre",
    /// which is what HEALPix means and what every real message states.
    pub grid_point_position: u8,
    /// `true` for NESTED, `false` for RING (code table 3.12).
    pub nested: bool,
    pub scanning_mode: u8,
}

impl HealpixTemplate {
    /// Number of pixels the field carries: `12·Nside²`.
    pub fn npix(&self) -> u64 {
        fieldglass_core::healpix::npix(self.nside)
    }
}

/// Parsed template payload. Templates outside the supported set surface as
/// `Unsupported` so callers can still expose section-header fields and a
/// useful name without erroring out.
#[derive(Debug, Clone, Copy)]
pub enum GridTemplate {
    LatLon(LatLonTemplate),
    RotatedLatLon(RotatedLatLonTemplate),
    Mercator(MercatorTemplate),
    TransverseMercator(TransverseMercatorTemplate),
    PolarStereographic(PolarStereographicTemplate),
    Lambert(LambertTemplate),
    LambertAzimuthal(LambertAzimuthalTemplate),
    Gaussian(GaussianTemplate),
    SpaceView(SpaceViewTemplate),
    SphericalHarmonic(SphericalHarmonicTemplate),
    BiFourier(BiFourierTemplate),
    Healpix(HealpixTemplate),
    Unsupported(u16),
}

/// Parsed contents of the Grid Definition Section.
///
/// Not `Copy`: [`Self::row_widths`] owns the `PL` list of a reduced grid, which
/// is one entry per row and so cannot be a fixed-size field. The section is
/// parsed once per message and cloned nowhere on a hot path (#503).
#[derive(Debug, Clone)]
pub struct GridDefinitionSection {
    pub section_length: u32,
    pub source: u8,
    pub num_data_points: u32,
    pub optional_list_octet_size: u8,
    pub optional_list_interp: u8,
    pub template_number: u16,
    pub template: GridTemplate,
    /// §3's optional list of numbers, read as the `PL` row widths of a reduced
    /// grid — empty when the section carries no list, or one it does not
    /// describe (see `parse_optional_row_widths`).
    ///
    /// This is the raw list. Read it through [`Self::points_per_row`], which
    /// answers only where the list really is a per-row point count.
    pub row_widths: Vec<u32>,
}

impl LambertTemplate {
    /// Geographic `(lat, lon)` of the last scanned grid point — the corner
    /// diagonally opposite the declared first point.
    ///
    /// §3.30 states no second corner (unlike §3.0, which carries La2/Lo2), so
    /// it is recovered the way `fieldglass-grib1` recovers its Lambert corner:
    /// forward-project the first point to plane metres, step `(Nx-1)·Dx` and
    /// `(Ny-1)·Dy` along the grid's own scan, and invert. `None` for a
    /// degenerate cone, which would otherwise report `NaN`.
    pub fn last_point(&self) -> Option<(f64, f64)> {
        let (dx, dy) = signed_grid_increments(
            self.dx_metres,
            self.dy_metres,
            self.scanning_mode & 0x80 != 0,
            self.scanning_mode & 0x40 != 0,
        );
        let projector = LambertProjector::new(LambertParams {
            earth_radius_m: self.earth_radius_m,
            ni: self.nx,
            nj: self.ny,
            lat_first: self.la1,
            lon_first: self.lo1,
            lad: self.lad,
            lov: self.lov,
            dx_metres: dx,
            dy_metres: dy,
            latin1: self.latin1,
            latin2: self.latin2,
        });
        finite_lonlat(
            projector.is_well_defined(),
            projector.last_grid_point_lonlat(),
        )
    }
}

impl LambertAzimuthalTemplate {
    /// Geographic `(lat, lon)` of the last scanned grid point. §3.140 states a
    /// real first point but no second one, so the corner is derived the same
    /// way §3.20 and §3.30 derive theirs.
    pub fn last_point(&self) -> Option<(f64, f64)> {
        let (dx, dy) = signed_grid_increments(
            self.dx_metres,
            self.dy_metres,
            self.scanning_mode & 0x80 != 0,
            self.scanning_mode & 0x40 != 0,
        );
        let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
            semi_major_m: self.earth_major_m,
            semi_minor_m: self.earth_minor_m,
            ni: self.nx,
            nj: self.ny,
            lat_first: self.la1,
            lon_first: self.lo1,
            standard_parallel: self.standard_parallel,
            central_longitude: self.central_longitude,
            dx_metres: dx,
            dy_metres: dy,
        });
        finite_lonlat(
            projector.is_well_defined(),
            projector.last_grid_point_lonlat(),
        )
    }
}

impl PolarStereographicTemplate {
    /// Geographic `(lat, lon)` of the last scanned grid point. Same derivation
    /// as [`LambertTemplate::last_point`] — §3.20 likewise states only the
    /// first point.
    pub fn last_point(&self) -> Option<(f64, f64)> {
        let (dx, dy) = signed_grid_increments(
            self.dx_metres,
            self.dy_metres,
            self.scanning_mode & 0x80 != 0,
            self.scanning_mode & 0x40 != 0,
        );
        let projector = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: self.earth_radius_m,
            ni: self.nx,
            nj: self.ny,
            lat_first: self.la1,
            lon_first: self.lo1,
            lov: self.lov,
            lad: self.lad,
            dx_metres: dx,
            dy_metres: dy,
            south_pole: self.south_pole,
        });
        finite_lonlat(true, projector.last_grid_point_lonlat())
    }
}

/// A derived corner, or `None` when the projection cannot place one. The
/// inverse is `lov + atan2(…)` and can land outside [-180, 180] (a grid with
/// `LoV` = 247° yields ~328°), so the longitude is normalised to the same
/// convention the declared first point uses.
fn finite_lonlat(well_defined: bool, (lat, lon): (f64, f64)) -> Option<(f64, f64)> {
    (well_defined && lat.is_finite() && lon.is_finite()).then(|| (lat, normalise_lon(lon)))
}

impl GridDefinitionSection {
    /// The `PL` row widths of a reduced grid, or `None` for a grid whose rows
    /// are all `Ni` wide. Mirrors `fieldglass_grib1::GridDescription::points_per_row`.
    ///
    /// Answers only where the list really is a per-row point count:
    ///
    /// * the template is a reduced Gaussian one (3.40 with `Ni` missing);
    /// * the list holds exactly `Nj` entries — a list of any other length
    ///   describes a grid this does not understand, the same gate
    ///   [`GaussianTemplate::is_octahedral`] is classified behind;
    /// * §3's interpretation of the list (octet 12, Code Table 3.11) is not `3`.
    ///   Codes `1` and `2` both mean "numbers of points"; code `3` puts
    ///   *latitudes in microdegrees* in the same list, and reading those as row
    ///   widths would invent a grid.
    ///
    /// Everything other than code `3` is accepted, rather than only `1` and
    /// `2`, because eccodes reads the list from `numberOfOctectsForNumberOfPoints`
    /// alone and never consults the interpretation (`PLPresent` in
    /// `section.3.def`). An encoder that writes the list and leaves the
    /// interpretation at its `0` default produces a file eccodes reads and the
    /// stricter rule would refuse — a breadth regression on real data, traded
    /// for a guard against a code nothing writes. Code `3` is the one case
    /// where the numbers are demonstrably something else, so it is the one
    /// refused.
    pub fn points_per_row(&self) -> Option<&[u32]> {
        let GridTemplate::Gaussian(t) = &self.template else {
            return None;
        };
        const LIST_IS_LATITUDES: u8 = 3;
        let counts_points = self.optional_list_interp != LIST_IS_LATITUDES;
        // A grid with no rows is not a reduced grid. Without this, a section
        // declaring `Nj = 0` and a list it does not actually carry would agree
        // with itself and report a `0 × 0` raster, where the same section read
        // as a regular grid reports no dimensions at all.
        (t.is_reduced
            && counts_points
            && !self.row_widths.is_empty()
            && self.row_widths.len() == t.nj as usize)
            .then_some(self.row_widths.as_slice())
    }

    /// `(ni, nj)` if the grid has a raster shape. For a reduced grid `ni` is the
    /// *widest* row — the column count the row-expanded raster needs — paired
    /// with the true row count `Nj`, exactly as `fieldglass_grib1` reports it.
    /// That pair is derived, not stated: the file says `N32`, which
    /// [`Self::size_label`] answers with and a display should prefer.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match &self.template {
            GridTemplate::LatLon(t) => Some((t.ni, t.nj)),
            GridTemplate::RotatedLatLon(t) => Some((t.ni, t.nj)),
            GridTemplate::Mercator(t) => Some((t.ni, t.nj)),
            GridTemplate::TransverseMercator(t) => Some((t.ni, t.nj)),
            GridTemplate::PolarStereographic(t) => Some((t.nx, t.ny)),
            GridTemplate::Lambert(t) => Some((t.nx, t.ny)),
            GridTemplate::LambertAzimuthal(t) => Some((t.nx, t.ny)),
            // A reduced grid has no `Ni`; the raster its rows expand into
            // does, and that is the shape every decode-side consumer needs.
            GridTemplate::Gaussian(t) => match self.points_per_row() {
                Some(pl) => Some((fieldglass_core::reduced_raster_width(pl), t.nj)),
                None => t.ni.map(|ni| (ni, t.nj)),
            },
            GridTemplate::SpaceView(t) => Some((t.nx, t.ny)),
            // Spherical harmonics are coefficients, not a gridded layout.
            GridTemplate::SphericalHarmonic(_) => None,
            // Bi-Fourier coefficients are likewise not a gridded layout.
            GridTemplate::BiFourier(_) => None,
            // HEALPix is a list of pixels, not rows and columns. Reporting
            // `(npix, 1)` here would make it look like a one-row raster to
            // every consumer that keys on this; #443 synthesises a real
            // lat/lon grid instead.
            GridTemplate::Healpix(_) => None,
            GridTemplate::Unsupported(_) => None,
        }
    }

    /// How the file names its own grid, where `Ni × Nj` is not how it is
    /// described.
    ///
    /// Three templates are not laid out in rows and columns, so `Ni × Nj` is
    /// not just unknown for them but meaningless — and a display that shows
    /// `—` implies the message does not say how big it is, when in fact it
    /// says so precisely, in that family's own units. Spectral fields are
    /// sized by their truncation, HEALPix by `Nside`. A reduced Gaussian grid
    /// is a fourth case: it has rows, but of differing width, so it is named
    /// rather than measured — `N32` classic, `O32` octahedral.
    ///
    /// A caller showing a size should prefer this where it is present and fall
    /// back to [`Self::dimensions`]. A reduced grid answers both since #503:
    /// the label is what the file says, the dimensions are the raster this
    /// crate expands its rows into. `fieldglass_grib1` answers the same way for
    /// the same grid.
    ///
    /// Formatting lives here rather than in a host because the convention is
    /// per-family domain knowledge — `T639` is not a string a UI should be
    /// assembling from parts it half-understands.
    pub fn size_label(&self) -> Option<String> {
        match &self.template {
            // Triangular truncation (J = K = M) is what real data carries and
            // the only shape the coefficient traversal is defined for, so it
            // gets the conventional `T` form. A pentagonal message still
            // parses, and says so rather than being flattened to a wrong `T`.
            GridTemplate::SphericalHarmonic(t) => Some(if t.j == t.k && t.k == t.m {
                format!("T{}", t.j)
            } else {
                format!("J{} K{} M{}", t.j, t.k, t.m)
            }),
            // Bi-Fourier is resolved separately in each direction, and there
            // is no single-letter convention for it, so both are named. This
            // field cannot be rendered (there is no inverse transform), but
            // its size is still something the message states.
            GridTemplate::BiFourier(t) => Some(format!("N{} M{}", t.bif_i, t.bif_j)),
            // `Nside` is the HEALPix resolution: the pixel count follows from
            // it as 12·Nside², so naming it names the size.
            GridTemplate::Healpix(t) => Some(format!("Nside {}", t.nside)),
            // A reduced Gaussian grid has no constant `Ni` — the row width
            // varies — but it has a name, and it is the name every tool that
            // prints one uses: `O1280` octahedral, `N320` classic. A regular
            // Gaussian grid reports `Ni × Nj` and needs no label.
            GridTemplate::Gaussian(t) if t.is_reduced => Some(format!(
                "{}{}",
                if t.is_octahedral { "O" } else { "N" },
                t.n_parallels
            )),
            _ => None,
        }
    }

    /// The §3 scanning-mode flags (Flag Table 3.4) the template carries.
    /// `None` for templates that define no data-point layout (`Unsupported`,
    /// spherical harmonics).
    pub fn scanning_mode(&self) -> Option<u8> {
        match &self.template {
            GridTemplate::LatLon(t) => Some(t.scanning_mode),
            GridTemplate::RotatedLatLon(t) => Some(t.scanning_mode),
            GridTemplate::Mercator(t) => Some(t.scanning_mode),
            GridTemplate::TransverseMercator(t) => Some(t.scanning_mode),
            GridTemplate::PolarStereographic(t) => Some(t.scanning_mode),
            GridTemplate::Lambert(t) => Some(t.scanning_mode),
            GridTemplate::LambertAzimuthal(t) => Some(t.scanning_mode),
            GridTemplate::Gaussian(t) => Some(t.scanning_mode),
            GridTemplate::SpaceView(t) => Some(t.scanning_mode),
            GridTemplate::SphericalHarmonic(_) => None,
            GridTemplate::BiFourier(_) => None,
            GridTemplate::Healpix(t) => Some(t.scanning_mode),
            GridTemplate::Unsupported(_) => None,
        }
    }

    /// `(la1, lo1, la2, lo2)` corner coordinates in degrees, when the
    /// template defines them. The projection grids that lack an explicit last
    /// grid point — Lambert and polar stereographic — return
    /// `(la1, lo1, lad, lov)`, the natural projection parameters that pair
    /// with the metadata the napi layer surfaces. Space view carries no
    /// corner coordinates (only a sub-satellite point) and returns `None`.
    /// The `(lat, lon)` of the declared first grid point, for the templates
    /// that state one.
    ///
    /// Distinct from [`Self::bounds`], and the reason it exists: a projected
    /// grid always declares where it *starts*, even when its projection is too
    /// degenerate to place the corner it ends at. `bounds` reports a pair, so
    /// it has to give up both; this gives up only what the message does not
    /// state. §3.12 is absent because its origin is `X1`/`Y1` in projection
    /// metres — a caller wanting that inverts it through the projector.
    pub fn first_point(&self) -> Option<(f64, f64)> {
        match &self.template {
            GridTemplate::LatLon(t) => Some((t.la1, t.lo1)),
            GridTemplate::RotatedLatLon(t) => Some((t.la1, t.lo1)),
            GridTemplate::Mercator(t) => Some((t.la1, t.lo1)),
            GridTemplate::PolarStereographic(t) => Some((t.la1, t.lo1)),
            GridTemplate::Lambert(t) => Some((t.la1, t.lo1)),
            GridTemplate::LambertAzimuthal(t) => Some((t.la1, t.lo1)),
            GridTemplate::Gaussian(t) => Some((t.la1, t.lo1)),
            GridTemplate::TransverseMercator(_)
            | GridTemplate::SpaceView(_)
            | GridTemplate::SphericalHarmonic(_)
            | GridTemplate::BiFourier(_)
            // A HEALPix message states no corner: pixel 0 is wherever the
            // ordering puts it, which `healpix::pix2ang` answers.
            | GridTemplate::Healpix(_)
            | GridTemplate::Unsupported(_) => None,
        }
    }

    pub fn bounds(&self) -> Option<(f64, f64, f64, f64)> {
        match &self.template {
            GridTemplate::LatLon(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::RotatedLatLon(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::Mercator(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            // Transverse Mercator carries no corner latitudes at all — `X1`
            // and `Y1` are projection metres — and unlike Lambert and polar
            // stereographic below, substituting the projection parameters
            // would put a false easting of 400 000 in a field the message
            // table prints as a longitude. Callers that want its corners
            // invert them through `TransverseMercatorProjector`, the same way
            // space view derives its own.
            GridTemplate::TransverseMercator(_) => None,
            // Both state only the first grid point, and both can derive the
            // last one from their projection (#472). They used to report
            // `LaD`/`LoV` in its place — degrees, so it looked plausible, but a
            // latitude of true scale in a column labelled "last point", and the
            // same two values are already reported as the projection parameters
            // they are. A grid whose projection cannot place the corner reports
            // the first point alone rather than a substitute.
            GridTemplate::PolarStereographic(t) => {
                t.last_point().map(|(la2, lo2)| (t.la1, t.lo1, la2, lo2))
            }
            GridTemplate::Lambert(t) => t.last_point().map(|(la2, lo2)| (t.la1, t.lo1, la2, lo2)),
            // §3.140 likewise states a real first point and no second one.
            // It used to report the tangent point in the corner's place; the
            // napi layer already replaced that with the derived corner, and now
            // the crate does, so both agree (#472).
            GridTemplate::LambertAzimuthal(t) => {
                t.last_point().map(|(la2, lo2)| (t.la1, t.lo1, la2, lo2))
            }
            GridTemplate::Gaussian(t) => Some((t.la1, t.lo1, t.la2, t.lo2)),
            GridTemplate::SpaceView(_) => None,
            GridTemplate::SphericalHarmonic(_) => None,
            GridTemplate::BiFourier(_) => None,
            // A HEALPix message states no corners; its extent is the whole
            // sphere, which is not what this reports.
            GridTemplate::Healpix(_) => None,
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
            GridTemplate::TransverseMercator(_) => "transverse_mercator".to_string(),
            GridTemplate::PolarStereographic(_) => "polar_stereo".to_string(),
            GridTemplate::Lambert(_) => "lambert".to_string(),
            GridTemplate::LambertAzimuthal(_) => "lambert_azimuthal".to_string(),
            // eccodes calls these `regular_gg` and `reduced_gg`, and
            // `fieldglass_grib1` calls the second `reduced_gaussian`. One
            // template number covers both, but they are not the same grid: one
            // has a constant `Ni` and the other a `PL` list, and the message
            // table shows this string to a reader. Naming them apart is what
            // lets the same grid read the same way in both editions (#503).
            GridTemplate::Gaussian(t) if t.is_reduced => "reduced_gaussian".to_string(),
            GridTemplate::Gaussian(_) => "gaussian".to_string(),
            GridTemplate::SpaceView(_) => "space_view".to_string(),
            GridTemplate::SphericalHarmonic(_) => "spherical_harmonic".to_string(),
            GridTemplate::Healpix(_) => "healpix".to_string(),
            GridTemplate::BiFourier(_) => "bifourier".to_string(),
            GridTemplate::Unsupported(n) => format!("unsupported(3.{n})"),
        }
    }

    /// Borrow the spherical-harmonic template if that's what the section
    /// carries. Other templates return `None`.
    pub fn spherical_harmonic(&self) -> Option<&SphericalHarmonicTemplate> {
        match &self.template {
            GridTemplate::SphericalHarmonic(t) => Some(t),
            _ => None,
        }
    }

    /// Borrow the bi-Fourier spectral template if that's what the section
    /// carries. Other templates return `None`.
    pub fn bifourier(&self) -> Option<&BiFourierTemplate> {
        match &self.template {
            GridTemplate::BiFourier(t) => Some(t),
            _ => None,
        }
    }
}

/// Alternate-row ("boustrophedon") scanning flag in §3 Flag Table 3.4 — bit 4,
/// "adjacent rows scan in the opposite direction".
pub const SCAN_ALTERNATE_ROWS: u8 = 0x10;
/// §3 Flag Table 3.4 bit 3 — when set, adjacent points in the **j** direction
/// are consecutive (columns), rather than the usual `i`-consecutive rows.
pub const SCAN_J_CONSECUTIVE: u8 = 0x20;

/// Undo alternate-row scanning in a flat, row-major (`i`-consecutive) field.
///
/// GRIB2 Flag Table 3.4 bit 4 (`0x10`) marks a grid whose adjacent rows scan in
/// opposite directions (boustrophedon). The decoder emits points in storage
/// order, so every second row lands column-reversed; the projector addresses
/// the field as `raw[j·ni + i]` and expects a regular raster, so those rows must
/// be flipped back. Row 0 scans in the nominal (`i`-flag) direction, so the
/// odd-indexed rows (1, 3, 5, …) are the reversed ones.
///
/// Only the common `i`-consecutive layout is handled — the caller must confirm
/// `SCAN_ALTERNATE_ROWS` is set and `SCAN_J_CONSECUTIVE` is clear, and pass
/// `ni` = points per row. A `values` length that is not a whole number of `ni`
/// rows leaves any trailing partial row untouched.
pub fn undo_alternate_rows(values: &mut [Option<f64>], ni: usize) {
    if ni == 0 {
        return;
    }
    // Checked throughout: `ni` is a `u32` widened to `usize`, so on a 32-bit
    // target a declared row width near `u32::MAX` makes `start + ni` wrap
    // where a 64-bit build is comfortable. The row cap upstream bounds
    // `ni · nj`, which does not bound `ni` when `nj` is zero.
    let mut start = ni; // first odd row
    while let Some(end) = start.checked_add(ni) {
        if end > values.len() {
            break;
        }
        values[start..end].reverse();
        // `ni <= end <= values.len()` here (`start` opens at `ni` and only
        // grows), so this sum is at most `2 · values.len()` — and a real slice
        // of 16-byte elements cannot be half of the address space.
        start = end + ni; // next odd row
    }
}

/// The ragged sibling of [`undo_alternate_rows`], for a reduced grid whose rows
/// differ in width.
///
/// Row `j` holds `points_per_row[j]` stored values, so there is no single `ni`
/// to step by; the odd rows are still the ones stored east-to-west. This runs
/// on the *stored* field, before row expansion — reversing an expanded row is
/// not the same operation, because expansion maps columns by longitude and a
/// reversal after it would land the row half a cell off.
///
/// No reduced grid in the wild sets the alternate-row flag (they carry simple
/// or complex packing written west-to-east), so this exists to keep the flag
/// honoured rather than quietly ignored on one grid family.
pub fn undo_alternate_reduced_rows(values: &mut [Option<f64>], points_per_row: &[u32]) {
    let mut start = 0usize;
    for (row, &width) in points_per_row.iter().enumerate() {
        let width = width as usize;
        let end = start.saturating_add(width).min(values.len());
        if row % 2 == 1 {
            values[start.min(end)..end].reverse();
        }
        start = end;
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
    // §3's optional list of numbers trails the template payload. It is read
    // here, once, because its length and per-entry size are section fields
    // (octets 11-12) rather than template ones — a reduced lat/lon grid would
    // carry the same list behind template 3.0.
    let row_widths = template_payload_len(template_number)
        .and_then(|fixed| parse_optional_row_widths(payload, fixed, optional_list_octet_size))
        .unwrap_or_default();
    let template = match template_number {
        0 => GridTemplate::LatLon(parse_template_3_0(payload)?),
        1 => GridTemplate::RotatedLatLon(parse_template_3_1(payload)?),
        10 => GridTemplate::Mercator(parse_template_3_10(payload)?),
        12 => GridTemplate::TransverseMercator(parse_template_3_12(payload)?),
        20 => GridTemplate::PolarStereographic(parse_template_3_20(payload)?),
        30 => GridTemplate::Lambert(parse_template_3_30(payload)?),
        140 => GridTemplate::LambertAzimuthal(parse_template_3_140(payload)?),
        40 => GridTemplate::Gaussian(parse_template_3_40(
            payload,
            optional_list_octet_size,
            &row_widths,
        )?),
        90 => GridTemplate::SpaceView(parse_template_3_90(payload)?),
        50 => GridTemplate::SphericalHarmonic(parse_template_3_50(payload)?),
        // Bi-Fourier spectral subdomains — 3.61 (Mercator), 3.62 (polar
        // stereographic), 3.63 (Lambert). They share the `template.3.bf.def`
        // head (spectralType / N / M / truncation-type); only the discarded
        // projection tail differs, so one parser serves all three.
        61..=63 => GridTemplate::BiFourier(parse_template_3_bf(payload)?),
        150 => GridTemplate::Healpix(parse_template_3_150(payload)?),
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
        row_widths,
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

/// Template 3.140 payload starts at GDS octet 15. Payload length = 50 bytes
/// (octets 15..=64 of the section), so a §3.140 GDS is 64 octets long.
///
/// Offsets checked against an eccodes-encoded message, as for §3.12. The trap
/// here is the same one and only one of the two: `Lo1`, `standardParallel` and
/// `centralLongitude` are all declared signed, so a message with a western
/// central meridian carries `0x80989680` for -10°, which read unsigned is
/// 2157.48°. The grid lengths, unlike §3.12's, really are millimetres.
fn parse_template_3_140(p: &[u8]) -> Result<LambertAzimuthalTemplate, FieldglassError> {
    if p.len() < 50 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.140 needs 50 bytes of payload, got {}",
            p.len()
        )));
    }
    let (major, minor) = resolve_earth_shape(p);
    Ok(LambertAzimuthalTemplate {
        shape_of_earth: p[0],
        earth_major_m: major,
        earth_minor_m: minor,
        nx: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        ny: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        la1: read_lat_degrees(&p[24..28]),
        lo1: read_lat_degrees(&p[28..32]),
        standard_parallel: read_lat_degrees(&p[32..36]),
        central_longitude: read_lat_degrees(&p[36..40]),
        resolution_flags: p[40],
        dx_metres: read_metre_increment(&p[41..45]),
        dy_metres: read_metre_increment(&p[45..49]),
        scanning_mode: p[49],
    })
}

/// Template 3.12 payload starts at GDS octet 15. Payload length = 70 bytes
/// (octets 15..=84 of the section), which is why a §3.12 GDS is 84 octets long.
///
/// The offsets are those of `grib2/template.3.12.def`, checked against an
/// eccodes-encoded message rather than counted off the WMO table: shape of the
/// earth occupies `p[0..16]`, then Ni, Nj, `LaR`, `LoR`, the resolution flags,
/// the IEEE scale factor, `XR`, `YR`, the scanning mode, `Di`, `Dj`, and the
/// four corner coordinates.
fn parse_template_3_12(p: &[u8]) -> Result<TransverseMercatorTemplate, FieldglassError> {
    if p.len() < 70 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.12 needs 70 bytes of payload, got {}",
            p.len()
        )));
    }
    let (major, minor) = resolve_earth_shape(p);
    Ok(TransverseMercatorTemplate {
        shape_of_earth: p[0],
        earth_major_m: major,
        earth_minor_m: minor,
        ni: u32::from_be_bytes([p[16], p[17], p[18], p[19]]),
        nj: u32::from_be_bytes([p[20], p[21], p[22], p[23]]),
        lat_ref: read_lat_degrees(&p[24..28]),
        // `read_lat_degrees` for a longitude on purpose: §3.12 declares `LoR`
        // signed, and a UKV message really does carry -2° as `0x801e8480`.
        // Reading it unsigned gives 2149.48°.
        lon_ref: read_lat_degrees(&p[28..32]),
        resolution_flags: p[32],
        scale_factor: read_ieee_f32(&p[33..37]) as f64,
        false_easting_m: read_signed_centimetres(&p[37..41]),
        false_northing_m: read_signed_centimetres(&p[41..45]),
        scanning_mode: p[45],
        di_metres: read_centimetre_increment(&p[46..50]),
        dj_metres: read_centimetre_increment(&p[50..54]),
        x1_metres: read_signed_centimetres(&p[54..58]),
        y1_metres: read_signed_centimetres(&p[58..62]),
        x2_metres: read_signed_centimetres(&p[62..66]),
        y2_metres: read_signed_centimetres(&p[66..70]),
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
fn parse_template_3_50(p: &[u8]) -> Result<SphericalHarmonicTemplate, FieldglassError> {
    // J / K / M (4 bytes each) then the type and mode code octets: 14 bytes,
    // section octets 15–28.
    if p.len() < 14 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.50 needs 14 bytes of payload, got {}",
            p.len()
        )));
    }
    Ok(SphericalHarmonicTemplate {
        j: u32::from_be_bytes([p[0], p[1], p[2], p[3]]),
        k: u32::from_be_bytes([p[4], p[5], p[6], p[7]]),
        m: u32::from_be_bytes([p[8], p[9], p[10], p[11]]),
        spectral_type: p[12],
        spectral_mode: p[13],
    })
}

/// Templates 3.61 / 3.62 / 3.63 payloads all begin with the 10-byte
/// `template.3.bf.def` head (GDS octets 15–24): spectralType (1),
/// biFourierResolutionParameterN (4), biFourierResolutionParameterM (4),
/// biFourierTruncationType (1). Only these four fields are read; the projection
/// tail that follows is not needed to decode the §5.53 coefficients.
fn parse_template_3_bf(p: &[u8]) -> Result<BiFourierTemplate, FieldglassError> {
    if p.len() < 10 {
        return Err(FieldglassError::Parse(format!(
            "GDS bi-Fourier template needs 10 bytes of payload for its bf head, got {}",
            p.len()
        )));
    }
    Ok(BiFourierTemplate {
        spectral_type: p[0],
        bif_i: u32::from_be_bytes([p[1], p[2], p[3], p[4]]),
        bif_j: u32::from_be_bytes([p[5], p[6], p[7], p[8]]),
        truncation_type: p[9],
    })
}

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
/// Read §3's optional list of numbers — the `PL` row widths of a reduced grid.
///
/// The list follows the template in the section payload, `octet_size` octets
/// per entry, one entry per row. Both the size and the byte count come from an
/// untrusted file, so this reads what is actually there rather than what the
/// section claims: entries are taken from whatever remains after the template,
/// and each width is assembled big-endian into a `u32`.
///
/// `None` when there is no list, when an entry could not fit a `u32`
/// (`octet_size > 4`), or when the remaining bytes are not a whole number of
/// entries — a partial trailing width is a section that does not say what it
/// claims to, and rounding it off would put a fabricated row into the grid.
fn parse_optional_row_widths(
    payload: &[u8],
    template_len: usize,
    octet_size: u8,
) -> Option<Vec<u32>> {
    let width = usize::from(octet_size);
    if width == 0 || width > 4 {
        return None;
    }
    let list = payload.get(template_len..)?;
    if list.is_empty() || !list.len().is_multiple_of(width) {
        return None;
    }
    Some(
        list.chunks_exact(width)
            .map(|entry| {
                entry
                    .iter()
                    .fold(0u32, |acc, &byte| (acc << 8) | u32::from(byte))
            })
            .collect(),
    )
}

/// The fixed payload length of template 3.40, before its optional `PL` list.
const TEMPLATE_3_40_LEN: usize = 58;

/// Where a template's fixed payload ends, for the templates that may carry §3's
/// optional list of numbers behind it. `None` means this crate reads no list
/// for that template, and the bytes past the payload are left alone.
///
/// Only 3.40 for now. A reduced *lat/lon* grid (3.0 with `Ni` missing) carries
/// the same list and would be one arm more, but its template is parsed with a
/// plain `ni: u32` and nothing downstream expands it, so reading the list would
/// store a `PL` no caller could act on.
fn template_payload_len(template_number: u16) -> Option<usize> {
    match template_number {
        40 => Some(TEMPLATE_3_40_LEN),
        _ => None,
    }
}

fn parse_template_3_40(
    p: &[u8],
    optional_list_octet_size: u8,
    row_widths: &[u32],
) -> Result<GaussianTemplate, FieldglassError> {
    if p.len() < TEMPLATE_3_40_LEN {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.40 needs {TEMPLATE_3_40_LEN} bytes of payload, got {}",
            p.len()
        )));
    }
    let is_reduced = optional_list_octet_size > 0;
    let nj = u32::from_be_bytes([p[20], p[21], p[22], p[23]]);
    // Classify only when the list is the one the section declares: `Nj` rows,
    // no more and no fewer. A list of any other length describes a grid this
    // does not understand, and a confident `O`/`N` on it would be a guess.
    let is_octahedral = is_reduced
        && row_widths.len() == nj as usize
        && fieldglass_core::is_octahedral_pl(row_widths);
    Ok(GaussianTemplate {
        shape_of_earth: p[0],
        ni: read_u32_or_missing(&p[16..20]),
        nj,
        la1: read_lat_degrees(&p[32..36]),
        lo1: read_lon_degrees(&p[36..40]),
        resolution_flags: p[40],
        la2: read_lat_degrees(&p[41..45]),
        lo2: read_lon_degrees(&p[45..49]),
        di: read_increment_degrees(&p[49..53]),
        n_parallels: u32::from_be_bytes([p[53], p[54], p[55], p[56]]),
        scanning_mode: p[57],
        is_reduced,
        is_octahedral,
    })
}

/// §3.150 (HEALPix). Payload after the 16-octet shape-of-the-earth block and
/// the resolution flags: `Nside`, the first rhomboid's longitude, the point
/// position, the ordering, and the scanning mode — 28 octets in all.
fn parse_template_3_150(p: &[u8]) -> Result<HealpixTemplate, FieldglassError> {
    if p.len() < 28 {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.150 needs 28 bytes of payload, got {}",
            p.len()
        )));
    }
    let nside = u32::from_be_bytes([p[17], p[18], p[19], p[20]]);
    if nside == 0 {
        return Err(FieldglassError::Parse(
            "GDS template 3.150 states Nside = 0, which is not a grid".to_string(),
        ));
    }
    // `12·Nside²` passes `u64::MAX` above 2^31, and these are four untrusted
    // octets. The decode cap (`MAX_GRID_POINTS`) refuses anything remotely this
    // large anyway, so the only messages this turns away are malformed ones.
    if nside > (1 << 24) {
        return Err(FieldglassError::Parse(format!(
            "GDS template 3.150 states Nside {nside}, which implies more pixels than any grid holds"
        )));
    }
    Ok(HealpixTemplate {
        shape_of_earth: p[0],
        resolution_flags: p[16],
        nside,
        lon_first: read_lon_degrees(&p[21..25]),
        grid_point_position: p[25],
        // Code table 3.12: 0 ring, 1 nested. Anything else is a value the
        // standard does not define; treating it as ring would silently
        // reorder the field, so it is refused.
        nested: match p[26] {
            0 => false,
            1 if nside.is_power_of_two() => true,
            // NESTED is a quadtree over each base face, so it exists only for
            // a power-of-two Nside; RING works for any. Refused here rather
            // than left to fail pixel by pixel, which would render as an empty
            // field rather than as a malformed message.
            1 => {
                return Err(FieldglassError::Parse(format!(
                    "GDS template 3.150 states nested ordering with Nside {nside}, which is not a \
                     power of two"
                )));
            }
            other => {
                return Err(FieldglassError::Parse(format!(
                    "GDS template 3.150 states ordering {other}, which is neither ring (0) nor nested (1)"
                )));
            }
        },
        scanning_mode: p[27],
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

    /// Build a §3 with template 3.50 (spherical harmonics): 14-byte header plus
    /// J / K / M / type / mode, 28 bytes total.
    fn build_gds_3_50(j: u32, k: u32, m: u32) -> Vec<u8> {
        let mut buf: Vec<u8> = Vec::new();
        push_be(&mut buf, 28, 4); // section length
        buf.push(3); // section number
        buf.push(0); // source
        push_be(&mut buf, (j + 1) * (j + 2), 4); // num_data_points
        buf.push(0); // optional list size
        buf.push(0); // optional list interp
        push_be(&mut buf, 50, 2); // template number 3.50
        push_be(&mut buf, j, 4);
        push_be(&mut buf, k, 4);
        push_be(&mut buf, m, 4);
        buf.push(1); // spectral type
        buf.push(1); // spectral mode
        assert_eq!(buf.len(), 28);
        buf
    }

    #[test]
    fn parses_template_3_50() {
        let gds = parse_grid_definition(&build_gds_3_50(63, 63, 63)).expect("parse 3.50");
        assert_eq!(gds.template_number, 50);
        assert_eq!(gds.template_name(), "spherical_harmonic");
        assert_eq!(gds.num_data_points, 64 * 65);
        // A spectral message has no grid dimensions, scanning mode, or bounds.
        assert_eq!(gds.dimensions(), None);
        assert_eq!(gds.scanning_mode(), None);
        assert_eq!(gds.bounds(), None);
        let sh = gds.spherical_harmonic().expect("3.50 template");
        assert_eq!((sh.j, sh.k, sh.m), (63, 63, 63));
        assert_eq!(sh.spectral_type, 1);
        assert_eq!(sh.spectral_mode, 1);
    }

    #[test]
    fn rejects_short_template_3_50() {
        let mut buf = build_gds_3_50(63, 63, 63);
        // Declare a length that truncates the 14-byte template payload.
        buf[3] = 20;
        buf.truncate(20);
        assert!(parse_grid_definition(&buf).is_err());
    }

    /// Build a §3 with a bi-Fourier template (default 3.63 / Lambert): the
    /// 10-byte bf head (spectralType / N / M / truncation-type) plus a short
    /// filler tail standing in for the discarded projection fields.
    fn build_gds_3_bf(template_number: u16, n: u32, m: u32, trunc: u8) -> Vec<u8> {
        let mut p: Vec<u8> = Vec::new();
        p.push(2); // spectralType
        push_be(&mut p, n, 4); // biFourierResolutionParameterN
        push_be(&mut p, m, 4); // biFourierResolutionParameterM
        p.push(trunc); // biFourierTruncationType
        p.extend_from_slice(&[0u8; 20]); // discarded projection tail (filler)
        wrap_gds(template_number, 4 * (n + 1) * (m + 1), &p)
    }

    #[test]
    fn parses_bifourier_templates_3_61_62_63() {
        for tn in [61u16, 62, 63] {
            let gds = parse_grid_definition(&build_gds_3_bf(tn, 4, 4, 88)).expect("parse bf");
            assert_eq!(gds.template_number, tn);
            assert_eq!(gds.template_name(), "bifourier");
            // Coefficients, not a grid: no dimensions, scanning mode, or bounds.
            assert_eq!(gds.dimensions(), None);
            assert_eq!(gds.scanning_mode(), None);
            assert_eq!(gds.bounds(), None);
            let bf = gds.bifourier().expect("bf template");
            assert_eq!((bf.bif_i, bf.bif_j), (4, 4));
            assert_eq!(bf.spectral_type, 2);
            assert_eq!(bf.truncation_type, 88);
            assert!(gds.spherical_harmonic().is_none());
        }
    }

    #[test]
    fn rejects_short_bifourier_template() {
        let mut buf = build_gds_3_bf(63, 4, 4, 88);
        // Declare a length that truncates the 10-byte bf head.
        buf[0..4].copy_from_slice(&18u32.to_be_bytes());
        buf.truncate(18);
        assert!(parse_grid_definition(&buf).is_err());
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

    /// A cone too degenerate to place the far corner still reports the corner
    /// the message states. `bounds` gives up the pair, because it *is* a pair;
    /// `first_point` gives up only the half the message does not state.
    #[test]
    fn a_degenerate_lambert_keeps_its_declared_first_point() {
        // latin1 == latin2 == 0 collapses the cone (n = sin 0 = 0).
        let mut p = Vec::new();
        p.push(6u8); // shape of earth
        p.extend([0u8; 15]);
        push_be(&mut p, 10, 4); // nx
        push_be(&mut p, 10, 4); // ny
        p.extend_from_slice(&signed_lat_bytes(45_000_000)); // la1
        push_be(&mut p, 250_000_000, 4); // lo1
        p.push(0x30); // resolution
        p.extend_from_slice(&signed_lat_bytes(0)); // LaD = 0
        push_be(&mut p, 265_000_000, 4); // LoV
        push_be(&mut p, 12_000_000, 4); // Dx
        push_be(&mut p, 12_000_000, 4); // Dy
        p.push(0); // projection centre
        p.push(64); // scanning mode
        p.extend_from_slice(&signed_lat_bytes(0)); // Latin1 = 0
        p.extend_from_slice(&signed_lat_bytes(0)); // Latin2 = 0
        p.extend_from_slice(&signed_lat_bytes(0)); // lat south pole
        push_be(&mut p, 0, 4); // lon south pole
        let bytes = wrap_gds(30, 100, &p);
        let gds = parse_grid_definition(&bytes).expect("parse 3.30");
        let GridTemplate::Lambert(t) = gds.template else {
            panic!("expected Lambert");
        };
        assert!((t.la1 - 45.0).abs() < 1e-9 && (t.lo1 - 250.0).abs() < 1e-9);
        assert_eq!(
            t.last_point(),
            None,
            "a collapsed cone cannot place the far corner"
        );
        assert_eq!(gds.bounds(), None, "so there is no corner pair to report");
        let (la1, lo1) = gds.first_point().expect("the first point is stated");
        assert!((la1 - 45.0).abs() < 1e-9 && (lo1 - 250.0).abs() < 1e-9);
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
        // The last point is derived from the projection, not substituted with
        // `LaD`/`LoV` as it was before #472. Checked against eccodes' own
        // point iterator: encoding this exact grid (via the eccodes 2.48
        // Python wheel, which can write the 512×512 values array) and running
        // `grib_get_data` from the pinned 2.34.1 CLI reports the last point as
        // (6.919425280, 182.657847210), i.e. -177.342152790 in the ±180
        // convention this crate reports.
        let (la1, lo1, la2, lo2) = gds.bounds().expect("polar stereo has bounds");
        assert_eq!((la1, lo1), (-20.0, 225.0), "first point unchanged");
        assert!(
            (la2 - 6.919_425_280).abs() < 1e-6 && (lo2 - (-177.342_152_790)).abs() < 1e-6,
            "derived last point ({la2}, {lo2}) should match eccodes' iterator"
        );
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
        assert_eq!(gds.scanning_mode(), None);
    }

    fn some(vs: &[f64]) -> Vec<Option<f64>> {
        vs.iter().map(|v| Some(*v)).collect()
    }

    #[test]
    fn undo_alternate_rows_flips_only_odd_rows() {
        // 3 rows of 4. Rows 0 and 2 stay; row 1 (odd) reverses.
        let mut v = some(&[
            1.0, 2.0, 3.0, 4.0, // row 0
            8.0, 7.0, 6.0, 5.0, // row 1, stored reversed
            9.0, 10.0, 11.0, 12.0, // row 2
        ]);
        undo_alternate_rows(&mut v, 4);
        assert_eq!(
            v,
            some(&[
                1.0, 2.0, 3.0, 4.0, //
                5.0, 6.0, 7.0, 8.0, //
                9.0, 10.0, 11.0, 12.0,
            ])
        );
    }

    #[test]
    fn undo_alternate_rows_carries_masked_points() {
        let mut v = vec![Some(1.0), None, Some(3.0), None, Some(6.0), Some(4.0)];
        undo_alternate_rows(&mut v, 3); // row 1 = [None, 6, 4] -> [4, 6, None]
        assert_eq!(
            v,
            vec![Some(1.0), None, Some(3.0), Some(4.0), Some(6.0), None]
        );
    }

    #[test]
    fn undo_alternate_rows_leaves_trailing_partial_row() {
        // 4 full elems (row 0) + 4 (row 1) + 2 trailing: the partial row is
        // shorter than ni and must not be touched (would corrupt it).
        let mut v = some(&[1.0, 2.0, 8.0, 7.0, 99.0, 98.0]);
        undo_alternate_rows(&mut v, 2);
        assert_eq!(v, some(&[1.0, 2.0, 7.0, 8.0, 99.0, 98.0]));
    }

    /// A row width that overflows when doubled must stop, not wrap.
    ///
    /// `ni` is a `u32` from §3 widened to `usize`; the decoder's cap bounds
    /// `ni · nj`, which says nothing about `ni` when `nj` is zero. On a 32-bit
    /// target the first row start then wraps — this is that same arithmetic,
    /// written at a width where any target reaches it.
    #[test]
    fn undo_alternate_rows_does_not_overflow_on_a_huge_width() {
        let mut v = some(&[1.0, 2.0, 3.0]);
        undo_alternate_rows(&mut v, usize::MAX);
        assert_eq!(v, some(&[1.0, 2.0, 3.0]));
        undo_alternate_rows(&mut v, usize::MAX / 2 + 1);
        assert_eq!(v, some(&[1.0, 2.0, 3.0]));
    }

    #[test]
    fn undo_alternate_rows_ni_zero_is_noop() {
        let mut v = some(&[1.0, 2.0, 3.0]);
        undo_alternate_rows(&mut v, 0);
        assert_eq!(v, some(&[1.0, 2.0, 3.0]));
    }

    #[test]
    fn scan_flag_constants_match_table_3_4() {
        // scanningMode 80 (NBM): j-positive (0x40) + alternate rows (0x10).
        let sm: u8 = 80;
        assert_eq!(sm, 0x40 | SCAN_ALTERNATE_ROWS);
        assert!(sm & SCAN_ALTERNATE_ROWS != 0);
        assert!(sm & SCAN_J_CONSECUTIVE == 0);
    }
    /// The `PL` reader refuses every shape it cannot read honestly.
    ///
    /// Both the entry size and the byte count come from the file, so these are
    /// the values a fuzzer reaches first. The rule throughout is that a list
    /// this cannot read exactly becomes `None` rather than a shorter list: a
    /// fabricated or dropped row would put a wrong grid in front of someone
    /// with no indication anything was missing.
    #[test]
    fn optional_row_widths_refuses_what_it_cannot_read_exactly() {
        // Four rows of two octets, immediately after a 4-byte "template".
        let payload = [0xAA, 0xAA, 0xAA, 0xAA, 0, 20, 0, 24, 0, 24, 0, 20];
        assert_eq!(
            parse_optional_row_widths(&payload, 4, 2),
            Some(vec![20, 24, 24, 20])
        );

        // No list declared, and entries too wide to fit a u32.
        assert_eq!(parse_optional_row_widths(&payload, 4, 0), None);
        assert_eq!(parse_optional_row_widths(&payload, 4, 5), None);
        assert_eq!(parse_optional_row_widths(&payload, 4, 255), None);

        // A trailing partial entry: 8 bytes is not a whole number of 3-octet
        // widths, so this is a section that does not say what it claims.
        // Truncating to two rows would invent a grid.
        assert_eq!(parse_optional_row_widths(&payload, 4, 3), None);

        // No bytes after the template, and a template longer than the payload
        // — neither may index out of bounds.
        assert_eq!(parse_optional_row_widths(&payload, payload.len(), 2), None);
        assert_eq!(
            parse_optional_row_widths(&payload, payload.len() + 99, 2),
            None
        );

        // Four octets is the widest entry that fits, read big-endian.
        assert_eq!(
            parse_optional_row_widths(&[0xFF, 0xFF, 0xFF, 0xFF], 0, 4),
            Some(vec![u32::MAX])
        );
    }

    /// A section declaring no rows is not a reduced grid.
    ///
    /// `Nj = 0` with a list the section does not carry makes the length gate
    /// agree with itself — zero entries, zero rows — and `dimensions` would then
    /// report a `0 × 0` raster for a message that states no shape at all. The
    /// same octets read as a regular Gaussian grid report `None`, and that is
    /// the honest answer for both.
    #[test]
    fn a_section_with_no_rows_is_not_a_reduced_grid() {
        let mut payload = vec![0u8; TEMPLATE_3_40_LEN];
        payload[16..20].copy_from_slice(&u32::MAX.to_be_bytes()); // Ni missing
        payload[20..24].copy_from_slice(&0u32.to_be_bytes()); // Nj = 0
        let widths = parse_optional_row_widths(&payload, TEMPLATE_3_40_LEN, 2);
        assert_eq!(widths, None, "there are no bytes past the template");
        let template = parse_template_3_40(&payload, 2, &[]).expect("parses");
        assert!(template.is_reduced, "the section declares a list");

        let gds = GridDefinitionSection {
            section_length: 14 + TEMPLATE_3_40_LEN as u32,
            source: 0,
            num_data_points: 0,
            optional_list_octet_size: 2,
            optional_list_interp: 1,
            template_number: 40,
            template: GridTemplate::Gaussian(template),
            row_widths: Vec::new(),
        };
        assert_eq!(gds.points_per_row(), None, "no rows, no PL list");
        assert_eq!(gds.dimensions(), None, "and so no raster either");
    }

    /// A `PL` list of the wrong length never produces a grid name.
    ///
    /// The classification is gated on the list holding exactly `Nj` rows. A
    /// list of any other length describes a grid this does not understand, and
    /// `O32` on it would be a confident guess — the failure the `size_label`
    /// seam exists to avoid.
    #[test]
    fn a_pl_list_that_is_not_nj_rows_is_not_classified() {
        // Octahedral widths, but three rows where Nj declares four.
        let mut payload = vec![0u8; TEMPLATE_3_40_LEN];
        payload[20..24].copy_from_slice(&4u32.to_be_bytes());
        payload[53..57].copy_from_slice(&1u32.to_be_bytes());
        for width in [20u16, 24, 28] {
            payload.extend_from_slice(&width.to_be_bytes());
        }
        let widths =
            parse_optional_row_widths(&payload, TEMPLATE_3_40_LEN, 2).expect("a readable list");
        let template = parse_template_3_40(&payload, 2, &widths).expect("parses");
        assert!(template.is_reduced, "a list is present");
        assert!(
            !template.is_octahedral,
            "but it is not the declared Nj rows"
        );
    }
}
