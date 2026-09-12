#![forbid(unsafe_code)]
//! The host-neutral Fieldglass API: bytes in, plain data out.
//!
//! This is the crate a Rust consumer reaches for, and the one every host binds
//! ([ADR-0006](https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md)).
//! It sits between the format crates — which stay independently usable — and a
//! binding, so a host carries only four things: buffer conversion, error
//! mapping, method forwarding, and packaging.
//!
//! ```no_run
//! # fn main() -> Result<(), fieldglass::Error> {
//! let bytes = std::fs::read("forecast.grib2").unwrap();
//! let session = fieldglass::Session::open(bytes)?;
//! let field = session.decode(0, &Default::default())?;
//! let palette = session.palette(&field, &Default::default())?;
//! // `palette.lut` is a 256-entry RGBA table; `palette.t0` / `t1` the domain.
//! # Ok(()) }
//! ```
//!
//! # What is here, and what is not
//!
//! This is the first cut, filed under #460 to give `fieldglass-wasm` something
//! to bind. It carries the GRIB decoders, the four grid families NOAA NODD and
//! ECMWF publish, and the operations a browser map needs: decode, warp,
//! palette, render, probe, contours. #464 moves the rest of the render
//! orchestration out of `fieldglass-napi` and collapses that host onto this
//! crate; NetCDF landed in #662, addressed by variable, and reduced-resolution
//! decode (#463) arrives with its own issue. Caller-sized output landed in #465: `WarpOptions` and the two
//! lat/lon-box targets of `RenderOptions` take a `width`/`height` pair, so a
//! map view asks for a window at a pixel size rather than taking whatever
//! raster the source grid implies. (Named in prose rather than linked: both
//! types are behind the `render` feature, and a link to them does not resolve
//! in a build that declined it.)
//!
//! # Feature flags
//!
//! Every one is on by default, so a consumer that says nothing gets the whole
//! surface; a consumer that says `default-features = false` is asking to pay
//! for less and names what it wants back (#552).
//!
//! - **`grib1`**, **`grib2`** *(default)* — the decoders [`Session::open`]
//!   dispatches to. At least one is required. [`Session::open`] answers
//!   [`Error::UnsupportedFormat`] for a container this build recognises but
//!   cannot decode, naming the feature, and format detection itself stays
//!   unconditional: "this is GRIB1 and I cannot read it" is a different answer
//!   from "I do not know what this is", and a host needs to tell them apart.
//! - **`netcdf`**, **`zarr`** *(default)* — the containers of named arrays:
//!   a NetCDF file through [`Session::open`], a Zarr store from its objects
//!   through `Session::open_store`. Both are addressed by variables and answer
//!   through one arm, over the `ArraySource` each reader presents, with core's
//!   CF rules for the variable list and the slice placement (#704).
//! - **`render`** *(default)* — the projection and paint pipeline:
//!   `Session::warp`, `Session::palette`, `Session::render`, `render::project`,
//!   `render::probe_pixel`, `render::overlay_polylines`, and the `shader`
//!   module. Forwards `fieldglass-core/render`.
//! - **`analysis`** *(default)* — operations that take values and return
//!   values: `Session::combine`, `Session::contours`, `render::field_csv`, and
//!   the `combine` module. Forwards `fieldglass-core/analysis`. Independent of
//!   `render` — a values-first host wants contours without the painter —
//!   except for `render::contour_polylines`, which traces isolines and then
//!   projects them onto the render raster and so needs both.
//! - **`schema`** *(default)* — `schemars::JsonSchema` on every API type. A
//!   host's TypeScript or Python declarations are generated from the schema
//!   rather than kept by hand. Off for `fieldglass-wasm`, whose declarations
//!   come from wasm-bindgen and whose bundle pays for every byte.
//! - **`conformance`** *(default)* — the suite of cases and recorded
//!   expectations every host binding is checked against (ADR-0006 decision 3,
//!   #573). Default because it is this crate's own gate too, and
//!   `cargo test --workspace` does not enable optional features. Both hosts
//!   take this crate with `default-features = false`, so neither the addon nor
//!   the browser bundle carries it.
//! - **`fetchplan`** *(default)* — reading a cloud-native manifest into a
//!   chunk plan, the bytes a host should fetch and which chunk or message each
//!   range is (#461, #685, [ADR-0005] decision 5). It answers in both
//!   addressing shapes: a query over the messages of a GRIB sidecar, and a
//!   chunk index over an array in a kerchunk reference document (#687). Plus
//!   the two halves a pure planner cannot have: a `ParameterResolver` over the
//!   GRIB2 tables, and the semantic half of verifying that the bytes that came
//!   back are the message the sidecar promised. Default for the
//!   reason `conformance` is — `cargo test --workspace` enables no optional
//!   feature, so off-by-default would mean the planner's own gate never runs —
//!   and carried by neither host, both of which take this crate with
//!   `default-features = false`.
//!
//! [ADR-0005]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md
//!
//! A gated item is named in a code span above rather than an intra-doc link: a
//! link to an item this build compiled out is a hard error under the
//! workspace's `rustdoc::all = "deny"`, so a doc page that linked them would
//! build only with every feature on — the one configuration that cannot show
//! the gating works.

// A `Session` with no decoder behind it can only ever refuse, so the mistake is
// worth a message at compile time rather than an `UnsupportedFormat` at every
// call. Stated here and not in the manifest because cargo has no way to say
// "at least one of these".
#[cfg(not(any(
    feature = "grib1",
    feature = "grib2",
    feature = "netcdf",
    feature = "zarr"
)))]
compile_error!(
    "fieldglass needs at least one format feature: enable `grib1`, `grib2`, `netcdf`, \
     or any combination (all three are on by default; a `default-features = false` \
     consumer names back the ones it opens)"
);

pub mod api;
#[cfg(feature = "analysis")]
pub mod combine;
#[cfg(feature = "conformance")]
pub mod conformance;
pub mod error;
#[cfg(feature = "fetchplan")]
pub mod fetchplan;
// The GRIB1 surface a host needs beyond `Session`: the `P1` edit, which a
// read-only session has no shape for (#726). Gated with the decoder it uses.
#[cfg(feature = "grib1")]
pub mod grib1;
// The NetCDF surface a host needs beyond `Session`, so no host names a format
// crate in its manifest (#662). Gated with the decoder it re-exports.
#[cfg(feature = "netcdf")]
pub mod netcdf;
pub mod render;
pub mod session;
#[cfg(feature = "render")]
pub mod shader;

#[cfg(feature = "render")]
pub use api::Warped;
pub use api::{
    Addressing, AxisUnits, DimensionInfo, Dtype, Field, Georef, LeftOutArray, MessageInfo, Probe,
    Scan, SourceFormat, Stats, Values, VariableInfo,
};
#[cfg(feature = "analysis")]
pub use api::{CombineOpInfo, Isoline};
#[cfg(feature = "analysis")]
pub use combine::{CombineOp, aligned, combine_ops, combine_values, op_from_wire};
pub use error::Error;
// The placement a host paints with (#659), beside the `Session` that hands it
// over. Not in `api`: it carries a `GridGeometry`, which is the engine's own
// shape rather than anything a host serialises.
pub use render::Source;
#[cfg(feature = "render")]
pub use render::{PixelProbe, Projected, RenderOptions, ResolvedOptions, TargetKind, WarpTarget};
pub use session::PlacedSlice;
pub use session::{DecodeOptions, Session};
#[cfg(feature = "render")]
pub use session::{PaletteOptions, Raster, WarpOptions};

// The seams a **host** implements, and the error they return. `Session::open`
// takes bytes and `Session::open_store` takes an `impl ObjectSource`, so a host
// that brings its own — the addon reading a directory (#659), a browser filling
// one from a bucket — has to be able to name the trait. Re-exported for the
// reason a format crate re-exports the `core` names in its signatures (#583):
// depending on `fieldglass` alone has to be enough to use it, and
// `fieldglass-wasm` depends on nothing else.
//
// `MemoryObjects` comes with them because it is the implementation a host that
// has already fetched everything wants, and `FieldglassError` because it is what
// every method of both traits returns. Ungated: a host implements the seam
// whether or not it paints.
pub use fieldglass_core::FieldglassError;
pub use fieldglass_core::bytes::{ByteSource, MemoryObjects, ObjectSource};
/// `core`'s colour type, re-exported: a host consumes the painter's own table
/// rather than implementing a second colour path (ADR-0006 decision 3).
#[cfg(feature = "render")]
pub use fieldglass_core::colormap::{Colormap, PALETTE_LUT_LEN, Palette, ScaleMode, colormaps};
#[cfg(feature = "render")]
pub use shader::{GLSL, shader_index, shader_mask, shader_values};
