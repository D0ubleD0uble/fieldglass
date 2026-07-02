#![forbid(unsafe_code)]

pub mod bits;
pub mod colormap;
pub mod detect;
pub mod error;
pub mod metadata;
pub mod overlay;
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
pub use overlay::{ProjectedPolylines, SourceOverlayTarget, project_polylines};
pub use projection::{
    GaussianParams, GaussianProjector, GeostationaryParams, GeostationaryProjector, GridIndex,
    LambertParams, LambertProjector, LatLonParams, MercatorParams, PlanarGridProjector,
    PolarStereoParams, PolarStereoProjector, RotatedLatLonParams, RotatedLatLonProjector,
    eastward_lon_span, gaussian_inverse, gaussian_latitudes, geostationary_inverse,
    lambert_forward, lambert_inverse, lambert_inverse_xy, latlon_inverse, lon_grid_is_global,
    mercator_inverse, polar_stereo_forward, polar_stereo_inverse, polar_stereo_inverse_xy,
    rotate_latlon, unrotate_latlon,
};
pub use reader::DataMessage;
pub use reader::FormatReader;
pub use warp::{
    ForwardMap, Orthographic, PolarStereographic, PreparedTarget, Resampling, SourceGrid,
    TargetProjection, TargetRaster, WarpedRaster, WebMercator, warp, warp_to_equirectangular,
};
