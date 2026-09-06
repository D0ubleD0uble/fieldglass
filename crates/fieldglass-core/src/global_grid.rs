//! The global lat/lon grid a synthesised field is put onto, stated once.
//!
//! Two grid families arrive as something other than a rectangle of values and
//! have to be *put* on one before anything downstream can draw them: spectral
//! messages, whose coefficients [`crate::sht`] evaluates anywhere, and HEALPix
//! fields, whose pixels [`crate::healpix`] resamples. Neither has a raster
//! shape of its own, so the shape is a choice — and the same choice for both:
//!
//! - **Latitudes run pole to pole**, `90 … −90`, `nj` of them, north first.
//! - **Longitudes run `0 … 360 − Δ`**, `ni` of them, with **no duplicated wrap
//!   column**. The gap past the last one closes the circle back to column 0.
//!
//! The wrap column is the half that bites. Repeat longitude 0 at the eastern
//! edge and the field is doubled at the antimeridian; declare `lon_last = 360`
//! for a grid that stops a step short and the warp reads one cell of the field
//! twice. So the axes, the eastern corner the render meta declares
//! ([`GlobalGrid::lon_last`]) and the [`LatLonParams`] a host builds from the
//! grid all come from here rather than being spelled out per call site.
//!
//! # The 0.5° pin
//!
//! [`SYNTHESIS_STEP_DEG`] is the resolution both paths pin to, and they mean
//! subtly different things by it — worth keeping straight:
//!
//! - A spectral field is *band-limited*: any grid at or above the truncation's
//!   minimum reproduces it exactly, and a finer one is a sharper picture rather
//!   than interpolation between samples. 0.5° is therefore the grid itself, for
//!   every truncation ([`crate::sht::spectral_render_dims`]).
//! - A HEALPix field is *sampled*: the pixel scale is the rule, and 0.5° is only
//!   the *cap* on it ([`crate::healpix::healpix_render_dims`]). Finer than the
//!   pixel scale merely repeats pixels.

use crate::projection::{LatLonParams, reduced_raster_lon_last};

/// The finest step either synthesis path resamples at, in degrees.
///
/// Half a degree: 720 columns and 361 rows pole to pole. See the module docs
/// for why the spectral path treats this as the grid and the HEALPix path as a
/// ceiling.
pub const SYNTHESIS_STEP_DEG: f64 = 0.5;

/// Columns in the [`SYNTHESIS_STEP_DEG`] grid: `360 / 0.5`.
pub const SYNTHESIS_NI: usize = 720;

/// Rows in the [`SYNTHESIS_STEP_DEG`] grid: `180 / 0.5 + 1`, the `+ 1` because
/// both poles are grid points.
pub const SYNTHESIS_NJ: usize = 361;

/// A field put on a [`GlobalGrid`]: the grid, and one cell per point of it in
/// row-major order from the north-west corner.
///
/// The pairing is the point. Choosing the grid and evaluating the field on it
/// are separate steps, and every call that does both hands them back together
/// so a host cannot declare one shape in its render geometry and evaluate at
/// another — see [`crate::healpix::resample_to_global`] and the GRIB readers'
/// `synthesize_message_global`.
///
/// `Option` per cell, matching what a raster decode returns, so a caller
/// substitutes one for the other. The spectral path fills every cell (its
/// coefficients evaluate everywhere); the HEALPix path can leave one absent,
/// because a masked pixel stays masked through the resample.
pub type SynthesisedField = (GlobalGrid, Vec<Option<f64>>);

/// A global regular lat/lon grid of `ni × nj` points, in the convention this
/// module's docs state.
///
/// Only the shape is stored. Every coordinate — the axes, both derived corners,
/// and the [`LatLonParams`] a host declares — comes from that shape, so a grid
/// whose axes disagree with its declared corners is unrepresentable, degenerate
/// shapes included.
///
/// Nothing bounds `ni` and `nj`: [`latitudes`](Self::latitudes) and
/// [`longitudes`](Self::longitudes) allocate one element each, so a caller
/// building a grid from an untrusted number owns that bound. The rules that
/// build one here ([`crate::sht::spectral_render_grid`],
/// [`crate::healpix::healpix_render_grid`]) cap it at 720 × 361.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GlobalGrid {
    /// Columns — longitudes, `0 … 360 − 360/ni`.
    pub ni: usize,
    /// Rows — latitudes, `90 … −90`.
    pub nj: usize,
}

impl GlobalGrid {
    /// The grid at the [`SYNTHESIS_STEP_DEG`] pin: `720 × 361`.
    pub const FINEST: Self = Self {
        ni: SYNTHESIS_NI,
        nj: SYNTHESIS_NJ,
    };

    /// A grid of `ni` columns by `nj` rows.
    pub const fn new(ni: usize, nj: usize) -> Self {
        Self { ni, nj }
    }

    /// `(ni, nj)` — the pair the render seams pass around.
    pub const fn dims(&self) -> (usize, usize) {
        (self.ni, self.nj)
    }

    /// Number of grid points, `ni · nj`.
    ///
    /// Saturating: `ni` reaches a synthesis rule that derives it from an
    /// untrusted `Nside` ([`crate::healpix::healpix_render_dims`]), so a wrapped
    /// count would make an out-of-range value look in range. (The spectral rule
    /// ignores its truncation, so that half cannot be driven from a message.)
    pub const fn len(&self) -> usize {
        self.ni.saturating_mul(self.nj)
    }

    /// Whether the grid holds no points at all.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The latitudes, north pole first: `90 … −90`, `nj` of them.
    ///
    /// A grid of fewer than two rows has no span to divide, so every row it does
    /// have sits on the north pole rather than on a division by zero.
    pub fn latitudes(&self) -> Vec<f64> {
        if self.nj < 2 {
            return vec![90.0; self.nj];
        }
        (0..self.nj)
            .map(|j| 90.0 - (j as f64) * 180.0 / (self.nj as f64 - 1.0))
            .collect()
    }

    /// The longitudes, `0 … 360 − 360/ni`, `ni` of them — no duplicated wrap
    /// column.
    pub fn longitudes(&self) -> Vec<f64> {
        (0..self.ni)
            .map(|i| (i as f64) * 360.0 / self.ni as f64)
            .collect()
    }

    /// Both axes at once, in the order the synthesis and resample calls take
    /// them.
    pub fn axes(&self) -> (Vec<f64>, Vec<f64>) {
        (self.latitudes(), self.longitudes())
    }

    /// The eastern corner a render meta declares: the last longitude, one step
    /// short of 360.
    ///
    /// `(ni − 1) · 360 / ni` — the same expression
    /// [`longitudes`](Self::longitudes) evaluates for its final element, so the
    /// declared corner is bit-identical to the coordinate the field was
    /// evaluated at. `360 − 360/ni` is the same number in exact arithmetic but
    /// not in `f64` (they differ at `ni` = 19, 43 and 2071 below 4000), which is
    /// why the corner is derived rather than respelled per call site.
    ///
    /// Delegated to [`reduced_raster_lon_last`], which states the identical rule
    /// for a reduced grid's expanded raster and had it first — a synthesis grid
    /// is that grid with `lon_first = 0`. The `u32` clamp cannot bite: a grid
    /// that wide has no axes to build, since [`longitudes`](Self::longitudes)
    /// would allocate `ni` elements.
    ///
    /// Zero for an empty grid, which has no last column.
    pub fn lon_last(&self) -> f64 {
        reduced_raster_lon_last(0.0, u32::try_from(self.ni).unwrap_or(u32::MAX))
    }

    /// The southern corner: `−90`, matching
    /// [`latitudes().last()`](Self::latitudes).
    ///
    /// The north pole for a grid of fewer than two rows, which has no span to
    /// divide and so never reaches the south — the corner and the axis agree
    /// there too rather than the corner asserting a row the axis does not hold.
    /// Zero for a grid with no rows at all.
    pub fn lat_last(&self) -> f64 {
        match self.nj {
            0 => 0.0,
            1 => 90.0,
            _ => -90.0,
        }
    }
}

impl From<(usize, usize)> for GlobalGrid {
    /// The `(ni, nj)` pair the `*_render_dims` rules return.
    fn from((ni, nj): (usize, usize)) -> Self {
        Self::new(ni, nj)
    }
}

impl From<GlobalGrid> for LatLonParams {
    /// The grid as an ordinary regular lat/lon grid — which is exactly what it
    /// is once the field is on it, so probe, contours, CSV and overlays need no
    /// special case (`docs/architecture/planned/03-composition.md`).
    ///
    /// The dimensions saturate into `u32`. Every rule that builds a
    /// [`GlobalGrid`] caps it far below that, so the clamp is unreachable
    /// rather than lossy; it is here so the conversion is total.
    fn from(grid: GlobalGrid) -> Self {
        LatLonParams {
            ni: u32::try_from(grid.ni).unwrap_or(u32::MAX),
            nj: u32::try_from(grid.nj).unwrap_or(u32::MAX),
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: grid.lat_last(),
            lon_last: grid.lon_last(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pinned_grid_is_the_half_degree_step() {
        assert_eq!(SYNTHESIS_NI as f64 * SYNTHESIS_STEP_DEG, 360.0);
        assert_eq!((SYNTHESIS_NJ - 1) as f64 * SYNTHESIS_STEP_DEG, 180.0);
        assert_eq!(GlobalGrid::FINEST.dims(), (720, 361));
        assert_eq!(GlobalGrid::FINEST.len(), 720 * 361);
        assert!(!GlobalGrid::FINEST.is_empty());
    }

    #[test]
    fn the_axes_run_pole_to_pole_without_a_duplicated_wrap_column() {
        let grid = GlobalGrid::FINEST;
        let (lats, lons) = grid.axes();
        assert_eq!((lons.len(), lats.len()), (720, 361));
        assert_eq!(lats.first(), Some(&90.0), "starts at the north pole");
        assert_eq!(lats.last(), Some(&-90.0), "ends at the south");
        assert_eq!(lons.first(), Some(&0.0));
        // One step short of 360: the gap past the last column closes the circle
        // back to column 0, rather than repeating longitude 0 at both edges.
        let step = 360.0 / lons.len() as f64;
        assert!((lons[lons.len() - 1] - (360.0 - step)).abs() < 1e-9);
        assert!(lons[lons.len() - 1] < 360.0);
    }

    /// The corner the meta declares must be the coordinate the field was
    /// evaluated at, bit for bit — a warp that reads a cell twice at the seam is
    /// what a mismatch here looks like.
    #[test]
    fn lon_last_is_bit_identical_to_the_final_longitude() {
        for ni in (2..=740).chain([1, 1024, 2071, 4096]) {
            let grid = GlobalGrid::new(ni, 3);
            assert_eq!(
                grid.longitudes().last().copied(),
                Some(grid.lon_last()),
                "ni {ni}"
            );
        }
    }

    /// `360 − 360/ni` is the other spelling this convention used to carry, and
    /// it is not the same `f64`. Naming the disagreement keeps the choice of
    /// expression from looking arbitrary.
    #[test]
    fn the_other_spelling_of_the_corner_really_does_disagree() {
        let disagree: Vec<usize> = (2..4000)
            .filter(|&ni| GlobalGrid::new(ni, 3).lon_last() != 360.0 - 360.0 / ni as f64)
            .collect();
        assert_eq!(disagree, vec![19, 43, 2071]);
    }

    /// The type doc's guarantee, checked rather than asserted: both derived
    /// corners are the axes' own endpoints, for every shape including the
    /// degenerate ones a public `new` can build.
    #[test]
    fn the_derived_corners_are_the_axes_own_endpoints() {
        for (ni, nj) in [(720, 361), (26, 14), (2, 2), (1, 1), (4, 1), (0, 4), (4, 0)] {
            let grid = GlobalGrid::new(ni, nj);
            let (lats, lons) = grid.axes();
            assert_eq!(
                lats.last().copied().unwrap_or(0.0),
                grid.lat_last(),
                "{grid:?}: lat_last is not the last latitude"
            );
            assert_eq!(
                lons.last().copied().unwrap_or(0.0),
                grid.lon_last(),
                "{grid:?}: lon_last is not the last longitude"
            );
            let params = LatLonParams::from(grid);
            assert_eq!(
                (params.lat_last, params.lon_last),
                (grid.lat_last(), grid.lon_last())
            );
        }
    }

    /// The rule already had a home for a reduced grid's expanded raster, and
    /// this is that grid with `lon_first = 0`. If the two ever disagree, one of
    /// them has been rewritten.
    #[test]
    fn the_corner_agrees_with_the_reduced_raster_rule_it_delegates_to() {
        for ni in (0..=2048).chain([4096, 65_536]) {
            assert_eq!(
                GlobalGrid::new(ni, 3).lon_last(),
                reduced_raster_lon_last(0.0, ni as u32),
                "ni {ni}"
            );
        }
    }

    #[test]
    fn a_degenerate_grid_answers_rather_than_dividing_by_zero() {
        // A single row has no span to divide: it sits on the pole.
        assert_eq!(GlobalGrid::new(4, 1).latitudes(), vec![90.0]);
        assert!(GlobalGrid::new(4, 0).latitudes().is_empty());
        assert!(GlobalGrid::new(0, 4).longitudes().is_empty());
        assert_eq!(GlobalGrid::new(0, 4).lon_last(), 0.0);
        assert!(GlobalGrid::new(0, 4).is_empty());
        assert!(GlobalGrid::new(4, 0).is_empty());
        for grid in [GlobalGrid::new(4, 1), GlobalGrid::new(4, 0)] {
            assert!(
                grid.latitudes().iter().all(|lat| lat.is_finite()),
                "{grid:?} produced a non-finite latitude"
            );
        }
    }

    #[test]
    fn the_grid_converts_to_the_lat_lon_params_it_describes() {
        let grid = GlobalGrid::FINEST;
        let params = LatLonParams::from(grid);
        assert_eq!((params.ni, params.nj), (720, 361));
        assert_eq!((params.lat_first, params.lat_last), (90.0, -90.0));
        assert_eq!(params.lon_first, 0.0);
        // 359.5 by hand: 720 columns at 0.5°, the last one a step short of 360.
        assert_eq!(params.lon_last, 359.5);
        assert_eq!(params.lon_last, grid.lon_last());
        assert_eq!(GlobalGrid::from((720, 361)), grid);
    }
}
