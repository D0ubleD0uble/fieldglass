//! NetCDF format reader. Covers the classic on-disk layout (CDF-1, CDF-2,
//! CDF-5) and the NetCDF-4 / HDF5 layout end to end: dimensions, variables,
//! and attributes, plus per-variable value decode into `Vec<Option<f64>>`.
//! See the per-module docs for the per-layout detail.

#![forbid(unsafe_code)]
// `missing_docs` is `warn` in `[workspace.lints]`, and 118 public items in this
// crate do not satisfy it yet. Allowed here rather than downgraded workspace-wide
// so the standard stays one line in the root manifest and the debt stays visible
// per crate: deleting this attribute is what finishes the burn-down, and a crate
// added without it starts held to the lint. See `tools/check_workspace_lints.py`,
// which is the list of crates still carrying one.
#![allow(missing_docs)]

pub mod classic;
pub mod geometry;
pub mod hdf5;
pub mod projection;
pub mod reader;

pub use classic::{Attribute, ClassicHeader, ClassicVersion, Dimension, NcType, Variable};
pub use geometry::{
    AxisKind, CurvilinearCoords, DatasetView, DimView, RenderableVariable, SliceGeometry, VarView,
    corner_and_regularity, detect_axis, extract_plane, synthesize_geometry,
};
pub use hdf5::attribute::{Hdf5Attribute, RawAttribute, list_attributes, raw_attribute};
pub use hdf5::dataset::{DatasetShape, describe as describe_dataset};
pub use hdf5::dataspace::Dataspace;
pub use hdf5::datatype::{ByteOrder, Datatype, DatatypeClass};
pub use hdf5::dimensions::{
    DimensionInfo, Hdf5Metadata, VariableInfo, resolve as resolve_hdf5_metadata,
};
pub use hdf5::group::{ChildKind, GroupChild, list_all_children, list_root_children};
pub use hdf5::object_header::{HeaderMessage, ObjectHeader};
pub use hdf5::{Hdf5Probe, root_group_address};
pub use projection::{
    GeostationaryGrid, WRF_EARTH_RADIUS_M, WrfLambertGrid, WrfLatLonGrid, WrfMapProj,
    WrfMercatorGrid, WrfPolarStereoGrid, apply_scale_offset, cf_scale_offset,
    resolve_cf_geostationary, resolve_wrf_lambert, resolve_wrf_latlon, resolve_wrf_mercator,
    resolve_wrf_polar_stereo, unpack_cf_data, wrf_map_proj,
};
pub use reader::{NetcdfBacking, NetcdfReader};
