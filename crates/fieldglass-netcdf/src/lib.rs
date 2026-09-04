//! NetCDF format reader. Covers the classic on-disk layout (CDF-1, CDF-2,
//! CDF-5) and the NetCDF-4 / HDF5 layout end to end: dimensions, variables,
//! and attributes, plus per-variable value decode into `Vec<Option<f64>>`.
//! See the per-module docs for the per-layout detail.
//!
//! The error type every `Result` here returns ([`FieldglassError`]) and the
//! two halves of the byte-access seam ([`ByteRange`], [`ByteSource`]) are
//! re-exported from `fieldglass-core`, so this crate can be the only
//! Fieldglass dependency in a consumer's manifest.

#![forbid(unsafe_code)]
pub mod classic;
pub mod geometry;
pub mod hdf5;
pub mod projection;
pub mod reader;

pub use classic::{Attribute, ClassicHeader, ClassicVersion, Dimension, NcType, Variable};
// The `fieldglass_core` types this crate's own signatures name (#537), so a
// consumer needs no direct dependency on `fieldglass-core` — and cannot
// accidentally take one without `default-features = false`, which would
// re-enable `render` and `fs` across the whole dependency graph.
// `ByteRange` and `ByteSource` are here because `classic::variable_plan` hands
// ranges out and `classic::decode_variable_values_from` takes a source back:
// the seam is unusable from outside if its two types cannot be named.
pub use fieldglass_core::{ByteRange, ByteSource, FieldglassError};
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
