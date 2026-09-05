//! Polar stereographic grids — GRIB1 `grid_type` 5, GRIB2 template 3.20.
//!
//! Snyder, PP-1395 §21 (sphere, polar aspect), eqs 21-33/21-34 (forward) and
//! 20-14/20-17 (inverse). The pole scale factor `k₀ = (1 + sin|LaD|)/2` follows
//! the latitude of true scale `LaD`: GRIB1 fixes it at ±60°
//! (`k₀ ≈ 0.93301270…`), while GRIB2 §3.20 carries `LaD` explicitly (e.g. true
//! scale at the pole → `k₀ = 1`).

use std::f64::consts::PI;

use super::{DEG2RAD, GridIndex, PlanarGridProjector, RAD2DEG};

/// A polar stereographic grid — GRIB1 `grid_type` 5, GRIB2 template 3.20.
/// The plane touches one pole and is metres.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PolarStereoParams {
    /// Radius of the spherical Earth the grid is projected on, in metres. See
    /// [`super::LambertParams::earth_radius_m`].
    pub earth_radius_m: f64,
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the grid origin (first scanned point), degrees.
    pub lat_first: f64,
    /// Longitude of the grid origin (first scanned point), degrees.
    pub lon_first: f64,
    /// Orientation longitude (`LoV`) — meridian parallel to the y-axis,
    /// degrees.
    pub lov: f64,
    /// Latitude of true scale (`LaD`) — the parallel at which `dx_metres` /
    /// `dy_metres` are specified, degrees. GRIB1 fixes this at ±60°; GRIB2
    /// §3.20 carries it explicitly, so grids whose true scale is at the pole
    /// (90°) or another parallel scale correctly.
    pub lad: f64,
    /// Grid spacing in metres along x at the latitude of true scale.
    pub dx_metres: f64,
    /// Grid spacing in metres along y at the latitude of true scale.
    pub dy_metres: f64,
    /// `true` ⇒ south-pole projection; `false` ⇒ north-pole. GRIB1 carries
    /// this in the projection-centre flag; GRIB2 in §3.20 octet 17 bit 2.
    pub south_pole: bool,
}

/// Pole-scale terms derived once from a [`PolarStereoParams`], since a warp
/// reuses them for every pixel.
///
/// `pub` so projector helpers can hand them around; the fields are private.
#[derive(Debug, Clone, Copy)]
pub struct PolarStereoConstants {
    /// `2 · R · k₀` where `k₀ = (1 + sin|LaD|)/2` is the pole scale factor for
    /// a projection whose latitude of true scale is `LaD` (Snyder PP-1395,
    /// eq. 21-15). The product is what every forward/inverse formula consumes.
    two_r_k0: f64,
    sign: f64,
}

impl PolarStereoConstants {
    /// Whether these constants describe a usable plane. `sign` is `±1` by
    /// construction and cannot go wrong, so the whole projection rests on
    /// `two_r_k0`.
    ///
    /// A declared Earth radius of zero (or less) is the case this exists for,
    /// and it is the same quiet one
    /// [`LambertConstants::well_defined`](super::LambertConstants) guards on the
    /// cone. Nothing goes infinite: `two_r_k0` is zero, so `rho` is zero for
    /// every latitude, the forward map sends the whole Earth to `(0, 0)` — the
    /// projected origin, since the origin is forward-projected too — and the
    /// index division answers `(0, 0)` for every point. A render then samples
    /// one cell for every pixel and paints a flat field of one value, with
    /// nothing to say it is wrong. A negative radius is louder but no better:
    /// `rho` comes back negative and the inverse reports latitudes past ±90°.
    ///
    /// `LaD` is checked here too, and it needed checking. The pole scale factor
    /// `k₀ = (1 + sin|LaD|)/2` is in `[0.5, 1]` for a *real* latitude of true
    /// scale, `|LaD| ≤ 90`, which is where the standing claim that no
    /// declarable `LaD` drives it to zero came from. §3.20 states `LaD` as
    /// four octets of sign-magnitude microdegrees, so it can declare ±270°,
    /// where `sin|LaD| = -1` and `k₀` is exactly zero — the same collapsed plane
    /// a zero radius gives, reached from the other factor. A non-finite `LaD`
    /// poisons the product as well.
    fn well_defined(&self) -> bool {
        self.two_r_k0.is_finite() && self.two_r_k0 > 0.0
    }
}

fn polar_stereo_constants(lad: f64, south_pole: bool, earth_radius_m: f64) -> PolarStereoConstants {
    // The pole scale factor depends on the magnitude of the latitude of true
    // scale; the hemisphere is handled separately by `sign`.
    let k0 = (1.0 + (lad.abs() * DEG2RAD).sin()) / 2.0;
    PolarStereoConstants {
        two_r_k0: 2.0 * earth_radius_m * k0,
        sign: if south_pole { -1.0 } else { 1.0 },
    }
}

fn polar_stereo_forward_with(k: &PolarStereoConstants, lov: f64, lat: f64, lon: f64) -> (f64, f64) {
    let lat_r = lat * DEG2RAD;
    let d_lon = (lon - lov) * DEG2RAD;
    // Snyder 21-33 (north) / 21-34 (south). For south-polar, `sign = -1`
    // flips the latitude argument so the same `tan(π/4 - φ_s/2)` form
    // works after substituting `φ_s = -lat`.
    let rho = k.two_r_k0 * (PI / 4.0 - k.sign * lat_r / 2.0).tan();
    let x = rho * d_lon.sin();
    let y = -k.sign * rho * d_lon.cos();
    (x, y)
}

fn polar_stereo_inverse_xy_with(k: &PolarStereoConstants, lov: f64, x: f64, y: f64) -> (f64, f64) {
    let rho = (x * x + y * y).sqrt();
    if rho == 0.0 {
        // At the pole every meridian converges; longitude is undefined.
        // Return lov as a convention so warp setup that hits this case
        // doesn't NaN-pollute downstream min/max.
        return (k.sign * 90.0, lov);
    }
    let c = 2.0 * (rho / k.two_r_k0).atan();
    let lat = k.sign * (PI / 2.0 - c) * RAD2DEG;
    // Snyder 20-16: λ = λ₀ + atan2(x, -y) for north-polar; flip the y-sign
    // for south-polar (same `sign` flip used in the forward direction).
    let lon = lov + x.atan2(-k.sign * y) * RAD2DEG;
    (lat, lon)
}

/// Precomputed inverse map for a polar stereographic grid. Owns the
/// pole-scale constant and the forward-projected grid origin — both
/// invariant across every output pixel of a warp.
#[derive(Debug)]
pub struct PolarStereoProjector {
    /// The grid this projector was built for.
    pub params: PolarStereoParams,
    constants: PolarStereoConstants,
    origin: (f64, f64),
}

impl PolarStereoProjector {
    /// Precompute the pole-scale constant and the projected origin for
    /// `params`. Build once outside a warp loop.
    pub fn new(params: PolarStereoParams) -> Self {
        let constants =
            polar_stereo_constants(params.lad, params.south_pole, params.earth_radius_m);
        let origin =
            polar_stereo_forward_with(&constants, params.lov, params.lat_first, params.lon_first);
        Self {
            params,
            constants,
            origin,
        }
    }

    /// Project `(lat, lon)` back to the source-grid fractional index, or
    /// `None` when it falls outside the `ni × nj` extent. The shared planar
    /// body — kept as an inherent method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
    }

    /// Project a geographic point to projection-plane metres, in a coordinate
    /// system centred on the projection pole with the y-axis along `lov`.
    ///
    /// Defined everywhere except the *opposite* pole, where `tan` diverges. In
    /// `f64` that divergence does not actually reach infinity — `(PI/4 + PI/4).tan()`
    /// is about 1.6e16, because `PI/2` is not exactly representable — so a caller
    /// at the antipodal pole gets a finite `(x, y)` around 1e23 rather than `±inf`.
    /// [`inverse`](Self::inverse) relies on that landing far outside any grid's
    /// extent rather than on a finiteness check.
    ///
    /// `rho` is strictly monotonic in latitude across the whole open range, so
    /// the projection is injective there: a point in the hemisphere *opposite*
    /// the projection pole has a large `rho`, but it is still that point's own
    /// `rho` and cannot alias onto another. Grids that reach across the equator
    /// — the CMC regional grid is one — depend on this.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        polar_stereo_forward_with(&self.constants, self.params.lov, lat, lon)
    }

    /// Invert projection-plane metres back to `(lat, lon)` in degrees.
    /// `(0, 0)` is the projection pole, where longitude is undefined; it
    /// answers with the pole latitude and `lov` by convention, so warp setup
    /// that hits it does not NaN-pollute a downstream min/max.
    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        polar_stereo_inverse_xy_with(&self.constants, self.params.lov, x, y)
    }

    /// The grid's first scanned point, in projection-plane metres.
    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }

    /// `true` when the projection pole (origin in projected metres) falls
    /// inside the grid extent. Warp setup uses this to detect the case
    /// where every meridian is represented in the grid and the
    /// equirectangular target should span the full 360° of longitude.
    pub fn pole_inside_grid(&self) -> bool {
        let (ox, oy) = self.origin;
        let max_x = ox + (self.params.ni as f64 - 1.0) * self.params.dx_metres;
        let max_y = oy + (self.params.nj as f64 - 1.0) * self.params.dy_metres;
        let (x_min, x_max) = if ox <= max_x {
            (ox, max_x)
        } else {
            (max_x, ox)
        };
        let (y_min, y_max) = if oy <= max_y {
            (oy, max_y)
        } else {
            (max_y, oy)
        };
        x_min <= 0.0 && 0.0 <= x_max && y_min <= 0.0 && 0.0 <= y_max
    }

    /// Whether the plane and the grid's position in it are usable. `false` for
    /// a declared Earth radius of zero or less (see
    /// `PolarStereoConstants::well_defined`); such a projector's
    /// [`inverse`](Self::inverse) returns `None` for every point, so callers can
    /// surface "not reprojectable" instead of rendering a flat field.
    ///
    /// The projected origin is checked the same way
    /// [`LambertProjector::is_well_defined`](super::LambertProjector::is_well_defined)
    /// checks its own: the plane can be fine and the grid still state a first
    /// point the forward map cannot follow, leaving the raster with no position
    /// in a plane that is otherwise sound.
    ///
    /// The plane is finally checked against the grid's own step, by
    /// [`plane_spans_a_grid_cell`](super::plane_spans_a_grid_cell): its radius
    /// can be positive, finite and still far too small to carry the raster,
    /// and the whole plane then collapses inside one cell (#610).
    ///
    /// The quantity measured is `2·R·k₀`, not the declared `R`. **Both factors
    /// shrink the plane and a message states both.** A radius of 1e-6 m is the
    /// case #610 was filed for, but §3.20 carries the latitude of true scale in
    /// sign-magnitude microdegrees, so `LaD = 268°` is as declarable as 60° and
    /// puts `k₀ = (1 + sin|LaD|)/2` at 3.0e-4 — a plane 3.9 km across on a
    /// perfectly ordinary Earth, against 60 km CMC cells. #603 closed only the
    /// exact `k₀ == 0` point at `LaD = ±270°`, and a floor under the declared
    /// radius would not have caught this at all.
    ///
    /// The band this leaves is worth stating, since it is the price of the
    /// loose floor rather than an oversight: on that grid `LaD = 262°` gives a
    /// plane 1.03 cells wide and is admitted, and only `LaD ≳ 263°` is refused.
    /// A tighter rule would have to compare the plane against the grid's whole
    /// extent, and it cannot: a real `LaD = 0°` grid has a plane radius of 106
    /// cells against a 135-cell raster, so the honest grids sit on both sides
    /// of that line. See [`plane_spans_a_grid_cell`](super::plane_spans_a_grid_cell).
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined()
            && self.origin.0.is_finite()
            && self.origin.1.is_finite()
            && super::plane_spans_a_grid_cell(
                self.constants.two_r_k0,
                self.params.dx_metres,
                self.params.dy_metres,
            )
    }
}

impl PlanarGridProjector for PolarStereoProjector {
    fn grid_origin(&self) -> (f64, f64) {
        self.origin
    }
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
    }
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        // One predicate, not two, for the reason its three siblings give: a
        // second copy of the rule here is how a condition gets added in one
        // place and missed in the other. The plane this rejects is the one a
        // declared radius of zero collapses, where the arithmetic stays finite
        // and every point lands on index (0, 0).
        self.is_well_defined()
    }
    fn grid_dims(&self) -> (u32, u32) {
        (self.params.ni, self.params.nj)
    }
    fn grid_spacing(&self) -> (f64, f64) {
        (self.params.dx_metres, self.params.dy_metres)
    }
    fn inverse_lonlat(&self, x: f64, y: f64) -> (f64, f64) {
        self.inverse_xy(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::DEFAULT_EARTH_RADIUS_M;
    use crate::projection::near;

    /// CMC regional model grid (135×95, 60 km at 60°N, lon_first ≈ −110°,
    /// lov = 247°). Matches the `cmc_wind_300_2010052400_p012.grib`
    /// fixture used by the GRIB1 integration tests.
    fn cmc_polar_params() -> PolarStereoParams {
        PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 135,
            nj: 95,
            lat_first: 11.43,
            lon_first: -110.27,
            lov: 247.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        }
    }

    #[test]
    fn polar_stereo_forward_inverse_round_trip_north() {
        let proj = PolarStereoProjector::new(cmc_polar_params());
        for (lat, lon) in [(45.0, -90.0), (60.0, 0.0), (80.0, 100.0)] {
            let (x, y) = proj.forward(lat, lon);
            let (lat_back, lon_back) = proj.inverse_xy(x, y);
            assert!(near(lat_back, lat, 1e-7), "lat {lat} → {lat_back}");
            // Normalise to [-180, 180] before comparing — atan2 returns
            // (-π, π] and the test inputs are in that range too.
            let lon_back = ((lon_back + 180.0).rem_euclid(360.0)) - 180.0;
            let lon_norm = ((lon + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(near(lon_back, lon_norm, 1e-7), "lon {lon} → {lon_back}");
        }
    }

    #[test]
    fn polar_stereo_forward_inverse_round_trip_south() {
        let proj = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            south_pole: true,
            lat_first: -11.43,
            ..cmc_polar_params()
        });
        for (lat, lon) in [(-45.0, -90.0), (-60.0, 0.0), (-80.0, 100.0)] {
            let (x, y) = proj.forward(lat, lon);
            let (lat_back, lon_back) = proj.inverse_xy(x, y);
            assert!(near(lat_back, lat, 1e-7), "lat {lat} → {lat_back}");
            let lon_back = ((lon_back + 180.0).rem_euclid(360.0)) - 180.0;
            let lon_norm = ((lon + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(near(lon_back, lon_norm, 1e-7), "lon {lon} → {lon_back}");
        }
    }

    #[test]
    fn polar_stereo_inverse_honours_north_to_south_scan() {
        // A north-polar grid scanning north→south (jScansPositively = 0): row 0
        // is the northernmost row, successive rows step south. The napi builder
        // encodes that as a *negative* dy; the projector's j must then advance
        // southward. (See `signed_polar_increments` in the napi crate.)
        let base = PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 10,
            nj: 10,
            lat_first: 80.0,
            lon_first: 0.0,
            lov: 0.0,
            lad: 60.0,
            dx_metres: 50_000.0,
            dy_metres: -50_000.0, // north→south scan
            south_pole: false,
        };
        let proj = PolarStereoProjector::new(base);
        // The first scanned point is the projection origin → index (0, 0).
        let origin = proj.inverse(80.0, 0.0).expect("origin resolves");
        assert!(
            origin.i.abs() < 1e-6 && origin.j.abs() < 1e-6,
            "origin {origin:?}"
        );
        // A point ~2° south of the first row lies several rows *into* the grid.
        let south = proj.inverse(78.0, 0.0).expect("southward point resolves");
        assert!(
            south.j > 0.0,
            "north→south scan must increase j going south, got j={}",
            south.j
        );

        // Regression guard: the pre-fix code fed the unsigned magnitude
        // (positive dy), which maps the same southward point to negative j and
        // drops it from the grid entirely.
        let unsigned = PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            dy_metres: 50_000.0,
            ..base
        };
        assert!(
            PolarStereoProjector::new(unsigned)
                .inverse(78.0, 0.0)
                .is_none(),
            "positive (unsigned) dy mis-maps the southward point to negative j"
        );
    }

    #[test]
    fn polar_stereo_north_pole_projects_to_origin() {
        let (x, y) = PolarStereoProjector::new(cmc_polar_params()).forward(90.0, 0.0);
        assert!(near(x, 0.0, 1e-6));
        assert!(near(y, 0.0, 1e-6));
    }

    /// GRIB2 §3.20 carries a variable latitude of true scale (`LaD`); the
    /// pole scale factor `k₀ = (1 + sin|LaD|)/2` must follow it. A grid with
    /// true scale at the pole (LaD = 90°, k₀ = 1) projects to a radius
    /// `1/k₀(60°) = 1.07180…×` larger than the same point under the GRIB1
    /// fixed-60° convention (Snyder PP-1395, eq. 21-15).
    #[test]
    fn polar_stereo_lad_drives_pole_scale_factor() {
        let at_60 = PolarStereoProjector::new(cmc_polar_params()); // lad = 60.0
        let at_90 = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            lad: 90.0,
            ..cmc_polar_params()
        });
        let (x60, y60) = at_60.forward(45.0, 247.0);
        let (x90, y90) = at_90.forward(45.0, 247.0);
        let rho60 = (x60 * x60 + y60 * y60).sqrt();
        let rho90 = (x90 * x90 + y90 * y90).sqrt();
        let k0_60 = (1.0 + (60.0_f64 * DEG2RAD).sin()) / 2.0;
        assert!(
            near(rho90 / rho60, 1.0 / k0_60, 1e-9),
            "LaD=90 vs 60 ratio {} ≠ {}",
            rho90 / rho60,
            1.0 / k0_60
        );
        // Sanity: the two are genuinely different (regression guard against a
        // hardcoded constant silently ignoring LaD).
        assert!((rho90 - rho60).abs() > 1.0, "LaD ignored — radii identical");
    }

    #[test]
    fn polar_stereo_south_pole_projects_to_origin() {
        let (x, y) = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            south_pole: true,
            ..cmc_polar_params()
        })
        .forward(-90.0, 0.0);
        assert!(near(x, 0.0, 1e-6));
        assert!(near(y, 0.0, 1e-6));
    }

    #[test]
    fn polar_stereo_inverse_maps_first_corner_to_zero() {
        let p = cmc_polar_params();
        let idx = PolarStereoProjector::new(p)
            .inverse(p.lat_first, p.lon_first)
            .expect("corner");
        assert!(near(idx.i, 0.0, 1e-6));
        assert!(near(idx.j, 0.0, 1e-6));
    }

    #[test]
    fn polar_stereo_inverse_rejects_wrong_hemisphere() {
        let p = cmc_polar_params();
        assert!(
            PolarStereoProjector::new(p).inverse(-45.0, 0.0).is_none(),
            "north grid + south lat"
        );
        let south = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            south_pole: true,
            lat_first: -11.43,
            ..p
        });
        assert!(south.inverse(45.0, 0.0).is_none(), "south grid + north lat");
    }

    #[test]
    fn polar_stereo_inverse_rejects_off_grid_points() {
        // A point in Antarctica is on the wrong hemisphere for a north-polar
        // grid; a tropical point near the equator is on the right hemisphere
        // but well outside the 135×95 box around the pole.
        let proj = PolarStereoProjector::new(cmc_polar_params());
        assert!(proj.inverse(5.0, 0.0).is_none());
    }

    #[test]
    fn polar_stereo_inverse_rejects_nonfinite_and_degenerate_dims() {
        let p = cmc_polar_params();
        let proj = PolarStereoProjector::new(p);
        assert!(proj.inverse(f64::NAN, 0.0).is_none());
        assert!(proj.inverse(60.0, f64::INFINITY).is_none());
        let degenerate = PolarStereoProjector::new(PolarStereoParams { ni: 1, ..p });
        assert!(degenerate.inverse(60.0, 0.0).is_none());
        let zero_dx = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            dx_metres: 0.0,
            ..p
        });
        assert!(zero_dx.inverse(60.0, 0.0).is_none());
    }

    #[test]
    fn polar_stereo_pole_inside_grid_detection() {
        // CMC is a regional tile NE of the pole, not a hemispheric grid —
        // its projected box doesn't actually contain (0,0).
        let cmc = PolarStereoProjector::new(cmc_polar_params());
        assert!(
            !cmc.pole_inside_grid(),
            "CMC regional tile excludes the pole"
        );

        // A synthetic hemispheric grid whose NW corner sits at d_lon = -135°
        // from `lov`, at a southern-enough latitude that the projected origin
        // lands at roughly (-3e6, +3e6) metres. Scanning east + south at 2 Mm
        // step over 4×4 cells crosses the pole at (0, 0).
        let hemispheric = PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 4,
            nj: 4,
            lat_first: 50.8,
            lon_first: -135.0,
            lov: 0.0,
            lad: 60.0,
            dx_metres: 2_000_000.0,
            dy_metres: -2_000_000.0,
            south_pole: false,
        };
        let projector = PolarStereoProjector::new(hemispheric);
        assert!(
            projector.pole_inside_grid(),
            "hemispheric grid origin {:?} should bracket the pole",
            projector.origin()
        );
    }

    /// A declared Earth radius of zero collapses the plane onto its own origin.
    /// Nothing goes non-finite there, so the pre-fix code answered a real-looking
    /// index for every point on Earth — including points nowhere near the grid.
    #[test]
    fn polar_stereo_rejects_a_declared_radius_that_leaves_no_plane() {
        let p = cmc_polar_params();
        for radius in [0.0, -DEFAULT_EARTH_RADIUS_M, f64::NAN, f64::INFINITY] {
            let proj = PolarStereoProjector::new(PolarStereoParams {
                earth_radius_m: radius,
                ..p
            });
            assert!(
                !proj.is_well_defined(),
                "radius {radius} should leave no usable plane"
            );
            // The point that made this a defect rather than a curiosity: the
            // South Atlantic is not on a Canadian grid, and the pre-fix code
            // placed it at cell (0, 0) along with everywhere else.
            assert!(
                proj.inverse(-20.0, 10.0).is_none(),
                "radius {radius}: a point off the grid resolved onto it"
            );
            assert!(
                proj.inverse(60.0, -90.0).is_none(),
                "radius {radius}: a point on the grid resolved on a collapsed plane"
            );
        }
        assert!(
            PolarStereoProjector::new(p).is_well_defined(),
            "the real CMC grid is unaffected"
        );
    }

    /// #610: the other side of the guard above. A declared radius of 1e-6 m —
    /// GRIB2 shape-of-earth 1 with `scale = 6, value = 1` — is positive,
    /// finite, and leaves `2·R·k₀` positive too, so every check the zero case
    /// added still passes while the whole plane fits inside one 60 km cell.
    #[test]
    fn polar_stereo_rejects_an_earth_smaller_than_one_grid_cell() {
        let p = cmc_polar_params();
        // `2·R·k₀` is what must fall under the 60 km cell, and `k₀` is 0.933 at
        // the CMC grid's ±60° true scale, so the radius itself has to be under
        // ~32 km. That the boundary is not simply `R < dx` is the point: the
        // plane, not the planet, is what the raster sits on.
        for radius in [1e-6, 1.0, 30_000.0] {
            let proj = PolarStereoProjector::new(PolarStereoParams {
                earth_radius_m: radius,
                ..p
            });
            assert!(
                !proj.is_well_defined(),
                "radius {radius} m cannot carry a {} m cell",
                p.dx_metres
            );
            assert!(
                proj.inverse(60.0, -90.0).is_none(),
                "radius {radius}: a point resolved on a collapsed plane"
            );
            assert!(
                proj.inverse(-20.0, 10.0).is_none(),
                "radius {radius}: a point off the grid resolved onto it"
            );
        }
        // A smaller planet is not the problem, only one smaller than the grid.
        for radius in [469_730.0, 1_737_400.0, 3_396_190.0] {
            assert!(
                PolarStereoProjector::new(PolarStereoParams {
                    earth_radius_m: radius,
                    ..p
                })
                .is_well_defined(),
                "a {radius} m body still carries the grid"
            );
        }
    }

    /// The other half of the plane, and the quieter one. `2·R·k₀` is what the
    /// forward map multiplies by, and a message states both factors: §3.20
    /// carries `LaD` in sign-magnitude microdegrees, so 260° is as declarable
    /// as 60° and puts `k₀ = (1 + sin|LaD|)/2` at 0.0076. The Earth is then
    /// perfectly ordinary and the whole plane is 97 km across — 1.6 CMC cells.
    ///
    /// #603 closed only the exact `k₀ == 0` point at ±270°, and a floor under
    /// the *declared radius* would not have caught this at all, which is why
    /// `is_well_defined` measures the constants rather than `params`.
    #[test]
    fn polar_stereo_rejects_a_true_scale_latitude_that_shrinks_the_plane() {
        let p = cmc_polar_params();
        for lad in [265.0, 268.0, -265.0, 269.9] {
            let proj = PolarStereoProjector::new(PolarStereoParams { lad, ..p });
            assert!(
                !proj.is_well_defined(),
                "LaD {lad} leaves a plane smaller than the grid"
            );
            // The South Atlantic is not on a Canadian grid, and a collapsed
            // plane used to place it there along with everywhere else.
            assert!(
                proj.inverse(-20.0, 10.0).is_none(),
                "LaD {lad}: a point off the grid resolved onto it"
            );
            assert!(
                proj.inverse(60.0, -90.0).is_none(),
                "LaD {lad}: a point resolved on a collapsed plane"
            );
        }
        // The parallels a message really states are untouched, by two orders
        // of magnitude: these give planes 106 to 212 cells wide.
        for lad in [90.0, 60.0, 30.0, 0.0, -60.0] {
            assert!(
                PolarStereoProjector::new(PolarStereoParams { lad, ..p }).is_well_defined(),
                "LaD {lad} is a true scale latitude grids are published on"
            );
        }
    }

    /// The forward map is what makes a zero radius quiet: it stays finite, so
    /// only the constants check can catch it. Recorded as the arithmetic the
    /// gate above stands on, not as behaviour a caller should rely on.
    #[test]
    fn polar_stereo_zero_radius_forward_is_finite_and_collapsed() {
        let proj = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: 0.0,
            ..cmc_polar_params()
        });
        for (lat, lon) in [(60.0, -90.0), (-20.0, 10.0), (11.43, -110.27)] {
            let (x, y) = proj.forward(lat, lon);
            assert!(
                x.is_finite() && y.is_finite(),
                "({lat}, {lon}) went non-finite"
            );
            assert_eq!((x, y), (0.0, 0.0), "({lat}, {lon})");
        }
        let (ox, oy) = proj.origin();
        assert_eq!(
            (ox, oy),
            (0.0, 0.0),
            "the origin lands on the same point everything else does, \
             which is why the spacing guard cannot see this"
        );
    }

    /// The other factor in `two_r_k0`, and the one the standing claim was wrong
    /// about. `k₀ = (1 + sin|LaD|)/2` is in `[0.5, 1]` only for `|LaD| ≤ 90`;
    /// §3.20 states `LaD` in sign-magnitude microdegrees, so ±270° is
    /// declarable and puts `sin|LaD|` at -1, zeroing the factor with the radius
    /// untouched.
    #[test]
    fn polar_stereo_rejects_a_latitude_of_true_scale_past_the_pole() {
        let p = cmc_polar_params();
        for lad in [270.0, -270.0, f64::NAN] {
            let proj = PolarStereoProjector::new(PolarStereoParams { lad, ..p });
            assert!(!proj.is_well_defined(), "LaD {lad} should leave no plane");
            assert!(proj.inverse(60.0, -90.0).is_none(), "LaD {lad}");
        }
        // A real latitude of true scale — and the two beyond ±90° that do *not*
        // zero the factor — are untouched, so the gate is on the factor rather
        // than on a range check applied to `LaD`.
        for lad in [0.0, 60.0, 90.0, -90.0, 180.0, 210.0] {
            assert!(
                PolarStereoProjector::new(PolarStereoParams { lad, ..p }).is_well_defined(),
                "LaD {lad} leaves a usable plane"
            );
        }
    }

    /// A grid whose first point the forward map cannot follow has no position
    /// in a plane that is otherwise fine — the origin half of the predicate.
    ///
    /// This one leaked further than the collapsed plane did. `inverse` rejects a
    /// non-finite *query* and a non-finite forward result, but it divides by a
    /// non-finite origin without looking: `(x - NaN) / dx` is `NaN`, and the
    /// extent comparisons `i < 0.0 || i > i_max` are both false for `NaN`, so
    /// the pre-fix code answered `Some(GridIndex { i: NaN, j: NaN })`. Polar
    /// stereographic was the only planar family this could happen to, because
    /// it was the only one whose `accepts` defaulted to `true`.
    #[test]
    fn polar_stereo_rejects_a_first_point_the_forward_map_cannot_follow() {
        let proj = PolarStereoProjector::new(PolarStereoParams {
            lat_first: f64::NAN,
            ..cmc_polar_params()
        });
        assert!(!proj.is_well_defined());
        assert!(
            proj.inverse(60.0, -90.0).is_none(),
            "a NaN origin used to answer a NaN index rather than nothing"
        );
    }

    #[test]
    fn polar_stereo_inverse_xy_origin_returns_pole_with_lov() {
        let p = cmc_polar_params();
        let (lat, lon) = PolarStereoProjector::new(p).inverse_xy(0.0, 0.0);
        assert!(near(lat, 90.0, 1e-9));
        assert!(near(lon, p.lov, 1e-9));
    }
}
