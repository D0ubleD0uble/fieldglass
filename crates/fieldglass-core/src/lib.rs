#![forbid(unsafe_code)]
//! Format-agnostic traits and shared types for the Fieldglass data viewer.
//!
//! The crate serves two audiences behind one API. The format crates
//! (`fieldglass-grib1`, `-grib2`, `-netcdf`) consume only the *parsing*
//! surface: [`error`], [`bits`], [`bytes`], [`detect`], [`cct_tables`] (both
//! GRIB editions share the WMO sub-centre lookup), [`projection`] (GRIB1's GDS
//! uses the projectors to recover grid corners), [`scan`] (the storage orders a
//! decoder regularises), and the three grids that arrive as something other
//! than a rectangle of values — [`sht`], [`matrix`], and [`healpix`]. What
//! those modules have in common is that none of them is behind a feature, which
//! is what makes a `default-features = false` dependency work. Pre-commit builds
//! the three format crate libraries against a `core` with every feature off,
//! so reaching for gated code fails there rather than at a consumer.
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
//! - **`fs`** *(default)* — `detect::detect_format`, which opens a path. The
//!   only host-filesystem call in the format crates. Depend with
//!   `default-features = false` on a target without a filesystem;
//!   `wasm32-unknown-unknown` compiles `std::fs` but fails every call at
//!   runtime, so the gate is what stops detection from silently falling back to
//!   guessing from the file extension. Not a `no_std` switch — the crate still
//!   links `std` for float math.

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
pub mod healpix;
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

pub use bytes::{ByteRange, ByteSource};
#[cfg(feature = "analysis")]
pub use combine::{CombineOp, combine_fields};
#[cfg(feature = "analysis")]
pub use contour::{
    ContourLevel, GridSegment, contour_segments, contour_segments_global, nice_levels,
};
pub use detect::Format;
#[cfg(feature = "fs")]
pub use detect::detect_format;
pub use detect::detect_from_bytes;
pub use error::FieldglassError;
#[cfg(feature = "render")]
pub use overlay::{ProjectedPolylines, SourceOverlayTarget, project_polylines};
// The projector types, plus the free functions a format crate or a host calls.
// The per-call `<family>_forward` / `_inverse` wrappers that used to sit here
// were a second public path to arithmetic the projectors already expose, so
// they are gone; build the projector once and call its methods.
pub use projection::{
    CornerPair, DEFAULT_EARTH_RADIUS_M, GaussianParams, GaussianProjector, GeostationaryParams,
    GeostationaryProjector, GridGeometry, GridIndex, GridResampling, LambertAzimuthalParams,
    LambertAzimuthalProjector, LambertParams, LambertProjector, LatLonParams, LonLatBox,
    MercatorParams, PlanarGridProjector, PlaneAffine, PlaneUnits, PolarStereoParams,
    PolarStereoProjector, RotatedLatLonParams, RotatedLatLonProjector, TransverseMercatorParams,
    TransverseMercatorProjector, eastward_lon_span, expand_reduced_to_regular, gaussian_latitudes,
    is_octahedral_pl, latlon_inverse, latlon_point, lon_grid_is_global, mercator_inverse,
    mercator_point, normalise_lon, plane_spans_a_grid_cell, reduced_raster_lon_last,
    reduced_raster_width, rotated_latlon_point, signed_grid_increments,
};
pub use scan::{StoredRuns, reverse_alternate_runs, transpose_j_consecutive};
pub use spatial_index::SpatialIndex;
#[cfg(feature = "render")]
pub use warp::{
    EqualEarth, ForwardMap, Mollweide, Orthographic, PolarStereographic, PreparedTarget,
    Resampling, Robinson, SourceGrid, TargetProjection, TargetRaster, WarpedRaster, WebMercator,
    warp, warp_to_equirectangular,
};
