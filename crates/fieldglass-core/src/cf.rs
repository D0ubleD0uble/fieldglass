//! The CF conventions over the array model: which arrays a slice picker
//! offers, which of their axes are the horizontal ones, and where a slice sits
//! on the Earth (#704).
//!
//! None of this is NetCDF's. The rules are the CF conventions — `units` and
//! `standard_name` on a coordinate, `coordinates` naming a 2-D pair,
//! `grid_mapping` naming a CRS — plus WRF's `MAP_PROJ` globals, and they read
//! attributes and coordinate values, never a file layout. They lived in
//! `fieldglass-netcdf` because NetCDF was the only container that needed them;
//! a Zarr store written by xarray carries exactly the same attributes, and a
//! format crate cannot depend on a sibling (ADR-0010 decision 6). So they are
//! written here, once, over [`ArraySource`](crate::array::ArraySource) and the
//! core [`Group`](crate::array::Group), and every container of named arrays
//! places its slices by the same rules.
//!
//! * [`geometry`] — CF axis detection, the renderable-array list, the 2-D
//!   coordinate pair a `coordinates` attribute names, and the plane and
//!   corner helpers the placement is built from.
//! * [`placement`] — the precedence that picks a slice's geometry, and its
//!   guard against reading a projected CRS's metres as degrees.
//! * [`resolvers`] — the pure resolvers each arm of that precedence calls:
//!   CF geostationary and WRF's four `MAP_PROJ` domains, attributes and
//!   coordinate values in, parameters out.
//!
//! Names are compared the way [`Group::arrays_qualified`](crate::array::Group::arrays_qualified)
//! spells them, and a name an attribute gives (`coordinates = "lat lon"`,
//! `grid_mapping = "crs"`, WRF's `XLAT`) is looked up in the group of the array
//! that gave it — which for a container that keeps every array in its root
//! group, as the NetCDF view does, is the whole container.

pub mod geometry;
pub mod placement;
pub mod resolvers;

pub use geometry::{
    AxisKind, CurvilinearPair, RenderableArray, SliceGeometry, corner_and_regularity,
    curvilinear_axes, curvilinear_pair, detect_axis, extract_plane, renderable_arrays,
    synthesize_geometry,
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
