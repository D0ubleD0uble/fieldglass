use fieldglass_core::{
    CornerPair, FieldglassError, LambertParams, LambertProjector, PlanarGridProjector,
    PolarStereoParams, PolarStereoProjector, bits::ibm_float_to_f64, signed_grid_increments,
};

// ---------------------------------------------------------------------------
// Flag bytes
// ---------------------------------------------------------------------------

/// Resolution and component flags — WMO ON388 Code Table 7 (GDS octet 17).
#[derive(Debug)]
pub struct ResolutionFlags {
    /// True if Di/Dj increments are given in the GDS.
    pub increments_given: bool,
    /// True if earth is oblate spheroid; false = spherical (radius 6367.47 km).
    pub earth_oblate: bool,
    /// True if u/v vector components are resolved relative to the grid (i,j)
    /// rather than to geographic east/north.
    pub uv_relative_to_grid: bool,
}

/// GRIB1's spherical Earth (ON388 Code Table 7, earth-shape bit clear). This is
/// the value eccodes reports as `radius` for a GRIB1 message, and the one its
/// grid iterators project with.
pub const GRIB1_SPHERICAL_RADIUS_M: f64 = 6_367_470.0;

/// The IAU 1965 spheroid GRIB1 selects when the earth-shape bit is set.
const IAU_1965_MAJOR_AXIS_M: f64 = 6_378_160.0;
const IAU_1965_MINOR_AXIS_M: f64 = 6_356_775.0;

/// Latitude of true scale of a GRIB1 polar stereographic grid, in degrees.
/// GRIB2 §3.20 states `LaD` in the template; GRIB1 does not carry the field at
/// all, and ON388 fixes it at ±60°. Surfaced through
/// [`PolarStereoGrid::lad`], which is where a consumer should read it.
const POLAR_STEREO_LAD_DEG: f64 = 60.0;

impl ResolutionFlags {
    /// Radius of the sphere to project this grid on, in metres.
    ///
    /// The spherical case is exact — GRIB1 fixes it at 6 367 470 m, and using
    /// anything else misplaces a continental grid by kilometres.
    ///
    /// The oblate case is an approximation: eccodes projects an oblate grid on
    /// the true spheroid, while the projections here are spherical, so we take
    /// the spheroid's mean radius `(2a + b) / 3`. That is within ~0.1 % of the
    /// true figure and is far closer than ignoring the shape altogether. Oblate
    /// GRIB1 Lambert / polar-stereo grids are rare; true ellipsoidal projection
    /// is a follow-up.
    pub fn earth_radius_m(&self) -> f64 {
        if self.earth_oblate {
            (2.0 * IAU_1965_MAJOR_AXIS_M + IAU_1965_MINOR_AXIS_M) / 3.0
        } else {
            GRIB1_SPHERICAL_RADIUS_M
        }
    }

    fn from_byte(b: u8) -> Self {
        Self {
            increments_given: b & 0x80 != 0,
            earth_oblate: b & 0x40 != 0,
            uv_relative_to_grid: b & 0x08 != 0,
        }
    }
}

/// Scanning mode flags — WMO ON388 Flag Table 8 (GDS octet 28).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanningMode {
    /// True = points scan in −i direction (east→west); false = west→east.
    pub i_negative: bool,
    /// True = points scan in +j direction (south→north); false = north→south.
    pub j_positive: bool,
    /// True = adjacent points are consecutive in j (column-major); false =
    /// row-major.
    ///
    /// Unlike the two direction bits above, this one changes the *order* the
    /// points are stored in rather than where they sit, so no geometry absorbs
    /// it: [`crate::Grib1Reader::decode_message_raster`] transposes the field
    /// instead, and the boustrophedonic undo takes its run length from `Nj`.
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
#[derive(Debug)]
pub struct LatLonGrid {
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the first scanned point (`La1`), degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point (`Lo1`), degrees.
    pub lon_first: f64,
    /// Latitude of the last scanned point (`La2`), degrees.
    pub lat_last: f64,
    /// Longitude of the last scanned point (`Lo2`), degrees.
    pub lon_last: f64,
    /// East-west increment in degrees.
    pub di: f64,
    /// North-south increment in degrees.
    pub dj: f64,
    /// Resolution and component flags (GDS octet 17).
    pub resolution_flags: ResolutionFlags,
    /// Scanning-mode flags (GDS octet 28), which fix the order of the points.
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
#[derive(Debug)]
pub struct RotatedLatLonGrid {
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the first scanned point (`La1`), degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point (`Lo1`), degrees.
    pub lon_first: f64,
    /// Latitude of the last scanned point (`La2`), degrees.
    pub lat_last: f64,
    /// Longitude of the last scanned point (`Lo2`), degrees.
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
    /// Resolution and component flags (GDS octet 17).
    pub resolution_flags: ResolutionFlags,
    /// Scanning-mode flags (GDS octet 28), which fix the order of the points.
    pub scanning_mode: ScanningMode,
}

/// Grid type 0 (reduced) — quasi-regular Latitude/Longitude.
///
/// A "reduced" grid drops `Ni` (the GDS encodes it as the missing marker
/// `0xFFFF`) and instead carries a `PL` list giving the number of points in
/// each of the `Nj` rows — fewer points toward the poles. The total point
/// count is `points_per_row.sum()`, not `Ni·Nj`.
#[derive(Debug)]
pub struct ReducedLatLonGrid {
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the first scanned point (`La1`), degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point (`Lo1`), degrees.
    pub lon_first: f64,
    /// Latitude of the last scanned point (`La2`), degrees.
    pub lat_last: f64,
    /// Longitude of the last scanned point (`Lo2`), degrees.
    pub lon_last: f64,
    /// North-south increment in degrees.
    pub dj: f64,
    /// Number of points in each of the `Nj` rows (the GDS `PL` list).
    pub points_per_row: Vec<u32>,
    /// Resolution and component flags (GDS octet 17).
    pub resolution_flags: ResolutionFlags,
    /// Scanning-mode flags (GDS octet 28), which fix the order of the points.
    pub scanning_mode: ScanningMode,
}

/// Grid type 4 (reduced) — quasi-regular Gaussian Latitude/Longitude.
///
/// As [`ReducedLatLonGrid`], but the row latitudes are Gauss–Legendre nodes
/// (`n_gaussians` between pole and equator) rather than equispaced. This is the
/// common ECMWF "reduced_gg" layout.
#[derive(Debug)]
pub struct ReducedGaussianGrid {
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the first scanned point (`La1`), degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point (`Lo1`), degrees.
    pub lon_first: f64,
    /// Latitude of the last scanned point (`La2`), degrees.
    pub lat_last: f64,
    /// Longitude of the last scanned point (`Lo2`), degrees.
    pub lon_last: f64,
    /// Number of Gaussian latitudes between pole and equator.
    pub n_gaussians: u16,
    /// Number of points in each of the `Nj` rows (the GDS `PL` list).
    pub points_per_row: Vec<u32>,
    /// Resolution and component flags (GDS octet 17).
    pub resolution_flags: ResolutionFlags,
    /// Scanning-mode flags (GDS octet 28), which fix the order of the points.
    pub scanning_mode: ScanningMode,
}

/// Grid type 4 — Gaussian Latitude/Longitude.
#[derive(Debug)]
pub struct GaussianGrid {
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the first scanned point (`La1`), degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point (`Lo1`), degrees.
    pub lon_first: f64,
    /// Latitude of the last scanned point (`La2`), degrees.
    pub lat_last: f64,
    /// Longitude of the last scanned point (`Lo2`), degrees.
    pub lon_last: f64,
    /// East-west increment in degrees (may be absent; check resolution_flags).
    pub di: f64,
    /// Number of Gaussian latitudes between pole and equator.
    pub n_gaussians: u16,
    /// Resolution and component flags (GDS octet 17).
    pub resolution_flags: ResolutionFlags,
    /// Scanning-mode flags (GDS octet 28), which fix the order of the points.
    pub scanning_mode: ScanningMode,
}

/// Grid type 5 — Polar Stereographic.
#[derive(Debug)]
pub struct PolarStereoGrid {
    /// Points along the projection plane's x axis (`Nx`).
    pub nx: u32,
    /// Points along the projection plane's y axis (`Ny`).
    pub ny: u32,
    /// Latitude of the first scanned point (`La1`), degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point (`Lo1`), degrees.
    pub lon_first: f64,
    /// Orientation longitude — meridian parallel to y-axis (degrees).
    pub lov: f64,
    /// Grid spacing in x at the 60° parallel (metres).
    pub dx_m: u32,
    /// Grid spacing in y at the 60° parallel (metres).
    pub dy_m: u32,
    /// True = South Pole on projection plane; false = North Pole.
    pub south_pole: bool,
    /// Resolution and component flags (GDS octet 17).
    pub resolution_flags: ResolutionFlags,
    /// Scanning-mode flags (GDS octet 28), which fix the order of the points.
    pub scanning_mode: ScanningMode,
}

impl PolarStereoGrid {
    /// Latitude of true scale, in degrees.
    ///
    /// GRIB1 has no `LaD` field: ON388 fixes a polar stereographic grid's
    /// true-scale parallel at ±60°, so every consumer building
    /// [`PolarStereoParams`] has to supply it. The projectors take the
    /// magnitude, so this is 60° for a South-Pole grid too — the hemisphere is
    /// [`Self::south_pole`], not the sign of this.
    pub fn lad(&self) -> f64 {
        POLAR_STEREO_LAD_DEG
    }

    /// `(Dx, Dy)` in metres with the scanning-mode sign applied.
    ///
    /// [`Self::dx_m`] and [`Self::dy_m`] are the unsigned magnitudes ON388
    /// encodes (GDS octets 21–23 / 24–26); the direction lives in the scanning
    /// mode. Anything that walks from the first scanned point — the warp, the
    /// far-corner recovery below, [`crate::geometry`] — needs the signed pair,
    /// and a grid scanning north-to-south walks `-Dy`.
    pub fn signed_increments(&self) -> (f64, f64) {
        signed_grid_increments(
            f64::from(self.dx_m),
            f64::from(self.dy_m),
            self.scanning_mode.i_negative,
            self.scanning_mode.j_positive,
        )
    }

    /// Geographic `(lat, lon)` of the last scanned grid point — the corner
    /// diagonally opposite the origin.
    ///
    /// GRIB1 polar-stereographic GDS encodes only the first point (La1/Lo1);
    /// unlike a lat/lon grid there is no La2/Lo2 to read. The opposite corner
    /// is recovered by forward-projecting the origin to plane metres, stepping
    /// `(Nx-1)·Dx` / `(Ny-1)·Dy`, and inverse-projecting back to lat/lon.
    fn last_point(&self) -> Option<(f64, f64)> {
        // Dx/Dy are unsigned magnitudes; the scanning mode says which way the
        // grid runs from its first point, so the walk has to carry that sign or
        // a north→south grid reports a corner on the wrong side of its origin
        // (#472). The warp has always applied it — this is the same rule.
        let (dx, dy) = self.signed_increments();
        let projector = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: self.resolution_flags.earth_radius_m(),
            ni: self.nx,
            nj: self.ny,
            lat_first: self.lat_first,
            lon_first: self.lon_first,
            lov: self.lov,
            lad: self.lad(),
            dx_metres: dx,
            dy_metres: dy,
            south_pole: self.south_pole,
        });
        // The inverse is `lov + atan2(..)` and can land outside [-180, 180]
        // (e.g. lov=247 yields ~328°); normalise so the reported corner is
        // consistent with the first point's longitude convention.
        finite_lonlat(
            projector.is_well_defined(),
            projector.last_grid_point_lonlat(),
        )
    }
}

/// Grid type 3 — Lambert Conformal (conic or bi-polar).
#[derive(Debug)]
pub struct LambertGrid {
    /// Points along the projection plane's x axis (`Nx`).
    pub nx: u32,
    /// Points along the projection plane's y axis (`Ny`).
    pub ny: u32,
    /// Latitude of the first scanned point (`La1`), degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point (`Lo1`), degrees.
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
    /// Resolution and component flags (GDS octet 17).
    pub resolution_flags: ResolutionFlags,
    /// Scanning-mode flags (GDS octet 28), which fix the order of the points.
    pub scanning_mode: ScanningMode,
}

impl LambertGrid {
    /// `(Dx, Dy)` in metres with the scanning-mode sign applied. Same rule as
    /// [`PolarStereoGrid::signed_increments`], for the same reason.
    pub fn signed_increments(&self) -> (f64, f64) {
        signed_grid_increments(
            f64::from(self.dx_m),
            f64::from(self.dy_m),
            self.scanning_mode.i_negative,
            self.scanning_mode.j_positive,
        )
    }

    /// Geographic `(lat, lon)` of the last scanned grid point — the corner
    /// diagonally opposite the origin.
    ///
    /// Like polar stereographic, a GRIB1 Lambert GDS encodes only the first
    /// point; the opposite corner is recovered from the projection. `LaD`
    /// (latitude of true scale) is taken as the first standard parallel,
    /// matching how the warp path builds [`LambertParams`].
    fn last_point(&self) -> Option<(f64, f64)> {
        // Scan-signed, for the reason in [`PolarStereoGrid::last_point`].
        let (dx, dy) = self.signed_increments();
        let projector = LambertProjector::new(LambertParams {
            earth_radius_m: self.resolution_flags.earth_radius_m(),
            ni: self.nx,
            nj: self.ny,
            lat_first: self.lat_first,
            lon_first: self.lon_first,
            lad: self.latin1,
            lov: self.lov,
            dx_metres: dx,
            dy_metres: dy,
            latin1: self.latin1,
            latin2: self.latin2,
        });
        // A collapsed cone (both standard parallels on the equator, say)
        // inverts to `NaN`, which used to reach the message table as the text
        // "NaN"; report no corner instead.
        finite_lonlat(
            projector.is_well_defined(),
            projector.last_grid_point_lonlat(),
        )
    }
}

// ---------------------------------------------------------------------------
// Top-level enum
// ---------------------------------------------------------------------------

/// The GDS, parsed into whichever grid family its type octet named.
///
/// [`GridDescription::grid_type_name`] is the stable string the rest of
/// the workspace keys on; matching on the variant gets the parameters.
#[derive(Debug)]
pub enum GridDescription {
    /// Grid type 0 — regular latitude/longitude.
    LatLon(LatLonGrid),
    /// Grid type 10 — rotated latitude/longitude.
    RotatedLatLon(RotatedLatLonGrid),
    /// Grid type 0 with `Ni` absent — quasi-regular latitude/longitude.
    ReducedLatLon(ReducedLatLonGrid),
    /// Grid type 4 — regular Gaussian latitude/longitude.
    Gaussian(GaussianGrid),
    /// Grid type 4 with `Ni` absent — quasi-regular Gaussian.
    ReducedGaussian(ReducedGaussianGrid),
    /// Grid type 5 — polar stereographic.
    PolarStereographic(PolarStereoGrid),
    /// Grid type 3 — Lambert conformal.
    LambertConformal(LambertGrid),
    /// Spherical-harmonic coefficients (grid type 50). Not a grid at all: the
    /// message stores the field's spectral coefficients, so it has no `Ni`/`Nj`
    /// and no data points in the usual sense. Decode it with
    /// [`crate::Grib1Reader::decode_spectral_message`].
    SphericalHarmonic(SphericalHarmonicGrid),
    /// Grid type present but not yet supported by this parser.
    Unsupported {
        /// The unsupported type's ON388 Table 6 code, so a caller can name it.
        grid_type: u8,
    },
}

/// Pentagonal resolution parameters of a spherical-harmonic "grid" (GDS data
/// representation type 50). Real data is always triangular (`j == k == m`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphericalHarmonicGrid {
    /// Pentagonal resolution parameter J (octets 7-8).
    pub j: u16,
    /// Pentagonal resolution parameter K (octets 9-10).
    pub k: u16,
    /// Pentagonal resolution parameter M (octets 11-12).
    pub m: u16,
    /// Octet 13. 1 = associated Legendre polynomials (the only value defined).
    pub representation_type: u8,
    /// Octet 14. 1 = the complex/triangular packing ECMWF writes.
    pub representation_mode: u8,
}

impl GridDescription {
    /// The grid family's stable name — `"latlon"`, `"lambert"`,
    /// `"polar_stereo"`, … — as every other crate in the workspace spells it.
    pub fn grid_type_name(&self) -> &'static str {
        match self {
            Self::LatLon(_) => "latlon",
            Self::RotatedLatLon(_) => "rotated_latlon",
            Self::ReducedLatLon(_) => "reduced_latlon",
            Self::Gaussian(_) => "gaussian",
            Self::ReducedGaussian(_) => "reduced_gaussian",
            Self::PolarStereographic(_) => "polar_stereo",
            Self::LambertConformal(_) => "lambert",
            Self::SphericalHarmonic(_) => "spherical_harmonic",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// The scanning-mode flags (GDS octet 28), for the grids that have them.
    ///
    /// `None` for the two variants that describe no raster: spherical-harmonic
    /// coefficients live in wavenumber space and an unsupported grid type was
    /// never parsed past its number, so neither carries a scan direction.
    /// Mirrors `fieldglass_grib2::gds::GridDefinition::scanning_mode`, which
    /// answers the same question about a GRIB2 §3 template.
    ///
    /// Prefer this over matching the variant: every consumer that has done so
    /// has ended up with an arm list that quietly omits a grid family.
    pub fn scanning_mode(&self) -> Option<&ScanningMode> {
        match self {
            Self::LatLon(g) => Some(&g.scanning_mode),
            Self::RotatedLatLon(g) => Some(&g.scanning_mode),
            Self::ReducedLatLon(g) => Some(&g.scanning_mode),
            Self::Gaussian(g) => Some(&g.scanning_mode),
            Self::ReducedGaussian(g) => Some(&g.scanning_mode),
            Self::PolarStereographic(g) => Some(&g.scanning_mode),
            Self::LambertConformal(g) => Some(&g.scanning_mode),
            Self::SphericalHarmonic(_) => None,
            Self::Unsupported { .. } => None,
        }
    }

    /// Whether this message's values lie on a raster at all.
    ///
    /// False for spherical-harmonic coefficients (wavenumber space, decoded by
    /// [`crate::Grib1Reader::decode_spectral_message`]) and for a grid type
    /// this parser does not model. Those are exactly the variants for which
    /// [`Self::dimensions`], [`Self::first_point`] and [`Self::bounds`] have
    /// nothing to report, so a consumer that reads them without checking first
    /// gets `None` where it expected a number — the shape of the #288 crash,
    /// which was patched in the TypeScript host rather than named here.
    ///
    /// The converse does not hold for [`Self::bounds`] alone: a planar grid
    /// whose projection places no point has a raster but no far corner, because
    /// that corner is recovered from the projection rather than stated — a
    /// Lambert cone the standard parallels collapse, or either family's cell
    /// declared wider than the plane it sits in. `dimensions` and `first_point`
    /// are `Some` exactly when this is true.
    pub fn has_raster(&self) -> bool {
        match self {
            Self::LatLon(_)
            | Self::RotatedLatLon(_)
            | Self::ReducedLatLon(_)
            | Self::Gaussian(_)
            | Self::ReducedGaussian(_)
            | Self::PolarStereographic(_)
            | Self::LambertConformal(_) => true,
            Self::SphericalHarmonic(_) | Self::Unsupported { .. } => false,
        }
    }

    /// Grid dimensions, if available. For reduced grids `Ni` is the *widest*
    /// row (`max(points_per_row)`) — the column count a row-expanded raster
    /// needs — paired with the true row count `Nj`.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        match self {
            Self::LatLon(g) => Some((g.ni, g.nj)),
            Self::RotatedLatLon(g) => Some((g.ni, g.nj)),
            Self::ReducedLatLon(g) => Some((
                fieldglass_core::reduced_raster_width(&g.points_per_row),
                g.nj,
            )),
            Self::Gaussian(g) => Some((g.ni, g.nj)),
            Self::ReducedGaussian(g) => Some((
                fieldglass_core::reduced_raster_width(&g.points_per_row),
                g.nj,
            )),
            Self::PolarStereographic(g) => Some((g.nx, g.ny)),
            Self::LambertConformal(g) => Some((g.nx, g.ny)),
            // Spectral coefficients are not laid out on a grid, so there is no
            // Ni x Nj to report. The scalar decode path refuses on this basis.
            Self::SphericalHarmonic(_) => None,
            Self::Unsupported { .. } => None,
        }
    }

    /// How the file names its own grid, where `Ni × Nj` is not how it is
    /// described.
    ///
    /// Spectral coefficients are not laid out in rows and columns, so `Ni × Nj`
    /// is meaningless for them; a reduced Gaussian grid has rows of differing
    /// width, so the pair [`Self::dimensions`] reports is a raster this crate
    /// derives rather than anything the file says. Both state their size
    /// precisely in their own terms — a truncation, a Gaussian number — and
    /// that is what this returns. Mirrors
    /// `fieldglass_grib2::gds::GridDefinition::size_label`.
    ///
    /// `None` for a grid whose `Ni × Nj` *is* its description, which is most of
    /// them. A caller showing a size should prefer this where it is present
    /// and fall back to [`Self::dimensions`].
    pub fn size_label(&self) -> Option<String> {
        match self {
            // Triangular (J = K = M) is what real data carries; a pentagonal
            // message says so rather than being flattened to a wrong `T`.
            Self::SphericalHarmonic(g) => Some(if g.j == g.k && g.k == g.m {
                format!("T{}", g.j)
            } else {
                format!("J{} K{} M{}", g.j, g.k, g.m)
            }),
            // A reduced Gaussian grid does have `dimensions()` here — the
            // widest row paired with the row count, which is the raster a
            // row-expanded field needs — but that is a shape this crate
            // computes, not one the file states. The file states `N32`, and
            // that is what every tool printing this grid shows, so it is what
            // the size column should show too. GRIB2 answers the same way.
            Self::ReducedGaussian(g) => Some(format!(
                "{}{}",
                if fieldglass_core::is_octahedral_pl(&g.points_per_row) {
                    "O"
                } else {
                    "N"
                },
                g.n_gaussians
            )),
            _ => None,
        }
    }

    /// Number of stored data points. For regular grids this is `Ni·Nj`; for
    /// reduced grids it is the sum of the `PL` list, since rows differ in width.
    pub fn num_data_points(&self) -> Option<usize> {
        match self {
            Self::ReducedLatLon(g) => Some(g.points_per_row.iter().map(|&n| n as usize).sum()),
            Self::ReducedGaussian(g) => Some(g.points_per_row.iter().map(|&n| n as usize).sum()),
            _ => {
                let (ni, nj) = self.dimensions()?;
                (ni as usize).checked_mul(nj as usize)
            }
        }
    }

    /// The per-row point counts (`PL` list) for a reduced grid; `None` for the
    /// regular grids whose rows are all `Ni` wide.
    pub fn points_per_row(&self) -> Option<&[u32]> {
        match self {
            Self::ReducedLatLon(g) => Some(&g.points_per_row),
            Self::ReducedGaussian(g) => Some(&g.points_per_row),
            _ => None,
        }
    }

    /// The `(lat, lon)` of the declared first grid point. Unlike
    /// [`Self::bounds`] this survives a projection too degenerate to place the
    /// far corner: the message states where the grid starts either way.
    pub fn first_point(&self) -> Option<(f64, f64)> {
        match self {
            Self::LatLon(g) => Some((g.lat_first, g.lon_first)),
            Self::RotatedLatLon(g) => Some((g.lat_first, g.lon_first)),
            Self::ReducedLatLon(g) => Some((g.lat_first, g.lon_first)),
            Self::Gaussian(g) => Some((g.lat_first, g.lon_first)),
            Self::ReducedGaussian(g) => Some((g.lat_first, g.lon_first)),
            Self::PolarStereographic(g) => Some((g.lat_first, g.lon_first)),
            Self::LambertConformal(g) => Some((g.lat_first, g.lon_first)),
            Self::SphericalHarmonic(_) | Self::Unsupported { .. } => None,
        }
    }

    /// The corners **as the message states them**.
    ///
    /// For [`Self::RotatedLatLon`] these are the corner coordinates in the
    /// rotated frame, not geographic; unrotating them is the reprojector's job.
    ///
    /// For a reduced grid this is not the box the raster
    /// [`Self::dimensions`] reports occupies — see [`Self::raster_bounds`],
    /// which is what a consumer placing decoded values wants.
    pub fn bounds(&self) -> Option<CornerPair> {
        match self {
            Self::LatLon(g) => Some(CornerPair::new(
                g.lat_first,
                g.lon_first,
                g.lat_last,
                g.lon_last,
            )),
            Self::RotatedLatLon(g) => Some(CornerPair::new(
                g.lat_first,
                g.lon_first,
                g.lat_last,
                g.lon_last,
            )),
            Self::ReducedLatLon(g) => Some(CornerPair::new(
                g.lat_first,
                g.lon_first,
                g.lat_last,
                g.lon_last,
            )),
            Self::Gaussian(g) => Some(CornerPair::new(
                g.lat_first,
                g.lon_first,
                g.lat_last,
                g.lon_last,
            )),
            Self::ReducedGaussian(g) => Some(CornerPair::new(
                g.lat_first,
                g.lon_first,
                g.lat_last,
                g.lon_last,
            )),
            // Neither states a second corner, so it is derived from the
            // projection; a grid whose projection cannot place one reports no
            // pair at all rather than a `NaN` (its declared first point is
            // still available from [`Self::first_point`]).
            Self::PolarStereographic(g) => g
                .last_point()
                .map(|(la2, lo2)| CornerPair::new(g.lat_first, g.lon_first, la2, lo2)),
            Self::LambertConformal(g) => g
                .last_point()
                .map(|(la2, lo2)| CornerPair::new(g.lat_first, g.lon_first, la2, lo2)),
            // Spectral coefficients have no corner coordinates: the field is
            // global by construction and lives in wavenumber space.
            Self::SphericalHarmonic(_) => None,
            Self::Unsupported { .. } => None,
        }
    }

    /// The corners of the raster [`Self::dimensions`] describes.
    ///
    /// Identical to [`Self::bounds`] for every grid whose rows are all the same
    /// width, which is most of them. It differs for a reduced grid, and only in
    /// the eastern corner: [`Self::dimensions`] reports the widest row, and the
    /// raster those rows expand into puts its last column at
    /// `lon_first + (width - 1)·360/width` by construction. The message's own
    /// `Lo2` describes the `4N`-wide reference grid instead — the same number
    /// for a classic `N32`, wrong by up to an eighth of a cell for an
    /// octahedral `O32`, whose widest row is 144 against a declared 128. See
    /// [`fieldglass_core::reduced_raster_lon_last`].
    ///
    /// Pair this with [`crate::Grib1Reader::decode_message_raster`]: together
    /// they are the shape, the values and the extent of one rectangle, with no
    /// correction left for the caller. A message table showing what the file
    /// says wants [`Self::bounds`].
    pub fn raster_bounds(&self) -> Option<CornerPair> {
        let corners = self.bounds()?;
        match self.points_per_row() {
            Some(pl) => Some(CornerPair {
                lon_last: fieldglass_core::reduced_raster_lon_last(
                    corners.lon_first,
                    fieldglass_core::reduced_raster_width(pl),
                ),
                ..corners
            }),
            None => Some(corners),
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
    let section = &bytes[..section_len];
    // A reduced (quasi-regular) grid encodes Ni as the 2-byte missing marker
    // (0xFFFF) and carries a per-row PL list instead. Needs octets 7-8.
    let ni_is_missing = section_len >= 8 && section[6] == 0xFF && section[7] == 0xFF;

    match grid_type {
        0 if ni_is_missing => Ok(GridDescription::ReducedLatLon(parse_reduced_latlon(
            section,
        )?)),
        0 => Ok(GridDescription::LatLon(parse_latlon(section)?)),
        3 => Ok(GridDescription::LambertConformal(parse_lambert(section)?)),
        50 => Ok(GridDescription::SphericalHarmonic(
            parse_spherical_harmonic(section)?,
        )),
        4 if ni_is_missing => Ok(GridDescription::ReducedGaussian(parse_reduced_gaussian(
            section,
        )?)),
        4 => Ok(GridDescription::Gaussian(parse_gaussian(section)?)),
        10 => Ok(GridDescription::RotatedLatLon(parse_rotated_latlon(
            section,
        )?)),
        5 => Ok(GridDescription::PolarStereographic(parse_polar_stereo(
            section,
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

fn parse_reduced_latlon(b: &[u8]) -> Result<ReducedLatLonGrid, FieldglassError> {
    require_len(b, 32, "Reduced LatLon GDS")?;
    let nj = u16::from_be_bytes([b[8], b[9]]) as u32;
    let points_per_row = parse_pl_list(b, nj)?;
    Ok(ReducedLatLonGrid {
        nj,
        lat_first: read_signed_magnitude_24(&b[10..13]) as f64 / 1000.0,
        lon_first: read_signed_magnitude_24(&b[13..16]) as f64 / 1000.0,
        resolution_flags: ResolutionFlags::from_byte(b[16]),
        lat_last: read_signed_magnitude_24(&b[17..20]) as f64 / 1000.0,
        lon_last: read_signed_magnitude_24(&b[20..23]) as f64 / 1000.0,
        dj: u16::from_be_bytes([b[25], b[26]]) as f64 / 1000.0,
        scanning_mode: ScanningMode::from_byte(b[27]),
        points_per_row,
    })
}

fn parse_reduced_gaussian(b: &[u8]) -> Result<ReducedGaussianGrid, FieldglassError> {
    require_len(b, 32, "Reduced Gaussian GDS")?;
    let nj = u16::from_be_bytes([b[8], b[9]]) as u32;
    let points_per_row = parse_pl_list(b, nj)?;
    Ok(ReducedGaussianGrid {
        nj,
        lat_first: read_signed_magnitude_24(&b[10..13]) as f64 / 1000.0,
        lon_first: read_signed_magnitude_24(&b[13..16]) as f64 / 1000.0,
        resolution_flags: ResolutionFlags::from_byte(b[16]),
        lat_last: read_signed_magnitude_24(&b[17..20]) as f64 / 1000.0,
        lon_last: read_signed_magnitude_24(&b[20..23]) as f64 / 1000.0,
        n_gaussians: u16::from_be_bytes([b[25], b[26]]),
        scanning_mode: ScanningMode::from_byte(b[27]),
        points_per_row,
    })
}

/// Read the `PL` list — `Nj` big-endian `u16` point-counts, one per row — from
/// a reduced grid's GDS. Following eccodes `grib1/section.2.def`: the PV/PL
/// block begins at octet `pvlLocation` (GDS octet 5, 1-based; 33 when unset),
/// the optional `NV` vertical-coordinate IBM floats (4 bytes each) come first,
/// then the `PL` list.
fn parse_pl_list(b: &[u8], nj: u32) -> Result<Vec<u32>, FieldglassError> {
    let nv = b[3] as usize;
    let pvl_location = b[4] as usize;
    // pvlLocation is a 1-based octet index; 255 ("neither present") falls back
    // to the fixed post-grid-definition offset (octet 33 → index 32).
    let block_start = if pvl_location != 255 {
        pvl_location.saturating_sub(1)
    } else {
        32
    };
    let pl_start = block_start + nv * 4;
    let nj = nj as usize;
    let pl_end = pl_start + nj * 2;
    if b.len() < pl_end {
        return Err(FieldglassError::Parse(format!(
            "reduced grid PL list needs {pl_end} bytes, GDS section has {}",
            b.len()
        )));
    }
    Ok((0..nj)
        .map(|i| {
            let off = pl_start + i * 2;
            u16::from_be_bytes([b[off], b[off + 1]]) as u32
        })
        .collect())
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

/// Parse a spherical-harmonic GDS (data representation type 50).
///
/// Octets 7-8 / 9-10 / 11-12 are the pentagonal resolution parameters J, K, M;
/// octet 13 is the representation type (1 = associated Legendre polynomials) and
/// octet 14 the representation mode (1 = the complex packing ECMWF writes).
/// There is no `Ni`/`Nj` — a spectral message describes coefficients, not points.
fn parse_spherical_harmonic(b: &[u8]) -> Result<SphericalHarmonicGrid, FieldglassError> {
    if b.len() < 14 {
        return Err(FieldglassError::Parse(format!(
            "spherical-harmonic GDS requires 14 octets, got {}",
            b.len()
        )));
    }
    Ok(SphericalHarmonicGrid {
        j: u16::from_be_bytes([b[6], b[7]]),
        k: u16::from_be_bytes([b[8], b[9]]),
        m: u16::from_be_bytes([b[10], b[11]]),
        representation_type: b[12],
        representation_mode: b[13],
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

/// A derived corner, or `None` when the projection could not place one, with the
/// longitude wrapped into the half-open range (-180, 180].
///
/// Takes the projector's `is_well_defined` as well as the coordinates, the same
/// shape `fieldglass_grib2`'s helper of the same name has, and for the same
/// reason: a projection can be too degenerate to place a point while every
/// number it produces stays finite, so finiteness alone is not the test.
///
/// GRIB1 states less of its projection than §3 does, and the two collapses a §3
/// message reaches through its declared Earth are indeed out of reach here:
/// [`ResolutionFlags::earth_radius_m`] answers one of two constants from a
/// single flag bit, so there is no declared zero (#603), and the polar
/// stereographic `LaD` is fixed at 60°, so the pole scale factor cannot be
/// driven to zero either. Three collapses remain, and GRIB1 states all three —
/// but only one of them needs this argument.
///
/// Two go non-finite on their own, and finiteness has always caught them: the
/// standard parallels are stated, so a Lambert cone still collapses on
/// `Latin1 == Latin2 == 0` (a cone constant of zero); and `La1` is three octets
/// of sign-magnitude millidegrees, so a northern cone can be handed a first
/// point at the south pole, where `tan(π/4 + φ/2)` is exactly zero and the
/// projected origin runs to infinity.
///
/// The third is the quiet one, and it is why the flag is here. Dx/Dy are three
/// octets of unsigned metres, so a grid can declare a 16 777 km cell: wider than
/// either GRIB1 Earth, and wider than the 11 882 km plane a ±60° latitude of
/// true scale leaves the polar stereographic family. The whole projection
/// collapses inside one cell (#610) with every number staying finite — a corner
/// comes back, and it is not a position.
fn finite_lonlat(well_defined: bool, (lat, lon): (f64, f64)) -> Option<(f64, f64)> {
    (well_defined && lat.is_finite() && lon.is_finite()).then(|| (lat, normalise_longitude(lon)))
}

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
        let CornerPair {
            lat_first: la1,
            lon_first: lo1,
            lat_last: la2,
            lon_last: lo2,
        } = parsed.bounds().expect("Lambert has bounds");
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
            earth_radius_m: g.resolution_flags.earth_radius_m(),
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

    /// Both standard parallels on the equator give a cone constant of zero, so
    /// the forward map divides by it and the recovered corner is `NaN`. It has
    /// always been refused; the test is here so the two ways a GRIB1 planar
    /// grid loses its corner sit side by side.
    #[test]
    fn lambert_collapsed_cone_reports_no_corner() {
        let mut body = Vec::new();
        body.extend(u16be(601));
        body.extend(u16be(401));
        body.extend(sm24(38_500));
        body.extend(sm24(-126_000));
        body.push(0xC0);
        body.extend(sm24(-95_000));
        body.extend(u24(13_545));
        body.extend(u24(13_545));
        body.push(0);
        body.push(0x40);
        body.extend(sm24(0)); // latin1 = 0
        body.extend(sm24(0)); // latin2 = 0 ⇒ n = sin 0 = 0
        body.extend(sm24(0));
        body.extend(sm24(0));

        let parsed = parse_grid_description(&build_gds(3, &body)).expect("parses");
        assert!(parsed.has_raster(), "the raster shape is still declared");
        assert_eq!(parsed.first_point(), Some((38.500, -126.000)));
        assert_eq!(parsed.bounds(), None, "a collapsed cone places no corner");
    }

    /// Dx/Dy are three octets of unsigned metres, so a GRIB1 message can
    /// declare a cell wider than the Earth it is projected on. Nothing goes
    /// non-finite there — the corner inverts to a perfectly ordinary lat/lon —
    /// but the projection has collapsed inside one cell and the position means
    /// nothing, so no corner is reported (#610).
    #[test]
    fn lambert_cell_wider_than_the_plane_reports_no_corner() {
        let mut body = Vec::new();
        body.extend(u16be(2));
        body.extend(u16be(2));
        body.extend(sm24(38_500));
        body.extend(sm24(-126_000));
        body.push(0xC0);
        body.extend(sm24(-95_000));
        body.extend(u24(16_777_215)); // dx_m — the widest a u24 can say
        body.extend(u24(16_777_215)); // dy_m, both > the 6 367 470 m Earth
        body.push(0);
        body.push(0x40);
        body.extend(sm24(38_500));
        body.extend(sm24(38_500));
        body.extend(sm24(0));
        body.extend(sm24(0));

        let parsed = parse_grid_description(&build_gds(3, &body)).expect("parses");
        let GridDescription::LambertConformal(g) = &parsed else {
            panic!("expected LambertConformal");
        };
        // The corner the projection hands back is finite — this is exactly the
        // case finiteness cannot catch.
        let (dx, dy) = g.signed_increments();
        let projector = LambertProjector::new(LambertParams {
            earth_radius_m: g.resolution_flags.earth_radius_m(),
            ni: g.nx,
            nj: g.ny,
            lat_first: g.lat_first,
            lon_first: g.lon_first,
            lad: g.latin1,
            lov: g.lov,
            dx_metres: dx,
            dy_metres: dy,
            latin1: g.latin1,
            latin2: g.latin2,
        });
        let (lat, lon) = projector.last_grid_point_lonlat();
        assert!(lat.is_finite() && lon.is_finite(), "({lat}, {lon})");
        assert!(!projector.is_well_defined(), "the plane holds no cell");

        assert!(parsed.has_raster(), "the raster shape is still declared");
        assert_eq!(parsed.bounds(), None, "a collapsed plane places no corner");
    }

    /// The third way a GRIB1 Lambert grid loses its corner: a usable cone
    /// handed a first point at the pole it opens away from. `La1` is three
    /// octets of sign-magnitude millidegrees, so -90.000 is as declarable as
    /// any other latitude, and there `tan(π/4 + φ/2)` is exactly zero — the
    /// forward map divides by it and the projected origin runs to infinity.
    /// This one has always been refused, by finiteness rather than by the
    /// projector's own verdict; the test is here so all three sit together.
    #[test]
    fn lambert_first_point_at_the_far_pole_reports_no_corner() {
        let mut body = Vec::new();
        body.extend(u16be(601));
        body.extend(u16be(401));
        body.extend(sm24(-90_000)); // lat_first — the pole a northern cone opens away from
        body.extend(sm24(-126_000));
        body.push(0xC0);
        body.extend(sm24(-95_000));
        body.extend(u24(13_545));
        body.extend(u24(13_545));
        body.push(0);
        body.push(0x40);
        body.extend(sm24(38_500)); // a perfectly ordinary cone
        body.extend(sm24(38_500));
        body.extend(sm24(0));
        body.extend(sm24(0));

        let parsed = parse_grid_description(&build_gds(3, &body)).expect("parses");
        let GridDescription::LambertConformal(g) = &parsed else {
            panic!("expected LambertConformal");
        };
        let (dx, dy) = g.signed_increments();
        let projector = LambertProjector::new(LambertParams {
            earth_radius_m: g.resolution_flags.earth_radius_m(),
            ni: g.nx,
            nj: g.ny,
            lat_first: g.lat_first,
            lon_first: g.lon_first,
            lad: g.latin1,
            lov: g.lov,
            dx_metres: dx,
            dy_metres: dy,
            latin1: g.latin1,
            latin2: g.latin2,
        });
        let (ox, oy) = projector.origin();
        assert!(!ox.is_finite() || !oy.is_finite(), "origin ({ox}, {oy})");
        assert!(!projector.is_well_defined());

        assert!(parsed.has_raster(), "the raster shape is still declared");
        assert_eq!(parsed.first_point(), Some((-90.0, -126.000)));
        assert_eq!(
            parsed.bounds(),
            None,
            "an unreachable origin places no corner"
        );
    }

    /// The polar stereographic half of the same rule. ON388 fixes the latitude
    /// of true scale at ±60°, so the plane is `2·R·k₀` ≈ 11 882 km wide — and a
    /// u24 Dx reaches past it.
    #[test]
    fn polar_stereo_cell_wider_than_the_plane_reports_no_corner() {
        let mut body = Vec::new();
        body.extend(u16be(2)); // nx
        body.extend(u16be(2)); // ny
        body.extend(sm24(-20_826)); // lat_first
        body.extend(sm24(-145_000)); // lon_first
        body.push(0x88);
        body.extend(sm24(-80_000)); // lov
        body.extend(u24(16_777_215)); // dx_m
        body.extend(u24(16_777_215)); // dy_m
        body.push(0x80); // south pole on plane
        body.push(0x40);

        let parsed = parse_grid_description(&build_gds(5, &body)).expect("parses");
        let GridDescription::PolarStereographic(g) = &parsed else {
            panic!("expected PolarStereographic");
        };
        let (dx, dy) = g.signed_increments();
        let projector = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: g.resolution_flags.earth_radius_m(),
            ni: g.nx,
            nj: g.ny,
            lat_first: g.lat_first,
            lon_first: g.lon_first,
            lov: g.lov,
            lad: g.lad(),
            dx_metres: dx,
            dy_metres: dy,
            south_pole: g.south_pole,
        });
        let (lat, lon) = projector.last_grid_point_lonlat();
        assert!(lat.is_finite() && lon.is_finite(), "({lat}, {lon})");
        assert!(!projector.is_well_defined(), "the plane holds no cell");

        assert!(parsed.has_raster(), "the raster shape is still declared");
        assert_eq!(parsed.bounds(), None, "a collapsed plane places no corner");
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
        assert_eq!(
            parsed.bounds(),
            Some(CornerPair::new(-18.0, -12.0, 20.0, 15.0))
        );
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

    /// Build a reduced-grid GDS: the 22-octet grid header (octets 7-28), four
    /// reserved octets, then the `PL` list. Sets `pvlLocation` (octet 5) to 255
    /// so the parser falls back to the post-grid-definition offset (octet 33),
    /// which is exactly where the appended `PL` list begins.
    fn build_reduced_gds(grid_type: u8, header: &[u8], pl: &[u16]) -> Vec<u8> {
        assert_eq!(header.len(), 22, "grid header is octets 7-28");
        let mut body = Vec::new();
        body.extend_from_slice(header);
        body.extend([0, 0, 0, 0]); // octets 29-32, reserved
        for &count in pl {
            body.extend(u16be(count));
        }
        let mut gds = build_gds(grid_type, &body);
        gds[4] = 255; // pvlLocation = neither-present sentinel
        gds
    }

    /// The octets 7-28 grid header shared by reduced lat/lon and Gaussian: Ni
    /// missing, four rows, a 60°N..60°S / 0..350°E box. The two trailing octets
    /// (`tail`) are Dj for lat/lon or N for Gaussian.
    fn reduced_header(tail: [u8; 2]) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend(u16be(0xFFFF)); // Ni missing → reduced
        h.extend(u16be(4)); // Nj = 4 rows
        h.extend(sm24(60_000)); // lat_first = 60.000°
        h.extend(sm24(0)); // lon_first = 0.000°
        h.push(0x80); // resolution flags: increments_given
        h.extend(sm24(-60_000)); // lat_last = -60.000°
        h.extend(sm24(350_000)); // lon_last = 350.000°
        h.extend(u16be(0xFFFF)); // Di missing (varies per row)
        h.extend(tail); // Dj (lat/lon) or N (Gaussian)
        h.push(0x00); // scanning mode
        h
    }

    #[test]
    fn parses_reduced_latlon_gds() {
        // Four rows of 4, 8, 8, 4 points → 24 stored points, widest row 8.
        let header = reduced_header(u16be(2_500)); // Dj = 2.5°
        let gds = build_reduced_gds(0, &header, &[4, 8, 8, 4]);
        let parsed = parse_grid_description(&gds).expect("reduced lat/lon GDS parses");
        assert_eq!(parsed.grid_type_name(), "reduced_latlon");
        assert_eq!(parsed.dimensions(), Some((8, 4)), "Ni = widest row");
        assert_eq!(parsed.num_data_points(), Some(24), "sum of PL");
        assert_eq!(parsed.points_per_row(), Some([4u32, 8, 8, 4].as_slice()));
        assert_eq!(
            parsed.bounds(),
            Some(CornerPair::new(60.0, 0.0, -60.0, 350.0))
        );
        let GridDescription::ReducedLatLon(g) = parsed else {
            panic!("expected ReducedLatLon");
        };
        assert_eq!(g.nj, 4);
        assert_eq!(g.dj, 2.5);
    }

    /// `raster_bounds()` describes the rectangle a decoded field lands in;
    /// `bounds()` keeps saying what the file says (#543).
    ///
    /// The two differ for a reduced grid, and only in the eastern corner. This
    /// grid's rows step by exactly four (`is_octahedral_pl`), so its widest row
    /// is 32 while the declared `Lo2` of 350° describes a narrower reference
    /// grid — trusting it would place 32 columns on a span that holds fewer.
    #[test]
    fn raster_bounds_derives_the_east_edge_of_a_reduced_grid() {
        let mut header = reduced_header(u16be(4)); // N = 4
        header[2..4].copy_from_slice(&u16be(8)); // Nj = 8 rows
        let pl = [20u16, 24, 28, 32, 32, 28, 24, 20];
        let parsed = parse_grid_description(&build_reduced_gds(4, &header, &pl))
            .expect("octahedral reduced Gaussian GDS parses");

        let widths: Vec<u32> = pl.iter().map(|&n| u32::from(n)).collect();
        assert!(
            fieldglass_core::is_octahedral_pl(&widths),
            "the premise: rows step by four"
        );
        assert_eq!(
            parsed.dimensions(),
            Some((32, 8)),
            "widest row by row count"
        );

        // Unchanged: the message table shows the file's own corner.
        assert_eq!(
            parsed.bounds(),
            Some(CornerPair::new(60.0, 0.0, -60.0, 350.0))
        );

        let CornerPair {
            lat_first: la1,
            lon_first: lo1,
            lat_last: la2,
            lon_last: lo2,
        } = parsed.raster_bounds().expect("a reduced grid has a raster");
        assert_eq!((la1, lo1, la2), (60.0, 0.0, -60.0), "only Lo2 is derived");
        // 32 columns around the circle put the last one at 31 * 360/32.
        assert!((lo2 - 348.75).abs() < 1e-9, "raster east edge {lo2}");
        assert_ne!(lo2, 350.0, "the declared corner is not the raster's");
    }

    /// Every grid whose rows are all `Ni` wide reports the same box twice —
    /// the correction above is reduced-grid-only, not a second geometry.
    #[test]
    fn raster_bounds_matches_bounds_for_a_regular_grid() {
        let mut body = Vec::new();
        body.extend(u16be(360)); // ni
        body.extend(u16be(181)); // nj
        body.extend(sm24(90_000)); // lat_first = 90.000°
        body.extend(sm24(0)); // lon_first = 0.000°
        body.push(0x80); // resolution flags: increments_given
        body.extend(sm24(-90_000)); // lat_last = -90.000°
        body.extend(sm24(359_000)); // lon_last = 359.000°
        body.extend(u16be(1_000)); // Di = 1.000°
        body.extend(u16be(1_000)); // Dj = 1.000°
        body.push(0x00); // scanning mode
        let parsed = parse_grid_description(&build_gds(0, &body)).expect("lat/lon GDS parses");
        assert_eq!(parsed.raster_bounds(), parsed.bounds());
        assert!(parsed.bounds().is_some(), "and it is not None twice");
    }

    #[test]
    fn parses_reduced_gaussian_gds() {
        // N = 2 (two Gaussian latitudes pole-to-equator), rows 4, 8, 8, 4.
        let header = reduced_header(u16be(2)); // N = 2
        let gds = build_reduced_gds(4, &header, &[4, 8, 8, 4]);
        let parsed = parse_grid_description(&gds).expect("reduced Gaussian GDS parses");
        assert_eq!(parsed.grid_type_name(), "reduced_gaussian");
        assert_eq!(parsed.dimensions(), Some((8, 4)));
        assert_eq!(parsed.num_data_points(), Some(24));
        assert_eq!(parsed.points_per_row(), Some([4u32, 8, 8, 4].as_slice()));
        let GridDescription::ReducedGaussian(g) = parsed else {
            panic!("expected ReducedGaussian");
        };
        assert_eq!(g.nj, 4);
        assert_eq!(g.n_gaussians, 2);
    }

    #[test]
    fn reduced_grid_truncated_pl_list_errors() {
        // Promise four rows but supply only two PL entries.
        let header = reduced_header(u16be(2_500));
        let mut body = Vec::new();
        body.extend_from_slice(&header);
        body.extend([0, 0, 0, 0]);
        body.extend(u16be(4));
        body.extend(u16be(8)); // only 2 of the 4 promised rows
        let mut gds = build_gds(0, &body);
        gds[4] = 255;
        let Err(err) = parse_grid_description(&gds) else {
            panic!("truncated PL list should error");
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
        let CornerPair {
            lat_first: la1,
            lon_first: lo1,
            lat_last: la2,
            lon_last: lo2,
        } = parsed.bounds().expect("polar stereo has bounds");
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
            earth_radius_m: g.resolution_flags.earth_radius_m(),
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
        // grid_type 13 (oblique Lambert) isn't one we implement; the parser
        // should return the Unsupported variant carrying the offending byte
        // rather than fail. Body bytes are irrelevant for the unsupported
        // branch, but the section must still pass the length-prefix validation.
        // (50 used to stand in here; it is spherical-harmonic and now parses.)
        let body = vec![0u8; 22];
        let gds = build_gds(13, &body);
        let parsed = parse_grid_description(&gds).expect("unsupported parses cleanly");
        let GridDescription::Unsupported { grid_type } = parsed else {
            panic!("expected Unsupported variant");
        };
        assert_eq!(grid_type, 13);
    }

    /// The scanning mode decides which way the walk to the far corner runs.
    /// Dx/Dy are unsigned magnitudes, so a north→south grid that ignored the
    /// flag would report a corner on the wrong side of its own first point —
    /// and disagree with the warp, which has always applied the sign (#472).
    /// A collapsed cone used to put the text "NaN" in the message table's
    /// coordinate columns. The corner is now simply absent, and the first
    /// point — which the message states outright — survives.
    #[test]
    fn a_degenerate_lambert_reports_no_corner_rather_than_nan() {
        let mut body = Vec::new();
        body.extend(u16be(10)); // nx
        body.extend(u16be(10)); // ny
        body.extend(sm24(45_000)); // lat_first
        body.extend(sm24(-110_000)); // lon_first
        body.push(0x88);
        body.extend(sm24(-95_000)); // lov
        body.extend(u24(12_000)); // dx
        body.extend(u24(12_000)); // dy
        body.push(0x00);
        body.push(0x40); // scanning mode
        body.extend(sm24(0)); // latin1 = 0
        body.extend(sm24(0)); // latin2 = 0
        body.extend(sm24(0)); // lat south pole
        body.extend(sm24(0)); // lon south pole
        let parsed = parse_grid_description(&build_gds(3, &body)).expect("parses");
        assert_eq!(parsed.bounds(), None, "no corner pair for a collapsed cone");
        assert_eq!(
            parsed.first_point(),
            Some((45.0, -110.0)),
            "the declared first point is still reported"
        );
    }

    #[test]
    fn polar_stereo_bounds_follow_the_scanning_mode() {
        let corner_for = |scanning_mode: u8| {
            let mut body = Vec::new();
            body.extend(u16be(200)); // nx
            body.extend(u16be(200)); // ny
            body.extend(sm24(60_000)); // lat_first = 60°N
            body.extend(sm24(-100_000)); // lon_first
            body.push(0x88);
            body.extend(sm24(-100_000)); // lov
            body.extend(u24(20_000)); // dx = 20 km
            body.extend(u24(20_000)); // dy = 20 km
            body.push(0x00); // projection centre: north pole on plane
            body.push(scanning_mode);
            let parsed = parse_grid_description(&build_gds(5, &body)).expect("parses");
            let CornerPair {
                lat_last: la2,
                lon_last: lo2,
                ..
            } = parsed.bounds().expect("polar stereo has bounds");
            (la2, lo2)
        };

        // 0x40 = j scans positive, 0x00 = north→south. The first point sits on
        // the LoV meridian, which lies at negative y in a north-polar plane, so
        // stepping +y runs toward the pole and stepping −y runs away from it:
        // the two corners land on opposite sides of the origin, tens of degrees
        // apart. Before the sign was applied both read the +j answer.
        let (plus_lat, plus_lon) = corner_for(0x40);
        let (minus_lat, minus_lon) = corner_for(0x00);
        assert!(
            plus_lat > minus_lat + 10.0,
            "a +j grid reaches higher latitudes than a -j one: {plus_lat} vs {minus_lat}"
        );
        assert!(
            (plus_lon - minus_lon).abs() > 10.0,
            "and a different meridian: {plus_lon} vs {minus_lon}"
        );
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

#[cfg(test)]
mod accessor_tests {
    //! The grid-policy accessors (#544): the flags, signs and fixed constants
    //! a consumer would otherwise re-derive from the raw fields. Built from
    //! synthetic structs so every variant of the enum is covered, including
    //! the two that describe no raster; `tests/decode_real_grib1.rs` checks
    //! the same accessors against a real polar-stereographic message.

    use super::*;

    fn flags() -> ResolutionFlags {
        ResolutionFlags {
            increments_given: true,
            earth_oblate: false,
            uv_relative_to_grid: false,
        }
    }

    fn scan(i_negative: bool, j_positive: bool) -> ScanningMode {
        ScanningMode {
            i_negative,
            j_positive,
            j_consecutive: false,
        }
    }

    fn polar(scanning_mode: ScanningMode) -> PolarStereoGrid {
        PolarStereoGrid {
            nx: 135,
            ny: 95,
            lat_first: 27.203,
            lon_first: -135.213,
            lov: 249.0,
            dx_m: 60_000,
            dy_m: 60_000,
            south_pole: false,
            resolution_flags: flags(),
            scanning_mode,
        }
    }

    fn lambert(scanning_mode: ScanningMode) -> LambertGrid {
        LambertGrid {
            nx: 10,
            ny: 8,
            lat_first: 20.0,
            lon_first: -120.0,
            lov: -95.0,
            dx_m: 12_000,
            dy_m: 12_000,
            south_pole: false,
            latin1: 25.0,
            latin2: 25.0,
            lat_south_pole: -90.0,
            lon_south_pole: 0.0,
            resolution_flags: flags(),
            scanning_mode,
        }
    }

    /// A discriminant per variant. Adding a grid family to the enum stops this
    /// compiling until it is given an arm, which is what makes
    /// [`every_variant`]'s claim to cover the whole enum enforceable rather
    /// than a promise — see [`every_variant_is_one_of_each`].
    fn variant_tag(gds: &GridDescription) -> usize {
        match gds {
            GridDescription::LatLon(_) => 0,
            GridDescription::RotatedLatLon(_) => 1,
            GridDescription::ReducedLatLon(_) => 2,
            GridDescription::Gaussian(_) => 3,
            GridDescription::ReducedGaussian(_) => 4,
            GridDescription::PolarStereographic(_) => 5,
            GridDescription::LambertConformal(_) => 6,
            GridDescription::SphericalHarmonic(_) => 7,
            GridDescription::Unsupported { .. } => 8,
        }
    }

    /// One of every variant, so the exhaustive-match accessors below are
    /// checked against the whole enum rather than the grids a test happened to
    /// name.
    fn every_variant() -> Vec<GridDescription> {
        let latlon = |scanning_mode| LatLonGrid {
            ni: 4,
            nj: 3,
            lat_first: 60.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 30.0,
            di: 10.0,
            dj: 30.0,
            resolution_flags: flags(),
            scanning_mode,
        };
        vec![
            GridDescription::LatLon(latlon(scan(false, false))),
            GridDescription::RotatedLatLon(RotatedLatLonGrid {
                ni: 4,
                nj: 3,
                lat_first: 60.0,
                lon_first: 0.0,
                lat_last: 0.0,
                lon_last: 30.0,
                di: 10.0,
                dj: 30.0,
                south_pole_lat: -30.0,
                south_pole_lon: 10.0,
                angle_of_rotation: 0.0,
                resolution_flags: flags(),
                scanning_mode: scan(true, false),
            }),
            GridDescription::ReducedLatLon(ReducedLatLonGrid {
                nj: 2,
                lat_first: 60.0,
                lon_first: 0.0,
                lat_last: 30.0,
                lon_last: 350.0,
                dj: 30.0,
                points_per_row: vec![4, 6],
                resolution_flags: flags(),
                scanning_mode: scan(false, true),
            }),
            GridDescription::Gaussian(GaussianGrid {
                ni: 8,
                nj: 4,
                lat_first: 60.0,
                lon_first: 0.0,
                lat_last: -60.0,
                lon_last: 315.0,
                di: 45.0,
                n_gaussians: 2,
                resolution_flags: flags(),
                scanning_mode: scan(false, false),
            }),
            GridDescription::ReducedGaussian(ReducedGaussianGrid {
                nj: 2,
                lat_first: 60.0,
                lon_first: 0.0,
                lat_last: -60.0,
                lon_last: 350.0,
                n_gaussians: 1,
                points_per_row: vec![4, 6],
                resolution_flags: flags(),
                scanning_mode: scan(true, true),
            }),
            GridDescription::PolarStereographic(polar(scan(false, true))),
            GridDescription::LambertConformal(lambert(scan(false, false))),
            GridDescription::SphericalHarmonic(SphericalHarmonicGrid {
                j: 63,
                k: 63,
                m: 63,
                representation_type: 1,
                representation_mode: 1,
            }),
            GridDescription::Unsupported { grid_type: 13 },
        ]
    }

    /// The list above is one of each, in discriminant order. Without this, a
    /// new grid family could be given a wrong arm in `has_raster` and no test
    /// below would ever see it: they only check the examples the list holds.
    #[test]
    fn every_variant_is_one_of_each() {
        let tags: Vec<usize> = every_variant().iter().map(variant_tag).collect();
        assert_eq!(tags, (0..=8).collect::<Vec<_>>());
    }

    /// The flags come back from the variant that holds them, and only the two
    /// non-raster variants have none. Reading the pair off each grid is what
    /// the hand-written arm lists in the hosts used to do.
    #[test]
    fn scanning_mode_reports_the_flags_of_every_grid_family() {
        let expected = [
            Some(scan(false, false)),
            Some(scan(true, false)),
            Some(scan(false, true)),
            Some(scan(false, false)),
            Some(scan(true, true)),
            Some(scan(false, true)),
            Some(scan(false, false)),
            None,
            None,
        ];
        let got: Vec<Option<ScanningMode>> = every_variant()
            .iter()
            .map(|g| g.scanning_mode().copied())
            .collect();
        assert_eq!(got, expected);
    }

    /// `has_raster` is exactly "dimensions and first point are reportable" —
    /// the property a consumer actually wants when it asks. If a new variant
    /// makes the two disagree, this fails rather than the host crashing on a
    /// `None` it did not expect (#288).
    #[test]
    fn has_raster_agrees_with_dimensions_and_first_point() {
        for gds in every_variant() {
            let name = gds.grid_type_name();
            assert_eq!(
                gds.has_raster(),
                gds.dimensions().is_some(),
                "{name}: has_raster disagrees with dimensions()"
            );
            assert_eq!(
                gds.has_raster(),
                gds.first_point().is_some(),
                "{name}: has_raster disagrees with first_point()"
            );
        }
        // Named, so the invariant above cannot be satisfied by everything
        // answering the same way.
        assert!(GridDescription::PolarStereographic(polar(scan(false, true))).has_raster());
        assert!(!GridDescription::Unsupported { grid_type: 13 }.has_raster());
    }

    /// Dx/Dy come out of the GDS as magnitudes; the scanning mode is what makes
    /// them a direction. A north-to-south scan — the operational default —
    /// walks `-Dy`.
    #[test]
    fn signed_increments_apply_the_scan_direction() {
        assert_eq!(
            polar(scan(false, false)).signed_increments(),
            (60_000.0, -60_000.0)
        );
        assert_eq!(
            polar(scan(true, true)).signed_increments(),
            (-60_000.0, 60_000.0)
        );
        assert_eq!(
            lambert(scan(false, false)).signed_increments(),
            (12_000.0, -12_000.0)
        );
        assert_eq!(
            lambert(scan(true, true)).signed_increments(),
            (-12_000.0, 12_000.0)
        );
    }

    /// ON388 fixes the true-scale parallel at ±60° and the projectors take the
    /// magnitude, so a South-Pole grid reports the same 60 — the hemisphere is
    /// `south_pole`, not the sign of `lad`.
    #[test]
    fn polar_stereo_lad_is_sixty_in_both_hemispheres() {
        let north = polar(scan(false, true));
        assert_eq!(north.lad(), 60.0);
        let mut south = polar(scan(false, true));
        south.south_pole = true;
        assert_eq!(south.lad(), 60.0);
    }
}
