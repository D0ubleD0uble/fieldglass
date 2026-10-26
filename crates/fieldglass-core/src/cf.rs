//! Functions that follow the CF conventions over those descriptions: which
//! arrays can be drawn, which axes are horizontal, vertical or time, where a
//! slice sits on the Earth, and how stored values become physical units. It
//! also reads the map settings WRF writes in its own attributes.
//!
//! The CF (Climate and Forecast) conventions are the rules most NetCDF and
//! Zarr weather data follows to say what its arrays mean. An array is often
//! 3-D or 4-D, such as time × level × latitude × longitude, and nothing in the
//! file format says which axes are horizontal or where the grid sits. CF says
//! it with attributes: `units`, `standard_name` and `axis` on coordinate
//! arrays, `coordinates` naming a pair of 2-D latitude and longitude arrays,
//! and `grid_mapping` naming a map projection. WRF, a widely used weather
//! model, doesn't follow CF and states its projection in global attributes of
//! its own, so those are read here too.
//!
//! These rules read only attributes and coordinate values, never a file
//! layout, so they are written once over
//! [`ArraySource`](crate::array::ArraySource) and
//! [`Group`](crate::array::Group). A NetCDF file and a Zarr store written by
//! xarray carry the same attributes, and both are placed by the same code.
//!
//! The work is split three ways, and the most used items of each are also
//! available directly from this module:
//!
//! - [`geometry`] finds each array's axes and lists the arrays that can be
//!   drawn.
//! - [`placement`] decides where one 2-D slice sits on the Earth, trying each
//!   way a file can say so in a fixed order.
//! - [`resolvers`] turns a file's projection attributes into projection
//!   parameters, for CF's geostationary mapping and WRF's projections, and
//!   applies CF's scale and offset.
//!
//! A name an attribute gives, such as `coordinates = "lat lon"` or
//! `grid_mapping = "crs"`, is looked up in the group of the array that gave
//! it, and compared the way
//! [`Group::arrays_qualified`](crate::array::Group::arrays_qualified) spells
//! names.

pub mod geometry;
pub mod placement;
pub mod resolvers;

pub use geometry::{
    AxisKind, CurvilinearPair, Positive, RenderableArray, SliceGeometry, VerticalAxis,
    corner_and_regularity, curvilinear_axes, curvilinear_pair, detect_axis, extract_plane,
    is_time_axis, read_plane, renderable_arrays, synthesize_geometry, undrawable_arrays,
    vertical_axis,
};
pub use placement::{
    CfMapping, SOURCE_ONLY, SlicePlacement, classify_grid_mapping, coordinate_plane,
    curvilinear_lookup, slice_placement,
};
pub use resolvers::{
    GeostationaryGrid, WRF_EARTH_RADIUS_M, WrfLambertGrid, WrfLatLonGrid, WrfMapProj,
    WrfMercatorGrid, WrfPolarStereoGrid, apply_scale_offset, cf_scale_offset,
    resolve_cf_geostationary, resolve_wrf_lambert, resolve_wrf_latlon, resolve_wrf_mercator,
    resolve_wrf_polar_stereo, unpack_cf_data, wrf_map_proj,
};
