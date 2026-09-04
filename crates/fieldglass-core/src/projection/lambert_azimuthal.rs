//! Lambert azimuthal equal-area grids — GRIB2 template 3.140.
//!
//! Snyder, PP-1395 §24. Unlike the conic and stereographic families this one is
//! computed on the spheroid: ETRS89-LAEA (EPSG:3035) is GRS80, and a mean-radius
//! approximation puts the far corner of an EFAS grid kilometres out. eccodes'
//! own iterator branches the same way on `grib_is_earth_oblate`.

use super::{DEG2RAD, GridIndex, PlanarGridProjector, RAD2DEG, SnapEps};

/// A Lambert azimuthal equal-area grid: the plane is tangent at one point and
/// area is preserved exactly, which is why Europe's statistical grids and the
/// CEMS/EFAS flood archive are published on it (ETRS89-LAEA, EPSG:3035), along
/// with EUMETSAT OSI SAF sea-ice products.
///
/// # Why this one is on the spheroid too
///
/// Same reason as [`super::TransverseMercatorParams`], further along: ETRS89-LAEA is
/// GRS80, and over the EFAS domain the spheroid's mean radius puts the far
/// corner **13.5 km** from where it belongs — several grid cells at 5 km.
///
/// It is also what eccodes does. Its `lambert_azimuthal_equal_area` iterator
/// branches on `grib_is_earth_oblate` and runs the authalic-latitude algorithm
/// for an oblate shape, so a mean-radius implementation could not agree with
/// the oracle the acceptance criterion names, never mind with the ground.
///
/// The series degenerates on its own when `a == b`: the authalic corrections
/// are a power series in the eccentricity, `qsfn` collapses to `2 sin φ`, and
/// what is left is the spherical formula eccodes' own `init_sphere` uses.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LambertAzimuthalParams {
    /// Semi-major and semi-minor axes in metres, as the message declares them.
    pub semi_major_m: f64,
    /// Semi-minor axis in metres, as the message declares it.
    pub semi_minor_m: f64,
    /// Points along a row (`Ni`).
    pub ni: u32,
    /// Rows (`Nj`).
    pub nj: u32,
    /// First grid point, in degrees. §3.140 states it geographically, so the
    /// projector forward-projects it to find the grid origin in the plane.
    pub lat_first: f64,
    /// Longitude of the first scanned point, degrees.
    pub lon_first: f64,
    /// The tangent point: `standardParallel` is its latitude and
    /// `centralLongitude` its longitude.
    pub standard_parallel: f64,
    /// The tangent point's `centralLongitude`, degrees.
    pub central_longitude: f64,
    /// Grid spacing in metres, carrying the scanning-mode sign.
    pub dx_metres: f64,
    /// Grid spacing along y in metres — see `dx_metres`.
    pub dy_metres: f64,
}

/// Authalic-projection constants for one spheroid and tangent point. All of it
/// depends only on `(a, b, standard_parallel)`, so a warp loop computes it once.
///
/// `pub` so projector helpers can hand them around; the fields are private.
#[derive(Debug, Clone, Copy)]
pub struct LambertAzimuthalConstants {
    /// First eccentricity, and its square. Zero for a sphere, which collapses
    /// every correction below.
    e: f64,
    es: f64,
    /// `q` at the pole — the authalic normalising constant.
    qp: f64,
    /// `√(qp / 2)`, the authalic sphere's radius in units of `a`.
    rq: f64,
    /// Authalic sine and cosine of the tangent latitude.
    sinb1: f64,
    cosb1: f64,
    /// The oblateness stretch that keeps the projection equal-area, and the two
    /// scale factors derived from it.
    dd: f64,
    xmf: f64,
    ymf: f64,
    /// Authalic → geodetic latitude series.
    apa: [f64; 3],
    semi_major_m: f64,
}

impl LambertAzimuthalConstants {
    /// Whether these constants describe a usable projection. A degenerate
    /// spheroid otherwise yields finite, meaningless coordinates — the same
    /// trap [`super::TransverseMercatorConstants::well_defined`] guards.
    fn well_defined(&self) -> bool {
        self.qp.is_finite()
            && self.qp > 0.0
            && self.rq.is_finite()
            && self.rq > 0.0
            && self.dd.is_finite()
            && self.dd != 0.0
            && self.xmf.is_finite()
            && self.ymf.is_finite()
            && self.semi_major_m.is_finite()
            && self.semi_major_m > 0.0
    }
}

/// `q`, the authalic area function. eccodes' `pj_qsfn`, which is PROJ's.
///
/// The `e >= 1e-7` guard is theirs too, and it is what makes the spherical case
/// fall out rather than dividing by an eccentricity of zero.
fn authalic_q(sin_lat: f64, e: f64, one_es: f64) -> f64 {
    const EPSILON: f64 = 1.0e-7;
    if e < EPSILON {
        return sin_lat + sin_lat;
    }
    let con = e * sin_lat;
    let div1 = 1.0 - con * con;
    let div2 = 1.0 + con;
    if div1 == 0.0 || div2 == 0.0 {
        return f64::INFINITY;
    }
    one_es * (sin_lat / div1 - (0.5 / e) * ((1.0 - con) / div2).ln())
}

/// Authalic → geodetic latitude, via PROJ's `pj_authset` / `pj_authlat` series.
fn authalic_series(es: f64) -> [f64; 3] {
    const P00: f64 = 0.333_333_333_333_333_3;
    const P01: f64 = 0.172_222_222_222_222_22;
    const P02: f64 = 0.102_579_365_079_365_08;
    const P10: f64 = 0.063_888_888_888_888_88;
    const P11: f64 = 0.066_402_116_402_116_4;
    const P20: f64 = 0.016_776_895_943_562_61;
    let es2 = es * es;
    let es3 = es2 * es;
    [
        es * P00 + es2 * P01 + es3 * P02,
        es2 * P10 + es3 * P11,
        es3 * P20,
    ]
}

fn authalic_to_geodetic(beta: f64, apa: &[f64; 3]) -> f64 {
    let t = beta + beta;
    beta + apa[0] * t.sin() + apa[1] * (t + t).sin() + apa[2] * (t + t + t).sin()
}

fn lambert_azimuthal_constants(p: &LambertAzimuthalParams) -> LambertAzimuthalConstants {
    let a = p.semi_major_m;
    let b = p.semi_minor_m;
    // Same guard, and the same reason, as `transverse_mercator_constants`: an
    // impossible spheroid must fail `well_defined` rather than project.
    if !(a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && b <= a) {
        return LambertAzimuthalConstants {
            e: f64::NAN,
            es: f64::NAN,
            qp: f64::NAN,
            rq: f64::NAN,
            sinb1: f64::NAN,
            cosb1: f64::NAN,
            dd: f64::NAN,
            xmf: f64::NAN,
            ymf: f64::NAN,
            apa: [f64::NAN; 3],
            semi_major_m: f64::NAN,
        };
    }
    let f = (a - b) / a;
    let es = 2.0 * f - f * f;
    let one_es = 1.0 - es;
    let e = es.sqrt();

    let qp = authalic_q(1.0, e, one_es);
    let rq = (0.5 * qp).sqrt();
    let lat1 = p.standard_parallel * DEG2RAD;
    let sin_lat1 = lat1.sin();
    // The pole's authalic latitude *is* the pole, so pin it rather than
    // deriving it. `authalic_q(-1)` takes the logarithm of the reciprocal of
    // the argument `qp` was built from, and floating point does not make those
    // exact negatives: the derived `sinb1` lands a few ulps inside -1, leaving
    // `cosb1` at about 1.5e-8 instead of 0. That leaks into the inverse's `ab`
    // term and biases every latitude on a south-polar grid by a constant 19 cm,
    // at every radius. The north pole is exact by luck — `authalic_q(1)` is
    // literally the expression `qp` came from — which is why the bug only shows
    // on one hemisphere.
    const POLAR_EPS: f64 = 1.0e-10;
    let is_polar = (lat1.abs() - std::f64::consts::FRAC_PI_2).abs() < POLAR_EPS;
    let (sinb1, cosb1) = if is_polar {
        (sin_lat1.signum(), 0.0)
    } else {
        let sinb1 = authalic_q(sin_lat1, e, one_es) / qp;
        (sinb1, (1.0 - sinb1 * sinb1).max(0.0).sqrt())
    };
    // The oblateness stretch is undefined at the pole and unnecessary there.
    // PROJ decides this from the *latitude*, as above; eccodes dropped that
    // branch and guards `cosb1 == 0.0` instead, which is not the same test.
    // With `cosb1` coming out at 1.5e-8 rather than 0 at the south pole, the
    // guard misses, `dd` becomes cos(-π/2) — 6e-17, not 0 — divided by it, and
    // the projected plane inflates by eight orders of magnitude. eccodes 2.48
    // asked for a south-polar §3.140's latitudes answers
    // `Invalid value: arcsin argument=7.60531e+06` and declines to build the
    // iterator at all.
    let dd = if is_polar || cosb1 == 0.0 {
        1.0
    } else {
        lat1.cos() / ((1.0 - es * sin_lat1 * sin_lat1).sqrt() * rq * cosb1)
    };
    LambertAzimuthalConstants {
        e,
        es,
        qp,
        rq,
        sinb1,
        cosb1,
        dd,
        xmf: rq * dd,
        ymf: rq / dd,
        apa: authalic_series(es),
        semi_major_m: a,
    }
}

/// How close to the antipode of the tangent point counts as "off the map".
/// The projection maps the whole sphere onto a disc, and its far edge is a
/// single point; `b` in the forward map goes to zero there and the scale to
/// infinity.
const LAEA_EPS: f64 = 1.0e-10;

/// Forward Lambert azimuthal equal-area: `(lat, lon)` in degrees → `(x, y)` in
/// metres from the tangent point.
///
/// Returns non-finite coordinates at the antipode of the tangent point, where
/// the projection is undefined. Callers going through
/// [`LambertAzimuthalProjector::inverse`] get `None` there instead.
///
/// **Recomputes the constants per call.** For warp loops use
/// [`LambertAzimuthalProjector`].
pub fn lambert_azimuthal_forward(p: &LambertAzimuthalParams, lat: f64, lon: f64) -> (f64, f64) {
    lambert_azimuthal_forward_with(&lambert_azimuthal_constants(p), p, lat, lon)
}

fn lambert_azimuthal_forward_with(
    k: &LambertAzimuthalConstants,
    p: &LambertAzimuthalParams,
    lat: f64,
    lon: f64,
) -> (f64, f64) {
    // Wrapped before the trig, the same trap `lambert_forward_with` documents.
    let d_lon = ((lon - p.central_longitude + 180.0).rem_euclid(360.0) - 180.0) * DEG2RAD;
    let one_es = 1.0 - k.es;
    let sinb = authalic_q((lat * DEG2RAD).sin(), k.e, one_es) / k.qp;
    let cosb = (1.0 - sinb * sinb).max(0.0).sqrt();
    let denom = 1.0 + k.sinb1 * sinb + k.cosb1 * cosb * d_lon.cos();
    if denom.abs() < LAEA_EPS {
        return (f64::NAN, f64::NAN);
    }
    let bb = (2.0 / denom).sqrt();
    (
        k.semi_major_m * k.xmf * bb * cosb * d_lon.sin(),
        k.semi_major_m * k.ymf * bb * (k.cosb1 * sinb - k.sinb1 * cosb * d_lon.cos()),
    )
}

/// Inverse Lambert azimuthal equal-area: `(x, y)` in metres → `(lat, lon)` in
/// degrees. Same recompute caveat as [`lambert_azimuthal_forward`].
pub fn lambert_azimuthal_inverse_xy(p: &LambertAzimuthalParams, x: f64, y: f64) -> (f64, f64) {
    lambert_azimuthal_inverse_xy_with(&lambert_azimuthal_constants(p), p, x, y)
}

fn lambert_azimuthal_inverse_xy_with(
    k: &LambertAzimuthalConstants,
    p: &LambertAzimuthalParams,
    x: f64,
    y: f64,
) -> (f64, f64) {
    let mut xy_x = (x / k.semi_major_m) / k.dd;
    let mut xy_y = (y / k.semi_major_m) * k.dd;
    let rho = xy_x.hypot(xy_y);
    // The tangent point itself. eccodes asserts `rho >= 1e-10` here and aborts
    // the process if a grid ever lands on it; answering with the tangent point
    // is both correct and survivable.
    if rho < LAEA_EPS {
        return (p.standard_parallel, p.central_longitude);
    }
    let asin_arg = 0.5 * rho / k.rq;
    if !(-1.0..=1.0).contains(&asin_arg) {
        // Beyond the projection disc — off the map, not a point on Earth.
        return (f64::NAN, f64::NAN);
    }
    let ce = 2.0 * asin_arg.asin();
    let (sin_ce, cos_ce) = ce.sin_cos();
    xy_x *= sin_ce;
    let ab = cos_ce * k.sinb1 + xy_y * sin_ce * k.cosb1 / rho;
    xy_y = rho * k.cosb1 * cos_ce - xy_y * k.sinb1 * sin_ce;
    let lon = p.central_longitude + xy_x.atan2(xy_y) * RAD2DEG;
    let lat = authalic_to_geodetic(ab.clamp(-1.0, 1.0).asin(), &k.apa) * RAD2DEG;
    (lat, lon)
}

/// Inverse warp: `(lat, lon)` → fractional source grid index. **Recomputes the
/// constants per call** — for warp loops prefer [`LambertAzimuthalProjector`].
pub fn lambert_azimuthal_inverse(
    p: &LambertAzimuthalParams,
    lat: f64,
    lon: f64,
) -> Option<GridIndex> {
    LambertAzimuthalProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a Lambert azimuthal equal-area grid. Owns the
/// authalic constants and the forward-projected grid origin, both invariant
/// across a warp's output pixels.
#[derive(Debug)]
pub struct LambertAzimuthalProjector {
    /// The grid this projector was built for.
    pub params: LambertAzimuthalParams,
    constants: LambertAzimuthalConstants,
    origin: (f64, f64),
}

impl LambertAzimuthalProjector {
    /// Precompute the authalic constants and the projected origin for
    /// `params`. Build once outside a warp loop.
    pub fn new(params: LambertAzimuthalParams) -> Self {
        let constants = lambert_azimuthal_constants(&params);
        let origin =
            lambert_azimuthal_forward_with(&constants, &params, params.lat_first, params.lon_first);
        Self {
            params,
            constants,
            origin,
        }
    }

    /// Project `(lat, lon)` back to the source-grid fractional index, or `None`
    /// when it falls outside the `ni × nj` extent. The shared planar body —
    /// kept as an inherent method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
    }

    /// Forward-project through the cached constants.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        lambert_azimuthal_forward_with(&self.constants, &self.params, lat, lon)
    }

    /// Inverse-project projected metres back to `(lat, lon)`.
    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        lambert_azimuthal_inverse_xy_with(&self.constants, &self.params, x, y)
    }

    /// Read-only access to the precomputed grid origin in projected metres.
    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }

    /// Whether the projection is usable. `false` leaves
    /// [`inverse`](Self::inverse) returning `None` for every point, so callers
    /// can surface "not reprojectable" rather than render blank.
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined() && self.origin.0.is_finite() && self.origin.1.is_finite()
    }
}

impl PlanarGridProjector for LambertAzimuthalProjector {
    fn grid_origin(&self) -> (f64, f64) {
        self.origin
    }
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
    }
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        self.is_well_defined()
    }
    fn snap_eps(&self) -> SnapEps {
        // The only projector here whose round trip does not close to float
        // noise. `authalic_to_geodetic` is a three-term series and so is not
        // the exact inverse of the `authalic_q` that produced the latitude —
        // PROJ and eccodes carry the same asymmetry, at about a millimetre. A
        // fixed nanometre of a cell is below that for any real grid (5e-9 of a
        // 200 km cell), so the first grid point projected back to `-1.2e-9`
        // and was rejected, dropping the whole outer row and column to
        // background.
        //
        // A centimetre is three orders of magnitude under the coarsest sane
        // grid spacing and one above the series' own error.
        SnapEps::Metres(0.01)
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
    use crate::projection::{DEFAULT_EARTH_RADIUS_M, metres_apart};

    /// The §3.140 fixture's grid: ETRS89-LAEA's tangent point (52°N 10°E) on
    /// GRS80, 20 × 16 at 200 km from a first point at 35°N 10°W, scanning
    /// `+i, +j`. See `tools/build_grib2_lambert_azimuthal_fixture.py`.
    fn efas_params() -> LambertAzimuthalParams {
        LambertAzimuthalParams {
            semi_major_m: 6_378_137.0,
            semi_minor_m: 6_356_752.314,
            ni: 20,
            nj: 16,
            lat_first: 35.0,
            lon_first: -10.0,
            standard_parallel: 52.0,
            central_longitude: 10.0,
            dx_metres: 200_000.0,
            dy_metres: 200_000.0,
        }
    }

    /// `(i, j, lat, lon)` from eccodes 2.34.1's own
    /// `lambert_azimuthal_equal_area` iterator, read at full precision through
    /// the `latitudes`/`longitudes` keys.
    ///
    /// Unlike transverse Mercator, eccodes *does* geolocate this template — and
    /// it branches on `grib_is_earth_oblate`, running the authalic-latitude
    /// algorithm for an oblate shape. So the oracle is the house one, and it is
    /// ellipsoidal. PROJ 9.4.0 agrees with it to 0.0006 mm over all 320 grid
    /// points, which is what makes either of them trustworthy.
    const ECCODES_OBLATE: [(u32, u32, f64, f64); 6] = [
        (0, 0, 34.999999991, -10.000000000),
        (19, 0, 34.622366847, 31.637938749),
        (0, 15, 60.108219531, -24.240260690),
        (19, 15, 59.435027618, 46.673881242),
        (10, 8, 51.572695998, 12.561377772),
        (5, 3, 42.105393368, 0.059125317),
    ];

    /// The same grid on the WMO mean sphere, from PROJ. The spherical case is
    /// not a separate code path — it is the same authalic algorithm with the
    /// eccentricity at zero — so "it degenerates correctly" is checked, not
    /// argued.
    const PROJ_SPHERE_LAEA: [(u32, u32, f64, f64); 6] = [
        (0, 0, 35.000000000, -10.000000000),
        (19, 0, 34.604641267, 31.713042728),
        (0, 15, 60.098327642, -24.303142513),
        (19, 15, 59.390373763, 46.848782337),
        (10, 8, 51.561375604, 12.618538968),
        (5, 3, 42.098693508, 0.078012754),
    ];

    #[test]
    fn lambert_azimuthal_matches_eccodes_on_the_spheroid() {
        let projector = LambertAzimuthalProjector::new(efas_params());
        let mut worst: f64 = 0.0;
        for (i, j, lat, lon) in ECCODES_OBLATE {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        // The oracle is quoted to 1e-9°, about a tenth of a millimetre on the
        // ground, so a millimetre is the tightest bound it can support.
        assert!(
            worst < 1e-3,
            "worst deviation from eccodes was {worst} m, expected sub-millimetre"
        );
    }

    #[test]
    fn lambert_azimuthal_degenerates_to_the_sphere_when_the_axes_are_equal() {
        let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
            semi_major_m: DEFAULT_EARTH_RADIUS_M,
            semi_minor_m: DEFAULT_EARTH_RADIUS_M,
            ..efas_params()
        });
        let mut worst: f64 = 0.0;
        for (i, j, lat, lon) in PROJ_SPHERE_LAEA {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(
            worst < 1e-3,
            "spherical degenerate case deviated {worst} m from PROJ"
        );
    }

    /// The spheroid is load-bearing here too, and more so than for transverse
    /// Mercator: over the EFAS domain the mean-radius approximation is
    /// kilometres out, and it would also disagree with eccodes — which projects
    /// an oblate §3.140 on the true spheroid — so it could not pass the test
    /// above at any tolerance.
    #[test]
    fn the_spherical_approximation_would_be_kilometres_out_on_an_efas_grid() {
        let spheroid = efas_params();
        let mean_radius = (2.0 * spheroid.semi_major_m + spheroid.semi_minor_m) / 3.0;
        let sphere = LambertAzimuthalProjector::new(LambertAzimuthalParams {
            semi_major_m: mean_radius,
            semi_minor_m: mean_radius,
            ..spheroid
        });
        let exact = LambertAzimuthalProjector::new(spheroid);
        let mut worst: f64 = 0.0;
        for j in 0..spheroid.nj {
            for i in 0..spheroid.ni {
                let (a_lat, a_lon) = exact.grid_point_lonlat(i, j);
                let (b_lat, b_lon) = sphere.grid_point_lonlat(i, j);
                worst = worst.max(metres_apart(a_lat, a_lon, b_lat, b_lon));
            }
        }
        assert!(
            worst > 2_000.0,
            "expected the spherical approximation to be kilometres out, got {worst} m"
        );
    }

    #[test]
    fn lambert_azimuthal_round_trips_through_the_grid_index() {
        let p = efas_params();
        let projector = LambertAzimuthalProjector::new(p);
        for j in 0..p.nj {
            for i in 0..p.ni {
                let (lat, lon) = projector.grid_point_lonlat(i, j);
                let idx = projector
                    .inverse(lat, lon)
                    .unwrap_or_else(|| panic!("grid point ({i}, {j}) did not invert"));
                assert!(
                    (idx.i - i as f64).abs() < 1e-6 && (idx.j - j as f64).abs() < 1e-6,
                    "({i}, {j}) round-tripped to ({}, {})",
                    idx.i,
                    idx.j
                );
            }
        }
    }

    /// The first grid point lands back on the `La1`/`Lo1` the message states —
    /// to within the algorithm's own asymmetry, which is about a millimetre and
    /// is not ours to remove.
    ///
    /// `authalic_to_geodetic` is a three-term series, so it is not the exact
    /// inverse of the `authalic_q` that the forward map used: 35°N comes back
    /// as 34.999999991°, a centimetre-scale wobble that PROJ and eccodes share
    /// exactly. eccodes reports *the same* 34.999999991 for this fixture's
    /// first point, which is the real check — landing on the declared value
    /// more precisely than eccodes does would mean a different algorithm, not a
    /// better one. A sign or wrap error in the forward map still shows up here
    /// first, at degrees rather than nanodegrees.
    #[test]
    fn the_lambert_azimuthal_origin_is_the_declared_first_grid_point() {
        let p = efas_params();
        let projector = LambertAzimuthalProjector::new(p);
        let (lat, lon) = projector.grid_point_lonlat(0, 0);
        assert!(
            metres_apart(p.lat_first, p.lon_first, lat, lon) < 5e-3,
            "origin came back as ({lat}, {lon})"
        );
        // And it agrees with eccodes' own answer far more tightly than it
        // agrees with the declared value.
        assert!(
            metres_apart(34.999999991, -10.0, lat, lon) < 1e-4,
            "origin ({lat}, {lon}) disagrees with eccodes"
        );
    }

    /// The polar aspect, where the oblateness stretch is undefined — and where
    /// eccodes gets the *south* pole wrong.
    ///
    /// Its guard is `cosb1 == 0.0`, which holds at +90° by luck and fails at
    /// −90° because `authalic_q(-1)` is not the exact negation of the `qp` it is
    /// divided by. Asked for the latitudes of a south-polar §3.140 message,
    /// eccodes 2.48 answers
    /// `Invalid value: arcsin argument=7.60531e+06` and refuses to build the
    /// iterator at all — the projected plane has inflated by eight orders of
    /// magnitude. So this case has no eccodes oracle, and PROJ supplies it.
    #[test]
    fn lambert_azimuthal_handles_the_north_polar_aspect() {
        let p = LambertAzimuthalParams {
            standard_parallel: 90.0,
            central_longitude: 0.0,
            lat_first: 60.0,
            lon_first: 0.0,
            ni: 8,
            nj: 6,
            ..efas_params()
        };
        let projector = LambertAzimuthalProjector::new(p);
        assert!(projector.is_well_defined());
        // eccodes 2.34.1 geolocates this one; these are its answers.
        for (i, j, lat, lon) in [
            (0u32, 0u32, 60.0_f64, 0.0_f64),
            (1, 0, 59.9439, 3.4580),
            (2, 0, 59.7762, 6.8909),
        ] {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            assert!(
                metres_apart(lat, lon, got_lat, got_lon) < 20.0,
                "({i}, {j}) gave ({got_lat}, {got_lon}), eccodes says ({lat}, {lon})"
            );
        }
    }

    /// The south-polar aspect, against PROJ — the case eccodes cannot answer.
    #[test]
    fn lambert_azimuthal_handles_the_south_polar_aspect() {
        let p = LambertAzimuthalParams {
            standard_parallel: -90.0,
            central_longitude: 0.0,
            lat_first: -60.0,
            lon_first: 0.0,
            ni: 8,
            nj: 6,
            ..efas_params()
        };
        let projector = LambertAzimuthalProjector::new(p);
        assert!(
            projector.is_well_defined(),
            "the south-polar aspect is unusable"
        );
        let mut worst: f64 = 0.0;
        for (i, j, lat, lon) in [
            (0u32, 0u32, -59.999999998, 0.000000000),
            (7, 0, -57.352900900, 22.927574019),
            (0, 5, -50.586924754, 0.000000000),
            (7, 5, -48.462661034, 17.995848243),
            (4, 3, -53.617984807, 11.563846694),
        ] {
            let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
            let idx = projector.inverse(got_lat, got_lon).expect("inverts");
            assert!((idx.i - i as f64).abs() < 1e-6 && (idx.j - j as f64).abs() < 1e-6);
        }
        assert!(worst < 1e-3, "worst deviation from PROJ was {worst} m");
    }

    /// The tangent point itself is where `rho` goes to zero. eccodes asserts on
    /// that case and aborts the process; answering with the tangent point is
    /// both correct and survivable.
    #[test]
    fn lambert_azimuthal_inverts_its_own_tangent_point() {
        let p = efas_params();
        let (lat, lon) = lambert_azimuthal_inverse_xy(&p, 0.0, 0.0);
        assert!(
            (lat - p.standard_parallel).abs() < 1e-9 && (lon - p.central_longitude).abs() < 1e-9,
            "the tangent point inverted to ({lat}, {lon})"
        );
    }

    /// The projection maps the globe onto a disc whose edge is the antipode of
    /// the tangent point. Beyond it there is no point on Earth, and past the
    /// antipode the forward map's scale is infinite — both must decline rather
    /// than fold back onto a plausible index.
    #[test]
    fn lambert_azimuthal_declines_off_the_projection_disc() {
        let p = efas_params();
        // The antipode of 52°N 10°E.
        let (x, y) = lambert_azimuthal_forward(&p, -52.0, -170.0);
        assert!(
            !x.is_finite() || !y.is_finite(),
            "the antipode projected to a finite ({x}, {y})"
        );
        // Well outside the disc: 4 Earth radii from the tangent point.
        let (lat, lon) = lambert_azimuthal_inverse_xy(&p, 4.0 * p.semi_major_m, 0.0);
        assert!(lat.is_nan() && lon.is_nan(), "got ({lat}, {lon})");
        // And the grid inverse declines for a point nowhere near the grid.
        let projector = LambertAzimuthalProjector::new(p);
        for (lat, lon) in [(-52.0, -170.0), (-80.0, 100.0), (5.0, -150.0)] {
            assert!(
                projector.inverse(lat, lon).is_none(),
                "({lat}, {lon}) resolved to a grid index"
            );
        }
    }

    #[test]
    fn lambert_azimuthal_rejects_a_degenerate_spheroid() {
        for (major, minor) in [
            (0.0, 0.0),
            (1.0, -1.0),
            (-6_371_229.0, -6_371_229.0),
            (f64::NAN, 6_371_229.0),
            (6_356_752.0, 6_378_137.0),
        ] {
            let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
                semi_major_m: major,
                semi_minor_m: minor,
                ..efas_params()
            });
            assert!(!projector.is_well_defined(), "({major}, {minor}) is usable");
            assert!(
                projector.inverse(52.0, 10.0).is_none(),
                "({major}, {minor}) resolved a grid index"
            );
        }
    }

    /// A longitude carried in [0, 360) has to land in the same place as its
    /// signed twin.
    #[test]
    fn lambert_azimuthal_wraps_longitude_before_projecting() {
        let p = efas_params();
        for (lat, lon) in [(35.0, -10.0), (52.0, 10.0), (60.1, -24.2)] {
            let signed = lambert_azimuthal_forward(&p, lat, lon);
            let unsigned = lambert_azimuthal_forward(&p, lat, lon + 360.0);
            assert!(
                (signed.0 - unsigned.0).abs() < 1e-6 && (signed.1 - unsigned.1).abs() < 1e-6,
                "{lon} and {} projected differently",
                lon + 360.0
            );
        }
    }

    /// A grid that runs off the projection disc must still produce a finite
    /// bounding box around the part of it that exists.
    ///
    /// Found by pushing the §3.140 extent past the antipode: 1 369 of 1 600
    /// points stop being points on Earth, every perimeter sample but the first
    /// corner comes back `NaN`, and the longitude bound came out `NaN` — which
    /// the warp consumes as a target extent and renders as nothing. The shared
    /// perimeter walk now skips non-projectable samples, which also closes the
    /// same hole for Lambert, whose forward map is documented to return `±inf`
    /// at the projection pole.
    #[test]
    fn a_bbox_over_a_partly_unprojectable_grid_stays_finite() {
        let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
            ni: 40,
            nj: 40,
            dx_metres: 900_000.0,
            dy_metres: 900_000.0,
            ..efas_params()
        });
        let (lat_min, lat_max, lon_min, lon_max) = projector.lonlat_bbox();
        assert!(
            lat_min.is_finite() && lat_max.is_finite(),
            "latitude bound was ({lat_min}, {lat_max})"
        );
        assert!(
            lon_min.is_finite() && lon_max.is_finite(),
            "longitude bound was ({lon_min}, {lon_max})"
        );
        assert!(lat_min < lat_max, "empty latitude span");
    }
}
