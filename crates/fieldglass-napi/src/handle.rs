//! The one handle this addon exports, and the three ways to get one (#1073).
//!
//! ADR-0006 decision 1 makes every host a binding over `fieldglass::Session`,
//! and the browser module binds it as one `Handle`. This addon used to ship
//! four classes, one per container, with four sets of names for the same
//! questions. Now it ships one, and a method that both hosts have is spelt the
//! same and takes the same arguments in both: `count`, `message`, `variables`,
//! `leftOut`, `undrawable`, `decode`, `decodeSlice`, `decodeLine`,
//! `decodeDisplay`, `format`, `addressing` and `close`.
//!
//! The rest of this handle is the extension's map pipeline, which the browser
//! module does not have: a field painted, its overlays projected and its pixels
//! probed in one raster's pixel space, from a value memo the handle keeps
//! (ADR-0011). Those methods take a `GridTarget` — a message index, a slice
//! of a variable, or two of either combined — so a GRIB file, a NetCDF file and
//! a Zarr store are drawn by the same calls. Their names are this host's own,
//! and none of them reuses a name the browser module gives a different
//! operation: the browser's `probe` reads a point, so this one's is
//! `probePixel`.
//!
//! The per-container work stays where it was, in `Grib1File`, `Grib2File`,
//! `NetcdfFile` and `ZarrStore`; this file only reads the arguments, picks
//! the container and maps the error. An API refusal crosses as a JS `Error`
//! with `code` set, as on the browser module.
//!
//! # Reserved for the async layer
//!
//! Everything here is synchronous, and stays so: the bytes-in path is the
//! primary one (ADR-0005). The asynchronous layer (#1085) is to be added beside
//! it under names reserved now, so it lands without renaming anything:
//! `load(source, { signal })` beside `open(bytes)`, and `<name>Async` beside
//! each long call, taking the same arguments and a trailing `{ signal }`.
//! `tools/check_js_api.py` fails while either host declares one of them early.

use super::*;
use fieldglass::{Addressing, DecodeOptions, Error, LineRequest, SliceRequest, SourceFormat};
use napi::JsValue;
use napi::bindgen_prelude::{Float64Array, Object, Uint8Array, Uint32Array, Unknown};

/// Open a GRIB1, GRIB2 or NetCDF file from its bytes. The format is detected
/// from the bytes; `detectBytes` answers the same question without opening.
///
/// The bytes are copied; the caller may reuse its buffer at once.
#[napi]
pub fn open(env: Env, bytes: Uint8Array) -> napi::Result<Handle> {
    Handle::from_vec(bytes.to_vec()).map_err(|e| e.throw(&env))
}

/// Open the Zarr store rooted at the directory `path` (#659).
///
/// The browser module's `openStore` takes the store's objects, which a page
/// fetched; this one reads them from the directory. A directory holding none
/// of `zarr.json`, `.zmetadata`, `.zgroup` or `.zarray` is refused as not a
/// store rather than opened empty.
#[napi]
pub fn open_store(path: String) -> napi::Result<Handle> {
    Ok(Handle {
        inner: Some(Inner::Zarr(ZarrStore::from_directory(path)?)),
    })
}

/// Whether a directory looks like a Zarr store, without opening it — what a
/// folder picker asks so it can refuse in its own words. The `.zarr` suffix is
/// a convention and is **not** what this checks.
#[napi]
pub fn is_store(path: String) -> bool {
    ZarrStore::is_store(path)
}

/// Make `close` the `[Symbol.dispose]` of each class here, so `using` releases
/// a handle at the end of its block (#1073).
///
/// napi-rs names methods by string only, so the symbol is set on the
/// prototypes once the classes are registered. A runtime without
/// `Symbol.dispose` gets nothing set, and `close()` works there as everywhere.
#[napi(module_exports)]
pub fn wire_dispose(exports: Object, env: Env) -> napi::Result<()> {
    // `Symbol` and a class are functions, which a typed `Object` read refuses;
    // `ToObject` on a function is the function itself.
    let symbol: Unknown = env.get_global()?.get_named_property("Symbol")?;
    let dispose: Unknown = symbol.coerce_to_object()?.get_named_property("dispose")?;
    if dispose.get_type()? != napi::ValueType::Symbol {
        return Ok(());
    }
    for class in ["Handle", "DisplayGrid"] {
        let constructor: Unknown = exports.get_named_property(class)?;
        let mut prototype: Object = constructor
            .coerce_to_object()?
            .get_named_property("prototype")?;
        let close: Unknown = prototype.get_named_property("close")?;
        prototype.set_property(dispose, close)?;
    }
    Ok(())
}

/// An open container: a GRIB file, a NetCDF file or a Zarr store.
///
/// Ask `addressing` first, as on the browser module: a GRIB
/// file is a stream of messages, addressed by index, and a NetCDF file or a
/// store holds variables, addressed by a slice. A message question asked of a
/// variable container, or the reverse, is refused with `code:
/// "wrong_addressing"` naming the call to make instead.
///
/// Call `close()`, or open it with `using`, to release the file and its memos
/// before the garbage collector would.
#[napi]
#[derive(Debug)]
pub struct Handle {
    /// `None` once closed.
    inner: Option<Inner>,
}

/// The container a handle holds.
#[derive(Debug)]
enum Inner {
    Grib1(Grib1File),
    Grib2(Grib2File),
    // Boxed: a NetCDF file carries its dataset view and two memos, several
    // times the size of the others.
    Netcdf(Box<NetcdfFile>),
    Zarr(ZarrStore),
}

/// A field a pipeline call draws: a message index, or one slice of a variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GridField {
    /// A message of a GRIB file.
    Message(u32),
    /// A slice of a NetCDF or Zarr variable.
    Slice(SliceRequest),
}

/// What a pipeline call draws: one field, or two combined cell by cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GridTarget {
    /// One field.
    One(GridField),
    /// `a` combined with `b` under `op`, a `value` tag from `combineOps()`.
    Combined {
        a: GridField,
        b: GridField,
        op: String,
    },
}

/// The arrays an overlay projects: flat `[lat, lon, …]` and how many points
/// each ring has.
#[napi(object)]
pub struct Polylines {
    /// Flat `[lat0, lon0, lat1, lon1, …]`, in degrees.
    pub latlon: Float64Array,
    /// How many points each ring holds, so `sum(ringLengths) * 2 ===
    /// latlon.length`.
    pub ring_lengths: Uint32Array,
}

/// Lengths rather than the coordinates, as the other typed-array DTOs print.
impl std::fmt::Debug for Polylines {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Polylines")
            .field("latlon.len", &self.latlon.len())
            .field("ring_lengths.len", &self.ring_lengths.len())
            .finish()
    }
}

/// How contours are spaced (#238).
#[napi(object)]
#[derive(Debug)]
pub struct ContourSpacing {
    /// A manual spacing between levels; left out, about eight round levels over
    /// the range painted.
    pub interval: Option<f64>,
}

/// Two messages drawn as arrows (#241), and how.
#[napi(object)]
#[derive(Debug)]
pub struct VectorPair {
    /// The message holding the eastward (or grid-`x`) component.
    pub u: u32,
    /// The message holding the northward (or grid-`y`) component.
    pub v: u32,
    /// Grid cells between arrows; left out, a spacing for about forty arrows
    /// across the raster.
    pub spacing: Option<u32>,
    /// Overrides the component frame the pair states: `true` along the grid's
    /// own axes, `false` east and north. Left out, the pair's own is used.
    pub grid_relative: Option<bool>,
}

/// The two image axes of a variable, without a position on the rest: all that
/// placing a slice needs.
#[napi(object)]
#[derive(Debug)]
pub struct SliceAxes {
    /// Which variable, as `variables()` numbers it.
    pub variable: u32,
    /// Which of its axes runs down the rows.
    pub y_dim: u32,
    /// Which of its axes runs across the columns.
    pub x_dim: u32,
}

impl Handle {
    /// A handle over `bytes`, for callers that are not JavaScript: the
    /// detection and the per-format open, with the engine's refusal kept.
    pub(crate) fn from_vec(bytes: Vec<u8>) -> Result<Self, RenderError> {
        let inner = match fieldglass::detect_from_bytes(&bytes) {
            fieldglass::Format::Grib1 => Inner::Grib1(Grib1File::from_vec(bytes)?),
            fieldglass::Format::Grib2 => Inner::Grib2(Grib2File::from_vec(bytes)?),
            fieldglass::Format::NetCdf => Inner::Netcdf(Box::new(NetcdfFile::from_vec(bytes)?)),
            // Asked of the session so the refusal is the one every host gives
            // for the same bytes, with its `unsupported_format` code.
            _ => {
                return Err(match fieldglass::Session::open(bytes) {
                    Err(e) => e.into(),
                    Ok(_) => Error::UnsupportedFormat {
                        detail: "a container this host does not open".to_string(),
                    }
                    .into(),
                });
            }
        };
        Ok(Self { inner: Some(inner) })
    }

    /// The container, or the refusal a closed handle gives.
    fn inner(&self) -> Result<&Inner, RenderError> {
        self.inner.as_ref().ok_or_else(|| {
            napi::Error::from_reason(
                "this handle was closed; open the file again to read it".to_string(),
            )
            .into()
        })
    }

    /// The message stream behind a GRIB handle, or the refusal `called` gets on
    /// a variable container.
    fn messages(&self, called: &str) -> Result<&MessageStream, RenderError> {
        match self.inner()? {
            Inner::Grib1(h) => Ok(&h.stream),
            Inner::Grib2(h) => Ok(&h.stream),
            Inner::Netcdf(_) | Inner::Zarr(_) => Err(wrong_addressing(
                Addressing::Variables,
                &format!(
                    "`{called}` addresses a message; call `decodeSlice` or `variables` instead"
                ),
            )),
        }
    }
}

/// `Error::WrongAddressing` for a handle in `mode`.
fn wrong_addressing(mode: Addressing, detail: &str) -> RenderError {
    let expected = match mode {
        Addressing::Messages => "messages",
        Addressing::Variables => "variables",
        // `Addressing` is `#[non_exhaustive]`.
        _ => "another addressing",
    };
    Error::WrongAddressing {
        expected: expected.to_string(),
        detail: detail.to_string(),
    }
    .into()
}

/// The refusal a target of the wrong kind gets: a slice asked of a message
/// stream, or a message index asked of a variable container.
fn wrong_target(inner: &Inner) -> RenderError {
    match inner {
        Inner::Grib1(_) | Inner::Grib2(_) => wrong_addressing(
            Addressing::Messages,
            "a slice addresses a variable, and this handle holds messages; pass a message index",
        ),
        Inner::Netcdf(_) | Inner::Zarr(_) => wrong_addressing(
            Addressing::Variables,
            "a message index addresses a message, and this handle holds variables; pass a slice \
             { variable, yDim, xDim, sliceIndices }",
        ),
    }
}

/// An argument that did not read is the caller's to fix: `invalid_option`.
fn unreadable(what: &str, e: &dyn std::fmt::Display) -> RenderError {
    Error::invalid_option(format!("could not read the {what}: {e}")).into()
}

/// Read decode options the way the browser module does: `null` or left out is
/// the default.
fn decode_options(value: Option<serde_json::Value>) -> Result<DecodeOptions, RenderError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(DecodeOptions::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| unreadable("options object", &e)),
    }
}

/// One field of a target: a message index, or a slice object read strictly.
fn grid_field(value: serde_json::Value) -> Result<GridField, RenderError> {
    match value {
        serde_json::Value::Number(n) => n
            .as_u64()
            .and_then(|i| u32::try_from(i).ok())
            .map(GridField::Message)
            .ok_or_else(|| unreadable("field", &format!("{n} is not a message index"))),
        object @ serde_json::Value::Object(_) => serde_json::from_value(object)
            .map(GridField::Slice)
            .map_err(|e| unreadable("slice", &e)),
        other => Err(unreadable(
            "field",
            &format!(
                "{other} is neither a message index nor a slice {{ variable, yDim, xDim, \
                 sliceIndices }}"
            ),
        )),
    }
}

/// What a pipeline call draws, read strictly: an object naming `a`, `b` or
/// `op` is a combination and must name all three and nothing else.
pub(crate) fn grid_target(value: serde_json::Value) -> Result<GridTarget, RenderError> {
    if let serde_json::Value::Object(map) = &value
        && ["a", "b", "op"].iter().any(|k| map.contains_key(*k))
    {
        let serde_json::Value::Object(mut map) = value else {
            unreachable!("matched as an object above");
        };
        if let Some(extra) = map.keys().find(|k| !["a", "b", "op"].contains(&k.as_str())) {
            return Err(unreadable(
                "combined target",
                &format!("unknown key `{extra}`; it takes `a`, `b` and `op`"),
            ));
        }
        let (Some(a), Some(b), Some(op)) = (map.remove("a"), map.remove("b"), map.remove("op"))
        else {
            return Err(unreadable(
                "combined target",
                &"it takes all of `a`, `b` and `op`",
            ));
        };
        let serde_json::Value::String(op) = op else {
            return Err(unreadable(
                "combined target",
                &"`op` is a `combineOps()` value",
            ));
        };
        return Ok(GridTarget::Combined {
            a: grid_field(a)?,
            b: grid_field(b)?,
            op,
        });
    }
    grid_field(value).map(GridTarget::One)
}

/// The two slices of a combined slice target, which are drawn on the same
/// axes: the second slice's are checked rather than ignored.
fn same_axes(a: &SliceRequest, b: &SliceRequest) -> Result<(), RenderError> {
    if (a.y_dim, a.x_dim) == (b.y_dim, b.x_dim) {
        Ok(())
    } else {
        Err(Error::invalid_option(format!(
            "the two slices of a combined map are drawn on the same axes; `a` is on yDim {} and \
             xDim {}, `b` on yDim {} and xDim {}",
            a.y_dim, a.x_dim, b.y_dim, b.x_dim
        ))
        .into())
    }
}

/// Run `$body` with `$h` bound to whichever slice container `$inner` holds,
/// for the methods NetCDF files and Zarr stores both have under one signature.
macro_rules! on_slices {
    ($inner:expr, |$h:ident| $body:expr) => {
        match $inner {
            Inner::Netcdf($h) => Some($body),
            Inner::Zarr($h) => Some($body),
            Inner::Grib1(_) | Inner::Grib2(_) => None,
        }
    };
}

/// `on_slices` for the two GRIB files.
macro_rules! on_messages {
    ($inner:expr, |$h:ident| $body:expr) => {
        match $inner {
            Inner::Grib1($h) => Some($body),
            Inner::Grib2($h) => Some($body),
            Inner::Netcdf(_) | Inner::Zarr(_) => None,
        }
    };
}

/// The JavaScript surface. Each method reads its arguments, calls the Rust twin
/// below it of the same name without `_for_js`, and throws the refusal with its
/// `code`; the twins are what this crate's tests and the conformance runner
/// call, so they exercise this same dispatch.
#[napi]
impl Handle {
    /// Release the file and every memo now, rather than when the garbage
    /// collector runs. `[Symbol.dispose]` is the same method, so `using handle
    /// = open(bytes)` closes it at the end of the block. Calling it twice is
    /// harmless; any other call on a closed handle throws.
    #[napi]
    pub fn close(&mut self) {
        self.inner = None;
    }

    /// `"grib1"`, `"grib2"`, `"netcdf"` or `"zarr"`.
    #[napi(js_name = "format", ts_return_type = "SourceFormat")]
    pub fn format_for_js(&self, env: Env) -> napi::Result<serde_json::Value> {
        self.format().map_err(|e| e.throw(&env))
    }

    /// `"messages"` for a GRIB file, `"variables"` for a NetCDF file or a
    /// store. Ask this first: it says which calls apply.
    #[napi(js_name = "addressing", ts_return_type = "Addressing")]
    pub fn addressing_for_js(&self, env: Env) -> napi::Result<serde_json::Value> {
        self.addressing().map_err(|e| e.throw(&env))
    }

    /// How many messages the file holds: `message(i)` answers for every `i`
    /// below it. Zero for a variable container.
    #[napi(js_name = "count")]
    pub fn count_for_js(&self, env: Env) -> napi::Result<u32> {
        self.count().map_err(|e| e.throw(&env))
    }

    /// One message's metadata, the API's `MessageInfo` key for key: the same
    /// object the browser module's `message(index)` returns. Every key is
    /// present, `null` where there is nothing to report (#574). Built on each
    /// call, so a caller listing a file asks once per message and keeps it.
    #[napi(js_name = "message", ts_return_type = "MessageInfo")]
    pub fn message_for_js(&self, env: Env, index: u32) -> napi::Result<serde_json::Value> {
        self.message(index).map_err(|e| e.throw(&env))
    }

    /// The variables a slice can be drawn from, each with its axes and the
    /// image axes detected on it. Empty for a message stream.
    ///
    /// This host's own listing rather than the API's `VariableInfo`, in two
    /// places on purpose: `variableIndex` is the number every slice here takes
    /// (for a NetCDF file the file's own numbering), and `units` is typeset for
    /// display (ADR-0007).
    #[napi(js_name = "variables")]
    pub fn variables_for_js(&self, env: Env) -> napi::Result<Vec<NetcdfVariableMeta>> {
        self.variables().map_err(|e| e.throw(&env))
    }

    /// The arrays the container holds and this build will not read, each with
    /// the reader's reason and a `cause` code to word it by (#709, #1129).
    /// Empty for a message stream. The browser module's `leftOut()`.
    #[napi(js_name = "leftOut", ts_return_type = "LeftOutArray[]")]
    pub fn left_out_for_js(&self, env: Env) -> napi::Result<serde_json::Value> {
        self.left_out().map_err(|e| e.throw(&env))
    }

    /// The arrays the container reads that no map can be drawn from, each with
    /// why (#1129): text, a coordinate, fewer than two dimensions. With
    /// `leftOut()` and `variables()`, every array once. Empty for a message
    /// stream.
    #[napi(js_name = "undrawable", ts_return_type = "LeftOutArray[]")]
    pub fn undrawable_for_js(&self, env: Env) -> napi::Result<serde_json::Value> {
        self.undrawable().map_err(|e| e.throw(&env))
    }

    /// A NetCDF file's dimensions, global attributes and variables, for the
    /// metadata view, from the reader this handle already holds. Refused as
    /// `unsupported` for any other container.
    #[napi(js_name = "metadata")]
    pub fn metadata_for_js(&self, env: Env) -> napi::Result<DatasetMeta> {
        self.metadata().map_err(|e| e.throw(&env))
    }

    /// Decode one message: `options` is `{ dtype?: "auto" | "f32" | "f64" }`,
    /// the browser module's `decode` options.
    ///
    /// A spectral or HEALPix message has no raster of its own and comes back
    /// on the global lat/lon grid every other call here draws it on (#330,
    /// #580). `values` is at the width `dtype` asks for and a masked cell is
    /// `0`: read `mask` first.
    #[napi(js_name = "decode")]
    pub fn decode_for_js(
        &self,
        env: Env,
        index: u32,
        #[napi(ts_arg_type = "DecodeOptions | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> napi::Result<DecodedGrid> {
        self.decode(index, options).map_err(|e| e.throw(&env))
    }

    /// Decode one 2-D slice of a variable into the same object `decode`
    /// returns: the browser module's `decodeSlice`.
    ///
    /// `slice` names the variable, its two image axes and a position on every
    /// axis, the two image entries ignored. A `sliceIndices` of the wrong length
    /// is refused rather than padded.
    #[napi(js_name = "decodeSlice")]
    pub fn decode_slice_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "SliceRequest")] slice: serde_json::Value,
        #[napi(ts_arg_type = "DecodeOptions | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> napi::Result<DecodedGrid> {
        self.decode_slice(slice, options).map_err(|e| e.throw(&env))
    }

    /// One line through a variable: its values along `line.alongDim`, every
    /// other axis held at `line.sliceIndices` (#172). The browser module's
    /// `decodeLine`, with this host's line object.
    #[napi(js_name = "decodeLine")]
    pub fn decode_line_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "LineRequest")] line: serde_json::Value,
        #[napi(ts_arg_type = "DecodeOptions | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> napi::Result<LineResult> {
        self.decode_line(line, options).map_err(|e| e.throw(&env))
    }

    /// Decode one message to **draw**, at a reduced resolution (#1082):
    /// `options` is `{ reduce?, dtype? }`, the browser module's
    /// `decodeDisplay` options.
    ///
    /// `reduce` discards that many of a JPEG 2000 message's finest wavelet
    /// levels, so `1` is a field half the size on each axis. Those values are a
    /// low-pass the file does not hold, so this returns a `DisplayGrid`, which
    /// only `renderDisplayGrid` takes. `reduce: 0` is the message's own field;
    /// above zero, a message with no coarse field throws with `code:
    /// "unsupported"`, so ask again at `0`.
    #[napi(js_name = "decodeDisplay")]
    pub fn decode_display_for_js(
        &self,
        env: Env,
        index: u32,
        #[napi(ts_arg_type = "DisplayDecodeOptions | null | undefined")] options: Option<
            serde_json::Value,
        >,
    ) -> napi::Result<DisplayGrid> {
        self.decode_display(index, options)
            .map_err(|e| e.throw(&env))
    }

    /// Decode, project and paint a field into an RGBA raster under `options`.
    ///
    /// `target` is a message index, a slice `{ variable, yDim, xDim,
    /// sliceIndices }`, or `{ a, b, op }`: two of either combined cell by cell
    /// under `op`, a `value` from `combineOps()`, painted against `a`'s grid.
    /// The two must sit on the same grid, and two slices on the same axes.
    #[napi(js_name = "renderGrid")]
    pub fn render_grid_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridTarget")] target: serde_json::Value,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        self.render_grid(target, options).map_err(|e| e.throw(&env))
    }

    /// Paint a `DisplayGrid` through the `renderGrid` pipeline (#1082). The
    /// automatic colour range is the full field's, from `fullStats` — the
    /// `stats` of `decode` of the same message — and `null` is allowed only
    /// when both range ends are set.
    #[napi(js_name = "renderDisplayGrid")]
    pub fn render_display_grid_for_js(
        &self,
        env: Env,
        field: &DisplayGrid,
        #[napi(ts_arg_type = "Stats | null")] full_stats: Option<serde_json::Value>,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        self.render_display_grid(field, full_stats, &options)
            .map_err(|e| e.throw(&env))
    }

    /// Project geographic polylines (coastline, graticule, a user's shapes)
    /// onto the raster `renderGrid` paints for this target and these
    /// `options`, as pixel-space runs for the overlay canvas (#72).
    /// Geometry only: no values are decoded, and a combined target projects
    /// onto `a`'s grid.
    #[napi(js_name = "projectOverlay")]
    pub fn project_overlay_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridTarget")] target: serde_json::Value,
        options: RenderOptions,
        lines: Polylines,
    ) -> napi::Result<ProjectedOverlay> {
        self.project_overlay(target, options, lines)
            .map_err(|e| e.throw(&env))
    }

    /// Contour isolines of the target, projected onto its render raster
    /// (#238). `spacing.interval` sets the spacing between levels; left out,
    /// about eight round levels over the range painted. A combined target is
    /// contoured as the combined map, not as `a` (#329).
    #[napi(js_name = "projectContours")]
    pub fn project_contours_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridTarget")] target: serde_json::Value,
        options: RenderOptions,
        spacing: Option<ContourSpacing>,
    ) -> napi::Result<ProjectedOverlay> {
        self.project_contours(target, options, spacing.and_then(|s| s.interval))
            .map_err(|e| e.throw(&env))
    }

    /// Arrows for a vector field built from two messages (#241), in the pixel
    /// space the coastlines come back in. One arrow is one run of five
    /// vertices. Refused when the two are not on the same grid (#793) or state
    /// different component frames (#805); drawn, with `pairing` saying so,
    /// when they are not a u/v pair at all (#1049).
    #[napi(js_name = "projectVectors")]
    pub fn project_vectors_for_js(
        &self,
        env: Env,
        pair: VectorPair,
        options: RenderOptions,
    ) -> napi::Result<ProjectedVectors> {
        self.project_vectors(pair, options)
            .map_err(|e| e.throw(&env))
    }

    /// The field under one pixel of the raster `renderGrid` paints (#172):
    /// `(px, py)` in output pixels, after the flip. `null` off the raster or
    /// off the globe. A combined target reads the combined map (#329), and a
    /// band-limited spectral map reads its full-detail value beside it (#637).
    ///
    /// Not the browser module's `probe`, which reads a geographic point of a
    /// field rather than a pixel of a raster.
    #[napi(js_name = "probePixel")]
    pub fn probe_pixel_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridTarget")] target: serde_json::Value,
        options: RenderOptions,
        px: u32,
        py: u32,
    ) -> napi::Result<Option<ProbeResult>> {
        self.probe_pixel(target, options, px, py)
            .map_err(|e| e.throw(&env))
    }

    /// Each row's mean over longitude, against latitude (#240). Refused for a
    /// grid whose rows are not circles of latitude (rotated, projected,
    /// curvilinear).
    #[napi(js_name = "zonalMean")]
    pub fn zonal_mean_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridField")] field: serde_json::Value,
    ) -> napi::Result<LineResult> {
        self.zonal_mean(field).map_err(|e| e.throw(&env))
    }

    /// One field as CSV, as its UTF-8 bytes to write straight to disk (#341):
    /// `"matrix"` (a 2-D grid of values) or `"long"` (a `lat,lon,value`
    /// table), missing points as empty cells. The long form needs every
    /// point's position, so it covers the grids contours do.
    #[napi(js_name = "exportCsv")]
    pub fn export_csv_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridField")] field: serde_json::Value,
        #[napi(ts_arg_type = "\"matrix\" | \"long\"")] format: String,
    ) -> napi::Result<Uint8Array> {
        self.export_csv(field, format)
            .map(Uint8Array::from)
            .map_err(|e| e.throw(&env))
    }

    /// Where a slice on these image axes sits, as the render panel's picker asks
    /// it (#574, #776): the family to caption, whether it is placed, whether it
    /// can be reprojected, and what it supports. Decodes nothing.
    #[napi(js_name = "sliceGrid")]
    pub fn slice_grid_for_js(&self, env: Env, axes: SliceAxes) -> napi::Result<SliceGrid> {
        self.slice_grid(&axes).map_err(|e| e.throw(&env))
    }

    /// The coordinate values along one axis of a variable, for labelling a
    /// cross-section (#171). Reads the coordinate array only, never the field.
    #[napi(js_name = "axisValues", ts_return_type = "AxisValues")]
    pub fn axis_values_for_js(
        &self,
        env: Env,
        variable: u32,
        dim: u32,
    ) -> napi::Result<serde_json::Value> {
        self.axis_values(variable, dim).map_err(|e| e.throw(&env))
    }

    /// Patch the PDS `P1` (forecast period) octet of one GRIB1 message and
    /// return the whole edited file. Open a new handle over those bytes: a
    /// handle never changes under an edit. Refused for any other container.
    #[napi(js_name = "setP1")]
    pub fn set_p1_for_js(&self, env: Env, index: u32, value: u32) -> napi::Result<Uint8Array> {
        self.set_p1(index, value)
            .map(Uint8Array::from)
            .map_err(|e| e.throw(&env))
    }
}

/// The Rust twins of the methods above, with the engine's refusal kept rather
/// than thrown.
impl Handle {
    /// See `format_for_js`.
    pub(crate) fn format(&self) -> Result<serde_json::Value, RenderError> {
        let format = match self.inner()? {
            Inner::Grib1(h) => h.stream.session.format(),
            Inner::Grib2(h) => h.stream.session.format(),
            Inner::Netcdf(_) => SourceFormat::NetCdf,
            Inner::Zarr(h) => h.session.format(),
        };
        Ok(to_js(serde_json::to_value(format))?)
    }

    /// See `addressing_for_js`.
    pub(crate) fn addressing(&self) -> Result<serde_json::Value, RenderError> {
        let addressing = match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Addressing::Messages,
            Inner::Netcdf(_) | Inner::Zarr(_) => Addressing::Variables,
        };
        Ok(to_js(serde_json::to_value(addressing))?)
    }

    /// See `count_for_js`.
    pub(crate) fn count(&self) -> Result<u32, RenderError> {
        Ok(match self.inner()? {
            Inner::Grib1(h) => h.count(),
            Inner::Grib2(h) => h.count(),
            Inner::Netcdf(_) => 0,
            Inner::Zarr(h) => h.session.count(),
        })
    }

    /// See `message_for_js`.
    pub(crate) fn message(&self, index: u32) -> Result<serde_json::Value, RenderError> {
        let stream = self.messages("message")?;
        Ok(to_js(serde_json::to_value(stream.session.message(index)?))?)
    }

    /// See `variables_for_js`.
    pub(crate) fn variables(&self) -> Result<Vec<NetcdfVariableMeta>, RenderError> {
        Ok(match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Vec::new(),
            Inner::Netcdf(h) => h.variables(),
            Inner::Zarr(h) => h.variables(),
        })
    }

    /// See `left_out_for_js`.
    pub(crate) fn left_out(&self) -> Result<serde_json::Value, RenderError> {
        let arrays = match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Vec::new(),
            Inner::Netcdf(h) => h.left_out(),
            Inner::Zarr(h) => h.session.left_out(),
        };
        Ok(to_js(serde_json::to_value(arrays))?)
    }

    /// See `undrawable_for_js`.
    pub(crate) fn undrawable(&self) -> Result<serde_json::Value, RenderError> {
        let arrays = match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Vec::new(),
            Inner::Netcdf(h) => h.undrawable(),
            Inner::Zarr(h) => h.session.undrawable(),
        };
        Ok(to_js(serde_json::to_value(arrays))?)
    }

    /// See `metadata_for_js`.
    pub(crate) fn metadata(&self) -> Result<DatasetMeta, RenderError> {
        match self.inner()? {
            Inner::Netcdf(h) => Ok(h.metadata()),
            _ => Err(Error::Unsupported {
                detail: "`metadata` describes a NetCDF file, and this handle holds another \
                         container"
                    .to_string(),
            }
            .into()),
        }
    }

    /// See `decode_for_js`.
    pub(crate) fn decode(
        &self,
        index: u32,
        options: Option<serde_json::Value>,
    ) -> Result<DecodedGrid, RenderError> {
        let options = decode_options(options)?;
        Ok(
            on_messages!(self.inner()?, |h| h.decode(index, &options)).ok_or_else(|| {
                wrong_addressing(
                    Addressing::Variables,
                    "`decode` addresses a message; call `decodeSlice` instead",
                )
            })??,
        )
    }

    /// See `decode_slice_for_js`.
    pub(crate) fn decode_slice(
        &self,
        slice: serde_json::Value,
        options: Option<serde_json::Value>,
    ) -> Result<DecodedGrid, RenderError> {
        let slice: SliceRequest =
            serde_json::from_value(slice).map_err(|e| unreadable("slice", &e))?;
        let options = decode_options(options)?;
        match self.inner()? {
            Inner::Netcdf(h) => Ok(h.decode_slice(&slice, &options)?),
            Inner::Zarr(h) => h.decode_slice(&slice, &options),
            Inner::Grib1(_) | Inner::Grib2(_) => Err(wrong_addressing(
                Addressing::Messages,
                "`decodeSlice` addresses a variable; call `decode` instead",
            )),
        }
    }

    /// See `decode_line_for_js`.
    pub(crate) fn decode_line(
        &self,
        line: serde_json::Value,
        options: Option<serde_json::Value>,
    ) -> Result<LineResult, RenderError> {
        let line: LineRequest = serde_json::from_value(line).map_err(|e| unreadable("line", &e))?;
        let options = decode_options(options)?;
        on_slices!(self.inner()?, |h| h.line(
            line.variable,
            line.along_dim,
            line.slice_indices.clone(),
            &options
        ))
        .ok_or_else(|| {
            wrong_addressing(
                Addressing::Messages,
                "`decodeLine` addresses a variable; a message has no axes to read along",
            )
        })?
    }

    /// See `decode_display_for_js`.
    pub(crate) fn decode_display(
        &self,
        index: u32,
        options: Option<serde_json::Value>,
    ) -> Result<DisplayGrid, RenderError> {
        self.messages("decodeDisplay")?
            .decode_display(index, options)
    }

    /// See `render_display_grid_for_js`.
    pub(crate) fn render_display_grid(
        &self,
        field: &DisplayGrid,
        full_stats: Option<serde_json::Value>,
        options: &RenderOptions,
    ) -> Result<RenderedGrid, RenderError> {
        self.inner()?;
        render_display_grid(field, full_stats, options)
    }

    /// See `project_overlay_for_js`.
    pub(crate) fn project_overlay(
        &self,
        target: serde_json::Value,
        options: RenderOptions,
        lines: Polylines,
    ) -> Result<ProjectedOverlay, RenderError> {
        let field = match grid_target(target)? {
            GridTarget::One(f) | GridTarget::Combined { a: f, .. } => f,
        };
        let Polylines {
            latlon,
            ring_lengths,
        } = lines;
        match (self.inner()?, field) {
            (Inner::Grib1(h), GridField::Message(i)) => {
                Ok(h.project_overlay(i, options, latlon, ring_lengths)?)
            }
            (Inner::Grib2(h), GridField::Message(i)) => {
                Ok(h.project_overlay(i, options, latlon, ring_lengths)?)
            }
            (Inner::Netcdf(h), GridField::Slice(s)) => {
                Ok(h.project_overlay(s.variable, s.y_dim, s.x_dim, options, latlon, ring_lengths)?)
            }
            (Inner::Zarr(h), GridField::Slice(s)) => {
                Ok(h.project_overlay(s.variable, s.y_dim, s.x_dim, options, latlon, ring_lengths)?)
            }
            (inner, _) => Err(wrong_target(inner)),
        }
    }

    /// See `project_vectors_for_js`.
    pub(crate) fn project_vectors(
        &self,
        pair: VectorPair,
        options: RenderOptions,
    ) -> Result<ProjectedVectors, RenderError> {
        let VectorPair {
            u,
            v,
            spacing,
            grid_relative,
        } = pair;
        on_messages!(self.inner()?, |h| h
            .project_vectors(u, v, options, spacing, grid_relative)
            .map_err(RenderError::from))
        .ok_or_else(|| {
            wrong_addressing(
                Addressing::Variables,
                "`projectVectors` pairs two messages, and this handle holds variables",
            )
        })?
    }

    /// See `zonal_mean_for_js`.
    pub(crate) fn zonal_mean(&self, field: serde_json::Value) -> Result<LineResult, RenderError> {
        match (self.inner()?, grid_field(field)?) {
            (Inner::Grib1(h), GridField::Message(i)) => Ok(h.zonal_mean(i)?),
            (Inner::Grib2(h), GridField::Message(i)) => Ok(h.zonal_mean(i)?),
            (Inner::Netcdf(h), GridField::Slice(s)) => {
                Ok(h.zonal_mean(s.variable, s.y_dim, s.x_dim, s.slice_indices)?)
            }
            (Inner::Zarr(h), GridField::Slice(s)) => {
                Ok(h.zonal_mean(s.variable, s.y_dim, s.x_dim, s.slice_indices)?)
            }
            (inner, _) => Err(wrong_target(inner)),
        }
    }

    /// See `export_csv_for_js`: the CSV's bytes.
    pub(crate) fn export_csv(
        &self,
        field: serde_json::Value,
        format: String,
    ) -> Result<Vec<u8>, RenderError> {
        Ok(match (self.inner()?, grid_field(field)?) {
            (Inner::Grib1(h), GridField::Message(i)) => h.export_csv(i, format)?,
            (Inner::Grib2(h), GridField::Message(i)) => h.export_csv(i, format)?,
            (Inner::Netcdf(h), GridField::Slice(s)) => {
                h.export_csv(s.variable, s.y_dim, s.x_dim, s.slice_indices, format)?
            }
            (Inner::Zarr(h), GridField::Slice(s)) => {
                h.export_csv(s.variable, s.y_dim, s.x_dim, s.slice_indices, format)?
            }
            (inner, _) => return Err(wrong_target(inner)),
        })
    }

    /// See `slice_grid_for_js`.
    pub(crate) fn slice_grid(&self, axes: &SliceAxes) -> Result<SliceGrid, RenderError> {
        on_slices!(self.inner()?, |h| h
            .slice_grid(axes.variable, axes.y_dim, axes.x_dim)
            .map_err(RenderError::from))
        .ok_or_else(|| {
            wrong_addressing(
                Addressing::Messages,
                "`sliceGrid` places a slice, and this handle holds messages; read the message's \
                 `grid` instead",
            )
        })?
    }

    /// See `axis_values_for_js`.
    pub(crate) fn axis_values(
        &self,
        variable: u32,
        dim: u32,
    ) -> Result<serde_json::Value, RenderError> {
        on_slices!(self.inner()?, |h| h
            .axis_values(variable, dim)
            .map_err(RenderError::from))
        .ok_or_else(|| {
            wrong_addressing(
                Addressing::Messages,
                "`axisValues` reads a variable's axis, and this handle holds messages",
            )
        })?
    }

    /// See `set_p1_for_js`: the edited file's bytes.
    pub(crate) fn set_p1(&self, index: u32, value: u32) -> Result<Vec<u8>, RenderError> {
        match self.inner()? {
            Inner::Grib1(h) => Ok(h.patched_p1_bytes(index, value)?),
            _ => Err(Error::Unsupported {
                detail: "`setP1` edits a GRIB1 message, and this handle holds another container"
                    .to_string(),
            }
            .into()),
        }
    }
}

/// The three pipeline twins with a combined form.
impl Handle {
    /// See `render_grid_for_js`.
    pub(crate) fn render_grid(
        &self,
        target: serde_json::Value,
        options: RenderOptions,
    ) -> Result<RenderedGrid, RenderError> {
        let inner = self.inner()?;
        match grid_target(target)? {
            GridTarget::One(field) => match (inner, field) {
                (Inner::Grib1(h), GridField::Message(i)) => h.render_grid(i, options),
                (Inner::Grib2(h), GridField::Message(i)) => h.render_grid(i, options),
                (Inner::Netcdf(h), GridField::Slice(s)) => {
                    h.render_slice(s.variable, s.y_dim, s.x_dim, s.slice_indices, options)
                }
                (Inner::Zarr(h), GridField::Slice(s)) => {
                    h.render_slice(s.variable, s.y_dim, s.x_dim, s.slice_indices, options)
                }
                (inner, _) => Err(wrong_target(inner)),
            },
            GridTarget::Combined { a, b, op } => match (inner, a, b) {
                (Inner::Grib1(h), GridField::Message(a), GridField::Message(b)) => {
                    h.render_grid_combined(a, b, op, options)
                }
                (Inner::Grib2(h), GridField::Message(a), GridField::Message(b)) => {
                    h.render_grid_combined(a, b, op, options)
                }
                (Inner::Netcdf(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    h.render_slice_combined(
                        a.variable,
                        a.y_dim,
                        a.x_dim,
                        a.slice_indices,
                        b.variable,
                        b.slice_indices,
                        op,
                        options,
                    )
                }
                (Inner::Zarr(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    h.render_slice_combined(
                        a.variable,
                        a.y_dim,
                        a.x_dim,
                        a.slice_indices,
                        b.variable,
                        b.slice_indices,
                        op,
                        options,
                    )
                }
                (inner, _, _) => Err(wrong_target(inner)),
            },
        }
    }

    /// See `project_contours_for_js`.
    pub(crate) fn project_contours(
        &self,
        target: serde_json::Value,
        options: RenderOptions,
        interval: Option<f64>,
    ) -> Result<ProjectedOverlay, RenderError> {
        let inner = self.inner()?;
        match grid_target(target)? {
            GridTarget::One(field) => match (inner, field) {
                (Inner::Grib1(h), GridField::Message(i)) => {
                    h.project_contours(i, options, interval)
                }
                (Inner::Grib2(h), GridField::Message(i)) => {
                    h.project_contours(i, options, interval)
                }
                (Inner::Netcdf(h), GridField::Slice(s)) => h.project_contours(
                    s.variable,
                    s.y_dim,
                    s.x_dim,
                    s.slice_indices,
                    options,
                    interval,
                ),
                (Inner::Zarr(h), GridField::Slice(s)) => h.project_contours(
                    s.variable,
                    s.y_dim,
                    s.x_dim,
                    s.slice_indices,
                    options,
                    interval,
                ),
                (inner, _) => Err(wrong_target(inner)),
            },
            GridTarget::Combined { a, b, op } => match (inner, a, b) {
                (Inner::Grib1(h), GridField::Message(a), GridField::Message(b)) => {
                    h.project_contours_combined(a, b, op, options, interval)
                }
                (Inner::Grib2(h), GridField::Message(a), GridField::Message(b)) => {
                    h.project_contours_combined(a, b, op, options, interval)
                }
                (Inner::Netcdf(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    h.project_contours_slice_combined(
                        a.variable,
                        a.y_dim,
                        a.x_dim,
                        a.slice_indices,
                        b.variable,
                        b.slice_indices,
                        op,
                        options,
                        interval,
                    )
                }
                (Inner::Zarr(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    h.project_contours_slice_combined(
                        a.variable,
                        a.y_dim,
                        a.x_dim,
                        a.slice_indices,
                        b.variable,
                        b.slice_indices,
                        op,
                        options,
                        interval,
                    )
                }
                (inner, _, _) => Err(wrong_target(inner)),
            },
        }
    }

    /// See `probe_pixel_for_js`.
    pub(crate) fn probe_pixel(
        &self,
        target: serde_json::Value,
        options: RenderOptions,
        px: u32,
        py: u32,
    ) -> Result<Option<ProbeResult>, RenderError> {
        let inner = self.inner()?;
        let probed = match grid_target(target)? {
            GridTarget::One(field) => match (inner, field) {
                (Inner::Grib1(h), GridField::Message(i)) => h.probe(i, options, px, py),
                (Inner::Grib2(h), GridField::Message(i)) => h.probe(i, options, px, py),
                (Inner::Netcdf(h), GridField::Slice(s)) => h.probe(
                    s.variable,
                    s.y_dim,
                    s.x_dim,
                    s.slice_indices,
                    options,
                    px,
                    py,
                ),
                (Inner::Zarr(h), GridField::Slice(s)) => h.probe(
                    s.variable,
                    s.y_dim,
                    s.x_dim,
                    s.slice_indices,
                    options,
                    px,
                    py,
                ),
                (inner, _) => return Err(wrong_target(inner)),
            },
            GridTarget::Combined { a, b, op } => match (inner, a, b) {
                (Inner::Grib1(h), GridField::Message(a), GridField::Message(b)) => {
                    h.probe_combined(a, b, op, options, px, py)
                }
                (Inner::Grib2(h), GridField::Message(a), GridField::Message(b)) => {
                    h.probe_combined(a, b, op, options, px, py)
                }
                (Inner::Netcdf(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    h.probe_slice_combined(
                        a.variable,
                        a.y_dim,
                        a.x_dim,
                        a.slice_indices,
                        b.variable,
                        b.slice_indices,
                        op,
                        options,
                        px,
                        py,
                    )
                }
                (Inner::Zarr(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    h.probe_slice_combined(
                        a.variable,
                        a.y_dim,
                        a.x_dim,
                        a.slice_indices,
                        b.variable,
                        b.slice_indices,
                        op,
                        options,
                        px,
                        py,
                    )
                }
                (inner, _, _) => return Err(wrong_target(inner)),
            },
        };
        Ok(probed?)
    }
}

#[cfg(test)]
mod tests {
    //! The dispatch through the one handle (#1073): what a target reads as,
    //! which container answers it, and what the wrong question is told.

    use super::*;
    use crate::netcdf_slice_tests::opts;
    use serde_json::json;

    const GRIB1: &[u8] =
        include_bytes!("../../fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib");
    const GRIB2: &[u8] =
        include_bytes!("../../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");
    const ERSST: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc");

    fn code(e: &RenderError) -> Option<&'static str> {
        e.api().map(Error::code)
    }

    /// `sst` and `ssta`, on the same `lat × lon` plane, as this host numbers
    /// them.
    fn ersst() -> (Handle, i32, i32) {
        let handle = Handle::from_vec(ERSST.to_vec()).expect("ERSST opens");
        let variables = handle.variables().expect("lists");
        let index = |name: &str| {
            variables
                .iter()
                .find(|v| v.name == name)
                .map(|v| v.variable_index)
                .expect("listed")
        };
        (handle, index("sst"), index("ssta"))
    }

    fn slice(variable: i32) -> serde_json::Value {
        json!({ "variable": variable, "yDim": 2, "xDim": 3, "sliceIndices": [0, 0, 0, 0] })
    }

    #[test]
    fn open_detects_each_container_and_says_how_it_is_addressed() {
        for (bytes, format, addressing, count) in [
            (GRIB1, "grib1", "messages", 1),
            (GRIB2, "grib2", "messages", 1),
            (ERSST, "netcdf", "variables", 0),
        ] {
            let handle = Handle::from_vec(bytes.to_vec()).expect("opens");
            assert_eq!(handle.format().unwrap(), json!(format));
            assert_eq!(handle.addressing().unwrap(), json!(addressing));
            assert_eq!(handle.count().unwrap(), count, "{format}");
        }
    }

    #[test]
    fn bytes_that_are_no_container_are_unsupported_format() {
        let e = Handle::from_vec(b"not a weather file".to_vec()).expect_err("refused");
        assert_eq!(code(&e), Some("unsupported_format"), "{e}");
    }

    #[test]
    fn a_closed_handle_refuses_every_call_and_closes_twice() {
        let mut handle = Handle::from_vec(GRIB2.to_vec()).expect("opens");
        handle.close();
        handle.close();
        let e = handle.count().expect_err("closed");
        assert!(e.to_string().contains("closed"), "{e}");
        assert!(handle.render_grid(json!(0), opts("source")).is_err());
    }

    #[test]
    fn each_mode_refuses_the_others_question_by_name() {
        let grib = Handle::from_vec(GRIB2.to_vec()).expect("opens");
        let (netcdf, sst, _) = ersst();
        for e in [
            netcdf.decode(0, None).expect_err("a message question"),
            netcdf.message(0).expect_err("a message question"),
            netcdf
                .render_grid(json!(0), opts("source"))
                .expect_err("a message target"),
            grib.decode_slice(slice(sst), None)
                .expect_err("a variable question"),
            grib.render_grid(slice(sst), opts("source"))
                .expect_err("a slice target"),
        ] {
            assert_eq!(code(&e), Some("wrong_addressing"), "{e}");
        }
        // A message stream answers the listing questions empty, as the browser
        // module does, rather than refusing them.
        assert_eq!(grib.variables().unwrap().len(), 0);
        assert_eq!(grib.left_out().unwrap(), json!([]));
        assert_eq!(grib.undrawable().unwrap(), json!([]));
    }

    #[test]
    fn a_target_reads_strictly() {
        assert_eq!(
            grid_target(json!(3)).unwrap(),
            GridTarget::One(GridField::Message(3))
        );
        assert_eq!(
            grid_target(slice(4)).unwrap(),
            GridTarget::One(GridField::Slice(SliceRequest::new(
                4,
                2,
                3,
                vec![0, 0, 0, 0]
            )))
        );
        assert_eq!(
            grid_target(json!({ "a": 1, "b": 2, "op": "a_minus_b" })).unwrap(),
            GridTarget::Combined {
                a: GridField::Message(1),
                b: GridField::Message(2),
                op: "a_minus_b".to_string(),
            }
        );
        for bad in [
            json!(-1),
            json!(1.5),
            json!("0"),
            json!({ "a": 1, "b": 2 }),
            json!({ "a": 1, "b": 2, "op": "a_minus_b", "c": 3 }),
            json!({ "variable": 0, "yDim": 2, "xDim": 3 }),
            json!({ "variable": 0, "yDim": 2, "xDim": 3, "sliceIndices": [0], "sliceIndex": 0 }),
        ] {
            let e = grid_target(bad.clone()).expect_err("refused");
            assert_eq!(code(&e), Some("invalid_option"), "{bad}: {e}");
        }
    }

    /// `a` and `b` reach the container in that order: the difference of two
    /// different variables is the mirror of its swap, and the same as the
    /// container's own combined render.
    #[test]
    fn a_combined_target_keeps_its_order() {
        let (handle, sst, ssta) = ersst();
        let combined = |a: i32, b: i32| {
            handle
                .render_grid(
                    json!({ "a": slice(a), "b": slice(b), "op": "a_minus_b" }),
                    opts("source"),
                )
                .expect("renders")
        };
        let forward = combined(sst, ssta);
        let backward = combined(ssta, sst);
        assert_eq!(forward.used_min.map(|v| -v), backward.used_max);
        let Some(Inner::Netcdf(file)) = &handle.inner else {
            panic!("a NetCDF handle");
        };
        let direct = file
            .render_slice_combined(
                sst as u32,
                2,
                3,
                vec![0, 0, 0, 0],
                ssta as u32,
                vec![0, 0, 0, 0],
                "a_minus_b".to_string(),
                opts("source"),
            )
            .expect("renders");
        assert_eq!(&forward.rgba[..], &direct.rgba[..]);
        assert_ne!(&forward.rgba[..], &backward.rgba[..]);
    }

    #[test]
    fn two_slices_on_different_axes_are_not_combined() {
        let (handle, sst, _) = ersst();
        let other = json!({ "variable": sst, "yDim": 3, "xDim": 2, "sliceIndices": [0, 0, 0, 0] });
        let e = handle
            .render_grid(
                json!({ "a": slice(sst), "b": other, "op": "a_plus_b" }),
                opts("source"),
            )
            .expect_err("refused");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
    }

    #[test]
    fn decode_packs_at_the_width_asked_for() {
        let handle = Handle::from_vec(GRIB2.to_vec()).expect("opens");
        let narrow = handle
            .decode(0, Some(json!({ "dtype": "f32" })))
            .expect("decodes");
        let wide = handle
            .decode(0, Some(json!({ "dtype": "f64" })))
            .expect("decodes");
        assert!(matches!(narrow.values, napi::Either::A(_)));
        assert_eq!(narrow.dtype, "f32");
        assert!(matches!(wide.values, napi::Either::B(_)));
        assert_eq!(wide.dtype, "f64");
        assert_eq!(narrow.stats["validCount"], wide.stats["validCount"]);
        let e = handle
            .decode(0, Some(json!({ "dtype": "f16" })))
            .expect_err("an unknown width");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
    }

    #[test]
    fn only_a_grib1_file_takes_a_p1_edit() {
        let grib1 = Handle::from_vec(GRIB1.to_vec()).expect("opens");
        let edited = grib1.set_p1(0, 7).expect("edits");
        assert_eq!(edited.len(), GRIB1.len());
        let grib2 = Handle::from_vec(GRIB2.to_vec()).expect("opens");
        let e = grib2.set_p1(0, 7).expect_err("refused");
        assert_eq!(code(&e), Some("unsupported"), "{e}");
    }
}
