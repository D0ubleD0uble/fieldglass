//! Transverse Mercator grids — GRIB2 template 3.12.
//!
//! Krüger series on the spheroid (the same formulation PROJ uses), because the
//! templates that carry this projection — UKV, UTM zones — declare a spheroid
//! and the spherical approximation is kilometres out across a national domain.
//! eccodes 2.34.1 has no geoiterator for this template, so the oracle for the
//! tests below is PROJ.

use super::{DEG2RAD, GridIndex, PlanarGridProjector, RAD2DEG};

/// A transverse Mercator grid: the UTM / British National Grid construction,
/// with the projection cylinder tangent along a meridian rather than the
/// equator. The Met Office publishes UKV, its 1.5 km UK model, on one.
///
/// # Why this one is on the spheroid
///
/// Every other projection in this module runs on a sphere, because the grids
/// that use them declare a spherical Earth (`shapeOfTheEarth` 6, R = 6 371 229 m)
/// and the mean-radius fallback is then exact. Transverse Mercator is where that
/// stops being true: a UKV message declares Airy 1830, and projecting it on the
/// spheroid's mean radius places the field about **2.8 km** from where it
/// belongs — a couple of grid cells at 1.5 km, visible as a coastline offset.
///
/// So this uses the Krüger *n*-series, which is exact on the spheroid to well
/// below a millimetre over a grid this size, and which **degenerates to the
/// spherical formulae on its own** when `a == b`: every α, β and δ coefficient
/// is a power series in `n = f / (2 - f)`, and `n` is zero for a sphere. There
/// is no separate spherical path to keep in step, and no accuracy to trade.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransverseMercatorParams {
    /// Semi-major and semi-minor axes in metres, as the message declares them.
    pub semi_major_m: f64,
    pub semi_minor_m: f64,
    pub ni: u32,
    pub nj: u32,
    /// `LaR` / `LoR` — the reference point, in degrees. `lon_ref` is the
    /// central meridian.
    pub lat_ref: f64,
    pub lon_ref: f64,
    /// `m` — scale factor at the central meridian.
    pub scale_factor: f64,
    /// `XR` / `YR` — false easting and northing in metres.
    pub false_easting_m: f64,
    pub false_northing_m: f64,
    /// `X1` / `Y1` — the first scanned grid point, in projection metres. Unlike
    /// Lambert and polar stereographic, §3.12 states the origin in the plane, so
    /// there is no forward projection to do to find it.
    pub x1_metres: f64,
    pub y1_metres: f64,
    /// Grid spacing in metres, carrying the scanning-mode sign.
    pub dx_metres: f64,
    pub dy_metres: f64,
}

/// Krüger series coefficients for one spheroid, plus the meridional arc to the
/// reference latitude. All of it depends only on `(a, b, lat_ref)`, so a warp
/// loop computes it once.
///
/// `pub` so projector helpers can hand them around; the fields are private.
#[derive(Debug, Clone, Copy)]
pub struct TransverseMercatorConstants {
    /// Third flattening, `n = f / (2 - f)`. Zero for a sphere, which zeroes
    /// every series coefficient below.
    n: f64,
    /// Rectifying radius `A`, the radius of the sphere with the same meridian
    /// arc length. Equals `a` when `n == 0`.
    rectifying_radius: f64,
    /// Forward (geodetic → projected) series.
    alpha: [f64; 5],
    /// Inverse (projected → conformal) series.
    beta: [f64; 5],
    /// Conformal → geodetic latitude series.
    delta: [f64; 5],
    /// ξ at the reference latitude on the central meridian — the arc §3.12's
    /// `YR` is measured from.
    xi_ref: f64,
}

impl TransverseMercatorConstants {
    /// Whether these constants describe a usable projection. A degenerate
    /// spheroid otherwise produces a projection that is arithmetically finite
    /// and geographically meaningless — `a = 1, b = -1` clamps to a one-metre
    /// sphere whose every point still lands inside a UK-sized grid — so this is
    /// the gate that turns such a message into "not reprojectable" rather than
    /// into a raster of garbage.
    fn well_defined(&self) -> bool {
        self.n.is_finite()
            && self.n.abs() < 1.0
            && self.rectifying_radius.is_finite()
            && self.rectifying_radius > 0.0
            && self.xi_ref.is_finite()
    }
}

fn transverse_mercator_constants(p: &TransverseMercatorParams) -> TransverseMercatorConstants {
    let a = p.semi_major_m;
    let b = p.semi_minor_m;
    // Reject a spheroid that is not one before deriving anything from it. The
    // arithmetic below is happy to carry on: `a = 1, b = -1` gives `f = 2`,
    // which the guard on `2 - f` turns into `n = 0`, i.e. a perfectly usable
    // one-metre sphere. Poisoning the rectifying radius makes
    // `well_defined` — and so the projector — reject it instead.
    if !(a.is_finite() && b.is_finite() && a > 0.0 && b > 0.0 && b <= a) {
        return TransverseMercatorConstants {
            n: f64::NAN,
            rectifying_radius: f64::NAN,
            alpha: [f64::NAN; 5],
            beta: [f64::NAN; 5],
            delta: [f64::NAN; 5],
            xi_ref: f64::NAN,
        };
    }
    // A sphere and a spheroid go down the same path: `f` and therefore `n` are
    // zero, every coefficient below vanishes, and `A` collapses to `a`.
    let f = (a - b) / a;
    let n = if (2.0 - f) != 0.0 { f / (2.0 - f) } else { 0.0 };
    let (n2, n3, n4, n5) = (n * n, n * n * n, n.powi(4), n.powi(5));
    let rectifying_radius = a / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n.powi(6) / 256.0);
    // Krüger's series to sixth order. Within a few degrees of the central
    // meridian these are exact to well under a millimetre, which the tests
    // check against PROJ rather than assert from the literature.
    let alpha = [
        n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0,
        13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0,
        61.0 * n3 / 240.0 - 103.0 * n4 / 140.0 + 15061.0 * n5 / 26880.0,
        49561.0 * n4 / 161280.0 - 179.0 * n5 / 168.0,
        34729.0 * n5 / 80640.0,
    ];
    let beta = [
        n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0 - 81.0 * n5 / 512.0,
        n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0 + 46.0 * n5 / 105.0,
        17.0 * n3 / 480.0 - 37.0 * n4 / 840.0 - 209.0 * n5 / 4480.0,
        4397.0 * n4 / 161280.0 - 11.0 * n5 / 504.0,
        4583.0 * n5 / 161280.0,
    ];
    let delta = [
        2.0 * n - 2.0 * n2 / 3.0 - 2.0 * n3 + 116.0 * n4 / 45.0 + 26.0 * n5 / 45.0,
        7.0 * n2 / 3.0 - 8.0 * n3 / 5.0 - 227.0 * n4 / 45.0 + 2704.0 * n5 / 315.0,
        56.0 * n3 / 15.0 - 136.0 * n4 / 35.0 - 1262.0 * n5 / 105.0,
        4279.0 * n4 / 630.0 - 332.0 * n5 / 35.0,
        4174.0 * n5 / 315.0,
    ];
    // ξ of the reference latitude, on the central meridian where η' is zero and
    // every cosh term is 1.
    let xi_ref_prime = conformal_xi_eta(n, p.lat_ref * DEG2RAD, 0.0).0;
    let mut xi_ref = xi_ref_prime;
    for (j, a_j) in alpha.iter().enumerate() {
        xi_ref += a_j * (2.0 * (j as f64 + 1.0) * xi_ref_prime).sin();
    }
    TransverseMercatorConstants {
        n,
        rectifying_radius,
        alpha,
        beta,
        delta,
        xi_ref,
    }
}

/// `(ξ', η')` — the Gauss-Schreiber coordinates of a geodetic latitude and a
/// longitude offset from the central meridian, both in radians.
///
/// The isometric-latitude step uses `e = 2√n / (1 + n)`, which is the first
/// eccentricity exactly (`e² = 4n/(1+n)² = f(2-f)`), so no separate `e` needs
/// carrying. For a sphere `n` is zero and `t` reduces to `tan φ`.
fn conformal_xi_eta(n: f64, lat: f64, d_lon: f64) -> (f64, f64) {
    let sin_lat = lat.sin();
    let t = if n > 0.0 {
        let e = 2.0 * n.sqrt() / (1.0 + n);
        (sin_lat.atanh() - e * (e * sin_lat).atanh()).sinh()
    } else {
        lat.tan()
    };
    let xi = t.atan2(d_lon.cos());
    // `sin(dλ) / √(1 + t²)` is bounded by 1 in exact arithmetic, but round-off
    // at |dλ| = 90° can nudge it past — where `atanh` is infinite. Clamp just
    // inside instead of returning a coordinate that renders as a blank column.
    let eta = (d_lon.sin() / t.hypot(1.0)).clamp(-1.0 + f64::EPSILON, 1.0 - f64::EPSILON);
    (xi, eta.atanh())
}

/// Forward transverse Mercator: `(lat, lon)` in degrees → `(x, y)` in metres,
/// including the false easting and northing.
///
/// **Recomputes the series coefficients per call.** For warp loops use
/// [`TransverseMercatorProjector`], which caches them once.
pub fn transverse_mercator_forward(p: &TransverseMercatorParams, lat: f64, lon: f64) -> (f64, f64) {
    transverse_mercator_forward_with(&transverse_mercator_constants(p), p, lat, lon)
}

fn transverse_mercator_forward_with(
    k: &TransverseMercatorConstants,
    p: &TransverseMercatorParams,
    lat: f64,
    lon: f64,
) -> (f64, f64) {
    // Wrap into [-180, 180] before the trig: a query longitude carried in
    // [0, 360) against a central meridian in [-180, 180] otherwise lands on the
    // far side of the cylinder, the same trap `lambert_forward_with` documents.
    let d_lon = ((lon - p.lon_ref + 180.0).rem_euclid(360.0) - 180.0) * DEG2RAD;
    let (xi_p, eta_p) = conformal_xi_eta(k.n, lat * DEG2RAD, d_lon);
    let (mut xi, mut eta) = (xi_p, eta_p);
    for (j, a_j) in k.alpha.iter().enumerate() {
        let two_j = 2.0 * (j as f64 + 1.0);
        xi += a_j * (two_j * xi_p).sin() * (two_j * eta_p).cosh();
        eta += a_j * (two_j * xi_p).cos() * (two_j * eta_p).sinh();
    }
    let scale = p.scale_factor * k.rectifying_radius;
    (
        p.false_easting_m + scale * eta,
        p.false_northing_m + scale * (xi - k.xi_ref),
    )
}

/// Inverse transverse Mercator: `(x, y)` in metres → `(lat, lon)` in degrees.
/// Same recompute caveat as [`transverse_mercator_forward`].
pub fn transverse_mercator_inverse_xy(p: &TransverseMercatorParams, x: f64, y: f64) -> (f64, f64) {
    transverse_mercator_inverse_xy_with(&transverse_mercator_constants(p), p, x, y)
}

fn transverse_mercator_inverse_xy_with(
    k: &TransverseMercatorConstants,
    p: &TransverseMercatorParams,
    x: f64,
    y: f64,
) -> (f64, f64) {
    let scale = p.scale_factor * k.rectifying_radius;
    let xi = (y - p.false_northing_m) / scale + k.xi_ref;
    let eta = (x - p.false_easting_m) / scale;
    let (mut xi_p, mut eta_p) = (xi, eta);
    for (j, b_j) in k.beta.iter().enumerate() {
        let two_j = 2.0 * (j as f64 + 1.0);
        xi_p -= b_j * (two_j * xi).sin() * (two_j * eta).cosh();
        eta_p -= b_j * (two_j * xi).cos() * (two_j * eta).sinh();
    }
    // Conformal latitude, then the footpoint series back to geodetic.
    let chi = (xi_p.sin() / eta_p.cosh()).clamp(-1.0, 1.0).asin();
    let mut lat = chi;
    for (j, d_j) in k.delta.iter().enumerate() {
        lat += d_j * (2.0 * (j as f64 + 1.0) * chi).sin();
    }
    let lon = p.lon_ref + eta_p.sinh().atan2(xi_p.cos()) * RAD2DEG;
    (lat * RAD2DEG, lon)
}

/// Inverse warp: `(lat, lon)` → fractional source grid index. **Recomputes the
/// series per call** — for warp loops prefer [`TransverseMercatorProjector`].
pub fn transverse_mercator_inverse(
    p: &TransverseMercatorParams,
    lat: f64,
    lon: f64,
) -> Option<GridIndex> {
    TransverseMercatorProjector::new(*p).inverse(lat, lon)
}

/// Precomputed inverse map for a transverse Mercator grid. Owns the Krüger
/// coefficients and the reference arc, both invariant across a warp's output
/// pixels. Build once outside the per-pixel loop.
#[derive(Debug)]
pub struct TransverseMercatorProjector {
    pub params: TransverseMercatorParams,
    constants: TransverseMercatorConstants,
}

impl TransverseMercatorProjector {
    pub fn new(params: TransverseMercatorParams) -> Self {
        let constants = transverse_mercator_constants(&params);
        Self { params, constants }
    }

    /// Project `(lat, lon)` back to the source-grid fractional index, or
    /// `None` when it falls outside the `ni × nj` extent. The shared planar
    /// body — kept as an inherent method so callers need not import the trait.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        PlanarGridProjector::inverse(self, lat, lon)
    }

    /// Forward-project through the cached coefficients.
    pub fn forward(&self, lat: f64, lon: f64) -> (f64, f64) {
        transverse_mercator_forward_with(&self.constants, &self.params, lat, lon)
    }

    /// Inverse-project projected metres back to `(lat, lon)`.
    pub fn inverse_xy(&self, x: f64, y: f64) -> (f64, f64) {
        transverse_mercator_inverse_xy_with(&self.constants, &self.params, x, y)
    }

    /// Whether the projection is usable. `false` leaves
    /// [`inverse`](Self::inverse) returning `None` for every point, so callers
    /// can surface "not reprojectable" rather than render blank.
    pub fn is_well_defined(&self) -> bool {
        self.constants.well_defined()
    }
}

impl PlanarGridProjector for TransverseMercatorProjector {
    fn grid_origin(&self) -> (f64, f64) {
        (self.params.x1_metres, self.params.y1_metres)
    }
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64) {
        self.forward(lat, lon)
    }
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        // The scale factor multiplies the rectifying radius: zero collapses
        // the whole plane onto the false origin, and a non-finite one makes
        // every index `NaN`.
        self.constants.well_defined()
            && self.params.scale_factor.is_finite()
            && self.params.scale_factor != 0.0
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

    /// The §3.12 fixture's grid, field for field: British National Grid
    /// parameters on Airy 1830, `shapeOfTheEarth = 3` having rounded the axes
    /// to the metre and the IEEE-32 scale factor having rounded `0.9996012717`
    /// to `0.99960124`. Same extent as the real UKV domain, at 48 km instead of
    /// 2 km — see `tools/build_grib2_transverse_mercator_fixture.py`.
    fn ukv_params() -> TransverseMercatorParams {
        TransverseMercatorParams {
            semi_major_m: 6_377_563.0,
            semi_minor_m: 6_356_257.0,
            ni: 24,
            nj: 30,
            lat_ref: 49.0,
            lon_ref: -2.0,
            scale_factor: 0.999_601_244_926_452_6,
            false_easting_m: 400_000.0,
            false_northing_m: -100_000.0,
            x1_metres: -238_000.0,
            y1_metres: 1_222_000.0,
            dx_metres: 48_000.0,
            dy_metres: -48_000.0,
        }
    }

    /// `(x, y, lat, lon)` from PROJ 9.4.0, `cs2cs` from the tmerc definition
    /// above to `+proj=longlat` on the *same* spheroid — no datum shift, so any
    /// disagreement is the projection maths and nothing else.
    ///
    /// PROJ rather than eccodes because eccodes has no transverse-Mercator
    /// geoiterator at all: `codes_grib_iterator_new` on a §3.12 message answers
    /// "Function not yet implemented", at the pinned 2.34.1 and still at 2.48.
    /// There is no version of the usual oracle to fall back to, so the check is
    /// against the reference implementation of the projection itself.
    const PROJ_ELLIPSOID: [(f64, f64, f64, f64); 11] = [
        (-238000.0, 1222000.0, 60.374180125, -13.611297212),
        (-216000.0, 1222000.0, 60.408174975, -13.220013439),
        (-238000.0, 1194000.0, 60.127855836, -13.523227104),
        (-216000.0, 1194000.0, 60.161514449, -13.134756726),
        (-228000.0, 1208000.0, 60.266544782, -13.389971606),
        (400000.0, -100000.0, 49.000000000, -2.000000000),
        (0.0, 0.0, 49.766185845, -7.556449264),
        (856000.0, 1222000.0, 60.620756496, 6.348094228),
        (-238000.0, -184000.0, 47.925395454, -10.544600945),
        (1200000.0, 600000.0, 54.653549529, 10.433629205),
        (-600000.0, 600000.0, 54.301200951, -17.429687151),
    ];

    /// The same points with `+R=6371229`, the WMO mean sphere. Pinned because
    /// the spherical case is not a separate code path here — it is the same
    /// Krüger series with `n = 0` — and "it degenerates correctly" is a claim
    /// that has to be checked rather than argued.
    const PROJ_SPHERE: [(f64, f64, f64, f64); 11] = [
        (-238000.0, 1222000.0, 60.383180300, -13.655835175),
        (-216000.0, 1222000.0, 60.417371041, -13.263119140),
        (-238000.0, 1194000.0, 60.136433927, -13.567091090),
        (-216000.0, 1194000.0, 60.170284169, -13.177208281),
        (-228000.0, 1208000.0, 60.275421703, -13.433525242),
        (400000.0, -100000.0, 49.000000000, -2.000000000),
        (0.0, 0.0, 49.765852937, -7.572796392),
        (856000.0, 1222000.0, 60.631190800, 6.380489047),
        (-238000.0, -184000.0, 47.924585237, -10.568782438),
        (1200000.0, 600000.0, 54.654239552, 10.474202872),
        (-600000.0, 600000.0, 54.300446549, -17.479364963),
    ];

    #[test]
    fn transverse_mercator_inverse_matches_proj_on_the_spheroid() {
        let p = ukv_params();
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in PROJ_ELLIPSOID {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        // A millimetre, three orders of magnitude below the 2 km grid spacing
        // and far below the accuracy the oracle itself is quoted to.
        assert!(
            worst < 1e-3,
            "worst deviation from PROJ was {worst} m, expected sub-millimetre"
        );
    }

    #[test]
    fn transverse_mercator_forward_matches_proj_on_the_spheroid() {
        let p = ukv_params();
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in PROJ_ELLIPSOID {
            let (got_x, got_y) = transverse_mercator_forward(&p, lat, lon);
            worst = worst.max((got_x - x).hypot(got_y - y));
        }
        // Looser than the inverse only because the oracle latitudes are quoted
        // to 1e-9°, which is itself about a tenth of a millimetre on the ground.
        assert!(
            worst < 1e-2,
            "worst forward deviation from PROJ was {worst} m"
        );
    }

    /// `a == b` must reduce to spherical transverse Mercator exactly, because
    /// that is the only spherical implementation there is.
    #[test]
    fn transverse_mercator_degenerates_to_the_sphere_when_the_axes_are_equal() {
        let p = TransverseMercatorParams {
            semi_major_m: DEFAULT_EARTH_RADIUS_M,
            semi_minor_m: DEFAULT_EARTH_RADIUS_M,
            ..ukv_params()
        };
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in PROJ_SPHERE {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(
            worst < 1e-3,
            "spherical degenerate case deviated {worst} m from PROJ"
        );
    }

    /// The spheroid is not a detail that can be dropped for this template. A
    /// UKV grid projected on the mean radius instead of Airy 1830 lands about
    /// 2.8 km away — nearly two grid cells at 1.5 km, and a visible coastline
    /// offset. This is what justifies carrying two axes through the seam
    /// rather than one mean radius, so it is asserted rather than commented.
    #[test]
    fn the_spherical_approximation_would_be_kilometres_out_on_a_ukv_grid() {
        let spheroid = ukv_params();
        let mean_radius = (2.0 * spheroid.semi_major_m + spheroid.semi_minor_m) / 3.0;
        let sphere = TransverseMercatorParams {
            semi_major_m: mean_radius,
            semi_minor_m: mean_radius,
            ..spheroid
        };
        let mut worst: f64 = 0.0;
        for (x, y, _, _) in PROJ_ELLIPSOID {
            let (a_lat, a_lon) = transverse_mercator_inverse_xy(&spheroid, x, y);
            let (b_lat, b_lon) = transverse_mercator_inverse_xy(&sphere, x, y);
            worst = worst.max(metres_apart(a_lat, a_lon, b_lat, b_lon));
        }
        assert!(
            (2_000.0..4_000.0).contains(&worst),
            "expected the spherical approximation to be a few km out, got {worst} m"
        );
    }

    /// The Krüger series is not UKV-specific, and neither is the template: a
    /// §3.12 message can carry any UTM zone. `lat_ref = 0` in particular takes
    /// a different path through the reference-arc term, where `xi_ref` is zero
    /// and a sign error in it would be invisible against a UKV grid.
    #[test]
    fn transverse_mercator_matches_proj_for_a_utm_zone() {
        // UTM zone 31N on WGS84.
        let p = TransverseMercatorParams {
            semi_major_m: 6_378_137.0,
            semi_minor_m: 6_356_752.314_245,
            lat_ref: 0.0,
            lon_ref: 3.0,
            scale_factor: 0.9996,
            false_easting_m: 500_000.0,
            false_northing_m: 0.0,
            ..ukv_params()
        };
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in [
            (500000.0, 0.0, 0.000000000, 3.000000000),
            (300000.0, 5000000.0, 45.125153848, 0.456876510),
            (700000.0, 5000000.0, 45.125153848, 5.543123490),
            (500000.0, 9000000.0, 81.060880975, 3.000000000),
            (166000.0, 1000000.0, 9.033978846, -0.037660965),
            (834000.0, 1000000.0, 9.033978846, 6.037660965),
        ] {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(worst < 1e-3, "worst deviation from PROJ was {worst} m");
    }

    /// A southern-hemisphere zone, where the reference latitude is negative and
    /// the false northing is the 5 000 km offset the southern UTM convention
    /// uses. Both signs would cancel in a north-only test.
    #[test]
    fn transverse_mercator_matches_proj_south_of_the_equator() {
        let p = TransverseMercatorParams {
            semi_major_m: 6_378_137.0,
            semi_minor_m: 6_356_752.314_245,
            lat_ref: -33.0,
            lon_ref: 151.0,
            scale_factor: 0.99994,
            false_easting_m: 300_000.0,
            false_northing_m: 5_000_000.0,
            ..ukv_params()
        };
        let mut worst: f64 = 0.0;
        for (x, y, lat, lon) in [
            (300000.0, 5000000.0, -33.000000000, 151.000000000),
            (100000.0, 4800000.0, -34.783580919, 148.814928632),
            (500000.0, 5200000.0, -31.179175500, 153.097987240),
            (300000.0, 5600000.0, -27.587337109, 151.000000000),
        ] {
            let (got_lat, got_lon) = transverse_mercator_inverse_xy(&p, x, y);
            worst = worst.max(metres_apart(lat, lon, got_lat, got_lon));
        }
        assert!(worst < 1e-3, "worst deviation from PROJ was {worst} m");
    }

    /// The grid walk has to follow the scanning mode, and the sign of the
    /// increments is the only thing that carries it. A `j`-positive grid starts
    /// at the same `X1`/`Y1` and walks *up*; getting that backwards renders the
    /// field upside down, which no value check would catch.
    #[test]
    fn transverse_mercator_walks_the_grid_in_the_declared_scan_direction() {
        for (dx, dy) in [
            (48_000.0, -48_000.0),  // default scan: +i, -j
            (48_000.0, 48_000.0),   // j scans positively
            (-48_000.0, -48_000.0), // i scans negatively
            (-48_000.0, 48_000.0),
        ] {
            let p = TransverseMercatorParams {
                dx_metres: dx,
                dy_metres: dy,
                ..ukv_params()
            };
            let projector = TransverseMercatorProjector::new(p);
            // The origin is `X1`/`Y1` whichever way the grid scans.
            let (ox, oy) = projector.grid_origin();
            assert!((ox - p.x1_metres).abs() < 1e-9 && (oy - p.y1_metres).abs() < 1e-9);
            // And the last scanned point is the origin plus the full signed
            // extent — the corner a §3.12 message states as `X2`/`Y2`.
            let (lx, ly) = projector.grid_corners_xy()[3];
            assert!(
                (lx - (p.x1_metres + (p.ni as f64 - 1.0) * dx)).abs() < 1e-9
                    && (ly - (p.y1_metres + (p.nj as f64 - 1.0) * dy)).abs() < 1e-9,
                "({dx}, {dy}) walked to ({lx}, {ly})"
            );
            // Every point still inverts to its own index, in every direction.
            for (i, j) in [(0, 0), (p.ni - 1, 0), (0, p.nj - 1), (p.ni - 1, p.nj - 1)] {
                let (lat, lon) = projector.grid_point_lonlat(i, j);
                let idx = projector
                    .inverse(lat, lon)
                    .unwrap_or_else(|| panic!("({dx}, {dy}) lost corner ({i}, {j})"));
                assert!((idx.i - i as f64).abs() < 1e-6 && (idx.j - j as f64).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn transverse_mercator_round_trips_through_the_grid_index() {
        let p = ukv_params();
        let projector = TransverseMercatorProjector::new(p);
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

    /// A point outside the grid must not be silently clamped onto its edge —
    /// that is how a warp paints a border of smeared data instead of background.
    #[test]
    fn transverse_mercator_rejects_points_outside_the_grid() {
        let projector = TransverseMercatorProjector::new(ukv_params());
        // The grid covers the UK and its shelf seas; these are all well
        // outside it, including two on the central meridian so the rejection
        // is about extent rather than an easy longitude miss.
        for (lat, lon) in [(0.0, 0.0), (30.0, -2.0), (-45.0, 170.0), (89.0, -2.0)] {
            assert!(
                projector.inverse(lat, lon).is_none(),
                "({lat}, {lon}) resolved to a grid index"
            );
        }
    }

    /// A longitude carried in [0, 360) has to land in the same place as its
    /// signed twin — the wrap trap `lambert_forward_with` documents, which bit
    /// the Eta Lambert grid.
    #[test]
    fn transverse_mercator_wraps_longitude_before_projecting() {
        let p = ukv_params();
        for (lat, lon) in [(60.3, -13.5), (49.0, -2.0), (60.2, -13.2)] {
            let signed = transverse_mercator_forward(&p, lat, lon);
            let unsigned = transverse_mercator_forward(&p, lat, lon + 360.0);
            assert!(
                (signed.0 - unsigned.0).abs() < 1e-6 && (signed.1 - unsigned.1).abs() < 1e-6,
                "{lon} and {} projected differently",
                lon + 360.0
            );
        }
    }

    /// Degenerate parameters must disable the projector rather than produce a
    /// plausible-looking index.
    ///
    /// `(1.0, -1.0)` is the case worth keeping: the flattening it implies is 2,
    /// the `2 - f` guard turns that into `n = 0`, and what comes out is a
    /// working projection on a one-metre sphere — where every point on Earth
    /// maps to within a metre of the false origin and therefore *inside* a
    /// UK-sized grid. It reports an index for every query, all of them wrong.
    /// Nothing downstream would notice.
    #[test]
    fn transverse_mercator_rejects_a_degenerate_spheroid() {
        for (major, minor) in [
            (0.0, 0.0),
            (1.0, -1.0),
            (-6_371_229.0, -6_371_229.0),
            (f64::NAN, 6_371_229.0),
            (6_371_229.0, f64::INFINITY),
            // Prolate: a spheroid squashed the other way is not something WMO's
            // shape table can describe, so it is corrupt rather than exotic.
            (6_356_257.0, 6_377_563.0),
        ] {
            let projector = TransverseMercatorProjector::new(TransverseMercatorParams {
                semi_major_m: major,
                semi_minor_m: minor,
                ..ukv_params()
            });
            assert!(!projector.is_well_defined(), "({major}, {minor}) is usable");
            assert!(
                projector.inverse(54.0, -2.0).is_none(),
                "({major}, {minor}) resolved a grid index"
            );
        }
    }

    /// A scale factor of zero collapses the plane onto the false origin; a
    /// non-finite one makes every index `NaN`. Both must decline rather than
    /// answer.
    #[test]
    fn transverse_mercator_rejects_a_degenerate_scale_factor() {
        for scale_factor in [0.0, f64::NAN, f64::INFINITY] {
            let projector = TransverseMercatorProjector::new(TransverseMercatorParams {
                scale_factor,
                ..ukv_params()
            });
            assert!(
                projector.inverse(54.0, -2.0).is_none(),
                "scale factor {scale_factor} resolved a grid index"
            );
        }
    }
}
