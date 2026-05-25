#![forbid(unsafe_code)]

pub mod bits;
pub mod colormap;
pub mod detect;
pub mod error;
pub mod metadata;
pub mod projection;
pub mod reader;
pub mod warp;

pub use detect::Format;
pub use detect::detect_format;
pub use detect::detect_from_bytes;
pub use error::FieldglassError;
pub use metadata::GridDefinition;
pub use metadata::Level;
pub use metadata::Metadata;
pub use metadata::Parameter;
pub use projection::{
    GaussianParams, GaussianProjector, GridIndex, LambertParams, LambertProjector, LatLonParams,
    PlanarGridProjector, PolarStereoParams, PolarStereoProjector, gaussian_inverse,
    gaussian_latitudes, lambert_forward, lambert_inverse, lambert_inverse_xy, latlon_inverse,
    polar_stereo_forward, polar_stereo_inverse, polar_stereo_inverse_xy,
};
pub use reader::DataMessage;
pub use reader::FormatReader;
pub use warp::{
    Orthographic, PolarStereographic, Resampling, SourceGrid, TargetProjection, TargetRaster,
    WarpedRaster, WebMercator, warp, warp_to_equirectangular,
};
