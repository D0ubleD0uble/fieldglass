//! NetCDF format reader. Covers the classic on-disk layout (CDF-1, CDF-2,
//! CDF-5) and the NetCDF-4 / HDF5 layout end to end: dimensions, variables,
//! and attributes, plus per-variable value decode into `Vec<Option<f64>>`.
//! See the per-module docs for the per-layout detail.
//!
//! **Decoding is two stages, and the reader offers both composed.**
//! [`NetcdfReader::decode_variable_raw`] returns the raw on-disk codes with
//! only the fill / missing sentinels masked; the CF `scale_factor` /
//! `add_offset` / `valid_range` mask-and-scale is applied on top of them, from
//! the variable's own attributes. A caller that wants physical units — which is
//! nearly always, since GOES, MERRA-2 and ERA5 all store packed `int16` — takes
//! [`NetcdfReader::decode_variable_physical`], or
//! [`NetcdfReader::decode_plane`] to pick a 2-D plane out of an N-D variable in
//! the same call. [`NetcdfReader::view`] builds the neutral [`DatasetView`]
//! those need without the caller naming which on-disk layout it opened.
//!
//! The error type every `Result` here returns ([`FieldglassError`]) and the
//! two halves of the byte-access seam ([`ByteRange`], [`ByteSource`]) are
//! re-exported from `fieldglass-core`, so this crate can be the only
//! Fieldglass dependency in a consumer's manifest.

#![forbid(unsafe_code)]
pub mod arrays;
pub mod classic;
pub mod geometry;
pub mod hdf5;
pub mod projection;
pub mod reader;
pub mod resolve;

pub use classic::{Attribute, ClassicHeader, ClassicVersion, Dimension, NcType, Variable};
// The geometry every host and the umbrella consume, so a consumer of this
// crate's `slice_geometry` needs no `fieldglass-core` line of its own (#537).
/// The shared allocation cap this crate's own cap is built from (#707), so a
/// consumer of this crate alone can name the bar every reader is held to
/// without a `fieldglass-core` line of its own.
/// The shape a container states an array it left out in (#709), so a consumer of
/// this crate alone can read `ArraySource::left_out` without a
/// `fieldglass-core` line of its own.
pub use fieldglass_core::LeftOut;
pub use fieldglass_core::MAX_VARIABLE_ELEMENTS;
pub use fieldglass_core::{GridGeometry, Scan};
// The `fieldglass_core` types this crate's own signatures name (#537), so a
// consumer needs no direct dependency on `fieldglass-core` — and cannot
// accidentally take one without `default-features = false`, which would
// re-enable `render` and `fs` across the whole dependency graph.
// `ByteRange` and `ByteSource` are here because `classic::variable_plan` hands
// ranges out and `classic::decode_variable_raw_from` takes a source back:
// the seam is unusable from outside if its two types cannot be named.
pub use fieldglass_core::{ByteRange, ByteSource, FieldglassError};
pub use geometry::{
    AxisKind, CurvilinearCoords, DatasetView, RenderableVariable, SliceGeometry, VarView,
    corner_and_regularity, detect_axis, extract_plane, synthesize_geometry,
};

/// `fieldglass-core`'s array model, which a [`DatasetView`] is made of (#684).
///
/// Re-exported as a module rather than name by name at the crate root, because
/// two of its names are taken there: [`Attribute`] and [`Dimension`] are the
/// classic header's own records. Under `array::` they read as they do in
/// `fieldglass-core`, and a consumer of this crate alone can still name every
/// type a view hands back.
pub mod array {
    pub use fieldglass_core::array::{
        ArrayDescription, ArraySource, Attribute, AttributeValue, Dimension, ElementType, Group,
    };
}
pub use arrays::NetcdfArrays;
pub use hdf5::attribute::{Hdf5Attribute, RawAttribute, list_attributes, raw_attribute};
pub use hdf5::dataset::{DatasetShape, describe as describe_dataset};
pub use hdf5::dataspace::Dataspace;
pub use hdf5::datatype::{ByteOrder, Datatype, DatatypeClass};
pub use hdf5::dimensions::{
    DimensionInfo, Hdf5Metadata, UnsupportedVariable, VariableInfo,
    resolve as resolve_hdf5_metadata,
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

/// Compiles and runs the README's usage snippet as a doc test, so the crate's
/// crates.io front page cannot drift from the API it describes (#539).
///
/// `#[cfg(doctest)]` is what keeps this out of every other build: rustdoc sets
/// it when it collects doc tests and nothing else does, so the type itself is
/// never compiled into the library.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeSnippet;
