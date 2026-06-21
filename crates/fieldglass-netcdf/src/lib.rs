//! NetCDF format reader. Covers the classic on-disk layout (CDF-1, CDF-2,
//! CDF-5) end-to-end at the header level and probes HDF5 / NetCDF-4 files
//! enough to confirm the format. See the per-module docs for what's parsed
//! and what's deferred.

#![forbid(unsafe_code)]

pub mod classic;
pub mod geometry;
pub mod hdf5;
pub mod projection;
pub mod reader;

pub use classic::{Attribute, ClassicHeader, ClassicVersion, Dimension, NcType, Variable};
pub use geometry::{
    AxisKind, DatasetView, DimView, RenderableVariable, SliceGeometry, VarView,
    corner_and_regularity, detect_axis, extract_plane, synthesize_geometry,
};
pub use hdf5::attribute::{Hdf5Attribute, RawAttribute, list_attributes, raw_attribute};
pub use hdf5::dataset::{DatasetShape, describe as describe_dataset};
pub use hdf5::dataspace::Dataspace;
pub use hdf5::datatype::{ByteOrder, Datatype, DatatypeClass};
pub use hdf5::dimensions::{
    DimensionInfo, Hdf5Metadata, VariableInfo, resolve as resolve_hdf5_metadata,
};
pub use hdf5::group::{ChildKind, GroupChild, list_root_children};
pub use hdf5::object_header::{HeaderMessage, ObjectHeader};
pub use hdf5::{Hdf5Probe, root_group_address};
pub use projection::{
    GeostationaryGrid, WrfLambertGrid, apply_scale_offset, cf_scale_offset,
    resolve_cf_geostationary, resolve_wrf_lambert, unpack_cf_data,
};
pub use reader::{NetcdfBacking, NetcdfReader};
