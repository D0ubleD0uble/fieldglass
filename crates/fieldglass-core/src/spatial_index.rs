//! Nearest-cell lookup for grids that are a *list of cell centres* rather than
//! a formula.
//!
//! Three families need it: NetCDF 2-D coordinate variables (#218), GRIB2
//! §3.204 (#418) and ICON §3.101 (#420). [`crate::warp`] is already
//! inverse-mapped — it walks output pixels and asks the source grid which cell
//! contains each one — so those families need an implementation of that
//! question, not a different renderer.
//!
//! # Why the index is built in three dimensions
//!
//! Cell centres arrive as latitude and longitude, and searching in that space
//! needs a special case for the antimeridian (where +179.9° and −179.9° are
//! neighbours) and another for the poles (where every longitude meets). Mapping
//! each centre to a unit vector removes both: a seam-crossing pair is simply
//! two nearby points, and the poles are ordinary interior points.
//!
//! It is also still the right answer. The chord between two unit vectors is
//! `2·sin(θ/2)` for a central angle `θ`, which increases strictly over
//! `θ ∈ [0, π]` — so whichever centre is nearest by chord is nearest by
//! great-circle too, and an ordinary Euclidean k-d tree gives the geodesic
//! answer. `nearest_by_chord_agrees_with_great_circle` checks that rather than
//! taking it on trust.
//!
//! # Nearest only
//!
//! A lookup grid answers with a cell, not a position within one. Index-adjacent
//! cells need not be spatially adjacent — a tripolar ocean grid folds, so
//! `(i, j)` and `(i + 1, j)` can be far apart — which makes the fractional part
//! of a [`GridIndex`] and the "next column" neighbour meaningless. The index
//! therefore reports [`GridResampling::NearestOnly`], and `warp` honours it
//! rather than leaving it to callers.

use crate::projection::{GridIndex, GridResampling};

/// Nearest-cell lookup over an explicit list of cell centres.
///
/// Build is `O(n log n)` and the query is `O(log n)` on well-distributed
/// centres. The tree is a permutation of cell indices with the median at the
/// middle of each range, so it costs one `u32` per cell beyond the coordinates
/// themselves.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "IndexCoords", into = "IndexCoords")]
pub struct SpatialIndex {
    ni: u32,
    nj: u32,
    /// Cell centres as unit vectors, in row-major `j * ni + i` order. A centre
    /// the source left non-finite (a NetCDF fill value in a coordinate
    /// variable) is stored so indices stay aligned, but never entered into the
    /// tree, so it can never be returned.
    xyz: Vec<[f64; 3]>,
    /// Cell indices, permuted into k-d tree order. Excludes non-finite centres.
    tree: Vec<u32>,
    /// Chord beyond which a query is off the grid rather than near its edge.
    max_chord: f64,
}

/// How far past the grid's own spacing a query may sit and still be counted as
/// inside it, as a multiple of the 95th-percentile centre spacing.
///
/// A lookup grid has no formula to say "outside", so without a cutoff every
/// query returns *some* cell and a regional grid warped onto a world map paints
/// the entire map with its edge cells. Two is loose enough not to punch holes
/// where a curvilinear grid's spacing varies, and tight enough that a point a
/// couple of cells off the edge is refused.
const EDGE_SLACK: f64 = 2.0;

/// Cells sampled when measuring the grid's own spacing. Deterministic (a
/// stride, not a random draw) so the same grid always yields the same cutoff.
const SPACING_SAMPLES: usize = 1024;

impl SpatialIndex {
    /// Build an index over `ni × nj` cell centres given in degrees, row-major.
    ///
    /// Returns `None` if the lengths disagree with `ni × nj`, if either
    /// dimension is zero, or if no centre is finite — a grid with nothing to
    /// find is not an index, and answering every query with `None` would look
    /// like a rendering bug rather than a malformed file.
    pub fn new(ni: u32, nj: u32, lats: &[f64], lons: &[f64]) -> Option<Self> {
        let n = (ni as usize).checked_mul(nj as usize)?;
        if n == 0 || lats.len() != n || lons.len() != n {
            return None;
        }
        let xyz: Vec<[f64; 3]> = lats
            .iter()
            .zip(lons)
            .map(|(&lat, &lon)| unit_vector(lat, lon))
            .collect();

        let mut tree: Vec<u32> = (0..n as u32)
            .filter(|&k| xyz[k as usize][0].is_finite())
            .collect();
        if tree.is_empty() {
            return None;
        }
        build(&mut tree, &xyz, 0);

        let mut index = Self {
            ni,
            nj,
            xyz,
            tree,
            max_chord: f64::INFINITY,
        };
        index.max_chord = index.measure_spacing() * EDGE_SLACK;
        Some(index)
    }

    /// Override the cutoff with a great-circle distance in metres, against a
    /// sphere of `earth_radius_m`. For a caller that knows the grid's real cell
    /// size better than the centres do.
    pub fn with_max_distance(mut self, metres: f64, earth_radius_m: f64) -> Self {
        let theta = (metres / earth_radius_m).clamp(0.0, std::f64::consts::PI);
        self.max_chord = 2.0 * (theta / 2.0).sin();
        self
    }

    pub fn dims(&self) -> (u32, u32) {
        (self.ni, self.nj)
    }

    /// Cells actually searchable — fewer than `ni × nj` when the source left
    /// some centres non-finite.
    pub fn len(&self) -> usize {
        self.tree.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    /// A lookup grid is always [`GridResampling::NearestOnly`]; see the module
    /// docs.
    pub fn resampling(&self) -> GridResampling {
        GridResampling::NearestOnly
    }

    /// The cell containing `(lat, lon)`, or `None` when the point is further
    /// from every centre than the grid's own spacing allows.
    ///
    /// The returned index is always integral. Nothing here interpolates, so a
    /// caller must not read the fractional part as a position within the cell.
    pub fn nearest(&self, lat: f64, lon: f64) -> Option<GridIndex> {
        if !lat.is_finite() || !lon.is_finite() {
            return None;
        }
        let target = unit_vector(lat, lon);
        let mut best = (f64::INFINITY, u32::MAX);
        search(&self.tree, &self.xyz, target, 0, &mut best);
        let (dist2, cell) = best;
        if cell == u32::MAX || dist2.sqrt() > self.max_chord {
            return None;
        }
        Some(GridIndex {
            i: (cell % self.ni) as f64,
            j: (cell / self.ni) as f64,
        })
    }

    /// The 95th percentile of the distance from a sampled centre to its nearest
    /// other centre — the grid's own spacing, robust to a few outliers but not
    /// so tight that a sparse region is refused.
    fn measure_spacing(&self) -> f64 {
        let stride = (self.tree.len() / SPACING_SAMPLES).max(1);
        let mut spacings: Vec<f64> = self
            .tree
            .iter()
            .step_by(stride)
            .filter_map(|&cell| {
                let target = self.xyz[cell as usize];
                let mut best = (f64::INFINITY, u32::MAX);
                // Exclude the sample itself, which is at distance zero.
                search_excluding(&self.tree, &self.xyz, target, cell, 0, &mut best);
                (best.1 != u32::MAX).then(|| best.0.sqrt())
            })
            .collect();
        if spacings.is_empty() {
            // One searchable cell: nothing to measure a spacing against, so any
            // query near it should find it and the cutoff must not refuse.
            return f64::INFINITY;
        }
        spacings.sort_by(|a, b| a.partial_cmp(b).expect("chords are finite"));
        spacings[(spacings.len() * 95 / 100).min(spacings.len() - 1)]
    }
}

/// `(lat, lon)` in degrees to a unit vector. A non-finite input yields a
/// non-finite vector, which `SpatialIndex::new` uses to exclude the cell.
fn unit_vector(lat: f64, lon: f64) -> [f64; 3] {
    let (lat_r, lon_r) = (lat.to_radians(), lon.to_radians());
    let (sin_lat, cos_lat) = lat_r.sin_cos();
    let (sin_lon, cos_lon) = lon_r.sin_cos();
    [cos_lat * cos_lon, cos_lat * sin_lon, sin_lat]
}

fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

/// Permute `idx` into k-d tree order: the median on the current axis sits at
/// the middle of each range, with the two halves recursively arranged the same
/// way. Axes cycle x, y, z with depth.
fn build(idx: &mut [u32], pts: &[[f64; 3]], depth: usize) {
    if idx.len() <= 1 {
        return;
    }
    let axis = depth % 3;
    let mid = idx.len() / 2;
    idx.select_nth_unstable_by(mid, |&a, &b| {
        pts[a as usize][axis]
            .partial_cmp(&pts[b as usize][axis])
            .expect("tree holds only finite points")
    });
    let (left, rest) = idx.split_at_mut(mid);
    build(left, pts, depth + 1);
    build(&mut rest[1..], pts, depth + 1);
}

/// Branch-and-bound nearest search. `best` is `(squared chord, cell)`.
fn search(idx: &[u32], pts: &[[f64; 3]], target: [f64; 3], depth: usize, best: &mut (f64, u32)) {
    search_inner(idx, pts, target, depth, best, u32::MAX);
}

fn search_excluding(
    idx: &[u32],
    pts: &[[f64; 3]],
    target: [f64; 3],
    exclude: u32,
    depth: usize,
    best: &mut (f64, u32),
) {
    search_inner(idx, pts, target, depth, best, exclude);
}

fn search_inner(
    idx: &[u32],
    pts: &[[f64; 3]],
    target: [f64; 3],
    depth: usize,
    best: &mut (f64, u32),
    exclude: u32,
) {
    if idx.is_empty() {
        return;
    }
    let axis = depth % 3;
    let mid = idx.len() / 2;
    let node = idx[mid];
    if node != exclude {
        let d = dist2(pts[node as usize], target);
        if d < best.0 {
            *best = (d, node);
        }
    }

    let delta = target[axis] - pts[node as usize][axis];
    let (near, far) = if delta < 0.0 {
        (&idx[..mid], &idx[mid + 1..])
    } else {
        (&idx[mid + 1..], &idx[..mid])
    };
    search_inner(near, pts, target, depth + 1, best, exclude);
    // The far side can only hold something closer if the splitting plane
    // itself is closer than the best found so far.
    if delta * delta < best.0 {
        search_inner(far, pts, target, depth + 1, best, exclude);
    }
}

// ---------------------------------------------------------------------------
// Serialisation
// ---------------------------------------------------------------------------

/// The wire form: the centres, and nothing derived from them.
///
/// ADR-0006 wants every API type serde-derivable, but the tree and the cutoff
/// are a cache of the coordinates — writing them out would make the wire format
/// depend on the build algorithm, and reading them back would let a
/// hand-written payload claim a tree that does not match its own points. Both
/// are recomputed on the way in.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct IndexCoords {
    ni: u32,
    nj: u32,
    lats: Vec<f64>,
    lons: Vec<f64>,
}

impl From<SpatialIndex> for IndexCoords {
    fn from(v: SpatialIndex) -> Self {
        // Back out the centres from the unit vectors rather than storing a
        // second copy: `atan2`/`asin` invert `unit_vector` exactly enough that
        // a round trip lands on the same cell, which is all the index promises.
        let (lats, lons) = v
            .xyz
            .iter()
            .map(|&[x, y, z]| {
                if x.is_finite() {
                    (
                        z.clamp(-1.0, 1.0).asin().to_degrees(),
                        y.atan2(x).to_degrees(),
                    )
                } else {
                    (f64::NAN, f64::NAN)
                }
            })
            .unzip();
        Self {
            ni: v.ni,
            nj: v.nj,
            lats,
            lons,
        }
    }
}

impl TryFrom<IndexCoords> for SpatialIndex {
    type Error = &'static str;

    fn try_from(c: IndexCoords) -> Result<Self, Self::Error> {
        SpatialIndex::new(c.ni, c.nj, &c.lats, &c.lons)
            .ok_or("cell centres do not describe an ni x nj grid with any finite point")
    }
}
