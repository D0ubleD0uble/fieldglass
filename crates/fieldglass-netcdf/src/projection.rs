//! The projected-grid resolvers — CF geostationary and WRF's `MAP_PROJ`
//! domains — and the CF scale helpers, re-exported from where they live now.
//!
//! They moved to [`fieldglass_core::cf::resolvers`] with #704: they read
//! attributes and coordinate values and nothing about a NetCDF file, and a Zarr
//! store written by xarray needs exactly the same rules. This module keeps the
//! paths this crate has always published, so a caller naming
//! `fieldglass_netcdf::projection::resolve_wrf_lambert` still compiles.

pub use fieldglass_core::cf::resolvers::{
    GeostationaryGrid, WRF_EARTH_RADIUS_M, WrfLambertGrid, WrfLatLonGrid, WrfMapProj,
    WrfMercatorGrid, WrfPolarStereoGrid, apply_scale_offset, cf_scale_offset,
    resolve_cf_geostationary, resolve_wrf_lambert, resolve_wrf_latlon, resolve_wrf_mercator,
    resolve_wrf_polar_stereo, unpack_cf_data, wrf_map_proj,
};
