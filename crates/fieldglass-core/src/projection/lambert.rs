//! Lambert Conformal Conic grids — GRIB1 `grid_type` 3, GRIB2 template 3.30.
//!
//! Snyder, "Map Projections: A Working Manual" (USGS PP-1395), pp. 104-110.
//! Two-standard-parallel form, with a tangent-cone branch when
//! `latin1 == latin2`. The projection plane is metres, so the family
//! implements [`super::PlanarGridProjector`] and gets its grid-index,
//! corner and bounding-box machinery from there.

use std::f64::consts::PI;

use super::{DEG2RAD, GridIndex, PlanarGridProjector, RAD2DEG};

/// A Lambert Conformal Conic grid — GRIB1 `grid_type` 3, GRIB2 template
/// 3.30. Two standard parallels, with a tangent-cone branch when they are
/// equal; the plane is metres.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LambertParams {
    /// Radius of the spherical Earth the grid is projected on, in metres. The
    /// message declares it (GRIB1's earth-shape flag, GRIB2's
    /// `shapeOfTheEarth`); [`super::DEFAULT_EARTH_RADIUS_M`] is the fallback.
    pub earth_radius_m: f64,
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// Latitude of the first scanned point, degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point, degrees.
    pub lon_first: f64,
    /// Latitude of true scale (`LaD`), in degrees.
    pub lad: f64,
    /// Orientation longitude (`LoV`), in degrees.
    pub lov: f64,
    /// Grid spacing in metres along x and y at the latitude of true scale.
    pub dx_metres: f64,
    /// Grid spacing along y in metres — see `dx_metres`.
    pub dy_metres: f64,
    /// First standard parallel (`Latin1`), degrees.
    pub latin1: f64,
    /// Second standard parallel (`Latin2`), degrees. Equal to `latin1` for a
    /// tangent cone.
    pub latin2: f64,
}

/// `pub` so projector helpers can hand them around, but the fields are
/// private — callers shouldn't construct these directly.
#[derive(Debug, Clone, Copy)]
pub struct LambertConstants {
    n: f64,
    f_const: f64,
    rho0: f64,
    earth_r: f64,
}

impl LambertConstants {
    /// Whether these constants describe a usable cone. Degenerate standard
    /// parallels make the cone constant `n` zero or non-finite — both tangent
    /// parallels on the equator (`latin1 == latin2 == 0`, so `n = sin 0 = 0`)
    /// or a parallel at a pole (`cos → 0`, so `F = cos·tanⁿ / n` blows up). The
    /// `/ n` in `f_const` would then divide by zero, yielding `inf`/`NaN` that
    /// silently render blank; callers should reject the grid instead.
    ///
    /// A declared Earth radius of zero (or less) is the quieter case, and the
    /// reason the radius is checked here rather than left to the `is_finite`
    /// tests above: it scales `rho0` to zero without making anything infinite,
    /// so every point on the cone inverts to the pole. That reads as a real
    /// coordinate to anything downstream — a rendered map, a CSV row — instead
    /// of as a broken grid.
    fn well_defined(&self) -> bool {
        self.earth_r.is_finite()
            && self.earth_r > 0.0
            && self.n.is_finite()
            && self.n != 0.0
            && self.f_const.is_finite()
            && self.rho0.is_finite()
    }
}

fn lambert_constants(p: &LambertParams) -> LambertConstants {
    let lat1 = p.latin1 * DEG2RAD;
    let lat2 = p.latin2 * DEG2RAD;
    let lad = p.lad * DEG2RAD;
    let tan1 = (PI / 4.0 + lat1 / 2.0).tan();
    let tan2 = (PI / 4.0 + lat2 / 2.0).tan();
    let n = if (p.latin1 - p.latin2).abs() < 1e-9 {
        lat1.sin()
    } else {
        (lat1.cos() / lat2.cos()).ln() / (tan2 / tan1).ln()
    };
    // `f_const`/`rho0` are non-finite for degenerate parallels (n == 0, or a
    // pole-tangent cone). Rather than clamp here — which would invent a cone
    // the grid never described — we let the values stay non-finite and gate on
    // `LambertConstants::well_defined` at the projection boundary.
    let f_const = lat1.cos() * tan1.powf(n) / n;
    let rho0 = p.earth_radius_m * f_const / (PI / 4.0 + lad / 2.0).tan().powf(n);
    LambertConstants {
        n,
        f_const,
        rho0,
        earth_r: p.earth_radius_m,
    }
}

fn lambert_forward_with(k: &LambertConstants, lov: f64, lat: f64, lon: f64) -> (f64, f64) {
    let lat_r = lat * DEG2RAD;
    // Wrap (lon − lov) into [-180, 180] *before* scaling by the cone constant.
    // Unlike the polar projector — whose `d_lon` only ever reaches `sin`/`cos`
    // and is therefore 360°-periodic — Lambert multiplies the difference by the
    // cone constant `n` before the trig, so an unwrapped 360° offset (e.g. a
    // query longitude in [-180, 180] against a `LoV` carried in [0, 360), as
    // NCEP/Eta files store it) shifts the cone angle by `n·360°` and throws the
    // point far outside the grid — which is why `equirectangular` rendered blank
    // for the Eta Lambert grid. The inverse-index path (`LambertProjector::
    // inverse`) routes through this forward map, so fixing it here is enough.
    let d_lon = ((lon - lov + 180.0).rem_euclid(360.0) - 180.0) * DEG2RAD;
    let rho = k.earth_r * k.f_const / (PI / 4.0 + lat_r / 2.0).tan().powf(k.n);
    let x = rho * (k.n * d_lon).sin();
    let y = k.rho0 - rho * (k.n * d_lon).cos();
    (x, y)
}

fn lambert_inverse_xy_with(k: &LambertConstants, lov: f64, x: f64, y: f64) -> (f64, f64) {
    let dy = k.rho0 - y;
    let rho = k.n.signum() * (x * x + dy * dy).sqrt();
    let theta = x.atan2(dy);
    let lon = lov + (theta / k.n) * RAD2DEG;
    let lat = (2.0 * ((k.earth_r * k.f_const / rho).powf(1.0 / k.n)).atan() - PI / 2.0) * RAD2DEG;
    (lat, lon)
}

/// Precomputed inverse map for a Lambert grid. Owns the cone constants
/// (`n`, `F`, `ρ₀`) and the forward-projected grid origin — both
/// invariant across every output pixel of a warp. Build once outside
/// the per-pixel loop; call [`Self::inverse`] inside it.
#[derive(Debug)]
pub struct LambertProjector {
    /// The grid this projector was built for.
    pub params: LambertParams,
    constants: LambertConstants,
    origin: (f64, f64),
}

impl LambertProjector {
    /// Precompute the cone constants and the projected origin for `params`.
    /// Build once outside a warp loop.
    pub fn new(params: LambertParams) -> Self {
        let constants = lambert_constants(&params);
        let origin =
            lambert_forward_with(&constants, params.lov, params.lat_first, params.lon_first);
        Self {
            params,
            constants,
            origin,
        }
    }

    /// Project `(lat, lon)` back to the source-grid fractional index.
    /// Returns `None` when the projected coordinates fall outside the
    /// `ni × nj` grid extent. The shared planar body — kept as an inherent
    /// method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
    }

    /// Forward-project a `(lat, lon)` through the cached constants. Used
    /// by warp setup to derive equirectangular target bounds from the
    /// four source corners.
    ///
    /// Lambert Conformal is undefined at the projection poles
    /// (`tan(π/4 ± π/4) = ±∞`). Real-world Lambert grids cover continental
    /// tiles and never reach the pole on their own pole side, so this is
    /// academic — but callers passing a pole latitude here will get `±inf`
    /// / `NaN`.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        lambert_forward_with(&self.constants, self.params.lov, lat, lon)
    }

    /// Inverse-project a projected-metres `(x, y)` back to `(lat, lon)`.
    /// Same pole caveat as [`forward`](Self::forward).
    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        lambert_inverse_xy_with(&self.constants, self.params.lov, x, y)
    }

    /// Read-only access to the precomputed grid origin in projected
    /// metres. Useful for warp setup that wants to enumerate the
    /// non-origin corners.
    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }

    /// Whether the cone constants are usable. `false` for degenerate standard
    /// parallels (see `LambertConstants::well_defined`); such a projector's
    /// [`inverse`](Self::inverse) always returns `None`, so callers can surface
    /// "not reprojectable" instead of rendering blank.
    ///
    /// The projected origin is checked too, the same way
    /// [`LambertAzimuthalProjector::is_well_defined`](super::LambertAzimuthalProjector::is_well_defined)
    /// checks its own. A grid may state a usable cone and then put its first
    /// point at the pole the cone opens away from, where `tan(π/4 + φ/2)` is
    /// zero and the forward map divides by it: the origin becomes infinite,
    /// `inverse` declines every point, and the inverse map still hands back a
    /// finite `(-90, lon)` for each of them — a coordinate, not a refusal.
    ///
    /// The declared radius is finally checked against the grid's own step, by
    /// [`plane_spans_a_grid_cell`](super::plane_spans_a_grid_cell): a radius can
    /// be positive, finite and still far too small to carry the raster, and the
    /// whole cone then collapses inside one cell (#610).
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined()
            && self.origin.0.is_finite()
            && self.origin.1.is_finite()
            && super::plane_spans_a_grid_cell(
                self.params.earth_radius_m,
                self.params.dx_metres,
                self.params.dy_metres,
            )
    }
}

impl PlanarGridProjector for LambertProjector {
    fn grid_origin(&self) -> (f64, f64) {
        self.origin
    }
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
    }
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        // One predicate, not two. Degenerate standard parallels (see
        // `LambertConstants::well_defined`) leave no usable cone, and a second
        // copy of the rule here is how a later condition — the origin check —
        // would be added in one place and missed in the other.
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

    fn lambert_params() -> LambertParams {
        LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 93,
            nj: 65,
            lat_first: 12.19,
            lon_first: -133.459,
            lad: 25.0,
            lov: -95.0,
            dx_metres: 81_271.0,
            dy_metres: 81_271.0,
            latin1: 25.0,
            latin2: 25.0,
        }
    }

    #[test]
    fn lambert_forward_inverse_round_trip() {
        let proj = LambertProjector::new(lambert_params());
        let (x, y) = proj.forward(40.0, -100.0);
        let (lat, lon) = proj.inverse_xy(x, y);
        assert!(near(lat, 40.0, 1e-6));
        assert!(near(lon, -100.0, 1e-6));
    }

    #[test]
    fn lambert_handles_0_360_lov_convention() {
        // Eta-style grid: LoV + Lo1 carried in [0, 360) (265°E / 226.541°E), as
        // NCEP files store them, rather than the ±180 form `lambert_params`
        // uses. Regression for the cone-angle wrap bug that rendered such grids
        // blank under equirectangular reprojection.
        let p = LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            lov: 265.0,
            lon_first: 226.541,
            ..lambert_params()
        };
        // The forward map must be invariant to a 360° shift in the query
        // longitude (the fix wraps `lon − lov` before scaling by the cone
        // constant; without it the two differ by n·360°).
        let proj = LambertProjector::new(p);
        let f_pm180 = proj.forward(40.0, -95.0);
        let f_0_360 = proj.forward(40.0, 265.0);
        assert!(
            near(f_pm180.0, f_0_360.0, 1e-6),
            "x invariant to +360 shift"
        );
        assert!(
            near(f_pm180.1, f_0_360.1, 1e-6),
            "y invariant to +360 shift"
        );
        // And a ±180 query longitude (what the equirectangular target feeds in)
        // resolves to an in-grid index instead of falling off the grid.
        let idx = proj
            .inverse(40.0, -95.0)
            .expect("on-grid point on the LoV meridian");
        assert!(idx.i >= 0.0 && idx.i <= (p.ni as f64 - 1.0));
        assert!(idx.j >= 0.0 && idx.j <= (p.nj as f64 - 1.0));
    }

    #[test]
    fn lambert_inverse_maps_first_corner_to_zero() {
        let p = lambert_params();
        let idx = LambertProjector::new(p)
            .inverse(p.lat_first, p.lon_first)
            .expect("corner");
        assert!(near(idx.i, 0.0, 1e-6));
        assert!(near(idx.j, 0.0, 1e-6));
    }

    #[test]
    fn lambert_inverse_rejects_off_grid_points() {
        let proj = LambertProjector::new(lambert_params());
        assert!(proj.inverse(70.0, -100.0).is_none(), "north");
        assert!(proj.inverse(0.0, 0.0).is_none(), "southeast");
    }

    #[test]
    fn lambert_inverse_rejects_nonfinite_and_degenerate_dims() {
        let p = lambert_params();
        let proj = LambertProjector::new(p);
        assert!(proj.inverse(f64::NAN, -100.0).is_none(), "NaN lat");
        assert!(proj.inverse(40.0, f64::INFINITY).is_none(), "inf lon");
        let degenerate = LambertProjector::new(LambertParams { ni: 1, ..p });
        assert!(degenerate.inverse(40.0, -100.0).is_none(), "ni < 2");
        let zero_dx = LambertProjector::new(LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            dx_metres: 0.0,
            ..p
        });
        assert!(
            zero_dx.inverse(40.0, -100.0).is_none(),
            "dx_metres = 0 must not divide"
        );
    }

    #[test]
    fn lambert_rejects_degenerate_standard_parallels() {
        // Both standard parallels on the equator: cone constant n = sin 0 = 0,
        // so `F = cos·tanⁿ / n` divides by zero. The constants must report
        // themselves ill-defined and the inverse must return None for every
        // query, rather than emitting an index off a non-finite projection.
        let equator = LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            latin1: 0.0,
            latin2: 0.0,
            ..lambert_params()
        };
        let proj = LambertProjector::new(equator);
        assert!(
            !proj.is_well_defined(),
            "equator-tangent cone is degenerate"
        );
        assert!(proj.inverse(40.0, -100.0).is_none());
        assert!(proj.inverse(equator.lat_first, equator.lon_first).is_none());
        // A healthy cone still reports itself usable.
        assert!(LambertProjector::new(lambert_params()).is_well_defined());
    }

    /// #610: a radius that is positive and finite and still leaves no plane.
    /// GRIB2 shape-of-earth 1 states a scale factor and a scaled value, so
    /// `scale = 6, value = 1` is a legal 1e-6 m sphere. Every `> 0.0` check
    /// passes, the cone constants are fine, and the whole Earth projects to
    /// within a micrometre of the origin — so the Eta grid's own 81 km step
    /// puts every point on the planet at index 0.
    #[test]
    fn lambert_rejects_an_earth_smaller_than_one_grid_cell() {
        let p = lambert_params();
        for radius in [1e-6, 1.0, p.dx_metres, p.dx_metres * 0.999] {
            let proj = LambertProjector::new(LambertParams {
                earth_radius_m: radius,
                ..p
            });
            assert!(
                !proj.is_well_defined(),
                "radius {radius} m cannot carry an {} m cell",
                p.dx_metres
            );
            // The point the pre-fix code placed at cell (0, 0) along with
            // everywhere else on Earth.
            assert!(
                proj.inverse(40.0, -95.0).is_none(),
                "radius {radius}: a point resolved on a collapsed plane"
            );
        }
    }

    /// The floor is the grid's own step, not a guess about planets: any body
    /// big enough to carry the raster keeps working, which is why an absolute
    /// metre threshold was not the fix.
    #[test]
    fn lambert_accepts_any_body_that_can_carry_the_grid() {
        let p = lambert_params();
        for (body, radius) in [
            ("Earth", DEFAULT_EARTH_RADIUS_M),
            ("Mars", 3_396_190.0),
            ("Moon", 1_737_400.0),
            ("Ceres", 469_730.0),
            // Smaller than the grid it carries is the failure; a body merely
            // smaller than the grid *spacing is not* — 82 km against an 81 km
            // cell is absurd but arithmetically sound, and refusing it would
            // mean picking a planet size.
            ("a body one cell across", p.dx_metres * 1.01),
        ] {
            assert!(
                LambertProjector::new(LambertParams {
                    earth_radius_m: radius,
                    ..p
                })
                .is_well_defined(),
                "{body} ({radius} m) should still carry the grid"
            );
        }
    }

    #[test]
    fn lambert_inverse_honours_north_to_south_scan() {
        // A Lambert grid scanning north→south (jScansPositively = 0): row 0 is
        // the northernmost row. The napi builder encodes that as a negative dy,
        // and the projector's j must advance southward — identical mechanism to
        // the polar-stereo case, since both map `j = (y - origin_y) / dy` in the
        // LoV plane.
        let base = LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 50,
            nj: 50,
            lat_first: 50.0,
            lon_first: -100.0,
            lad: 40.0,
            lov: -100.0,
            dx_metres: 20_000.0,
            dy_metres: -20_000.0, // north→south scan
            latin1: 40.0,
            latin2: 40.0,
        };
        let proj = LambertProjector::new(base);
        // First scanned point (on the central meridian) → index (0, 0).
        let origin = proj.inverse(50.0, -100.0).expect("origin resolves");
        assert!(
            origin.i.abs() < 1e-6 && origin.j.abs() < 1e-6,
            "origin {origin:?}"
        );
        // A point 5° south of the first row lies several rows into the grid.
        let south = proj
            .inverse(45.0, -100.0)
            .expect("southward point resolves");
        assert!(
            south.j > 0.0,
            "north→south scan must increase j going south, got j={}",
            south.j
        );

        // Regression guard: the unsigned magnitude (positive dy) drops the
        // southward point to negative j and rejects it.
        let unsigned = LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            dy_metres: 20_000.0,
            ..base
        };
        assert!(
            LambertProjector::new(unsigned)
                .inverse(45.0, -100.0)
                .is_none(),
            "positive (unsigned) dy mis-maps the southward point to negative j"
        );
    }

    #[test]
    fn lambert_tangent_cone_at_origin() {
        let p = LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            latin1: 40.0,
            latin2: 40.0,
            lad: 40.0,
            ..lambert_params()
        };
        let (x, y) = LambertProjector::new(p).forward(40.0, -95.0);
        // At the projection origin (lad, lov), x and y should be ~0 in
        // the bare projection (no false-easting / false-northing).
        assert!(near(x, 0.0, 1.0));
        assert!(near(y, 0.0, 1.0));
    }
}
