#![forbid(unsafe_code)]
//! Format-agnostic traits and shared types for the Fieldglass data viewer.
//!
//! The crate serves two audiences behind one API. The format crates
//! (`fieldglass-grib1`, `-grib2`, `-netcdf`) consume only the *parsing* surface:
//! [`error`], [`bits`], [`detect`], [`reader`], [`metadata`], and
//! [`projection`] (GRIB1's GDS uses the projectors to recover grid corners).
//!
//! # Feature flags
//!
//! - **`render`** *(default)* — the viewer-domain modules `warp`, `overlay`,
//!   and `colormap`, consumed only by `fieldglass-napi`. Depend with
//!   `default-features = false` to get just the parsing surface (no warp
//!   pipeline in your API). [`projection`] stays available either way, since
//!   decode-side consumers need it.

pub mod bits;
#[cfg(feature = "render")]
pub mod colormap;
/// Generated colormap anchor tables (`tools/gen_colormaps.py`).
#[cfg(feature = "render")]
mod colormap_tables;
pub mod combine;
pub mod contour;
pub mod csv;
pub mod detect;
pub mod error;
pub mod metadata;
#[cfg(feature = "render")]
pub mod overlay;
pub mod projection;
pub mod reader;
pub mod sht;
#[cfg(feature = "render")]
pub mod warp;

pub use combine::{CombineOp, combine_fields};
pub use contour::{ContourLevel, GridSegment, contour_segments, nice_levels};
pub use detect::Format;
pub use detect::detect_format;
pub use detect::detect_from_bytes;
pub use error::FieldglassError;
pub use metadata::GridDefinition;
pub use metadata::Level;
pub use metadata::Metadata;
pub use metadata::Parameter;
#[cfg(feature = "render")]
pub use overlay::{ProjectedPolylines, SourceOverlayTarget, project_polylines};
pub use projection::{
    DEFAULT_EARTH_RADIUS_M, GaussianParams, GaussianProjector, GeostationaryParams,
    GeostationaryProjector, GridIndex, LambertParams, LambertProjector, LatLonParams,
    MercatorParams, PlanarGridProjector, PolarStereoParams, PolarStereoProjector,
    RotatedLatLonParams, RotatedLatLonProjector, eastward_lon_span, gaussian_inverse,
    gaussian_latitudes, geostationary_inverse, lambert_forward, lambert_inverse,
    lambert_inverse_xy, latlon_inverse, latlon_point, lon_grid_is_global, mercator_inverse,
    mercator_point, normalise_lon, polar_stereo_forward, polar_stereo_inverse,
    polar_stereo_inverse_xy, rotate_latlon, rotated_latlon_point, unrotate_latlon,
};
pub use reader::DataMessage;
pub use reader::FormatReader;
#[cfg(feature = "render")]
pub use warp::{
    EqualEarth, ForwardMap, Mollweide, Orthographic, PolarStereographic, PreparedTarget,
    Resampling, Robinson, SourceGrid, TargetProjection, TargetRaster, WarpedRaster, WebMercator,
    warp, warp_to_equirectangular,
};
