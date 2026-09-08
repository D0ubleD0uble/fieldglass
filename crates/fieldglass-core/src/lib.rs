#![forbid(unsafe_code)]
//! Format-agnostic traits and shared types for the Fieldglass data viewer.
//!
//! The crate serves two audiences behind one API. The format crates
//! (`fieldglass-grib1`, `-grib2`, `-netcdf`) consume only the *parsing*
//! surface:
//!
//! <!-- parsing-surface: the set of core modules the three format crate
//!      libraries name, checked by tools/check_parsing_surface.py. The README
//!      states it again; both regions have to match the code. -->
//! [`error`], [`bits`], [`bytes`], [`cct_tables`] (both GRIB editions share the
//! WMO sub-centre lookup), [`projection`] (GRIB1's GDS uses the projectors to
//! recover grid corners), [`scan`] (the storage orders a decoder regularises),
//! [`lead_time`] (the forecast-lead rules the two editions share), the three
//! grids that arrive as something other than a rectangle of values — [`sht`],
//! [`matrix`], and [`healpix`] — [`global_grid`], the lat/lon grid the
//! first and last of those are put onto, and [`spatial_index`], which
//! `fieldglass-netcdf` builds over a swath's 2-D coordinate arrays so a grid
//! that is a list of cell centres can be placed like any other (#549).
//! <!-- /parsing-surface -->
//!
//! What those modules have in common is that none of them is behind a feature,
//! which is what makes a `default-features = false` dependency work. Pre-commit
//! builds the three format crate libraries against a `core` with every feature
//! off, so reaching for gated code fails there rather than at a consumer — and
//! checks the list above against what those libraries actually name, which is
//! the stronger claim the sentence is making. `detect`, `spatial_index` and
//! `units` are ungated too and are deliberately not on it: no format crate
//! library uses them, and a list that quietly grows says nothing about how
//! small the surface is.
//!
//! # Feature flags
//!
//! - **`render`** *(default)* — the viewer-domain modules `warp`, `overlay`,
//!   and `colormap`, consumed only by `fieldglass-napi`. Depend with
//!   `default-features = false` to get just the parsing surface (no warp
//!   pipeline in your API). [`projection`] stays available either way, since
//!   decode-side consumers need it.
//! - **`analysis`** *(default)* — the modules `contour`, `csv`, and `combine`:
//!   operations that take a decoded field and return values, not pixels.
//!   Separate from `render` because a host can want isolines without the
//!   painter, and separate from the parsing surface because a decode-only
//!   consumer should not compile them at all.
//! - **`serde`** *(default)* — the `Serialize` / `Deserialize` derives on the
//!   geometry, colour and spatial types (#641). On by default so nothing
//!   downstream changes by upgrading; the saving is for the crates that already
//!   take this one with `default-features = false` — the four format crates —
//!   which stop resolving `serde`, `serde_core` and `serde_derive` for derives
//!   their parsing surface never reaches. `fieldglass` names it back
//!   unconditionally, because its wire types embed this crate's geometry and it
//!   is the layer with something to serialise. One member is gated by hand
//!   rather than by attribute: `colormap`'s lookup table is past the
//!   32-element ceiling serde derives array impls up to, so it carries its own
//!   `serialize` / `deserialize` pair.
//! - **`fs`** *(default)* — `detect::detect_format`, which opens a path. The
//!   only call in this crate that touches a host filesystem; nothing in the
//!   workspace calls it, and a format crate could not, since all three take
//!   `core` with `default-features = false`. Depend that way on a target
//!   without a filesystem;
//!   `wasm32-unknown-unknown` compiles `std::fs` but fails every call at
//!   runtime, so the gate is what stops detection from silently falling back to
//!   guessing from the file extension. Not a `no_std` switch — the crate still
//!   links `std` for float math.

// The shape model of a chunked array: which chunk holds a value, and what the
// object holding it is called (ADR-0010 decision 1). Ungated and dependency
// free — pure integer arithmetic — so a consumer that takes one format crate
// links nothing new because of it.
pub mod array;
pub mod bits;
pub mod bytes;
pub mod cct_tables;
#[cfg(feature = "render")]
pub mod colormap;
/// Generated colormap anchor tables (`tools/gen_colormaps.py`).
#[cfg(feature = "render")]
mod colormap_tables;
#[cfg(feature = "analysis")]
pub mod combine;
#[cfg(feature = "analysis")]
pub mod contour;
#[cfg(feature = "analysis")]
pub mod csv;
/// Format sniffing from the leading bytes of a file.
pub mod detect;
/// The crate's one error type.
pub mod error;
pub mod global_grid;
pub mod healpix;
pub mod lead_time;
pub mod matrix;
#[cfg(feature = "render")]
pub mod overlay;
pub mod projection;
pub mod scan;
pub mod sht;
pub mod spatial_index;
pub mod units;
#[cfg(feature = "render")]
pub mod warp;

pub use array::{ArrayError, ChunkGrid, ChunkKeyEncoding};
pub use bytes::{ByteRange, ByteSource, MemoryObjects, ObjectSource};
#[cfg(feature = "analysis")]
pub use combine::{CombineOp, combine_cell, combine_fields};
#[cfg(feature = "analysis")]
pub use contour::{
    ContourLevel, GridSegment, contour_segments, contour_segments_global, nice_levels,
};
pub use detect::Format;
#[cfg(feature = "fs")]
pub use detect::detect_format;
pub use detect::detect_from_bytes;
pub use error::{FieldglassError, printable_bytes};
pub use global_grid::{GlobalGrid, SynthesisedField};
#[cfg(feature = "render")]
pub use overlay::{ProjectedPolylines, SourceOverlayTarget, project_polylines};
// The projector types, plus the free functions a format crate or a host calls.
// The per-call `<family>_forward` / `_inverse` wrappers that used to sit here
// were a second public path to arithmetic the projectors already expose, so
// they are gone; build the projector once and call its methods.
pub use projection::{
    CornerPair, DEFAULT_EARTH_RADIUS_M, ForwardAt, GaussianParams, GaussianProjector,
    GeostationaryParams, GeostationaryProjector, GridGeometry, GridIndex, GridResampling,
    LambertAzimuthalParams, LambertAzimuthalProjector, LambertParams, LambertProjector,
    LatLonParams, LonLatBox, MercatorParams, PlanarGridProjector, PlaneAffine, PlaneUnits,
    PolarStereoParams, PolarStereoProjector, RotatedLatLonParams, RotatedLatLonProjector, Scan,
    TransverseMercatorParams, TransverseMercatorProjector, eastward_lon_span,
    expand_reduced_to_regular, gaussian_latitudes, is_octahedral_pl, latlon_inverse, latlon_point,
    lon_grid_is_global, mercator_inverse, mercator_point, normalise_lon, plane_spans_a_grid_cell,
    reduced_raster_lon_last, reduced_raster_width, rotated_latlon_point, signed_grid_increments,
};
pub use scan::{StoredRuns, reverse_alternate_runs, transpose_j_consecutive};
pub use spatial_index::SpatialIndex;
#[cfg(feature = "render")]
pub use warp::{
    EqualEarth, ForwardMap, Mollweide, Orthographic, PolarStereographic, PreparedTarget,
    Resampling, Robinson, SourceGrid, TargetProjection, TargetRaster, WarpedRaster, WebMercator,
    warp, warp_to_equirectangular,
};
