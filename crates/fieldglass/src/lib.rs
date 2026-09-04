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
//! crate; NetCDF, caller-sized output (#465), and reduced-resolution decode
//! (#463) arrive with their own issues.
//!
//! # Feature flags
//!
//! - **`schema`** *(default)* — `schemars::JsonSchema` on every API type. A
//!   host's TypeScript or Python declarations are generated from the schema
//!   rather than kept by hand. Off for `fieldglass-wasm`, whose declarations
//!   come from wasm-bindgen and whose bundle pays for every byte.

pub mod api;
pub mod error;
pub mod session;
pub mod shader;

pub use api::{
    AxisUnits, ContourLevel, Dtype, Field, Format, Georef, MessageInfo, Probe, Scan, Stats, Values,
    Warped,
};
pub use error::Error;
pub use session::{DecodeOptions, PaletteOptions, Raster, Session, WarpOptions};

/// `core`'s colour type, re-exported: a host consumes the painter's own table
/// rather than implementing a second colour path (ADR-0006 decision 3).
pub use fieldglass_core::colormap::{Colormap, PALETTE_LUT_LEN, Palette, ScaleMode, colormaps};
pub use shader::{GLSL, shader_index, shader_mask, shader_values};
