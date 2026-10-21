//! The one handle this addon exports, and the two ways to open one (#1073).
//!
//! ADR-0006 decision 1 makes every host a binding over `fieldglass::Session`,
//! and the browser module binds it as one `Handle`. This addon used to ship
//! four classes, one per container, with four sets of names for the same
//! questions. Now it ships one, and a method that both hosts have is spelt the
//! same and takes the same arguments in both: `count`, `message`, `variables`,
//! `dimensions`, `vectorPairing`, `leftOut`, `undrawable`, `decode`,
//! `decodeSlice`, `decodeLine`, `decodeDisplay`, `format`, `addressing` and
//! `close`.
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
use fieldglass::{
    Addressing, DecodeOptions, Error, LineRequest, SliceRequest, SourceFormat, VariableInfo,
};
use napi::JsValue;
use napi::bindgen_prelude::{
    Float64ArraySlice, FromNapiValue, Function, Object, Uint8Array, Uint32ArraySlice, Unknown,
};

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
pub fn open_store(env: Env, path: String) -> napi::Result<Handle> {
    Handle::from_store(path).map_err(|e| e.throw(&env))
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
        // Skipped rather than failed on: the addon loading matters more than
        // `using` working, and `close()` is there either way.
        let constructor: Unknown = exports.get_named_property(class)?;
        if constructor.get_type()? != napi::ValueType::Function {
            continue;
        }
        let mut prototype: Object = constructor
            .coerce_to_object()?
            .get_named_property("prototype")?;
        let close: Unknown = prototype.get_named_property("close")?;
        if close.get_type()? != napi::ValueType::Function {
            continue;
        }
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
    Netcdf(Box<NetcdfOpen>),
    Zarr(ZarrStore),
}

/// A NetCDF file, and its variables as `Session` lists and numbers them.
///
/// The file reads through its own reader, view and memos (ADR-0011), which
/// number a variable by its place in the file. Every slice a JavaScript caller
/// names uses the position in `variables()` instead, on this host as on the
/// browser module (#1073), so the listing is the API's own and each position is
/// translated to the file's number here, once, at the boundary.
#[derive(Debug)]
pub(crate) struct NetcdfOpen {
    file: NetcdfFile,
    /// `fieldglass::variables_of` over the file: the browser module's list.
    variables: Vec<VariableInfo>,
    /// The file's own index for each position in `variables`, `None` for one
    /// the view does not list as renderable (none, so far).
    decode_index: Vec<Option<u32>>,
}

impl NetcdfOpen {
    fn new(file: NetcdfFile) -> Self {
        let variables = fieldglass::variables_of(&fieldglass::netcdf::NetcdfArrays::new(
            &file.reader,
            &file.view,
        ));
        let renderable = file.view.renderable_variables();
        // The array model names a variable without the leading `/` a NetCDF-4
        // group path carries in the view, as `NetcdfFile::line` matches them.
        let decode_index = variables
            .iter()
            .map(|v| {
                renderable
                    .iter()
                    .find(|r| fieldglass::netcdf::presented_name(&r.name) == v.name)
                    .and_then(|r| u32::try_from(r.decode_index).ok())
            })
            .collect();
        Self {
            file,
            variables,
            decode_index,
        }
    }

    /// The file's own index for position `variable` of `variables()`.
    fn index_of(&self, variable: u32) -> Result<u32, RenderError> {
        match self.decode_index.get(variable as usize) {
            Some(Some(i)) => Ok(*i),
            Some(None) => Err(Error::Unsupported {
                detail: format!("variable {variable} has no plane this host reads"),
            }
            .into()),
            None => Err(Error::NoSuchMessage {
                index: variable,
                count: u32::try_from(self.variables.len()).unwrap_or(u32::MAX),
            }
            .into()),
        }
    }

    /// A slice with its variable translated to the file's own index.
    fn translate(&self, mut slice: SliceRequest) -> Result<SliceRequest, RenderError> {
        slice.variable = self.index_of(slice.variable)?;
        Ok(slice)
    }
}

impl std::ops::Deref for NetcdfOpen {
    type Target = NetcdfFile;

    fn deref(&self) -> &NetcdfFile {
        &self.file
    }
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
//
// Borrowed views rather than napi's owned `Float64Array` and `Uint32Array`.
// The owned types make a slice from whatever data pointer the engine hands
// over, and Electron hands an empty array over with one that is not aligned
// for its element type: a slice of it is undefined behaviour, which a debug
// build aborts on (#1247). The views make an empty one from an aligned
// dangling pointer instead.
#[napi(object, object_to_js = false)]
pub struct Polylines<'env> {
    /// Flat `[lat0, lon0, lat1, lon1, …]`, in degrees.
    #[napi(ts_type = "Float64Array")]
    pub latlon: Float64ArraySlice<'env>,
    /// How many points each ring holds, so `sum(ringLengths) * 2 ===
    /// latlon.length`.
    #[napi(ts_type = "Uint32Array")]
    pub ring_lengths: Uint32ArraySlice<'env>,
}

/// Lengths rather than the coordinates, as the other typed-array DTOs print.
impl std::fmt::Debug for Polylines<'_> {
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
    /// A handle over the Zarr store at directory `path`, with the engine's
    /// refusal kept. See `open_store`.
    pub(crate) fn from_store(path: String) -> Result<Self, RenderError> {
        Ok(Handle {
            inner: Some(Inner::Zarr(ZarrStore::from_directory(path)?)),
        })
    }

    /// A handle over `bytes`, for callers that are not JavaScript: the
    /// detection and the per-format open, with the engine's refusal kept.
    pub(crate) fn from_vec(bytes: Vec<u8>) -> Result<Self, RenderError> {
        let inner = match fieldglass::detect_from_bytes(&bytes) {
            fieldglass::Format::Grib1 => Inner::Grib1(Grib1File::from_vec(bytes)?),
            fieldglass::Format::Grib2 => Inner::Grib2(Grib2File::from_vec(bytes)?),
            fieldglass::Format::NetCdf => {
                Inner::Netcdf(Box::new(NetcdfOpen::new(NetcdfFile::from_vec(bytes)?)))
            }
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

    /// A target with each slice's variable in the numbering the container's
    /// own code takes: the file's for NetCDF, the listing's for a store.
    fn resolve(&self, target: GridTarget) -> Result<GridTarget, RenderError> {
        let Inner::Netcdf(open) = self.inner()? else {
            return Ok(target);
        };
        let field = |f: GridField| -> Result<GridField, RenderError> {
            Ok(match f {
                GridField::Slice(s) => GridField::Slice(open.translate(s)?),
                message => message,
            })
        };
        Ok(match target {
            GridTarget::One(f) => GridTarget::One(field(f)?),
            GridTarget::Combined { a, b, op } => GridTarget::Combined {
                a: field(a)?,
                b: field(b)?,
                op,
            },
        })
    }

    /// [`resolve`](Self::resolve) for one field.
    fn resolve_field(&self, field: GridField) -> Result<GridField, RenderError> {
        match self.resolve(GridTarget::One(field))? {
            GridTarget::One(f) => Ok(f),
            GridTarget::Combined { .. } => unreachable!("one field in, one out"),
        }
    }

    /// A variable position in the numbering the container's own code takes.
    fn resolve_variable(&self, variable: u32) -> Result<u32, RenderError> {
        match self.inner()? {
            Inner::Netcdf(open) => open.index_of(variable),
            _ => Ok(variable),
        }
    }

    /// How many warps this handle has planned (#1233): what a test counts to
    /// hold that a repaint which changes only the colours, or the field on the
    /// same grid, does not warp again.
    #[cfg(test)]
    pub(crate) fn plans_made(&self) -> u64 {
        match self.inner.as_ref().expect("an open handle") {
            Inner::Grib1(h) => h.stream.warps.plans_made(),
            Inner::Grib2(h) => h.stream.warps.plans_made(),
            Inner::Netcdf(open) => open.file.warps.plans_made(),
            Inner::Zarr(h) => h.warps.plans_made(),
        }
    }

    /// The container, or the refusal a closed handle gives.
    fn inner(&self) -> Result<&Inner, RenderError> {
        self.inner
            .as_ref()
            .ok_or_else(|| Error::closed("handle").into())
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

/// An argument that must be an object: `Ok(None)` when left out or `null`, and
/// refused as `invalid_option` when it is anything but an object.
///
/// A serde-derived struct also reads from a JSON array, field by field, where
/// `deny_unknown_fields` never applies: `["f64"]` would read as decode options
/// here and be refused by the browser module. Every options object and request
/// this handle reads goes through this first (#1073).
///
/// Both hosts do the same: an array, a string or a number is `invalid_option`,
/// and `null` or `undefined` is the default (#1210). The one input they take
/// beyond that is a plain object keyed `"0"`, `"1"`, … as `sliceIndices`: napi
/// hands a `Uint32Array` over as exactly that object, so it cannot be told from
/// a typed array here, and the browser module reads it as the array too. A
/// typed array as the argument itself never gets this far: [`object_arg`] and
/// [`target_arg`] refuse it while it is still a JavaScript value (#1239).
pub(crate) fn object_or_absent(
    value: Option<serde_json::Value>,
    what: &str,
) -> Result<Option<serde_json::Value>, RenderError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) if v.is_object() => Ok(Some(v)),
        Some(_) => Err(unreadable(what, &"it is not an object")),
    }
}

/// What `Object.prototype.toString` calls a value: `[object ArrayBuffer]`,
/// `[object SharedArrayBuffer]`, `[object Object]` and so on.
///
/// The name holds for a value made in another realm (a `vm` context, an
/// iframe), where `instanceof` does not. It is how both hosts recognise a
/// `SharedArrayBuffer`, which no napi check names: `napi_is_arraybuffer` is
/// false for one (#1242).
fn brand(env: &Env, value: Unknown<'_>) -> napi::Result<String> {
    let object: Function<'_, (), Unknown<'_>> = env.get_global()?.get_named_property("Object")?;
    let prototype: Object<'_> = object.get_named_property("prototype")?;
    let to_string: Function<'_, (), String> = prototype.get_named_property("toString")?;
    to_string.apply(value, ())
}

/// Whether a JavaScript value is binary data: a typed array (a Node `Buffer`
/// is one), a `DataView`, an `ArrayBuffer` or a `SharedArrayBuffer`.
///
/// napi reads each into `serde_json` as an object keyed `"0"`, `"1"`, … or as
/// one with no keys at all, and into a `#[napi(object)]` struct as one whose
/// fields are all missing. Past that read, `new Uint8Array(1)` and `{ 0: 0 }`
/// are the same value, so the question is asked before it (#1239). The two
/// buffers are named by [`brand`] as well as by napi's own check, as the
/// browser module names them, so the hosts refuse the same values (#1242).
fn is_binary(env: &Env, value: Unknown<'_>) -> napi::Result<bool> {
    if value.is_typedarray()? || value.is_dataview()? || value.is_arraybuffer()? {
        return Ok(true);
    }
    if value.get_type()? != napi::ValueType::Object {
        return Ok(false);
    }
    Ok(matches!(
        brand(env, value)?.as_str(),
        "[object ArrayBuffer]" | "[object SharedArrayBuffer]"
    ))
}

/// Read an object argument, refusing what is not one before napi reads it
/// into `T` (#1239).
///
/// The option types ignore keys they do not name, so a typed array read as an
/// object was the defaults: `decode(0, new Uint8Array(1))` decoded at `auto`,
/// where the browser module refuses it. Once napi has read it, a typed array
/// and `{ 0: 0 }` are the same value, so the check is made here, on the
/// argument as napi hands it over.
///
/// So this refuses what the browser module's `refuse_non_object` refuses: a
/// value that is not an object, an array, and binary data ([`is_binary`]).
/// Each is `invalid_option` with the same words, `it is not an object`. Only
/// the top level is checked: a `Uint32Array` of `sliceIndices` inside a
/// request is read as before.
///
/// A `GridTarget` or `GridField` may be a message index, a number, so it is
/// read by [`target_arg`] instead.
pub(crate) fn object_arg<T: FromNapiValue>(
    env: &Env,
    value: Unknown<'_>,
    what: &str,
) -> Result<T, RenderError> {
    if value.get_type()? != napi::ValueType::Object || value.is_array()? || is_binary(env, value)? {
        return Err(unreadable(what, &"it is not an object"));
    }
    Ok(T::from_unknown(value)?)
}

/// [`object_arg`] for an argument that may be left out. napi reads `null` and
/// `undefined` as `None` before this is asked, so they stay the default.
pub(crate) fn object_arg_or_absent<T: FromNapiValue>(
    env: &Env,
    value: Option<Unknown<'_>>,
    what: &str,
) -> Result<Option<T>, RenderError> {
    value.map(|value| object_arg(env, value, what)).transpose()
}

/// Read a `GridTarget` or `GridField` argument into JSON, refusing binary data
/// before it becomes JSON (#1239).
///
/// A target is a message index or an object, so unlike [`object_arg`] this
/// lets a number, and anything else that is not binary, through to the
/// target's own reading and its own refusals. Binary data ([`is_binary`]) is
/// refused as `invalid_option`. A `Buffer` used to fail inside napi's own
/// conversion, with napi's code.
pub(crate) fn target_arg(
    env: &Env,
    value: Unknown<'_>,
    what: &str,
) -> Result<serde_json::Value, RenderError> {
    if is_binary(env, value)? {
        return Err(unreadable(
            what,
            &"binary data is neither a message index nor a slice { variable, yDim, xDim, \
              sliceIndices }",
        ));
    }
    Ok(serde_json::Value::from_unknown(value)?)
}

/// Read decode options the way the browser module does: `null` or left out is
/// the default, and anything but an object is refused.
fn decode_options(value: Option<serde_json::Value>) -> Result<DecodeOptions, RenderError> {
    match object_or_absent(value, "options object")? {
        None => Ok(DecodeOptions::default()),
        Some(v) => serde_json::from_value(v).map_err(|e| unreadable("options object", &e)),
    }
}

/// A slice or line request read strictly, with a typed array of slice indices
/// taken as the array it holds.
///
/// napi hands a `Uint32Array` to `serde_json` as an object keyed `"0"`, `"1"`,
/// …, which a `Vec<u32>` refuses. The browser module reads one as an array, so
/// this host does too, rather than the two refusing different inputs (#1073).
pub(crate) fn read_request<T: RequestType>(
    value: serde_json::Value,
    what: &str,
) -> Result<T, RenderError> {
    // Anything but an object is refused first, as on the browser module: see
    // `object_or_absent`. A request is never optional, so `null` is refused too.
    let Some(mut value) = object_or_absent(Some(value), what)? else {
        return Err(unreadable(what, &"it is not an object"));
    };
    if let Some(indices) = value.get_mut("sliceIndices")
        && let serde_json::Value::Object(map) = indices
        && (0..map.len()).all(|i| map.contains_key(&i.to_string()))
    {
        let array = (0..map.len())
            .map(|i| {
                map.remove(&i.to_string())
                    .unwrap_or(serde_json::Value::Null)
            })
            .collect();
        *indices = serde_json::Value::Array(array);
    }
    T::from_wire(value).map_err(|e| unreadable(what, &e))
}

/// The two request types [`read_request`] reads; a trait rather than serde's
/// own bound because this crate names no `serde` (only `serde_json`).
pub(crate) trait RequestType: Sized {
    /// `serde_json::from_value` for this type.
    fn from_wire(value: serde_json::Value) -> Result<Self, serde_json::Error>;
}

impl RequestType for SliceRequest {
    fn from_wire(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

impl RequestType for LineRequest {
    fn from_wire(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
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
        object @ serde_json::Value::Object(_) => {
            read_request(object, "slice").map(GridField::Slice)
        }
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
    match combined_parts(value, "combined target")? {
        Ok((a, b, op)) => Ok(GridTarget::Combined {
            a: grid_field(a)?,
            b: grid_field(b)?,
            op,
        }),
        Err(value) => grid_field(value).map(GridTarget::One),
    }
}

/// Two lines combined point by point, read strictly as `grid_target` reads a
/// combined map: `{ a, b, op }` and nothing else, `a` and `b` each a line
/// request (#1208).
pub(crate) fn combined_lines(
    value: serde_json::Value,
) -> Result<(LineRequest, LineRequest, String, bool), RenderError> {
    // `holdB` is the one key a combined line adds to a combined map's three.
    let mut value = value;
    let hold_b = match &mut value {
        serde_json::Value::Object(map) => map.remove("holdB"),
        _ => None,
    };
    let hold_b = match hold_b {
        Some(serde_json::Value::Bool(hold)) => hold,
        Some(other) => {
            return Err(unreadable(
                "combined line",
                &format!("`holdB` is true or false, not {other}"),
            ));
        }
        None => {
            return Err(unreadable(
                "combined line",
                &"it takes `holdB`: whether the line runs along a held axis",
            ));
        }
    };
    match combined_parts(value, "combined line")? {
        Ok((a, b, op)) => Ok((
            read_request(a, "line `a`")?,
            read_request(b, "line `b`")?,
            op,
            hold_b,
        )),
        Err(_) => Err(unreadable(
            "combined line",
            &"it takes `a` and `b`, two line requests, `op` and `holdB`",
        )),
    }
}

/// An object naming `a`, `b` or `op`, taken apart: all three and nothing
/// else, `op` a string. Anything else is handed back as it came, as one field.
#[allow(clippy::type_complexity)]
fn combined_parts(
    value: serde_json::Value,
    what: &str,
) -> Result<Result<(serde_json::Value, serde_json::Value, String), serde_json::Value>, RenderError>
{
    let serde_json::Value::Object(mut map) = value else {
        return Ok(Err(value));
    };
    if !["a", "b", "op"].iter().any(|k| map.contains_key(*k)) {
        return Ok(Err(serde_json::Value::Object(map)));
    }
    if let Some(extra) = map.keys().find(|k| !["a", "b", "op"].contains(&k.as_str())) {
        return Err(unreadable(
            what,
            &format!("unknown key `{extra}`; it takes `a`, `b` and `op`"),
        ));
    }
    let (Some(a), Some(b), Some(op)) = (map.remove("a"), map.remove("b"), map.remove("op")) else {
        return Err(unreadable(what, &"it takes all of `a`, `b` and `op`"));
    };
    let serde_json::Value::String(op) = op else {
        return Err(unreadable(what, &"`op` is a `combineOps()` value"));
    };
    Ok(Ok((a, b, op)))
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
    /// image axes detected on it: the API's `VariableInfo`, as the browser
    /// module returns it. Empty for a message stream.
    ///
    /// `index` is the position every slice and line request names. `units` is
    /// the file's own spelling, which `typesetUnits` typesets for display
    /// (ADR-0007).
    #[napi(js_name = "variables", ts_return_type = "VariableInfo[]")]
    pub fn variables_for_js(&self, env: Env) -> napi::Result<serde_json::Value> {
        self.variables()
            .and_then(|v| to_js(serde_json::to_value(v)))
            .map_err(|e| e.throw(&env))
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

    /// The dataset's shared dimensions, in the order the file declares them:
    /// the API's `DimensionInfo[]`, as the browser module returns it. Two
    /// variables naming one dimension are on one axis, so a page offers one
    /// slider for it. Empty for a message stream (#1205).
    #[napi(js_name = "dimensions", ts_return_type = "DimensionInfo[]")]
    pub fn dimensions_for_js(&self, env: Env) -> napi::Result<serde_json::Value> {
        self.dimensions().map_err(|e| e.throw(&env))
    }

    /// Whether `u` and `v` are the two components of one vector, by the
    /// parameter each names: `"recognised"`, `"swapped"`, `"not_a_pair"` or
    /// `"unknown"`, the API's `VectorPairing` as the browser module returns it.
    /// The indices are messages for GRIB and the `variables()` list for NetCDF
    /// and Zarr. Not a refusal when it is not a pair; an index past the end
    /// throws `no_such_message` (#1205).
    #[napi(js_name = "vectorPairing", ts_return_type = "VectorPairing")]
    pub fn vector_pairing_for_js(
        &self,
        env: Env,
        u: u32,
        v: u32,
    ) -> napi::Result<serde_json::Value> {
        self.vector_pairing(u, v).map_err(|e| e.throw(&env))
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
        #[napi(ts_arg_type = "DecodeOptions | null")] options: Option<Unknown<'_>>,
    ) -> napi::Result<DecodedGrid> {
        let throw = |e: RenderError| e.throw(&env);
        let options = object_arg_or_absent(&env, options, "options object").map_err(throw)?;
        self.decode(index, options).map_err(throw)
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
        #[napi(ts_arg_type = "SliceRequest")] slice: Unknown<'_>,
        #[napi(ts_arg_type = "DecodeOptions | null")] options: Option<Unknown<'_>>,
    ) -> napi::Result<DecodedGrid> {
        let throw = |e: RenderError| e.throw(&env);
        let slice = object_arg(&env, slice, "slice").map_err(throw)?;
        let options = object_arg_or_absent(&env, options, "options object").map_err(throw)?;
        self.decode_slice(slice, options).map_err(throw)
    }

    /// One line through a variable: its values along `line.alongDim`, every
    /// other axis held at `line.sliceIndices` (#172). The browser module's
    /// `decodeLine`, with this host's line object.
    #[napi(js_name = "decodeLine")]
    pub fn decode_line_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "LineRequest")] line: Unknown<'_>,
        #[napi(ts_arg_type = "DecodeOptions | null")] options: Option<Unknown<'_>>,
    ) -> napi::Result<LineResult> {
        let throw = |e: RenderError| e.throw(&env);
        let line = object_arg(&env, line, "line").map_err(throw)?;
        let options = object_arg_or_absent(&env, options, "options object").map_err(throw)?;
        self.decode_line(line, options).map_err(throw)
    }

    /// One line through a combined map (#1208): `{ a, b, op, holdB }`, two
    /// line requests along the same `alongDim`, combined point by point under
    /// `op`, a `value` from `combineOps()`.
    ///
    /// `holdB` says which kind of axis the line runs along. `false`, an axis
    /// of the map: `b` is read along it too, and through a cell written into
    /// both requests the line is a row or column of the combined map. `true`,
    /// a held axis (a time or a level): `b` is its one cell at its own
    /// position, its entry for `alongDim` included, combined with every step
    /// of `a`, so the point at `a`'s entry is the combined map's value there.
    /// Named, with units and coordinates, as `a`'s line.
    #[napi(js_name = "decodeLineCombined")]
    pub fn decode_line_combined_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "CombinedLines")] line: Unknown<'_>,
        #[napi(ts_arg_type = "DecodeOptions | null")] options: Option<Unknown<'_>>,
    ) -> napi::Result<LineResult> {
        let throw = |e: RenderError| e.throw(&env);
        let line = object_arg(&env, line, "combined line").map_err(throw)?;
        let options = object_arg_or_absent(&env, options, "options object").map_err(throw)?;
        self.decode_line_combined(line, options).map_err(throw)
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
        #[napi(ts_arg_type = "DisplayDecodeOptions | null")] options: Option<Unknown<'_>>,
    ) -> napi::Result<DisplayGrid> {
        let throw = |e: RenderError| e.throw(&env);
        let options = object_arg_or_absent(&env, options, "options object").map_err(throw)?;
        self.decode_display(index, options).map_err(throw)
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
        #[napi(ts_arg_type = "GridTarget")] target: Unknown<'_>,
        #[napi(ts_arg_type = "RenderOptions")] options: Unknown<'_>,
    ) -> napi::Result<RenderedGrid> {
        let throw = |e: RenderError| e.throw(&env);
        let target = target_arg(&env, target, "target").map_err(throw)?;
        let options = object_arg(&env, options, "options object").map_err(throw)?;
        self.render_grid(target, options).map_err(throw)
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
        #[napi(ts_arg_type = "Stats | null")] full_stats: Option<Unknown<'_>>,
        #[napi(ts_arg_type = "RenderOptions")] options: Unknown<'_>,
    ) -> napi::Result<RenderedGrid> {
        let throw = |e: RenderError| e.throw(&env);
        let full_stats =
            object_arg_or_absent(&env, full_stats, "full-field stats").map_err(throw)?;
        let options = object_arg(&env, options, "options object").map_err(throw)?;
        self.render_display_grid(field, full_stats, &options)
            .map_err(throw)
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
        #[napi(ts_arg_type = "GridTarget")] target: Unknown<'_>,
        #[napi(ts_arg_type = "RenderOptions")] options: Unknown<'_>,
        #[napi(ts_arg_type = "Polylines")] lines: Unknown<'_>,
    ) -> napi::Result<ProjectedOverlay> {
        let throw = |e: RenderError| e.throw(&env);
        let target = target_arg(&env, target, "target").map_err(throw)?;
        let options = object_arg(&env, options, "options object").map_err(throw)?;
        let lines = object_arg(&env, lines, "polylines").map_err(throw)?;
        self.project_overlay(target, options, lines).map_err(throw)
    }

    /// Contour isolines of the target, projected onto its render raster
    /// (#238). `spacing.interval` sets the spacing between levels; left out,
    /// about eight round levels over the range painted. A combined target is
    /// contoured as the combined map, not as `a` (#329).
    #[napi(js_name = "projectContours")]
    pub fn project_contours_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridTarget")] target: Unknown<'_>,
        #[napi(ts_arg_type = "RenderOptions")] options: Unknown<'_>,
        #[napi(ts_arg_type = "ContourSpacing | undefined | null")] spacing: Option<Unknown<'_>>,
    ) -> napi::Result<ProjectedOverlay> {
        let throw = |e: RenderError| e.throw(&env);
        let target = target_arg(&env, target, "target").map_err(throw)?;
        let options = object_arg(&env, options, "options object").map_err(throw)?;
        let spacing = object_arg_or_absent::<ContourSpacing>(&env, spacing, "contour spacing")
            .map_err(throw)?;
        self.project_contours(target, options, spacing.and_then(|s| s.interval))
            .map_err(throw)
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
        #[napi(ts_arg_type = "VectorPair")] pair: Unknown<'_>,
        #[napi(ts_arg_type = "RenderOptions")] options: Unknown<'_>,
    ) -> napi::Result<ProjectedVectors> {
        let throw = |e: RenderError| e.throw(&env);
        let pair = object_arg(&env, pair, "vector pair").map_err(throw)?;
        let options = object_arg(&env, options, "options object").map_err(throw)?;
        self.project_vectors(pair, options).map_err(throw)
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
        #[napi(ts_arg_type = "GridTarget")] target: Unknown<'_>,
        #[napi(ts_arg_type = "RenderOptions")] options: Unknown<'_>,
        px: u32,
        py: u32,
    ) -> napi::Result<Option<ProbeResult>> {
        let throw = |e: RenderError| e.throw(&env);
        let target = target_arg(&env, target, "target").map_err(throw)?;
        let options = object_arg(&env, options, "options object").map_err(throw)?;
        self.probe_pixel(target, options, px, py).map_err(throw)
    }

    /// Each row's mean over longitude, against latitude (#240). Refused for a
    /// grid whose rows are not circles of latitude (rotated, projected,
    /// curvilinear). A combined target is averaged as the combined map, not
    /// as `a` (#1208), and named as `a` is.
    #[napi(js_name = "zonalMean")]
    pub fn zonal_mean_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridTarget")] target: Unknown<'_>,
    ) -> napi::Result<LineResult> {
        let throw = |e: RenderError| e.throw(&env);
        let target = target_arg(&env, target, "target").map_err(throw)?;
        self.zonal_mean(target).map_err(throw)
    }

    /// One field as CSV, as its UTF-8 bytes to write straight to disk (#341):
    /// `"matrix"` (a 2-D grid of values) or `"long"` (a `lat,lon,value`
    /// table), missing points as empty cells. The long form needs every
    /// point's position, so it covers the grids contours do.
    #[napi(js_name = "exportCsv")]
    pub fn export_csv_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "GridField")] field: Unknown<'_>,
        #[napi(ts_arg_type = "\"matrix\" | \"long\"")] format: String,
    ) -> napi::Result<Uint8Array> {
        let throw = |e: RenderError| e.throw(&env);
        let field = target_arg(&env, field, "field").map_err(throw)?;
        self.export_csv(field, format)
            .map(Uint8Array::from)
            .map_err(throw)
    }

    /// Where a slice on these image axes sits, as the render panel's picker asks
    /// it (#574, #776): the family to caption, whether it is placed, whether it
    /// can be reprojected, and what it supports. Decodes nothing.
    #[napi(js_name = "sliceGrid")]
    pub fn slice_grid_for_js(
        &self,
        env: Env,
        #[napi(ts_arg_type = "SliceAxes")] axes: Unknown<'_>,
    ) -> napi::Result<SliceGrid> {
        let throw = |e: RenderError| e.throw(&env);
        let axes = object_arg(&env, axes, "slice axes").map_err(throw)?;
        self.slice_grid(&axes).map_err(throw)
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
        to_js(serde_json::to_value(format))
    }

    /// See `addressing_for_js`.
    pub(crate) fn addressing(&self) -> Result<serde_json::Value, RenderError> {
        let addressing = match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Addressing::Messages,
            Inner::Netcdf(_) | Inner::Zarr(_) => Addressing::Variables,
        };
        to_js(serde_json::to_value(addressing))
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
        to_js(serde_json::to_value(stream.session.message(index)?))
    }

    /// See `variables_for_js`.
    pub(crate) fn variables(&self) -> Result<Vec<VariableInfo>, RenderError> {
        Ok(match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Vec::new(),
            Inner::Netcdf(open) => open.variables.clone(),
            Inner::Zarr(h) => h.session.variables(),
        })
    }

    /// See `left_out_for_js`.
    pub(crate) fn left_out(&self) -> Result<serde_json::Value, RenderError> {
        let arrays = match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Vec::new(),
            Inner::Netcdf(h) => h.left_out(),
            Inner::Zarr(h) => h.session.left_out(),
        };
        to_js(serde_json::to_value(arrays))
    }

    /// See `undrawable_for_js`.
    pub(crate) fn undrawable(&self) -> Result<serde_json::Value, RenderError> {
        let arrays = match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Vec::new(),
            Inner::Netcdf(h) => h.undrawable(),
            Inner::Zarr(h) => h.session.undrawable(),
        };
        to_js(serde_json::to_value(arrays))
    }

    /// See `dimensions_for_js`.
    pub(crate) fn dimensions(&self) -> Result<serde_json::Value, RenderError> {
        let dimensions = match self.inner()? {
            Inner::Grib1(_) | Inner::Grib2(_) => Vec::new(),
            Inner::Netcdf(h) => h.dimensions(),
            Inner::Zarr(h) => h.session.dimensions(),
        };
        to_js(serde_json::to_value(dimensions))
    }

    /// See `vector_pairing_for_js`.
    pub(crate) fn vector_pairing(&self, u: u32, v: u32) -> Result<serde_json::Value, RenderError> {
        let pairing = match self.inner()? {
            Inner::Grib1(h) => h.stream.session.vector_pairing(u, v)?,
            Inner::Grib2(h) => h.stream.session.vector_pairing(u, v)?,
            Inner::Netcdf(h) => h.vector_pairing(u, v)?,
            Inner::Zarr(h) => h.session.vector_pairing(u, v)?,
        };
        to_js(serde_json::to_value(pairing))
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
        on_messages!(self.inner()?, |h| h.decode(index, &options)).ok_or_else(|| {
            wrong_addressing(
                Addressing::Variables,
                "`decode` addresses a message; call `decodeSlice` instead",
            )
        })?
    }

    /// See `decode_slice_for_js`.
    pub(crate) fn decode_slice(
        &self,
        slice: serde_json::Value,
        options: Option<serde_json::Value>,
    ) -> Result<DecodedGrid, RenderError> {
        let slice: SliceRequest = read_request(slice, "slice")?;
        let options = decode_options(options)?;
        match self.inner()? {
            Inner::Netcdf(h) => Ok(h.decode_slice(&h.translate(slice)?, &options)?),
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
        let mut line: LineRequest = read_request(line, "line")?;
        line.variable = self.resolve_variable(line.variable)?;
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

    /// See `decode_line_combined_for_js`.
    pub(crate) fn decode_line_combined(
        &self,
        line: serde_json::Value,
        options: Option<serde_json::Value>,
    ) -> Result<LineResult, RenderError> {
        let (mut a, mut b, op, hold_b) = combined_lines(line)?;
        if a.along_dim != b.along_dim {
            return Err(Error::invalid_option(format!(
                "the two lines of a combined line run along the same axis; `a` reads along \
                 dimension {}, `b` along dimension {}",
                a.along_dim, b.along_dim
            ))
            .into());
        }
        a.variable = self.resolve_variable(a.variable)?;
        b.variable = self.resolve_variable(b.variable)?;
        let options = decode_options(options)?;
        on_slices!(self.inner()?, |h| h
            .line_combined(&a, &b, hold_b, &op, &options))
        .ok_or_else(|| {
            wrong_addressing(
                Addressing::Messages,
                "`decodeLineCombined` addresses variables; a message has no axes to read along",
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
        lines: Polylines<'_>,
    ) -> Result<ProjectedOverlay, RenderError> {
        let field = match self.resolve(grid_target(target)?)? {
            GridTarget::One(f) | GridTarget::Combined { a: f, .. } => f,
        };
        let Polylines {
            latlon,
            ring_lengths,
        } = lines;
        match (self.inner()?, field) {
            (Inner::Grib1(h), GridField::Message(i)) => {
                Ok(h.project_overlay(i, options, &latlon, &ring_lengths)?)
            }
            (Inner::Grib2(h), GridField::Message(i)) => {
                Ok(h.project_overlay(i, options, &latlon, &ring_lengths)?)
            }
            (Inner::Netcdf(h), GridField::Slice(s)) => Ok(h.project_overlay(
                s.variable,
                s.y_dim,
                s.x_dim,
                options,
                &latlon,
                &ring_lengths,
            )?),
            (Inner::Zarr(h), GridField::Slice(s)) => Ok(h.project_overlay(
                s.variable,
                s.y_dim,
                s.x_dim,
                options,
                &latlon,
                &ring_lengths,
            )?),
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
        on_messages!(self.inner()?, |h| h.project_vectors(
            u,
            v,
            options,
            spacing,
            grid_relative
        ))
        .ok_or_else(|| {
            wrong_addressing(
                Addressing::Variables,
                "`projectVectors` pairs two messages, and this handle holds variables",
            )
        })?
    }

    /// See `zonal_mean_for_js`.
    pub(crate) fn zonal_mean(&self, target: serde_json::Value) -> Result<LineResult, RenderError> {
        let inner = self.inner()?;
        match self.resolve(grid_target(target)?)? {
            GridTarget::One(field) => match (inner, field) {
                (Inner::Grib1(h), GridField::Message(i)) => Ok(h.zonal_mean(i)?),
                (Inner::Grib2(h), GridField::Message(i)) => Ok(h.zonal_mean(i)?),
                (Inner::Netcdf(h), GridField::Slice(s)) => {
                    Ok(h.zonal_mean(s.variable, s.y_dim, s.x_dim, s.slice_indices)?)
                }
                (Inner::Zarr(h), GridField::Slice(s)) => {
                    Ok(h.zonal_mean(s.variable, s.y_dim, s.x_dim, s.slice_indices)?)
                }
                (inner, _) => Err(wrong_target(inner)),
            },
            GridTarget::Combined { a, b, op } => match (inner, a, b) {
                (Inner::Grib1(h), GridField::Message(a), GridField::Message(b)) => {
                    Ok(h.zonal_mean_combined(a, b, &op)?)
                }
                (Inner::Grib2(h), GridField::Message(a), GridField::Message(b)) => {
                    Ok(h.zonal_mean_combined(a, b, &op)?)
                }
                (Inner::Netcdf(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    Ok(h.zonal_mean_slice_combined(&a, &b, &op)?)
                }
                (Inner::Zarr(h), GridField::Slice(a), GridField::Slice(b)) => {
                    same_axes(&a, &b)?;
                    Ok(h.zonal_mean_slice_combined(&a, &b, &op)?)
                }
                (inner, _, _) => Err(wrong_target(inner)),
            },
        }
    }

    /// See `export_csv_for_js`: the CSV's bytes.
    pub(crate) fn export_csv(
        &self,
        field: serde_json::Value,
        format: String,
    ) -> Result<Vec<u8>, RenderError> {
        Ok(
            match (self.inner()?, self.resolve_field(grid_field(field)?)?) {
                (Inner::Grib1(h), GridField::Message(i)) => h.export_csv(i, format)?,
                (Inner::Grib2(h), GridField::Message(i)) => h.export_csv(i, format)?,
                (Inner::Netcdf(h), GridField::Slice(s)) => {
                    h.export_csv(s.variable, s.y_dim, s.x_dim, s.slice_indices, format)?
                }
                (Inner::Zarr(h), GridField::Slice(s)) => {
                    h.export_csv(s.variable, s.y_dim, s.x_dim, s.slice_indices, format)?
                }
                (inner, _) => return Err(wrong_target(inner)),
            },
        )
    }

    /// See `slice_grid_for_js`.
    pub(crate) fn slice_grid(&self, axes: &SliceAxes) -> Result<SliceGrid, RenderError> {
        let variable = self.resolve_variable(axes.variable)?;
        on_slices!(self.inner()?, |h| h
            .slice_grid(variable, axes.y_dim, axes.x_dim))
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
        let variable = self.resolve_variable(variable)?;
        on_slices!(self.inner()?, |h| h.axis_values(variable, dim)).ok_or_else(|| {
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
        match self.resolve(grid_target(target)?)? {
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
        match self.resolve(grid_target(target)?)? {
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
        match self.resolve(grid_target(target)?)? {
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
                (inner, _) => Err(wrong_target(inner)),
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
                (inner, _, _) => Err(wrong_target(inner)),
            },
        }
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
    fn ersst() -> (Handle, u32, u32) {
        let handle = Handle::from_vec(ERSST.to_vec()).expect("ERSST opens");
        let variables = handle.variables().expect("lists");
        let index = |name: &str| {
            variables
                .iter()
                .find(|v| v.name == name)
                .map(|v| v.index)
                .expect("listed")
        };
        (handle, index("sst"), index("ssta"))
    }

    fn slice(variable: u32) -> serde_json::Value {
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
        assert_eq!(code(&e), Some("closed"), "{e}");
        let e = handle
            .render_grid(json!(0), opts("source"))
            .expect_err("closed");
        assert_eq!(code(&e), Some("closed"), "{e}");
    }

    /// The addon's own refusals carry an API code (#1203), the same one the
    /// browser module gives for the same input: a position the file does not
    /// list, a `sliceIndices` of the wrong length, an axis the variable lacks.
    #[test]
    fn the_addons_own_refusals_carry_an_api_code() {
        let (handle, sst, _) = ersst();
        let slice_of = |value: serde_json::Value| handle.decode_slice(value, None);
        let e = slice_of(slice(99)).expect_err("a variable the file does not list");
        assert_eq!(code(&e), Some("no_such_message"), "{e}");
        let short = json!({ "variable": sst, "yDim": 2, "xDim": 3, "sliceIndices": [0, 0] });
        let e = slice_of(short).expect_err("a sliceIndices of the wrong length");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
        // An axis the variable lacks, or one axis named twice: the reader
        // refuses these, so the code is whatever `Session` gives the same
        // request, and the two hosts agree on it.
        let session = fieldglass::Session::open(ERSST.to_vec()).expect("opens");
        for (y, x) in [(9, 3), (3, 3)] {
            let request =
                json!({ "variable": sst, "yDim": y, "xDim": x, "sliceIndices": [0, 0, 0, 0] });
            let e = slice_of(request.clone()).expect_err("a bad axis");
            let theirs = session
                .decode_slice(sst, y, x, &[0, 0, 0, 0], &DecodeOptions::default())
                .expect_err("the session refuses it too");
            assert_eq!(code(&e), Some(theirs.code()), "{e}");
            // The map pipeline checks them first, and names the same kind of
            // fault.
            let e = handle
                .render_grid(request, opts("source"))
                .expect_err("a bad axis");
            assert_eq!(code(&e), Some("invalid_option"), "{e}");
        }
        // The map pipeline reaches the same checks by another route.
        let e = handle
            .render_grid(
                json!({ "variable": sst, "yDim": 2, "xDim": 3, "sliceIndices": [0] }),
                opts("source"),
            )
            .expect_err("a sliceIndices of the wrong length");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
    }

    /// A directory that is no Zarr store is a container this build cannot open,
    /// and says so with that code (#1203).
    #[test]
    fn a_directory_that_is_no_store_is_unsupported_format() {
        // A directory in the repo that holds no Zarr marker.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        let e = Handle::from_store(dir.to_string()).expect_err("no store");
        assert_eq!(code(&e), Some("unsupported_format"), "{e}");
        assert!(e.to_string().contains("is not a Zarr store"), "{e}");
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
        // A serde-derived struct would read an array field by field; a request
        // is an object or nothing, as the browser module reads it.
        let positional = json!([4, 2, 3, [0, 0, 0, 0]]);
        for bad in [positional.clone(), json!([[4, 2, 3, [0, 0, 0, 0]]])] {
            let e = grid_target(bad.clone()).expect_err("refused");
            assert_eq!(code(&e), Some("invalid_option"), "{bad}: {e}");
        }
        let e = read_request::<SliceRequest>(positional, "slice").expect_err("refused");
        assert!(e.to_string().contains("not an object"), "{e}");
        let e = read_request::<LineRequest>(json!([4, 2, [0, 0]]), "line").expect_err("refused");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
        // Options objects too: an array is not decode options on either host.
        let grib = Handle::from_vec(GRIB2.to_vec()).expect("opens");
        for bad in [json!(["f64"]), json!("f64"), json!(3)] {
            let e = grib.decode(0, Some(bad.clone())).expect_err("refused");
            assert_eq!(code(&e), Some("invalid_option"), "{bad}: {e}");
            let e = grib
                .decode_display(0, Some(bad.clone()))
                .expect_err("refused");
            assert_eq!(code(&e), Some("invalid_option"), "{bad}: {e}");
        }
        assert!(
            grib.decode(0, Some(serde_json::Value::Null)).is_ok(),
            "null is the default"
        );
    }

    /// `a` and `b` reach the container in that order: the difference of two
    /// different variables is the mirror of its swap, and the same as the
    /// container's own combined render.
    #[test]
    fn a_combined_target_keeps_its_order() {
        let (handle, sst, ssta) = ersst();
        let combined = |a: u32, b: u32| {
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
        // The file's own code numbers variables by their place in the file;
        // the handle translated the listing's positions to those.
        let direct = file
            .render_slice_combined(
                file.index_of(sst).expect("listed"),
                2,
                3,
                vec![0, 0, 0, 0],
                file.index_of(ssta).expect("listed"),
                vec![0, 0, 0, 0],
                "a_minus_b".to_string(),
                opts("source"),
            )
            .expect("renders");
        assert_eq!(&forward.rgba[..], &direct.rgba[..]);
        assert_ne!(&forward.rgba[..], &backward.rgba[..]);
    }

    /// A `Uint32Array` reaches `serde_json` as an index-keyed object, and is
    /// read as the array it is, as the browser module reads it. A plain
    /// `{ "0": 0, … }` is taken the same way on both hosts (#1210).
    #[test]
    fn a_typed_array_of_slice_indices_reads_as_its_array() {
        let as_napi_hands_it = json!({
            "variable": 4, "yDim": 2, "xDim": 3,
            "sliceIndices": { "0": 0, "1": 1, "2": 0, "3": 0 },
        });
        assert_eq!(
            grid_target(as_napi_hands_it).unwrap(),
            GridTarget::One(GridField::Slice(SliceRequest::new(
                4,
                2,
                3,
                vec![0, 1, 0, 0]
            )))
        );
        // An object that is not index-keyed is still refused.
        let e = grid_target(json!({
            "variable": 4, "yDim": 2, "xDim": 3, "sliceIndices": { "time": 0 },
        }))
        .expect_err("refused");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
        // So is one with a gap, as on the browser module (#1210).
        let e = grid_target(json!({
            "variable": 4, "yDim": 2, "xDim": 3, "sliceIndices": { "0": 0, "2": 0 },
        }))
        .expect_err("refused");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
    }

    /// The listing is the API's own, numbered by position as `Session` numbers
    /// it, so the two hosts mean one variable by one number (#1073).
    #[test]
    fn a_netcdf_listing_is_the_sessions() {
        let (handle, _, _) = ersst();
        let session = fieldglass::Session::open(ERSST.to_vec()).expect("opens");
        assert_eq!(handle.variables().unwrap(), session.variables());
        let past = u32::try_from(session.variables().len()).unwrap();
        let e = handle
            .decode_slice(
                json!({ "variable": past, "yDim": 2, "xDim": 3, "sliceIndices": [0, 0, 0, 0] }),
                None,
            )
            .expect_err("past the list");
        assert_eq!(code(&e), Some("no_such_message"), "{e}");
    }

    /// A NetCDF-4 file's variables in nested groups: each position resolves to
    /// the file's own variable, whose view name carries the group path with a
    /// leading `/`, and decodes to what `Session` decodes at that position.
    #[test]
    fn grouped_variables_resolve_by_their_group_path() {
        let bytes = include_bytes!("../../fieldglass-netcdf/tests/fixtures/netcdf4_grouped.nc");
        let handle = Handle::from_vec(bytes.to_vec()).expect("opens");
        let session = fieldglass::Session::open(bytes.to_vec()).expect("opens");
        let listed = handle.variables().unwrap();
        assert!(
            listed.iter().any(|v| v.name.contains('/')),
            "the fixture has grouped variables"
        );
        let Some(Inner::Netcdf(open)) = &handle.inner else {
            panic!("a NetCDF handle");
        };
        assert!(
            open.decode_index.iter().all(Option::is_some),
            "{:?}",
            open.decode_index
        );
        for v in &listed {
            let (y, x) = (v.detected_y_dim.unwrap_or(0), v.detected_x_dim.unwrap_or(1));
            let indices = vec![0; v.dims.len()];
            let ours = handle
                .decode_slice(
                    json!({ "variable": v.index, "yDim": y, "xDim": x, "sliceIndices": indices }),
                    Some(json!({ "dtype": "f64" })),
                )
                .unwrap_or_else(|e| panic!("{}: {e}", v.name));
            let theirs = session
                .decode_slice(
                    v.index,
                    y,
                    x,
                    &indices,
                    &fieldglass::DecodeOptions::new(fieldglass::Dtype::F64),
                )
                .expect("decodes");
            assert_eq!((ours.ni, ours.nj), (theirs.ni, theirs.nj), "{}", v.name);
            assert_eq!(&ours.mask[..], &theirs.mask[..], "{}", v.name);
            let napi::Either::B(values) = &ours.values else {
                panic!("f64 asked for");
            };
            assert_eq!(&values[..], &theirs.values.to_f64()[..], "{}", v.name);
        }
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

    /// A line's points, absent ones as `None`.
    fn points(line: &LineResult) -> Vec<Option<f64>> {
        line.values
            .iter()
            .zip(&line.mask)
            .map(|(&v, &m)| (m == 1).then_some(v))
            .collect()
    }

    /// `sst` less `ssta` along latitude row `j`, from the two single lines.
    fn row_difference(handle: &Handle, sst: u32, ssta: u32, j: u32) -> Vec<Option<f64>> {
        let row = |variable: u32| {
            let line = json!({ "variable": variable, "alongDim": 3, "sliceIndices": [0, 0, j, 0] });
            points(&handle.decode_line(line, None).expect("a row"))
        };
        row(sst)
            .into_iter()
            .zip(row(ssta))
            .map(|(a, b)| fieldglass::combine_cell(a, b, fieldglass::CombineOp::Difference))
            .collect()
    }

    /// A combined line and a combined zonal mean read the map the combined
    /// target draws (#1208): each point the two fields' combine, and each
    /// row's mean the mean of the combined row.
    #[test]
    fn a_combined_line_and_zonal_mean_read_the_combined_map() {
        let (handle, sst, ssta) = ersst();
        let line = |variable: u32, j: u32| json!({ "variable": variable, "alongDim": 3, "sliceIndices": [0, 0, j, 0] });
        let combined = handle
            .decode_line_combined(
                json!({ "a": line(sst, 40), "b": line(ssta, 40), "op": "a_minus_b", "holdB": false }),
                None,
            )
            .expect("the combined row");
        let want = row_difference(&handle, sst, ssta, 40);
        assert!(want.iter().any(Option::is_some), "the row has sea in it");
        assert_eq!(points(&combined), want);
        assert_eq!(combined.variable.as_deref(), Some("sst"), "named as `a`");

        let zonal = handle
            .zonal_mean(json!({ "a": slice(sst), "b": slice(ssta), "op": "a_minus_b" }))
            .expect("the combined zonal mean");
        let rows = u32::try_from(zonal.values.len()).expect("rows");
        let got = points(&zonal);
        for j in 0..rows {
            let row: Vec<f64> = row_difference(&handle, sst, ssta, j)
                .into_iter()
                .flatten()
                .collect();
            let want = (!row.is_empty()).then(|| row.iter().sum::<f64>() / row.len() as f64);
            match (got[j as usize], want) {
                (Some(g), Some(w)) => assert!((g - w).abs() < 1e-9, "row {j}: {g} against {w}"),
                (g, w) => assert_eq!(g, w, "row {j}"),
            }
        }

        // A message stream's combined zonal mean: a message and itself summed
        // is twice the message.
        let pair = Handle::from_vec([GRIB2, GRIB2].concat()).expect("opens");
        let single = pair.zonal_mean(json!(0)).expect("one message");
        let sum = pair
            .zonal_mean(json!({ "a": 0, "b": 1, "op": "a_plus_b" }))
            .expect("the sum");
        let doubled: Vec<Option<f64>> = points(&single)
            .into_iter()
            .map(|v| v.map(|v| 2.0 * v))
            .collect();
        assert_eq!(points(&sum), doubled);
    }

    /// A store answers a combined line and zonal mean as its NetCDF twin does,
    /// though the store goes through `Session` and the file through its own
    /// reader: `t − a` along each axis, and averaged by row (#1208).
    #[test]
    fn a_store_combines_as_its_netcdf_twin_does() {
        let store =
            Handle::from_store("../fieldglass-zarr/tests/fixtures/stores/cf_v3".to_string())
                .expect("the store opens");
        let twin = Handle::from_vec(
            std::fs::read("../fieldglass-zarr/tests/fixtures/cf_twin.nc").expect("the twin"),
        )
        .expect("the twin opens");
        let index = |handle: &Handle, name: &str| {
            handle
                .variables()
                .expect("lists")
                .iter()
                .find(|v| v.name == name)
                .map(|v| v.index)
                .expect("listed")
        };
        let answers = |handle: &Handle| {
            let (t, a) = (index(handle, "t"), index(handle, "a"));
            let mut out = Vec::new();
            for (along, at) in [(1u32, [2u32, 0u32]), (0, [0, 3])] {
                let line = |variable: u32| json!({ "variable": variable, "alongDim": along, "sliceIndices": at });
                let combined = handle
                    .decode_line_combined(
                        json!({ "a": line(t), "b": line(a), "op": "a_minus_b", "holdB": false }),
                        None,
                    )
                    .expect("the combined line");
                let single = |variable: u32| {
                    points(&handle.decode_line(line(variable), None).expect("a line"))
                };
                let want: Vec<Option<f64>> = single(t)
                    .into_iter()
                    .zip(single(a))
                    .map(|(x, y)| fieldglass::combine_cell(x, y, fieldglass::CombineOp::Difference))
                    .collect();
                assert_eq!(points(&combined), want, "along {along}");
                out.push(points(&combined));
            }
            let slice = |variable: u32| json!({ "variable": variable, "yDim": 0, "xDim": 1, "sliceIndices": [0, 0] });
            let zonal = handle
                .zonal_mean(json!({ "a": slice(t), "b": slice(a), "op": "a_minus_b" }))
                .expect("the combined zonal mean");
            out.push(points(&zonal));
            out
        };
        let (from_store, from_twin) = (answers(&store), answers(&twin));
        assert!(from_store[2].iter().any(Option::is_some), "a zonal mean");
        assert_eq!(from_store, from_twin);
    }

    /// A GRIB1 file's combined zonal mean (#1208): a message and itself
    /// summed is twice it, and their ratio is one wherever the message is not
    /// zero.
    #[test]
    fn a_grib1_combined_zonal_mean_reads_the_combined_map() {
        let one: &[u8] =
            include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");
        let pair = Handle::from_vec([one, one].concat()).expect("opens");
        let single = points(&pair.zonal_mean(json!(0)).expect("one message"));
        assert!(single.iter().any(Option::is_some), "a zonal mean");
        let sum = pair
            .zonal_mean(json!({ "a": 0, "b": 1, "op": "a_plus_b" }))
            .expect("the sum");
        let doubled: Vec<Option<f64>> = single.iter().map(|v| v.map(|v| 2.0 * v)).collect();
        assert_eq!(points(&sum), doubled);
        let mean = pair
            .zonal_mean(json!({ "a": 0, "b": 1, "op": "mean" }))
            .expect("the mean");
        assert_eq!(
            points(&mean),
            single,
            "the mean of a message and itself is the message"
        );
    }

    /// A combined zonal mean of two fields that do not line up is refused, as
    /// the combined map is: two GRIB1 grids, or two slices on other axes.
    #[test]
    fn a_combined_zonal_mean_of_misaligned_fields_is_refused() {
        let latlon: &[u8] =
            include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");
        let gaussian: &[u8] =
            include_bytes!("../../fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1");
        let pair = Handle::from_vec([latlon, gaussian].concat()).expect("opens");
        let e = pair
            .zonal_mean(json!({ "a": 0, "b": 1, "op": "a_minus_b" }))
            .expect_err("two grids");
        // The words the alignment gate uses for the map, which is refused too.
        assert!(e.to_string().contains("different grids"), "{e}");
        let render = pair
            .render_grid(json!({ "a": 0, "b": 1, "op": "a_minus_b" }), opts("source"))
            .expect_err("the map is refused too");
        assert!(render.to_string().contains("different grids"), "{render}");

        let (handle, sst, _) = ersst();
        let other = json!({ "variable": sst, "yDim": 3, "xDim": 2, "sliceIndices": [0, 0, 0, 0] });
        let e = handle
            .zonal_mean(json!({ "a": slice(sst), "b": other, "op": "a_minus_b" }))
            .expect_err("two slices on other axes");
        assert_eq!(code(&e), Some("invalid_option"), "{e}");
    }

    /// Along a held axis, `holdB` reads Field B once, at its own position,
    /// and combines it with every step of Field A (#1208): on
    /// `temperature(time, lat, lon) = t·12 + j·4 + i`, A at the cell is
    /// `[6, 18]` along time and B at time 0 is 6.
    #[test]
    fn a_held_combined_line_reads_b_once() {
        let handle = Handle::from_vec(
            include_bytes!("../../fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc").to_vec(),
        )
        .expect("opens");
        let t = handle
            .variables()
            .expect("lists")
            .iter()
            .find(|v| v.name == "temperature")
            .map(|v| v.index)
            .expect("listed");
        let line = |at: [u32; 3]| json!({ "variable": t, "alongDim": 0, "sliceIndices": at });
        let combined = |hold_b: bool| {
            points(
                &handle
                    .decode_line_combined(
                        json!({ "a": line([1, 1, 2]), "b": line([0, 1, 2]), "op": "a_minus_b", "holdB": hold_b }),
                        None,
                    )
                    .expect("the line"),
            )
        };
        assert_eq!(combined(true), vec![Some(0.0), Some(12.0)]);
        assert_eq!(
            combined(false),
            vec![Some(0.0), Some(0.0)],
            "read along, B steps with A"
        );
    }

    #[test]
    fn a_combined_line_reads_strictly() {
        let (handle, sst, ssta) = ersst();
        let line = |variable: u32, along: u32| json!({ "variable": variable, "alongDim": along, "sliceIndices": [0, 0, 0, 0] });
        for bad in [
            line(sst, 3),
            json!({ "a": line(sst, 3), "b": line(ssta, 3), "holdB": false }),
            json!({ "a": line(sst, 3), "b": line(ssta, 3), "op": "a_minus_b" }),
            json!({ "a": line(sst, 3), "b": line(ssta, 3), "op": "a_minus_b", "holdB": 1 }),
            json!({ "a": line(sst, 3), "b": line(ssta, 3), "op": "a_minus_b", "holdB": false, "c": 0 }),
            json!({ "a": 0, "b": line(ssta, 3), "op": "a_minus_b", "holdB": false }),
            json!({ "a": line(sst, 3), "b": line(ssta, 2), "op": "a_minus_b", "holdB": false }),
            json!({ "a": line(sst, 3), "b": line(ssta, 3), "op": "a_times_b", "holdB": false }),
        ] {
            let e = handle
                .decode_line_combined(bad.clone(), None)
                .expect_err("refused");
            assert_eq!(code(&e), Some("invalid_option"), "{bad}: {e}");
        }
        let grib = Handle::from_vec(GRIB2.to_vec()).expect("opens");
        let e = grib
            .decode_line_combined(
                json!({ "a": line(0, 0), "b": line(0, 0), "op": "a_minus_b", "holdB": false }),
                None,
            )
            .expect_err("a message has no axes");
        assert_eq!(code(&e), Some("wrong_addressing"), "{e}");
    }
    const UV_GRIB2: &[u8] = include_bytes!("../../fieldglass/tests/fixtures/uv_pairs.grib2");

    /// A Zarr v2 store on disk with one `lat x lon` variable per `(name,
    /// standard_name)`, under `target/`, removed when dropped.
    struct UvStore(std::path::PathBuf);

    impl UvStore {
        fn new(tag: &str, variables: &[(&str, Option<&str>)]) -> Self {
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../target")
                .join(format!("napi_uv_store_{tag}"));
            let _ = std::fs::remove_dir_all(&root);
            let f64s = |v: &[f64]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
            let array = |shape: &str| {
                format!(
                    r#"{{"zarr_format":2,"shape":{shape},"chunks":{shape},"dtype":"<f8","compressor":null,"fill_value":null,"order":"C","filters":null}}"#
                )
            };
            let mut files: Vec<(String, Vec<u8>)> = vec![
                (".zgroup".into(), br#"{"zarr_format":2}"#.to_vec()),
                ("lat/.zarray".into(), array("[2]").into_bytes()),
                (
                    "lat/.zattrs".into(),
                    br#"{"_ARRAY_DIMENSIONS":["lat"],"units":"degrees_north"}"#.to_vec(),
                ),
                ("lat/0".into(), f64s(&[10.0, 20.0])),
                ("lon/.zarray".into(), array("[2]").into_bytes()),
                (
                    "lon/.zattrs".into(),
                    br#"{"_ARRAY_DIMENSIONS":["lon"],"units":"degrees_east"}"#.to_vec(),
                ),
                ("lon/0".into(), f64s(&[30.0, 40.0])),
            ];
            for (name, standard) in variables {
                let mut attrs = json!({ "_ARRAY_DIMENSIONS": ["lat", "lon"] });
                if let Some(standard) = standard {
                    attrs["standard_name"] = (*standard).into();
                }
                files.push((format!("{name}/.zarray"), array("[2,2]").into_bytes()));
                files.push((format!("{name}/.zattrs"), attrs.to_string().into_bytes()));
                files.push((format!("{name}/0.0"), f64s(&[1.0, 2.0, 3.0, 4.0])));
            }
            for (key, bytes) in files {
                let path = root.join(&key);
                std::fs::create_dir_all(path.parent().expect("a parent")).expect("a directory");
                std::fs::write(path, bytes).expect("a file");
            }
            Self(root)
        }

        fn open(&self) -> Handle {
            Handle::from_store(self.0.to_string_lossy().into_owned()).expect("the store opens")
        }
    }

    impl Drop for UvStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// `dimensions()` is the umbrella's answer on every container, whichever
    /// reader the handle holds: a message stream has none, a NetCDF file gives
    /// what `Session` gives for the same bytes though the handle reads it
    /// through its own reader, and a store gives its session's (#1205).
    #[test]
    fn dimensions_are_the_sessions_on_every_container() {
        let none = Handle::from_vec(GRIB2.to_vec()).expect("opens");
        assert_eq!(none.dimensions().unwrap(), json!([]));

        let (nc, _, _) = ersst();
        let session = fieldglass::Session::open(ERSST.to_vec()).expect("opens");
        let expected = serde_json::to_value(session.dimensions()).expect("serialises");
        assert_eq!(nc.dimensions().unwrap(), expected);
        assert_eq!(expected.as_array().map(Vec::len), Some(4), "{expected}");

        let store = UvStore::new("dimensions", &[("t", None)]).open();
        assert_eq!(
            store.dimensions().unwrap(),
            json!([{ "name": "lat", "length": 2 }, { "name": "lon", "length": 2 }]),
        );
    }

    /// `vector_pairing` reaches each container's answer: GRIB messages by their
    /// parameter, and a store's variables by their CF `standard_name`, both in
    /// every order and with the refusal for an index past the end (#1205).
    #[test]
    fn vector_pairing_is_the_sessions_on_every_container() {
        let grib = Handle::from_vec(UV_GRIB2.to_vec()).expect("opens");
        for (u, v, expected) in [
            (0, 1, "recognised"),
            (1, 0, "swapped"),
            (0, 2, "not_a_pair"),
        ] {
            assert_eq!(
                grib.vector_pairing(u, v).unwrap(),
                json!(expected),
                "{u},{v}"
            );
        }
        let e = grib.vector_pairing(0, 4).expect_err("past the end");
        assert_eq!(code(&e), Some("no_such_message"), "{e}");

        let store = UvStore::new(
            "pairing",
            &[
                ("u10", Some("eastward_wind")),
                ("v10", Some("northward_wind")),
                ("t", Some("air_temperature")),
                ("plain", None),
            ],
        )
        .open();
        let at = |name: &str| {
            store
                .variables()
                .expect("lists")
                .iter()
                .find(|v| v.name == name)
                .map(|v| v.index)
                .expect("listed")
        };
        let (u, v, t, plain) = (at("u10"), at("v10"), at("t"), at("plain"));
        for (a, b, expected) in [
            (u, v, "recognised"),
            (v, u, "swapped"),
            (u, t, "not_a_pair"),
            (u, plain, "unknown"),
        ] {
            assert_eq!(
                store.vector_pairing(a, b).unwrap(),
                json!(expected),
                "{a},{b}"
            );
        }
        let e = store.vector_pairing(u, 99).expect_err("past the end");
        assert_eq!(code(&e), Some("no_such_message"), "{e}");

        // A NetCDF file goes through the handle's own reader, not a `Session`.
        let (nc, sst, ssta) = ersst();
        let session = fieldglass::Session::open(ERSST.to_vec()).expect("opens");
        assert_eq!(
            nc.vector_pairing(sst, ssta).unwrap(),
            serde_json::to_value(session.vector_pairing(sst, ssta).expect("answers")).unwrap(),
        );
        let e = nc.vector_pairing(sst, 99).expect_err("past the end");
        assert_eq!(code(&e), Some("no_such_message"), "{e}");
    }

    #[test]
    fn a_closed_handle_refuses_dimensions_and_vector_pairing() {
        let mut handle = Handle::from_vec(UV_GRIB2.to_vec()).expect("opens");
        handle.close();
        for e in [
            handle.dimensions().expect_err("closed"),
            handle.vector_pairing(0, 1).expect_err("closed"),
        ] {
            assert_eq!(code(&e), Some("closed"), "{e}");
        }
    }
}
