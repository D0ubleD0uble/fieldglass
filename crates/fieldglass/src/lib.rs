//! Opens GRIB, NetCDF and Zarr weather data and answers the same questions
//! about each of them in the same way.
//!
//! You hand it a file's bytes and get plain data back: what the file holds, a
//! field's values, where on the Earth they sit, and a colored picture of them.
//! This is the crate a Rust program uses, and the one the Fieldglass VS Code
//! extension and the `@fieldglass/wasm` browser package are both built on, so
//! all three give the same answers
//! ([ADR-0006](https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md)).
//! The format crates underneath it can each be used on their own.
//!
//! These pages are for writing code against the crate. The
//! [crate's README](https://crates.io/crates/fieldglass) covers what it reads
//! and has short runnable examples, one per question.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes = std::fs::read("forecast.grib2")?;
//! let session = fieldglass::Session::open(bytes)?;
//! let field = session.decode(0, &Default::default())?;
//! let palette = session.palette(&field, &Default::default())?;
//! // `palette.lut` is a 256-entry RGBA table; `palette.t0` / `t1` the domain.
//! # Ok(()) }
//! ```
//!
//! # What it covers
//!
//! It reads GRIB1, GRIB2, NetCDF and Zarr, whichever of the four this build
//! includes. It places a field on the Earth for every grid family it knows:
//! lat/lon, Gaussian, Mercator, rotated lat/lon, Lambert conformal, polar
//! stereographic, transverse Mercator, Lambert azimuthal equal-area, the
//! geostationary satellite view, and lookup grids that give a latitude and
//! longitude for every cell. Spherical harmonic and HEALPix fields are put on
//! a global lat/lon grid when they are decoded, so every operation treats them
//! like any other field. Bi-Fourier fields are the one kind it still can't
//! place.
//!
//! A map view can ask for a window at a given pixel size: `WarpOptions` and the
//! two lat/lon-box targets of `RenderOptions` take a `width` and `height`.
//! (Those two types are named in plain text rather than linked, because they
//! sit behind the `render` feature and a link to them would break a build
//! without it.)
//!
//! # Feature flags
//!
//! Every feature but `schema` and `conformance` is on by default, so a program
//! that says nothing gets everything. A program that sets
//! `default-features = false` is asking to carry less, and names what it wants
//! back.
//!
//! - **`grib1`** and **`grib2`** add the GRIB readers that [`Session::open`]
//!   hands a file to. At least one is required. A file this build recognizes
//!   but can't read is refused with [`Error::UnsupportedFormat`], naming the
//!   feature it needs. Recognizing the format never depends on a feature, so
//!   "this is GRIB1 and this build can't read it" stays a different answer from
//!   "I don't know what this is".
//! - **`netcdf`** and **`zarr`** add the readers for named arrays: a NetCDF
//!   file through [`Session::open`], and a Zarr store through
//!   `Session::open_store`. Both are read by variable and share the same CF
//!   rules for listing variables and placing a slice.
//! - **`render`** adds reprojecting and drawing: `Session::warp`,
//!   `Session::palette`, `Session::render`, `render::project`,
//!   `render::probe_pixel`, `render::overlay_polylines`, and the `shader`
//!   module.
//! - **`analysis`** adds the operations that take values and return values:
//!   `Session::combine`, `Session::contours`, `render::field_csv`, and the
//!   `combine` module. It doesn't need `render`, so a program can have contours
//!   without the drawing code. The one exception is
//!   `render::contour_polylines`, which traces contours and then places them on
//!   the drawn picture, so it needs both.
//! - **`fetchplan`** adds planning which byte ranges of a remote file to fetch,
//!   from a GRIB sidecar index or a kerchunk reference document, and checking
//!   that the bytes that came back are the field that was asked for
//!   ([ADR-0005](https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md)).
//!   Neither the extension nor the browser package uses it.
//! - **`schema`** (off) adds a JSON Schema for every data type. Other
//!   languages' type definitions, such as the browser package's TypeScript,
//!   are generated from it.
//! - **`conformance`** (off) adds the shared test suite every Fieldglass host
//!   is checked against. Its cases point at files in the repository, which are
//!   not in the published crate, so it is only useful from a checkout.
//!
//! Items behind a feature are named in plain text above rather than linked,
//! because a link to something a build left out fails the documentation
//! build, and a page that only builds with every feature on can't show that
//! the features work.
//!
//! # What a `Field` is
//!
//! A decoded field is its values in one contiguous list, a separate `u8` mask
//! marking which cells have data, and a description of where it sits on the
//! Earth. It is not a `Vec<Option<f64>>`, which costs a branch for every value
//! that crosses into another language and isn't a typed array there. Missing
//! cells are not `NaN` either, because `isnan()` is unreliable on some mobile
//! GPUs and a `NaN` spoils linear filtering in a texture.
//!
//! The element type follows the source. `Dtype::Auto` narrows to `f32` only
//! when every value survives the round trip. That is stricter than checking
//! whether the packing used 24 bits or fewer: a packed GRIB value is
//! `(R + X·2ᴱ)·10⁻ᴰ`, and once the reference value `R` sits far from zero, or
//! `D` makes the step a negative power of ten, fitting the packed integers in
//! an `f32` says nothing about fitting the values. A program that wants `f32`
//! anyway asks for `Dtype::F32` and accepts the loss.
//!
//! `Georef` says where the field sits in two parts: a coordinate system it can
//! name (`proj4`), and an `affine` placing the raster in it (`x0`, `y0`, `dx`,
//! `dy`). These are degrees for the geographic grids and meters on the
//! projection plane for the projected ones, and `units` says which. A rotated
//! lat/lon grid reports degrees too, in its own rotated frame. When a grid
//! can't state something, the value is `None` rather than a guess. A Gaussian
//! grid's rows are not evenly spaced, so it has no `dy`, and inventing an
//! average one would misplace every row but the middle. A field with no plane
//! at all, such as spectral coefficients, has no `affine` and no units.
//!
//! # Color is decided once
//!
//! `Palette` is the painter's own 256-entry color table plus the range it
//! covers, handed out as data. A program drawing on the GPU uploads it as a
//! 256 × 1 texture and pastes `fieldglass::GLSL` into its fragment shader, and
//! `shader_values` prepares the values the shader reads. The CPU painter uses
//! the same `Palette`, so it is the reference a GPU path is tested against
//! rather than a second implementation of color.
//!
//! The values are shifted by the start of the range (`t0`) here in Rust, in
//! `f64`, before they reach the shader, and that matters. A shader doing the
//! same subtraction in `f32` loses the range itself: once the gap between
//! `f32` values near `t0` is larger than one step of the table, colors can be
//! wrong by any amount. It was measured at up to 127 entries, half the table,
//! for a range of 1.0 over values near ten million, which geopotential in
//! m²/s² and pressure in Pa both reach under a tight manual range.
//!
//! # Wire format
//!
//! Structs are `camelCase` when they cross into another language and
//! `snake_case` in Rust. Enum variants stay `snake_case` in both, because a
//! program compares them as strings and `"polar_stereo"` is what `core`
//! already reports. Every API type is `#[non_exhaustive]`, so build an options
//! struct with its `new` (every options type has one) and set the rest.
//!
//! `render(field, options, flip_y)` combines `flip_y` with the field's own row
//! order rather than replacing it, so `false` means north up, not "rows in the
//! order the file stored them". `Georef::scan` still carries the order for a
//! program drawing something other than a picture.
//!
//! # Combining two fields
//!
//! `Session::combine(a, b, op)` makes a difference map and its siblings, and
//! is the one operation here that takes two fields. The two must line up cell
//! for cell: the same grid (a lookup grid is compared by
//! `SpatialIndex::fingerprint`, since a `NaN` center makes one unequal to
//! itself), the same raster shape and the same row order. If any of the three
//! differ, it returns `Error::Unsupported` naming which. The result is a
//! `Field` on the first field's placement, so every other operation takes it
//! like any other field, and it keeps the first field's `parameter` and
//! `units`; a caption such as `A − B` is the program's to write.
//! `combine_ops()` lists the operations for a picker, and `op_from_wire` reads
//! one back from its name.
//!
//! # The conformance suite
//!
//! `conformance/suite.json` records what every operation answers for a set of
//! committed GRIB files, and `fieldglass::conformance` is what a test runner
//! needs to replay it (ADR-0006 decision 3). Three runners replay it: this
//! crate's own tests, natively and on `wasm32-wasip1`; `fieldglass-napi`'s
//! `conformance_host`, through the Node addon; and
//! `crates/fieldglass-wasm/tests/node/conformance.mjs`, through the built
//! browser package. Adding a new host comes down to handing over the bytes,
//! mapping the errors, and passing the suite.
//!
//! Counts, lengths, picture sizes, the mask and every error code must match
//! exactly. Positions on the Earth must match within the tolerance
//! [ADR-0009](https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0009-cross-target-floating-point-agreement.md)
//! measured. To re-record it, run
//! `FIELDGLASS_UPDATE_CONFORMANCE=1 cargo test -p fieldglass --features conformance --test conformance`;
//! that run then fails on purpose, so a re-record is never mistaken for a pass.

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
     `zarr`, or any combination (all four are on by default; a `default-features = false` \
     consumer names back the ones it opens)"
);

#[cfg(any(feature = "render", feature = "analysis"))]
mod align;
pub mod api;
// The one color-range rule both hosts and the contour levels follow (#1155).
#[cfg(any(feature = "render", feature = "analysis"))]
mod color_range;
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
// The unit tests that read a format crate's fixtures, and how they stand aside
// in the published crate, which cannot carry them (#926).
#[cfg(all(test, feature = "grib2", feature = "render", feature = "analysis"))]
mod test_fixtures;

/// Whether two fields line up cell for cell: the gate `combine` and
/// `render::vector_polylines` both ask before pairing two value arrays.
#[cfg(any(feature = "render", feature = "analysis"))]
pub use align::aligned;
#[cfg(any(feature = "render", feature = "analysis"))]
pub use align::component_frame;
#[cfg(any(feature = "render", feature = "analysis"))]
pub use align::same_grid;
#[cfg(feature = "render")]
pub use api::Warped;
pub use api::{
    Addressing, Affine, AxisUnits, AxisValues, ColorRange, DimensionInfo, DisplayField, Dtype,
    Field, FullDetail, Georef, GridSupport, Identification, LeftOutArray, LeftOutCause, Line,
    MessageFingerprint, MessageInfo, MessageProbe, OutOfRange, Placement, Positive, Probe,
    RangeEnd, RangeRefusal, RangeRefusalReason, RangeSource, Scan, SourceFormat,
    SpectralTruncation, StatisticalStep, Stats, SurfaceCodes, Values, VariableInfo, VectorPairing,
    Vertical,
};
#[cfg(feature = "analysis")]
pub use api::{CombineOpInfo, Isoline};
/// The color range every host paints over, chosen by one rule (#1155), and
/// the contour level count both hosts space their automatic levels with, and
/// the full-field range a display field is colored over (#1082).
#[cfg(any(feature = "render", feature = "analysis"))]
pub use color_range::{AUTO_CONTOUR_LEVELS, color_range, full_field_extremes};
#[cfg(feature = "analysis")]
pub use combine::{
    CombineOp, combine_cell, combine_ops, combine_truncation, combine_values, op_from_wire,
};
pub use error::Error;
/// Pixel-space runs, which `render::overlay_polylines` and
/// `render::contour_polylines` return — named so a host can take the return
/// value apart. Named in prose: the first is behind `render`, the second behind
/// `render` and `analysis`.
#[cfg(feature = "render")]
pub use fieldglass_core::ProjectedPolylines;
/// The cell-center index a lookup [`GridGeometry`] wraps.
///
/// Re-exported so depending on `fieldglass` alone is enough to hold one (#574):
/// a host keeps a placement between calls, and the addon caches a curvilinear
/// slice's lookup geometry per coordinate pair and asks the index which way its
/// rows run. Ungated, because [`Georef`] is.
pub use fieldglass_core::SpatialIndex;
/// The finite range of a field's present cells: the `extremes` a host painting a
/// `render::project` result on the CPU hands [`color_range()`], taken over the
/// field it decoded and not the warped raster (#1155), as
/// `render::contour_polylines` takes them.
#[cfg(feature = "render")]
pub use fieldglass_core::colormap::min_max_ignoring_mask;
/// The grid a [`Source`] and a [`Georef`] carry, so a host can hold one (#574).
pub use fieldglass_core::projection::GridGeometry;
/// A unit string typeset for display (ADR-0007). [`Session`] reports a file's
/// units as the file spells them; a host that shows them typesets them with
/// this, so two hosts cannot typeset one unit two ways.
pub use fieldglass_core::units::normalize_units;
/// Which container a buffer holds, from its leading bytes — what a host asks
/// before choosing which handle to open, and what [`Session::open`] asks too.
pub use fieldglass_core::{Format, detect_from_bytes};
#[cfg(feature = "analysis")]
pub use session::ContourOptions;
// The placement a host paints with (#659), beside the `Session` that hands it
// over. Not in `api`: it carries a `GridGeometry`, which is the engine's own
// shape rather than anything a host serializes.
pub use render::Source;
#[cfg(feature = "render")]
pub use render::{
    PixelProbe, Projected, RenderOptions, ResolvedOptions, TargetKind, VectorOptions, WarpMemo,
    WarpTarget,
};
pub use session::PlacedSlice;
/// The held-index check `Session::decode_slice` runs, for a host that holds its
/// own array source (#1301).
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub use session::check_slice_indices;
/// One line through a combined map, for a host that holds an `ArraySource`
/// rather than a `Session` (#1208).
#[cfg(all(feature = "analysis", any(feature = "netcdf", feature = "zarr")))]
pub use session::line_through_combined;
/// A lat/lon box over a variable, planned into the chunks it touches and
/// decoded from them alone (#1083).
#[cfg(feature = "zarr")]
pub use session::{
    ChunkRef, IndexLocation, InnerChunkRef, ShardIndexRef, WindowPlan, WindowRequest,
};
pub use session::{
    DecodeOptions, DisplayDecodeOptions, LineRequest, Session, SliceRequest, pack_values,
};
#[cfg(feature = "render")]
pub use session::{PaletteOptions, Raster, WarpOptions};
/// One line through a named array, and one axis of it with its coordinates, for
/// a host that holds an `ArraySource` rather than a `Session` (#172, #171).
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub use session::{
    axis_values, dimensions_of, left_out_of, line_through, undrawable_of, variables_of,
    vector_pairing_of,
};

// The seams a **host** implements, and the error they return. `Session::open`
// takes bytes and `Session::open_store` takes an `impl ObjectSource`, so a host
// that brings its own — the addon reading a directory (#659), a browser filling
// one from a bucket — has to be able to name the trait. Re-exported for the
// reason a format crate re-exports the `core` names in its signatures (#583):
// depending on `fieldglass` alone has to be enough to use it, and
// `fieldglass-wasm` depends on nothing else.
//
// `MemoryObjects` (a whole store) and `PrefetchedObjects` (what a host fetched
// so far, which knows what it was not given) are the maps a host fills, and
// `FieldglassError` is what every method of both traits returns. Ungated: a
// host implements the seam whether or not it paints.
pub use fieldglass_core::FieldglassError;
pub use fieldglass_core::bytes::ByteRange;
pub use fieldglass_core::bytes::{ByteSource, MemoryObjects, ObjectSource, PrefetchedObjects};
/// `core`'s color palette table reader (#236). A host parses an imported
/// `.cpt` once, keeps the lookup table [`ColorTable::lut`] compiles, and sends
/// it as [`RenderOptions::colormap_table`] or [`PaletteOptions::colormap_table`].
#[cfg(feature = "render")]
pub use fieldglass_core::color_table::{ColorTable, CptError, parse_cpt};
/// `core`'s color type, re-exported: a host consumes the painter's own table
/// rather than implementing a second color path (ADR-0006 decision 3).
#[cfg(feature = "render")]
pub use fieldglass_core::colormap::{Colormap, PALETTE_LUT_LEN, Palette, ScaleMode, colormaps};
#[cfg(feature = "render")]
pub use shader::{GLSL, shader_index, shader_mask, shader_values};
