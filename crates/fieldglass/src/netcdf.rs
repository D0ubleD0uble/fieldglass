//! The NetCDF surface a host needs beyond what [`Session`] answers.
//!
//! **[`Session`] is the way in, and this is not an alternative to it.** A host
//! that wants a variable list, a slice, or a placed raster asks `Session` —
//! [`Session::variables`], [`Session::decode_slice`] — and gets the same answer
//! whatever container it opened, which is the whole point of the umbrella
//! ([ADR-0006]). What is re-exported here is the remainder: the NetCDF-specific
//! metadata a file viewer shows, and the geometry types the extension's own
//! caches are keyed on.
//!
//! It exists so **no host names a format crate in its manifest** (#662). Before
//! this, `fieldglass-napi` depended on `fieldglass-netcdf` *alongside* the
//! umbrella and built its NetCDF path against the reader directly, so the two
//! hosts had diverged: the browser bundle took `fieldglass` and could open a
//! NetCDF file only because the umbrella reached the reader, while the addon
//! reached it twice over by two different routes. One dependency edge means one
//! place where the surface a host may use is decided.
//!
//! # What is still to move
//!
//! This is the first step of a transition, not the end of it. `fieldglass-napi`
//! keeps its own handle types, its decoded-value memo, and its curvilinear
//! lookup cache, because those are keyed on reader internals `Session` does not
//! expose — and the cache is load-bearing rather than incidental: a global ocean
//! mesh costs about two seconds and 400 MB to index, paid once per file instead
//! of once per repaint. Moving them behind `Session` is a design decision about
//! *where a host's memo lives*, which belongs on #662 rather than in a
//! re-export.
//!
//! [`Session`]: crate::Session
//! [`Session::variables`]: crate::Session::variables
//! [`Session::decode_slice`]: crate::Session::decode_slice
//! [ADR-0006]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0006-one-umbrella-crate-and-host-bindings-over-it.md

/// The file's variables as an `ArraySource`, over a reader and view a host
/// already holds — borrowed, so nothing is rebuilt. What lets a host that keeps
/// its own `NetcdfReader` reach an operation written against the array model,
/// such as `crate::line_through` (#172), without a second implementation of it.
pub use fieldglass_netcdf::NetcdfArrays;
/// The file's structure as the reader resolved it, and the variables in it.
///
/// A host shows more of a NetCDF file than `Session` describes — the backing's
/// own attributes, which variables are renderable and why the others are not —
/// and this is that surface.
pub use fieldglass_netcdf::{
    DatasetView, Hdf5Attribute, Hdf5Metadata, NetcdfBacking, NetcdfReader, RenderableVariable,
    extract_plane,
};

/// Which axis a file's own conventions say is latitude or longitude.
///
/// A hint rather than an answer — a WRF or satellite file's horizontal axes are
/// projected and carry no CF axis attribute — which is why a host asks and then
/// lets the caller pick.
pub use fieldglass_netcdf::{AxisKind, detect_axis};

/// The projected grids a file states in its `grid_mapping` attributes.
///
/// A host builds these in its own tests to check that a projected slice is
/// placed where the file says, which is why they are part of the surface rather
/// than an internal of the resolver.
pub use fieldglass_netcdf::{GeostationaryGrid, WrfLambertGrid, WrfPolarStereoGrid};

/// Where one slice sits on the Earth.
///
/// The geometry types a host's own lookup cache is keyed on, until that cache
/// moves behind `Session` (#662).
pub use fieldglass_netcdf::{SliceGeometry, resolve};
