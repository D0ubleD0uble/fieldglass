#![forbid(unsafe_code)]
//! Shared core pieces for specific Fieldglass readers.
//!
//! The GRIB, NetCDF, and Zarr readers are each specific implementation crates
//! utilizing the same shared pieces which are defined within this crate. You
//! likely want to import this crate if you are writing your own reader or want
//! to name these shared types. To read weather data, you should use
//! [`fieldglass`](https://crates.io/crates/fieldglass) or a specific format
//! crate instead.
//!
//! # Core modules
//!
//! Every module below is always built, even when features are turned off.
//! A reader needs to always be able to depend on this crate even with
//! `default-features` being off.
//!
//! <!-- parsing-surface: the set of core modules the format crate libraries
//!      name, checked by tools/check_parsing_surface.py. The README states it
//!      again; both regions have to match the code. The entries after it
//!      sit outside it because no format crate library names them. -->
//! ## Reading bytes
//!
//! The readers get their bytes and check them with these.
//!
//! - [`error`], error types and helpers. Currently only has
//!   [`FieldglassError`], plus [`printable_bytes`], which turns raw bytes into
//!   readable text for an error message.
//! - [`bytes`], traits a host implements to hand over a file or a store, and
//!   the types and helpers readers use to read and search through them.
//! - [`bits`], a bit reader that reads values of any bit width from packed
//!   bytes, and functions that help decode specific byte formats.
//! - [`detect`](mod@detect), the [`Format`] type and functions that tell file
//!   types apart from path and bytes.
//! - [`byte_shuffle`], the byte shuffle HDF5 and Zarr apply before
//!   compressing, and its reverse.
//! - [`output_limits`], safety check functions that cap how much memory a
//!   potentially damaged file can produce.
//!
//! ## Shared GRIB modules
//!
//! The GRIB1 and GRIB2 readers share these.
//!
//! - [`bitmap`], functions that read the GRIB bitmap mask which defines which
//!   grid points have a value.
//! - [`unpacking`], a type holding a field's reference value and scale
//!   factors, and functions that use them to unpack stored values into real
//!   values.
//! - [`packing_groups`], functions that expand the groups of values in
//!   second-order and complex packing, where each group is a reference value
//!   plus one small offset per point.
//! - [`spatial_differencing`], functions that rebuild values stored as
//!   differences between neighboring points.
//! - [`storage_order`], functions that put a grid's values into one standard
//!   order when the file stored every other row reversed, or columns first.
//! - [`center_tables`], the WMO table of sub-centers, the groups within a
//!   weather center that author a file.
//! - [`lead_time`], two small functions both editions share for showing a
//!   forecast's lead time.
//! - [`matrix`], functions that reshape fields holding a small matrix of
//!   values at every grid point.
//!
//! ## Describing a dataset
//!
//! The NetCDF and Zarr readers describe what a file holds with these.
//!
//! - [`array`](mod@array), the types that describe a file's groups,
//!   dimensions, attributes and arrays and how an array is split into chunks,
//!   and the [`ArraySource`](array::ArraySource) trait the NetCDF and Zarr
//!   readers implement to read a region of an array.
//! - [`cf`], functions that follow the CF conventions over those descriptions:
//!   which arrays can be drawn, which axes are horizontal, vertical or time,
//!   where a slice sits on the Earth, and how stored values become physical
//!   units. It also reads the map settings WRF writes in its own attributes.
//!
//! ## Placing a grid on the Earth
//!
//! - [`projection`], the map projections GRIB and NetCDF grids use, each with
//!   a type for its parameters and a projector that converts between latitude
//!   and longitude and a position on the grid. It also has
//!   [`GridGeometry`], one type for where any grid
//!   sits on the Earth, and the [`PlanarGridProjector`] trait the projected
//!   grids share.
//! - [`sht`], functions that turn a spectral field's wave coefficients into
//!   values on a grid, the inverse spherical-harmonic transform.
//! - [`healpix`], functions that convert between a HEALPix pixel number and
//!   its position, and resample a HEALPix field onto a latitude and longitude
//!   grid.
//! - [`global_grid`], a type and constants for the fixed half-degree world
//!   grid that spectral and HEALPix fields are put onto.
//!   <!-- /parsing-surface -->
//! - [`spatial_index`], a type that finds the nearest cell in a grid stored
//!   as a list of cell centers. [`cf`] uses it to place those grids.
//!
//! ## For showing a field
//!
//! The `fieldglass` crate uses these when it hands a field to a host.
//!
//! - [`units`], functions that rewrite unit strings into one consistent form
//!   for display, so `kg m-2` and `kg/m2` both read `kg m⁻²`.
//! - [`vector_pair`], a type and functions that read two fields' parameter
//!   names to tell whether they are the two halves of one wind or current.
//!
//! # The traits
//!
//! The traits are the places where you plug in your own behavior.
//! [`ByteSource`] and [`ObjectSource`] are where the bytes come from, a buffer
//! in memory, a file or something fetched over the network.
//! [`PlanarGridProjector`] is how a projected grid turns a latitude and
//! longitude back into a row and column. With `render` on,
//! `TargetProjection` and `ForwardMap` are how a field is reprojected onto a
//! picture.
//!
//! A reader takes any [`ByteSource`], names the ranges it needs, and then
//! reads them. A file already in memory is a `ByteSource` as it is:
//!
//! ```
//! use fieldglass_core::{ByteRange, ByteSource, FieldglassError};
//!
//! // Reads the edition number from a GRIB message's first eight bytes.
//! fn grib_edition<S: ByteSource + ?Sized>(file: &S) -> Result<u8, FieldglassError> {
//!     let header = ByteRange::new(0, 8);
//!     // Naming the ranges first lets a remote source fetch them in one batch.
//!     file.prefetch(&[header])?;
//!     let bytes = file.read(header)?;
//!     Ok(bytes[7])
//! }
//!
//! let file: Vec<u8> = b"GRIB\0\0\0\x02".to_vec();
//! assert_eq!(grib_edition(&file)?, 2);
//! # Ok::<(), FieldglassError>(())
//! ```
//!
//! # Feature flags
//!
//! The four below are on by default. Turn off the ones you don't need with
//! `default-features = false`, and your program won't include their code. A
//! fifth, `testing`, is off and only tests turn it on.
//!
//! - `render` adds reprojecting, coloring and placing lines on a picture
//!   (`warp`, `overlay`, `colormap` and `color_table`). Map projections
//!   themselves are always available.
//! - `analysis` adds contours, CSV export and combining two fields (`contour`,
//!   `csv` and `combine`). It doesn't need `render`.
//! - `serde` adds saving and loading the geometry, color and placement types
//!   with serde. The `fieldglass` crate always turns it on, because the data it
//!   hands a host contains these types.
//! - `fs` adds `detect_format`, which recognizes a file's format from a path
//!   on disk. Turn it off for a target with no file system, such as a browser.
//!   There `std::fs` compiles but every call fails, and without the gate format
//!   detection would quietly fall back to guessing from the file extension.

// The shape model of a chunked array: which chunk holds a value, and what the
// object holding it is called (ADR-0010 decision 1). Ungated and dependency
// free — pure integer arithmetic — so a consumer that takes one format crate
// links nothing new because of it.
pub mod array;
pub mod bitmap;
pub mod bits;
pub mod bytes;
// Buffer growth that stops at a decompressor's limit instead of reserving up
// to twice it (#981). Dependency free; the codecs stay in the format crates.
pub mod byte_shuffle;
pub mod center_tables;
pub mod cf;
/// Generated X11 color names for the CPT parser (`tools/gen_color_names.py`).
#[cfg(feature = "render")]
mod color_names;
#[cfg(feature = "render")]
pub mod color_table;
#[cfg(feature = "render")]
pub mod colormap;
/// Generated colormap anchor tables (`tools/gen_colormaps.py`).
#[cfg(feature = "render")]
mod colormap_tables;
#[cfg(feature = "analysis")]
pub mod combine;
#[cfg(feature = "analysis")]
pub mod contour;
#[cfg(feature = "analysis")]
pub mod csv;
pub mod detect;
pub mod error;
pub mod global_grid;
pub mod healpix;
pub mod lead_time;
pub mod matrix;
pub mod output_limits;
#[cfg(feature = "render")]
pub mod overlay;
pub mod packing_groups;
pub mod projection;
pub mod sht;
pub mod spatial_differencing;
pub mod spatial_index;
pub mod storage_order;
pub mod unpacking;
// Deliberately **not** part of the parsing surface, and gated so it is linked
// by nothing a consumer builds: sources that behave the way a transport does,
// for the tests of every reader on the `bytes` seams (#708). `test` as well as
// the feature, so this crate's own unit tests reach it without a second copy.
#[cfg(any(feature = "testing", test))]
pub mod testing;
pub mod units;
// Whether two fields are the two components of one vector, read from their
// parameter names (#1049). Ungated and dependency free; the umbrella asks it
// for every pair a host draws arrows from.
pub mod vector_pair;
#[cfg(feature = "render")]
pub mod warp;

pub use array::{
    ArrayDescription, ArrayError, Attribute, AttributeValue, CfUnpacking, ChunkGrid,
    ChunkKeyEncoding, Dimension, ElementType, Group, LeftOut, LeftOutCause, MAX_FIELD_POINTS,
    MAX_VARIABLE_BYTES, MAX_VARIABLE_ELEMENTS, whole_variable_read_bytes,
};
pub use bytes::{ByteRange, ByteSource, MemoryObjects, ObjectSource, SourceIdentity};
#[cfg(feature = "analysis")]
pub use combine::{CombineOp, combine_cell, combine_fields};
#[cfg(feature = "analysis")]
pub use contour::{
    ContourLevel, GridSegment, contour_segments, contour_segments_global, nice_levels,
};
pub use detect::Format;
#[cfg(feature = "fs")]
pub use detect::detect_format;
pub use detect::detect_from_bytes;
pub use detect::{DETECT_WINDOW, HDF5_SIGNATURE, HDF5_SIGNATURE_OFFSETS, find_hdf5_signature};
pub use error::{FieldglassError, printable_bytes};
pub use global_grid::{GlobalGrid, SynthesizedField};
#[cfg(feature = "render")]
pub use overlay::{ProjectedPolylines, SourceOverlayTarget, project_polylines};
// The projector types, plus the free functions a format crate or a host calls.
// The per-call `<family>_forward` / `_inverse` wrappers that used to sit here
// were a second public path to arithmetic the projectors already expose, so
// they are gone; build the projector once and call its methods.
pub use projection::{
    CornerPair, DEFAULT_EARTH_RADIUS_M, ForwardAt, GaussianParams, GaussianProjector,
    GeostationaryParams, GeostationaryProjector, GridFingerprint, GridGeometry, GridIndex,
    GridResampling, LambertAzimuthalParams, LambertAzimuthalProjector, LambertParams,
    LambertProjector, LatLonParams, LonLatBox, MercatorParams, PlanarGridProjector, PlaneAffine,
    PlaneUnits, PolarStereoParams, PolarStereoProjector, RotatedLatLonParams,
    RotatedLatLonProjector, Scan, TransverseMercatorParams, TransverseMercatorProjector,
    eastward_lon_span, expand_reduced_to_regular, gaussian_latitudes, is_oblate_spheroid,
    is_octahedral_pl, latlon_inverse, latlon_point, lon_grid_is_global, mercator_inverse,
    mercator_point, normalise_lon, plane_spans_a_grid_cell, reduced_raster_lon_last,
    reduced_raster_width, rotated_latlon_point, signed_grid_increments,
};
pub use spatial_index::SpatialIndex;
pub use storage_order::{StoredRuns, reverse_alternate_runs, transpose_j_consecutive};
#[cfg(feature = "render")]
pub use warp::{
    EqualEarth, ForwardMap, Mollweide, Orthographic, PIXEL_CENTER_OFFSET, PolarStereographic,
    PreparedTarget, Resampling, Robinson, SourceGrid, TargetProjection, TargetRaster, WarpPlan,
    WarpedRaster, WebMercator, warp, warp_to_equirectangular,
};
