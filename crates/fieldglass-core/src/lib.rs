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
    GaussianParams, GridIndex, LambertParams, LatLonParams, gaussian_inverse, gaussian_latitudes,
    lambert_forward, lambert_inverse, lambert_inverse_xy, latlon_inverse,
};
pub use reader::DataMessage;
pub use reader::FormatReader;
pub use warp::{Resampling, SourceGrid, TargetRaster, WarpedRaster, warp_to_equirectangular};
