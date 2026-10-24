//! Project geographic polylines (coastlines, graticule, user shapes) onto a
//! warped raster for the 2-D render panel's overlay layer (#72).
//!
//! The caller supplies polylines as flat `(lat, lon)` vertices plus the
//! vertex count of each input ring, and a [`ForwardMap`] describing the
//! same raster the warp painted. We run every vertex through the target's
//! forward map ([`ForwardMap::lonlat_to_pixel`]) and emit the *visible*
//! pixel-space runs, ready for the webview to stroke on its overlay canvas.
//!
//! A single input ring becomes zero or more output runs. We break a run
//! whenever a vertex leaves the projection's visible domain (the back of an
//! orthographic globe, past a polar disc's equator rim), and whenever a segment
//! crosses the target's longitude seam — the meridian drawn on both edges of the
//! map, where a polyline that wraps around the far side must not be stroked
//! straight across. How the seam is detected is the target's own business,
//! reported through [`ForwardMap::seam_split`] ([`SeamSplit`]):
//!
//! * The **world** targets (Mollweide, Robinson, Equal Earth) and the
//!   **lat/lon-box** targets (equirectangular, Web Mercator) know their centre
//!   meridian, so the seam is at `centre ± 180`. We test it *geographically*:
//!   split when the two vertices' centre-relative longitudes differ by more than
//!   180°. This never looks at pixels, so it is exact on a map that tapers toward
//!   the poles — Mollweide's ellipse narrows below half the raster width above
//!   ~70.4°, and an earlier pixel-distance test missed a genuine seam hop there
//!   and streaked a line clear across the map (#278).
//! * The **source** projection lays the rectangular grid straight into pixels, so
//!   a column wrap really is a greater-than-half-width jump in pixel x; it splits
//!   on that. Its wrap meridian is not carried here, and a projected or regional
//!   grid may have no single seam (off-coverage vertices invert to `None` and
//!   break the run anyway). A **lookup** grid (a list of cell centres, such as
//!   a tripolar ocean mesh) can also fold, putting two neighbours on the ground
//!   far apart in pixels by less than half the width; it is tested segment by
//!   segment through the segment's geographic midpoint, which also lets a run
//!   follow the grid where it bends ([`SeamSplit::Folded`], #1430). On a
//!   periodic grid the picture is one whole turn, so a run that crosses the
//!   wrap is cut at one side edge and goes on from the other
//!   ([`SeamSplit::PixelWrap`], #1464).
//! * The **azimuthal** targets (orthographic, polar stereographic) have no seam:
//!   a chord between two on-disc points stays on the near hemisphere, so a large
//!   pixel gap is never a seam. They break a run only on a domain exit.
//!
//! Runs whose bounding box never reaches the viewport, or that are left with
//! fewer than two vertices, are dropped (nothing visible to stroke).
//!
//! A run that leaves the domain stops at its last vertex inside, and the canvas
//! clips whatever lies past the raster. The source projection is the exception:
//! its domain is the picture itself, so a run that leaves is carried to the
//! picture's edge, found by halving the segment that leaves
//! ([`ForwardMap::runs_reach_domain_edge`]), and each point is drawn in the
//! middle of its cell ([`ForwardMap::pixel_offset`], #1464).
//!
//! The geographic seam test pairs with the seam-vertex placement fixed in #277
//! (`wrap_to_seam_span`, which keeps a vertex sitting *on* the seam on the edge
//! its sign names): placement decides which edge a seam vertex lands on, this
//! decides where a run is cut. Both are needed for a correct high-latitude
//! overlay.
//!
//! This module is deliberately projection-agnostic: it knows nothing about
//! "coastline" vs "graticule" vs a user-drawn shape — they are all just
//! `(lat, lon)` rings. The render contract (`RenderOptions`/`RenderedGrid`)
//! is untouched; this is an additive, geometry-only path.

use crate::projection::{GridIndex, GridResampling};
use crate::spatial_index::unit_vector;
use crate::warp::{ForwardMap, SeamSplit, wrap_to_seam_span};

/// Overlay forward map for the **source projection** — the unwarped grid shown
/// as-is. `paint_source` lays source grid point `(i, j)` straight into output
/// pixel `(i, j)`, so the warp's own inverse map (geographic `(lat, lon)` →
/// fractional grid index) *is* the forward pixel map here. Wrapping it as a
/// [`ForwardMap`] lets the overlay layer (#72) project coastlines /
/// graticule onto the source projection too — reusing [`project_polylines`]'s
/// visibility + seam splitting, and working for every grid type (latlon,
/// gaussian, lambert, polar stereographic) since each already supplies an
/// inverse map.
///
/// A lookup grid ([`GridResampling::NearestOnly`]) also splits a segment that
/// jumps across a fold in the grid ([`SeamSplit::Folded`]), which is why the
/// grid's resampling is carried alongside its inverse.
///
/// Grid point `(i, j)` is painted as the pixel from `(i, j)` to
/// `(i + 1, j + 1)`, so the picture runs half a cell past the outer points.
/// Two things follow (#1464). A point is drawn half a pixel on from its index,
/// in the middle of its cell ([`ForwardMap::pixel_offset`]). And the inverse
/// to wrap is the one over the cells,
/// [`GridGeometry::cell_inverse_at`](crate::GridGeometry::cell_inverse_at),
/// whose domain is the whole picture, so that a run carried to the edge of the
/// domain ([`ForwardMap::runs_reach_domain_edge`]) ends at the picture's edge.
///
/// Holds a borrowed inverse closure — a shared reference, hence `Copy`; the
/// borrow only needs to outlive the synchronous `project_polylines` call.
#[derive(Clone, Copy)]
pub struct SourceOverlayTarget<'a> {
    inverse: &'a dyn Fn(f64, f64) -> Option<GridIndex>,
    resampling: GridResampling,
    wraps: bool,
}

/// Written out rather than derived: the inverse is a `dyn Fn`, which has no
/// `Debug`, so it prints as a placeholder beside the grid's resampling.
impl std::fmt::Debug for SourceOverlayTarget<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SourceOverlayTarget")
            .field("inverse", &"<closure>")
            .field("resampling", &self.resampling)
            .field("wraps", &self.wraps)
            .finish()
    }
}

impl<'a> SourceOverlayTarget<'a> {
    /// Wrap the warp's inverse map (`lat, lon →` fractional grid index) as the
    /// source projection's overlay forward map. `resampling` is the grid's own
    /// ([`GridGeometry::resampling`](crate::GridGeometry::resampling)): a
    /// lookup grid's runs are split where they jump across a fold.
    pub fn new(
        inverse: &'a dyn Fn(f64, f64) -> Option<GridIndex>,
        resampling: GridResampling,
    ) -> Self {
        Self {
            inverse,
            resampling,
            wraps: false,
        }
    }

    /// Say whether the grid's columns go the whole way round
    /// ([`GridGeometry::is_periodic_x`](crate::GridGeometry::is_periodic_x)).
    /// With the inverse over the cells, a periodic grid's picture is one whole
    /// turn, so a run that crosses its wrap is carried to both side edges
    /// ([`SeamSplit::PixelWrap`]). A lookup grid is never periodic here: its
    /// jumps are folds, which no edge continues across.
    #[must_use]
    pub fn wrapping(mut self, wraps: bool) -> Self {
        self.wraps = wraps;
        self
    }
}

impl ForwardMap for SourceOverlayTarget<'_> {
    fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
        (self.inverse)(lat, lon).map(|GridIndex { i, j }| (i, j))
    }

    fn pixel_offset(&self) -> f64 {
        SOURCE_PIXEL_OFFSET
    }

    fn runs_reach_domain_edge(&self) -> bool {
        // Given the inverse over the cells (`GridGeometry::cell_inverse_at`),
        // the domain's edge is the picture's edge.
        true
    }

    fn seam_split(&self) -> SeamSplit {
        // The source raster is the grid laid straight in, so a longitude wrap is
        // a > half-width jump in pixel x. The grid's own seam meridian is not
        // carried here, and a projected / regional grid may have none. A lookup
        // grid can also fold, which no pixel threshold sees (#1430).
        match (self.resampling, self.wraps) {
            (GridResampling::Any, false) => SeamSplit::PixelHalfWidth,
            (GridResampling::Any, true) => SeamSplit::PixelWrap,
            (GridResampling::NearestOnly, _) => SeamSplit::Folded,
        }
    }
}

/// Where in its pixel the source projection draws a grid point: the middle.
/// A host strokes a coordinate `x` at `x` pixels from the left edge, and the
/// source view paints grid point `i` as the pixel from `i` to `i + 1`, so the
/// point is drawn at `i + 0.5` (#1464). The one place this offset is stated:
/// [`project_polylines`] applies it through [`ForwardMap::pixel_offset`], and a
/// caller that places source-view pixels from grid indices itself (a lookup
/// grid's contours and arrows) adds the same constant.
pub const SOURCE_PIXEL_OFFSET: f64 = 0.5;

/// Projected overlay geometry in output pixel space. `xy` is flat
/// `[x0, y0, x1, y1, …]`; `seg_lengths` gives the vertex count of each
/// visible run, so `seg_lengths.iter().sum::<u32>() * 2 == xy.len()`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProjectedPolylines {
    /// Flat `[x0, y0, x1, y1, …]` vertex coordinates in output pixel space.
    pub xy: Vec<f64>,
    /// Vertex count of each visible run, in the order the runs appear in `xy`.
    pub seg_lengths: Vec<u32>,
}

impl ProjectedPolylines {
    /// Append one pixel-space run (flat `[x0, y0, x1, y1, …]`) if it is worth
    /// stroking on a `width × height` raster, and say whether it was kept.
    ///
    /// The rule [`project_polylines`] applies to every run it emits, for a
    /// caller that builds its runs in pixel space itself (the vector arrows):
    /// at least two vertices, and a bounding box that overlaps the viewport
    /// expanded by one raster on each side. Testing the *box*, not single
    /// vertices, keeps a run whose ends both fall outside the margin but whose
    /// chord crosses the screen (a long coastline segment on a heavily zoomed
    /// window), while runs entirely off to one side are not marshalled and
    /// stroked for nothing.
    pub fn push_visible(&mut self, run: &[f64], width: u32, height: u32) -> bool {
        let (w, h) = (f64::from(width), f64::from(height));
        let (mut min_x, mut max_x) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut min_y, mut max_y) = (f64::INFINITY, f64::NEG_INFINITY);
        let vertices = run.as_chunks::<2>().0;
        for &[x, y] in vertices {
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let keep = vertices.len() >= 2
            && min_x <= 2.0 * w
            && max_x >= -w
            && min_y <= 2.0 * h
            && max_y >= -h;
        if keep {
            // Vertex count of one run, bounded by the overlay vector the host
            // handed in; the render surface counts segments in `u32`.
            self.seg_lengths.push(vertices.len() as u32);
            self.xy.extend_from_slice(&run[..2 * vertices.len()]);
        }
        keep
    }
}

/// Project `(lat, lon)` rings onto the raster described by `prepared`, of
/// size `width × height`. When `flip_y` is set the output Y is flipped to
/// match a vertically-flipped render (the `paint_grid_rgba` flip).
///
/// The seam split is the target's own decision, read from
/// [`ForwardMap::seam_split`] — geographic for the world and lat/lon-box targets,
/// a half-width pixel jump for the source projection (and a midpoint test on a
/// lookup grid, which can fold), none for the azimuthal targets. See the module
/// docs for what each means and why.
///
/// `latlon` is flat `[lat, lon, lat, lon, …]`; `ring_lengths[k]` is the
/// vertex count of ring `k`. See the module docs for the run-splitting rules.
pub fn project_polylines<P: ForwardMap + ?Sized>(
    prepared: &P,
    width: u32,
    height: u32,
    flip_y: bool,
    latlon: &[f64],
    ring_lengths: &[u32],
) -> ProjectedPolylines {
    let mut out = ProjectedPolylines::default();
    if width == 0 || height == 0 {
        return out;
    }
    let (w, h) = (width as f64, height as f64);
    let to_edge = prepared.runs_reach_domain_edge();
    let mut cursor = 0usize; // Index into `latlon`, in floats (2 per vertex).
    let mut state = RunState {
        seam: prepared.seam_split(),
        // Half the raster width: a larger jump in pixel x is a seam wrap for
        // the `PixelHalfWidth` (source) target.
        wrap_threshold: w / 2.0,
        y_top: h - 1.0,
        flip_y,
        offset: prepared.pixel_offset(),
        width,
        height,
        run: Vec::new(),
        bridge: Vec::new(),
        prev_x: None,
        prev_dlon: None,
        prev_vertex: None,
    };

    for &len in ring_lengths {
        state.restart();
        // The vertex before, while the map could not place it: where a run
        // that comes back into the domain enters from.
        let mut outside: Option<(f64, f64)> = None;
        for _ in 0..len {
            // Stop the ring if `ring_lengths` overruns `latlon` (or a final
            // `lat` has no paired `lon`) rather than fabricating a vertex.
            let (Some(&lat), Some(&lon)) = (latlon.get(cursor), latlon.get(cursor + 1)) else {
                break;
            };
            cursor += 2;

            match prepared.lonlat_to_pixel(lat, lon) {
                None => {
                    // Left the visible domain. Carry the run to its edge when
                    // the target asks, then end it.
                    if let (true, Some((inside, _))) = (to_edge, state.prev_vertex)
                        && let Some(edge) = domain_edge(prepared, inside, (lat, lon))
                    {
                        state.place(prepared, edge, &mut out);
                    }
                    state.flush(&mut out);
                    state.restart();
                    outside = Some((lat, lon));
                }
                Some(pixel) => {
                    // Came back into the domain: start the run at its edge.
                    if let (true, Some(from)) = (to_edge, outside)
                        && let Some(edge) = domain_edge(prepared, (lat, lon), from)
                    {
                        state.place(prepared, edge, &mut out);
                    }
                    outside = None;
                    state.place(prepared, ((lat, lon), pixel), &mut out);
                }
            }
        }
        state.flush(&mut out);
    }
    out
}

/// One run [`project_polylines`] is building, and what its seam tests
/// remember of the vertex before.
struct RunState {
    seam: SeamSplit,
    wrap_threshold: f64,
    y_top: f64,
    flip_y: bool,
    offset: f64,
    width: u32,
    height: u32,
    run: Vec<f64>,
    /// What a lookup grid puts between two vertices; reused across segments.
    bridge: Vec<Option<Vertex>>,
    /// The previous vertex's unflipped pixel x, for the source projection's
    /// pixel test. Reset on a domain exit so a gap is never read as a seam.
    prev_x: Option<f64>,
    /// The previous vertex's centre-relative longitude, for the world and box
    /// targets' geographic test.
    prev_dlon: Option<f64>,
    /// The previous vertex as `(lat, lon)` and its unflipped pixel, for the
    /// lookup grid's fold test and for carrying a run to the domain's edge.
    prev_vertex: Option<Vertex>,
}

impl RunState {
    /// End the run where the segment from `prev` to `next` (unflipped pixels
    /// either side of the source raster's longitude wrap) crosses the
    /// picture's side edge, and start the next run on the other side edge at
    /// the same height (#1464). The picture is one period of the wrap, so
    /// `next` moved a raster's width over is where the segment really goes,
    /// and the crossing is where that straight segment meets the edge. A
    /// segment that does not meet it that way is broken as before.
    fn carry_across_seam(
        &mut self,
        (xp, yp): (f64, f64),
        (xc, yc): (f64, f64),
        out: &mut ProjectedPolylines,
    ) {
        let w = f64::from(self.width);
        let shift = if xc < xp { w } else { -w };
        // The edge on `prev`'s side, as an undrawn pixel: the picture spans
        // `-offset..width - offset` before the offset is added.
        let edge = if shift > 0.0 {
            w - self.offset
        } else {
            -self.offset
        };
        let t = (edge - xp) / (xc + shift - xp);
        if !(0.0..=1.0).contains(&t) {
            self.flush(out);
            return;
        }
        let y = yp + t * (yc - yp);
        let (ex, ey) = self.drawn((edge, y));
        self.run.extend_from_slice(&[ex, ey]);
        self.flush(out);
        let (ex, ey) = self.drawn((edge - shift, y));
        self.run.extend_from_slice(&[ex, ey]);
    }

    /// Forget the previous vertex: a new ring, or a break in this one.
    fn restart(&mut self) {
        self.prev_x = None;
        self.prev_dlon = None;
        self.prev_vertex = None;
    }

    /// Emit the run if it is worth stroking (see
    /// [`ProjectedPolylines::push_visible`]) and start an empty one.
    fn flush(&mut self, out: &mut ProjectedPolylines) {
        out.push_visible(&self.run, self.width, self.height);
        self.run.clear();
    }

    /// Where an unflipped pixel is drawn: rows flipped to match a flipped
    /// render, then moved by the target's [`ForwardMap::pixel_offset`].
    fn drawn(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let y = if self.flip_y { self.y_top - y } else { y };
        if self.offset == 0.0 {
            // Left alone rather than `+ 0.0`, which turns a `-0.0` into `0.0`
            // and changes the bytes a host compares.
            (x, y)
        } else {
            (x + self.offset, y + self.offset)
        }
    }

    /// Add a vertex the map placed, breaking the run first where the segment
    /// to it crosses the target's seam.
    fn place<P: ForwardMap + ?Sized>(
        &mut self,
        prepared: &P,
        vertex: Vertex,
        out: &mut ProjectedPolylines,
    ) {
        let ((_, lon), (x, _)) = vertex;
        // This vertex's centre-relative longitude, for a Meridian seam.
        // `wrap_to_seam_span` is sign-preserving on the seam (#277), so a
        // vertex on ±180 keeps its rim and does not read as a full-turn jump
        // from its same-rim neighbour.
        let dlon = match self.seam {
            SeamSplit::Meridian { centre_deg } => Some(wrap_to_seam_span(lon - centre_deg, 180.0)),
            _ => None,
        };
        let crosses_seam = match self.seam {
            // A > half-turn gap between two centre-relative longitudes: the
            // segment runs the short way through the seam, not through the
            // centre.
            SeamSplit::Meridian { .. } => matches!(
                (self.prev_dlon, dlon),
                (Some(p), Some(d)) if (d - p).abs() > 180.0
            ),
            // A > half-width jump in the source's rectangular raster.
            SeamSplit::PixelHalfWidth | SeamSplit::PixelWrap | SeamSplit::Folded => {
                matches!(self.prev_x, Some(px) if (x - px).abs() > self.wrap_threshold)
            }
            // No seam: only a domain exit (the `None` arm) breaks.
            SeamSplit::None => false,
        };
        if crosses_seam {
            // Seam crossing — break before this vertex, carried to the
            // picture's two side edges where they are one meridian.
            match (self.seam, self.prev_vertex) {
                (SeamSplit::PixelWrap, Some(prev)) => self.carry_across_seam(prev.1, vertex.1, out),
                _ => self.flush(out),
            }
        } else if let (SeamSplit::Folded, Some(prev)) = (self.seam, self.prev_vertex) {
            // A lookup grid: follow the grid between the two vertices, and
            // break where the segment jumps a fold.
            let mut bridge = std::mem::take(&mut self.bridge);
            bridge.clear();
            bridge_segment(prepared, prev, vertex, FOLD_BISECTIONS, &mut bridge);
            for step in &bridge {
                match *step {
                    Some((_, pixel)) => {
                        let (bx, by) = self.drawn(pixel);
                        self.run.push(bx);
                        self.run.push(by);
                    }
                    None => self.flush(out),
                }
            }
            self.bridge = bridge;
        }
        let (dx, dy) = self.drawn(vertex.1);
        self.run.push(dx);
        self.run.push(dy);
        self.prev_x = Some(x);
        self.prev_dlon = dlon;
        self.prev_vertex = Some(vertex);
    }
}

/// How many times [`domain_edge`] halves a segment. Thirty-two halvings put
/// the edge within a four-billionth of the segment: under a millimetre on the
/// longest segment of a 1:110m layer, a ten-millionth of a pixel on a 3 km
/// grid.
const EDGE_BISECTIONS: u32 = 32;

/// Where the great circle from `inside` (a point the map places) towards
/// `outside` (one it does not) leaves the map's domain, found by halving, and
/// that point's unflipped pixel. `None` when no point past `inside` is placed,
/// or the two are antipodal and have no great circle between them.
///
/// The halving keeps the last point placed, so the edge it returns is always
/// one the map answers for: on the source projection, a point on the
/// picture's edge rather than past it.
fn domain_edge<P: ForwardMap + ?Sized>(
    prepared: &P,
    inside: (f64, f64),
    outside: (f64, f64),
) -> Option<Vertex> {
    let (ua, ub) = (
        unit_vector(inside.0, inside.1),
        unit_vector(outside.0, outside.1),
    );
    let (mut placed, mut unplaced) = (0.0f64, 1.0f64);
    let mut edge = None;
    for _ in 0..EDGE_BISECTIONS {
        let t = (placed + unplaced) / 2.0;
        let m = [0, 1, 2].map(|k| ua[k] * (1.0 - t) + ub[k] * t);
        let norm = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
        if !norm.is_finite() || norm <= 1e-12 {
            return None;
        }
        let at = (
            (m[2] / norm).clamp(-1.0, 1.0).asin().to_degrees(),
            m[1].atan2(m[0]).to_degrees(),
        );
        match prepared.lonlat_to_pixel(at.0, at.1) {
            Some(pixel) => {
                placed = t;
                edge = Some((at, pixel));
            }
            None => unplaced = t,
        }
    }
    edge
}

/// A polyline vertex as `(lat, lon)` and its unflipped pixel `(x, y)`.
type Vertex = ((f64, f64), (f64, f64));

/// Pixels a fold test forgives on top of a quarter of the segment's length. A
/// lookup grid answers with whole cells, so each end and the midpoint can sit
/// up to about 0.7 px from where a continuous map would put them.
const FOLD_SLACK_PX: f64 = 2.0;

/// The longest segment, in pixels, a fold test leaves alone. Across a fold it
/// would be a tick a few pixels long, and below it a lookup grid's own
/// ambiguity takes over: where cells overlap, as a swath's edge fields of view
/// do, the nearest cell to a point between two vertices can sit a few rows
/// away from both, which is no jump at all.
const FOLD_SHORTEST_PX: f64 = 2.0 * FOLD_SLACK_PX;

/// How many times [`bridge_segment`] may halve a segment before it gives up on
/// it as a jump. Eight halvings put up to 255 vertices between two input ones,
/// far more than a curved stretch of grid needs; a segment across a fold
/// fails at every depth, so it is the one that reaches the limit, along a
/// single path of halves (the half that does not hold the fold passes at once).
const FOLD_BISECTIONS: u32 = 8;

/// Fill `out` with what a lookup grid's raster puts between vertices `a` and
/// `b`: `Some` for a vertex to insert, in order, and `None` for a break.
/// Nothing at all when the straight pixel segment already follows the grid, or
/// is no longer than [`FOLD_SHORTEST_PX`].
///
/// The test is the segment's great-circle midpoint. On a smooth stretch of grid
/// it inverts to near the middle of the pixel segment, off it only by the
/// grid's curvature, which is second order in the segment's length; the
/// segment is drawn straight when the midpoint is within a quarter of its
/// length (plus [`FOLD_SLACK_PX`]) of the middle. Otherwise the midpoint is
/// inserted and each half tested again, so a stretch where the grid bends
/// (the bipolar patch's rows are anything but parallels) follows the bend
/// rather than cutting the corner. A segment across a fold — a tripolar grid's
/// last row is the row below it in reverse, so column `i` of one is beside
/// column `ni - 1 - i` of the other (#1430) — stays long in pixels however
/// short it gets on the ground, so it fails at every depth and becomes a
/// break. So does a midpoint the grid cannot place, and two antipodal
/// vertices, which have no midpoint.
///
/// The midpoint is taken through unit vectors, so a segment crossing the
/// antimeridian or passing near a pole needs no special case.
fn bridge_segment<P: ForwardMap + ?Sized>(
    prepared: &P,
    a: Vertex,
    b: Vertex,
    depth: u32,
    out: &mut Vec<Option<Vertex>>,
) {
    let ((ax, ay), (bx, by)) = (a.1, b.1);
    let length = (bx - ax).hypot(by - ay);
    if length <= FOLD_SHORTEST_PX {
        return;
    }
    let (ua, ub) = (unit_vector(a.0.0, a.0.1), unit_vector(b.0.0, b.0.1));
    let m = [ua[0] + ub[0], ua[1] + ub[1], ua[2] + ub[2]];
    let norm = (m[0] * m[0] + m[1] * m[1] + m[2] * m[2]).sqrt();
    if !norm.is_finite() || norm <= 1e-12 {
        out.push(None);
        return;
    }
    let mid = (
        (m[2] / norm).clamp(-1.0, 1.0).asin().to_degrees(),
        m[1].atan2(m[0]).to_degrees(),
    );
    let Some((mx, my)) = prepared.lonlat_to_pixel(mid.0, mid.1) else {
        out.push(None);
        return;
    };
    let off_middle = (mx - (ax + bx) / 2.0).hypot(my - (ay + by) / 2.0);
    if off_middle <= length / 4.0 + FOLD_SLACK_PX {
        return;
    }
    if depth == 0 {
        out.push(None);
        return;
    }
    let inserted = (mid, (mx, my));
    bridge_segment(prepared, a, inserted, depth - 1, out);
    out.push(Some(inserted));
    bridge_segment(prepared, inserted, b, depth - 1, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warp::{
        EqualEarth, Mollweide, Orthographic, Robinson, TargetProjection, TargetRaster,
    };

    /// A full-globe equirectangular raster for box-target tests.
    fn global_equirect(width: u32, height: u32) -> impl ForwardMap {
        TargetRaster {
            width,
            height,
            lat_max: 90.0,
            lat_min: -90.0,
            lon_min: -180.0,
            lon_max: 180.0,
            lon_periodic: false,
        }
        .prepare()
    }

    #[test]
    fn projects_an_interior_polyline_to_one_run() {
        let prep = global_equirect(361, 181);
        // Three points well inside the map.
        let latlon = [10.0, -20.0, 12.0, -18.0, 14.0, -16.0];
        let ring_lengths = [3u32];
        let out = project_polylines(&prep, 361, 181, false, &latlon, &ring_lengths);
        assert_eq!(out.seg_lengths, vec![3]);
        assert_eq!(out.xy.len(), 6);
        // First vertex must match the target's own forward map.
        let (x0, y0) = prep.lonlat_to_pixel(10.0, -20.0).unwrap();
        assert!((out.xy[0] - x0).abs() < 1e-9 && (out.xy[1] - y0).abs() < 1e-9);
    }

    #[test]
    fn splits_a_run_at_the_antimeridian_seam() {
        let prep = global_equirect(361, 181);
        // A segment hopping the antimeridian: lon 170 → -170 wraps edge-to-edge.
        let latlon = [0.0, 160.0, 0.0, 170.0, 0.0, -170.0, 0.0, -160.0];
        let out = project_polylines(&prep, 361, 181, false, &latlon, &[4]);
        // The wrap breaks the 4-vertex ring into two 2-vertex runs.
        assert_eq!(out.seg_lengths, vec![2, 2]);
    }

    #[test]
    fn mollweide_splits_at_the_seam_but_keeps_wide_visible_runs() {
        // Mollweide wraps longitude about its centre, so a polyline hopping the
        // ±180° seam crosses the seam meridian and must break there, exactly
        // like the lat/lon-box targets.
        let prep = Mollweide::new(360, 180, 0.0).prepare();
        let seam = [0.0, 160.0, 0.0, 170.0, 0.0, -170.0, 0.0, -160.0];
        let out = project_polylines(&prep, 360, 180, false, &seam, &[4]);
        assert_eq!(out.seg_lengths, vec![2, 2], "seam crossing must split");
        // A genuine 160°-wide equatorial segment (no seam crossing) spans less
        // than 180° of longitude, so it must stay one run rather than
        // false-split.
        let wide = [0.0, -80.0, 0.0, 80.0];
        let out = project_polylines(&prep, 360, 180, false, &wide, &[2]);
        assert_eq!(out.seg_lengths, vec![2], "wide visible run must not split");
    }

    #[test]
    fn mollweide_splits_a_high_latitude_seam_hop() {
        // The #278 bug: at 71°N Mollweide's ellipse is ~1414 px wide on a
        // 2880-wide canvas — under the old w/2 = 1440 px split threshold — so a
        // genuine rim-to-rim seam hop was NOT split and streaked across the map.
        // The geographic test splits it regardless of taper. Two vertices per
        // side so each survives the ≥2-vertex run filter.
        let (w, h) = (2880u32, 1440u32);
        let prep = Mollweide::new(w, h, 0.0).prepare();
        let hop = [71.0, 178.0, 71.0, 179.0, 71.0, -179.0, 71.0, -178.0];
        let out = project_polylines(&prep, w, h, false, &hop, &[4]);
        assert_eq!(
            out.seg_lengths,
            vec![2, 2],
            "a high-latitude seam hop must split into two runs"
        );
        // Neither run spans anything like the map: no streak survived.
        for run in [&out.xy[0..4], &out.xy[4..8]] {
            let (min_x, max_x) = run
                .as_chunks::<2>()
                .0
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), v| {
                    (lo.min(v[0]), hi.max(v[0]))
                });
            assert!(
                max_x - min_x < 0.25 * w as f64,
                "run spans {} px — a streak survived",
                max_x - min_x
            );
        }
    }

    #[test]
    fn mollweide_splits_a_recentred_seam_but_not_a_wide_run() {
        // Recentring the map (a shipped control) moves the seam off ±180 onto
        // un-clipped coastline — the general case the #278 default (centre 0)
        // hides. Centre 90°, seam at −90°: a segment crossing −90 at high
        // latitude must split; a wide segment that does not cross it must not.
        let (w, h) = (2880u32, 1440u32);
        let prep = Mollweide::new(w, h, 90.0).prepare();
        let across = [75.0, -88.0, 75.0, -89.0, 75.0, -91.0, 75.0, -92.0];
        let out = project_polylines(&prep, w, h, false, &across, &[4]);
        assert_eq!(
            out.seg_lengths,
            vec![2, 2],
            "a segment crossing the recentred seam must split"
        );
        // 10°→170° (centre-relative −80°→+80°) never crosses the −90° seam.
        let wide = [0.0, 10.0, 0.0, 170.0];
        let out = project_polylines(&prep, w, h, false, &wide, &[2]);
        assert_eq!(
            out.seg_lengths,
            vec![2],
            "a non-crossing wide run must not split"
        );
    }

    #[test]
    fn a_ring_touching_the_seam_draws_whole_and_stays_on_its_rim() {
        // Wrangel Island as Natural Earth 1:110m ships it: the antimeridian
        // clips it into a −180° piece and a +180° piece, each carrying vertices
        // *exactly* on the seam. Those must land on their own rim beside their
        // neighbours — a flip to the far rim drew a streak across the whole map
        // at ~71°N (the ellipse is ~1414 px wide there, under the w/2 = 1440
        // seam-split threshold, so the split did not mask it as it does lower
        // down). Every vertex must survive: no split, one run per ring.
        let (w, h) = (2880u32, 1440u32);
        let prep = Mollweide::new(w, h, 0.0).prepare();
        let west_piece: [f64; 14] = [
            71.516, -180.0, 71.558, -179.872, 71.556, -179.024, 71.269, -177.578, 71.133, -177.664,
            70.893, -178.694, 70.832, -180.0,
        ];
        let east_piece: [f64; 8] = [
            70.832, 180.0, 70.781, 178.903, 71.099, 178.725, 71.516, 180.0,
        ];
        // Antarctica's ring is clipped the same way and streaked the same way,
        // at −84.7°: it runs seam to seam, so *both* its ends touch one.
        let antarctic_west: [f64; 4] = [-84.713, -180.0, -84.721, -179.942];
        let antarctic_east: [f64; 4] = [-84.473, 178.277, -84.713, 180.0];
        let mid = (w as f64 - 1.0) / 2.0;

        for (label, ring, want_west_of_centre) in [
            ("wrangel −180° piece", &west_piece[..], true),
            ("wrangel +180° piece", &east_piece[..], false),
            ("antarctica −180° end", &antarctic_west[..], true),
            ("antarctica +180° end", &antarctic_east[..], false),
        ] {
            let n = (ring.len() / 2) as u32;
            let out = project_polylines(&prep, w, h, false, ring, &[n]);
            assert_eq!(
                out.seg_lengths,
                vec![n],
                "{label}: must stay one unbroken run of every vertex"
            );
            // The whole piece sits on one rim — no vertex crossed the centre.
            for v in out.xy.as_chunks::<2>().0 {
                assert_eq!(
                    v[0] < mid,
                    want_west_of_centre,
                    "{label}: vertex x={} fell on the wrong side of centre {mid}",
                    v[0]
                );
            }
            // …and it is a small island, not a streak: it spans a sliver of map.
            let (min_x, max_x) = out
                .xy
                .as_chunks::<2>()
                .0
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), v| {
                    (lo.min(v[0]), hi.max(v[0]))
                });
            assert!(
                max_x - min_x < 0.05 * w as f64,
                "{label}: spans {} px — a streak, not an island",
                max_x - min_x
            );
        }
    }

    #[test]
    fn robinson_and_equal_earth_split_at_the_seam() {
        // Same seam rule as Mollweide: these targets wrap longitude about their
        // centre, so a polyline hopping the ±180° seam jumps rim-to-rim and must
        // break there rather than smear a line back across the map. A genuine
        // wide run stays whole.
        let seam = [0.0, 160.0, 0.0, 170.0, 0.0, -170.0, 0.0, -160.0];
        let wide = [0.0, -80.0, 0.0, 80.0];

        let robin = Robinson::new(360, 180, 0.0).prepare();
        let out = project_polylines(&robin, 360, 180, false, &seam, &[4]);
        assert_eq!(out.seg_lengths, vec![2, 2], "Robinson seam must split");
        let out = project_polylines(&robin, 360, 180, false, &wide, &[2]);
        assert_eq!(out.seg_lengths, vec![2], "Robinson wide run must not split");

        let ee = EqualEarth::new(360, 180, 0.0).prepare();
        let out = project_polylines(&ee, 360, 180, false, &seam, &[4]);
        assert_eq!(out.seg_lengths, vec![2, 2], "Equal Earth seam must split");
        let out = project_polylines(&ee, 360, 180, false, &wide, &[2]);
        assert_eq!(
            out.seg_lengths,
            vec![2],
            "Equal Earth wide run must not split"
        );
    }

    #[test]
    fn flip_y_mirrors_the_output_row() {
        let prep = global_equirect(361, 181);
        let latlon = [45.0, 0.0];
        let unflipped = project_polylines(&prep, 361, 181, false, &latlon, &[1]);
        // A single-vertex run is dropped, so compare a 2-vertex run instead.
        let latlon2 = [45.0, 0.0, 45.0, 1.0];
        let plain = project_polylines(&prep, 361, 181, false, &latlon2, &[2]);
        let flipped = project_polylines(&prep, 361, 181, true, &latlon2, &[2]);
        assert!(unflipped.xy.is_empty(), "single-vertex run is dropped");
        // y_flipped = (height-1) - y_plain.
        assert!((flipped.xy[1] - (180.0 - plain.xy[1])).abs() < 1e-9);
        assert!(
            (flipped.xy[0] - plain.xy[0]).abs() < 1e-9,
            "x unaffected by flip"
        );
    }

    #[test]
    fn breaks_runs_where_the_polyline_leaves_the_visible_disc() {
        // Orthographic centred on (0, 0): a ring crossing to the antipode side.
        let t = Orthographic::new(101, 101, 0.0, 0.0);
        let prep = t.prepare();
        // Front (lon 0), back (lon 180, off-disc), front again.
        let latlon = [0.0, -10.0, 0.0, 0.0, 0.0, 180.0, 0.0, 10.0, 0.0, 20.0];
        // Azimuthal target: `seam_split` is `None`, so runs break only on
        // leaving the disc (`None` pixel), never on distance.
        let out = project_polylines(&prep, 101, 101, false, &latlon, &[5]);
        // The off-disc vertex splits the ring; the back side is dropped.
        // Front run #1 = 2 verts (lon -10, 0); front run #2 = 2 verts (lon 10, 20).
        assert_eq!(out.seg_lengths, vec![2, 2]);
    }

    #[test]
    fn culls_runs_that_project_far_outside_a_regional_window() {
        // A narrow North-America window; a polyline over Asia projects many
        // raster-widths off to the side and must be culled, not marshalled.
        let prep = TargetRaster {
            width: 80,
            height: 40,
            lat_max: 50.0,
            lat_min: 30.0,
            lon_min: -120.0,
            lon_max: -100.0,
            lon_periodic: false,
        }
        .prepare();
        let asia = [35.0, 100.0, 36.0, 101.0, 37.0, 102.0];
        let culled = project_polylines(&prep, 80, 40, false, &asia, &[3]);
        assert!(
            culled.xy.is_empty() && culled.seg_lengths.is_empty(),
            "off-window polyline should be culled"
        );
        // A polyline inside the window is still kept.
        let here = [40.0, -110.0, 41.0, -109.0, 42.0, -108.0];
        let kept = project_polylines(&prep, 80, 40, false, &here, &[3]);
        assert_eq!(kept.seg_lengths, vec![3]);
    }

    #[test]
    fn drops_runs_shorter_than_two_vertices() {
        let prep = global_equirect(361, 181);
        let out = project_polylines(&prep, 361, 181, false, &[0.0, 0.0], &[1]);
        assert!(out.xy.is_empty() && out.seg_lengths.is_empty());
    }

    #[test]
    fn source_target_projects_through_the_inverse_map() {
        use crate::projection::{LatLonParams, latlon_inverse};
        // A global 1° lat/lon grid: 361×181, north-up. Its inverse lands a
        // point at index i = lon + 180, j = 90 - lat, which the source raster
        // draws in the middle of that cell's pixel, half a pixel on (#1464).
        let p = LatLonParams {
            ni: 361,
            nj: 181,
            lat_first: 90.0,
            lon_first: -180.0,
            lat_last: -90.0,
            lon_last: 180.0,
        };
        let inverse = move |lat: f64, lon: f64| latlon_inverse(&p, lat, lon);
        let target = SourceOverlayTarget::new(&inverse, GridResampling::Any);
        let out = project_polylines(&target, 361, 181, false, &[45.0, 0.0, 45.0, 10.0], &[2]);
        assert_eq!(out.seg_lengths, vec![2]);
        assert!((out.xy[0] - 180.5).abs() < 1e-6, "lon 0 → px {}", out.xy[0]);
        assert!((out.xy[1] - 45.5).abs() < 1e-6, "lat 45 → py {}", out.xy[1]);
        assert!(
            (out.xy[2] - 190.5).abs() < 1e-6,
            "lon 10 → px {}",
            out.xy[2]
        );
    }

    /// A regional 1° grid, 0..10° in both axes, rows north to south.
    fn regional() -> crate::GridGeometry {
        crate::GridGeometry::LatLon(crate::projection::LatLonParams {
            ni: 11,
            nj: 11,
            lat_first: 10.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 10.0,
        })
    }

    #[test]
    fn a_source_run_that_leaves_the_grid_is_carried_to_the_edge() {
        // Along the parallel at 5°N from 5°E to 15°E, off the grid's east
        // edge. Through the plain inverse the run ends on the last column's
        // point, index 10, drawn at 10.5; through the inverse over the cells
        // it ends where the last column's cell does, on the picture's edge at
        // 11 (#1464). Either way it does not stop at its last vertex inside.
        let geometry = regional();
        let line = [5.0, 5.0, 5.0, 9.8, 5.0, 15.0, 5.0, 16.0];
        for (inverse, edge) in [
            (geometry.inverse_at(), 10.5),
            (geometry.cell_inverse_at(), 11.0),
        ] {
            let target = SourceOverlayTarget::new(inverse.as_ref(), GridResampling::Any);
            let out = project_polylines(&target, 11, 11, false, &line, &[4]);
            assert_eq!(out.seg_lengths, vec![3], "{:?}", out.xy);
            // The edge is where the segment's great circle crosses it, a
            // hair north of the parallel the vertices are on.
            let end = &out.xy[4..6];
            assert!(
                (end[0] - edge).abs() < 1e-5 && (end[1] - 5.5).abs() < 1e-2,
                "{end:?}"
            );
        }
        // And back in: the run starts on the edge too.
        let cells = geometry.cell_inverse_at();
        let target = SourceOverlayTarget::new(cells.as_ref(), GridResampling::Any);
        let back = [5.0, -3.0, 5.0, 2.0];
        let out = project_polylines(&target, 11, 11, false, &back, &[2]);
        assert_eq!(out.seg_lengths, vec![2], "{:?}", out.xy);
        assert!(
            out.xy[0].abs() < 1e-5 && (out.xy[2] - 2.5).abs() < 1e-9,
            "{:?}",
            out.xy
        );
    }

    #[test]
    fn a_warped_target_still_breaks_a_run_at_its_last_vertex() {
        // The default: no offset, and a run that leaves the domain stops where
        // its last placed vertex is. The orthographic globe's back is off it.
        let globe = Orthographic::new(101, 101, 0.0, 0.0).prepare();
        assert_eq!(globe.pixel_offset(), 0.0);
        assert!(!globe.runs_reach_domain_edge());
        let out = project_polylines(
            &globe,
            101,
            101,
            false,
            &[0.0, 0.0, 0.0, 60.0, 0.0, 120.0],
            &[3],
        );
        assert_eq!(out.seg_lengths, vec![2], "{:?}", out.xy);
    }

    #[test]
    fn degenerate_raster_returns_empty() {
        let prep = global_equirect(0, 181);
        let out = project_polylines(&prep, 0, 181, false, &[0.0, 0.0, 1.0, 1.0], &[2]);
        assert_eq!(out, ProjectedPolylines::default());
    }

    #[test]
    fn regional_window_west_edge_stays_continuous() {
        // A narrow window lon -120..-100 (80 px wide). A vertex 1° WEST of the
        // edge must map to a small *negative* pixel (continuous past the edge),
        // not wrap to the far right — so the segment clipping in from the west
        // is kept as one run, not split at the edge.
        let prep = TargetRaster {
            width: 80,
            height: 40,
            lat_max: 50.0,
            lat_min: 30.0,
            lon_min: -120.0,
            lon_max: -100.0,
            lon_periodic: false,
        }
        .prepare();
        let crossing = [40.0, -121.0, 40.0, -119.0];
        let out = project_polylines(&prep, 80, 40, false, &crossing, &[2]);
        assert_eq!(out.seg_lengths, vec![2], "edge-crossing run must not split");
        assert!(
            out.xy[0] < 0.0,
            "west-of-edge vertex → negative px, got {}",
            out.xy[0]
        );
        assert!(
            out.xy[2] > 0.0,
            "in-window vertex → positive px, got {}",
            out.xy[2]
        );
    }

    /// A direct geographic→pixel identity (`lon → x`, `lat → y`) so a test can
    /// place run vertices at exact pixel coordinates.
    #[derive(Clone, Copy)]
    struct IdentityPixels;
    impl ForwardMap for IdentityPixels {
        fn lonlat_to_pixel(&self, lat: f64, lon: f64) -> Option<(f64, f64)> {
            Some((lon, lat))
        }
    }

    #[test]
    fn keeps_a_chord_that_crosses_the_viewport_with_both_endpoints_off_margin() {
        // 100×100 raster; keep-margin is x,y ∈ [-100, 200]. Both endpoints fall
        // outside the margin (x = -150 and x = 250) but the chord crosses the
        // viewport, so the run must be kept (not culled vertex-by-vertex).
        // `IdentityPixels` has no seam (`seam_split` defaults to `None`), so the
        // > w/2 jump isn't treated as a seam.
        let crossing = [50.0, -150.0, 50.0, 250.0];
        let kept = project_polylines(&IdentityPixels, 100, 100, false, &crossing, &[2]);
        assert_eq!(kept.seg_lengths, vec![2], "viewport-crossing chord kept");
        // A run entirely off to one side is still culled.
        let off = [50.0, 250.0, 50.0, 300.0];
        let culled = project_polylines(&IdentityPixels, 100, 100, false, &off, &[2]);
        assert!(culled.xy.is_empty(), "run off to one side is culled");
    }

    #[test]
    fn azimuthal_target_does_not_split_far_apart_visible_points() {
        // Two visible front-hemisphere points project near opposite edges of the
        // disc (> half the raster apart). The azimuthal target's `seam_split` is
        // `None`, so it keeps them as one chord instead of mistaking the gap for
        // an antimeridian wrap (which would split + drop both as singletons).
        let prep = Orthographic::new(100, 100, 0.0, 0.0).prepare();
        let wide = [0.0, -80.0, 0.0, 80.0];
        let out = project_polylines(&prep, 100, 100, false, &wide, &[2]);
        assert_eq!(out.seg_lengths, vec![2]);
    }

    #[test]
    fn truncated_input_does_not_fabricate_a_vertex() {
        // `ring_lengths` claims 2 vertices but `latlon` holds only 1.5 pairs.
        // The dangling `lat` (30.0) has no paired `lon`, so it is dropped rather
        // than fabricating a (30, 0) vertex on the prime meridian.
        let prep = global_equirect(361, 181);
        let out = project_polylines(&prep, 361, 181, false, &[10.0, 20.0, 30.0], &[2]);
        assert!(
            out.xy.is_empty(),
            "truncated tail must not fabricate a vertex"
        );
    }

    /// The runs of `out` as lists of `(x, y)`.
    fn runs(out: &ProjectedPolylines) -> Vec<Vec<(f64, f64)>> {
        let mut start = 0;
        out.seg_lengths
            .iter()
            .map(|&n| {
                let run = out.xy[2 * start..2 * (start + n as usize)]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&[x, y]| (x, y))
                    .collect();
                start += n as usize;
                run
            })
            .collect()
    }

    /// A toy fold: the ground strip either side of the meridian 0 lies in two
    /// far-apart column ranges, as the two halves of a tripolar grid's last
    /// row do. West of 0 is columns 10..20, east of it 80..90, on a 200-wide
    /// raster, so a crossing jumps 62 px: under half the width.
    fn folded(lat: f64, lon: f64) -> Option<GridIndex> {
        let i = if lon < 0.0 { 20.0 + lon } else { 80.0 + lon };
        ((-10.0..=10.0).contains(&lon)).then_some(GridIndex { i, j: lat })
    }

    #[test]
    fn a_lookup_grid_breaks_a_segment_that_jumps_its_fold() {
        let target = SourceOverlayTarget::new(&folded, GridResampling::NearestOnly);
        let out = project_polylines(&target, 200, 10, false, &[5.0, -3.0, 5.0, 3.0], &[2]);
        let pieces = runs(&out);
        assert!(!pieces.is_empty(), "both sides of the fold still draw");
        for run in &pieces {
            for w in run.windows(2) {
                assert!(
                    (w[1].0 - w[0].0).abs() < 5.0,
                    "a segment still crosses the fold: {:?} -> {:?}",
                    w[0],
                    w[1]
                );
            }
        }
        // Each side reaches to within a pixel of the fold, so the line stops
        // where the grid does rather than short of it.
        let xs: Vec<f64> = pieces.iter().flatten().map(|p| p.0).collect();
        assert!(xs.iter().any(|&x| (x - 20.0).abs() < 1.0), "{xs:?}");
        assert!(xs.iter().any(|&x| (x - 80.0).abs() < 1.0), "{xs:?}");

        // A raster grid is never folded, and keeps the one straight segment.
        let raster = SourceOverlayTarget::new(&folded, GridResampling::Any);
        let out = project_polylines(&raster, 200, 10, false, &[5.0, -3.0, 5.0, 3.0], &[2]);
        assert_eq!(out.seg_lengths, vec![2]);
    }

    #[test]
    fn a_lookup_grid_follows_a_bend_instead_of_breaking_there() {
        // The equator from -5 to 5 maps to a parabola: both ends at row 40, the
        // middle at row 20. The straight segment would cut the corner by 20 px;
        // the run follows the curve and stays in one piece.
        let bent = |lat: f64, lon: f64| {
            Some(GridIndex {
                i: 50.0 + 5.0 * lon,
                j: 20.0 + 0.8 * lon * lon + lat,
            })
        };
        let target = SourceOverlayTarget::new(&bent, GridResampling::NearestOnly);
        let out = project_polylines(&target, 100, 60, false, &[0.0, -5.0, 0.0, 5.0], &[2]);
        let pieces = runs(&out);
        assert_eq!(pieces.len(), 1, "a bend is not a break: {pieces:?}");
        assert!(pieces[0].len() > 2, "the bend is followed: {pieces:?}");
        // Each vertex is drawn half a pixel on from its index.
        for &(x, y) in &pieces[0] {
            let lon = (x - SOURCE_PIXEL_OFFSET - 50.0) / 5.0;
            assert!(
                (y - SOURCE_PIXEL_OFFSET - (20.0 + 0.8 * lon * lon)).abs() < 1e-6,
                "({x}, {y}) is not on the curve"
            );
        }
    }

    #[test]
    fn a_lookup_grid_breaks_where_the_midpoint_is_off_the_grid() {
        // A gap in the grid's coverage between two covered vertices.
        let gappy = |lat: f64, lon: f64| {
            (lon.abs() > 0.5).then_some(GridIndex {
                i: 50.0 + 5.0 * lon,
                j: 10.0 + lat,
            })
        };
        let target = SourceOverlayTarget::new(&gappy, GridResampling::NearestOnly);
        let out = project_polylines(&target, 100, 20, false, &[0.0, -2.0, 0.0, 2.0], &[2]);
        assert!(out.xy.is_empty(), "the gap is not bridged: {:?}", out.xy);
    }

    #[test]
    fn a_fold_break_honours_the_row_flip() {
        // The inserted vertices are flipped like the input ones.
        let bent = |lat: f64, lon: f64| {
            Some(GridIndex {
                i: 50.0 + 5.0 * lon,
                j: 20.0 + 0.8 * lon * lon + lat,
            })
        };
        let target = SourceOverlayTarget::new(&bent, GridResampling::NearestOnly);
        let latlon = [0.0, -5.0, 0.0, 5.0];
        let up = project_polylines(&target, 100, 60, false, &latlon, &[2]);
        let down = project_polylines(&target, 100, 60, true, &latlon, &[2]);
        assert_eq!(up.seg_lengths, down.seg_lengths);
        for (a, b) in up
            .xy
            .as_chunks::<2>()
            .0
            .iter()
            .zip(down.xy.as_chunks::<2>().0)
        {
            assert_eq!(a[0], b[0]);
            // Row `j` is drawn at `j + 0.5`, flipped at `59 - j + 0.5`.
            assert!((a[1] - (60.0 - b[1])).abs() < 1e-9, "{a:?} vs {b:?}");
        }
    }
}
