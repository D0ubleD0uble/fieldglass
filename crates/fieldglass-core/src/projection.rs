//! Forward and inverse projections for the source grids the format readers
//! decode.
//!
//! One module per grid family — [`latlon`], [`mercator`], [`gaussian`],
//! [`lambert`], [`polar_stereo`], [`lambert_azimuthal`],
//! [`transverse_mercator`], [`rotated_latlon`] and [`geostationary`] — each
//! holding that family's parameters, its forward and inverse maps, its
//! projector, and the tests that pin them. Every public item is re-exported
//! here, so `fieldglass_core::projection::LambertParams` names what it always
//! did. The math references live with the family that uses them.
//!
//! What stays in this module is what more than one family needs: [`GridIndex`],
//! the [`PlanarGridProjector`] trait and the snapping policy it carries
//! ([`SnapEps`], [`GridResampling`]), the shared angle constants, and
//! [`GridGeometry`] — the typed value for "where does this grid sit on the
//! Earth".
//!
//! The render pipeline uses the inverse direction (output `(lat, lon)` → source
//! grid index) when warping into a target projection raster. The forward
//! direction geolocates a grid point, for export, contours and the point probe;
//! the two are algebraic mirrors, and the round-trip tests that hold them
//! together are in this module rather than in the families.

use std::f64::consts::PI;

pub mod gaussian;
pub mod geostationary;
pub mod lambert;
pub mod lambert_azimuthal;
pub mod latlon;
pub mod mercator;
pub mod polar_stereo;
pub mod rotated_latlon;
pub mod transverse_mercator;

use mercator::mercator_ordinate;

// Re-exported so the split is invisible from outside: every one of these has
// been reachable at `fieldglass_core::projection::<name>` since before the
// families had modules of their own.
pub use gaussian::{
    GaussianParams, GaussianProjector, expand_reduced_to_regular, gaussian_inverse,
    gaussian_latitudes, is_octahedral_pl, reduced_raster_lon_last, reduced_raster_width,
};
pub use geostationary::{
    GeostationaryConstants, GeostationaryParams, GeostationaryProjector, geostationary_inverse,
};
pub use lambert::{
    LambertConstants, LambertParams, LambertProjector, lambert_forward, lambert_inverse,
    lambert_inverse_xy,
};
pub use lambert_azimuthal::{
    LambertAzimuthalConstants, LambertAzimuthalParams, LambertAzimuthalProjector,
    lambert_azimuthal_forward, lambert_azimuthal_inverse, lambert_azimuthal_inverse_xy,
};
pub use latlon::{
    LatLonParams, eastward_lon_span, latlon_inverse, latlon_point, lon_grid_is_global,
};
pub use mercator::{MercatorParams, mercator_inverse, mercator_point};
pub use polar_stereo::{
    PolarStereoConstants, PolarStereoParams, PolarStereoProjector, polar_stereo_forward,
    polar_stereo_inverse, polar_stereo_inverse_xy,
};
pub use rotated_latlon::{
    RotatedLatLonParams, RotatedLatLonProjector, rotate_latlon, rotated_latlon_point,
    unrotate_latlon,
};
pub use transverse_mercator::{
    TransverseMercatorConstants, TransverseMercatorParams, TransverseMercatorProjector,
    transverse_mercator_forward, transverse_mercator_inverse, transverse_mercator_inverse_xy,
};

/// Earth radius used by Lambert projection math. WMO `shapeOfTheEarth = 6`
/// (spherical, R = 6 371 229 m) is the GRIB default; other shapes resolve
/// to nearby radii and the projection error is negligible at the scales
/// Fieldglass renders.
///
/// TODO: §3 GDS carries the actual `shape_of_earth` (and for oblate
/// spheroids: custom radius / axis lengths). Plumb that through
/// `LambertParams` / `GaussianParams` once we get a fixture whose
/// projection error against eccodes is visible at pixel scale.
/// Default spherical Earth radius: WMO GRIB2 code table 3.2 shape 6, the value
/// most operational GRIB2 producers declare. A message that declares its own
/// earth shape should pass that instead — the projections are sensitive to it.
/// Being off by one part in 1700 (GRIB1's 6 367 470 m) misplaces the far corner
/// of a continental grid by several kilometres.
pub const DEFAULT_EARTH_RADIUS_M: f64 = 6_371_229.0;

const DEG2RAD: f64 = PI / 180.0;
const RAD2DEG: f64 = 180.0 / PI;

/// Output of any inverse map: a fractional source-grid index, or `None`
/// when the requested `(lat, lon)` lies outside the grid coverage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridIndex {
    pub i: f64,
    pub j: f64,
}

/// Clamp `v` onto `[min(a,b), max(a,b)]` only when it sits within `eps` just
/// outside the range; otherwise return it unchanged. Used to absorb rotation
/// round-off at a grid edge without masking a genuinely out-of-range value.
///
/// Shared: the planar projectors snap a fractional grid index onto the raster,
/// geostationary snaps its scan-angle index, and the rotated lat/lon family
/// snaps a rotated coordinate back onto its declared corners.
fn snap_to_range(v: f64, a: f64, b: f64, eps: f64) -> f64 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    if v < lo && v >= lo - eps {
        lo
    } else if v > hi && v <= hi + eps {
        hi
    } else {
        v
    }
}

// ---------------------------------------------------------------------------
// Planar grids (Lambert, polar stereographic): shared corner geometry
// ---------------------------------------------------------------------------

/// Apply the GRIB scanning-mode sign to a planar projection's grid spacings.
///
/// Both GRIB1 and GRIB2 store Dx/Dy as unsigned magnitudes and carry the scan
/// direction in separate flags. The planar projectors map a point to a grid
/// index by `i = (x - origin_x) / dx`, `j = (y - origin_y) / dy` in the
/// LoV-oriented projection plane, so the increment sign *is* the scan
/// direction: `i` runs −x when it scans negatively, and `j` runs −y
/// (north→south) unless it scans positively. Default-scan grids keep positive
/// values.
///
/// Every caller that walks from the first scanned point to another grid point
/// needs this, which is why it lives beside the projectors rather than in one
/// of the format crates: the grid's declared corner is the *first scanned*
/// point, so stepping the wrong way puts the far corner on the wrong side of
/// it entirely.
pub fn signed_grid_increments(
    dx: f64,
    dy: f64,
    i_scans_negatively: bool,
    j_scans_positively: bool,
) -> (f64, f64) {
    let sdx = if i_scans_negatively {
        -dx.abs()
    } else {
        dx.abs()
    };
    let sdy = if j_scans_positively {
        dy.abs()
    } else {
        -dy.abs()
    };
    (sdx, sdy)
}

/// A projection whose source grid lies on a plane in metres — a fixed origin
/// at the first scanned point and constant `(dx, dy)` spacing. Lambert
/// conformal and polar stereographic both qualify; lat/lon and Gaussian grids
/// are already geographic and don't.
///
/// Implementors supply four cheap accessors; the trait derives the grid
/// corners from them. This is the one geometry shared by every planar warp
/// setup (target-bbox derivation) and by GRIB `bounds()` reporting, which
/// otherwise reimplement `origin + (n-1)·d` per projection.
/// How far past a grid edge a computed index may sit and still be snapped
/// What resampling a grid's geometry can support.
///
/// Lives here rather than beside [`Resampling`](crate::warp::Resampling)
/// because [`GridGeometry`] reports it and must stay outside the `render`
/// feature — the format crates take `core` with `default-features = false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GridResampling {
    /// A raster grid: the fractional part of a [`GridIndex`] is a position
    /// inside the cell and `(i + 1, j)` is the neighbour to its east, so
    /// blending between them is meaningful.
    Any,
    /// A lookup grid: the answer is a cell, not a position within one.
    /// Index-adjacent cells need not be spatially adjacent — a tripolar ocean
    /// grid folds — so blending `(i, j)` with `(i + 1, j)` would average two
    /// places that are nowhere near each other.
    NearestOnly,
}

#[cfg(feature = "render")]
impl GridResampling {
    /// What a `method` request actually becomes on this geometry.
    ///
    /// [`crate::warp::warp`] applies this before resampling, so a caller that
    /// wants to *report* what happened — a render summary naming the method —
    /// must ask the same question rather than echo the request back. Reporting
    /// "bilinear" for a lookup grid names a blend that was never performed.
    pub fn applied_to(self, method: crate::warp::Resampling) -> crate::warp::Resampling {
        match self {
            Self::NearestOnly => crate::warp::Resampling::Nearest,
            Self::Any => method,
        }
    }
}

/// The edge tolerance a projection whose round trip closes to float noise
/// needs: a nanometre of a grid cell — far above the round-off a well-behaved
/// round trip leaves, far below any real offset.
///
/// One value rather than two, because it is one rule. It is the default for
/// [`PlanarGridProjector::snap_eps`] and what [`GeostationaryProjector::
/// inverse`] uses; geostationary cannot implement that trait, since its grid
/// coordinates are scan angles in radians rather than projected metres.
///
/// # Which families snap, and which do not
///
/// #490 survived because nothing wrote this down. Keep it current when a
/// family is added:
///
/// | Family | Snap | Unit |
/// | --- | --- | --- |
/// | Lambert, polar stereo, transverse Mercator | [`PlanarGridProjector::snap_eps`] default | cell fractions |
/// | Lambert azimuthal | same hook, overridden | metres (its authalic series carries real error) |
/// | Rotated lat/lon | its own `EDGE_EPS` | **degrees**, applied in rotated space before [`latlon_inverse`] |
/// | Geostationary | this constant | cell fractions |
/// | lat/lon, Mercator, Gaussian | **none** | — |
///
/// The last row is deliberate, not an oversight of the #490 kind. Those three
/// invert by undoing their own forward arithmetic (`(lat - lat_first) / dlat`),
/// so the round trip closes to ~1e-13 cells and cannot cross a bound. A family
/// whose inverse is a *different computation* from its forward — geostationary
/// intersects a ray with an ellipsoid — has no such guarantee and needs the
/// snap. That is the question to ask of anything new, and
/// `tests/grid_round_trip.rs` is where the answer is checked.
pub const DEFAULT_SNAP_EPS: SnapEps = SnapEps::Cells(1e-9);

/// back onto it. See [`PlanarGridProjector::snap_eps`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SnapEps {
    /// A fraction of a grid cell, applied identically on both axes. What a
    /// projection whose round trip closes to float noise needs.
    Cells(f64),
    /// A distance in projected metres, divided by each axis' spacing. What a
    /// projection whose round trip carries a real, ground-scale error needs:
    /// the tolerance is then a property of the Earth, not of the cell, and a
    /// fixed cell fraction would be far too tight on a coarse grid.
    Metres(f64),
}

impl SnapEps {
    /// The `(i, j)` tolerances, in cell fractions, for a grid of this spacing.
    fn per_axis(self, dx: f64, dy: f64) -> (f64, f64) {
        match self {
            SnapEps::Cells(e) => (e, e),
            SnapEps::Metres(m) => (m / dx.abs(), m / dy.abs()),
        }
    }
}

pub trait PlanarGridProjector {
    /// Grid origin (first scanned point) in projected metres.
    fn grid_origin(&self) -> (f64, f64);
    /// `(ni, nj)` grid dimensions in points.
    fn grid_dims(&self) -> (u32, u32);
    /// `(dx, dy)` spacing in metres at the latitude of true scale.
    fn grid_spacing(&self) -> (f64, f64);
    /// Inverse-project projected metres back to `(lat, lon)` in degrees.
    fn inverse_lonlat(&self, x: f64, y: f64) -> (f64, f64);
    /// Forward-project `(lat, lon)` in degrees to projected metres — the
    /// direction [`Self::inverse_lonlat`] undoes. [`Self::inverse`] walks it:
    /// a grid index is `(forward_xy(lat, lon) - origin) / spacing`.
    fn forward_xy(&self, lat: f64, lon: f64) -> (f64, f64);

    /// Whether this projector can place `(lat, lon)` at all, asked before the
    /// forward map runs. Two kinds of answer live here. A projection whose own
    /// constants are degenerate rejects every point; one with a singularity
    /// within reach rejects the region that would hit it — polar
    /// stereographic's opposite hemisphere, where the forward `tan` runs to
    /// ±inf and the origin-relative division would turn that back into a
    /// plausible-looking index.
    ///
    /// The default accepts everything. [`Self::inverse`] already rejects a
    /// non-finite argument and a non-finite forward result, so a projector
    /// needs this only for a case those two miss.
    fn accepts(&self, _lat: f64, _lon: f64) -> bool {
        true
    }

    /// How far outside the grid an index may land and still be snapped back
    /// onto the edge.
    ///
    /// The projection arithmetic carries round-off — enough to push a point
    /// sitting exactly *on* a grid edge a hair outside it and have the extent
    /// check reject it, dropping the outermost row or column to background.
    /// The default is a nanometre of a grid cell: far above the round-off a
    /// well-behaved round trip leaves, far below any real offset. It is the
    /// same rule, for the same reason, that the rotated lat/lon inverse
    /// applies. Override it where the round trip is not exact to float noise.
    ///
    /// A new implementor inherits this without being asked, and getting it
    /// wrong is silent: the symptom is a missing outer row in a render, not an
    /// error. Check a new projector against its own
    /// [`grid_corners_lonlat`](Self::grid_corners_lonlat) — all four must come
    /// back through [`inverse`](Self::inverse) inside the grid.
    /// `tests/planar_inverse_golden.rs` does that for the projectors in its
    /// table; add yours to it.
    fn snap_eps(&self) -> SnapEps {
        DEFAULT_SNAP_EPS
    }

    /// `(lat, lon)` → fractional source-grid index, or `None` when the point
    /// falls outside the `ni × nj` extent or the projection cannot place it.
    ///
    /// The body every planar grid shares: reject what cannot be placed, walk
    /// the forward map, divide out the origin and the spacing, and let
    /// [`Self::snap_eps`] rescue a point that round-off pushed just past an
    /// edge. What differs between projections is in the three hooks, not here.
    fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() || !self.accepts(lat, lon) {
            return None;
        }
        let (ni, nj) = self.grid_dims();
        let (dx, dy) = self.grid_spacing();
        // A one-point axis has no cell to interpolate across, and a zero
        // spacing would divide the whole plane onto one index.
        if ni < 2 || nj < 2 || dx == 0.0 || dy == 0.0 {
            return None;
        }
        let (x, y) = self.forward_xy(lat, lon);
        if !x.is_finite() || !y.is_finite() {
            // The forward map hit a singularity — a pole for the conics, the
            // antipode of the tangent point for the azimuthals.
            return None;
        }
        let (ox, oy) = self.grid_origin();
        let (i_max, j_max) = (ni as f64 - 1.0, nj as f64 - 1.0);
        let (eps_i, eps_j) = self.snap_eps().per_axis(dx, dy);
        let i = snap_to_range((x - ox) / dx, 0.0, i_max, eps_i);
        let j = snap_to_range((y - oy) / dy, 0.0, j_max, eps_j);
        if i < 0.0 || i > i_max || j < 0.0 || j > j_max {
            return None;
        }
        Some(GridIndex { i, j })
    }

    /// `(lat, lon)` of grid point `(i, j)`: step out from the origin in
    /// projected metres and invert. The forward geolocation every planar grid
    /// (Lambert, polar stereographic) shares — the same `origin + i·d` walk
    /// [`Self::lonlat_bbox`] already does along the perimeter, opened up to the
    /// grid interior.
    fn grid_point_lonlat(&self, i: u32, j: u32) -> (f64, f64) {
        let (ox, oy) = self.grid_origin();
        let (dx, dy) = self.grid_spacing();
        self.inverse_lonlat(ox + i as f64 * dx, oy + j as f64 * dy)
    }

    /// The four grid corners in projected metres, ordered: origin, far-x
    /// edge, far-y edge, opposite corner.
    fn grid_corners_xy(&self) -> [(f64, f64); 4] {
        let (ox, oy) = self.grid_origin();
        let (ni, nj) = self.grid_dims();
        let (dx, dy) = self.grid_spacing();
        let ex = (ni as f64 - 1.0) * dx;
        let ey = (nj as f64 - 1.0) * dy;
        [(ox, oy), (ox + ex, oy), (ox, oy + ey), (ox + ex, oy + ey)]
    }

    /// The four grid corners as `(lat, lon)` in degrees. Longitudes are
    /// returned as the inverse produces them (may fall outside [-180, 180]);
    /// callers that need a normalised value should wrap it themselves.
    fn grid_corners_lonlat(&self) -> [(f64, f64); 4] {
        self.grid_corners_xy()
            .map(|(x, y)| self.inverse_lonlat(x, y))
    }

    /// `(lat, lon)` of the last scanned grid point — the corner diagonally
    /// opposite the origin. Same longitude caveat as [`Self::grid_corners_lonlat`].
    fn last_grid_point_lonlat(&self) -> (f64, f64) {
        self.grid_corners_lonlat()[3]
    }

    /// Axis-aligned lat/lon bounding box of the grid, returned as
    /// `(lat_min, lat_max, lon_min, lon_max)`.
    ///
    /// The box is taken over a dense sample of the grid **perimeter**, not
    /// just the four corners. A planar grid edge is a straight line in
    /// projected metres but a *curve* in lat/lon, and its lat/lon extrema
    /// generally fall in the interior of an edge — the classic case is the
    /// point of an edge closest to the projection pole, which maximises
    /// latitude and sits nowhere near a corner. Sampling only the corners
    /// badly underestimates the extent (e.g. the CMC polar grid: corners cap
    /// at 60°N while the top edge reaches ~80.6°N). Interior grid points can't
    /// exceed the perimeter's lat/lon range for a pole-exterior grid, so the
    /// boundary walk is sufficient.
    ///
    /// The longitude extent is the **minimum enclosing arc** of the perimeter
    /// samples, found as the complement of the largest empty gap between
    /// adjacent (sorted, wrapped) sample longitudes. This yields a tight,
    /// continuous span for a grid straddling the ±180° antimeridian and, unlike
    /// a single-reference unwrap, stays correct for grids whose azimuthal
    /// extent exceeds 180° (e.g. a wide Lambert tile). The result is recentered
    /// so its midpoint lies in [-180, 180]; `lon_min` may still be `< -180` (or
    /// `lon_max > 180`) to describe a dateline-spanning window — intentional,
    /// since the warp consumes it through periodic trig.
    ///
    /// A grid that fully *surrounds* the projection pole has no empty gap, so
    /// this arc degenerates; detect that with
    /// [`PolarStereoProjector::pole_inside_grid`] and override to the full 360°.
    fn lonlat_bbox(&self) -> (f64, f64, f64, f64) {
        // Subdivisions per edge. 512 puts samples ~16 km apart on an 8000 km
        // edge — fine enough to pin the closest-to-pole latitude to ~0.03°
        // while staying a trivial ~2k inverse projections regardless of grid
        // size.
        const PER_EDGE: u32 = 512;

        let (ox, oy) = self.grid_origin();
        let (ni, nj) = self.grid_dims();
        let (dx, dy) = self.grid_spacing();
        let ex = (ni as f64 - 1.0) * dx;
        let ey = (nj as f64 - 1.0) * dy;

        let mut lat_min = f64::INFINITY;
        let mut lat_max = f64::NEG_INFINITY;
        let mut lons: Vec<f64> = Vec::with_capacity(4 * (PER_EDGE as usize + 1));
        let mut visit = |x: f64, y: f64| {
            let (lat, lon) = self.inverse_lonlat(x, y);
            // Skip perimeter samples that are not points on Earth. Every planar
            // inverse here has somewhere it cannot answer: Lambert's blows up at
            // the projection pole, and Lambert azimuthal equal-area maps the
            // globe onto a disc whose outside is nowhere at all. One `NaN`
            // sample poisons `lon.rem_euclid(360.0)` and the whole longitude
            // bound with it, and a `NaN` bbox reaches the warp as target extent,
            // where it renders as an empty raster rather than as an error.
            //
            // A grid can legitimately be part-way off the disc — an oversized or
            // malformed §3.140 puts 1 369 of 1 600 points off it — and the right
            // answer for those is a box around the part that exists.
            if lat.is_finite() && lon.is_finite() {
                lat_min = lat_min.min(lat);
                lat_max = lat_max.max(lat);
                lons.push(lon.rem_euclid(360.0));
            }
        };
        for k in 0..=PER_EDGE {
            let t = k as f64 / PER_EDGE as f64;
            visit(ox + t * ex, oy); // bottom edge (j = 0)
            visit(ox + t * ex, oy + ey); // top edge (j = nj-1)
            visit(ox, oy + t * ey); // left edge (i = 0)
            visit(ox + ex, oy + t * ey); // right edge (i = ni-1)
        }

        // A perimeter with no projectable point at all — the whole grid is off
        // the map. Report the empty box rather than `±INFINITY`, which would
        // propagate into the target raster's size.
        if lons.is_empty() {
            return (0.0, 0.0, 0.0, 0.0);
        }

        // The longitude extent is the minimum enclosing arc of the perimeter
        // samples; see [`enclosing_lon_arc`].
        let (lon_min, lon_max) = enclosing_lon_arc(&mut lons);
        (lat_min, lat_max, lon_min, lon_max)
    }
}

/// Tightest longitude span (degrees) enclosing a set of perimeter-sample
/// longitudes, each already wrapped into `[0, 360)`. Returns
/// `(lon_min, lon_max)` recentred so the midpoint lies in `[-180, 180]`.
///
/// The span is the complement of the largest empty gap between adjacent
/// (sorted, wrapped) samples, so it stays tight and continuous for a grid
/// straddling the ±180° antimeridian and — unlike a single-reference unwrap —
/// for azimuthal extents wider than 180°. `lon_min < -180` (or `lon_max > 180`)
/// intentionally describes a dateline-spanning window; the warp consumes it
/// through periodic trig.
///
/// A sample set that *surrounds* a projection pole has no empty gap, so this
/// arc degenerates toward 360°; callers that can enclose a pole must detect
/// that case separately (e.g. [`PolarStereoProjector::pole_inside_grid`]).
///
/// `total_cmp`: callers feed finite longitudes, but a total order degrades
/// gracefully instead of panicking if a stray NaN ever slips through.
pub(crate) fn enclosing_lon_arc(lons: &mut [f64]) -> (f64, f64) {
    lons.sort_by(|a, b| a.total_cmp(b));
    let n = lons.len();
    let mut gap_start = 0usize; // index just after the largest gap
    let mut max_gap = lons[0] + 360.0 - lons[n - 1]; // wrap-around gap
    for i in 1..n {
        let gap = lons[i] - lons[i - 1];
        if gap > max_gap {
            max_gap = gap;
            gap_start = i;
        }
    }
    // The arc runs from the sample after the gap to the one before it, adding a
    // turn when the arc crosses 360° (interior gap).
    let lon_min = lons[gap_start];
    let lon_max = if gap_start == 0 {
        lons[n - 1]
    } else {
        lons[gap_start - 1] + 360.0
    };
    // Recenter on [-180, 180] by shifting a whole number of turns so the
    // midpoint is in range — preserves the (possibly antimeridian-spanning)
    // span while keeping the reported bounds human-sensible.
    let mid = (lon_min + lon_max) / 2.0;
    let shift = ((mid + 180.0).rem_euclid(360.0) - 180.0) - mid;
    (lon_min + shift, lon_max + shift)
}

// ---------------------------------------------------------------------------
// Forward geolocation: grid index → (lat, lon)
// ---------------------------------------------------------------------------
//
// The `*_inverse` maps answer "which grid point holds this lat/lon?" — the
// direction a warp needs, because it walks *output* pixels and samples the
// source. Exporting a field asks the opposite question: "where on Earth is grid
// point (i, j)?". Each family answers that in its own module, next to the
// inverse it is the algebraic mirror of, and is pinned against it by a
// round-trip test — so the two directions cannot drift apart. The two helpers
// every family's forward map shares live here.
//
// Longitudes come back as the underlying geometry produces them (they may sit
// outside [-180, 180]); [`normalise_lon`] is there for callers that want the
// conventional range.

/// Wrap a longitude into `[-180, 180)`. The forward maps return longitudes in
/// whatever range the grid's own corners imply (a 0..360 grid keeps 0..360);
/// an exporter that wants the conventional range applies this.
pub fn normalise_lon(lon: f64) -> f64 {
    // `rem_euclid` lands in [0, 360), so the shift lands in [-180, 180): the
    // half-open convention, with +180° folding onto -180°.
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

/// Position along an axis of `n` evenly spaced points running `first` → `last`.
///
/// The endpoints are returned *exactly*, not as `first + (n-1)·step`: the
/// declared corner is the grid's own definition of where its edge is, and
/// walking there in floating point lands an ulp away. That ulp is enough for
/// the `*_inverse` maps' inclusive range checks to reject the point as
/// off-grid, so an exporter would lose the last row of every field.
fn axis_position(first: f64, last: f64, n: u32, k: u32) -> f64 {
    if k == 0 {
        first
    } else if k == n - 1 {
        last
    } else {
        first + (last - first) * (k as f64 / (n as f64 - 1.0))
    }
}

// ---------------------------------------------------------------------------
// GridGeometry — one typed value for "where does this grid sit on the Earth"
// ---------------------------------------------------------------------------

/// Where a grid sits on the Earth, as one value per grid family.
///
/// This is the lid on [`LatLonParams`] and friends. Before it, a consumer
/// carried every family's parameters side by side behind a `grid_type` string
/// and re-derived the dispatch itself, which is what
/// `fieldglass-napi`'s 51-field `MessageMeta` view still does; nothing stopped
/// a caller reading `latin1` off a Gaussian grid. A variant carries only the
/// parameters its own family defines, so that read does not compile.
///
/// Every method answers `None` rather than guessing when the family cannot be
/// placed, and [`GridGeometry::Unsupported`] carries the label it could not
/// handle instead of erroring — a host can then say *which* grid it declined.
///
/// Every family `core` can project has a variant, so a grid that reaches
/// [`GridGeometry::Unsupported`] is one no projector exists for at all — a
/// spectral or bi-Fourier message, or a template the reader parsed only far
/// enough to name. The variants are ordered as the families are listed
/// throughout the docs: the two geographic ones, the two that are geographic
/// with a twist, then the four planar projections and the view from orbit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
#[serde(tag = "kind")]
pub enum GridGeometry {
    #[serde(rename = "latlon")]
    LatLon(LatLonParams),
    #[serde(rename = "gaussian")]
    Gaussian(GaussianParams),
    /// Evenly spaced in longitude and in the Mercator ordinate, so its rows
    /// crowd toward the equator; the corners still state it geographically.
    #[serde(rename = "mercator")]
    Mercator(MercatorParams),
    /// A lat/lon grid on a sphere whose pole has been moved. Its corners are
    /// **rotated-frame** degrees, which is why it is a family of its own rather
    /// than a [`LatLon`](Self::LatLon) with extra fields.
    #[serde(rename = "rotated_latlon")]
    RotatedLatLon(RotatedLatLonParams),
    #[serde(rename = "lambert")]
    Lambert(LambertParams),
    #[serde(rename = "polar_stereo")]
    PolarStereo(PolarStereoParams),
    /// Transverse Mercator (GRIB2 §3.12), on the spheroid the message declares
    /// rather than a mean sphere — see [`TransverseMercatorParams`].
    #[serde(rename = "transverse_mercator")]
    TransverseMercator(TransverseMercatorParams),
    /// Lambert azimuthal equal-area (GRIB2 §3.140), likewise on the spheroid.
    #[serde(rename = "lambert_azimuthal")]
    LambertAzimuthal(LambertAzimuthalParams),
    /// The view from geostationary orbit (GRIB2 §3.90). Tagged `space_view`
    /// rather than `geostationary` because that is the template name both
    /// readers and the hosts already print; the variant is named for the
    /// projection, the tag for the grid the message declares.
    #[serde(rename = "space_view")]
    Geostationary(GeostationaryParams),
    /// A grid that is a list of cell centres rather than a formula — NetCDF
    /// 2-D coordinates, GRIB2 §3.204, ICON §3.101. Answers with a cell, never
    /// a position inside one; see [`GridResampling::NearestOnly`].
    #[serde(rename = "lookup")]
    Lookup(crate::spatial_index::SpatialIndex),
    /// A family this type does not model yet. `label` is the grid type as the
    /// decoder named it, so the message can say what was declined.
    #[serde(rename = "unsupported")]
    Unsupported { label: String },
}

impl GridGeometry {
    /// The family tag, matching the `grid_type` strings the hosts already use
    /// (`"latlon"`, `"gaussian"`, `"lambert"`, `"polar_stereo"`), so #464 can
    /// swap the string dispatch for this type without renaming anything a
    /// consumer sees.
    ///
    /// This is exactly the serde tag — an unmodelled family reports
    /// `"unsupported"` here, not the grid it was, because a host deriving a DTO
    /// would otherwise read one string in JSON and another from this method.
    /// [`label`](Self::label) is the one to display.
    pub fn kind(&self) -> &str {
        match self {
            Self::LatLon(_) => "latlon",
            Self::Gaussian(_) => "gaussian",
            Self::Mercator(_) => "mercator",
            Self::RotatedLatLon(_) => "rotated_latlon",
            Self::Lambert(_) => "lambert",
            Self::PolarStereo(_) => "polar_stereo",
            Self::TransverseMercator(_) => "transverse_mercator",
            Self::LambertAzimuthal(_) => "lambert_azimuthal",
            Self::Geostationary(_) => "space_view",
            Self::Lookup(_) => "lookup",
            Self::Unsupported { .. } => "unsupported",
        }
    }

    /// The most specific name available: [`kind`](Self::kind) for a modelled
    /// family, and the decoder's own grid-type string for an unmodelled one.
    /// What a message saying which grid was declined should read.
    pub fn label(&self) -> &str {
        match self {
            Self::Unsupported { label } => label,
            other => other.kind(),
        }
    }

    /// `(ni, nj)` in grid points, or `None` for a family with no geometry.
    pub fn dims(&self) -> Option<(u32, u32)> {
        match self {
            Self::LatLon(p) => Some((p.ni, p.nj)),
            Self::Gaussian(p) => Some((p.ni, p.nj)),
            Self::Mercator(p) => Some((p.ni, p.nj)),
            Self::RotatedLatLon(p) => Some((p.ni, p.nj)),
            Self::Lambert(p) => Some((p.ni, p.nj)),
            Self::PolarStereo(p) => Some((p.ni, p.nj)),
            Self::TransverseMercator(p) => Some((p.ni, p.nj)),
            Self::LambertAzimuthal(p) => Some((p.ni, p.nj)),
            Self::Geostationary(p) => Some((p.ni, p.nj)),
            Self::Lookup(ix) => Some(ix.dims()),
            Self::Unsupported { .. } => None,
        }
    }

    /// Grid point `(i, j)` → `(lat, lon)` in degrees, or `None` when the index
    /// is off the grid or the family cannot be placed.
    pub fn forward(&self, i: u32, j: u32) -> Option<(f64, f64)> {
        let (ni, nj) = self.dims()?;
        if i >= ni || j >= nj {
            return None;
        }
        match self {
            Self::LatLon(p) => latlon_point(p, i, j),
            Self::Gaussian(p) => GaussianProjector::new(*p).grid_point_lonlat(i, j),
            Self::Mercator(p) => mercator_point(p, i, j),
            Self::RotatedLatLon(p) => rotated_latlon_point(p, i, j),
            Self::Lambert(p) => Some(LambertProjector::new(*p).grid_point_lonlat(i, j)),
            Self::PolarStereo(p) => Some(PolarStereoProjector::new(*p).grid_point_lonlat(i, j)),
            // The two spheroidal projections answer `None` for a spheroid that
            // is not one. Their constants stay finite when `a` and `b` are
            // nonsense, so without this gate a degenerate §3.12 message would
            // geolocate every point somewhere plausible-looking; it is the same
            // check the warp setup makes before offering a reprojection.
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                proj.is_well_defined().then(|| proj.grid_point_lonlat(i, j))
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                proj.is_well_defined().then(|| proj.grid_point_lonlat(i, j))
            }
            // A pixel whose line of sight misses the Earth is space, and the
            // projector already says so.
            Self::Geostationary(p) => GeostationaryProjector::new(*p).grid_point_lonlat(i, j),
            // Answered from the index's own centres (#445). The longitude
            // comes back normalised to [-180, 180]; see `SpatialIndex::centre`.
            Self::Lookup(ix) => ix.centre(i, j),
            Self::Unsupported { .. } => None,
        }
    }

    /// `(lat, lon)` in degrees → fractional grid index, or `None` when the
    /// point is outside the grid or the family cannot be placed. The direction
    /// a warp asks in.
    /// One-shot inverse. Builds the projection's constants and throws them
    /// away, so use [`inverse_at`](Self::inverse_at) for more than a handful of
    /// points; this routes through it so the two cannot answer differently.
    pub fn inverse(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        (self.inverse_at())(lat, lon)
    }

    /// The inverse map as a closure, with the projection's constants computed
    /// once instead of once per call.
    ///
    /// This is the seam a warp wants: it asks for an index per output pixel, and
    /// [`inverse`](Self::inverse) builds a projector — and with it the cone
    /// constant, its logarithms and the origin — on every one of them. A
    /// million-pixel raster pays that a million times. `SourceGrid::inverse_at`
    /// is already a closure for this reason; #464 wires this into it.
    ///
    /// An [`Unsupported`](Self::Unsupported) grid returns a closure that
    /// answers `None`, so a caller needs no second code path for it.
    pub fn inverse_at(&self) -> Box<dyn Fn(f64, f64) -> Option<GridIndex> + '_> {
        match self {
            Self::LatLon(p) => Box::new(move |lat, lon| latlon_inverse(p, lat, lon)),
            Self::Gaussian(p) => {
                // The Gaussian latitudes are an O(N²) Gauss-Legendre solve.
                // `gaussian_inverse` reaches them through a thread-local cache,
                // so the closure captures the params rather than a projector.
                Box::new(move |lat, lon| gaussian_inverse(p, lat, lon))
            }
            Self::Mercator(p) => Box::new(move |lat, lon| mercator_inverse(p, lat, lon)),
            Self::RotatedLatLon(p) => {
                // The projector caches the rotated-frame corner grid once; the
                // closure reuses it for every output pixel.
                let proj = RotatedLatLonProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::Lambert(p) => {
                let proj = LambertProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::Geostationary(p) => {
                let proj = GeostationaryProjector::new(*p);
                Box::new(move |lat, lon| proj.inverse(lat, lon))
            }
            Self::Lookup(ix) => Box::new(move |lat, lon| ix.nearest(lat, lon)),
            Self::Unsupported { .. } => Box::new(|_, _| None),
        }
    }

    /// What resampling this geometry supports. A lookup grid is
    /// [`GridResampling::NearestOnly`]; every formula grid is `Any`.
    pub fn resampling(&self) -> GridResampling {
        match self {
            Self::Lookup(ix) => ix.resampling(),
            _ => GridResampling::Any,
        }
    }

    /// The grid's geographic extent as `(lat_min, lat_max, lon_min, lon_max)`
    /// in degrees, or `None` for a family that cannot be placed.
    ///
    /// The projected families delegate to
    /// [`PlanarGridProjector::lonlat_bbox`], which subdivides each edge 512
    /// times rather than walking grid points: a conic's edges are curves and
    /// the extreme latitude sits between two points, not on one. It also skips
    /// perimeter samples that are not on the Earth at all, which an oversized
    /// §3.140 grid produces, and widens to the full 360° when the domain
    /// surrounds the projection pole and the enclosing longitude arc therefore
    /// degenerates.
    ///
    /// `lon_min` may fall below -180 (or `lon_max` above 180) to describe a
    /// window spanning the antimeridian — the existing convention, which the
    /// warp consumes through periodic trig. Do not normalise it into range
    /// without collapsing the span.
    pub fn lonlat_bbox(&self) -> Option<(f64, f64, f64, f64)> {
        match self {
            // The geographic families state their own corners, so the box is
            // the corners — unwrapped through `eastward_lon_span` so a grid
            // published from 180°E keeps its span instead of collapsing to one
            // cell.
            Self::LatLon(p) => Some(corner_bbox(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            )),
            Self::Gaussian(p) => Some(corner_bbox(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            )),
            // Rows crowd toward the equator, but the extreme latitudes are
            // still the two stated corners, so the box is the corners here too.
            Self::Mercator(p) => Some(corner_bbox(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            )),
            // The corners are rotated-frame degrees, so — unlike every other
            // geographic family — they are not the geographic extent. The
            // projector walks the rotated perimeter and unrotates it.
            Self::RotatedLatLon(p) => Some(RotatedLatLonProjector::new(*p).lonlat_bbox()),
            // The extent of its own centres (#445), widened to the full circle
            // when they surround a pole and the enclosing arc stops meaning
            // anything — the same degeneracy `PolarStereo` handles below.
            Self::Lookup(ix) => ix.lonlat_bbox(),
            Self::Lambert(p) => Some(LambertProjector::new(*p).lonlat_bbox()),
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                let (lat_min, lat_max, lon_min, lon_max) = proj.lonlat_bbox();
                if proj.pole_inside_grid() {
                    // Every meridian is present and the enclosing arc has no
                    // empty gap to be the complement of, so the walk's
                    // longitudes mean nothing here.
                    let (lat_min, lat_max) = if p.south_pole {
                        (-90.0, lat_max)
                    } else {
                        (lat_min, 90.0)
                    };
                    Some((lat_min, lat_max, -180.0, 180.0))
                } else {
                    Some((lat_min, lat_max, lon_min, lon_max))
                }
            }
            // Gated on the spheroid the same way `forward` is: a box walked
            // with degenerate constants is finite and meaningless, which frames
            // a render on nothing.
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                proj.is_well_defined().then(|| proj.lonlat_bbox())
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                proj.is_well_defined().then(|| proj.lonlat_bbox())
            }
            // The on-disk extent, so a cropped sector (GOES CONUS or
            // mesoscale, a Meteosat sector) frames its sector rather than a
            // hemisphere. A full disc whose whole perimeter is limb has no
            // on-disk sample to walk; fall back there to the full latitude span
            // and a quarter-turn of longitude either side of the sub-satellite
            // point, which is what the napi warp has framed such a grid with
            // since §3.90 landed. Off-disk pixels invert to `None` regardless,
            // so the fallback affects framing and never correctness.
            Self::Geostationary(p) => {
                Some(GeostationaryProjector::new(*p).lonlat_bbox().unwrap_or((
                    -90.0,
                    90.0,
                    p.sub_lon_deg - 90.0,
                    p.sub_lon_deg + 90.0,
                )))
            }
            Self::Unsupported { .. } => None,
        }
    }

    /// A PROJ definition string for the grid's coordinate reference system, or
    /// `None` for a family this type cannot place.
    ///
    /// What a browser map library wants: hand it this and the projected
    /// coordinates agree with [`forward`](Self::forward), which is what
    /// `tests/grid_geometry_proj.rs` checks against PROJ itself rather than by
    /// eye. The projected families emit absolute projection coordinates with no
    /// false easting or northing, because that is what `core`'s forward maps
    /// compute; the grid origin is applied on top, by
    /// [`inverse`](Self::inverse), not by the CRS.
    ///
    /// The Earth is the sphere the message declared (`+R=`), never a datum:
    /// a GRIB grid states a radius, and substituting WGS84 would move a
    /// continental grid by kilometres. The two spheroidal families state both
    /// axes (`+a=` / `+b=`) for the same reason — a UKV grid on Airy 1830 is
    /// 2.8 km out on a mean radius, an EFAS grid 13.5 km.
    ///
    /// Transverse Mercator is the one family whose CRS *does* carry a false
    /// easting and northing: §3.12 states them, and `X1`/`Y1` are already
    /// measured in the plane they define, so leaving them out of the string
    /// would move the grid by exactly them.
    ///
    /// [`RotatedLatLon`](Self::RotatedLatLon) answers `None` even though it
    /// places its points perfectly well. Its raster axes are degrees in the
    /// *rotated* frame, so the CRS would have to be a PROJ `ob_tran`, whose
    /// pole convention and output units this crate has no oracle for yet;
    /// naming a CRS that has not been checked against PROJ is worse than
    /// naming none, because the caller cannot tell.
    pub fn proj4(&self) -> Option<String> {
        match self {
            // Geographic: the values `forward` returns are already lon/lat.
            // No grid carries a radius for these, so the CRS states the one
            // core defaults to; it does not affect an angular coordinate.
            Self::LatLon(_) | Self::Gaussian(_) => Some(format!(
                "+proj=longlat +R={DEFAULT_EARTH_RADIUS_M} +no_defs"
            )),
            // The centres are already geodetic, so the CRS is the sphere they
            // are stated on — the same answer the two geographic families give.
            Self::Lookup(_) => Some(format!(
                "+proj=longlat +R={DEFAULT_EARTH_RADIUS_M} +no_defs"
            )),
            Self::Lambert(p) => Some(format!(
                "+proj=lcc +lat_1={} +lat_2={} +lat_0={} +lon_0={} +R={} +units=m +no_defs",
                p.latin1, p.latin2, p.lad, p.lov, p.earth_radius_m
            )),
            Self::PolarStereo(p) => Some(format!(
                "+proj=stere +lat_0={} +lat_ts={} +lon_0={} +R={} +units=m +no_defs",
                if p.south_pole { -90.0 } else { 90.0 },
                if p.south_pole {
                    -p.lad.abs()
                } else {
                    p.lad.abs()
                },
                p.lov,
                p.earth_radius_m
            )),
            // The grid is evenly spaced in longitude and in the Mercator
            // ordinate, which is exactly the plane `+proj=merc` lays out. The
            // params carry no radius — they pin the grid by its corners alone —
            // so the CRS states core's default sphere and the affine is
            // measured on it. `+lat_ts=0` keeps the scale on the equator, which
            // is where the ordinate is defined from.
            Self::Mercator(_) => Some(format!(
                "+proj=merc +lat_ts=0 +lon_0=0 +R={DEFAULT_EARTH_RADIUS_M} +units=m +no_defs"
            )),
            Self::TransverseMercator(p) => Some(format!(
                "+proj=tmerc +lat_0={} +lon_0={} +k_0={} +x_0={} +y_0={} +a={} +b={} \
                 +units=m +no_defs",
                p.lat_ref,
                p.lon_ref,
                p.scale_factor,
                p.false_easting_m,
                p.false_northing_m,
                p.semi_major_m,
                p.semi_minor_m
            )),
            Self::LambertAzimuthal(p) => Some(format!(
                "+proj=laea +lat_0={} +lon_0={} +a={} +b={} +units=m +no_defs",
                p.standard_parallel, p.central_longitude, p.semi_major_m, p.semi_minor_m
            )),
            // PROJ measures a geostationary plane in metres on the satellite's
            // sight line: one radian of scan angle is `+h` metres, and `+h` is
            // the height above the ellipsoid, not the distance from its centre
            // that `h_metres` carries.
            Self::Geostationary(p) => Some(format!(
                "+proj=geos +h={} +lon_0={} +sweep={} +a={} +b={} +units=m +no_defs",
                p.h_metres - p.r_eq,
                p.sub_lon_deg,
                if p.sweep_x { "x" } else { "y" },
                p.r_eq,
                p.r_pol
            )),
            // See the method's doc: it can place its points, but not yet name
            // the frame they are laid out in.
            Self::RotatedLatLon(_) => None,
            Self::Unsupported { .. } => None,
        }
    }

    /// Where the raster sits in the plane [`proj4`](Self::proj4) names: the
    /// first grid point and the step to the next one along each axis.
    ///
    /// This is the other half of what a map library needs. `proj4` says which
    /// plane; this says where in it the corner pixel goes and how big a pixel
    /// is. The two are computed from the same params in the same place so they
    /// cannot describe different planes — a host that derived the affine itself
    /// would be re-deriving the Mercator ordinate, the false easting, and the
    /// scan-angle-to-metre factor that the CRS strings already encode.
    ///
    /// `None` where there is no such plane: a lookup grid is a list of centres,
    /// a rotated lat/lon grid has no CRS to be affine in (see `proj4`), and an
    /// unmodelled family has neither. An axis with no constant step reports
    /// `dx`/`dy` of `None` while the origin still stands — a Gaussian grid's
    /// rows are Gauss–Legendre nodes, and a mean spacing would misplace every
    /// row but the middle one.
    ///
    /// `None` also for a grid whose own projection does not resolve — a
    /// spheroid that is not one, a Mercator corner at a pole. The implication
    /// runs one way only: an affine means there is a CRS to measure it in,
    /// while [`proj4`](Self::proj4) can still name the plane of a grid that
    /// cannot be placed in it, because the plane is a property of the
    /// projection and the affine is a property of the grid.
    pub fn plane_affine(&self) -> Option<PlaneAffine> {
        /// Spacing of `n` points spanning `span`, or `None` for a single-point
        /// axis, where no spacing is defined.
        fn step(span: f64, n: u32) -> Option<f64> {
            (n > 1).then(|| span / f64::from(n - 1))
        }
        /// The origin and spacing of a planar projection's grid, as the
        /// projector itself reports them.
        fn planar(proj: &dyn PlanarGridProjector) -> PlaneAffine {
            let (x0, y0) = proj.grid_origin();
            let (dx, dy) = proj.grid_spacing();
            PlaneAffine {
                x0,
                y0,
                dx: Some(dx),
                dy: Some(dy),
                units: PlaneUnits::Metres,
            }
        }
        match self {
            Self::LatLon(p) => Some(PlaneAffine {
                x0: p.lon_first,
                y0: p.lat_first,
                dx: step(eastward_lon_span(p.lon_first, p.lon_last), p.ni),
                dy: step(p.lat_last - p.lat_first, p.nj),
                units: PlaneUnits::Degrees,
            }),
            Self::Gaussian(p) => Some(PlaneAffine {
                x0: p.lon_first,
                y0: p.lat_first,
                dx: step(eastward_lon_span(p.lon_first, p.lon_last), p.ni),
                dy: None,
                units: PlaneUnits::Degrees,
            }),
            // Measured on the `+proj=merc` plane of core's default sphere, the
            // one `proj4` names: metres east are `R·λ` and metres north are
            // `R·ln(tan(π/4 + φ/2))`, which is what makes the rows uniform.
            Self::Mercator(p) => {
                let r = DEFAULT_EARTH_RADIUS_M;
                let y_first = mercator_ordinate(p.lat_first);
                let y_last = mercator_ordinate(p.lat_last);
                if !y_first.is_finite() || !y_last.is_finite() {
                    // A corner sits at a pole, where the ordinate diverges —
                    // the same malformed-grid guard `mercator_point` and
                    // `mercator_inverse` apply. Without it the affine leaves
                    // here as an infinite origin and a NaN step, which JSON
                    // renders as `null` and which place the raster nowhere.
                    return None;
                }
                Some(PlaneAffine {
                    x0: r * p.lon_first * DEG2RAD,
                    y0: r * y_first,
                    dx: step(
                        r * eastward_lon_span(p.lon_first, p.lon_last) * DEG2RAD,
                        p.ni,
                    ),
                    dy: step(r * (y_last - y_first), p.nj),
                    units: PlaneUnits::Metres,
                })
            }
            Self::Lambert(p) => Some(planar(&LambertProjector::new(*p))),
            Self::PolarStereo(p) => Some(planar(&PolarStereoProjector::new(*p))),
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                proj.is_well_defined().then(|| planar(&proj))
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                proj.is_well_defined().then(|| planar(&proj))
            }
            // Scan angles become metres on the same sight line `+h` measures
            // along, so the affine and the CRS agree by construction.
            Self::Geostationary(p) => {
                let h = p.h_metres - p.r_eq;
                Some(PlaneAffine {
                    x0: h * p.x0,
                    y0: h * p.y0,
                    dx: Some(h * p.dx_rad),
                    dy: Some(h * p.dy_rad),
                    units: PlaneUnits::Metres,
                })
            }
            Self::RotatedLatLon(_) | Self::Lookup(_) | Self::Unsupported { .. } => None,
        }
    }
}

/// The units a [`PlaneAffine`] is measured in — the axes of the CRS
/// [`GridGeometry::proj4`] names for the same grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PlaneUnits {
    /// `x` is a longitude and `y` a latitude, both in degrees.
    Degrees,
    /// Projection-plane metres.
    Metres,
}

/// Where a grid's raster sits in its own projection plane: the first scanned
/// point, and the step from it to the next one along each axis.
///
/// The step carries the scan sign, as the projected families' `dx`/`dy` do:
/// a north-to-south grid steps by a negative `dy`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaneAffine {
    pub x0: f64,
    pub y0: f64,
    /// `None` for an axis with no constant step — a single-point axis, or a
    /// Gaussian grid's rows.
    pub dx: Option<f64>,
    pub dy: Option<f64>,
    pub units: PlaneUnits,
}

/// Box of a geographic grid from its two stated corners, in the same
/// `(lat_min, lat_max, lon_min, lon_max)` order the projected families use.
///
/// The longitude runs `lon_first` eastward by [`eastward_lon_span`] rather than
/// `min`/`max` of the two corners: a grid published from 180°E reports
/// `lon_last` numerically below `lon_first`, and taking the extremes collapses
/// the span to a single grid step. `lon_max` may therefore exceed 180, which is
/// the convention [`GridGeometry::lonlat_bbox`] documents.
fn corner_bbox(
    lat_first: f64,
    lon_first: f64,
    lat_last: f64,
    lon_last: f64,
) -> (f64, f64, f64, f64) {
    let span = eastward_lon_span(lon_first, lon_last);
    let lon_min = lon_first;
    (
        lat_first.min(lat_last),
        lat_first.max(lat_last),
        lon_min,
        lon_min + span,
    )
}

/// Absolute-tolerance float compare, shared by every family's test module.
#[cfg(test)]
fn near(actual: f64, expected: f64, tol: f64) -> bool {
    (actual - expected).abs() < tol
}

/// Metres of great-circle error between two `(lat, lon)` pairs, close enough
/// for a comparison at this scale.
#[cfg(test)]
fn metres_apart(lat_a: f64, lon_a: f64, lat_b: f64, lon_b: f64) -> f64 {
    const M_PER_DEG: f64 = 111_320.0;
    let d_lat = (lat_a - lat_b) * M_PER_DEG;
    let d_lon = (lon_a - lon_b) * M_PER_DEG * (lat_a * DEG2RAD).cos();
    d_lat.hypot(d_lon)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed `rotated_latlon_surface.grib2` fixture: 16×31 grid, rotated
    /// corners (60,0)→(0,30), southern pole at geographic (0,0), no rotation
    /// angle. eccodes 2.34.1 `grib_get_data` reports the corner geographic
    /// coordinates used below as the oracle.
    fn rotated_fixture_params() -> RotatedLatLonParams {
        RotatedLatLonParams {
            ni: 16,
            nj: 31,
            lat_first: 60.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 30.0,
            south_pole_lat: 0.0,
            south_pole_lon: 0.0,
            angle_of_rotation: 0.0,
        }
    }

    // -----------------------------------------------------------------------
    // Forward geolocation (grid index → lat/lon)
    // -----------------------------------------------------------------------
    //
    // The load-bearing property for every grid type: the forward map must be
    // the exact inverse of the `*_inverse` map the warp already uses. Those
    // inverses are the ones validated against eccodes, so round-tripping every
    // grid point through forward → inverse pins the new direction against
    // known-good code rather than against a hand-copied constant.

    /// Assert `forward(i, j) → (lat, lon) → inverse → (i, j)` over every point
    /// of a grid, to within a fraction of a grid cell.
    fn assert_round_trips(
        ni: u32,
        nj: u32,
        forward: impl Fn(u32, u32) -> Option<(f64, f64)>,
        inverse: impl Fn(f64, f64) -> Option<GridIndex>,
        tol: f64,
        what: &str,
    ) {
        for j in 0..nj {
            for i in 0..ni {
                let (lat, lon) = forward(i, j).unwrap_or_else(|| panic!("{what}: no ({i},{j})"));
                let idx = inverse(lat, lon)
                    .unwrap_or_else(|| panic!("{what}: ({i},{j}) → ({lat},{lon}) → off-grid"));
                assert!(
                    near(idx.i, i as f64, tol) && near(idx.j, j as f64, tol),
                    "{what}: ({i},{j}) → ({lat},{lon}) → ({}, {})",
                    idx.i,
                    idx.j
                );
            }
        }
    }

    #[test]
    fn latlon_forward_inverts_the_inverse_map() {
        // A 0.25° global grid scanning north-to-south, the common GFS layout.
        let p = LatLonParams {
            ni: 41,
            nj: 21,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 359.0,
        };
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| latlon_point(&p, i, j),
            |lat, lon| latlon_inverse(&p, lat, lon),
            1e-9,
            "latlon",
        );
        // Anchor: the first point is the grid's own first corner, by definition.
        assert_eq!(latlon_point(&p, 0, 0), Some((90.0, 0.0)));
        // A degenerate grid has no step to walk and must not divide by zero.
        let degenerate = LatLonParams { ni: 1, ..p };
        assert!(latlon_point(&degenerate, 0, 0).is_none());
    }

    #[test]
    fn latlon_forward_handles_an_antimeridian_crossing_grid() {
        // ECMWF open data runs 180° → 359.75° → 0° → 179.75°, so `lon_last` is
        // numerically below `lon_first`. The forward map must walk the *eastward*
        // span (as the inverse does), not the negative difference of the corners
        // — otherwise it would march west and mirror the field.
        let p = LatLonParams {
            ni: 5,
            nj: 3,
            lat_first: 20.0,
            lon_first: 180.0,
            lat_last: -20.0,
            lon_last: 100.0, // 280° of eastward span, wrapping the seam.
        };
        let (_, lon0) = latlon_point(&p, 0, 0).expect("first point");
        let (_, lon_last) = latlon_point(&p, 4, 0).expect("last column");
        assert!(near(lon0, 180.0, 1e-9));
        // Eastward span = 100 - 180 + 360 = 280°, so the last column is at
        // 180 + 280 = 460° ≡ 100°. The raw value keeps the grid's own frame.
        assert!(near(lon_last, 460.0, 1e-9), "lon_last {lon_last}");
        assert!(near(normalise_lon(lon_last), 100.0, 1e-9));
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| latlon_point(&p, i, j),
            |lat, lon| latlon_inverse(&p, lat, lon),
            1e-9,
            "latlon seam",
        );
    }

    #[test]
    fn mercator_forward_inverts_the_inverse_map() {
        // Rows are even in the Mercator ordinate, not in latitude: a linear
        // latitude walk would misplace every interior row, and the round-trip
        // through `mercator_inverse` is what catches that.
        let p = MercatorParams {
            ni: 12,
            nj: 9,
            lat_first: -40.0,
            lon_first: -100.0,
            lat_last: 40.0,
            lon_last: -20.0,
        };
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| mercator_point(&p, i, j),
            |lat, lon| mercator_inverse(&p, lat, lon),
            1e-9,
            "mercator",
        );
        // The interior rows must NOT be evenly spaced in latitude — proof the
        // ordinate is what's being stepped.
        let lat = |j| mercator_point(&p, 0, j).expect("on grid").0;
        let (a, b, c) = (lat(0), lat(1), lat(2));
        assert!(
            ((b - a) - (c - b)).abs() > 1e-3,
            "latitude spacing must not be uniform: {a}, {b}, {c}"
        );
        // The end rows are the declared corners exactly — not an ulp away. The
        // ordinate round-trip is not bit-exact, and that drift is enough for
        // `mercator_inverse`'s inclusive latitude bounds to read the last row as
        // off-grid, which would silently drop it from an export.
        assert_eq!(mercator_point(&p, 0, 0).map(|c| c.0), Some(p.lat_first));
        assert_eq!(
            mercator_point(&p, 0, p.nj - 1).map(|c| c.0),
            Some(p.lat_last)
        );
        // Degenerate dimensions have no step to walk, as in the inverse.
        assert!(mercator_point(&MercatorParams { nj: 1, ..p }, 0, 0).is_none());
    }

    #[test]
    fn gaussian_forward_reads_the_true_row_latitudes() {
        // Gaussian rows sit at the Gauss–Legendre roots, unevenly spaced. The
        // forward map must read them from the cached table, not interpolate.
        let p = GaussianParams {
            ni: 16,
            nj: 8,
            lat_first: 78.0,
            lon_first: 0.0,
            lat_last: -78.0,
            lon_last: 337.5,
            n_parallels: 4,
        };
        let proj = GaussianProjector::new(p);
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| proj.grid_point_lonlat(i, j),
            |lat, lon| proj.inverse(lat, lon),
            1e-9,
            "gaussian",
        );
        // Row latitudes are the Gauss–Legendre roots, north-to-south here.
        let roots = gaussian_latitudes(4);
        for j in 0..p.nj {
            let (lat, _) = proj.grid_point_lonlat(0, j).expect("on grid");
            assert!(
                near(lat, roots[j as usize], 1e-12),
                "row {j}: {lat} vs root {}",
                roots[j as usize]
            );
        }
        // And they are *not* evenly spaced — the whole reason for the table.
        let d0 = roots[1] - roots[0];
        let d3 = roots[4] - roots[3];
        assert!((d0 - d3).abs() > 1e-3, "roots should not be uniform");
    }

    #[test]
    fn gaussian_forward_follows_a_south_to_north_scan() {
        // A south-first grid reverses the row order; the forward map must follow
        // the scan direction rather than always running north-to-south.
        let p = GaussianParams {
            ni: 8,
            nj: 8,
            lat_first: -78.0,
            lon_first: 0.0,
            lat_last: 78.0,
            lon_last: 315.0,
            n_parallels: 4,
        };
        let proj = GaussianProjector::new(p);
        let (first, _) = proj.grid_point_lonlat(0, 0).expect("on grid");
        assert!(first < 0.0, "a south-first scan must start south: {first}");
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| proj.grid_point_lonlat(i, j),
            |lat, lon| proj.inverse(lat, lon),
            1e-9,
            "gaussian s→n",
        );
    }

    #[test]
    fn rotated_latlon_forward_returns_geographic_coordinates() {
        let p = rotated_fixture_params();
        let proj = RotatedLatLonProjector::new(p);
        assert_round_trips(
            p.ni,
            p.nj,
            |i, j| rotated_latlon_point(&p, i, j),
            |lat, lon| proj.inverse(lat, lon),
            1e-6,
            "rotated latlon",
        );
        // The eccodes oracle already pinned in `unrotate_matches_eccodes_oracle`:
        // the first grid point, rotated (60, 0), is geographic (30, 180). The
        // forward map must report the *geographic* pair, not the rotated one.
        let (lat, lon) = rotated_latlon_point(&p, 0, 0).expect("first point");
        assert!(
            near(lat, 30.0, 1e-9) && near(lon.abs(), 180.0, 1e-9),
            "({lat},{lon})"
        );
    }

    #[test]
    fn planar_forward_walks_the_grid_from_its_origin() {
        // Lambert (CONUS) and polar stereographic (CMC) share the trait default:
        // origin + (i·dx, j·dy) in projected metres, then invert.
        let lambert = LambertProjector::new(LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 21,
            nj: 15,
            lat_first: 38.5,
            lon_first: -126.0,
            lad: 38.5,
            lov: -95.0,
            dx_metres: 13_545.0,
            dy_metres: 13_545.0,
            latin1: 38.5,
            latin2: 38.5,
        });
        assert_round_trips(
            21,
            15,
            |i, j| Some(lambert.grid_point_lonlat(i, j)),
            |lat, lon| lambert.inverse(lat, lon),
            1e-6,
            "lambert",
        );
        // Grid point (0, 0) is the declared first corner, by construction.
        let (lat, lon) = lambert.grid_point_lonlat(0, 0);
        assert!(
            near(lat, 38.5, 1e-6) && near(lon, -126.0, 1e-6),
            "({lat},{lon})"
        );

        let polar = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 21,
            nj: 17,
            lat_first: 27.203,
            lon_first: -135.213,
            lov: 249.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        });
        assert_round_trips(
            21,
            17,
            |i, j| Some(polar.grid_point_lonlat(i, j)),
            |lat, lon| polar.inverse(lat, lon),
            1e-6,
            "polar stereo",
        );
        let (lat, lon) = polar.grid_point_lonlat(0, 0);
        assert!(
            near(lat, 27.203, 1e-6) && near(normalise_lon(lon), -135.213, 1e-6),
            "({lat},{lon})"
        );
    }

    #[test]
    fn planar_inverse_accepts_a_point_sitting_exactly_on_the_grid_edge() {
        // Regression guard. The projection arithmetic carries ~1e-13 of a cell
        // in round-off, so a coordinate lying exactly *on* the first row came
        // back with j = -6.9e-14 and was rejected as off-grid by the strict
        // `j < 0.0` bound — silently dropping the outermost row/column of every
        // Lambert and polar-stereo field to background. (The rotated lat/lon
        // inverse already snapped for this reason; these two never did.)
        let lambert = LambertProjector::new(LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 21,
            nj: 15,
            lat_first: 38.5,
            lon_first: -126.0,
            lad: 38.5,
            lov: -95.0,
            dx_metres: 13_545.0,
            dy_metres: 13_545.0,
            latin1: 38.5,
            latin2: 38.5,
        });
        // Every point of the first row is on the edge; all must be accepted.
        for i in 0..21 {
            let (lat, lon) = lambert.grid_point_lonlat(i, 0);
            let idx = lambert
                .inverse(lat, lon)
                .unwrap_or_else(|| panic!("edge point ({i},0) → ({lat},{lon}) rejected"));
            assert!(near(idx.j, 0.0, 1e-6), "edge row j = {}", idx.j);
        }
        // A point genuinely outside the grid is still rejected — the snap must
        // not widen the grid, only absorb round-off.
        assert!(
            lambert.inverse(38.5, -126.0 - 5.0).is_none(),
            "a point well west of the grid must stay off-grid"
        );
    }

    #[test]
    fn geostationary_forward_locates_the_disk_and_rejects_space() {
        // A GOES-East-like full disk: the corners of the raster are space, so
        // the forward map must decline to invent a coordinate there, while the
        // centre sits under the satellite.
        let p = GeostationaryParams {
            ni: 21,
            nj: 21,
            h_metres: 42_164_160.0,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314_14,
            sub_lon_deg: -75.0,
            sweep_x: true,
            x0: -0.151844,
            dx_rad: 0.0151844,
            y0: 0.151844,
            dy_rad: -0.0151844,
        };
        let proj = GeostationaryProjector::new(p);
        // Centre pixel looks straight down: the sub-satellite point.
        let (lat, lon) = proj
            .grid_point_lonlat(10, 10)
            .expect("centre is on the disk");
        assert!(
            near(lat, 0.0, 1e-6) && near(lon, -75.0, 1e-6),
            "({lat},{lon})"
        );
        // The raster corners are off the limb.
        for (i, j) in [(0u32, 0u32), (20, 0), (0, 20), (20, 20)] {
            assert!(
                proj.grid_point_lonlat(i, j).is_none(),
                "corner ({i},{j}) should miss the Earth"
            );
        }
        // On-disk points round-trip through the inverse.
        for (i, j) in [(10u32, 10u32), (8, 12), (12, 8), (10, 6)] {
            let (lat, lon) = proj.grid_point_lonlat(i, j).expect("on disk");
            let idx = proj
                .inverse(lat, lon)
                .unwrap_or_else(|| panic!("({i},{j}) → ({lat},{lon}) → off-grid"));
            assert!(
                near(idx.i, i as f64, 1e-6) && near(idx.j, j as f64, 1e-6),
                "({i},{j}) → ({}, {})",
                idx.i,
                idx.j
            );
        }
    }

    #[test]
    fn latlon_forward_matches_the_eccodes_point_iterator() {
        // The round-trip tests above pin the forward map against our own
        // inverse; this pins it against an *outside* oracle. Geometry and
        // coordinates are eccodes' `grib_get_data` output for the committed
        // fixture `crates/fieldglass-grib2/tests/fixtures/ccsds_regular_latlon.grib2`
        // (16 × 31, 60°N 0°E → 0°N 30°E), which is what a field export must
        // reproduce point for point. The lat/lon family carries no Earth-radius
        // dependence, so this is an exact check rather than a tolerance.
        let p = LatLonParams {
            ni: 16,
            nj: 31,
            lat_first: 60.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 30.0,
        };
        for (i, j, lat, lon) in [
            (0, 0, 60.0, 0.0),   // first point
            (1, 0, 60.0, 2.0),   // one column east: Δλ = 30/15 = 2°
            (7, 10, 40.0, 14.0), // interior: lat 60 - 10·2, lon 7·2
            (15, 30, 0.0, 30.0), // last point
        ] {
            let got = latlon_point(&p, i, j).expect("on grid");
            assert!(
                near(got.0, lat, 1e-9) && near(got.1, lon, 1e-9),
                "({i},{j}) → {got:?}, eccodes says ({lat}, {lon})"
            );
        }
    }

    #[test]
    fn normalise_lon_wraps_into_the_conventional_range() {
        assert!(near(normalise_lon(460.0), 100.0, 1e-12));
        assert!(near(normalise_lon(-190.0), 170.0, 1e-12));
        assert!(near(normalise_lon(0.0), 0.0, 1e-12));
        // The half-open convention: +180 folds onto -180, and stays there.
        assert!(near(normalise_lon(180.0), -180.0, 1e-12));
        assert!(near(normalise_lon(-180.0), -180.0, 1e-12));
    }

    #[test]
    fn unrotate_matches_eccodes_oracle() {
        let p = rotated_fixture_params();
        // First grid point: rotated (60, 0) → geographic (30, 180).
        let (lat, lon) = unrotate_latlon(
            p.lat_first,
            p.lon_first,
            p.angle_of_rotation,
            p.south_pole_lat,
            p.south_pole_lon,
        );
        assert!(near(lat, 30.0, 1e-6), "first-point lat = {lat}");
        assert!(near(lon, 180.0, 1e-6), "first-point lon = {lon}");
        // Last grid point: rotated (0, 30) → geographic (60, 90).
        let (lat, lon) = unrotate_latlon(
            p.lat_last,
            p.lon_last,
            p.angle_of_rotation,
            p.south_pole_lat,
            p.south_pole_lon,
        );
        assert!(near(lat, 60.0, 1e-6), "last-point lat = {lat}");
        assert!(near(lon, 90.0, 1e-6), "last-point lon = {lon}");
        // An interior first-row point: rotated (60, 2) → geographic
        // (29.980, 178.846) per the oracle (printed to 3 decimals).
        let (lat, lon) = unrotate_latlon(60.0, 2.0, 0.0, 0.0, 0.0);
        assert!(near(lat, 29.980, 2e-3), "interior lat = {lat}");
        assert!(near(lon, 178.846, 2e-3), "interior lon = {lon}");
    }

    #[test]
    fn rotate_is_inverse_of_unrotate() {
        // A non-trivial pole so every matrix term is exercised, plus a rotation
        // angle to cover the longitude shift.
        let (sp_lat, sp_lon, angle) = (-36.0, 18.0, 12.0);
        for &(rlat, rlon) in &[(45.0, 10.0), (-20.0, -75.0), (5.0, 140.0)] {
            let (lat, lon) = unrotate_latlon(rlat, rlon, angle, sp_lat, sp_lon);
            let (back_lat, back_lon) = rotate_latlon(lat, lon, angle, sp_lat, sp_lon);
            assert!(near(back_lat, rlat, 1e-9), "rlat {rlat} -> {back_lat}");
            // Compare longitudes modulo 360 to ignore wrap.
            let dlon = ((back_lon - rlon + 180.0).rem_euclid(360.0)) - 180.0;
            assert!(near(dlon, 0.0, 1e-9), "rlon {rlon} -> {back_lon}");
        }
    }

    #[test]
    fn rotated_inverse_maps_corners_to_grid_extent() {
        let p = rotated_fixture_params();
        let proj = RotatedLatLonProjector::new(p);
        // Geographic first corner (30, 180) → index (0, 0).
        let first = proj.inverse(30.0, 180.0).expect("first corner");
        assert!(near(first.i, 0.0, 1e-6) && near(first.j, 0.0, 1e-6));
        // Geographic last corner (60, 90) → index (ni-1, nj-1).
        let last = proj.inverse(60.0, 90.0).expect("last corner");
        assert!(near(last.i, p.ni as f64 - 1.0, 1e-6), "i = {}", last.i);
        assert!(near(last.j, p.nj as f64 - 1.0, 1e-6), "j = {}", last.j);
    }

    #[test]
    fn rotated_inverse_rejects_off_grid_and_nonfinite() {
        let proj = RotatedLatLonProjector::new(rotated_fixture_params());
        // Geographic (0, 0) rotates to the antipodal side of the grid.
        assert!(proj.inverse(0.0, 0.0).is_none(), "off-grid point");
        assert!(proj.inverse(f64::NAN, 180.0).is_none(), "NaN lat");
        assert!(proj.inverse(30.0, f64::INFINITY).is_none(), "inf lon");
    }

    #[test]
    fn rotated_bbox_covers_corner_latitudes() {
        // The geographic corner latitudes (30 and 60) must lie within the
        // reported box, and the box must not collapse.
        let (lat_min, lat_max, lon_min, lon_max) =
            RotatedLatLonProjector::new(rotated_fixture_params()).lonlat_bbox();
        assert!(
            lat_min <= 30.0 + 1e-6 && lat_max >= 60.0 - 1e-6,
            "lat box too tight"
        );
        assert!(lon_max > lon_min, "degenerate lon span");
    }

    #[test]
    fn rotated_bbox_handles_antimeridian_crossing_rotated_columns() {
        // Real ECCC HRDPS continental grid: its rotated columns run
        // 345.18° → 42.31°, across the rotated antimeridian. Interpolating the
        // row edges over the raw corner difference swept the ~303° complement
        // arc — rotated longitudes that aren't in the grid — inflating the box
        // to nearly the whole globe, so the equirectangular render split the
        // field across the window's left and right edges.
        let p = RotatedLatLonParams {
            ni: 2540,
            nj: 1290,
            lat_first: -12.302501,
            lon_first: 345.17878,
            lat_last: 16.700001,
            lon_last: 42.306283,
            south_pole_lat: -36.08852,
            south_pole_lon: 245.305142,
            angle_of_rotation: 0.0,
        };
        let (lat_min, lat_max, lon_min, lon_max) = RotatedLatLonProjector::new(p).lonlat_bbox();
        // The HRDPS continental domain covers North America — roughly
        // 27°N..71°N, 153°W..41°W, a ~112° longitude window.
        assert!(near(lat_min, 27.28, 0.05), "lat_min = {lat_min}");
        assert!(near(lat_max, 70.61, 0.05), "lat_max = {lat_max}");
        assert!(near(lon_min, -152.73, 0.05), "lon_min = {lon_min}");
        assert!(near(lon_max, -40.71, 0.05), "lon_max = {lon_max}");
    }

    #[test]
    fn lonlat_bbox_unwraps_antimeridian_crossing_grid() {
        // The real CMC fixture (lov=249) has its `+y` corner at +177.2° while
        // the other three are negative — the grid straddles the dateline.
        // Naive min/max would give a ~312°-wide box; unwrapping must yield a
        // tight, continuous span instead.
        let proj = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 135,
            nj: 95,
            lat_first: 27.203,
            lon_first: -135.213,
            lov: 249.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        });
        let (lat_min, lat_max, lon_min, lon_max) = proj.lonlat_bbox();
        assert!(near(lat_min, 19.945, 1e-2), "lat_min {lat_min}");
        // The top edge bows toward the pole and reaches ~80.6°N — far above
        // the highest corner (60.5°N). Perimeter sampling must catch this.
        assert!(near(lat_max, 80.593, 5e-2), "lat_max {lat_max}");
        // +177.2° unwraps to ≈ -182.8°, giving a continuous ~151° span rather
        // than the spurious 312° box.
        assert!(near(lon_min, -182.805, 1e-2), "lon_min {lon_min}");
        assert!(near(lon_max, -31.933, 1e-2), "lon_max {lon_max}");
        assert!(lon_max - lon_min < 180.0, "span should be tight");
    }

    #[test]
    fn lonlat_bbox_lat_max_comes_from_edge_not_corner() {
        // Regression guard for the four-corner underestimate: the CMC grid's
        // corners top out at 60.5°N, but the boundary reaches ~80.6°N. A
        // corner-only box would report the former.
        let proj = PolarStereoProjector::new(PolarStereoParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 135,
            nj: 95,
            lat_first: 27.203,
            lon_first: -135.213,
            lov: 249.0,
            lad: 60.0,
            dx_metres: 60_000.0,
            dy_metres: 60_000.0,
            south_pole: false,
        });
        let corner_lat_max = proj
            .grid_corners_lonlat()
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let (_, lat_max, ..) = proj.lonlat_bbox();
        assert!(
            near(corner_lat_max, 60.476, 1e-2),
            "corner cap {corner_lat_max}"
        );
        assert!(
            lat_max > corner_lat_max + 15.0,
            "perimeter lat_max ({lat_max}) must clear the corner cap ({corner_lat_max})"
        );
    }

    #[test]
    fn lonlat_bbox_non_crossing_grid_encloses_corners() {
        // CONUS Lambert grid: all corners well clear of the dateline, so the
        // longitude unwrap is a no-op. The box must enclose every corner — and,
        // because edges bow, may extend beyond them in latitude (this grid's
        // boundary reaches ~83°N, above any corner).
        let proj = LambertProjector::new(LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 601,
            nj: 401,
            lat_first: 38.5,
            lon_first: -126.0,
            lad: 38.5,
            lov: -95.0,
            dx_metres: 13_545.0,
            dy_metres: 13_545.0,
            latin1: 38.5,
            latin2: 38.5,
        });
        let corners = proj.grid_corners_lonlat();
        let (lat_min, lat_max, lon_min, lon_max) = proj.lonlat_bbox();
        for (lat, lon) in corners {
            assert!(
                lat_min - 1e-6 <= lat && lat <= lat_max + 1e-6,
                "lat {lat} outside box"
            );
            assert!(
                lon_min - 1e-6 <= lon && lon <= lon_max + 1e-6,
                "lon {lon} outside box"
            );
        }
        // Edge bow lifts lat_max above the top corners.
        let corner_lat_max = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(lat_max > corner_lat_max, "edge should bow above corner lat");
    }

    #[test]
    fn lonlat_bbox_resolves_spans_wider_than_180_degrees() {
        // A synthetic projector whose perimeter sweeps 270° of longitude at a
        // constant latitude — wider than a single-reference unwrap can resolve.
        // The old code mis-bounded this (reporting a near-360° span); the
        // minimum-enclosing-arc must return the true ~270° window.
        struct WideMock;
        impl PlanarGridProjector for WideMock {
            fn grid_origin(&self) -> (f64, f64) {
                (0.0, 0.0)
            }
            fn grid_dims(&self) -> (u32, u32) {
                (271, 1)
            }
            fn grid_spacing(&self) -> (f64, f64) {
                (1.0, 1.0)
            }
            // Treat the plane x-coordinate directly as longitude (0..=270).
            fn inverse_lonlat(&self, x: f64, _y: f64) -> (f64, f64) {
                (12.0, x)
            }
            fn forward_xy(&self, _lat: f64, lon: f64) -> (f64, f64) {
                (lon, 0.0)
            }
        }

        let (lat_min, lat_max, lon_min, lon_max) = WideMock.lonlat_bbox();
        assert!((lat_min - 12.0).abs() < 1e-9 && (lat_max - 12.0).abs() < 1e-9);
        let span = lon_max - lon_min;
        assert!(
            (span - 270.0).abs() < 1.0,
            "expected a tight ~270° span, got {span} ([{lon_min}, {lon_max}])"
        );
    }
}
