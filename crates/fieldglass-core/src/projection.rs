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
    GaussianParams, GaussianProjector, expand_reduced_to_regular, gaussian_latitudes,
    is_octahedral_pl, reduced_raster_lon_last, reduced_raster_width,
};
pub use geostationary::{GeostationaryParams, GeostationaryProjector};
pub use lambert::{LambertParams, LambertProjector};
pub use lambert_azimuthal::{LambertAzimuthalParams, LambertAzimuthalProjector};
pub use latlon::{
    LatLonParams, eastward_lon_span, latlon_inverse, latlon_point, lon_grid_is_global,
};
pub use mercator::{MercatorParams, mercator_inverse, mercator_point};
pub use polar_stereo::{PolarStereoParams, PolarStereoProjector};
pub use rotated_latlon::{RotatedLatLonParams, RotatedLatLonProjector, rotated_latlon_point};
pub use transverse_mercator::{TransverseMercatorParams, TransverseMercatorProjector};

// Crate-internal: `GridGeometry` dispatches Gaussian queries through the
// recompute-per-call form rather than holding a projector. Not part of the
// crate's public contract — a caller reaches the same maths through
// [`GaussianProjector`]. The rotation pair (`rotate_latlon` / `unrotate_latlon`)
// is `pub(crate)` in its own module and named from there, as is each family's
// `*Constants`: a projector derives its own and no signature names one.
pub(crate) use gaussian::gaussian_inverse;

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
    /// Fractional column. `0.0` is the first column's centre.
    pub i: f64,
    /// Fractional row. `0.0` is the first row's centre.
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

/// What resampling a grid's geometry can support.
///
/// Lives here rather than beside `warp::Resampling` because [`GridGeometry`]
/// reports it and must stay outside the `render` feature — the format crates
/// take `core` with `default-features = false`. The rule it stands for is
/// applied on the render side, by `warp::Resampling::from_grid`. Both are
/// named in prose rather than linked for that reason: an intra-doc link from
/// here into `warp` does not resolve in the build this type exists to serve.
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

/// How far past a grid edge a computed index may sit and still be snapped
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

/// The shared body of every family whose plane is projected metres — Lambert,
/// polar stereographic, transverse Mercator, Lambert azimuthal.
///
/// An implementor supplies the four primitives below; the grid-index, corner
/// and bounding-box machinery is provided here, so a new planar family gets
/// them without restating the walk. The geographic families and the space view
/// do not implement it: their axes are degrees and scan angles respectively.
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
    /// within reach could reject the region that would hit it.
    ///
    /// Every implementor is currently the first kind, and each answers with its
    /// own `is_well_defined` rather than restating the rule — polar
    /// stereographic joined them in #603. No planar family needs the second: a
    /// point the forward map cannot reach either goes non-finite, which
    /// [`Self::inverse`] rejects on its own, or lands so far out that the
    /// extent check drops it (polar stereographic's opposite hemisphere is the
    /// case, where the forward `tan` grows to ~1e23 metres without ever
    /// reaching `inf`).
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

    /// Axis-aligned lat/lon bounding box of the grid.
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
    ///
    /// A grid with no projectable perimeter point at all reports the **empty
    /// box** at null island. Callers that must tell that apart from a real box
    /// there — [`GridGeometry::lonlat_bbox`] does, because the empty box frames
    /// a render on nothing — ask [`Self::placed_lonlat_bbox`] instead.
    fn lonlat_bbox(&self) -> LonLatBox {
        self.placed_lonlat_bbox()
            .unwrap_or(LonLatBox::new(0.0, 0.0, 0.0, 0.0))
    }

    /// [`Self::lonlat_bbox`], but `None` rather than the empty box when the
    /// projection can place no part of the grid.
    ///
    /// The perimeter walk skips samples that are not points on Earth, so a grid
    /// lying wholly outside the projection's reach leaves it with nothing to
    /// bound. `lonlat_bbox` reports `(0, 0, 0, 0)` there, which is
    /// indistinguishable from a real degenerate box at null island and which
    /// [`GridGeometry`] used to hand to a warp as its default window — a
    /// zero-extent render at 0°N 0°E instead of "this grid cannot be placed".
    fn placed_lonlat_bbox(&self) -> Option<LonLatBox> {
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
        // the map. Decline rather than report `±INFINITY`, which would
        // propagate into the target raster's size.
        if lons.is_empty() {
            return None;
        }

        // The longitude extent is the minimum enclosing arc of the perimeter
        // samples; see [`enclosing_lon_arc`].
        let (lon_min, lon_max) = enclosing_lon_arc(&mut lons);
        Some(LonLatBox::new(lat_min, lat_max, lon_min, lon_max))
    }
}

/// Whether a plane of radius `plane_radius_m` is wider than one cell of a grid
/// spaced `dx_metres` × `dy_metres`. The floor every planar family's
/// `is_well_defined` puts under the size of its own plane (#610).
///
/// All four of them place a raster measured in **metres** on a plane whose
/// entire content is a small multiple of that radius, and GRIB2 §3 lets a
/// message state the radius itself: shape-of-earth code 1 is "spherical, radius
/// specified by the data producer" as a scale factor and a scaled value, so
/// `scale = 6, value = 1` is a legal encoding of 1e-6 m. Every point on Earth
/// then projects to within a micrometre of the grid origin while the message
/// still declares a 12 km step, `(x − ox) / dx` is zero for all of them, and a
/// render samples cell (0, 0) for every pixel — a flat wash of one colour with
/// nothing to say it is wrong. The `> 0.0` radius checks the constants already
/// make (#603) all pass: 1e-6 is a perfectly good positive number.
///
/// **Pass the plane's radius, not the declared Earth's.** They differ, and a
/// message states both halves. `2·R·k₀` is the polar stereographic plane, and
/// `LaD = 260°` puts `k₀` at 0.0076 with `R` perfectly ordinary; `k · rectifying
/// radius` is the transverse Mercator plane, and `k` reaches the projector as an
/// unguarded IEEE `f32`. Either collapses the plane on a healthy Earth, so a
/// caller that hands over `R` alone has fixed half the defect. The two conics
/// have no such second factor: Lambert is conformal with unit scale at its
/// standard parallels, and the Lambert azimuthal authalic constants stay within
/// a factor of √2 of `a`, so the declared axis *is* their plane radius.
///
/// **The floor is the grid's own step, not a number about planets.** The plane
/// holds roughly `R / |dx|` distinguishable columns, so `R / |dx| < 1` is
/// "the whole plane is inside one cell" — the failure above. It is a loose
/// floor, deliberately: the true column count is a few times that (a conic's
/// image is ~2πR wide, and polar stereographic and transverse Mercator are
/// unbounded at their singularities), and a grid admitted at `R / |dx| = 1.2`
/// still renders an almost-flat wash. Tightening it would mean choosing a
/// multiple, and the multiple differs per family; erring loose costs a bad
/// picture in a case no producer publishes, where erring tight costs a refused
/// real grid. It scales with the message, so a regional model on Mars, on the
/// Moon, or on a 500 km body keeps working, where an absolute metre threshold
/// would have to guess which planets are allowed. It is also the same kind of
/// statement as the `ox + dx != ox` guard these families already apply to a
/// grid origin: a relation between the raster and its plane rather than a
/// constant.
///
/// Two alternatives were tried against the real arithmetic first. A fixed metre
/// threshold is arbitrary and has to admit every radius a producer might state.
/// Comparing the grid's projected *span* against its origin does not fire at
/// all: on a 1e-6 m sphere the origin is ~1e-7 m and the span still ~1e6 m, so
/// the span is the one number that still looks healthy.
///
/// A grid with **no** spacing passes vacuously, and deliberately: a zero step
/// is a different defect with its own refusals — that same `ox + dx != ox`
/// origin guard, and the explicit `dx == 0.0` in
/// [`PlanarGridProjector::inverse`] — and folding it in here would put one rule
/// in two places. The committed `polar_stereographic_surface.grib2` fixture is
/// such a grid, so this is a case that occurs, not a hypothetical.
///
/// A non-positive or `NaN` radius fails both comparisons on its own, so this
/// needs no separate positivity clause. The per-family constants keep the
/// `is_finite` and `> 0.0` tests they already have, which catch a plane radius
/// of exactly zero or one that has gone non-finite — the two cases this floor
/// does not distinguish from any other refusal, and the ones an infinite
/// declared radius reaches.
pub fn plane_spans_a_grid_cell(plane_radius_m: f64, dx_metres: f64, dy_metres: f64) -> bool {
    dx_metres.abs() < plane_radius_m && dy_metres.abs() < plane_radius_m
}

/// The floor [`GridGeometry::reprojectable`] puts under a metre-plane family,
/// measured on the radius the **message declared** rather than on the plane the
/// projection derives from it.
///
/// Three conditions the projectors' own `is_well_defined` does not all cover:
/// the declared radius has to describe a sphere at all, the grid has to have a
/// step to walk, and that step has to fit inside the declared radius. The last
/// is the one that differs — [`plane_spans_a_grid_cell`] is applied inside
/// `is_well_defined` too, but against `2·R·k₀` for polar stereographic and
/// `|k|·rectifying_radius` for transverse Mercator, either of which can be the
/// larger or the smaller number. Both are asked, so a grid stating a cell
/// between them is refused rather than offered and then refused again.
///
/// The zero-step clause is here rather than left to [`plane_spans_a_grid_cell`],
/// which passes a spacing-free grid vacuously by design: a zero step is a
/// different defect, and the committed `polar_stereographic_surface.grib2`
/// fixture is one.
fn declared_plane_carries_the_grid(radius_m: f64, dx_metres: f64, dy_metres: f64) -> bool {
    radius_m.is_finite()
        && radius_m > 0.0
        && dx_metres != 0.0
        && dy_metres != 0.0
        && plane_spans_a_grid_cell(radius_m, dx_metres, dy_metres)
}

/// Whether a planar grid can be placed on the Earth at all — the one predicate
/// [`GridGeometry::forward`], [`GridGeometry::lonlat_bbox`] and
/// [`GridGeometry::plane_affine`] gate on, so that none of the three can answer
/// for a grid whose [`GridGeometry::inverse`] declines every point of it.
///
/// Two conditions, and a message can fail either one on its own.
///
/// `projection_resolves` is the projector's own constants check. It is passed in
/// rather than read off the trait because it is an inherent `is_well_defined` on
/// each of the four planar projectors rather than a trait method — the same
/// shape the GRIB readers' `finite_lonlat` helpers use. `false` means
/// [`PlanarGridProjector::accepts`] rejects every point. The arithmetic does not
/// necessarily go non-finite there: a Lambert cone on a declared radius of zero
/// reports every grid point at the south pole, which reads downstream as a
/// coordinate rather than as a broken grid. [`plane_spans_a_grid_cell`] rides in
/// through this argument too, which is why an Earth small enough to fit inside
/// one grid cell is refused here without a second condition of its own.
///
/// The second is that the *raster* has a resolvable position in the plane the
/// projection defines. A grid may state a first point the forward map sends to
/// infinity — a §3.30 corner at the pole opposite its cone — or merely so far
/// out that one grid step no longer changes it: a §3.20 corner at the far pole
/// puts the origin at ~1.9e23 m, where adding a 60 km step is a no-op in `f64`
/// and every grid point collapses onto one position. `inverse` divides by that
/// step and refuses both, so these answers must too. What this deliberately does
/// **not** check is `ni`/`nj` — see [`GridGeometry::forward`], which still names
/// the point of a raster too thin for `inverse` to interpolate across.
fn planar_grid_is_placeable(projection_resolves: bool, proj: &dyn PlanarGridProjector) -> bool {
    let (ox, oy) = proj.grid_origin();
    let (dx, dy) = proj.grid_spacing();
    projection_resolves
        && ox.is_finite()
        && oy.is_finite()
        && dx.is_finite()
        && dy.is_finite()
        && ox + dx != ox
        && oy + dy != oy
}

/// A planar family's grid point `(i, j)`, or `None` where the projection has no
/// answer for it.
///
/// Beyond [`planar_grid_is_placeable`], which is about the grid, this rejects the
/// individual point the projection cannot reach: a well-defined projection still
/// has places it cannot invert — beyond a Lambert azimuthal equal-area disc,
/// `grid_point_lonlat` comes back `NaN` — and a `NaN` leaving here serialises to
/// JSON `null` or silently poisons an exporter's coordinates. Geostationary is
/// the model: a pixel whose line of sight misses the Earth is `None`.
fn placed_point(
    projection_resolves: bool,
    proj: &dyn PlanarGridProjector,
    i: u32,
    j: u32,
) -> Option<(f64, f64)> {
    if !planar_grid_is_placeable(projection_resolves, proj) {
        return None;
    }
    let (lat, lon) = proj.grid_point_lonlat(i, j);
    (lat.is_finite() && lon.is_finite()).then_some((lat, lon))
}

/// A planar family's lat/lon box, or `None` when the grid cannot be placed or no
/// part of its perimeter projects. The [`placed_point`] gate, for the box.
fn placed_bbox(projection_resolves: bool, proj: &dyn PlanarGridProjector) -> Option<LonLatBox> {
    planar_grid_is_placeable(projection_resolves, proj)
        .then(|| proj.placed_lonlat_bbox())
        .flatten()
}

/// A planar family's position in the plane its CRS names, or `None` when the
/// grid cannot be placed. The [`placed_point`] gate, for the affine.
///
/// The origin and spacing are the projector's own, and the gate has already
/// established that all four are finite — so unlike the Mercator arm, which
/// computes its ordinate here, this cannot emit an infinite origin or a `NaN`
/// step.
fn placed_affine(projection_resolves: bool, proj: &dyn PlanarGridProjector) -> Option<PlaneAffine> {
    if !planar_grid_is_placeable(projection_resolves, proj) {
        return None;
    }
    let (x0, y0) = proj.grid_origin();
    let (dx, dy) = proj.grid_spacing();
    Some(PlaneAffine {
        x0,
        y0,
        dx: Some(dx),
        dy: Some(dy),
        units: PlaneUnits::Metres,
    })
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
///
/// # Panics
///
/// On an empty slice — there is no arc to enclose, and both bounds index the
/// samples directly. Every caller filters its perimeter walk down to the
/// projectable samples and returns early when none survive, which is the check
/// that keeps this unreachable.
pub(crate) fn enclosing_lon_arc(lons: &mut [f64]) -> (f64, f64) {
    debug_assert!(!lons.is_empty(), "an arc needs at least one sample");
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
    /// Evenly spaced in degrees on both axes.
    #[serde(rename = "latlon")]
    LatLon(LatLonParams),
    /// Evenly spaced in longitude, with rows on the Gauss–Legendre nodes.
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
    /// Lambert conformal conic, on a sphere the message declares.
    #[serde(rename = "lambert")]
    Lambert(LambertParams),
    /// Polar stereographic, on a sphere the message declares.
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
    Unsupported {
        /// The grid type as the decoder named it.
        label: String,
    },
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
            // The planar families answer through one guard, so that a grid
            // whose `inverse` declines every point is declined here too and a
            // point beyond the projection's reach is space rather than `NaN`;
            // see [`placed_point`]. All four pass their own constants check:
            // polar stereographic used to pass `true` on the grounds that its
            // scale factor `(1 + sin|LaD|)/2` is one no declarable `LaD` drives
            // to zero. That is silent about the radius that multiplies it, and
            // wrong about `LaD` besides — §3.20 can state ±270°, where the
            // factor is exactly zero (#603).
            Self::Lambert(p) => {
                let proj = LambertProjector::new(*p);
                placed_point(proj.is_well_defined(), &proj, i, j)
            }
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                placed_point(proj.is_well_defined(), &proj, i, j)
            }
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                placed_point(proj.is_well_defined(), &proj, i, j)
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                placed_point(proj.is_well_defined(), &proj, i, j)
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

    /// The grid's geographic extent in degrees, or `None` for a family that
    /// cannot be placed.
    ///
    /// The projected families delegate to
    /// [`PlanarGridProjector::placed_lonlat_bbox`] — the `Option`-returning
    /// form, because the empty box its total sibling reports for a grid that
    /// projects nowhere would frame a render on null island. It subdivides each
    /// edge 512 times rather than walking grid points: a conic's edges are
    /// curves and the extreme latitude sits between two points, not on one. It
    /// also skips perimeter samples that are not on the Earth at all, which an
    /// oversized §3.140 grid produces, and widens to the full 360° when the
    /// domain surrounds the projection pole and the enclosing longitude arc
    /// therefore degenerates.
    ///
    /// The antimeridian convention on [`LonLatBox`] applies: `lon_min` may
    /// fall below -180 (or `lon_max` above 180) to describe a window spanning
    /// it, and normalising either into range collapses the span.
    pub fn lonlat_bbox(&self) -> Option<LonLatBox> {
        match self {
            // The geographic families state their own corners, so the box is
            // the corners — unwrapped through `eastward_lon_span` so a grid
            // published from 180°E keeps its span instead of collapsing to one
            // cell.
            Self::LatLon(p) => Some(LonLatBox::from_corners(CornerPair::new(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            ))),
            Self::Gaussian(p) => Some(LonLatBox::from_corners(CornerPair::new(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            ))),
            // Rows crowd toward the equator, but the extreme latitudes are
            // still the two stated corners, so the box is the corners here too.
            Self::Mercator(p) => Some(LonLatBox::from_corners(CornerPair::new(
                p.lat_first,
                p.lon_first,
                p.lat_last,
                p.lon_last,
            ))),
            // The corners are rotated-frame degrees, so — unlike every other
            // geographic family — they are not the geographic extent. The
            // projector walks the rotated perimeter and unrotates it.
            Self::RotatedLatLon(p) => Some(RotatedLatLonProjector::new(*p).lonlat_bbox()),
            // The extent of its own centres (#445), widened to the full circle
            // when they surround a pole and the enclosing arc stops meaning
            // anything — the same degeneracy `PolarStereo` handles below.
            Self::Lookup(ix) => ix.lonlat_bbox(),
            // Gated on the cone the same way `forward` is, and asked for the
            // box only of what could be placed: a cone with no usable standard
            // parallels, or on a declared radius of zero, otherwise reports a
            // box on the south pole for a grid whose `inverse` declines every
            // point of it.
            Self::Lambert(p) => {
                let proj = LambertProjector::new(*p);
                placed_bbox(proj.is_well_defined(), &proj)
            }
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                let box_ = placed_bbox(proj.is_well_defined(), &proj)?;
                if proj.pole_inside_grid() {
                    // Every meridian is present and the enclosing arc has no
                    // empty gap to be the complement of, so the walk's
                    // longitudes mean nothing here.
                    let (lat_min, lat_max) = if p.south_pole {
                        (-90.0, box_.lat_max)
                    } else {
                        (box_.lat_min, 90.0)
                    };
                    Some(LonLatBox::new(lat_min, lat_max, -180.0, 180.0))
                } else {
                    Some(box_)
                }
            }
            // Gated on the spheroid the same way `forward` is: a box walked
            // with degenerate constants is finite and meaningless, which frames
            // a render on nothing.
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                placed_bbox(proj.is_well_defined(), &proj)
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                placed_bbox(proj.is_well_defined(), &proj)
            }
            // The on-disk extent, so a cropped sector (GOES CONUS or
            // mesoscale, a Meteosat sector) frames its sector rather than a
            // hemisphere. A full disc whose whole perimeter is limb has no
            // on-disk sample to walk; fall back there to the full latitude span
            // and a quarter-turn of longitude either side of the sub-satellite
            // point, which is what the napi warp has framed such a grid with
            // since §3.90 landed. Off-disk pixels invert to `None` regardless,
            // so the fallback affects framing and never correctness.
            //
            // The projector's `None` has a second cause the fallback must not
            // cover: a raster it cannot walk at all — a single column or row,
            // or a zero scan-angle step — which `inverse` also declines for
            // every point. Framing a hemisphere for one of those describes a
            // grid the message does not contain.
            Self::Geostationary(p) => {
                let proj = GeostationaryProjector::new(*p);
                // The fallback describes the disc a satellite over `sub_lon_deg`
                // would see, so it is only available when there is a body to
                // see: a shapeless or unseeable ellipsoid has no hemisphere to
                // frame either, and `inverse` declines every pixel of it (#610).
                (proj.raster_is_walkable() && proj.is_well_defined()).then(|| {
                    proj.lonlat_bbox().unwrap_or(LonLatBox::new(
                        -90.0,
                        90.0,
                        p.sub_lon_deg - 90.0,
                        p.sub_lon_deg + 90.0,
                    ))
                })
            }
            Self::Unsupported { .. } => None,
        }
    }

    /// Whether the grid's column axis closes on itself: one column step past
    /// the last column lands back on the first.
    ///
    /// Only a geographic family can. A projected grid's columns are
    /// projection-plane metres and no finite number of them wraps the Earth; a
    /// lookup grid is a list of centres with no column axis to close. Judged
    /// from the stated corners and the column count, by [`lon_grid_is_global`],
    /// which admits a grid whose last column stops one step short of the seam —
    /// the ordinary case, since repeating the seam column would store it twice.
    ///
    /// [`RotatedLatLon`](Self::RotatedLatLon) is judged in its **rotated**
    /// frame, because that is the frame its corners and its inverse map are both
    /// stated in and therefore the frame a column index actually wraps in. A
    /// rotated grid that closes on itself there closes on itself on the sphere
    /// too.
    ///
    /// This is the one predicate for the question. Both hosts asked it
    /// separately before #571 and disagreed about rotated lat/lon: the umbrella
    /// widened such a grid's render window and the napi warp did not.
    pub fn is_periodic_x(&self) -> bool {
        // A reduced grid never reaches here as itself: both GRIB crates widen
        // its rows to a regular `ni` raster in the conversion, so it arrives as
        // `LatLon` or `Gaussian` with the raster's own east edge (#503, #543).
        let corners = match self {
            Self::LatLon(p) => (p.lon_first, p.lon_last, p.ni),
            Self::Gaussian(p) => (p.lon_first, p.lon_last, p.ni),
            // Evenly spaced in longitude like the two above, so the same corner
            // test decides it; only the row spacing differs.
            Self::Mercator(p) => (p.lon_first, p.lon_last, p.ni),
            Self::RotatedLatLon(p) => (p.lon_first, p.lon_last, p.ni),
            _ => return false,
        };
        lon_grid_is_global(eastward_lon_span(corners.0, corners.1), corners.2)
    }

    /// The window a render should frame, as opposed to
    /// [`lonlat_bbox`](Self::lonlat_bbox), which is where the data is.
    ///
    /// A grid whose columns close on themselves owns the gap between its last
    /// column and its first: the periodic sampler fills it, and a window that
    /// stops at the last declared column leaves the seam meridian as a stripe of
    /// background one cell wide. So such a grid's window is its extent carried a
    /// full turn east ([`LonLatBox::widened_to_full_turn`]), and everything
    /// else's window is its extent unchanged.
    ///
    /// Stating it here is the point: #553 gave the widening one home, and this
    /// gives *when to apply it* one home too. Before #571 the umbrella and the
    /// napi warp each decided it, under predicates that disagreed.
    ///
    /// **[`is_periodic_x`](Self::is_periodic_x) alone is not the condition**,
    /// and rotated lat/lon is why. Its periodicity is judged in the rotated
    /// frame — the frame a column index actually wraps in — while its extent is
    /// the unrotated perimeter walk, so composing the two would add 360° of
    /// *geographic* longitude on the strength of a turn measured somewhere
    /// else. Measured on a rotated grid covering rotated latitudes 80–90° over a
    /// full rotated turn: a polar cap 23° of geographic longitude wide, which
    /// the widening frames as the whole globe. Its seam gap is real, but closing
    /// it means walking the perimeter of the *closed* rotated turn and
    /// unrotating that, which is a larger change than the gap warrants — the
    /// same conclusion [`contour_seam_wraps`](Self::contour_seam_wraps) reaches
    /// about the same family, for the same reason, through the same predicate.
    pub fn render_window(&self) -> Option<LonLatBox> {
        let extent = self.lonlat_bbox()?;
        Some(if self.columns_advance_eastward() && self.is_periodic_x() {
            extent.widened_to_full_turn()
        } else {
            extent
        })
    }

    /// Whether one step in `i` is one step east in *geographic* longitude, so a
    /// column index and the box [`lonlat_bbox`](Self::lonlat_bbox) reports are
    /// measured in the same frame.
    ///
    /// True for the corner-pinned geographic families, whose stated corners are
    /// geographic and whose longitude is `lon_first + i · step`. False for
    /// [`RotatedLatLon`](Self::RotatedLatLon), whose corners are rotated-frame
    /// degrees and whose rows are small circles — geographic longitude along one
    /// is neither uniform nor monotonic. False for the projected families, whose
    /// columns are plane metres, and for a lookup grid, which has no column
    /// formula at all.
    ///
    /// Two questions turn on this, and either would otherwise compose an answer
    /// from one frame with an answer from another: whether the contour tracer
    /// may unwrap the seam eastward, and whether the render window may be
    /// carried a full turn east.
    ///
    /// The reduced families arrive widened to their regular sibling, so they
    /// reach this as `LatLon` or `Gaussian`; leaving them out gave the same grid
    /// a seam gap in GRIB1 and none in GRIB2 (#503).
    fn columns_advance_eastward(&self) -> bool {
        matches!(
            self,
            Self::LatLon(_) | Self::Gaussian(_) | Self::Mercator(_)
        )
    }

    /// Whether the contour pass may march the seam cell that wraps column
    /// `ni - 1` round to column `0`.
    ///
    /// Two conditions, and both matter:
    ///
    /// 1. The grid is periodic — [`is_periodic_x`](Self::is_periodic_x), the
    ///    same answer the warp and the probe use (#332), read off the stated
    ///    corners, which for a rotated grid are *rotated* coordinates: the space
    ///    the index actually wraps in.
    /// 2. The family's geographic longitude advances uniformly eastward with
    ///    `i`. The seam interpolation runs in geographic longitude — a contour
    ///    vertex never sees any other coordinate — so it is valid only where one
    ///    step in `i` is one step east.
    ///
    /// A rotated grid that is global in rotated longitude therefore keeps its
    /// seam gap, which is why it fails the second condition and not the first.
    /// Its rows are small circles whose geographic longitude is neither uniform
    /// nor monotonic, so unwrapping such a seam eastward could sweep most of the
    /// way round the globe and draw a rim-to-rim streak in place of closing a
    /// one-cell gap. Closing it properly needs the seam interpolated in rotated
    /// space and rotated back, which is a larger change than the gap warrants.
    pub fn contour_seam_wraps(&self) -> bool {
        self.columns_advance_eastward() && self.is_periodic_x()
    }

    /// Whether a render may offer to reproject this grid onto a map — that is,
    /// whether [`inverse_at`](Self::inverse_at) will actually place points of
    /// it.
    ///
    /// **One predicate, and it is the one the render consults.** It used to be
    /// two, in the host: a grid-type allow-list that saw a family name and two
    /// spacings, and a second pass that re-asked the warp setup. A message can
    /// state every number a planar family needs, all present and finite, and
    /// still describe a projection that places no point — a cone whose standard
    /// parallels are both on the equator, an Earth smaller than one cell of the
    /// grid drawn on it, a latitude of true scale past ±90°, a geostationary
    /// ellipsoid the satellite has no line of sight to. Answering `true` there
    /// promises a host a target the warp then refuses (#603, #610).
    ///
    /// The corner-pinned families answer from `scan` alone. Their inverse maps
    /// assume columns run west to east, so an `i_negative` grid stays in its
    /// source projection; the row direction does not matter, because the map is
    /// built from the two stated corners whichever way they run.
    ///
    /// The projected families ignore `scan` entirely, because they have already
    /// absorbed it: [`signed_grid_increments`] baked the direction bits into
    /// their spacings before the variant was built. What they are asked instead
    /// is whether the plane is real *and the raster has a place in it*, through
    /// the same `planar_grid_is_placeable` that [`forward`](Self::forward),
    /// [`lonlat_bbox`](Self::lonlat_bbox) and
    /// [`plane_affine`](Self::plane_affine) gate on — so this cannot offer a
    /// grid whose other three answers are all `None`. A §3.20 corner at the far
    /// pole is such a grid: the origin lands at 1.9e23 m, where adding a 60 km
    /// step is a no-op in `f64`, and `inverse` then declines every pixel.
    ///
    /// The plane itself is measured twice, on purpose —
    /// against the **declared** radius, which is what a message states and what
    /// the render's own parameter builders measure, and against the family's own
    /// plane inside `is_well_defined`, which is a different number for three of
    /// the four. A polar stereographic plane is `2·R·k₀` (1.87·R at a ±60°
    /// latitude of true scale) and a transverse Mercator plane is
    /// `|k|·rectifying_radius`, so a grid stating a cell between the two would
    /// otherwise be offered here and refused by the render.
    ///
    /// [`Lookup`](Self::Lookup) is always reprojectable: its inverse is a
    /// nearest-cell search over centres that carry their own positions, so there
    /// is no scan direction to be wrong about and no projection to collapse
    /// (#445).
    pub fn reprojectable(&self, scan: Scan) -> bool {
        match self {
            // Reduced grids arrive widened to a regular raster, so they reach
            // this through the same two arms as their regular siblings.
            Self::LatLon(_) | Self::Gaussian(_) | Self::Mercator(_) | Self::RotatedLatLon(_) => {
                !scan.i_negative
            }
            Self::Lookup(_) => true,
            Self::Lambert(p) => {
                let proj = LambertProjector::new(*p);
                declared_plane_carries_the_grid(p.earth_radius_m, p.dx_metres, p.dy_metres)
                    && planar_grid_is_placeable(proj.is_well_defined(), &proj)
            }
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                declared_plane_carries_the_grid(p.earth_radius_m, p.dx_metres, p.dy_metres)
                    && planar_grid_is_placeable(proj.is_well_defined(), &proj)
            }
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                declared_plane_carries_the_grid(p.semi_major_m, p.dx_metres, p.dy_metres)
                    && planar_grid_is_placeable(proj.is_well_defined(), &proj)
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                declared_plane_carries_the_grid(p.semi_major_m, p.dx_metres, p.dy_metres)
                    && planar_grid_is_placeable(proj.is_well_defined(), &proj)
            }
            // No radius floor: a space view's axes are scan angles rather than
            // metres and every term of its maths is a ratio, so shrinking the
            // whole system leaves the picture where it was. Its own
            // `is_well_defined` catches a shapeless ellipsoid, and
            // `raster_is_walkable` catches the grid — the two conditions
            // `lonlat_bbox` and `inverse` gate on, asked here so all three
            // answer together. Spelling the raster half as `dx_rad != 0.0` would
            // miss a `NaN`, which *is* `!= 0.0`, and that is the silent NaN
            // index #603 was filed for.
            Self::Geostationary(p) => {
                let proj = GeostationaryProjector::new(*p);
                proj.raster_is_walkable() && proj.is_well_defined()
            }
            Self::Unsupported { .. } => false,
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
    /// spheroid that is not one, a §3.12 scale factor of zero, a Lambert cone
    /// with no usable standard parallels or on a declared radius of zero, a
    /// Mercator corner at a pole. It is the same predicate
    /// [`forward`](Self::forward) and [`lonlat_bbox`](Self::lonlat_bbox) use,
    /// so the three cannot disagree with [`inverse`](Self::inverse) about
    /// whether a grid can be placed. The implication
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
            // Gated on the same predicate as `forward` and `lonlat_bbox`: an
            // affine for a cone that resolves nowhere places the raster in a
            // plane no point of it can be read back out of, and an origin the
            // forward map sent to infinity reaches a host as JSON `null`.
            Self::Lambert(p) => {
                let proj = LambertProjector::new(*p);
                placed_affine(proj.is_well_defined(), &proj)
            }
            Self::PolarStereo(p) => {
                let proj = PolarStereoProjector::new(*p);
                placed_affine(proj.is_well_defined(), &proj)
            }
            Self::TransverseMercator(p) => {
                let proj = TransverseMercatorProjector::new(*p);
                placed_affine(proj.is_well_defined(), &proj)
            }
            Self::LambertAzimuthal(p) => {
                let proj = LambertAzimuthalProjector::new(*p);
                placed_affine(proj.is_well_defined(), &proj)
            }
            // Scan angles become metres on the same sight line `+h` measures
            // along, so the affine and the CRS agree by construction.
            //
            // A zero or non-finite scan-angle step is the space-view form of
            // the degeneracy `planar_grid_is_placeable` refuses: it puts every
            // pixel of the raster on one plane coordinate, and `inverse`
            // declines such a grid for exactly that reason.
            Self::Geostationary(p) => {
                let h = p.h_metres - p.r_eq;
                let (dx, dy) = (h * p.dx_rad, h * p.dy_rad);
                let (x0, y0) = (h * p.x0, h * p.y0);
                // And the ellipsoid itself has to describe a body the satellite
                // can see, the same gate the four planar arms above put on
                // `is_well_defined`. A prolate pair or a satellite inside the
                // Earth leaves `h` finite and non-zero, so the step test below
                // passes on a view whose every pixel `inverse` declines (#610).
                (GeostationaryProjector::new(*p).is_well_defined()
                    && x0.is_finite()
                    && y0.is_finite()
                    && dx.is_finite()
                    && dy.is_finite()
                    && x0 + dx != x0
                    && y0 + dy != y0)
                    .then_some(PlaneAffine {
                        x0,
                        y0,
                        dx: Some(dx),
                        dy: Some(dy),
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
    /// Plane coordinate of the first scanned point along the column axis.
    pub x0: f64,
    /// Plane coordinate of the first scanned point along the row axis.
    pub y0: f64,
    /// `None` for an axis with no constant step — a single-point axis, or a
    /// Gaussian grid's rows.
    pub dx: Option<f64>,
    /// Signed step between rows — see `dx`.
    pub dy: Option<f64>,
    /// What `x0` / `y0` / `dx` / `dy` are measured in.
    pub units: PlaneUnits,
}

/// The two corner grid points a message states: the first scanned point and
/// the last, in the file's own scan order.
///
/// This is *what the message says*, not an axis-aligned box: `lat_first` may be
/// north or south of `lat_last`, and `lon_last` may be numerically below
/// `lon_first` on a grid published from 180°E. For the extent the data covers,
/// convert with [`LonLatBox::from_corners`].
///
/// The named fields exist because the corner pair and [`LonLatBox`] were both
/// bare `(f64, f64, f64, f64)` in *different orders*, so passing one where the
/// other was wanted compiled silently and drew a plausible-looking wrong map
/// (#553). Substituting them is now a type error:
///
/// ```compile_fail
/// use fieldglass_core::{CornerPair, LonLatBox};
/// fn frame(_: LonLatBox) {}
/// frame(CornerPair::new(60.0, 0.0, -60.0, 350.0));
/// ```
///
/// The conversion is the only way across, and it is not the identity — the
/// grid above runs north-down, so the box reorders its latitudes:
///
/// ```
/// use fieldglass_core::{CornerPair, LonLatBox};
/// fn frame(_: LonLatBox) {}
/// let corners = CornerPair::new(60.0, 0.0, -60.0, 350.0);
/// assert_eq!(LonLatBox::from_corners(corners).lat_min, -60.0);
/// frame(LonLatBox::from_corners(corners));
/// ```
///
/// The second example is what keeps the first honest. `compile_fail` is
/// satisfied by *any* compilation failure — rustdoc does not enforce an
/// `E0308` annotation on stable — so on its own it would still pass if
/// `CornerPair::new` changed arity or either type were renamed. The passing
/// example names the same items, so that drift fails a test rather than
/// quietly weakening one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CornerPair {
    /// Latitude of the first scanned grid point.
    pub lat_first: f64,
    /// Longitude of the first scanned grid point.
    pub lon_first: f64,
    /// Latitude of the last scanned grid point — the corner diagonally
    /// opposite `lat_first`.
    pub lat_last: f64,
    /// Longitude of the last scanned grid point.
    pub lon_last: f64,
}

impl CornerPair {
    /// The pair, in the order a GRIB grid section states it: `La1`, `Lo1`,
    /// `La2`, `Lo2`.
    #[must_use]
    pub const fn new(lat_first: f64, lon_first: f64, lat_last: f64, lon_last: f64) -> Self {
        Self {
            lat_first,
            lon_first,
            lat_last,
            lon_last,
        }
    }
}

/// An axis-aligned lat/lon box in degrees: *where a grid's data is*.
///
/// `lon_min` may fall below -180 (or `lon_max` above 180) to describe a window
/// spanning the antimeridian — the workspace-wide convention, which the warp
/// consumes through periodic trig. Do not normalise either bound into range
/// without collapsing the span.
///
/// This is the min/max form. The corner form a message states is
/// [`CornerPair`], which orders its four numbers differently; see that type for
/// why both are named rather than bare tuples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LonLatBox {
    /// Southern edge.
    pub lat_min: f64,
    /// Northern edge.
    pub lat_max: f64,
    /// Western edge.
    pub lon_min: f64,
    /// Eastern edge. `>= lon_min` for a box from [`Self::from_corners`] or a
    /// projector's perimeter walk; [`Self::new`] validates nothing, so a box
    /// built from a caller's own numbers is only as ordered as they were.
    pub lon_max: f64,
}

impl LonLatBox {
    /// The box from its four edges.
    #[must_use]
    pub const fn new(lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) -> Self {
        Self {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        }
    }

    /// Box of a geographic grid from its two stated corners.
    ///
    /// The longitude runs `lon_first` eastward by [`eastward_lon_span`] rather
    /// than `min`/`max` of the two corners: a grid published from 180°E reports
    /// `lon_last` numerically below `lon_first`, and taking the extremes
    /// collapses the span to a single grid step. `lon_max` may therefore exceed
    /// 180, which is the convention [`GridGeometry::lonlat_bbox`] documents.
    #[must_use]
    pub fn from_corners(corners: CornerPair) -> Self {
        let CornerPair {
            lat_first,
            lon_first,
            lat_last,
            lon_last,
        } = corners;
        let span = eastward_lon_span(lon_first, lon_last);
        Self {
            lat_min: lat_first.min(lat_last),
            lat_max: lat_first.max(lat_last),
            lon_min: lon_first,
            lon_max: lon_first + span,
        }
    }

    /// The same western edge, carried a full turn east — *what window should a
    /// render frame*, as opposed to where the data is.
    ///
    /// A grid whose columns close on themselves owns the gap between its last
    /// column and its first: the periodic sampler fills it, and a window that
    /// stops at the last declared column leaves the seam meridian as a stripe
    /// of background one cell wide. Callers decide *whether* the grid is
    /// periodic — [`lon_grid_is_global`] for a corner-pinned family, the
    /// geometry's own answer elsewhere — and this states the widening once.
    #[must_use]
    pub fn widened_to_full_turn(self) -> Self {
        Self {
            lon_max: self.lon_min + 360.0,
            ..self
        }
    }

    /// The four edges as an array, for a plain-data boundary that carries them
    /// positionally (`[lat_min, lat_max, lon_min, lon_max]`).
    #[must_use]
    pub const fn to_array(self) -> [f64; 4] {
        [self.lat_min, self.lat_max, self.lon_min, self.lon_max]
    }

    /// Inverse of [`Self::to_array`].
    #[must_use]
    pub const fn from_array(edges: [f64; 4]) -> Self {
        Self::new(edges[0], edges[1], edges[2], edges[3])
    }
}

/// The direction a message walked its grid, travelling **beside** the geometry
/// rather than inside it (#571).
///
/// Beside, and not a field on each [`GridGeometry`] variant, for three reasons:
///
/// * The projected families already absorb the direction bits.
///   [`signed_grid_increments`] bakes the `i` and `j` signs into `dx_metres` /
///   `dy_metres` in both GRIB crates before a variant is built, so a flag on
///   [`LambertParams`] would state the same fact a second time and let the two
///   copies disagree.
/// * [`j_consecutive`](Self::j_consecutive) is not a property of the grid at
///   all. It says how the *message* stored its points, and both decoders
///   transpose such a field before anyone indexes it (`crate::scan`), so a
///   geometry carrying it would describe a layout that no longer exists.
/// * Only two questions need it, one flag each — reprojection eligibility reads
///   [`i_negative`](Self::i_negative), and which way up to draw the source view
///   reads [`j_positive`](Self::j_positive) — so a parameter is the honest
///   shape. [`GridGeometry::reprojectable`] and [`Self::flips_source_rows`] are
///   those two questions.
///
/// The flags describe the raster a decoder **returns**, not the raw GDS byte
/// (#541): a `j`-consecutive message is transposed on the way out, which is why
/// that flag is descriptive here and acting on it would transpose twice.
///
/// `#[non_exhaustive]`, because it surfaces on the `fieldglass` API as
/// `Georef::scan` and ADR-0006 requires it of every type there. Build one with
/// [`Self::new`] or [`Self::north_down`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Scan {
    /// Points run east→west rather than west→east (GRIB1 GDS octet 28 bit 1,
    /// GRIB2 §3 Flag Table 3.4 bit 1).
    pub i_negative: bool,
    /// Rows run south→north rather than north→south (bit 2).
    pub j_positive: bool,
    /// The *message* stored adjacent points consecutive in `j` (column-major,
    /// bit 3). Descriptive only — see the type docs.
    pub j_consecutive: bool,
}

impl Scan {
    /// The three flags, in the order the flag tables list them.
    #[must_use]
    pub const fn new(i_negative: bool, j_positive: bool, j_consecutive: bool) -> Self {
        Self {
            i_negative,
            j_positive,
            j_consecutive,
        }
    }

    /// West-to-east rows walked north to south, row-major: the orientation a
    /// grid with no scan flag at all is read as.
    ///
    /// A predefined GRIB1 grid carries no GDS to state one, and a NetCDF file
    /// has no scanning mode; both are treated as this rather than as unknown,
    /// which is what the hosts already do with a missing flag.
    #[must_use]
    pub const fn north_down() -> Self {
        Self::new(false, false, false)
    }

    /// Whether painting the source grid at one pixel per grid point has to flip
    /// the rows, given the flip the caller asked for.
    ///
    /// The source view paints grid point `(i, j)` at pixel `(i, j)`, so a
    /// [`j_positive`](Self::j_positive) grid — row 0 southernmost — arrives
    /// upside down on a canvas whose first row is the top. Flipping it is what
    /// makes north up (#286), and the caller's own request composes with it:
    /// asking for a flipped view of an already-flipped grid is the grid as
    /// stored.
    #[must_use]
    pub const fn flips_source_rows(self, requested_flip: bool) -> bool {
        requested_flip ^ self.j_positive
    }
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
    use rotated_latlon::{rotate_latlon, unrotate_latlon};

    /// The conversion the corner form and the box form differ by, on the two
    /// cases that motivated naming them (#553): the ordinary north-down grid,
    /// where the box swaps the latitudes the message states in scan order, and
    /// the grid published from 180°E, where taking `min`/`max` of the two
    /// longitudes would collapse a full turn to one cell.
    #[test]
    fn a_box_from_corners_orders_latitudes_and_runs_longitude_eastward() {
        // North-down, west-to-east: only the latitudes are reordered.
        assert_eq!(
            LonLatBox::from_corners(CornerPair::new(60.0, -10.0, 20.0, 30.0)),
            LonLatBox::new(20.0, 60.0, -10.0, 30.0)
        );
        // Published from 180°E and running east through the antimeridian.
        // `lon_min`/`lon_max` of the corners would report 170..180, a 10°
        // sliver, in place of the 350° the grid actually covers.
        let crossing = LonLatBox::from_corners(CornerPair::new(90.0, 180.0, -90.0, 170.0));
        assert_eq!(crossing.lon_min, 180.0);
        assert!(
            (crossing.lon_max - 530.0).abs() < 1e-9,
            "east edge {} should be 180 + 350",
            crossing.lon_max
        );
    }

    /// Widening answers "what window should a render frame", so it moves only
    /// the eastern edge and leaves where the data is alone. A grid already a
    /// full turn wide is unchanged by it, which is what makes it safe to apply
    /// on a periodic grid without asking whether the seam column is duplicated.
    #[test]
    fn widening_moves_only_the_eastern_edge() {
        let data = LonLatBox::new(-90.0, 90.0, 0.0, 357.5);
        let window = data.widened_to_full_turn();
        assert_eq!(window, LonLatBox::new(-90.0, 90.0, 0.0, 360.0));
        assert_eq!(window.widened_to_full_turn(), window, "idempotent");
        // A window that already spans the turn from a shifted origin keeps its
        // origin rather than being normalised into [-180, 180].
        let shifted = LonLatBox::new(0.0, 10.0, 180.0, 400.0).widened_to_full_turn();
        assert_eq!(shifted, LonLatBox::new(0.0, 10.0, 180.0, 540.0));
    }

    /// The array form is the plain-data boundary the API DTOs carry, so the
    /// order it writes and the order it reads must be the same one.
    #[test]
    fn the_array_form_round_trips() {
        let b = LonLatBox::new(-12.0, 34.0, 100.0, 220.0);
        assert_eq!(b.to_array(), [-12.0, 34.0, 100.0, 220.0]);
        assert_eq!(LonLatBox::from_array(b.to_array()), b);
    }

    /// The rule itself, at its boundary and on the radii that are not numbers
    /// describing a sphere. The doc claims the two comparisons subsume a
    /// positivity clause; this is where that claim is checked, because a later
    /// rewrite into `radius > dx.abs()` would quietly admit a `NaN` radius.
    #[test]
    fn a_plane_spans_a_cell_only_when_it_is_wider_than_one() {
        assert!(plane_spans_a_grid_cell(6_371_229.0, 12_000.0, -12_000.0));
        // The sign of the step is the grid's scan direction, not its size.
        assert!(plane_spans_a_grid_cell(20.0, -19.0, 19.0));
        // Exactly one cell across is still one cell: nothing is resolvable.
        assert!(!plane_spans_a_grid_cell(12_000.0, 12_000.0, 12_000.0));
        assert!(!plane_spans_a_grid_cell(11_999.0, 12_000.0, 12_000.0));
        // One axis is enough to collapse the raster.
        assert!(!plane_spans_a_grid_cell(20_000.0, 1_000.0, 30_000.0));
        // #610 itself: shape-of-earth 1, scale = 6, value = 1.
        assert!(!plane_spans_a_grid_cell(1e-6, 12_000.0, 12_000.0));
        // A grid with no spacing passes vacuously; the doc says why, and
        // `PlanarGridProjector::inverse` refuses it on its own account.
        assert!(plane_spans_a_grid_cell(1e-6, 0.0, 0.0));
        // No separate well-definedness clause needed — these fail the
        // comparisons on their own.
        for radius in [0.0, -6_371_229.0, f64::NAN] {
            assert!(
                !plane_spans_a_grid_cell(radius, 12_000.0, 12_000.0),
                "radius {radius} passed"
            );
        }
    }

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

        // #610: the geometry enum's three answers go with the projector's. A
        // satellite inside the body it is looking at leaves `h - r_eq` finite
        // and non-zero, so the affine's own step test would have handed back a
        // plane for a view that places nothing.
        let blind = GridGeometry::Geostationary(GeostationaryParams {
            h_metres: p.r_eq / 2.0,
            ..p
        });
        assert_eq!(blind.plane_affine(), None, "an unseeable Earth got a plane");
        assert_eq!(blind.forward(10, 10), None, "and placed a pixel");
        assert_eq!(blind.lonlat_bbox(), None, "and framed a box");
        // The healthy grid still answers all three.
        let seen = GridGeometry::Geostationary(p);
        assert!(seen.plane_affine().is_some());
        assert!(seen.forward(10, 10).is_some());
        assert!(seen.lonlat_bbox().is_some());
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
        let LonLatBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        } = RotatedLatLonProjector::new(rotated_fixture_params()).lonlat_bbox();
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
        let LonLatBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        } = RotatedLatLonProjector::new(p).lonlat_bbox();
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
        let LonLatBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        } = proj.lonlat_bbox();
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
        let LonLatBox { lat_max, .. } = proj.lonlat_bbox();
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
        let LonLatBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        } = proj.lonlat_bbox();
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

        let LonLatBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        } = WideMock.lonlat_bbox();
        assert!((lat_min - 12.0).abs() < 1e-9 && (lat_max - 12.0).abs() < 1e-9);
        let span = lon_max - lon_min;
        assert!(
            (span - 270.0).abs() < 1.0,
            "expected a tight ~270° span, got {span} ([{lon_min}, {lon_max}])"
        );
    }
}

/// The questions a host used to answer for itself, now `GridGeometry`'s (#571).
///
/// Their old homes were `fieldglass-napi`'s `grid_is_reprojectable` /
/// `gate_planar_reprojection` / `source_grid_is_periodic` / `contour_seam_wraps`
/// and `fieldglass`'s `geometry_is_periodic_x`; the coverage came with them,
/// widened to every family rather than to the grid-type strings one host
/// happened to spell.
#[cfg(test)]
mod grid_questions_tests {
    use super::*;

    fn latlon(ni: u32, lon_first: f64, lon_last: f64) -> GridGeometry {
        GridGeometry::LatLon(LatLonParams {
            ni,
            nj: 4,
            lat_first: 40.0,
            lon_first,
            lat_last: -40.0,
            lon_last,
        })
    }

    /// A healthy WRF-shaped Lambert domain: the family every planar assertion
    /// below perturbs one number of.
    fn lambert(earth_radius_m: f64, dx: f64, dy: f64) -> LambertParams {
        LambertParams {
            earth_radius_m,
            ni: 100,
            nj: 80,
            lat_first: 21.14,
            lon_first: -122.72,
            lad: 38.5,
            lov: -97.5,
            dx_metres: dx,
            dy_metres: dy,
            latin1: 38.5,
            latin2: 38.5,
        }
    }

    fn polar_stereo(earth_radius_m: f64, dx: f64, dy: f64) -> PolarStereoParams {
        PolarStereoParams {
            earth_radius_m,
            ni: 100,
            nj: 80,
            lat_first: 55.0,
            lon_first: -120.0,
            lov: -100.0,
            lad: 60.0,
            dx_metres: dx,
            dy_metres: dy,
            south_pole: false,
        }
    }

    /// Two flags decide two questions and nothing else reads them, which is the
    /// whole argument for scan travelling beside the geometry.
    #[test]
    fn a_south_to_north_scan_is_what_flips_the_source_view() {
        // North-down: the canvas order is already the grid order, so only the
        // caller's own request moves it.
        assert!(!Scan::north_down().flips_source_rows(false));
        assert!(Scan::north_down().flips_source_rows(true));
        // South-to-north: row 0 is the southernmost, so it arrives upside down
        // and the flip is what makes north up. Asking for a flipped view of it
        // is the grid as stored.
        let up = Scan::new(false, true, false);
        assert!(up.flips_source_rows(false));
        assert!(!up.flips_source_rows(true));
        // The other two flags never reach this answer.
        assert_eq!(
            Scan::new(true, false, true).flips_source_rows(false),
            Scan::north_down().flips_source_rows(false)
        );
    }

    /// The corner-pinned families reproject west-to-east and stay in their
    /// source projection otherwise: a −i scan would be read by their inverse
    /// maps as an antimeridian wrap.
    #[test]
    fn corner_pinned_families_answer_from_the_scan_alone() {
        let west_to_east = Scan::north_down();
        let east_to_west = Scan::new(true, false, false);
        let gaussian = GridGeometry::Gaussian(GaussianParams {
            ni: 8,
            nj: 4,
            lat_first: 80.0,
            lon_first: 0.0,
            lat_last: -80.0,
            lon_last: 315.0,
            n_parallels: 2,
        });
        let mercator = GridGeometry::Mercator(MercatorParams {
            ni: 8,
            nj: 4,
            lat_first: 40.0,
            lon_first: 0.0,
            lat_last: -40.0,
            lon_last: 315.0,
        });
        let rotated = GridGeometry::RotatedLatLon(RotatedLatLonParams {
            ni: 8,
            nj: 4,
            lat_first: 40.0,
            lon_first: 0.0,
            lat_last: -40.0,
            lon_last: 315.0,
            south_pole_lat: -30.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
        });
        for grid in [latlon(8, 0.0, 315.0), gaussian, mercator, rotated] {
            let named = grid.kind().to_string();
            assert!(grid.reprojectable(west_to_east), "{named} west-to-east");
            assert!(!grid.reprojectable(east_to_west), "{named} east-to-west");
        }
    }

    /// A lookup grid has no scan direction to be wrong about — its centres carry
    /// their own positions (#445) — and an unmodelled family has no map at all.
    #[test]
    fn a_lookup_grid_always_reprojects_and_an_unmodelled_one_never_does() {
        let index = crate::spatial_index::SpatialIndex::new(
            2,
            2,
            &[10.0, 10.0, 20.0, 20.0],
            &[0.0, 1.0, 0.0, 1.0],
        )
        .expect("four finite centres make an index");
        let lookup = GridGeometry::Lookup(index);
        assert!(lookup.reprojectable(Scan::north_down()));
        assert!(lookup.reprojectable(Scan::new(true, true, true)));
        assert!(
            !GridGeometry::Unsupported {
                label: "healpix".to_string(),
            }
            .reprojectable(Scan::north_down())
        );
    }

    /// The planar families ignore the scan — their direction bits are already in
    /// the signed spacings — and are asked instead whether the plane is real.
    #[test]
    fn planar_families_ignore_the_scan_and_need_a_plane_that_carries_a_cell() {
        let healthy = GridGeometry::Lambert(lambert(6_370_000.0, 3_000.0, 3_000.0));
        assert!(healthy.reprojectable(Scan::north_down()));
        assert!(
            healthy.reprojectable(Scan::new(true, true, false)),
            "a −i scan is already baked into Dx, so it must not gate a planar grid"
        );
        // A cone with both standard parallels on the equator resolves to
        // nothing, whatever its spacings say.
        assert!(
            !GridGeometry::Lambert(LambertParams {
                latin1: 0.0,
                latin2: 0.0,
                ..lambert(6_370_000.0, 3_000.0, 3_000.0)
            })
            .reprojectable(Scan::north_down())
        );
        // A grid with no step to walk, and one whose cell is wider than the
        // Earth it is drawn on.
        assert!(
            !GridGeometry::Lambert(lambert(6_370_000.0, 0.0, 3_000.0))
                .reprojectable(Scan::north_down())
        );
        assert!(
            !GridGeometry::Lambert(lambert(6_370_000.0, 12_000_000.0, 12_000_000.0))
                .reprojectable(Scan::north_down())
        );
        // Not a sphere at all.
        for radius in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert!(
                !GridGeometry::Lambert(lambert(radius, 3_000.0, 3_000.0))
                    .reprojectable(Scan::north_down()),
                "radius {radius} is not a sphere"
            );
        }
    }

    /// The band between the declared radius and the family's own plane, which is
    /// why both are measured. A polar stereographic plane is `2·R·k₀` — 11 886
    /// km at a ±60° latitude of true scale on a 6 370 km Earth — so a cell
    /// between the two passes `is_well_defined` and would have been offered by a
    /// predicate that asked only the projector, then refused by the render.
    #[test]
    fn the_declared_radius_is_measured_as_well_as_the_familys_own_plane() {
        let between = polar_stereo(6_370_000.0, 8_000_000.0, 8_000_000.0);
        assert!(
            PolarStereoProjector::new(between).is_well_defined(),
            "the projector's own plane carries this cell"
        );
        assert!(
            !GridGeometry::PolarStereo(between).reprojectable(Scan::north_down()),
            "but the declared Earth does not, and the render measures that one"
        );
        // The same grid at a real 5 km cell reprojects.
        assert!(
            GridGeometry::PolarStereo(polar_stereo(6_370_000.0, 5_000.0, 5_000.0))
                .reprojectable(Scan::north_down())
        );
    }

    /// A space view needs no radius floor — its axes are scan angles and its
    /// maths is all ratios — but it does need a step, and a body to look at.
    #[test]
    fn a_space_view_needs_a_scan_step_and_a_visible_body() {
        let goes = GeostationaryParams {
            ni: 100,
            nj: 100,
            h_metres: 42_164_160.0,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314_14,
            sub_lon_deg: -75.0,
            sweep_x: true,
            x0: -0.101332,
            dx_rad: 5.6e-5,
            y0: 0.128212,
            dy_rad: -5.6e-5,
        };
        assert!(GridGeometry::Geostationary(goes).reprojectable(Scan::north_down()));
        // An orthographic view states no camera altitude, so it has no step.
        assert!(
            !GridGeometry::Geostationary(GeostationaryParams {
                dx_rad: 0.0,
                ..goes
            })
            .reprojectable(Scan::north_down())
        );
        // A shapeless ellipsoid: `(r_pol/r_eq)²` is a NaN and the ray meets
        // nothing (#610).
        assert!(
            !GridGeometry::Geostationary(GeostationaryParams { r_eq: 0.0, ..goes })
                .reprojectable(Scan::north_down())
        );
    }

    /// Periodicity is a property of the column axis, so only the geographic
    /// families can have it, and a rotated grid is judged in the frame its
    /// corners are stated in.
    #[test]
    fn only_a_geographic_grid_closes_on_itself() {
        // 8 columns 45° apart, stopping one step short of the seam.
        assert!(latlon(8, 0.0, 315.0).is_periodic_x());
        assert!(!latlon(8, 0.0, 40.0).is_periodic_x());
        assert!(
            !GridGeometry::Lambert(lambert(6_370_000.0, 3_000.0, 3_000.0)).is_periodic_x(),
            "no finite number of projection-plane metres wraps the Earth"
        );
        assert!(
            !GridGeometry::Unsupported {
                label: "healpix".to_string(),
            }
            .is_periodic_x()
        );
    }

    /// The seam wrap is periodicity *and* a uniformly eastward longitude, which
    /// is what excludes the rotated family: its rows are small circles.
    #[test]
    fn the_contour_seam_wraps_only_where_longitude_advances_uniformly() {
        assert!(latlon(8, 0.0, 315.0).contour_seam_wraps());
        assert!(!latlon(8, 0.0, 40.0).contour_seam_wraps());
        let rotated_global = GridGeometry::RotatedLatLon(RotatedLatLonParams {
            ni: 16,
            nj: 8,
            lat_first: 40.0,
            lon_first: 0.0,
            lat_last: -40.0,
            lon_last: 337.5,
            south_pole_lat: -30.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
        });
        assert!(
            rotated_global.is_periodic_x(),
            "it really is periodic in its own frame"
        );
        assert!(
            !rotated_global.contour_seam_wraps(),
            "but its geographic longitude is neither uniform nor monotonic"
        );
    }

    /// Where the data is and what window to frame it with are two answers, and
    /// they differ by exactly one thing: the seam gap a periodic grid owns.
    #[test]
    fn the_render_window_is_the_extent_plus_the_seam_gap() {
        let global = latlon(8, 0.0, 315.0);
        let extent = global
            .lonlat_bbox()
            .expect("a lat/lon grid states an extent");
        let window = global.render_window().expect("and a window");
        assert_eq!(extent.lon_max, 315.0, "the data stops at the last column");
        assert_eq!(window.lon_max, 360.0, "the window runs the turn");
        assert_eq!(
            (window.lat_min, window.lat_max, window.lon_min),
            (extent.lat_min, extent.lat_max, extent.lon_min),
            "widening moves the east edge and nothing else"
        );
        // A regional grid's two answers are the same box.
        let regional = latlon(8, 0.0, 40.0);
        assert_eq!(regional.render_window(), regional.lonlat_bbox());
        // A family with no extent has no window either, rather than a plausible
        // box on null island.
        assert_eq!(
            GridGeometry::Unsupported {
                label: "healpix".to_string(),
            }
            .render_window(),
            None
        );
    }

    /// A rotated grid periodic in its own frame is **not** widened, and this is
    /// the measurement that says why rather than an argument that it is safe.
    ///
    /// The two frames come apart hardest on a rotated polar cap: rotated
    /// latitudes 80–90° over a full rotated turn is periodic in `i` and covers
    /// 23° of geographic longitude. Widening it on the strength of that
    /// periodicity frames the whole globe for a blob — a regression 15 times
    /// worse than the seam gap it would be closing. The band case, where the
    /// widening *is* nearly right, is here beside it so the two are not
    /// confused: a periodic rotated grid's window is its walked extent either
    /// way.
    #[test]
    fn a_periodic_rotated_grid_keeps_its_walked_extent() {
        let band = RotatedLatLonParams {
            ni: 16,
            nj: 8,
            lat_first: 40.0,
            lon_first: 0.0,
            lat_last: -40.0,
            lon_last: 337.5,
            south_pole_lat: -30.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
        };
        let cap = RotatedLatLonParams {
            lat_first: 90.0,
            lat_last: 80.0,
            ..band
        };
        for (label, params) in [("band", band), ("cap", cap)] {
            let grid = GridGeometry::RotatedLatLon(params);
            assert!(
                grid.is_periodic_x(),
                "{label}: the columns really do close on themselves"
            );
            assert!(
                !grid.columns_advance_eastward(),
                "{label}: but not in geographic longitude"
            );
            assert_eq!(
                grid.render_window(),
                grid.lonlat_bbox(),
                "{label}: so the window is the walked extent"
            );
        }
        // The number the rule is worth: widening the cap would frame 360° in
        // place of the 23° the data actually covers.
        let capped = GridGeometry::RotatedLatLon(cap)
            .lonlat_bbox()
            .expect("the walk places it");
        assert!(
            (capped.lon_max - capped.lon_min) < 30.0,
            "a rotated polar cap is a narrow blob, got {}°",
            capped.lon_max - capped.lon_min
        );
    }

    /// A space view answers `reprojectable` with the same two conditions
    /// `lonlat_bbox` and `inverse` gate on, so the three cannot disagree.
    ///
    /// The invariant, rather than a list of cases: a grid this offers must be a
    /// grid that can be framed. `dx_rad != 0.0` was the first spelling of the
    /// raster half and is `true` for a `NaN`, which is the silent NaN index
    /// #603 was filed for; a one-column raster is the other way it comes apart.
    #[test]
    fn a_space_view_offers_exactly_what_it_can_frame() {
        let goes = GeostationaryParams {
            ni: 100,
            nj: 100,
            h_metres: 42_164_160.0,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314_14,
            sub_lon_deg: -75.0,
            sweep_x: true,
            x0: -0.101332,
            dx_rad: 5.6e-5,
            y0: 0.128212,
            dy_rad: -5.6e-5,
        };
        let perturbations = [
            ("healthy", goes),
            (
                "nan dx",
                GeostationaryParams {
                    dx_rad: f64::NAN,
                    ..goes
                },
            ),
            (
                "nan dy",
                GeostationaryParams {
                    dy_rad: f64::NAN,
                    ..goes
                },
            ),
            (
                "zero dx",
                GeostationaryParams {
                    dx_rad: 0.0,
                    ..goes
                },
            ),
            (
                "nan x0",
                GeostationaryParams {
                    x0: f64::NAN,
                    ..goes
                },
            ),
            ("one column", GeostationaryParams { ni: 1, ..goes }),
            ("one row", GeostationaryParams { nj: 1, ..goes }),
            ("shapeless", GeostationaryParams { r_eq: 0.0, ..goes }),
            (
                "prolate",
                GeostationaryParams {
                    r_pol: 7.0e6,
                    ..goes
                },
            ),
            (
                "no line of sight",
                GeostationaryParams {
                    h_metres: 1.0e5,
                    ..goes
                },
            ),
        ];
        for (label, params) in perturbations {
            let grid = GridGeometry::Geostationary(params);
            assert_eq!(
                grid.reprojectable(Scan::north_down()),
                grid.lonlat_bbox().is_some(),
                "{label}: an offer that cannot be framed is a promise the warp \
                 then refuses"
            );
        }
        assert!(GridGeometry::Geostationary(goes).reprojectable(Scan::north_down()));
    }

    /// The metre-plane families answer the same way, through the same
    /// `planar_grid_is_placeable` the other three answers gate on.
    ///
    /// The far-pole §3.20 corner is the case: the plane is sound and the origin
    /// is finite, so `is_well_defined` alone says yes, but the origin lands so
    /// far out that adding a grid step is a no-op in `f64` and `inverse`
    /// declines every pixel.
    #[test]
    fn a_metre_plane_offers_exactly_what_it_can_frame() {
        let base = polar_stereo(6_370_000.0, 60_000.0, 60_000.0);
        for (label, params) in [
            ("healthy", base),
            (
                "corner at the far pole",
                PolarStereoParams {
                    lat_first: -90.0,
                    ..base
                },
            ),
            (
                "cell wider than the declared Earth",
                polar_stereo(6_370_000.0, 12_000_000.0, 12_000_000.0),
            ),
            ("no step", polar_stereo(6_370_000.0, 0.0, 60_000.0)),
        ] {
            let grid = GridGeometry::PolarStereo(params);
            assert_eq!(
                grid.reprojectable(Scan::north_down()),
                grid.lonlat_bbox().is_some(),
                "{label}: an offer that cannot be framed is a promise the warp \
                 then refuses"
            );
        }
        for (label, params) in [
            ("healthy", lambert(6_370_000.0, 3_000.0, 3_000.0)),
            (
                "cone open away from the first corner",
                LambertParams {
                    lat_first: -89.999_999,
                    latin1: 60.0,
                    latin2: 60.0,
                    ..lambert(6_370_000.0, 3_000.0, 3_000.0)
                },
            ),
        ] {
            let grid = GridGeometry::Lambert(params);
            assert_eq!(
                grid.reprojectable(Scan::north_down()),
                grid.lonlat_bbox().is_some(),
                "{label}: an offer that cannot be framed is a promise the warp \
                 then refuses"
            );
        }
    }
}
