//! The display half: warp, probe, contours, overlays and CSV, over a
//! [`GridGeometry`].
//!
//! Everything here used to live in `fieldglass-napi` and read that host's
//! `MessageMeta` DTO — a flat, every-family-at-once struct of `Option`s — which
//! meant each operation re-derived a grid's family from a string and re-read the
//! same slots to rebuild the same projector. #572 moved them here and gave them
//! `core`'s own model of a grid instead, so the rules live in one crate and both
//! hosts get the same answer.
//!
//! # What a host still owns
//!
//! Two things, and only two:
//!
//! * **Building the geometry.** A host reads its own wire fields, so the
//!   messages naming those fields ("missing `latFirst`") stay with the code that
//!   knows the names. [`Source::geometry`] carries either the geometry or the
//!   refusal.
//! * **Paint and packaging.** [`project`] returns values, a mask and the raster
//!   shape; painting them into RGBA and handing that across a language boundary
//!   is the binding's job.
//!
//! # Why these are free functions
//!
//! For the reason [`crate::Session::probe`] ignores its receiver: they read the
//! field handed to them and never the reader behind it, and a host whose handle
//! is not a [`Session`](crate::Session) — `fieldglass-napi`'s three are not —
//! must still be able to call them. [`Session`](crate::Session) forwards to each
//! one so a caller that does hold a session has them as methods.

use fieldglass_core::{
    EqualEarth, ForwardAt, GeostationaryProjector, LambertAzimuthalProjector, LambertProjector,
    LonLatBox, Mollweide, Orthographic, PlanarGridProjector, PolarStereoProjector,
    PolarStereographic, ProjectedPolylines, Resampling, Robinson, Scan, SourceGrid,
    SourceOverlayTarget, TargetRaster, TransverseMercatorProjector, WebMercator,
    colormap::{Colormap, ScaleMode, default_colormap, min_max_ignoring_mask},
    contour::{contour_segments, contour_segments_global, nice_levels},
    csv::{field_to_csv_long, field_to_csv_matrix},
    normalise_lon, project_polylines,
    projection::{GridGeometry, planar_grid_is_placeable},
    warp::{PreparedTarget, TargetProjection, WarpedRaster, warp},
};

use crate::error::Error;

// ---------------------------------------------------------------------------
// The caller's request
// ---------------------------------------------------------------------------

/// How a field should be projected and painted.
///
/// The loose strings are deliberate and are the host's wire shape: a picker
/// sends `"equirectangular"`, not an enum discriminant. [`ResolvedOptions`] is
/// where they become a closed vocabulary, and an unrecognised one is an error
/// there rather than a silent fallback — a typo should say so instead of
/// painting the wrong thing.
///
/// `#[non_exhaustive]`, like every other option struct here, so start from
/// [`Default`] and assign the fields the call needs.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct RenderOptions {
    /// Which target to paint into: `"source"`, `"equirectangular"`,
    /// `"web_mercator"`, `"orthographic"`, `"polar_stereographic"`,
    /// `"mollweide"`, `"robinson"` or `"equal_earth"`.
    pub projection: String,
    /// Preset selector for the parameterised targets. `"orthographic"` reads a
    /// centre preset (`"atlantic"` (0°N 0°E, default), `"indian"` (0°N 90°E),
    /// `"pacific"` (0°N 180°E), `"americas"` (0°N 270°E), `"north_pole"`,
    /// `"south_pole"`); `"polar_stereographic"` reads a hemisphere preset
    /// (`"north"` (default), `"south"`). Ignored by the lat/lon-box targets.
    /// `None`/unknown falls back to the default.
    ///
    /// Superseded by [`center_lat`](Self::center_lat) /
    /// [`center_lon`](Self::center_lon) when those are supplied: the free-form
    /// centre wins, the preset is only the fallback.
    pub projection_preset: Option<String>,
    /// Free-form projection centre for the azimuthal targets (degrees).
    /// `"orthographic"` reads both; `"polar_stereographic"` and the three world
    /// targets read only [`center_lon`](Self::center_lon), as their central
    /// meridian. Either field `None` falls back to the preset/default for that
    /// component.
    pub center_lat: Option<f64>,
    /// Free-form projection-centre longitude — see
    /// [`center_lat`](Self::center_lat).
    pub center_lon: Option<f64>,
    /// `"nearest"` or `"bilinear"`. A grid that is a list of cell centres
    /// downgrades to nearest whatever this says, since its index-adjacent cells
    /// need not be spatially adjacent.
    pub resampling: String,
    /// Paint row 0 at the bottom rather than the top.
    pub flip_y: bool,
    /// Low end of a manual display range. When either end is `None` the caller
    /// takes the computed min/max over the present cells.
    pub range_min: Option<f64>,
    /// High end of the manual range — see [`range_min`](Self::range_min).
    pub range_max: Option<f64>,
    /// South edge of a manual render window (degrees). All four must be `Some`
    /// and form a non-degenerate box, or the window falls back to the grid's
    /// own extent. `lon_min`/`lon_max` may lie outside [-180, 180] to describe a
    /// window that crosses the antimeridian.
    pub bounds_lat_min: Option<f64>,
    /// North edge of the manual window — see
    /// [`bounds_lat_min`](Self::bounds_lat_min).
    pub bounds_lat_max: Option<f64>,
    /// West edge of the manual window — see
    /// [`bounds_lat_min`](Self::bounds_lat_min).
    pub bounds_lon_min: Option<f64>,
    /// East edge of the manual window — see
    /// [`bounds_lat_min`](Self::bounds_lat_min).
    pub bounds_lon_max: Option<f64>,
    /// A colormap name `core` knows. `None` is the default map. An unknown name
    /// is an error rather than a silent fallback.
    pub colormap: Option<String>,
    /// Walk the colormap high-to-low. `None` is `false`.
    pub reverse_colormap: Option<bool>,
    /// `"linear"` (default) or `"log10"`. `None` is linear; anything else is an
    /// error, matching the colormap field.
    pub scale_mode: Option<String>,
}

impl RenderOptions {
    /// The two fields that have no sensible default — which target to paint
    /// into and how to resample — with every optional knob unset.
    ///
    /// A constructor rather than `Default` plus field assignment because this
    /// type is `#[non_exhaustive]`: a host cannot write a struct literal for it,
    /// and starting from [`Default`] to overwrite two required fields is the
    /// pattern `clippy::field_reassign_with_default` exists to discourage.
    pub fn new(projection: impl Into<String>, resampling: impl Into<String>) -> Self {
        Self {
            projection: projection.into(),
            resampling: resampling.into(),
            ..Self::default()
        }
    }
}

impl Default for RenderOptions {
    /// The source projection at nearest resampling: the field as stored,
    /// painted with the default colormap on a linear scale. Every knob a caller
    /// has not set is one it has not asked to change.
    fn default() -> Self {
        Self {
            projection: "source".to_string(),
            projection_preset: None,
            center_lat: None,
            center_lon: None,
            resampling: "nearest".to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
            colormap: None,
            reverse_colormap: None,
            scale_mode: None,
        }
    }
}

/// The grid a display operation reads, as the host resolved it.
///
/// [`geometry`](Self::geometry) is a `Result` rather than a [`GridGeometry`],
/// and that is the point of the type. A host builds one out of its own wire
/// fields, and a message can name a family and then fail to supply the numbers
/// for it — a §3.20 grid stating `Dx = 0`, a corner that is `NaN`. Raising that
/// at the call would refuse the operations that never place a point: the source
/// projection paints the array as stored, and a probe on it still reports the
/// value under the pixel with no coordinate to go with it. So the refusal
/// travels *with* the source and each operation decides whether it needs a
/// geometry — which is what these operations did when they read the host's DTO
/// slot by slot.
#[derive(Debug, Clone)]
pub struct Source<'a> {
    /// The grid, or why the host could not state one.
    pub geometry: Result<&'a GridGeometry, Error>,
    /// Grid columns. Stated separately because the source projection needs the
    /// raster shape from a message whose *geometry* did not resolve.
    pub ni: u32,
    /// Grid rows — see [`ni`](Self::ni).
    pub nj: u32,
    /// The scan flags the display consults, as the host read them. Only
    /// [`Scan::flips_source_rows`] is asked of this here.
    pub scan: Scan,
    /// What to call the family in a picker caption.
    ///
    /// The decoder's own name, which the geometry deliberately collapses: a
    /// reduced grid arrives widened to its regular sibling and a 2-D coordinate
    /// grid is a [`GridGeometry::Lookup`], so `geometry.label()` would name a
    /// different family than the message declares. It is also what a refusal
    /// quotes, so `reprojection not yet supported for grid type "healpix"` says
    /// the grid the file named.
    pub family: &'a str,
}

impl Source<'_> {
    /// The geometry, or the host's refusal, cloned into this crate's error.
    ///
    /// Not named `geometry`: an inherent method of that name shadows the field
    /// of that name for rustdoc's link resolver, which then reports every
    /// `[`Source::geometry`]` in this module as a link to a private item.
    fn placed(&self) -> Result<&GridGeometry, Error> {
        self.geometry.clone()
    }
}

// ---------------------------------------------------------------------------
// The resolved request
// ---------------------------------------------------------------------------

/// Validated picker state. Lifts [`RenderOptions`]'s loose strings into a
/// closed enum so the rest of the pipeline can pattern-match without silently
/// falling to defaults on a typo.
///
/// Public because a host paints from the same decision: the colormap, the
/// reversal, the scale mode and the manual range are read by the binding's own
/// paint step, and parsing them twice would let the two disagree.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ResolvedOptions {
    /// What the `projection` string resolved to.
    pub target: TargetKind,
    /// Nearest or bilinear, as asked for. A lookup grid downgrades this later.
    pub resampling: Resampling,
    /// Paint row 0 at the bottom.
    pub flip_y: bool,
    /// Low end of the manual display range.
    pub range_min: Option<f64>,
    /// High end of the manual display range.
    pub range_max: Option<f64>,
    /// The manual render window, when all four edges made a box.
    pub bounds: Option<LonLatBox>,
    /// The colormap the name resolved to.
    pub colormap: &'static Colormap,
    /// Walk the colormap high-to-low.
    pub reverse_colormap: bool,
    /// Linear or log10.
    pub scale: ScaleMode,
}

/// What the picker's `projection` string resolved to. Named `TargetKind` rather
/// than `TargetProjection` to avoid colliding with `core`'s
/// [`TargetProjection`] *trait* — this is the dispatch enum, not the per-target
/// math.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum TargetKind {
    /// Paint the source grid unchanged (no warp).
    Source,
    /// Inverse-warp into one of the geographic targets.
    Warp(WarpTarget),
}

/// The caller's manual render window, from the four optional [`RenderOptions`]
/// fields. Returns `None` unless every edge is present and they form a
/// non-degenerate box — a partially-filled or inverted box silently falls back
/// to the computed bounds, mirroring the manual-range behaviour.
fn manual_render_window(o: &RenderOptions) -> Option<LonLatBox> {
    let window = LonLatBox::new(
        o.bounds_lat_min?,
        o.bounds_lat_max?,
        o.bounds_lon_min?,
        o.bounds_lon_max?,
    );
    (window.lat_max > window.lat_min && window.lon_max > window.lon_min).then_some(window)
}

impl ResolvedOptions {
    /// Resolve every string in `options`, or say which one was not recognised.
    pub fn parse(options: &RenderOptions) -> Result<Self, Error> {
        let preset = options.projection_preset.as_deref();
        let target = match options.projection.as_str() {
            "source" => TargetKind::Source,
            "equirectangular" => TargetKind::Warp(WarpTarget::Equirectangular),
            "web_mercator" => TargetKind::Warp(WarpTarget::WebMercator),
            "orthographic" => TargetKind::Warp(orthographic_from_options(options, preset)),
            "polar_stereographic" => {
                TargetKind::Warp(polar_stereographic_from_options(options, preset))
            }
            "mollweide" => TargetKind::Warp(WarpTarget::Mollweide {
                lon0: world_central_meridian(options),
            }),
            "robinson" => TargetKind::Warp(WarpTarget::Robinson {
                lon0: world_central_meridian(options),
            }),
            "equal_earth" => TargetKind::Warp(WarpTarget::EqualEarth {
                lon0: world_central_meridian(options),
            }),
            other => {
                return Err(Error::InvalidOption {
                    detail: format!(
                        "unknown projection {other:?} (expected \"source\", \"equirectangular\", \
                         \"web_mercator\", \"orthographic\", \"polar_stereographic\", \
                         \"mollweide\", \"robinson\", or \"equal_earth\")"
                    ),
                });
            }
        };
        let resampling = match options.resampling.as_str() {
            "nearest" => Resampling::Nearest,
            "bilinear" => Resampling::Bilinear,
            other => {
                return Err(Error::InvalidOption {
                    detail: format!(
                        "unknown resampling {other:?} (expected \"nearest\" or \"bilinear\")"
                    ),
                });
            }
        };
        // An unknown colormap is an error, not a silent fallback to viridis: a
        // typo'd name should say so rather than paint the wrong colours.
        let colormap = match options.colormap.as_deref() {
            None => default_colormap(),
            Some(name) => Colormap::by_name(name).ok_or_else(|| {
                let known: Vec<&str> = fieldglass_core::colormap::colormaps()
                    .iter()
                    .map(|c| c.name())
                    .collect();
                Error::InvalidOption {
                    detail: format!(
                        "unknown colormap {name:?} (expected one of {})",
                        known.join(", ")
                    ),
                }
            })?,
        };
        // An unknown scale mode is an error for the same reason an unknown
        // colormap is: a typo should say so, not silently paint linearly.
        let scale = match options.scale_mode.as_deref() {
            None | Some("linear") => ScaleMode::Linear,
            Some("log10") => ScaleMode::Log10,
            Some(other) => {
                return Err(Error::InvalidOption {
                    detail: format!(
                        "unknown scale mode {other:?} (expected \"linear\" or \"log10\")"
                    ),
                });
            }
        };
        Ok(Self {
            target,
            resampling,
            flip_y: options.flip_y,
            range_min: options.range_min,
            range_max: options.range_max,
            bounds: manual_render_window(options),
            colormap,
            reverse_colormap: options.reverse_colormap.unwrap_or(false),
            scale,
        })
    }

    /// Whether this paints the source grid as stored rather than reprojecting.
    pub fn paints_the_source(&self) -> bool {
        matches!(self.target, TargetKind::Source)
    }
}

/// Resolve the orthographic centre. A free-form `center_lat`/`center_lon`
/// (degrees) wins per component; otherwise the named preset supplies it; with
/// neither the centre is the Atlantic view (0°N 0°E). (#71 shipped presets
/// only; #113 added the free-form centre, of which the presets are now named
/// shortcuts.)
fn orthographic_from_options(o: &RenderOptions, preset: Option<&str>) -> WarpTarget {
    let (preset_lat, preset_lon) = orthographic_preset_centre(preset);
    WarpTarget::Orthographic {
        lat0: o.center_lat.unwrap_or(preset_lat),
        lon0: o.center_lon.unwrap_or(preset_lon),
    }
}

/// The `(lat0, lon0)` of an orthographic centre preset. Unknown/`None`
/// defaults to the Atlantic view (0°N 0°E).
fn orthographic_preset_centre(preset: Option<&str>) -> (f64, f64) {
    match preset {
        Some("indian") => (0.0, 90.0),
        Some("pacific") => (0.0, 180.0),
        Some("americas") => (0.0, 270.0),
        Some("north_pole") => (90.0, 0.0),
        Some("south_pole") => (-90.0, 0.0),
        // "atlantic" / None / unknown
        _ => (0.0, 0.0),
    }
}

/// Resolve the polar-stereographic target. The pole is the hemisphere preset
/// (`"south"` ⇒ south aspect; otherwise north). `lon0` — the central meridian
/// oriented toward the bottom edge — is the free-form `center_lon` when given,
/// else 0°. (#113 added the free-form central meridian; #71 fixed it at 0°.)
fn polar_stereographic_from_options(o: &RenderOptions, preset: Option<&str>) -> WarpTarget {
    WarpTarget::PolarStereographic {
        south_pole: matches!(preset, Some("south")),
        lon0: o.center_lon.unwrap_or(0.0),
    }
}

/// The central meridian of a whole-world target (Mollweide, Robinson, Equal
/// Earth): the free-form `center_lon` when given, else 0° (Greenwich-centred).
/// These take no preset — they always show the whole globe, and recentring is
/// the only knob.
fn world_central_meridian(o: &RenderOptions) -> f64 {
    o.center_lon.unwrap_or(0.0)
}

// ---------------------------------------------------------------------------
// The targets
// ---------------------------------------------------------------------------

/// Which lat/lon → pixel target projection the warp paints into. Every one
/// shares the same source inverse map; they differ in how output pixels are
/// distributed and whether they have a lat/lon-box extent at all.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum WarpTarget {
    /// Rows linear in latitude. Honours a manual lat/lon window and echoes its
    /// geographic extent back.
    Equirectangular,
    /// Rows linear in Mercator Y, otherwise as
    /// [`Equirectangular`](Self::Equirectangular).
    WebMercator,
    /// Azimuthal target parameterised by a centre; fits a disc to a square
    /// raster and has no lat/lon-box extent to echo.
    Orthographic {
        /// Centre latitude, degrees.
        lat0: f64,
        /// Centre longitude, degrees.
        lon0: f64,
    },
    /// Azimuthal target parameterised by a hemisphere and a central meridian.
    PolarStereographic {
        /// South aspect when set, north otherwise.
        south_pole: bool,
        /// The meridian oriented toward the bottom edge, degrees.
        lon0: f64,
    },
    /// Pseudocylindrical equal-area world target parameterised by its central
    /// meridian; fits an ellipse to a 2:1 raster with no lat/lon-box extent.
    Mollweide {
        /// Central meridian, degrees.
        lon0: f64,
    },
    /// Pseudocylindrical *compromise* world target (Robinson's table), likewise
    /// parameterised by its central meridian only.
    Robinson {
        /// Central meridian, degrees.
        lon0: f64,
    },
    /// Pseudocylindrical equal-area world target (Equal Earth), likewise
    /// parameterised by its central meridian only.
    EqualEarth {
        /// Central meridian, degrees.
        lon0: f64,
    },
}

impl WarpTarget {
    /// The name a picker caption prints for this target.
    pub fn label(self) -> &'static str {
        match self {
            WarpTarget::Equirectangular => "equirectangular",
            WarpTarget::WebMercator => "web mercator",
            WarpTarget::Orthographic { .. } => "orthographic",
            WarpTarget::PolarStereographic { .. } => "polar stereographic",
            WarpTarget::Mollweide { .. } => "mollweide",
            WarpTarget::Robinson { .. } => "robinson",
            WarpTarget::EqualEarth { .. } => "equal earth",
        }
    }
}

/// A concrete, constructed render target — the same value the warp paints into
/// and the overlay projects polylines onto. Centralising construction here
/// guarantees the render and the overlay agree on the exact raster (dims,
/// clamped Mercator band, azimuthal disc side) pixel-for-pixel.
enum BuiltTarget {
    Equirect(TargetRaster),
    Mercator(WebMercator),
    Ortho(Orthographic),
    Polar(PolarStereographic),
    Moll(Mollweide),
    Robin(Robinson),
    EqEarth(EqualEarth),
}

impl BuiltTarget {
    fn dims(&self) -> (u32, u32) {
        match self {
            BuiltTarget::Equirect(t) => t.dims(),
            BuiltTarget::Mercator(t) => t.dims(),
            BuiltTarget::Ortho(t) => t.dims(),
            BuiltTarget::Polar(t) => t.dims(),
            BuiltTarget::Moll(t) => t.dims(),
            BuiltTarget::Robin(t) => t.dims(),
            BuiltTarget::EqEarth(t) => t.dims(),
        }
    }

    fn warp(&self, source: &SourceGrid<'_>, resampling: Resampling) -> WarpedRaster {
        match self {
            BuiltTarget::Equirect(t) => warp(source, t, resampling),
            BuiltTarget::Mercator(t) => warp(source, t, resampling),
            BuiltTarget::Ortho(t) => warp(source, t, resampling),
            BuiltTarget::Polar(t) => warp(source, t, resampling),
            BuiltTarget::Moll(t) => warp(source, t, resampling),
            BuiltTarget::Robin(t) => warp(source, t, resampling),
            BuiltTarget::EqEarth(t) => warp(source, t, resampling),
        }
    }

    /// Project geographic `(lat, lon)` rings onto this target's pixel space,
    /// applying `flip_y` to match a vertically-flipped render. Each target
    /// reports its own seam-split rule (`ForwardMap::seam_split`), so the only
    /// per-variant work here is preparing the concrete map.
    fn project(&self, flip_y: bool, latlon: &[f64], ring_lengths: &[u32]) -> ProjectedPolylines {
        let (w, h) = self.dims();
        match self {
            BuiltTarget::Equirect(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Mercator(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Ortho(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Polar(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Moll(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Robin(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::EqEarth(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
        }
    }

    /// The `(lat, lon)` a single output pixel maps to — the inverse of the
    /// target projection, the same map [`warp`] walks per pixel. `None` when the
    /// pixel falls off the globe (e.g. outside an azimuthal disc). `py` is in
    /// the raster's own orientation (row 0 = top), so a flipped render must
    /// un-flip the click first.
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        match self {
            BuiltTarget::Equirect(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Mercator(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Ortho(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Polar(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Moll(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Robin(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::EqEarth(t) => t.prepare().pixel_to_lonlat(px, py),
        }
    }
}

/// `(BuiltTarget, used extent)` — the concrete warp target plus the lat/lon box
/// it actually rendered (`None` for the azimuthal targets).
type BuiltWarpTarget = (BuiltTarget, Option<LonLatBox>);

/// Build the concrete [`BuiltTarget`] for a warp, returning the geographic
/// extent actually used for the lat/lon-box targets (echoed back to the UI) or
/// `None` for the azimuthal targets.
///
/// The lat/lon-box targets resolve a geographic extent (the grid's own render
/// window, possibly replaced by a manual one); the azimuthal targets fit a disc
/// to the raster, so they never ask for it — skipping the perimeter-walk bbox
/// for planar sources — and report no box extent. Output dims size to the source
/// grid for the box targets; the azimuthal discs use a square raster
/// (`side = max(ni, nj)`) so the globe stays circular rather than elliptical.
/// Every one of them is then floored by [`raise_to_min_raster`], so a coarse
/// grid's reprojection is drawn at display scale rather than at the data's
/// (#514). This is the only place that floor is applied, which is why the
/// `"source"` target — which never reaches here — keeps its native size.
///
/// `lon_periodic` says the *source* closes on itself in longitude
/// ([`GridGeometry::is_periodic_x`]), which is what decides whether a full-turn
/// window tiles as a circle or as an interval. It cannot be read back off the
/// window: a grid that declares a duplicated seam column also spans exactly
/// 360°, and wants the interval treatment.
fn build_warp_target(
    target_kind: WarpTarget,
    ni: u32,
    nj: u32,
    window_of_source: impl FnOnce() -> LonLatBox,
    bounds_override: Option<LonLatBox>,
    lon_periodic: bool,
    // Whether the source's array axes carry no geographic shape, so the box
    // targets must take theirs from the window instead ([`box_raster_dims`]).
    shapeless_axes: bool,
) -> Result<BuiltWarpTarget, Error> {
    match target_kind {
        WarpTarget::Equirectangular => {
            let window = bounds_override.unwrap_or_else(window_of_source);
            let LonLatBox {
                lat_min,
                lat_max,
                lon_min,
                lon_max,
            } = window;
            // The window's shape, then floored so the seam, the map body edge
            // and the overlays that must register against them resolve at
            // display scale rather than at the data's (#514).
            let (width, height) = raise_to_min_raster(if shapeless_axes {
                box_raster_dims(ni, nj, window)
            } else {
                (ni, nj)
            });
            let target = TargetRaster {
                width,
                height,
                lat_max,
                lat_min,
                lon_min,
                lon_max,
                lon_periodic,
            };
            Ok((BuiltTarget::Equirect(target), Some(window)))
        }
        WarpTarget::WebMercator => {
            let window = bounds_override.unwrap_or_else(window_of_source);
            let LonLatBox {
                lat_min,
                lat_max,
                lon_min,
                lon_max,
            } = window;
            // The window's shape, then floored so the seam, the map body edge
            // and the overlays that must register against them resolve at
            // display scale rather than at the data's (#514).
            let (width, height) = raise_to_min_raster(if shapeless_axes {
                box_raster_dims(ni, nj, window)
            } else {
                (ni, nj)
            });
            let merc = WebMercator::new(
                width,
                height,
                lat_min,
                lat_max,
                lon_min,
                lon_max,
                lon_periodic,
            );
            let LonLatBox {
                lat_min: used_lat_min,
                lat_max: used_lat_max,
                ..
            } = merc.extent();
            // A lat band lying entirely outside the ±85.0511° Web Mercator
            // cutoff clamps to a single edge, collapsing the Y span to zero and
            // smearing every row to one latitude. Reject it rather than emit a
            // degenerate single-row raster.
            if used_lat_max - used_lat_min <= f64::EPSILON {
                return Err(Error::InvalidOption {
                    detail: format!(
                        "Web Mercator latitude band [{lat_min}, {lat_max}] lies outside the \
                         renderable ±85.0511° range",
                    ),
                });
            }
            let used = merc.extent();
            Ok((BuiltTarget::Mercator(merc), Some(used)))
        }
        WarpTarget::Orthographic { lat0, lon0 } => {
            // Square so the globe stays circular, floored so its limb is a
            // curve rather than a staircase (#514).
            let (side, _) = raise_to_min_raster((ni.max(nj), ni.max(nj)));
            Ok((
                BuiltTarget::Ortho(Orthographic::new(side, side, lat0, lon0)),
                None,
            ))
        }
        WarpTarget::PolarStereographic { south_pole, lon0 } => {
            // Square so the globe stays circular, floored so its limb is a
            // curve rather than a staircase (#514).
            let (side, _) = raise_to_min_raster((ni.max(nj), ni.max(nj)));
            Ok((
                BuiltTarget::Polar(PolarStereographic::new(side, side, south_pole, lon0)),
                None,
            ))
        }
        WarpTarget::Mollweide { lon0 } => {
            let (w, h) = raise_to_min_raster(world_raster_dims(ni, nj, Mollweide::ASPECT_RATIO));
            Ok((BuiltTarget::Moll(Mollweide::new(w, h, lon0)), None))
        }
        WarpTarget::Robinson { lon0 } => {
            let (w, h) = raise_to_min_raster(world_raster_dims(ni, nj, Robinson::ASPECT_RATIO));
            Ok((BuiltTarget::Robin(Robinson::new(w, h, lon0)), None))
        }
        WarpTarget::EqualEarth { lon0 } => {
            let (w, h) = raise_to_min_raster(world_raster_dims(ni, nj, EqualEarth::ASPECT_RATIO));
            Ok((BuiltTarget::EqEarth(EqualEarth::new(w, h, lon0)), None))
        }
    }
}

/// Raster dims for a lat/lon-box target whose source array carries no
/// geographic shape (#515).
///
/// Every other family's array *is* its geography — a lat/lon or Gaussian grid
/// is rows of latitude by columns of longitude, so taking the raster straight
/// from `ni × nj` is not a fallback but the best answer available: the render
/// is a 1:1 copy of the field, at least until [`raise_to_min_raster`] floors a
/// coarse one. A curvilinear grid has no such correspondence.
/// A satellite swath is stored scan line by field of view, which says nothing
/// about where the pass went: a NOAA-21 half orbit is 96 × 768 for a window
/// spanning 355° of longitude by 117° of latitude, so drawing it at the array's
/// shape stretches it more than twentyfold.
///
/// So the shape comes from the window and the pixel budget from the source.
/// The longer output edge takes the source's longer edge — nothing is
/// downsampled, the same rule [`world_raster_dims`] applies to the whole-world
/// targets — and the aspect of the resolved extent fixes the other.
///
/// A degenerate window (no extent on one axis, or non-finite corners) has no
/// aspect to honour, so the source shape stands.
fn box_raster_dims(ni: u32, nj: u32, extent: LonLatBox) -> (u32, u32) {
    let geo_w = extent.lon_max - extent.lon_min;
    let geo_h = extent.lat_max - extent.lat_min;
    if !geo_w.is_finite() || !geo_h.is_finite() || geo_w <= 0.0 || geo_h <= 0.0 {
        return (ni, nj);
    }
    let aspect = geo_w / geo_h;
    let long_edge = f64::from(ni.max(nj));
    let (w, h) = if aspect >= 1.0 {
        (long_edge, long_edge / aspect)
    } else {
        (long_edge * aspect, long_edge)
    };
    // `as u32` saturates, and both edges keep at least one pixel so a sliver
    // window cannot collapse the raster to nothing.
    ((w.round() as u32).max(1), (h.round() as u32).max(1))
}

/// Raster dims for a whole-world target of the given width : height ratio.
/// Height is the source's larger edge, so nothing is downsampled, and width
/// follows from the projection's own aspect so the map body keeps its true
/// proportions — 2:1 for Mollweide, ≈1.97:1 for Robinson, ≈2.05:1 for Equal
/// Earth. These targets have no lat/lon-box extent to echo back to the UI.
///
/// The ratio is the whole of this function's job: the caller floors the result
/// with [`raise_to_min_raster`], which scales both edges together and so leaves
/// the proportions chosen here intact.
fn world_raster_dims(ni: u32, nj: u32, aspect: f64) -> (u32, u32) {
    let height = ni.max(nj);
    // A saturating `as u32` cast: `aspect` is a positive constant just under 2,
    // so an enormous source clamps at `u32::MAX` rather than wrapping to a tiny
    // raster. A zero-size source stays zero-size, as it did before.
    let width = (height as f64 * aspect).round() as u32;
    (width, height)
}

/// The shortest long edge a *reprojected* raster is drawn at (#514).
///
/// Every warp target sizes itself from the source grid, which is the right
/// instinct — nothing should be downsampled — but it was a ceiling with no
/// floor under it. A HEALPix `Nside 4` field resamples to 26 × 14 at its own
/// pixel scale, so it reprojected to a 26 × 26 orthographic disc: the data's
/// edge is a staircase while the coastline overlay is a smooth curve on the
/// projection's true limb, which reads as missing data around the rim, and the
/// exported PNG is a postage stamp.
///
/// 720 is the same 0.5° the two synthesis paths already pin to, and the history
/// is worth restating here: the spectral render met this exact symptom first —
/// "a postage-stamp render whose PNG exported 382 pixels wide" — and settled on
/// 0.5°. The HEALPix render then borrowed that number as its *cap* without the
/// floor that motivated it. This is the floor, applied where every warped target
/// passes through rather than per grid family, so a coarse grid of any origin is
/// covered.
///
/// Upsampling adds no information about the *field*. What it buys is that the
/// projection's own geometry — the limb, the seam, the map body outline — is
/// sampled at display scale instead of at the data's, so the overlays register
/// against it. The source projection is deliberately excluded: it never reaches
/// a warp target at all, and there blocky is the honest view of the
/// data.
pub const MIN_REPROJECTED_LONG_EDGE: u32 = 720;

/// Raise `dims` until its long edge reaches [`MIN_REPROJECTED_LONG_EDGE`],
/// keeping the ratio the caller chose.
///
/// The scale is uniform because every arm of [`build_warp_target`] has already
/// decided the aspect it wants — the source's shape, the window's, the
/// projection's, or a square for the azimuthal discs — and the floor's business
/// is density, not shape. It only ever raises, so no real forecast grid moves:
/// GFS is 1440 wide, HRRR 1799.
///
/// The result is bounded. The long edge lands on exactly the floor and the
/// short edge cannot exceed it, so this can never ask for more than 720 × 720
/// pixels however extreme the aspect. A zero-size raster stays zero-size, as
/// [`world_raster_dims`] promises.
fn raise_to_min_raster(dims: (u32, u32)) -> (u32, u32) {
    let (width, height) = dims;
    let long_edge = width.max(height);
    if width == 0 || height == 0 || long_edge >= MIN_REPROJECTED_LONG_EDGE {
        return dims;
    }
    let scale = f64::from(MIN_REPROJECTED_LONG_EDGE) / f64::from(long_edge);
    // Both edges keep at least one pixel: the short edge of an extreme aspect
    // rounds to zero long before the long edge reaches the floor.
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

// ---------------------------------------------------------------------------
// The source geometry, as the warp reads it
// ---------------------------------------------------------------------------

/// The inverse map and the lazy render window a warp needs from a geometry, or
/// the reason this grid cannot be reprojected at all.
///
/// This is what the nine per-family `*_warp_setup` functions in the napi host
/// collapsed into (#572). Each of them read the host's DTO, rebuilt one
/// family's parameters and then built the same two things; `core` now models
/// the grid, so there is one dispatch and it is the enum's own.
///
/// The gate below is deliberately **`planar_grid_is_placeable`**, not the
/// weaker `is_well_defined` the warp setups asked. A §3.20 corner at the far
/// pole puts the plane origin at 1.9e23 m, where adding a 60 km step is a no-op
/// in `f64`: the constants are all well defined, and `inverse` then declines
/// every pixel of the grid. It is the same predicate
/// [`GridGeometry::forward`], [`GridGeometry::lonlat_bbox`] and
/// [`GridGeometry::reprojectable`] gate on, so a grid this accepts is one all
/// four answer for.
fn require_reprojectable(geometry: &GridGeometry, family: &str) -> Result<(), Error> {
    let placeable = |ok: bool, proj: &dyn PlanarGridProjector| {
        planar_grid_is_placeable(ok, proj).then_some(()).ok_or({
            Error::Unsupported {
                detail: format!(
                    "grid type {:?} declares degenerate projection parameters, so its \
                     grid points have no geographic position",
                    geometry.kind()
                ),
            }
        })
    };
    match geometry {
        GridGeometry::Lambert(p) => {
            let proj = LambertProjector::new(*p);
            placeable(proj.is_well_defined(), &proj)
        }
        GridGeometry::PolarStereo(p) => {
            let proj = PolarStereoProjector::new(*p);
            placeable(proj.is_well_defined(), &proj)
        }
        GridGeometry::TransverseMercator(p) => {
            let proj = TransverseMercatorProjector::new(*p);
            placeable(proj.is_well_defined(), &proj)
        }
        GridGeometry::LambertAzimuthal(p) => {
            let proj = LambertAzimuthalProjector::new(*p);
            placeable(proj.is_well_defined(), &proj)
        }
        // The space view's analogue. Its grid is scan angles rather than metres,
        // so it needs no floor under the radius — the maths is all ratios and
        // shrinking the whole system changes nothing — but a shapeless
        // ellipsoid leaves it nothing to intersect. Say which numbers, rather
        // than letting the whole disc render transparent (#610).
        GridGeometry::Geostationary(p) => {
            if GeostationaryProjector::new(*p).is_well_defined() {
                Ok(())
            } else {
                Err(Error::Unsupported {
                    detail: format!(
                        "the message declares an Earth of r_eq = {} m and r_pol = {} m viewed \
                         from {} m (geosREq / geosRPol / geosHeight), which describes no body \
                         for the satellite's line of sight to meet",
                        p.r_eq, p.r_pol, p.h_metres
                    ),
                })
            }
        }
        GridGeometry::Unsupported { .. } => Err(Error::Unsupported {
            detail: format!("reprojection not yet supported for grid type {family:?}"),
        }),
        _ => Ok(()),
    }
}

/// The render window a warp target frames a grid with, from `core`.
///
/// [`GridGeometry::render_window`] is the whole rule: the grid's own extent,
/// carried a full turn east when the grid is periodic *and* its columns advance
/// eastward in geographic longitude, so the wrap column at the eastern edge is
/// painted too (the periodic sampler fills it). `lon_max` may therefore exceed
/// 360°, which the warp targets accept — query longitudes wrap to the nearest
/// 360° multiple.
///
/// A grid with no extent falls back to the whole globe. Before #572 the four
/// planar families reached this through `PlanarGridProjector::lonlat_bbox`,
/// whose empty-box fallback is `(0, 0, 0, 0)` — a window on null island rather
/// than a refusal — while `core` went through `placed_bbox`, which declines a
/// grid whose perimeter projects nowhere. `core`'s route is the one that
/// survived: [`require_reprojectable`] has already refused the grid whose plane
/// has collapsed, so what is left here is the grid whose perimeter walk found
/// no point on Earth at all, and framing the globe for it is both harmless (its
/// pixels all invert to `None`) and more useful than framing a degenerate box
/// at 0°N 0°E. It is also what the lookup and geostationary setups already did.
fn geometry_render_window(geometry: &GridGeometry) -> LonLatBox {
    geometry
        .render_window()
        .unwrap_or(LonLatBox::new(-90.0, 90.0, -180.0, 180.0))
}

// ---------------------------------------------------------------------------
// Projection: the source paint and the warp
// ---------------------------------------------------------------------------

/// One projection stage's output: the resampled values, where they are, and
/// what the pipeline did to get them.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Projected {
    /// Values in output-raster order, row-major from the top-left. A masked
    /// cell is `NaN` in the source view and unspecified elsewhere; read `mask`.
    pub values: Vec<f64>,
    /// One byte per output pixel: `1` present, `0` off-grid or masked.
    pub mask: Vec<u8>,
    /// Output columns.
    pub width: u32,
    /// Output rows.
    pub height: u32,
    /// `[lat_min, lat_max, lon_min, lon_max]` of the window actually rendered,
    /// for the lat/lon-box targets. `None` for the source view and the
    /// azimuthal targets, which have no box extent.
    pub bounds: Option<[f64; 4]>,
    /// The source → target chain, for a picker caption.
    pub summary: String,
}

/// `source: {family} {ni}×{nj}`, the left-hand side of every summary.
fn source_projection_summary(source: &Source<'_>) -> String {
    format!("source: {} {}×{}", source.family, source.ni, source.nj)
}

/// Project a decoded field into the target `options` names.
///
/// The dispatch the napi host's `render_with_options` used to make inline: the
/// `"source"` target paints the array as stored, everything else inverse-warps
/// through the geometry. Painting the result is the caller's — the values and
/// the mask come back so a GPU host never pays for a CPU paint it discards.
pub fn project(
    source: &Source<'_>,
    values: &[Option<f64>],
    options: &RenderOptions,
) -> Result<Projected, Error> {
    let resolved = ResolvedOptions::parse(options)?;
    match resolved.target {
        TargetKind::Source => Ok(paint_source(source, values)),
        TargetKind::Warp(target) => warp_field(source, values, target, &resolved),
    }
}

/// Source-projection paint: copy the decoded values into a buffer the same
/// shape as the source grid. `NaN` encodes a masked cell, matching the buffer
/// the painter consumes.
///
/// The only operation that needs no geometry — it paints grid point `(i, j)` at
/// pixel `(i, j)` and never asks where that is — which is why a grid whose
/// projection parameters did not resolve still renders here.
fn paint_source(source: &Source<'_>, raw: &[Option<f64>]) -> Projected {
    let n = (source.ni as usize).saturating_mul(source.nj as usize);
    let mut values = vec![0.0f64; n];
    let mut mask = vec![0u8; n];
    for (i, v) in raw.iter().enumerate().take(n) {
        if let Some(x) = v {
            values[i] = *x;
            mask[i] = 1;
        } else {
            values[i] = f64::NAN;
        }
    }
    // Right-hand side names the actual source projection (e.g. "latlon",
    // "lambert", "polar_stereo") so the picker caption reads
    // `source: latlon 240×121 → latlon (no reprojection)`, mirroring the
    // equirectangular shape but making it explicit *what* the source projection
    // is rather than just labelling it "source projection".
    let summary = format!(
        "{} → {} (no reprojection)",
        source_projection_summary(source),
        source.family,
    );
    Projected {
        values,
        mask,
        width: source.ni,
        height: source.nj,
        // No geographic extent for the source-projection target.
        bounds: None,
        summary,
    }
}

/// Inverse-warp a field into one of the geographic targets.
fn warp_field(
    source: &Source<'_>,
    raw: &[Option<f64>],
    target_kind: WarpTarget,
    resolved: &ResolvedOptions,
) -> Result<Projected, Error> {
    let geometry = source.placed()?;
    require_reprojectable(geometry, source.family)?;
    let (ni, nj) = (source.ni, source.nj);

    let sample = |i: usize, j: usize| -> Option<f64> {
        let k = j * ni as usize + i;
        raw.get(k).copied().flatten()
    };
    let sample_ref: &dyn Fn(usize, usize) -> Option<f64> = &sample;
    let inverse = geometry.inverse_at();

    let grid = SourceGrid {
        ni,
        nj,
        sample: sample_ref,
        inverse_at: inverse.as_ref(),
        periodic_i: geometry.is_periodic_x(),
        // A lookup grid answers with a cell, not a position inside one, and its
        // index-adjacent cells need not be spatially adjacent — a tripolar grid
        // folds. `warp` downgrades a bilinear request against it rather than
        // blending across the fold (#445).
        resampling: geometry.resampling(),
    };
    // Construct the concrete target (shared with the overlay-projection path so
    // both paint into byte-identical geometry), then warp the source into it.
    let (built, used_bounds) = build_warp_target(
        target_kind,
        ni,
        nj,
        || geometry_render_window(geometry),
        resolved.bounds,
        geometry.is_periodic_x(),
        matches!(geometry, GridGeometry::Lookup(_)),
    )?;
    let warped = built.warp(&grid, resolved.resampling);
    // What the warp actually did, not what was asked for: a lookup grid
    // downgrades bilinear, and a summary echoing the request would name a blend
    // that never happened (#445).
    let resample_label = match Resampling::from_grid(grid.resampling, resolved.resampling) {
        Resampling::Nearest => "nearest",
        Resampling::Bilinear => "bilinear",
    };
    let summary = format!(
        "{} → {} ({resample_label})",
        source_projection_summary(source),
        target_kind.label(),
    );
    Ok(Projected {
        values: warped.values,
        mask: warped.mask,
        width: warped.width,
        height: warped.height,
        bounds: used_bounds.map(LonLatBox::to_array),
        summary,
    })
}

// ---------------------------------------------------------------------------
// Overlay projection
// ---------------------------------------------------------------------------

/// Project geographic `(lat, lon)` polylines onto the warped raster for a
/// field, producing pixel-space runs for a render panel's overlay layer (#72).
/// Geometry-only: it rebuilds the *same* target the warp paints into, through
/// the same constructor, but never decodes or samples values.
///
/// `latlon` is flat `[lat, lon, …]`; `ring_lengths[k]` is the vertex count of
/// input ring `k`. See [`fieldglass_core::project_polylines`] for the
/// run-splitting (visibility / antimeridian) rules.
///
/// The `"source"` projection paints grid point `(i, j)` at pixel `(i, j)`, so
/// the warp's own inverse map (lat/lon → fractional grid index) doubles as its
/// forward pixel map — the overlay projects straight through it
/// ([`SourceOverlayTarget`]), no separate geographic forward projection needed.
pub fn overlay_polylines(
    source: &Source<'_>,
    options: &RenderOptions,
    latlon: &[f64],
    ring_lengths: &[u32],
) -> Result<ProjectedPolylines, Error> {
    let resolved = ResolvedOptions::parse(options)?;
    let geometry = source.placed()?;
    require_reprojectable(geometry, source.family)?;
    let inverse = geometry.inverse_at();
    let (ni, nj) = (source.ni, source.nj);
    match resolved.target {
        // A source grid can wrap longitude (a global grid's seam, or the cut
        // meridian of a projected grid); `SourceOverlayTarget::seam_split`
        // returns `PixelHalfWidth`, so a raster-width jump breaks the run. On a
        // regional grid, out-of-coverage vertices invert to `None` and break
        // runs there instead.
        // The source raster is flipped to face north-up by default (#286); the
        // overlay must ride the same flip so coastlines track the field.
        TargetKind::Source => Ok(project_polylines(
            &SourceOverlayTarget::new(inverse.as_ref()),
            ni,
            nj,
            source.scan.flips_source_rows(resolved.flip_y),
            latlon,
            ring_lengths,
        )),
        TargetKind::Warp(target_kind) => {
            let (built, _used_bounds) = build_warp_target(
                target_kind,
                ni,
                nj,
                || geometry_render_window(geometry),
                resolved.bounds,
                geometry.is_periodic_x(),
                matches!(geometry, GridGeometry::Lookup(_)),
            )?;
            Ok(built.project(resolved.flip_y, latlon, ring_lengths))
        }
    }
}

// ---------------------------------------------------------------------------
// Forward geolocation: grid point → (lat, lon)
// ---------------------------------------------------------------------------

// A grid's forward map is `core`'s [`ForwardAt`]: grid index `(i, j)` →
// `(lat, lon)`, or `None` for a point the projection cannot place, over a
// projector built once. What this module adds to it is below.

/// The grid families that carry a forward geolocation map, each paired with the
/// name it goes by in user-facing prose.
///
/// One table, two jobs: it is what [`geolocatable_families`] reads out in every
/// "unsupported grid" message, and it is the list [`forward_geolocation`]'s
/// dispatch is held to — so the prose and the dispatch can no longer drift apart
/// the way they did before #470 (contours and long CSV both refused the planar
/// grids their projectors had geolocated since #422/#423).
///
/// The keys are the decoder's own family names rather than
/// [`GridGeometry::kind`], because that is what a message quotes and what a user
/// reads. Two of them — the reduced pair — reach a geometry already widened to
/// their regular sibling, and `"curvilinear"` reaches it as
/// [`GridGeometry::Lookup`]; `the_geolocatable_table_matches_the_dispatch` holds
/// the two spellings together.
///
/// Two families are absent on purpose. Space view (§3.90) has grid points off
/// the disc entirely, which have no geographic position at all, and a family
/// this build does not model has none either.
const GEOLOCATABLE_GRIDS: &[(&str, &str)] = &[
    ("latlon", "regular lat/lon"),
    ("mercator", "Mercator"),
    ("rotated_latlon", "rotated lat/lon"),
    ("gaussian", "Gaussian"),
    // A reduced grid reaches every consumer already widened to a regular
    // raster, whose columns are evenly spaced by construction — so the forward
    // map is its family's, on the derived geometry the decoder supplies from
    // the GDS's own `raster_bounds()`. GRIB2 has always taken this route,
    // reporting a reduced Gaussian grid as `"gaussian"`; GRIB1 names its
    // reduced grids and was refused here while its *inverse* map already
    // treated them the same way, so the image and the contours over it
    // disagreed about one grid (#503).
    ("reduced_latlon", "reduced lat/lon"),
    ("reduced_gaussian", "reduced Gaussian"),
    // A 2-D coordinate grid geolocates from its own cell centres rather than a
    // formula, which is a different mechanism but the same answer shape (#445).
    ("curvilinear", "2-D coordinate (curvilinear)"),
    ("lambert", "Lambert conformal"),
    ("polar_stereo", "polar stereographic"),
    ("transverse_mercator", "transverse Mercator"),
    ("lambert_azimuthal", "Lambert azimuthal equal-area"),
];

/// [`GEOLOCATABLE_GRIDS`] as an Oxford-comma list ("a, b, and c") for the
/// "unsupported grid" messages.
fn geolocatable_families() -> String {
    let names: Vec<&str> = GEOLOCATABLE_GRIDS.iter().map(|(_, name)| *name).collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{}, and {last}", rest.join(", ")),
    }
}

/// Build the forward geolocation closure `(i, j) → (lat, lon)` for a geometry
/// whose family carries one ([`GEOLOCATABLE_GRIDS`]), or `None` for one that
/// does not.
///
/// [`GridGeometry::forward_at`] is the map; this adds the two things the
/// display needs on top of it.
///
/// **The gate.** Space view (§3.90) has grid points off the disc entirely,
/// which have no geographic position at all, and a family this build does not
/// model has no map. Those are the two `None`s, and they are what the contour
/// and long-CSV refusals are worded from.
///
/// **The normalisation, for the planar families only.** A planar inverse
/// reports `lov ± 180°`, so the CMC grid (`lov` = 247°) would otherwise export
/// points at longitude 427. The geographic families are deliberately left alone:
/// a global lat/lon grid published from 0° runs its columns to 359°, and pulling
/// that last column back to -1° would break the monotonic sweep
/// [`forward_bilinear`] interpolates along.
fn forward_geolocation(geometry: &GridGeometry) -> Option<ForwardAt<'_>> {
    let place = geometry.forward_at();
    match geometry {
        // Space view has grid points off the disc entirely; an unmodelled family
        // has no map at all.
        //
        // The wildcard below is forced: `GridGeometry` is `#[non_exhaustive]`,
        // so a `match` in this crate cannot be exhaustive over it. A family
        // added there without an arm here would read as "no forward map" rather
        // than failing to compile, which is why
        // `the_geolocatable_table_matches_the_dispatch` walks
        // [`GEOLOCATABLE_GRIDS`] and asserts each entry really has one.
        GridGeometry::Geostationary(_) | GridGeometry::Unsupported { .. } => None,
        GridGeometry::Lambert(_)
        | GridGeometry::PolarStereo(_)
        | GridGeometry::TransverseMercator(_)
        | GridGeometry::LambertAzimuthal(_) => Some(Box::new(move |i, j| {
            place(i, j).map(|(lat, lon)| (lat, normalise_lon(lon)))
        })),
        GridGeometry::LatLon(_)
        | GridGeometry::Gaussian(_)
        | GridGeometry::Mercator(_)
        | GridGeometry::RotatedLatLon(_)
        | GridGeometry::Lookup(_) => Some(place),
        _ => None,
    }
}

/// The forward map, or the caller's own refusal for a family that has none.
///
/// `unsupported` supplies the message given the family name — so the shared gate
/// reads "contours not yet supported…" for the contour path and points long-CSV
/// callers at the Matrix layout, instead of one feature's hard-coded wording
/// leaking into the others (#337).
fn require_forward_geolocation<'a>(
    source: &'a Source<'_>,
    unsupported: impl Fn(&str) -> String,
) -> Result<ForwardAt<'a>, Error> {
    let geometry = source.placed()?;
    let map = forward_geolocation(geometry).ok_or_else(|| Error::Unsupported {
        detail: unsupported(source.family),
    })?;
    // The family gate first, the grid's own constants second, and the order is
    // load-bearing: a space view is refused for what its *family* cannot do
    // whatever its numbers say, so it must not be told instead that its
    // parameters are degenerate. Only a family that has a map at all gets asked
    // whether this grid's projection resolves.
    //
    // Without this the refusal would be silent rather than absent: every point
    // of an unplaceable planar grid comes back `None` from the map above, so a
    // contour pass would draw nothing and a long CSV would write a header and
    // no rows, with nothing said about why (#603, #610).
    require_reprojectable(geometry, source.family)?;
    Ok(map)
}

/// `(lat, lon)` of a fractional grid position, bilinearly interpolated from the
/// four surrounding integer grid points via `forward`. A contour vertex sits on
/// a cell edge (one integer coordinate, one fractional), for which the bilinear
/// collapses to a linear interpolation along that edge. Longitudes come from the
/// forward map in the grid's own frame, which is monotonic within a cell for
/// the corner-pinned families. Where it is not — a rotated or planar grid whose
/// own ±180° cut runs through the cell — the corners are pulled onto a common
/// turn first; see [`cell_crosses_lon_cut`].
fn forward_bilinear(
    forward: &dyn Fn(u32, u32) -> Option<(f64, f64)>,
    ni: u32,
    nj: u32,
    fi: f64,
    fj: f64,
    periodic_i: bool,
) -> Option<(f64, f64)> {
    if ni < 2 || nj < 2 {
        return None;
    }
    // On a periodic grid a seam vertex sits in `(ni - 1, ni)`: its west corner
    // is the last column and its east corner is column 0 again. Saturating `i0`
    // at `ni - 2` the way a bounded grid does would drag both corners back onto
    // the last two columns, collapsing the whole seam cell onto the last column.
    let seam = periodic_i && fi > (ni - 1) as f64;
    let (i0, i1) = if seam {
        (ni - 1, 0)
    } else {
        let i0 = (fi.floor().max(0.0) as u32).min(ni - 2);
        (i0, i0 + 1)
    };
    let j0 = (fj.floor().max(0.0) as u32).min(nj - 2);
    let fx = (fi - i0 as f64).clamp(0.0, 1.0);
    let fy = (fj - j0 as f64).clamp(0.0, 1.0);
    let a = forward(i0, j0)?;
    let mut b = forward(i1, j0)?;
    let mut c = forward(i0, j0 + 1)?;
    let mut d = forward(i1, j0 + 1)?;
    // Either branch below leaves a corner longitude outside [-180, 180), so
    // remember whether one ran and normalise only then.
    let mut unwrapped = seam;
    if seam {
        // Column 0 lies one full turn east of the last column, not `east_span`
        // degrees west of it. Without the +360 the interpolation sweeps
        // backwards across the entire map and the "seam" segment is drawn as a
        // streak from one rim to the other.
        b.1 = unwrap_east_of(a.1, b.1);
        d.1 = unwrap_east_of(c.1, d.1);
    } else if cell_crosses_lon_cut([a.1, b.1, c.1, d.1]) {
        // The forward map's own ±180° cut runs through this cell: one corner
        // reads +179.9 and its neighbour a few kilometres away reads -179.9. A
        // vertex interpolated between them lands near 0°, drawing the same
        // rim-to-rim streak the seam branch exists to prevent. Planar grids meet
        // this on any Pacific-crossing domain (the CMC polar grid's top row runs
        // straight across the antimeridian); pull the other three corners onto
        // whichever turn is nearest `a` and the cell is contiguous again.
        b.1 = unwrap_near(a.1, b.1);
        c.1 = unwrap_near(a.1, c.1);
        d.1 = unwrap_near(a.1, d.1);
        unwrapped = true;
    }
    let bilerp = |va: f64, vb: f64, vc: f64, vd: f64| {
        let top = va + (vb - va) * fx;
        let bot = vc + (vd - vc) * fx;
        top + (bot - top) * fy
    };
    let lat = bilerp(a.0, b.0, c.0, d.0);
    let lon = bilerp(a.1, b.1, c.1, d.1);
    // Only an unwrapped cell can push a longitude past ±180 (the branches above
    // deliberately move a corner a whole turn), so normalise just those. Every
    // other vertex keeps the exact value it had before, which leaves the
    // overwhelmingly common path bit-for-bit unchanged.
    Some((lat, if unwrapped { normalise_lon(lon) } else { lon }))
}

/// Whether a grid cell's four corner longitudes straddle the ±180° cut.
///
/// No real grid cell spans half the globe, so a corner spread wider than 180°
/// is the cut running through the cell, not geometry — the giveaway that the
/// corners sit on different turns and must be unwrapped before interpolating.
/// (The one grid whose cell genuinely reaches that far is a polar one at the
/// pole itself, where longitude is degenerate anyway and the nearest-turn
/// unwrap still keeps the vertex beside its corners.)
fn cell_crosses_lon_cut(lons: [f64; 4]) -> bool {
    let (min, max) = lons
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &l| {
            (lo.min(l), hi.max(l))
        });
    max - min > 180.0
}

/// `lon` moved onto whichever turn sits nearest `from` — i.e. `from + delta`
/// where `delta ∈ [-180, 180)`. The two-sided counterpart of
/// [`unwrap_east_of`], for a cut that a cell straddles in either direction.
fn unwrap_near(from: f64, lon: f64) -> f64 {
    from + normalise_lon(lon - from)
}

/// `lon` expressed as the first value east of `from` — i.e. `from + delta`
/// where `delta ∈ [0, 360)`. Interpolating from the last column to column 0
/// across the seam needs the eastern corner to read (say) 360.0 rather than
/// 0.0, so the sweep is the quarter-degree gap and not 359.75° the wrong way.
fn unwrap_east_of(from: f64, lon: f64) -> f64 {
    from + (lon - from).rem_euclid(360.0)
}

// ---------------------------------------------------------------------------
// Contours
// ---------------------------------------------------------------------------

/// Contour levels at every multiple of `step` strictly inside `(min, max)`. The
/// manual-interval override; guarded against a tiny step producing an unbounded
/// list.
fn levels_by_interval(min: f64, max: f64, step: f64) -> Vec<f64> {
    if step <= 0.0 || !step.is_finite() || min >= max {
        return Vec::new();
    }
    let start = (min / step).ceil() * step;
    let mut levels = Vec::new();
    let mut k = 0i64;
    loop {
        let v = start + k as f64 * step;
        k += 1;
        if v <= min {
            continue;
        }
        if v >= max {
            break;
        }
        levels.push(v);
        if levels.len() > 2000 {
            break;
        }
    }
    levels
}

/// Extract contour isolines from a decoded field and project them onto the same
/// raster the render and the overlay use, returning pixel-space runs (#238).
/// Levels come from `interval` (a manual spacing) when given and positive, else
/// from [`nice_levels`] over the used range. The grid-space isolines are
/// geolocated through the family's forward map and then run through
/// [`overlay_polylines`], so they land on every target projection with the same
/// visibility and seam handling as the coastlines.
pub fn contour_polylines(
    source: &Source<'_>,
    values: &[Option<f64>],
    options: &RenderOptions,
    interval: Option<f64>,
) -> Result<ProjectedPolylines, Error> {
    let forward = require_forward_geolocation(source, |gt| {
        format!(
            "contours not yet supported for grid type {gt:?} (only {} for now)",
            geolocatable_families()
        )
    })?;
    let (ni, nj) = (source.ni, source.nj);

    // Levels span the same range the image is painted over, so contours line up
    // with the colours: a manual range override wins, else the present-cell
    // min/max.
    let (used_min, used_max) = match (options.range_min, options.range_max) {
        (Some(min), Some(max)) if max > min => (min, max),
        _ => min_max_ignoring_mask(values.iter().copied()).unwrap_or((0.0, 1.0)),
    };
    let levels = match interval {
        Some(step) if step > 0.0 => levels_by_interval(used_min, used_max, step),
        _ => nice_levels(used_min, used_max, 8),
    };

    // Each contour segment becomes a two-vertex ring in `(lat, lon)` order (what
    // `project_polylines` consumes); a vertex that can't be geolocated drops its
    // segment rather than the whole contour.
    //
    // `periodic_x` is the wrong question here and `contour_seam_wraps` is the
    // right one: the tracer may only unwrap the seam where geographic longitude
    // advances uniformly eastward with `i`, which a rotated grid's rows — small
    // circles — do not (#571).
    let periodic_i = source.placed()?.contour_seam_wraps();
    let contours = if periodic_i {
        contour_segments_global(values, ni as usize, nj as usize, &levels)
    } else {
        contour_segments(values, ni as usize, nj as usize, &levels)
    };
    let mut latlon: Vec<f64> = Vec::new();
    let mut ring_lengths: Vec<u32> = Vec::new();
    for level in &contours {
        for seg in &level.segments {
            let p0 = forward_bilinear(forward.as_ref(), ni, nj, seg[0].0, seg[0].1, periodic_i);
            let p1 = forward_bilinear(forward.as_ref(), ni, nj, seg[1].0, seg[1].1, periodic_i);
            if let (Some((lat0, lon0)), Some((lat1, lon1))) = (p0, p1) {
                latlon.extend_from_slice(&[lat0, lon0, lat1, lon1]);
                ring_lengths.push(2);
            }
        }
    }

    overlay_polylines(source, options, &latlon, &ring_lengths)
}

// ---------------------------------------------------------------------------
// Probe
// ---------------------------------------------------------------------------

/// The result of probing one output pixel (#172): the geographic point under
/// the pixel, the source grid cell it fell on, and the decoded value there.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct PixelProbe {
    /// Latitude under the pixel (degrees). `None` when the grid can't be
    /// geolocated (a source-projection view of a grid whose forward map isn't
    /// wired); the value is still reported.
    pub lat: Option<f64>,
    /// Longitude (degrees, normalised to `[-180, 180)`).
    pub lon: Option<f64>,
    /// The decoded value at the grid cell, or `None` when the pixel fell off the
    /// grid or onto a masked cell.
    pub value: Option<f64>,
    /// The source grid column the pixel resolved to; `None` off-grid.
    pub grid_i: Option<i32>,
    /// The source grid row the pixel resolved to; `None` off-grid.
    pub grid_j: Option<i32>,
}

/// Sample the field under one output pixel `(px, py)` in the *displayed* raster
/// (post-`flip_y`). Reproduces the warp's per-pixel map — output pixel →
/// `(lat, lon)` → source grid index → value — so the readout matches exactly
/// what the image shows. Returns `None` when the pixel is off the raster or off
/// the globe (outside an azimuthal disc), so there is nothing to report.
pub fn probe_pixel(
    source: &Source<'_>,
    values: &[Option<f64>],
    options: &RenderOptions,
    px: u32,
    py: u32,
) -> Result<Option<PixelProbe>, Error> {
    let resolved = ResolvedOptions::parse(options)?;
    let (ni, nj) = (source.ni, source.nj);
    // Read the decoded value at an integer grid cell, `None` if out of range or
    // masked.
    let value_at = |gi: i64, gj: i64| -> Option<f64> {
        // Compare at `i64`. Narrowing to `u32` first would wrap an index of
        // 2^32 back into range and let it through the guard; widening `ni`/`nj`
        // instead is exact for every value either side can hold.
        if gi < 0 || gj < 0 || gi >= i64::from(ni) || gj >= i64::from(nj) {
            return None;
        }
        // Both are now inside a `u32` grid, so the narrowing is exact even
        // where `usize` is 32 bits wide.
        values
            .get(gj as usize * ni as usize + gi as usize)
            .copied()
            .flatten()
    };

    match resolved.target {
        TargetKind::Source => {
            if px >= ni || py >= nj {
                return Ok(None);
            }
            // The source view paints grid (i, j) at pixel (i, j), then flips
            // vertically to face north-up; undo that flip to recover the row.
            let flip = source.scan.flips_source_rows(resolved.flip_y);
            let gj = if flip { nj - 1 - py } else { py };
            let (gi, gj) = (px, gj);
            let value = value_at(gi as i64, gj as i64);
            // Geolocate when the grid has a forward map; otherwise report the
            // value without a coordinate. A grid whose parameters did not
            // resolve at all reaches here too, and is the same answer.
            let latlon = source
                .placed()
                .ok()
                .and_then(forward_geolocation)
                .and_then(|f| f(gi, gj));
            let (lat, lon) = match latlon {
                Some((la, lo)) => (Some(la), Some(normalise_lon(lo))),
                None => (None, None),
            };
            Ok(Some(PixelProbe {
                lat,
                lon,
                value,
                grid_i: Some(gi as i32),
                grid_j: Some(gj as i32),
            }))
        }
        TargetKind::Warp(target) => {
            let geometry = source.placed()?;
            require_reprojectable(geometry, source.family)?;
            let (built, _) = build_warp_target(
                target,
                ni,
                nj,
                || geometry_render_window(geometry),
                resolved.bounds,
                geometry.is_periodic_x(),
                matches!(geometry, GridGeometry::Lookup(_)),
            )?;
            let (w, h) = built.dims();
            if px >= w || py >= h {
                return Ok(None);
            }
            // Undo the render's vertical flip to reach the warp raster row.
            let ry = if resolved.flip_y { h - 1 - py } else { py };
            let Some((lat, lon)) = built.pixel_to_lonlat(px, ry) else {
                // Off the globe (outside an azimuthal disc) — nothing there.
                return Ok(None);
            };
            let lon_n = normalise_lon(lon);
            let inverse = geometry.inverse_at();
            match inverse(lat, lon) {
                Some(idx) => {
                    // Resolve the fractional index to a cell exactly as the
                    // Nearest-resampling warp does (`sample_source` in
                    // fieldglass-core), so the probe reads back the same cell
                    // the pixel was painted from. On a periodic (global) grid
                    // the seam gap past the last column is on-grid and lands in
                    // `(ni-1, ni)`, which `round` sends to `ni`; wrapping with
                    // `rem_euclid` brings it back to column 0 instead of falling
                    // off the grid and reporting "no data" for a painted pixel
                    // (#332). A bounded grid clamps defensively against float
                    // error at the edge, matching the renderer.
                    let gi = if geometry.is_periodic_x() {
                        idx.i.round().rem_euclid(ni as f64) as i64
                    } else {
                        idx.i.round().clamp(0.0, (ni - 1) as f64) as i64
                    };
                    let gj = idx.j.round().clamp(0.0, (nj - 1) as f64) as i64;
                    Ok(Some(PixelProbe {
                        lat: Some(lat),
                        lon: Some(lon_n),
                        value: value_at(gi, gj),
                        grid_i: Some(gi as i32),
                        grid_j: Some(gj as i32),
                    }))
                }
                // On the globe but off this grid's coverage.
                None => Ok(Some(PixelProbe {
                    lat: Some(lat),
                    lon: Some(lon_n),
                    value: None,
                    grid_i: None,
                    grid_j: None,
                })),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// Format a decoded field as CSV. The `"long"` (`lat,lon,value`) format needs
/// the family's forward map, so it inherits that gate — space view and an
/// unmodelled family are refused, and the message reads out what is left; the
/// `"matrix"` format needs no coordinates and works for any grid with declared
/// dimensions.
pub fn field_csv(
    source: &Source<'_>,
    values: &[Option<f64>],
    format: &str,
) -> Result<String, Error> {
    let (ni, nj) = (source.ni, source.nj);
    match format {
        "matrix" => Ok(field_to_csv_matrix(values, ni as usize, nj as usize)),
        "long" => {
            let geo = require_forward_geolocation(source, |gt| {
                format!(
                    "the long CSV format needs per-point coordinates, which grid type \
                     {gt:?} doesn't provide (only {}); export as the Matrix format instead",
                    geolocatable_families()
                )
            })?;
            Ok(field_to_csv_long(
                values,
                ni as usize,
                nj as usize,
                // `i` / `j` walk the `u32` grid dimensions widened just above,
                // so the round trip back to `u32` is exact.
                |i, j| geo(i as u32, j as u32),
            ))
        }
        other => Err(Error::InvalidOption {
            detail: format!("unknown CSV format {other:?} (expected \"long\" or \"matrix\")"),
        }),
    }
}

// ---------------------------------------------------------------------------
// Session forwarding
// ---------------------------------------------------------------------------

impl crate::Session {
    /// Project a decoded field into the target `options` names — see
    /// [`project`].
    pub fn project(
        &self,
        source: &Source<'_>,
        values: &[Option<f64>],
        options: &RenderOptions,
    ) -> Result<Projected, Error> {
        project(source, values, options)
    }

    /// Sample the field under one output pixel — see [`probe_pixel`].
    pub fn probe_pixel(
        &self,
        source: &Source<'_>,
        values: &[Option<f64>],
        options: &RenderOptions,
        px: u32,
        py: u32,
    ) -> Result<Option<PixelProbe>, Error> {
        probe_pixel(source, values, options, px, py)
    }

    /// Isolines projected onto the render raster — see [`contour_polylines`].
    pub fn contour_polylines(
        &self,
        source: &Source<'_>,
        values: &[Option<f64>],
        options: &RenderOptions,
        interval: Option<f64>,
    ) -> Result<ProjectedPolylines, Error> {
        contour_polylines(source, values, options, interval)
    }

    /// Geographic polylines projected onto the render raster — see
    /// [`overlay_polylines`].
    pub fn overlay_polylines(
        &self,
        source: &Source<'_>,
        options: &RenderOptions,
        latlon: &[f64],
        ring_lengths: &[u32],
    ) -> Result<ProjectedPolylines, Error> {
        overlay_polylines(source, options, latlon, ring_lengths)
    }

    /// A decoded field as CSV text — see [`field_csv`].
    pub fn field_csv(
        &self,
        source: &Source<'_>,
        values: &[Option<f64>],
        format: &str,
    ) -> Result<String, Error> {
        field_csv(source, values, format)
    }
}
#[cfg(test)]
mod resolved_options_tests {
    use super::*;

    fn opts(projection: &str, resampling: &str) -> RenderOptions {
        RenderOptions::new(projection, resampling)
    }

    #[test]
    fn bounds_parse_requires_all_four_edges_and_valid_box() {
        // Complete, valid box → Some.
        let mut o = opts("equirectangular", "nearest");
        o.bounds_lat_min = Some(10.0);
        o.bounds_lat_max = Some(50.0);
        o.bounds_lon_min = Some(-120.0);
        o.bounds_lon_max = Some(-40.0);
        assert_eq!(
            ResolvedOptions::parse(&o).unwrap().bounds,
            Some(LonLatBox {
                lat_min: 10.0,
                lat_max: 50.0,
                lon_min: -120.0,
                lon_max: -40.0,
            })
        );

        // Antimeridian window: lon_min < -180 is allowed (lon_max > lon_min).
        o.bounds_lon_min = Some(-183.0);
        o.bounds_lon_max = Some(-32.0);
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_some());

        // Partial box → None (silent fallback to computed bounds).
        o.bounds_lon_max = None;
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_none());

        // Inverted box → None.
        o.bounds_lon_min = Some(50.0);
        o.bounds_lon_max = Some(-50.0);
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_none());
    }

    #[test]
    fn parses_valid_combinations() {
        let r = ResolvedOptions::parse(&opts("source", "nearest")).expect("source/nearest");
        assert!(matches!(r.target, TargetKind::Source));
        assert_eq!(r.resampling, Resampling::Nearest);

        let r = ResolvedOptions::parse(&opts("equirectangular", "bilinear")).expect("eqr/bilinear");
        assert!(matches!(
            r.target,
            TargetKind::Warp(WarpTarget::Equirectangular)
        ));
        assert_eq!(r.resampling, Resampling::Bilinear);

        let r = ResolvedOptions::parse(&opts("web_mercator", "nearest")).expect("merc/nearest");
        assert!(matches!(
            r.target,
            TargetKind::Warp(WarpTarget::WebMercator)
        ));

        // Azimuthal targets resolve their preset into concrete parameters.
        let r = ResolvedOptions::parse(&opts("orthographic", "nearest")).expect("ortho/nearest");
        assert!(matches!(
            r.target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 0.0
        ));
        let r =
            ResolvedOptions::parse(&opts("polar_stereographic", "nearest")).expect("polar/nearest");
        assert!(matches!(
            r.target,
            TargetKind::Warp(WarpTarget::PolarStereographic {
                south_pole: false,
                ..
            })
        ));
    }

    #[test]
    fn orthographic_preset_selects_centre() {
        let mut o = opts("orthographic", "nearest");
        o.projection_preset = Some("pacific".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 180.0
        ));
        o.projection_preset = Some("indian".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 90.0
        ));
        o.projection_preset = Some("americas".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 270.0
        ));
        o.projection_preset = Some("north_pole".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, .. }) if lat0 == 90.0
        ));
        // Unknown preset falls back to the Atlantic default.
        o.projection_preset = Some("nonsense".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 0.0
        ));
    }

    #[test]
    fn polar_stereographic_preset_selects_hemisphere() {
        let mut o = opts("polar_stereographic", "nearest");
        o.projection_preset = Some("south".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::PolarStereographic {
                south_pole: true,
                ..
            })
        ));
    }

    #[test]
    fn orthographic_free_form_centre_overrides_preset() {
        // A free-form centre is honoured verbatim, with no preset present.
        let mut o = opts("orthographic", "nearest");
        o.center_lat = Some(37.5);
        o.center_lon = Some(-122.25);
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 })
                if lat0 == 37.5 && lon0 == -122.25
        ));

        // Free-form centre wins over a preset that would say otherwise.
        o.projection_preset = Some("pacific".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 })
                if lat0 == 37.5 && lon0 == -122.25
        ));

        // Each component falls back independently: lon free-form, lat from preset.
        o.center_lat = None;
        o.center_lon = Some(10.0);
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            // "pacific" preset is (0.0, 180.0); lon overridden to 10, lat from preset.
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 })
                if lat0 == 0.0 && lon0 == 10.0
        ));
    }

    #[test]
    fn polar_stereographic_free_form_central_meridian() {
        // center_lon sets the central meridian; hemisphere still from the preset.
        let mut o = opts("polar_stereographic", "nearest");
        o.projection_preset = Some("south".to_string());
        o.center_lon = Some(-45.0);
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::PolarStereographic { south_pole: true, lon0 })
                if lon0 == -45.0
        ));

        // No center_lon → central meridian defaults to 0°.
        o.center_lon = None;
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().target,
            TargetKind::Warp(WarpTarget::PolarStereographic { south_pole: true, lon0 })
                if lon0 == 0.0
        ));
    }

    #[test]
    fn world_targets_take_the_central_meridian_from_center_lon() {
        // center_lon sets the central meridian; no preset applies to these.
        for (name, lon0) in [
            ("mollweide", -100.0),
            ("robinson", 25.0),
            ("equal_earth", 0.0),
        ] {
            let mut o = opts(name, "nearest");
            o.center_lon = Some(lon0);
            let got = match ResolvedOptions::parse(&o).unwrap().target {
                TargetKind::Warp(WarpTarget::Mollweide { lon0 })
                | TargetKind::Warp(WarpTarget::Robinson { lon0 })
                | TargetKind::Warp(WarpTarget::EqualEarth { lon0 }) => lon0,
                other => panic!("{name} did not resolve to a world target: {other:?}"),
            };
            assert_eq!(got, lon0, "{name} central meridian");
        }

        // Each name must resolve to *its own* target, not merely to some world
        // target — a copy-paste slip in the parse arms would pass the loop above.
        assert!(matches!(
            ResolvedOptions::parse(&opts("mollweide", "nearest"))
                .unwrap()
                .target,
            TargetKind::Warp(WarpTarget::Mollweide { .. })
        ));
        assert!(matches!(
            ResolvedOptions::parse(&opts("robinson", "nearest"))
                .unwrap()
                .target,
            TargetKind::Warp(WarpTarget::Robinson { .. })
        ));
        assert!(matches!(
            ResolvedOptions::parse(&opts("equal_earth", "nearest"))
                .unwrap()
                .target,
            TargetKind::Warp(WarpTarget::EqualEarth { .. })
        ));

        // No center_lon → central meridian defaults to 0° (Greenwich-centred).
        assert!(matches!(
            ResolvedOptions::parse(&opts("robinson", "nearest"))
                .unwrap()
                .target,
            TargetKind::Warp(WarpTarget::Robinson { lon0 }) if lon0 == 0.0
        ));
    }

    /// A lookup grid's array shape says nothing about its geography, so the box
    /// raster takes its shape from the window instead (#515).
    #[test]
    fn a_box_raster_takes_its_shape_from_the_window_not_the_array() {
        // The NOAA-21 half orbit: 96 fields of view by 768 scan lines, over a
        // window 355° wide and 117° tall. Drawn at the array's shape that is a
        // 3:1 area in a 1:8 raster — more than twentyfold too tall.
        let (w, h) = box_raster_dims(96, 768, LonLatBox::new(-89.93, 26.79, -30.33, 324.58));
        let aspect = f64::from(w) / f64::from(h);
        let want = (324.58 - -30.33) / (26.79 - -89.93);
        assert!(
            (aspect - want).abs() / want < 0.01,
            "raster {w}x{h} has aspect {aspect}, window wants {want}"
        );
        // Nothing is downsampled: the longer edge keeps the source's longer edge.
        assert_eq!(
            w.max(h),
            768,
            "long edge should be the source's, got {w}x{h}"
        );

        // A window taller than it is wide puts the source's long edge on height.
        let (w, h) = box_raster_dims(96, 768, LonLatBox::new(-80.0, 80.0, 0.0, 40.0));
        assert_eq!(h, 768);
        assert!(
            w < h,
            "a tall window must not produce a wide raster: {w}x{h}"
        );

        // Degenerate windows have no aspect to honour, so the source shape stands
        // rather than collapsing the raster.
        assert_eq!(
            box_raster_dims(96, 768, LonLatBox::new(10.0, 10.0, 0.0, 40.0)),
            (96, 768)
        );
        assert_eq!(
            box_raster_dims(96, 768, LonLatBox::new(0.0, 10.0, 5.0, 5.0)),
            (96, 768)
        );
        assert_eq!(
            box_raster_dims(96, 768, LonLatBox::new(0.0, f64::NAN, 0.0, 40.0)),
            (96, 768)
        );
    }

    #[test]
    fn world_raster_keeps_each_projections_true_proportions() {
        // Height is the source's larger edge; width follows the projection's own
        // aspect, so the three world maps are *not* interchangeable rasters.
        assert_eq!(
            world_raster_dims(100, 200, Mollweide::ASPECT_RATIO),
            (400, 200)
        );
        assert_eq!(
            world_raster_dims(100, 200, Robinson::ASPECT_RATIO),
            (394, 200)
        );
        assert_eq!(
            world_raster_dims(100, 200, EqualEarth::ASPECT_RATIO),
            (411, 200)
        );
        // A degenerate source stays degenerate rather than wrapping.
        assert_eq!(world_raster_dims(0, 0, Robinson::ASPECT_RATIO), (0, 0));
    }

    /// The floor raises a coarse raster to display scale and leaves everything
    /// else alone (#514).
    #[test]
    fn the_raster_floor_raises_only_what_is_below_it() {
        // A HEALPix Nside 4 field resamples to 26 × 14, the case in the report.
        let (w, h) = raise_to_min_raster((26, 14));
        assert_eq!(w, MIN_REPROJECTED_LONG_EDGE, "long edge lands on the floor");
        let aspect = f64::from(w) / f64::from(h);
        assert!(
            (aspect - 26.0 / 14.0).abs() / (26.0 / 14.0) < 0.01,
            "the source's aspect should survive, got {w}x{h}"
        );

        // Real forecast grids are already past the floor and must not move: a
        // silent resample of GFS or HRRR would be a far worse bug than the one
        // this fixes.
        assert_eq!(raise_to_min_raster((1440, 721)), (1440, 721), "GFS");
        assert_eq!(raise_to_min_raster((1799, 1059)), (1799, 1059), "HRRR");
        assert_eq!(raise_to_min_raster((2880, 1440)), (2880, 1440), "GFS world");

        // Exactly on the floor is already floored — no off-by-one rescale.
        assert_eq!(
            raise_to_min_raster((MIN_REPROJECTED_LONG_EDGE, 100)),
            (MIN_REPROJECTED_LONG_EDGE, 100)
        );

        // A square target stays square, so an azimuthal disc stays circular.
        let (w, h) = raise_to_min_raster((4, 4));
        assert_eq!(
            (w, h),
            (MIN_REPROJECTED_LONG_EDGE, MIN_REPROJECTED_LONG_EDGE)
        );

        // A degenerate raster stays degenerate, as `world_raster_dims` promises.
        assert_eq!(raise_to_min_raster((0, 0)), (0, 0));
        assert_eq!(raise_to_min_raster((0, 40)), (0, 40));
        assert_eq!(raise_to_min_raster((40, 0)), (40, 0));
    }

    /// Scaling to a *long* edge bounds the result: whatever the aspect, the
    /// short edge cannot pass the floor either, so the floor can never ask for
    /// more than `720 × 720` pixels.
    #[test]
    fn the_raster_floor_cannot_explode_an_extreme_aspect() {
        for dims in [(1, 700), (700, 1), (1, 1), (3, 719), (719, 3), (2, 2)] {
            let (w, h) = raise_to_min_raster(dims);
            assert_eq!(
                w.max(h),
                MIN_REPROJECTED_LONG_EDGE,
                "{dims:?} should reach the floor, got {w}x{h}"
            );
            assert!(
                w <= MIN_REPROJECTED_LONG_EDGE && h <= MIN_REPROJECTED_LONG_EDGE,
                "{dims:?} produced {w}x{h}, past the 720x720 bound"
            );
            assert!(w >= 1 && h >= 1, "{dims:?} collapsed an edge to zero");
        }
    }

    #[test]
    fn colormap_defaults_to_viridis_and_resolves_by_name() {
        // A caller that never sets a colormap renders exactly as before.
        let o = opts("source", "nearest");
        let r = ResolvedOptions::parse(&o).expect("default colormap");
        assert_eq!(r.colormap.name(), "viridis");
        assert!(!r.reverse_colormap);

        // Every registered name resolves to itself, so the picker and the
        // renderer can't disagree about what a name means.
        for c in fieldglass_core::colormap::colormaps() {
            let mut o = opts("source", "nearest");
            o.colormap = Some(c.name().to_string());
            o.reverse_colormap = Some(true);
            let r = ResolvedOptions::parse(&o).expect("registered colormap");
            assert_eq!(r.colormap.name(), c.name());
            assert!(r.reverse_colormap);
        }
    }

    #[test]
    fn rejects_unknown_colormap_naming_the_known_ones() {
        let mut o = opts("source", "nearest");
        o.colormap = Some("jet".to_string());
        let err = ResolvedOptions::parse(&o).expect_err("unknown colormap must error");
        let msg = err.to_string();
        assert!(msg.contains("unknown colormap"), "{msg}");
        // The message must list what *is* available, or it isn't actionable.
        assert!(msg.contains("viridis"), "{msg}");
    }

    #[test]
    fn scale_mode_defaults_to_linear_and_resolves_log10() {
        // Unset renders exactly as before.
        let r = ResolvedOptions::parse(&opts("source", "nearest")).expect("default scale");
        assert_eq!(r.scale, ScaleMode::Linear);

        for (wire, want) in [("linear", ScaleMode::Linear), ("log10", ScaleMode::Log10)] {
            let mut o = opts("source", "nearest");
            o.scale_mode = Some(wire.to_string());
            let r = ResolvedOptions::parse(&o).expect("valid scale mode");
            assert_eq!(r.scale, want, "wire {wire:?}");
        }
    }

    #[test]
    fn rejects_unknown_scale_mode() {
        let mut o = opts("source", "nearest");
        o.scale_mode = Some("symlog".to_string());
        let err = ResolvedOptions::parse(&o).expect_err("unknown scale mode must error");
        let msg = err.to_string();
        assert!(msg.contains("unknown scale mode"), "{msg}");
        assert!(
            msg.contains("log10"),
            "message names the valid modes: {msg}"
        );
    }

    #[test]
    fn rejects_unknown_projection() {
        let err = ResolvedOptions::parse(&opts("aitoff", "nearest"))
            .expect_err("unknown projection must error");
        assert!(
            err.to_string().contains("unknown projection"),
            "error names the field, got: {err}"
        );
    }

    #[test]
    fn rejects_unknown_resampling() {
        let err = ResolvedOptions::parse(&opts("source", "bicubic"))
            .expect_err("unknown resampling must error");
        assert!(
            err.to_string().contains("unknown resampling"),
            "error names the field, got: {err}"
        );
    }
}

#[cfg(test)]
mod forward_geolocation_tests {
    use super::*;
    use fieldglass_core::LatLonParams;

    /// A regular lat/lon grid over a small region — the shape the contour and
    /// CSV paths walk. `MessageMeta` literals used to stand in for this; the
    /// geometry states the same four corners without a family string to get
    /// wrong (#572).
    fn latlon(ni: u32, nj: u32) -> GridGeometry {
        GridGeometry::LatLon(LatLonParams {
            ni,
            nj,
            lat_first: 40.0,
            lon_first: 0.0,
            lat_last: 10.0,
            lon_last: 40.0,
        })
    }

    /// A global west-to-east grid: `ni` columns whose last one stops a step
    /// short of the seam, so the gap past it closes the circle back to column 0.
    fn global_latlon(ni: u32, nj: u32) -> GridGeometry {
        let GridGeometry::LatLon(mut p) = latlon(ni, nj) else {
            unreachable!("latlon builds a LatLon")
        };
        p.lon_first = 0.0;
        p.lon_last = 360.0 - 360.0 / f64::from(ni);
        GridGeometry::LatLon(p)
    }

    #[test]
    fn levels_by_interval_walks_the_range_on_multiples() {
        assert_eq!(levels_by_interval(0.0, 10.0, 2.0), vec![2.0, 4.0, 6.0, 8.0]);
        // Endpoints are excluded; a start below the range is skipped.
        assert_eq!(levels_by_interval(-3.0, 3.0, 3.0), vec![0.0]);
        // Degenerate inputs yield nothing.
        assert!(levels_by_interval(5.0, 5.0, 1.0).is_empty());
        assert!(levels_by_interval(0.0, 10.0, 0.0).is_empty());
        assert!(levels_by_interval(0.0, 10.0, -1.0).is_empty());
    }

    #[test]
    fn forward_geolocation_latlon_places_corners_then_interior() {
        let geometry = latlon(5, 4);
        let fwd = forward_geolocation(&geometry).expect("latlon forward map");
        // Corner (0,0) is (latFirst, lonFirst); (4,3) is (latLast, lonLast).
        assert_eq!(fwd(0, 0), Some((40.0, 0.0)));
        assert_eq!(fwd(4, 3), Some((10.0, 40.0)));
        // A fractional vertex on the bottom edge interpolates the longitude.
        let (lat, lon) = forward_bilinear(fwd.as_ref(), 5, 4, 2.5, 0.0, false).expect("interior");
        assert!(
            (lat - 40.0).abs() < 1e-9,
            "on the first row, lat = latFirst"
        );
        assert!(
            (lon - 25.0).abs() < 1e-9,
            "i=2.5 over 0..40/4 steps → lon 25, got {lon}"
        );
    }

    /// A seam vertex sits in `(ni-1, ni)`, where the west corner is the last
    /// column and the east corner is column 0 one turn further east. Clamping
    /// `i0` at `ni-2` (the bounded rule) drags both corners onto the last two
    /// columns and the seam collapses; interpolating without the +360 sweeps
    /// backwards across the whole map instead of across the seam gap.
    #[test]
    fn forward_bilinear_interpolates_across_the_periodic_seam() {
        let geometry = global_latlon(4, 3);
        let fwd = forward_geolocation(&geometry).expect("latlon forward map");
        // Columns are 90° apart: 0, 90, 180, 270.
        assert_eq!(fwd(0, 0).map(|p| p.1), Some(0.0));
        assert_eq!(fwd(3, 0).map(|p| p.1), Some(270.0));

        // Midway through the seam cell → 315°, which normalises to -45.
        let (_, lon) = forward_bilinear(fwd.as_ref(), 4, 3, 3.5, 0.0, true).expect("seam vertex");
        assert!(
            (normalise_lon(315.0) - lon).abs() < 1e-9,
            "the seam midpoint must read 315 (≡ -45), got {lon}",
        );

        // Without the periodic flag the same vertex clamps back onto the last
        // column — the collapse that left a gap at the seam meridian.
        let (_, bounded) =
            forward_bilinear(fwd.as_ref(), 4, 3, 3.5, 0.0, false).expect("bounded vertex");
        assert!(
            (bounded - 270.0).abs() < 1e-9,
            "the bounded rule clamps the seam vertex onto the last column \
             (lon 270, un-normalised), got {bounded}",
        );
    }
}

#[cfg(test)]
mod warp_target_tests {
    use super::*;
    use fieldglass_core::{
        DEFAULT_EARTH_RADIUS_M, GeostationaryParams, LambertParams, LatLonParams, MercatorParams,
        PolarStereoParams, RotatedLatLonParams,
    };

    /// Assert a reprojected raster sits on the minimum long edge and kept the
    /// aspect the target asked for (#514).
    ///
    /// `unfloored` is the raster the arm would have produced before the floor:
    /// the source's shape for the lat/lon-box targets, a square for the
    /// azimuthal discs. Spelled out as two properties rather than by calling
    /// [`raise_to_min_raster`], so the test states the contract instead of
    /// re-running the implementation it is checking.
    #[track_caller]
    fn assert_floored_raster(got: (u32, u32), unfloored: (u32, u32)) {
        let (w, h) = got;
        assert_eq!(
            w.max(h),
            super::MIN_REPROJECTED_LONG_EDGE,
            "long edge should sit on the floor, got {w}x{h}"
        );
        let want = f64::from(unfloored.0) / f64::from(unfloored.1);
        let aspect = f64::from(w) / f64::from(h);
        assert!(
            (aspect - want).abs() / want < 0.01,
            "raster {w}x{h} has aspect {aspect}, wanted {want}"
        );
    }

    /// A [`Source`] over one geometry, with the raster shape read back off it
    /// and the north-down scan every synthetic here declares.
    fn source<'a>(geometry: &'a GridGeometry, family: &'a str) -> Source<'a> {
        let (ni, nj) = geometry
            .dims()
            .expect("every synthetic here states its dimensions");
        Source {
            geometry: Ok(geometry),
            ni,
            nj,
            scan: Scan::north_down(),
            family,
        }
    }

    /// The resolved options a warp reads: a resampling and an optional manual
    /// window. [`warp_field`] takes its target as an argument, so the
    /// `projection` string here only has to be one the parser accepts.
    fn resolved(resampling: &str, window: Option<LonLatBox>) -> ResolvedOptions {
        let mut opts = RenderOptions::new("equirectangular", resampling);
        if let Some(w) = window {
            opts.bounds_lat_min = Some(w.lat_min);
            opts.bounds_lat_max = Some(w.lat_max);
            opts.bounds_lon_min = Some(w.lon_min);
            opts.bounds_lon_max = Some(w.lon_max);
        }
        ResolvedOptions::parse(&opts).expect("the strings above are the vocabulary")
    }

    /// The grid of the `cmc_wind_300_2010052400_p012.grib` fixture: 135×95
    /// polar-stereographic, 60 km at 60°N, north-polar. The message declares no
    /// Earth of its own, so it is placed on the WMO default sphere — which is
    /// what the host's parameter builder substitutes for an absent radius.
    fn cmc_polar() -> PolarStereoParams {
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
    fn source_overlay_projects_onto_a_polar_stereo_grid() {
        // #72: the source-projection overlay must work for *projected* grids,
        // not just regular lat/lon — it reuses the grid's own inverse map. A
        // short polyline over North America (inside the CMC polar grid)
        // projects to a non-empty, shape-consistent run.
        let opts = RenderOptions::new("source", "nearest");
        let geometry = GridGeometry::PolarStereo(cmc_polar());
        let latlon = [40.0, -100.0, 41.0, -99.0, 42.0, -98.0];
        let out = overlay_polylines(&source(&geometry, "polar_stereo"), &opts, &latlon, &[3])
            .expect("source overlay on polar grid");
        let total: u32 = out.seg_lengths.iter().copied().sum();
        assert_eq!(total as usize * 2, out.xy.len(), "shape invariant");
        assert!(!out.xy.is_empty(), "polyline over the grid should project");
    }

    #[test]
    fn warps_polar_stereo_to_equirectangular() {
        let geometry = GridGeometry::PolarStereo(cmc_polar());
        // Synthetic uniform field — we're testing the warp geometry, not
        // value transport. Every present output pixel should read back
        // exactly 1.0.
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let out = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("warp");

        assert_floored_raster((out.width, out.height), (135, 95));
        let present_count = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count > 0,
            "polar-stereo warp produced an entirely empty mask — \
             either the inverse map rejects every pixel or the render window \
             is wrong"
        );
        for (i, &m) in out.mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(out.values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(
            out.summary.contains("polar_stereo") && out.summary.contains("equirectangular"),
            "summary should name source kind + target, got: {}",
            out.summary
        );
    }

    #[test]
    fn warps_polar_stereo_to_web_mercator() {
        // The Web Mercator target shares the polar-stereo source inverse map
        // and render window; it just distributes rows in Mercator Y. Verify the
        // path produces a non-empty mask, transports values, clamps the latitude
        // extent into the Mercator band, and names the target in the summary.
        let geometry = GridGeometry::PolarStereo(cmc_polar());
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let out = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::WebMercator,
            &resolved("nearest", None),
        )
        .expect("mercator warp");

        assert_floored_raster((out.width, out.height), (135, 95));
        let present_count = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(present_count > 0, "mercator warp produced an empty mask");
        for (i, &m) in out.mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(out.values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        let LonLatBox {
            lat_min, lat_max, ..
        } = LonLatBox::from_array(out.bounds.expect("web mercator has bounds"));
        assert!(
            lat_min >= -85.06 && lat_max <= 85.06,
            "lat extent must be clamped to the Mercator band, got {lat_min}..{lat_max}"
        );
        assert!(
            out.summary.contains("polar_stereo") && out.summary.contains("web mercator"),
            "summary should name source kind + target, got: {}",
            out.summary
        );
    }

    #[test]
    fn warps_polar_stereo_to_azimuthal_targets() {
        // The orthographic and polar-stereographic *targets* fit a disc to the
        // raster: they share the source inverse map but report no lat/lon-box
        // extent. Verify both produce a non-empty mask and the right summary,
        // and that bounds come back `None`.
        let geometry = GridGeometry::PolarStereo(cmc_polar());
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];

        for (target, name) in [
            (
                WarpTarget::Orthographic {
                    lat0: 90.0,
                    lon0: 0.0,
                },
                "orthographic",
            ),
            (
                WarpTarget::PolarStereographic {
                    south_pole: false,
                    lon0: 0.0,
                },
                "polar stereographic",
            ),
        ] {
            let out = warp_field(
                &source(&geometry, "polar_stereo"),
                &raw,
                target,
                &resolved("nearest", None),
            )
            .unwrap_or_else(|e| panic!("{name} warp failed: {e}"));
            // Azimuthal discs render into a square raster (side = the larger
            // source axis) so the globe stays circular rather than stretching
            // into an ellipse on the 135×95 source.
            assert_floored_raster((out.width, out.height), (135, 135));
            let present = out.mask.iter().filter(|&&m| m == 1).count();
            assert!(present > 0, "{name} target produced an empty mask");
            for (i, &m) in out.mask.iter().enumerate() {
                if m == 1 {
                    assert_eq!(out.values[i], 1.0, "{name} present pixel {i} should be 1.0");
                }
            }
            assert!(
                out.bounds.is_none(),
                "{name} target has no lat/lon-box extent"
            );
            assert!(
                out.summary.contains("polar_stereo") && out.summary.contains(name),
                "{name} summary should name source + target, got: {}",
                out.summary
            );
        }
    }

    #[test]
    fn warps_south_polar_stereo_to_equirectangular() {
        // Mirror the CMC tile into the southern hemisphere: south-pole
        // projection, negative lat_first. Exercises the `sign = -1` branch
        // through the full warp path, not just the projection-level
        // round-trip tests.
        let geometry = GridGeometry::PolarStereo(PolarStereoParams {
            lat_first: -11.43,
            south_pole: true,
            ..cmc_polar()
        });
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let out = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("south-polar warp");

        assert_floored_raster((out.width, out.height), (135, 95));
        let present_count = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count > 0,
            "south-polar warp produced an entirely empty mask"
        );
        for (i, &m) in out.mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(out.values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(out.summary.contains("polar_stereo") && out.summary.contains("equirectangular"));
    }

    #[test]
    fn warps_hemispheric_grid_with_pole_inside() {
        // A synthetic hemispheric grid whose projected extent surrounds the
        // pole (same geometry as the projection-level
        // `polar_stereo_pole_inside_grid_detection` test). Two things this
        // pins that the regional CMC test does not:
        //   1. negative `dy_metres` (south-scanning) is handled by the warp,
        //      not just the projector unit test;
        //   2. the 360°-longitude / pole-clamp override path in
        //      `GridGeometry::lonlat_bbox` is reachable through the render
        //      window and produces a fully-covered raster instead of a thin
        //      four-corner sliver.
        let geometry = GridGeometry::PolarStereo(PolarStereoParams {
            ni: 4,
            nj: 4,
            lat_first: 50.8,
            lon_first: -135.0,
            lov: 0.0,
            dx_metres: 2_000_000.0,
            dy_metres: -2_000_000.0,
            south_pole: false,
            ..cmc_polar()
        });
        let raw: Vec<Option<f64>> = vec![Some(1.0); 4 * 4];
        let out = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("hemispheric warp");

        assert_floored_raster((out.width, out.height), (4, 4));
        // With the pole inside the grid the target spans the full hemisphere,
        // so a clear majority of the output pixels resolve to a source sample
        // rather than the handful a four-corner bbox would cover. Stated as a
        // fraction because the raster is floored to display scale (#514) — a
        // fixed count would be met by a sliver once the raster is this large.
        let total = (out.width * out.height) as usize;
        let present_count = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count * 2 >= total,
            "pole-inside grid should fill most of the raster, got {present_count}/{total} present"
        );
    }

    #[test]
    fn warps_grid_with_pole_exactly_on_origin() {
        // lat_first = 90° puts the first scanned point at the pole, so the
        // projected grid origin is exactly (0, 0). `pole_inside_grid` uses
        // inclusive bounds, so this edge case must still take the
        // 360°-longitude override and warp without panicking.
        let geometry = GridGeometry::PolarStereo(PolarStereoParams {
            ni: 4,
            nj: 4,
            lat_first: 90.0,
            lon_first: 0.0,
            lov: 0.0,
            dx_metres: 2_000_000.0,
            dy_metres: 2_000_000.0,
            south_pole: false,
            ..cmc_polar()
        });
        let raw: Vec<Option<f64>> = vec![Some(1.0); 4 * 4];
        let out = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("pole-on-origin warp");
        assert_floored_raster((out.width, out.height), (4, 4));
        assert!(
            out.mask.contains(&1),
            "pole-on-origin grid should still resolve some pixels"
        );
    }

    #[test]
    fn a_grid_the_host_could_not_state_is_refused_by_the_warp() {
        // The host reads its own wire fields, so a message that names a family
        // and then fails to supply the numbers for it refuses *there*, with a
        // message naming the field. `Source` carries that refusal rather than
        // raising it, and this is the operation that surfaces it: a warp needs
        // a geometry and there is none.
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let refusal = Error::Unsupported {
            detail: "missing polarStereoLov".to_string(),
        };
        let src = Source {
            geometry: Err(refusal.clone()),
            ni: 135,
            nj: 95,
            scan: Scan::north_down(),
            family: "polar_stereo",
        };
        let err = warp_field(
            &src,
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect_err("a source with no geometry cannot be warped");
        assert_eq!(err, refusal, "the host's own words reach the caller intact");
    }

    #[test]
    fn a_family_this_build_does_not_model_is_named_in_the_refusal() {
        // `GridGeometry::Unsupported` is not an error on its own — a source
        // render paints such a field happily — so the warp is where it becomes
        // one, and the message quotes the family the *file* named rather than
        // the `"unsupported"` tag the geometry reports.
        let geometry = GridGeometry::Unsupported {
            label: "healpix".to_string(),
        };
        let src = Source {
            geometry: Ok(&geometry),
            ni: 8,
            nj: 8,
            scan: Scan::north_down(),
            family: "healpix",
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 8 * 8];
        let err = warp_field(
            &src,
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect_err("an unmodelled family cannot be reprojected");
        assert_eq!(
            err.message(),
            "reprojection not yet supported for grid type \"healpix\""
        );
    }

    #[test]
    fn bounds_override_replaces_computed_extent_and_echoes_back() {
        let geometry = GridGeometry::PolarStereo(cmc_polar());
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];

        // Default: no override → echoed bounds are the computed source extent.
        let default_bounds = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("default warp")
        .bounds
        .expect("equirectangular has bounds");

        // Explicit window → that window is rendered and echoed back verbatim.
        let window = LonLatBox::new(30.0, 60.0, -140.0, -60.0);
        let used = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", Some(window)),
        )
        .expect("windowed warp")
        .bounds;
        assert_eq!(used, Some(window.to_array()));
        assert_ne!(
            used.unwrap(),
            default_bounds,
            "override should differ from the computed default"
        );
    }

    #[test]
    fn web_mercator_band_outside_clamp_is_rejected() {
        // A manual latitude band lying entirely poleward of the ±85.0511°
        // Web Mercator cutoff clamps to a single edge (zero Y span), which
        // would smear every row to one latitude. It must be rejected instead.
        let geometry = GridGeometry::PolarStereo(cmc_polar());
        let raw = vec![Some(1.0); 135 * 95];
        let band = LonLatBox::new(86.0, 88.0, -10.0, 10.0);
        let err = warp_field(
            &source(&geometry, "polar_stereo"),
            &raw,
            WarpTarget::WebMercator,
            &resolved("nearest", Some(band)),
        )
        .expect_err("a lat band entirely outside ±85.05° must be rejected");
        assert!(
            err.message().contains("Web Mercator"),
            "expected a Web-Mercator-band error, got: {}",
            err.message()
        );
    }

    /// A GRIB2 §3.10 Mercator grid (100×100 over the western tropics). The
    /// Mercator inverse map is pinned by the corner coordinates, so — like the
    /// regular lat/lon source — no metric grid spacing is needed.
    fn mercator() -> MercatorParams {
        MercatorParams {
            ni: 100,
            nj: 100,
            lat_first: 0.0,
            lon_first: -100.0,
            lat_last: 30.0,
            lon_last: -60.0,
        }
    }

    #[test]
    fn warps_mercator_to_equirectangular() {
        // #119: a GRIB2 Mercator (§3.10) source grid must reproject. Synthetic
        // uniform field — testing warp geometry, not value transport.
        let geometry = GridGeometry::Mercator(mercator());
        let raw: Vec<Option<f64>> = vec![Some(1.0); 100 * 100];
        let out = warp_field(
            &source(&geometry, "mercator"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("mercator warp");

        assert_floored_raster((out.width, out.height), (100, 100));
        let present = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "mercator warp produced an empty mask");
        for (i, &m) in out.mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(out.values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        // The source extent is the geographic corner box.
        let LonLatBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        } = LonLatBox::from_array(out.bounds.expect("mercator has bounds"));
        assert!(
            lat_min >= -0.01 && lat_max <= 30.01,
            "lat box {lat_min}..{lat_max}"
        );
        assert!(
            lon_min >= -100.01 && lon_max <= -59.99,
            "lon box {lon_min}..{lon_max}"
        );
        assert!(
            out.summary.contains("mercator") && out.summary.contains("equirectangular"),
            "summary should name source kind + target, got: {}",
            out.summary
        );
    }

    /// A GRIB2 §3.30 Lambert grid (100×100, 3 km, CONUS-like), on the WMO
    /// default sphere the host substitutes for a message declaring no radius.
    fn lambert() -> LambertParams {
        LambertParams {
            earth_radius_m: DEFAULT_EARTH_RADIUS_M,
            ni: 100,
            nj: 100,
            lat_first: 21.14,
            lon_first: -122.72,
            lad: 38.5,
            lov: -97.5,
            dx_metres: 3000.0,
            dy_metres: 3000.0,
            latin1: 38.5,
            latin2: 38.5,
        }
    }

    #[test]
    fn warps_grib2_lambert_to_equirectangular() {
        // #119 audit half: confirm the GRIB2 Lambert (§3.30) params reach the
        // warp and reproject a non-empty field — the same path GRIB1 Lambert
        // already uses.
        let geometry = GridGeometry::Lambert(lambert());
        let raw: Vec<Option<f64>> = vec![Some(1.0); 100 * 100];
        let out = warp_field(
            &source(&geometry, "lambert"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("lambert warp");

        assert_floored_raster((out.width, out.height), (100, 100));
        let present = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "lambert warp produced an empty mask");
        for (i, &m) in out.mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(out.values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(
            out.summary.contains("lambert") && out.summary.contains("equirectangular"),
            "summary should name source kind + target, got: {}",
            out.summary
        );
    }

    /// A GRIB2 §3.1 rotated lat/lon grid, mirroring the committed
    /// `rotated_latlon_surface.grib2` fixture: 16×31 grid, rotated corners
    /// (60,0)→(0,30), southern pole at geographic (0,0), no rotation.
    fn rotated_latlon() -> RotatedLatLonParams {
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

    /// The 2×2 corner of a regular lat/lon grid the CSV tests export. The
    /// dimensions are the raster's, not the fixture's: `field_csv` walks
    /// `ni × nj` cells and asks the geometry for each one's position.
    fn latlon_2x2() -> LatLonParams {
        LatLonParams {
            ni: 2,
            nj: 2,
            lat_first: 60.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 30.0,
        }
    }

    #[test]
    fn field_csv_matrix_format() {
        // 2×2 grid with a missing hole at (i=1, j=0).
        let values = vec![Some(1.0), None, Some(3.0), Some(4.0)];
        let geometry = GridGeometry::LatLon(latlon_2x2());
        let csv = field_csv(&source(&geometry, "latlon"), &values, "matrix").expect("matrix");
        assert_eq!(csv, "1,\n3,4\n");
    }

    #[test]
    fn field_csv_long_format_has_header_and_values() {
        let values = vec![Some(1.0), None, Some(3.0), Some(4.0)];
        let geometry = GridGeometry::LatLon(latlon_2x2());
        let csv = field_csv(&source(&geometry, "latlon"), &values, "long").expect("long");
        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some("lat,lon,value"));
        // One row per grid point, values in scan order in the 3rd column;
        // the missing point (i=1, j=0) has an empty value cell.
        let value_col: Vec<&str> = lines.map(|l| l.rsplit(',').next().unwrap()).collect();
        assert_eq!(value_col, vec!["1", "", "3", "4"]);
    }

    #[test]
    fn field_csv_rejects_unknown_format() {
        let geometry = GridGeometry::LatLon(latlon_2x2());
        let err =
            field_csv(&source(&geometry, "latlon"), &[Some(1.0)], "tsv").expect_err("bad format");
        assert!(format!("{err}").contains("unknown CSV format"));
    }

    #[test]
    fn field_csv_long_gates_grids_without_a_forward_map() {
        // A projected grid geolocates now (#470), so the long format exports it
        // rather than refusing.
        let polar = GridGeometry::PolarStereo(PolarStereoParams {
            ni: 1,
            nj: 1,
            ..cmc_polar()
        });
        let csv = field_csv(&source(&polar, "polar_stereo"), &[Some(1.0)], "long")
            .expect("a polar-stereographic grid exports the long format");
        let first = csv.lines().nth(1).expect("one data row");
        let cells: Vec<f64> = first
            .split(',')
            .map(|c| c.parse().expect("numeric"))
            .collect();
        assert!(
            (cells[0] - 11.43).abs() < 1e-6 && (cells[1] + 110.27).abs() < 1e-6,
            "the row carries the grid's own first point, got: {first}"
        );

        // Space view (§3.90) still has no forward map — part of its scan-angle
        // grid is off the disc — so it must error rather than emit bad
        // coordinates. The message must be about CSV and point at the Matrix
        // layout, not leak the shared gate's contour wording (#337).
        //
        // The disc below is nominal GOES-East: nothing here reads a coordinate
        // off it, because the refusal is the family's, not this grid's.
        let space_view = GridGeometry::Geostationary(GeostationaryParams {
            ni: 1,
            nj: 1,
            h_metres: 42_164_000.0,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314,
            sub_lon_deg: -75.0,
            sweep_x: true,
            x0: -0.1012,
            dx_rad: 0.0202,
            y0: 0.1012,
            dy_rad: -0.0202,
        });
        let err = field_csv(&source(&space_view, "space_view"), &[Some(1.0)], "long")
            .expect_err("a grid without coordinates is gated");
        let msg = format!("{err}");
        assert!(
            msg.contains("long CSV") && msg.contains("Matrix"),
            "actionable CSV-specific message, got: {msg}"
        );
        assert!(
            !msg.contains("contour"),
            "the CSV gate must not surface contour wording, got: {msg}"
        );
    }

    #[test]
    fn warps_grib2_rotated_latlon_to_equirectangular() {
        // #120: a GRIB2 rotated lat/lon (§3.1) source grid must reproject. The
        // corners are rotated-frame coordinates, so the warp rotates each
        // geographic output point into the grid's frame before sampling.
        // Synthetic uniform field — testing warp geometry, not value transport.
        let geometry = GridGeometry::RotatedLatLon(rotated_latlon());
        let raw: Vec<Option<f64>> = vec![Some(1.0); 16 * 31];
        let out = warp_field(
            &source(&geometry, "rotated_latlon"),
            &raw,
            WarpTarget::Equirectangular,
            &resolved("nearest", None),
        )
        .expect("rotated lat/lon warp");

        let present = out.mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "rotated warp produced an empty mask");
        for (i, &m) in out.mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(out.values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        // The geographic extent is the unrotated perimeter box. With the pole at
        // (0,0) the fixture's grid sweeps the high-latitude north side; the
        // reported box must be non-degenerate and stay within valid ranges.
        let LonLatBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        } = LonLatBox::from_array(out.bounds.expect("rotated grid has bounds"));
        assert!(
            lat_max > lat_min && lat_max <= 90.01,
            "lat box {lat_min}..{lat_max}"
        );
        assert!(lon_max > lon_min, "lon box {lon_min}..{lon_max}");
        assert!(out.width > 0 && out.height > 0);
        assert!(
            out.summary.contains("rotated_latlon") && out.summary.contains("equirectangular"),
            "summary should name source kind + target, got: {}",
            out.summary
        );
    }
}
#[cfg(test)]
mod planar_geolocation_tests {
    use super::*;
    use fieldglass_core::{
        GaussianParams, GeostationaryParams, LambertAzimuthalParams, LambertParams, LatLonParams,
        MercatorParams, PolarStereoParams, RotatedLatLonParams, SpatialIndex,
        TransverseMercatorParams,
    };

    /// NOAA Eta, GDS template 3.30 (Lambert conformal), 93×65 at 81.271 km.
    const ETA_LAMBERT: &str = "../fieldglass-grib2/tests/fixtures/eta_lambert_msg0.grib2";
    /// CMC 300 hPa wind, GRIB1 grid type 5 (polar stereographic), 135×95 at
    /// 60 km. Its top row runs across the antimeridian, which is why it is the
    /// fixture for the longitude-cut case below.
    const CMC_POLAR: &str = "../fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib";

    /// The geometry of message 0 of a GRIB2 fixture, as the decoder states it.
    fn grib2_geometry(path: &str) -> GridGeometry {
        let bytes = std::fs::read(path).expect("fixture");
        let reader = fieldglass_grib2::Grib2Reader::from_bytes(bytes).expect("grib2 parse");
        GridGeometry::from(&reader.messages[0].gds)
    }

    /// The geometry of message 0 of a GRIB1 fixture.
    fn grib1_geometry(path: &str) -> GridGeometry {
        let bytes = std::fs::read(path).expect("fixture");
        let reader = fieldglass_grib1::Grib1Reader::from_bytes(bytes).expect("grib1 parse");
        let gds = reader.messages[0]
            .gds
            .as_ref()
            .expect("the message carries a GDS");
        GridGeometry::from(gds)
    }

    /// A [`Source`] over a geometry that resolved, with the family name a host
    /// would print for it and the scan a message with no flags reads as.
    ///
    /// The dimensions fall back to zero for a family that states none — an
    /// unmodelled one — because the cases that use those are asking what the
    /// operation *refuses*, and a refusal never reaches the raster.
    fn source<'a>(geometry: &'a GridGeometry, family: &'a str) -> Source<'a> {
        let (ni, nj) = geometry.dims().unwrap_or((0, 0));
        Source {
            geometry: Ok(geometry),
            ni,
            nj,
            scan: Scan::north_down(),
            family,
        }
    }

    /// The Lambert cone the Eta fixture declares, so a case below can collapse
    /// one parameter of it and leave the rest real.
    fn eta_lambert_params() -> LambertParams {
        match grib2_geometry(ETA_LAMBERT) {
            GridGeometry::Lambert(p) => p,
            other => panic!("the Eta fixture is a Lambert grid, got {}", other.kind()),
        }
    }

    /// The polar stereographic grid the CMC fixture declares.
    fn cmc_polar_params() -> PolarStereoParams {
        match grib1_geometry(CMC_POLAR) {
            GridGeometry::PolarStereo(p) => p,
            other => panic!("the CMC fixture is a polar grid, got {}", other.kind()),
        }
    }

    /// A cell the ±180° cut runs through interpolates *across* the cut, not the
    /// long way round the globe. The CMC polar grid's top rows cross the
    /// antimeridian, so the naive average of two corners a few kilometres apart
    /// lands half a world away — the streak this guards against.
    #[test]
    fn a_cell_straddling_the_longitude_cut_interpolates_across_it() {
        let geometry = grib1_geometry(CMC_POLAR);
        let (ni, nj) = geometry.dims().expect("the polar grid states dims");
        assert_eq!((ni, nj), (135, 95), "the CMC fixture's raster");
        let forward = forward_geolocation(&geometry).expect("a polar grid geolocates");
        let lon = |i, j| forward(i, j).expect("every point geolocates").1;

        // The first cell whose two upper corners sit on opposite sides of the
        // cut — a contour vertex on that edge is the one that used to jump. The
        // search finding a cell at all is part of the assertion: without one the
        // test would pass vacuously.
        let straddle = (0..nj - 1)
            .flat_map(|j| (0..ni - 1).map(move |i| (i, j)))
            .find(|&(i, j)| (lon(i + 1, j) - lon(i, j)).abs() > 180.0);
        let (i, j) = straddle.expect("the CMC polar grid crosses the antimeridian");
        let (west, east) = (lon(i, j), lon(i + 1, j));
        assert!(
            cell_crosses_lon_cut([west, east, lon(i, j + 1), lon(i + 1, j + 1)]),
            "the cut runs through cell ({i},{j}): {west}..{east}"
        );

        let (_, mid) = forward_bilinear(forward.as_ref(), ni, nj, i as f64 + 0.5, j as f64, false)
            .expect("the cell's midpoint geolocates");
        // Half the short way from the west corner to the east one.
        let want = normalise_lon(west + normalise_lon(east - west) * 0.5);
        assert!(
            normalise_lon(mid - want).abs() < 1e-9,
            "cell ({i},{j}) spans {west}..{east}; its midpoint read {mid}, want {want}"
        );
        // What it used to read: the plain average of two corners on opposite
        // turns, most of a hemisphere from either of them.
        let naive = (west + east) / 2.0;
        assert!(
            normalise_lon(naive - want).abs() > 90.0,
            "this cell should be one the naive average gets badly wrong, \
             got {naive} against {want}"
        );
        // The vertex an unstraddled cell reports is untouched by any of this.
        let m = ni / 2;
        let (_, plain) = forward_bilinear(forward.as_ref(), ni, nj, m as f64 + 0.5, 0.0, false)
            .expect("a mid-grid vertex geolocates");
        let (w, e) = (lon(m, 0), lon(m + 1, 0));
        assert!(
            (plain - (w + e) / 2.0).abs() < 1e-9,
            "an ordinary cell still interpolates plainly: {plain} vs {w}..{e}"
        );
    }

    /// A grid whose projection constants are degenerate is refused outright,
    /// rather than exporting the coordinates such a projector still produces.
    /// The Lambert cone collapses when both standard parallels sit on the
    /// equator (`n = sin 0 = 0`), and the collapse is quiet: the arithmetic
    /// stays finite and every grid point simply inverts to the pole, so a CSV
    /// row of `-90,117.9,101333` looks exactly like a position.
    ///
    /// Both directions are asked. The warp refuses through
    /// [`require_reprojectable`]; the forward map — what the long CSV and the
    /// contour tracer read — must refuse with the same words, or the export
    /// comes back as a header with no rows under it, which reads as "no data"
    /// rather than as a broken grid.
    #[test]
    fn a_degenerate_projection_is_refused_rather_than_geolocated() {
        let real = GridGeometry::Lambert(eta_lambert_params());
        let flat_cone = GridGeometry::Lambert(LambertParams {
            latin1: 0.0,
            latin2: 0.0,
            ..eta_lambert_params()
        });

        let collapsed = source(&flat_cone, "lambert");
        let err = require_forward_geolocation(&collapsed, |gt| format!("unsupported {gt}"))
            .err()
            .expect("a collapsed cone has no forward map");
        assert!(
            err.message().contains("degenerate"),
            "the message should say why, got: {}",
            err.message()
        );
        assert!(
            require_reprojectable(&flat_cone, "lambert").is_err(),
            "and the warp refuses the same grid"
        );

        // The grid it was cloned from still geolocates, so the refusal is about
        // the parameters and not the family.
        let intact = source(&real, "lambert");
        assert!(
            require_forward_geolocation(&intact, |_| String::new()).is_ok(),
            "the real Eta cone geolocates"
        );
        assert!(require_reprojectable(&real, "lambert").is_ok());
    }

    /// The other way to collapse a polar stereographic plane, and the one a
    /// radius check cannot reach: `LaD` past ±90°.
    ///
    /// The pole scale factor `k₀ = (1 + sin|LaD|)/2` is in `[0.5, 1]` for a real
    /// latitude of true scale, which is why this arm carried no constants check
    /// at all. §3.20 states `LaD` in sign-magnitude microdegrees though, so a
    /// message can say 270°, where `sin|LaD|` is -1 and `k₀` is exactly zero.
    /// The radius is untouched and every number stays finite, so a host's own
    /// radius guard passes it straight through and only the projector's check
    /// refuses it (#603).
    #[test]
    fn a_polar_stereo_lad_past_the_pole_is_refused() {
        let real = GridGeometry::PolarStereo(cmc_polar_params());
        let intact = source(&real, "polar_stereo");
        assert!(
            require_forward_geolocation(&intact, |gt| format!("unsupported {gt}")).is_ok(),
            "the real CMC grid geolocates"
        );

        let past_the_pole = GridGeometry::PolarStereo(PolarStereoParams {
            lad: 270.0,
            ..cmc_polar_params()
        });
        // The radius is intact, so the check that catches this is the
        // projector's and not a guard on the declared sphere.
        let radius = cmc_polar_params().earth_radius_m;
        assert!(
            radius.is_finite() && radius > 0.0,
            "the radius has nothing to object to here, but reads {radius}"
        );
        let collapsed = source(&past_the_pole, "polar_stereo");
        let err = require_forward_geolocation(&collapsed, |gt| format!("unsupported {gt}"))
            .err()
            .expect("a zero pole scale factor leaves no plane");
        assert!(
            err.message().contains("degenerate projection parameters"),
            "the message should name the degeneracy, got: {}",
            err.message()
        );
        assert!(
            require_reprojectable(&past_the_pole, "polar_stereo").is_err(),
            "and the warp refuses it too"
        );
    }

    /// The table is the gate: every grid type it names resolves to a forward
    /// map, and the refusal message for one it doesn't name reads the same list
    /// back. A family added to the dispatch without a table row simply never
    /// arrives here, and a row without a dispatch arm fails the first half.
    ///
    /// The keys are the *decoder's* family names and the dispatch is over
    /// [`GridGeometry`], so the two spellings differ for three rows: the reduced
    /// pair arrive widened to their regular sibling, and `"curvilinear"` arrives
    /// as [`GridGeometry::Lookup`]. That mapping is the interesting half — it is
    /// where a row could quietly stop matching anything.
    #[test]
    fn the_geolocatable_table_matches_the_dispatch() {
        let prose = geolocatable_families();
        for (grid_type, name) in GEOLOCATABLE_GRIDS {
            assert!(
                prose.contains(name),
                "{grid_type} is geolocatable but {name:?} is missing from {prose:?}"
            );
        }
        assert!(
            prose.starts_with("regular lat/lon, Mercator") && prose.contains(", and "),
            "the families read as a list: {prose}"
        );

        for (grid_type, _) in GEOLOCATABLE_GRIDS {
            let geometry = representative(grid_type);
            let place = forward_geolocation(&geometry).unwrap_or_else(|| {
                panic!(
                    "{grid_type} is in the table but its geometry ({}) has no forward map",
                    geometry.kind()
                )
            });
            // And it really does place a point, so an arm wired to a geometry
            // whose parameters it cannot use fails here rather than exporting a
            // CSV header with no rows under it.
            assert!(
                place(0, 0).is_some(),
                "{grid_type}: the first grid point has no position"
            );
        }

        // The families that deliberately have no forward map: space view, where
        // points off the disc have no position at all, and one this build does
        // not model. Reduced grids used to be here; they are widened to a
        // regular raster before any consumer sees them, and that raster
        // geolocates (#503).
        for (grid_type, geometry) in [
            ("space_view", representative("space_view")),
            (
                "bifourier",
                GridGeometry::Unsupported {
                    label: "bifourier".to_string(),
                },
            ),
            (
                "",
                GridGeometry::Unsupported {
                    label: String::new(),
                },
            ),
        ] {
            let declined = source(&geometry, grid_type);
            let err = require_forward_geolocation(&declined, |gt| {
                format!(
                    "no coordinates for {gt:?} (only {})",
                    geolocatable_families()
                )
            })
            .err()
            .unwrap_or_else(|| panic!("{grid_type} must not geolocate"));
            let message = err.message();
            assert!(
                message.contains(grid_type) && message.contains(&prose),
                "the refusal for {grid_type:?} should name the grid and the \
                 supported list: {message}"
            );
        }
    }

    /// One well-formed geometry per [`GEOLOCATABLE_GRIDS`] key, plus the space
    /// view. Written out rather than read from fixtures because the point is the
    /// *key → variant* mapping: a row whose family no longer resolves to a
    /// variant with a forward map has to fail, and a corpus that happens not to
    /// carry that family would hide it.
    fn representative(grid_type: &str) -> GridGeometry {
        match grid_type {
            // Both reduced rows arrive widened to their regular sibling.
            "latlon" | "reduced_latlon" => GridGeometry::LatLon(LatLonParams {
                ni: 8,
                nj: 5,
                lat_first: 60.0,
                lon_first: -10.0,
                lat_last: 40.0,
                lon_last: 25.0,
            }),
            "gaussian" | "reduced_gaussian" => GridGeometry::Gaussian(GaussianParams {
                ni: 16,
                nj: 8,
                lat_first: 79.0,
                lon_first: 0.0,
                lat_last: -79.0,
                lon_last: 337.5,
                n_parallels: 4,
            }),
            "mercator" => GridGeometry::Mercator(MercatorParams {
                ni: 8,
                nj: 5,
                lat_first: 40.0,
                lon_first: -10.0,
                lat_last: 20.0,
                lon_last: 25.0,
            }),
            "rotated_latlon" => GridGeometry::RotatedLatLon(RotatedLatLonParams {
                ni: 8,
                nj: 5,
                lat_first: 10.0,
                lon_first: -10.0,
                lat_last: -10.0,
                lon_last: 25.0,
                south_pole_lat: -30.0,
                south_pole_lon: 10.0,
                angle_of_rotation: 0.0,
            }),
            // A lookup grid is a list of cell centres, so its representative is
            // the smallest index that builds: four cells with real positions.
            "curvilinear" => GridGeometry::Lookup(
                SpatialIndex::new(2, 2, &[50.0, 50.0, 49.0, 49.0], &[-1.0, 0.0, -1.0, 0.0])
                    .expect("four finite centres build an index"),
            ),
            "lambert" => GridGeometry::Lambert(eta_lambert_params()),
            "polar_stereo" => GridGeometry::PolarStereo(cmc_polar_params()),
            "transverse_mercator" => GridGeometry::TransverseMercator(TransverseMercatorParams {
                semi_major_m: 6_377_563.396,
                semi_minor_m: 6_356_256.909,
                ni: 8,
                nj: 5,
                lat_ref: 49.0,
                lon_ref: -2.0,
                scale_factor: 0.999_601_27,
                false_easting_m: 400_000.0,
                false_northing_m: -100_000.0,
                x1_metres: 200_000.0,
                y1_metres: 200_000.0,
                dx_metres: 2_000.0,
                dy_metres: 2_000.0,
            }),
            "lambert_azimuthal" => GridGeometry::LambertAzimuthal(LambertAzimuthalParams {
                semi_major_m: 6_378_137.0,
                semi_minor_m: 6_356_752.314_2,
                ni: 8,
                nj: 5,
                lat_first: 35.0,
                lon_first: -10.0,
                standard_parallel: 52.0,
                central_longitude: 10.0,
                dx_metres: 250_000.0,
                dy_metres: 250_000.0,
            }),
            "space_view" => GridGeometry::Geostationary(GeostationaryParams {
                ni: 8,
                nj: 5,
                h_metres: 35_786_023.0,
                r_eq: 6_378_137.0,
                r_pol: 6_356_752.314_2,
                sub_lon_deg: -75.0,
                sweep_x: true,
                x0: -0.101_332,
                dx_rad: 5.6e-5,
                y0: 0.128_212,
                dy_rad: -5.6e-5,
            }),
            other => panic!("no representative geometry for {other:?}"),
        }
    }
}
