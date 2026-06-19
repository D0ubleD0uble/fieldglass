//! NetCDF format reader. Covers the classic on-disk layout (CDF-1, CDF-2,
//! CDF-5) end-to-end at the header level and probes HDF5 / NetCDF-4 files
//! enough to confirm the format. See the per-module docs for what's parsed
//! and what's deferred.

#![forbid(unsafe_code)]

pub mod classic;
pub mod hdf5;
pub mod reader;

pub use classic::{Attribute, ClassicHeader, ClassicVersion, Dimension, NcType, Variable};
pub use hdf5::object_header::{HeaderMessage, ObjectHeader};
pub use hdf5::{Hdf5Probe, root_group_address};
pub use reader::{NetcdfBacking, NetcdfReader};
