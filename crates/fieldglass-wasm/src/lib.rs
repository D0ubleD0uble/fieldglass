#![forbid(unsafe_code)]
//! `fieldglass-wasm` — a synchronous browser façade over the Fieldglass
//! decoders (#460).
//!
//! Four things live here and nothing else (ADR-0006): the `Uint8Array` /
//! `Float32Array` handoff, the error mapping, method forwarding, and the GLSL
//! snippet a GPU host pastes into its own shader. Every decision about what a
//! field *is* was made in `fieldglass`.
//!
//! With the `fetchplan` feature the fetch planner is forwarded the same way
//! (#1081): `parseWgrib2Idx` and `parseEcmwfIndex` read a sidecar into a
//! `MessageManifest`, whose `select` turns a query into plan items, and
//! `httpRangeHeader` and `verifyEnvelope` are the two steps either side of the
//! Worker's `fetch`. `candidates` says which runs of a source should have
//! posted by now. The feature is off by default: the package's default entry
//! leaves it out, and its `@fieldglass/wasm/full` entry is built with it and
//! `zarr` (#1158, see `Cargo.toml`).
//!
//! With the `zarr` feature, `openStore` opens a Zarr store from the objects a
//! page fetched, and a handle over one says which objects a view needs:
//! `coordinateChunks` for the coordinate arrays, then `planWindow` for the
//! chunks a lat/lon box and a time step touch, which `decodeWindow` decodes
//! from (#1083). Off by default too, and in the `full` entry beside the planner.
//!
//! # The memory contract
//!
//! Linear memory never shrinks, and an animation holds many fields at once, so
//! **the façade keeps no decode cache**. `Handle.decode` hands a `WasmField`
//! to JS and JS owns it; `warp`, `render`, `probe`, and `contours` take it back
//! by reference. `Handle.decodeDisplay` hands over a `WasmDisplayField`
//! the same way. Call `field.close()` (or `free()`) when you are done with one,
//! or hold it with `using` — a dropped JS reference does not release the wasm
//! allocation until the host's `FinalizationRegistry` runs, if it runs at all.
//!
//! Every accessor that returns a typed array **copies** out of linear memory,
//! because a view into it dangles the moment wasm grows the heap. A host that
//! wants to reuse a buffer copies into its own once and keeps that.
//!
//! # Values first, pixels second
//!
//! `Handle.render` is a CPU fallback. The intended path is
//! `Handle.palette` plus `glsl_snippet`: colour is decided once, in Rust,
//! and exported as a 256-entry lookup table, so restyling never re-decodes and
//! the CPU painter stays the oracle the shader is checked against rather than a
//! second colour implementation.
//!
//! # Absent values are `null`
//!
//! Every object this façade returns carries all of its keys, and a field with
//! nothing to report is `null` — never missing and never `undefined` (#574,
//! decided 2026-09-29, the same wire form the VS Code addon hands over). The
//! declarations for those objects are generated from the API's JSON schema into
//! `src/api.generated.d.ts` and embedded in this package's `.d.ts`, so a
//! `MessageInfo` field reads `T | null` there and a `=== null` guard is the
//! right one.
//!
//! # Panics
//!
//! Built with `panic = "abort"`: a decoder panic kills the Worker. The fuzz
//! targets make that rare; treat the Worker as disposable and restart it.

use fieldglass::{
    ContourOptions, DecodeOptions, DisplayDecodeOptions, DisplayField, Field, Isoline, LineRequest,
    PaletteOptions, Session, SliceRequest, Stats, WarpOptions,
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[cfg(feature = "fetchplan")]
mod fetchplan;

#[cfg(feature = "fetchplan")]
pub use fetchplan::{
    MessageManifest, candidates, http_range_header, parse_ecmwf_index, parse_wgrib2_idx,
    validate_source, verify_envelope, verify_message,
};

#[cfg(feature = "zarr")]
mod zarr;

#[cfg(feature = "zarr")]
pub use zarr::open_store;

/// The API's wire types as TypeScript, generated from
/// `crates/fieldglass/schema/api.schema.json` by `tools/gen_api_declarations.py`
/// and checked against it in CI. wasm-bindgen copies this into the package's
/// `.d.ts`, which is what lets the methods below name `MessageInfo` rather than
/// `any`.
///
/// Every build embeds this file, and `RENDERED_RASTER` below. The planner's and
/// Zarr's types are in the two feature-gated sections after that,
/// which only a build with that feature embeds, so the default package's
/// `.d.ts` declares nothing it does not contain (#1158).
#[wasm_bindgen(typescript_custom_section)]
const API_DECLARATIONS: &str = include_str!("api.generated.d.ts");

/// What `render` and `renderDisplay` return (#1156): the API's `Raster`, with
/// the bytes as a `Uint8Array`. Written here rather than generated, because
/// the schema's `rgba` is the JSON array the wire spells it as.
#[wasm_bindgen(typescript_custom_section)]
const RENDERED_RASTER: &str = r#"
/** A painted raster: `Raster` with `rgba` as a `Uint8Array`. */
export interface RenderedRaster {
  /** Non-premultiplied RGBA, `width * height * 4` bytes, row-major from the top-left. */
  rgba: Uint8Array;
  /** Raster columns: the field's `ni`. */
  width: number;
  /** Raster rows: the field's `nj`. */
  height: number;
  /** The range the cells were coloured over, and how many fell outside it. */
  range: ColourRange;
}
"#;

/// The fetch planner's wire types (`PlanItem`, `Query`, `SourceSpec` and the
/// rest), split out of `api.generated.d.ts` by the same generator.
#[cfg(feature = "fetchplan")]
#[wasm_bindgen(typescript_custom_section)]
const FETCHPLAN_DECLARATIONS: &str = include_str!("api.fetchplan.generated.d.ts");

/// The Zarr window planner's wire types (`WindowRequest`, `WindowPlan`,
/// `ChunkRef`), split out the same way.
#[cfg(feature = "zarr")]
#[wasm_bindgen(typescript_custom_section)]
const ZARR_DECLARATIONS: &str = include_str!("api.zarr.generated.d.ts");

/// The shader snippet a GPU host pastes into its own fragment program.
///
/// One string, exported rather than described, so an app composes it and never
/// rewrites it. It lives in `fieldglass::shader` beside the function that
/// prepares the texture it reads, because the two only make sense together.
#[wasm_bindgen(js_name = glslSnippet)]
pub fn glsl_snippet() -> String {
    fieldglass::GLSL.to_string()
}

/// Map an API error onto a JS `Error` carrying the stable `code` as a property,
/// so a host branches on `e.code` and shows `e.message`.
///
/// A refused colour range also carries `rangeRefusal`, the API's
/// `RangeRefusal` (which end, the value given, the field's own range, and a
/// stable reason), so a page can point at the control that is wrong (#1155).
/// It is `null` on every other error.
fn throw(e: fieldglass::Error) -> JsValue {
    let err = js_sys::Error::new(&e.message());
    // A failed property set would mean a frozen `Error.prototype`; the message
    // is still correct without the code, so it is not worth failing the call.
    let _ = js_sys::Reflect::set(
        &err,
        &JsValue::from_str("code"),
        &JsValue::from_str(e.code()),
    );
    let refusal = e
        .range_refusal()
        .and_then(|r| to_js(r).ok())
        .unwrap_or(JsValue::NULL);
    let _ = js_sys::Reflect::set(&err, &JsValue::from_str("rangeRefusal"), &refusal);
    err.into()
}

/// How every returned object is serialised: a Rust `None` becomes `null`, with
/// its key kept.
///
/// serde-wasm-bindgen's default writes `undefined` instead, which is a third
/// spelling beside napi's and the recorded suite's, and the one a strict
/// `=== null` guard lets through (#288, #574).
const WIRE: serde_wasm_bindgen::Serializer =
    serde_wasm_bindgen::Serializer::new().serialize_missing_as_null(true);

/// A string the API may not have, as JavaScript reads it: the text, or `null`.
///
/// Not `Option<String>` as a return type, which `wasm-bindgen` crosses as
/// `undefined` — the #288 shape the wire contract rules out (#574).
fn nullable_text(text: Option<&str>) -> JsValue {
    text.map_or(JsValue::NULL, JsValue::from_str)
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    value
        .serialize(&WIRE)
        .map_err(|e| js_sys::Error::new(&e.to_string()).into())
}

fn from_js<T: serde::de::DeserializeOwned + Default>(value: JsValue) -> Result<T, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(T::default());
    }
    // `invalid_option`, with its `code`, as every other refused argument is and
    // as the Node addon refuses the same object (#1073).
    serde_wasm_bindgen::from_value(value).map_err(|e| unreadable("options object", &e))
}

/// Read an argument whose type refuses unknown keys, so that it does.
///
/// serde-wasm-bindgen reads a struct by asking the object for each field the
/// struct names, so a key it does not name is never looked at and
/// `deny_unknown_fields` cannot fire: `{ levelTxt: "2 mb" }` would read as an
/// empty query and select every record. Reading the object into a JSON value
/// first enumerates every key it has, and the type's own rule then applies.
///
/// Shared by the planner's arguments and the Zarr window's (#1081, #1083).
#[cfg(any(feature = "fetchplan", feature = "zarr"))]
fn strict<T: serde::de::DeserializeOwned>(value: JsValue, what: &str) -> Result<T, JsValue> {
    let json: serde_json::Value =
        serde_wasm_bindgen::from_value(value).map_err(|e| unreadable(what, &e))?;
    serde_json::from_value(json).map_err(|e| unreadable(what, &e))
}

/// Read a slice or line request (#1073), refusing a key it does not take.
///
/// The refusal [`strict`] makes, reached another way, for two reasons: the
/// default entry links no `serde_json`, and a JSON value cannot hold the
/// `Uint32Array` a page may pass as `sliceIndices`. So JavaScript lists the
/// object's own keys, each is checked against the ones the request reads, and
/// serde-wasm-bindgen then reads the object, typed arrays included, and refuses
/// one missing a field.
fn request<T: serde::de::DeserializeOwned + Fields>(
    value: JsValue,
    what: &str,
) -> Result<T, JsValue> {
    if !value.is_object() {
        return Err(unreadable(what, &"it is not an object"));
    }
    let object: &js_sys::Object = value.unchecked_ref();
    let keys = js_sys::Object::keys(object);
    for key in keys.iter() {
        let key = key.as_string().unwrap_or_default();
        if !T::FIELDS.contains(&key.as_str()) {
            return Err(unreadable(
                what,
                &format!(
                    "unknown field `{key}`, expected one of {}",
                    T::FIELDS.join(", ")
                ),
            ));
        }
    }
    serde_wasm_bindgen::from_value(value).map_err(|e| unreadable(what, &e))
}

/// The keys a request object may hold, for [`request`]. Spelt as the wire
/// spells them: a key missing here would refuse every request that names it,
/// which `tests/node/netcdf.mjs` and the conformance runner would both see.
trait Fields {
    /// Every key the type reads.
    const FIELDS: &'static [&'static str];
}

impl Fields for SliceRequest {
    const FIELDS: &'static [&'static str] = &["variable", "yDim", "xDim", "sliceIndices"];
}

impl Fields for LineRequest {
    const FIELDS: &'static [&'static str] = &["variable", "alongDim", "sliceIndices"];
}

/// An argument that did not read is the caller's to fix, so it is
/// `invalid_option`, with serde's own message naming the key.
fn unreadable(what: &str, e: &dyn std::fmt::Display) -> JsValue {
    throw(fieldglass::Error::invalid_option(format!(
        "could not read the {what}: {e}"
    )))
}

/// The refusal every call on a closed object gives (#1073).
fn closed(what: &str) -> JsValue {
    js_sys::Error::new(&format!("this {what} was closed; open or decode it again")).into()
}

/// Every field-combine operation, in menu order: `[{ value, label }, …]`.
///
/// What a Compare picker is built from. The list is
/// `fieldglass::combine_ops`'s, so this host and the VS Code one offer the
/// same operations under the same tags (#342) — and `value` is exactly what
/// `Handle.combine` takes back.
#[wasm_bindgen(js_name = combineOps, unchecked_return_type = "CombineOpInfo[]")]
pub fn combine_ops() -> Result<JsValue, JsValue> {
    to_js(&fieldglass::combine_ops())
}

/// An open file. Holds the bytes and the parsed message index, nothing else.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Handle {
    /// `None` once closed.
    session: Option<Session>,
}

impl Handle {
    /// The session, or the refusal a closed handle gives.
    fn session(&self) -> Result<&Session, JsValue> {
        self.session.as_ref().ok_or_else(|| closed("handle"))
    }
}

/// Open a container from its bytes. The format is detected from the bytes.
///
/// The buffer is copied into linear memory; the caller may reuse or free its
/// own immediately.
#[wasm_bindgen]
pub fn open(bytes: &[u8]) -> Result<Handle, JsValue> {
    let session = Session::open(bytes.to_vec()).map_err(throw)?;
    Ok(Handle {
        session: Some(session),
    })
}

#[wasm_bindgen]
impl Handle {
    /// Release the file and its parsed index now: the same as `free()`, and as `[Symbol.dispose]`, so
    /// `using` releases it at the end of its block (#1073). Any call on it
    /// afterwards throws, except `close()` itself, which is harmless twice.
    pub fn close(&mut self) {
        self.session = None;
    }

    /// `"grib1"`, `"grib2"`, `"netcdf"` or, from `openStore`, `"zarr"`.
    #[wasm_bindgen(unchecked_return_type = "SourceFormat")]
    pub fn format(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session()?.format())
    }

    /// Which of the two ways this container is addressed: `"messages"` or
    /// `"variables"`.
    ///
    /// **Ask this first.** GRIB is a stream of self-describing messages, so an
    /// index is the whole address and `Handle.count`, `Handle.message`
    /// and `Handle.decode` are the calls. NetCDF holds named variables over
    /// shared dimensions, so a field is a variable, two of its axes, and where
    /// you are standing on the rest — `Handle.variables`,
    /// `Handle.dimensions` and `decodeSlice`. Asking one mode's
    /// question of the other throws `wrong_addressing` naming the call to make
    /// instead, rather than an index error that would blame the number when the
    /// question was wrong.
    #[wasm_bindgen(unchecked_return_type = "Addressing")]
    pub fn addressing(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session()?.addressing())
    }

    /// How many messages the file holds. Zero for a variable container.
    pub fn count(&self) -> Result<u32, JsValue> {
        Ok(self.session()?.count())
    }

    /// The dataset's shared dimensions, in the order the file declares them.
    ///
    /// Shared is the point: two variables naming the same dimension are on the
    /// same axis, so a page offers one time slider for a file rather than one
    /// per variable. Empty for a message container.
    #[wasm_bindgen(unchecked_return_type = "DimensionInfo[]")]
    pub fn dimensions(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session()?.dimensions())
    }

    /// The variables a caller can decode a slice of.
    ///
    /// Renderable ones only — a variable of fewer than two dimensions has no
    /// raster to put on a map. Each carries the `index` `decodeSlice`
    /// takes. Empty for a message container.
    #[wasm_bindgen(unchecked_return_type = "VariableInfo[]")]
    pub fn variables(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session()?.variables())
    }

    /// The arrays the file holds and this build will not read, each with the
    /// reader's reason and a `cause` code to word it by (#709, #1129). Empty
    /// for a message container.
    #[wasm_bindgen(js_name = leftOut, unchecked_return_type = "LeftOutArray[]")]
    pub fn left_out(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session()?.left_out())
    }

    /// The arrays the file reads that no map can be drawn from, each with why
    /// (#1129): text, a coordinate, fewer than two dimensions. With
    /// `Handle.leftOut` and `Handle.variables`, every array once.
    #[wasm_bindgen(unchecked_return_type = "LeftOutArray[]")]
    pub fn undrawable(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session()?.undrawable())
    }

    /// Decode one 2-D slice of a variable into the same field
    /// `Handle.decode` returns.
    ///
    /// `slice` is `{ variable, yDim, xDim, sliceIndices }`: the `index` a
    /// `variables()` entry carries, two positions in that variable's own
    /// `dims` for the image axes, and **one entry per dimension** in
    /// `sliceIndices`, in the same declared order — so `sliceIndices[d]` is
    /// always the position on `dims[d]`, and the two image entries are ignored
    /// rather than absent. A wrong length throws `invalid_option` rather than
    /// being guessed: silently defaulting the unstated axes to zero is how a
    /// viewer shows the first time step and labels it the last. So does a key
    /// the object does not take, such as a misspelt `sliceIndex`.
    ///
    /// The field that comes back is not special. `render`, `probe`, `contours`,
    /// `combine`, `warp` and `palette` take it exactly as they take a decoded
    /// message, which is why the addressing split stops here.
    #[wasm_bindgen(js_name = decodeSlice)]
    pub fn decode_slice(
        &self,
        #[wasm_bindgen(unchecked_param_type = "SliceRequest")] slice: JsValue,
        #[wasm_bindgen(unchecked_optional_param_type = "DecodeOptions | null")] options: JsValue,
    ) -> Result<WasmField, JsValue> {
        let slice: SliceRequest = request(slice, "slice")?;
        let options: DecodeOptions = from_js(options)?;
        let field = self
            .session()?
            .decode_slice(
                slice.variable,
                slice.y_dim,
                slice.x_dim,
                &slice.slice_indices,
                &options,
            )
            .map_err(throw)?;
        Ok(WasmField { field: Some(field) })
    }

    /// One line through a variable: its values along `line.alongDim`, every
    /// other axis held at `line.sliceIndices` (#172). `options` is `{ dtype?:
    /// "auto" | "f32" | "f64" }`.
    ///
    /// The profile or time series a viewer plots beside a map when a user clicks
    /// a cell. `line` is `{ variable, alongDim, sliceIndices }`, and
    /// `sliceIndices` names a position on every axis, the one being read along
    /// included and ignored — so a page passes the vector it already holds for
    /// the slice on screen, with the clicked cell written into its two
    /// horizontal positions.
    ///
    /// Returned as a plain object rather than a class with typed-array accessors,
    /// as `decodeSlice`'s field is: a line is one axis long, so there is no large
    /// buffer to hand over without copying. `values` is `{ dtype, data }`, read
    /// `mask` before a value, and `coordinates` is absent when the axis has none.
    ///
    /// Throws `wrong_addressing` for a message stream, `no_such_message` for a
    /// variable past the list, and `invalid_option` for an axis the variable does
    /// not have, a `sliceIndices` of the wrong length, an index past its axis, or
    /// a key the object does not take.
    #[wasm_bindgen(js_name = decodeLine, unchecked_return_type = "Line")]
    pub fn decode_line(
        &self,
        #[wasm_bindgen(unchecked_param_type = "LineRequest")] line: JsValue,
        #[wasm_bindgen(unchecked_optional_param_type = "DecodeOptions | null")] options: JsValue,
    ) -> Result<JsValue, JsValue> {
        let line: LineRequest = request(line, "line")?;
        let options: DecodeOptions = from_js(options)?;
        let line = self
            .session()?
            .decode_line(line.variable, line.along_dim, &line.slice_indices, &options)
            .map_err(throw)?;
        to_js(&line)
    }

    /// One message's metadata, built on demand.
    ///
    /// Lazy on purpose: a thousand-message file should not serialise a thousand
    /// of these to open. Ask for the ones you are going to show.
    #[wasm_bindgen(unchecked_return_type = "MessageInfo")]
    pub fn message(&self, index: u32) -> Result<JsValue, JsValue> {
        let info = self.session()?.message(index).map_err(throw)?;
        to_js(&info)
    }

    /// Whether `u` and `v` are the two components of one vector, by the
    /// parameter each names (#1049): `"recognised"`, `"swapped"` (a pair in
    /// the wrong order), `"not_a_pair"`, or `"unknown"` when a field has no
    /// name to read, such as a GRIB parameter no table names. The indices are
    /// messages for GRIB and the `variables()` list for NetCDF and Zarr, whose
    /// CF `standard_name`s are compared. Not a refusal: a page drawing arrows
    /// from a pair that is not one says so beside them.
    #[wasm_bindgen(js_name = vectorPairing, unchecked_return_type = "VectorPairing")]
    pub fn vector_pairing(&self, u: u32, v: u32) -> Result<JsValue, JsValue> {
        let pairing = self.session()?.vector_pairing(u, v).map_err(throw)?;
        to_js(&pairing)
    }

    /// Decode one message. `options` is `{ dtype?: "auto" | "f32" | "f64" }`.
    ///
    /// The returned field is yours; free it when you are done. For a map drawn
    /// coarser than the file, see `Handle.decodeDisplay`.
    pub fn decode(
        &self,
        index: u32,
        #[wasm_bindgen(unchecked_optional_param_type = "DecodeOptions | null")] options: JsValue,
    ) -> Result<WasmField, JsValue> {
        let options: DecodeOptions = from_js(options)?;
        let field = self.session()?.decode(index, &options).map_err(throw)?;
        Ok(WasmField { field: Some(field) })
    }

    /// Decode one message to **draw**, at a reduced resolution: `options` is
    /// `{ reduce?: number, dtype?: "auto" | "f32" | "f64" }` (#1082).
    ///
    /// `reduce` discards that many of a JPEG 2000 message's finest wavelet
    /// levels, so `1` decodes a field half the size on each axis in about a
    /// quarter of the time. The values are then a low-pass the file does not
    /// hold, on every `2^reduce`-th point of its grid, so this returns a
    /// `WasmDisplayField` and not a `WasmField`: `renderDisplay`,
    /// `warpDisplay` and `paletteDisplay` take it, and `probe`, `contours`,
    /// `combine` and the shader accessors do not, which TypeScript checks. Its
    /// `grid()` is the derived grid. For the numbers, call `decode`.
    ///
    /// `reduce: 0` (the default) is `decode`'s field, for any message. Above
    /// zero, a message with no coarse field (GRIB1, a packing other than JPEG
    /// 2000, a bitmap, a reduced grid, or a grid family with no coarse grid)
    /// throws `unsupported`; ask again at `0`. Free the field when you are done.
    #[wasm_bindgen(js_name = decodeDisplay)]
    pub fn decode_display(
        &self,
        index: u32,
        #[wasm_bindgen(unchecked_optional_param_type = "DisplayDecodeOptions | null")]
        options: JsValue,
    ) -> Result<WasmDisplayField, JsValue> {
        let options: DisplayDecodeOptions = from_js(options)?;
        let field = self
            .session()?
            .decode_display(index, &options)
            .map_err(throw)?;
        Ok(WasmDisplayField { field: Some(field) })
    }

    /// Resample a field onto a geographic box without painting it.
    ///
    /// `options` is `{ bilinear?: boolean, bounds?: [latMin, latMax, lonMin,
    /// lonMax], width?: number, height?: number }` — a window at a pixel size,
    /// which is what a map view asks for (#465). Send `width` and `height`
    /// together or not at all; one alone throws `invalid_option`, as does a
    /// zero. With neither, the output is the source `ni × nj`, as before. A
    /// grid nothing places throws `unsupported`, with `bounds` or without, as
    /// the render path refuses it (#843).
    #[wasm_bindgen(
        unchecked_return_type = "{ values: Float32Array; mask: Uint8Array; width: number; height: number; bounds: [number, number, number, number] }"
    )]
    pub fn warp(
        &self,
        field: &WasmField,
        #[wasm_bindgen(unchecked_optional_param_type = "WarpOptions | null")] options: JsValue,
    ) -> Result<JsValue, JsValue> {
        let options: WarpOptions = from_js(options)?;
        let out = self
            .session()?
            .warp(field.field()?, &options)
            .map_err(throw)?;
        warped_object(&out)
    }

    /// `Handle.warp` for a field from `Handle.decodeDisplay`, on the grid
    /// it was decoded onto: the same options and the same object back. The
    /// values are display values, as a warp's always are.
    #[wasm_bindgen(
        js_name = warpDisplay,
        unchecked_return_type = "{ values: Float32Array; mask: Uint8Array; width: number; height: number; bounds: [number, number, number, number] }"
    )]
    pub fn warp_display(
        &self,
        field: &WasmDisplayField,
        #[wasm_bindgen(unchecked_optional_param_type = "WarpOptions | null")] options: JsValue,
    ) -> Result<JsValue, JsValue> {
        let options: WarpOptions = from_js(options)?;
        let out = self
            .session()?
            .warp_display(field.field()?, &options)
            .map_err(throw)?;
        warped_object(&out)
    }

    /// The colour decision as data: `{ lut, t0, t1, span, scale, maskedRgba,
    /// range }`.
    ///
    /// `lut` is 256 RGBA entries with any reversal already applied — the CPU
    /// painter's own table. Upload it as a 256 × 1 `RGBA8` texture sampled
    /// `NEAREST` and pair it with `glsl_snippet`.
    ///
    /// The domain is chosen by the rule both hosts follow (#1155): a `min` or
    /// `max` left out comes from the field's present cells. A range that does
    /// not hold — inverted, zero-width, not finite, or a log10 minimum at or
    /// below zero — throws `invalid_option` with `e.rangeRefusal` set. A field
    /// with no present cell and no manual range is not refused: every cell is
    /// masked, and the domain is a placeholder no cell reads.
    ///
    /// `range` is the `ColourRange` the palette spans, the one `Handle.render`
    /// reports for the same field and options, counts included (#1156): its two
    /// ends in data units (unlogged under log10, where `t0`/`t1` are logged),
    /// which ends were given, the center, and how many present cells fall
    /// below and above it and take an end's colour. Label a legend from it.
    /// Counting costs one pass over the field.
    #[wasm_bindgen(
        unchecked_return_type = "{ lut: Uint8Array; t0: number; t1: number; span: number; scale: \"linear\" | \"log10\"; maskedRgba: Uint8Array; range: ColourRange }"
    )]
    pub fn palette(
        &self,
        field: &WasmField,
        #[wasm_bindgen(unchecked_optional_param_type = "PaletteOptions | null")] options: JsValue,
    ) -> Result<JsValue, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let palette = self
            .session()?
            .palette(field.field()?, &options)
            .map_err(throw)?;
        let range = self
            .session()?
            .palette_range(field.field()?, &options)
            .map_err(throw)?;
        palette_object(&palette, &range)
    }

    /// `Handle.palette` for a field from `Handle.decodeDisplay`, with
    /// its automatic range taken from `fullStats`: the `stats()` of the same
    /// message from `decode`, held from an earlier full decode (#1082).
    ///
    /// Never from the display field's own values, which a wavelet low-pass
    /// moves: a map drawn at any reduction colours a value as the full map
    /// does. `fullStats` may be `null` when `min` and `max` are both given.
    /// Throws `invalid_option` when an end is left to the data and `fullStats`
    /// is `null`, states no range over a field with present cells, or is not a
    /// range. `range` is the one `Handle.renderDisplay` reports, with the
    /// display field's cells counted.
    #[wasm_bindgen(
        js_name = paletteDisplay,
        unchecked_return_type = "{ lut: Uint8Array; t0: number; t1: number; span: number; scale: \"linear\" | \"log10\"; maskedRgba: Uint8Array; range: ColourRange }"
    )]
    pub fn palette_display(
        &self,
        field: &WasmDisplayField,
        #[wasm_bindgen(js_name = "fullStats", unchecked_param_type = "Stats | null")]
        full_stats: JsValue,
        #[wasm_bindgen(unchecked_optional_param_type = "PaletteOptions | null")] options: JsValue,
    ) -> Result<JsValue, JsValue> {
        let full = full_stats_from_js(full_stats)?;
        let options: PaletteOptions = from_js(options)?;
        let palette = self
            .session()?
            .palette_display(field.field()?, full.as_ref(), &options)
            .map_err(throw)?;
        let range = self
            .session()?
            .palette_display_range(field.field()?, full.as_ref(), &options)
            .map_err(throw)?;
        palette_object(&palette, &range)
    }

    /// Paint a field to RGBA on the CPU — the fallback, and the oracle the
    /// shader path is checked against.
    ///
    /// `flipY` composes with the message's own scan order rather than replacing
    /// it, so `false` means **north up** and not "rows as stored": a grid
    /// scanning south-to-north is flipped for you. Pass the user's own request
    /// straight through; composing `grid().scan.jPositive` yourself would flip
    /// twice.
    ///
    /// Returns `{ rgba, width, height, range }`: the RGBA bytes as a
    /// `Uint8Array`, the field's `ni × nj`, and the `ColourRange` the cells
    /// were coloured over, with how many fell below and above it (#1156). The
    /// Node addon's render reports the same ends and range source for the same
    /// field and options. Its counts match only in the source projection: the
    /// addon counts the pixels of the raster it warped to, and this counts the
    /// field's cells.
    ///
    /// The range follows `Handle.palette`'s rule, and a field with no
    /// present cell paints fully transparent rather than throwing, with a
    /// `null` `usedMin` and `usedMax`.
    #[wasm_bindgen(js_name = render, unchecked_return_type = "RenderedRaster")]
    pub fn render(
        &self,
        field: &WasmField,
        #[wasm_bindgen(unchecked_param_type = "PaletteOptions | null | undefined")]
        options: JsValue,
        #[wasm_bindgen(js_name = "flipY")] flip_y: bool,
    ) -> Result<JsValue, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let raster = self
            .session()?
            .render(field.field()?, &options, flip_y)
            .map_err(throw)?;
        raster_object(&raster)
    }

    /// `Handle.render` for a field from `Handle.decodeDisplay`: its own
    /// `ni × nj` raster, coloured over the range `Handle.paletteDisplay`
    /// takes from `fullStats`, with `flipY` as `render` reads it. The same
    /// `{ rgba, width, height, range }` back, the range counting the display
    /// field's cells.
    #[wasm_bindgen(js_name = renderDisplay, unchecked_return_type = "RenderedRaster")]
    pub fn render_display(
        &self,
        field: &WasmDisplayField,
        #[wasm_bindgen(js_name = "fullStats", unchecked_param_type = "Stats | null")]
        full_stats: JsValue,
        #[wasm_bindgen(unchecked_param_type = "PaletteOptions | null | undefined")]
        options: JsValue,
        #[wasm_bindgen(js_name = "flipY")] flip_y: bool,
    ) -> Result<JsValue, JsValue> {
        let full = full_stats_from_js(full_stats)?;
        let options: PaletteOptions = from_js(options)?;
        let raster = self
            .session()?
            .render_display(field.field()?, full.as_ref(), &options, flip_y)
            .map_err(throw)?;
        raster_object(&raster)
    }

    /// Sample one geographic point. `null` when the point is off the grid or
    /// the family cannot place it — `null` rather than `undefined` for the
    /// reason every absent value on this surface is (#574).
    #[wasm_bindgen(unchecked_return_type = "Probe | null")]
    pub fn probe(&self, field: &WasmField, lat: f64, lon: f64) -> Result<JsValue, JsValue> {
        match self.session()?.probe(field.field()?, lat, lon) {
            Some(p) => to_js(&p),
            None => Ok(JsValue::NULL),
        }
    }

    /// Probe message `index` at one geographic point: `{ lat, lon, i, j, value,
    /// fullDetail }`, or `null` off the grid (#637).
    ///
    /// `value` is what the decoded field shows at the cell, as `Handle.probe`
    /// reads it. `fullDetail` is `null` except for a spectral message whose field
    /// reports a `truncation()`: its values are the field band-limited to what
    /// the 0.5° grid carries, and `fullDetail.value` is the full sum over every
    /// wavenumber the file holds at the same cell node, with
    /// `fullDetail.truncation` saying which two truncations the pair is. Show
    /// both. This decodes the message on every call; a caller holding the field
    /// should use `probe` and `probeFullDetail` instead.
    #[wasm_bindgen(js_name = probeMessage, unchecked_return_type = "MessageProbe | null")]
    pub fn probe_message(&self, index: u32, lat: f64, lon: f64) -> Result<JsValue, JsValue> {
        match self
            .session()?
            .probe_message(index, lat, lon)
            .map_err(throw)?
        {
            Some(p) => to_js(&p),
            None => Ok(JsValue::NULL),
        }
    }

    /// The full-detail value of message `index` at the cell a probe at
    /// `(lat, lon)` lands on: `{ value, truncation }`, or `null` when the
    /// message's decoded field already holds the file's values (every message
    /// but a band-limited spectral one) or the point is off the grid (#637).
    /// Decodes the coefficients each call: about a second at T7999.
    #[wasm_bindgen(js_name = probeFullDetail, unchecked_return_type = "FullDetail | null")]
    pub fn probe_full_detail(&self, index: u32, lat: f64, lon: f64) -> Result<JsValue, JsValue> {
        match self
            .session()?
            .probe_full_detail(index, lat, lon)
            .map_err(throw)?
        {
            Some(d) => to_js(&d),
            None => Ok(JsValue::NULL),
        }
    }

    /// Combine two aligned fields element by element — the difference map and
    /// its siblings (#579).
    ///
    /// `op` is one of the `value` tags `combine_ops` reports. The result is a
    /// field like any other, on **A's** placement, so `warp`, `palette`,
    /// `render`, `probe` and `contours` all take it; it is yours, so free it.
    ///
    /// Throws `invalid_option` for an op this build does not know, and
    /// `unsupported` when the two fields do not align cell for cell — the
    /// message names which property differs.
    pub fn combine(&self, a: &WasmField, b: &WasmField, op: &str) -> Result<WasmField, JsValue> {
        let op = fieldglass::op_from_wire(op).map_err(throw)?;
        let field = self
            .session()?
            .combine(a.field()?, b.field()?, op)
            .map_err(throw)?;
        Ok(WasmField { field: Some(field) })
    }

    /// Isolines in fractional grid coordinates. An empty `levels` asks for a
    /// nice set spanning the colour range `options` chooses: `{ min?, max?,
    /// center?, scale? }`, the range half of the options `render` takes, so
    /// pass the same values and the isolines follow the colours (#1155). A
    /// range `render` would refuse throws the same `invalid_option`, with
    /// `e.rangeRefusal`.
    #[wasm_bindgen(unchecked_return_type = "Isoline[]")]
    pub fn contours(
        &self,
        field: &WasmField,
        levels: &[f64],
        #[wasm_bindgen(unchecked_optional_param_type = "ContourOptions | null")] options: JsValue,
    ) -> Result<JsValue, JsValue> {
        let options: ContourOptions = from_js(options)?;
        let out: Vec<Isoline> = self
            .session()?
            .contours(field.field()?, levels, &options)
            .map_err(throw)?;
        to_js(&out)
    }

    /// The field as an `R32F` texture wants it: transformed by the palette's
    /// scale and **rebased by `t0`**, with the subtraction done in `f64`.
    ///
    /// Pass the same `options` you passed to `Handle.palette` — the palette
    /// is 256 entries and rebuilding it costs nothing next to the field.
    /// A shader that subtracts `t0` itself, in `f32`, loses the domain rather
    /// than the result; see `fieldglass::shader`.
    #[wasm_bindgen(js_name = shaderValues)]
    pub fn shader_values(
        &self,
        field: &WasmField,
        #[wasm_bindgen(unchecked_optional_param_type = "PaletteOptions | null")] options: JsValue,
    ) -> Result<js_sys::Float32Array, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let palette = self
            .session()?
            .palette(field.field()?, &options)
            .map_err(throw)?;
        let values = fieldglass::shader_values(field.field()?, &palette);
        Ok(js_sys::Float32Array::from(&values[..]))
    }

    /// The mask an `R8` texture wants: the field's own mask with the cells the
    /// palette's scale excludes also cleared. Equals `field.mask()` under a
    /// linear scale; under `log10` it also drops the non-positive cells, which
    /// is what keeps the GPU and the CPU painter agreeing about them.
    #[wasm_bindgen(js_name = shaderMask)]
    pub fn shader_mask(
        &self,
        field: &WasmField,
        #[wasm_bindgen(unchecked_optional_param_type = "PaletteOptions | null")] options: JsValue,
    ) -> Result<js_sys::Uint8Array, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let palette = self
            .session()?
            .palette(field.field()?, &options)
            .map_err(throw)?;
        let mask = fieldglass::shader_mask(field.field()?, &palette);
        Ok(js_sys::Uint8Array::from(&mask[..]))
    }
}

fn set(object: &js_sys::Object, key: &str, value: impl Into<JsValue>) -> Result<(), JsValue> {
    js_sys::Reflect::set(object, &JsValue::from_str(key), &value.into())?;
    Ok(())
}

/// A warp's output as `warp` and `warpDisplay` hand it over, with typed arrays.
fn warped_object(out: &fieldglass::Warped) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    set(
        &object,
        "values",
        js_sys::Float32Array::from(&out.values[..]),
    )?;
    set(&object, "mask", js_sys::Uint8Array::from(&out.mask[..]))?;
    set(&object, "width", JsValue::from_f64(f64::from(out.width)))?;
    set(&object, "height", JsValue::from_f64(f64::from(out.height)))?;
    set(&object, "bounds", to_js(&out.bounds)?)?;
    Ok(object.into())
}

/// A painted raster as `render` and `renderDisplay` hand it over: the API's
/// `Raster`, with the bytes as a typed array rather than an array of numbers.
fn raster_object(raster: &fieldglass::Raster) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    set(&object, "rgba", js_sys::Uint8Array::from(&raster.rgba[..]))?;
    set(&object, "width", JsValue::from_f64(f64::from(raster.width)))?;
    set(
        &object,
        "height",
        JsValue::from_f64(f64::from(raster.height)),
    )?;
    set(&object, "range", to_js(&raster.range)?)?;
    Ok(object.into())
}

/// A palette as `palette` and `paletteDisplay` hand it over, with the range it
/// spans (#1156).
fn palette_object(
    palette: &fieldglass::Palette,
    range: &fieldglass::ColourRange,
) -> Result<JsValue, JsValue> {
    let object = js_sys::Object::new();
    set(&object, "lut", js_sys::Uint8Array::from(&palette.lut[..]))?;
    set(&object, "t0", JsValue::from_f64(palette.t0))?;
    set(&object, "t1", JsValue::from_f64(palette.t1))?;
    // The one number the shader divides by, precomputed here so a host
    // cannot subtract two large `f64`s in JS and hand the shader a value
    // that already lost its precision.
    set(&object, "span", JsValue::from_f64(palette.t1 - palette.t0))?;
    set(&object, "scale", JsValue::from_str(palette.scale.as_str()))?;
    set(
        &object,
        "maskedRgba",
        js_sys::Uint8Array::from(&palette.masked_rgba[..]),
    )?;
    set(&object, "range", to_js(range)?)?;
    Ok(object.into())
}

/// The full field's `Stats` a display drawing call takes, as a page passes it
/// back from `field.stats()`: `null` or `undefined` is none.
fn full_stats_from_js(value: JsValue) -> Result<Option<Stats>, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(None);
    }
    serde_wasm_bindgen::from_value(value)
        .map(Some)
        .map_err(|e| {
            throw(fieldglass::Error::invalid_option(format!(
                "could not read the full-field stats: {e}"
            )))
        })
}

/// One decoded field, owned by the caller.
#[wasm_bindgen]
#[derive(Debug)]
pub struct WasmField {
    /// `None` once closed.
    field: Option<Field>,
}

impl WasmField {
    /// The field, or the refusal a closed one gives.
    fn field(&self) -> Result<&Field, JsValue> {
        self.field.as_ref().ok_or_else(|| closed("field"))
    }
}

#[wasm_bindgen]
impl WasmField {
    /// Release the field's memory now: the same as `free()`, and as `[Symbol.dispose]`, so
    /// `using` releases it at the end of its block (#1073). Any call on it
    /// afterwards throws, except `close()` itself, which is harmless twice.
    pub fn close(&mut self) {
        self.field = None;
    }

    /// The values, as a `Float32Array` or a `Float64Array` depending on what
    /// the source supports — see the `dtype` accessor. A copy: the array does
    /// not alias linear memory.
    #[wasm_bindgen(unchecked_return_type = "Float32Array | Float64Array")]
    pub fn values(&self) -> Result<JsValue, JsValue> {
        let field = self.field()?;
        Ok(match field.values.as_f32() {
            Some(v) => js_sys::Float32Array::from(v).into(),
            // Every other width crosses as `Float64Array`: the accessor's job
            // is a lossless handoff, and `f64` is the only typed array that is
            // lossless for every width the API can hold.
            None => js_sys::Float64Array::from(&field.values.to_f64()[..]).into(),
        })
    }

    /// `"f32"` or `"f64"` — which typed array `WasmField.values` returned.
    #[wasm_bindgen(unchecked_return_type = "\"f32\" | \"f64\"")]
    pub fn dtype(&self) -> Result<String, JsValue> {
        Ok(if self.field()?.values.as_f32().is_some() {
            "f32".to_string()
        } else {
            "f64".to_string()
        })
    }

    /// One byte per cell: `1` present, `0` absent. A separate array rather than
    /// `NaN` in the values, because `isnan()` is unreliable on some mobile GPUs
    /// and a `NaN` poisons linear filtering in a texture.
    pub fn mask(&self) -> Result<js_sys::Uint8Array, JsValue> {
        Ok(js_sys::Uint8Array::from(&self.field()?.mask[..]))
    }

    /// Grid columns (west-to-east point count of one row).
    pub fn ni(&self) -> Result<u32, JsValue> {
        Ok(self.field()?.ni)
    }

    /// Grid rows. `values()` is `ni * nj` long, row-major.
    pub fn nj(&self) -> Result<u32, JsValue> {
        Ok(self.field()?.nj)
    }

    /// Where the field sits on the Earth: `kind`, `boundsLonlat`, `proj4`,
    /// `affine`, `periodicX`, `scan`.
    #[wasm_bindgen(unchecked_return_type = "Georef")]
    pub fn grid(&self) -> Result<JsValue, JsValue> {
        to_js(&self.field()?.georef)
    }

    /// `{ min, max, validCount }` over the present cells.
    #[wasm_bindgen(unchecked_return_type = "Stats")]
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        to_js(&self.field()?.stats)
    }

    /// The parameter's name, e.g. `"Temperature"`. When no table in the crate
    /// resolves the message's parameter codes, this is `Parameter <codes>`
    /// naming the codes that went unresolved — `Parameter 209/10/0` for GRIB2,
    /// `Parameter 98/128/210` for GRIB1. See `fieldglass::api::Field` for the
    /// contract in full (#633). `null` for a GRIB2 product template that
    /// carries no parameter codes (#775).
    #[wasm_bindgen(unchecked_return_type = "string | null")]
    pub fn parameter(&self) -> Result<JsValue, JsValue> {
        Ok(nullable_text(self.field()?.parameter.as_deref()))
    }

    /// The parameter's units as the originating table states them, e.g.
    /// `"K"`. `null` when the parameter did not resolve, the table states no
    /// units for it, or the array's attributes state none (#775).
    #[wasm_bindgen(unchecked_return_type = "string | null")]
    pub fn units(&self) -> Result<JsValue, JsValue> {
        Ok(nullable_text(self.field()?.units.as_deref()))
    }

    /// `{ declared, truncatedTo }` when the values are a spectral field
    /// band-limited to what the grid carries, below the truncation the message
    /// declares (#637); `null` otherwise. Show it beside the field —
    /// "shown at T359 of T7999" — since the map is smoother than the file, and
    /// read a point's full-detail value with `Handle.probeFullDetail`.
    #[wasm_bindgen(unchecked_return_type = "SpectralTruncation | null")]
    pub fn truncation(&self) -> Result<JsValue, JsValue> {
        match &self.field()?.truncation {
            Some(t) => to_js(t),
            None => Ok(JsValue::NULL),
        }
    }
}

/// A field decoded to draw, from `Handle.decodeDisplay`, owned by the
/// caller (#1082).
///
/// Deliberately not a `WasmField`: above reduction zero its values are a
/// wavelet low-pass on a derived grid, so it has no `values()`, `stats()` or
/// `mask()` to measure with, and the calls that measure (`probe`, `contours`,
/// `combine`, the shader accessors) take a `WasmField` only. Its `reduction()`
/// is what a `WasmField` lacks, so neither class is assignable to the other in
/// TypeScript. Free it when you are done.
#[wasm_bindgen]
#[derive(Debug)]
pub struct WasmDisplayField {
    /// `None` once closed.
    field: Option<DisplayField>,
}

impl WasmDisplayField {
    /// The field, or the refusal a closed one gives.
    fn field(&self) -> Result<&DisplayField, JsValue> {
        self.field.as_ref().ok_or_else(|| closed("display field"))
    }
}

#[wasm_bindgen]
impl WasmDisplayField {
    /// Release the field's memory now: the same as `free()`, and as `[Symbol.dispose]`, so
    /// `using` releases it at the end of its block (#1073). Any call on it
    /// afterwards throws, except `close()` itself, which is harmless twice.
    pub fn close(&mut self) {
        self.field = None;
    }

    /// How many wavelet levels were discarded: `0` is the message's own field.
    pub fn reduction(&self) -> Result<u8, JsValue> {
        Ok(self.field()?.reduction)
    }

    /// Grid columns: `ceil(ni / 2^reduction)` of the message's.
    pub fn ni(&self) -> Result<u32, JsValue> {
        Ok(self.field()?.ni)
    }

    /// Grid rows: `ceil(nj / 2^reduction)` of the message's.
    pub fn nj(&self) -> Result<u32, JsValue> {
        Ok(self.field()?.nj)
    }

    /// Where the field sits: above reduction zero, the **derived** grid (every
    /// `2^reduction`-th point of the message's), never the message's own.
    #[wasm_bindgen(unchecked_return_type = "Georef")]
    pub fn grid(&self) -> Result<JsValue, JsValue> {
        to_js(&self.field()?.georef)
    }

    /// The parameter's name, as `WasmField.parameter`.
    #[wasm_bindgen(unchecked_return_type = "string | null")]
    pub fn parameter(&self) -> Result<JsValue, JsValue> {
        Ok(nullable_text(self.field()?.parameter.as_deref()))
    }

    /// The parameter's units, as `WasmField.units`.
    #[wasm_bindgen(unchecked_return_type = "string | null")]
    pub fn units(&self) -> Result<JsValue, JsValue> {
        Ok(nullable_text(self.field()?.units.as_deref()))
    }

    /// As `WasmField.truncation`; only a reduction-zero field has one.
    #[wasm_bindgen(unchecked_return_type = "SpectralTruncation | null")]
    pub fn truncation(&self) -> Result<JsValue, JsValue> {
        match &self.field()?.truncation {
            Some(t) => to_js(t),
            None => Ok(JsValue::NULL),
        }
    }
}
