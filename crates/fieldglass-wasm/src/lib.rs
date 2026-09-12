#![forbid(unsafe_code)]
//! `fieldglass-wasm` — a synchronous browser façade over the Fieldglass
//! decoders (#460).
//!
//! Four things live here and nothing else (ADR-0006): the `Uint8Array` /
//! `Float32Array` handoff, the error mapping, method forwarding, and the GLSL
//! snippet a GPU host pastes into its own shader. Every decision about what a
//! field *is* was made in [`fieldglass`].
//!
//! # The memory contract
//!
//! Linear memory never shrinks, and an animation holds many fields at once, so
//! **the façade keeps no decode cache**. [`Handle::decode`] hands a [`WasmField`]
//! to JS and JS owns it; `warp`, `render`, `probe`, and `contours` take it back
//! by reference. Call `field.free()` when you are done with one — a dropped JS
//! reference does not release the wasm allocation until the host's
//! `FinalizationRegistry` runs, if it runs at all.
//!
//! Every accessor that returns a typed array **copies** out of linear memory,
//! because a view into it dangles the moment wasm grows the heap. A host that
//! wants to reuse a buffer copies into its own once and keeps that.
//!
//! # Values first, pixels second
//!
//! [`Handle::render`] is a CPU fallback. The intended path is
//! [`Handle::palette`] plus [`glsl_snippet`]: colour is decided once, in Rust,
//! and exported as a 256-entry lookup table, so restyling never re-decodes and
//! the CPU painter stays the oracle the shader is checked against rather than a
//! second colour implementation.
//!
//! # Panics
//!
//! Built with `panic = "abort"`: a decoder panic kills the Worker. The fuzz
//! targets make that rare; treat the Worker as disposable and restart it.

use fieldglass::{DecodeOptions, Field, Isoline, PaletteOptions, Session, WarpOptions};
use wasm_bindgen::prelude::*;

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
fn throw(e: fieldglass::Error) -> JsValue {
    let err = js_sys::Error::new(&e.message());
    // A failed property set would mean a frozen `Error.prototype`; the message
    // is still correct without the code, so it is not worth failing the call.
    let _ = js_sys::Reflect::set(
        &err,
        &JsValue::from_str("code"),
        &JsValue::from_str(e.code()),
    );
    err.into()
}

fn to_js<T: serde::Serialize>(value: &T) -> Result<JsValue, JsValue> {
    serde_wasm_bindgen::to_value(value).map_err(|e| js_sys::Error::new(&e.to_string()).into())
}

fn from_js<T: serde::de::DeserializeOwned + Default>(value: JsValue) -> Result<T, JsValue> {
    if value.is_undefined() || value.is_null() {
        return Ok(T::default());
    }
    serde_wasm_bindgen::from_value(value)
        .map_err(|e| js_sys::Error::new(&format!("could not read the options object: {e}")).into())
}

/// Every field-combine operation, in menu order: `[{ value, label }, …]`.
///
/// What a Compare picker is built from. The list is
/// [`fieldglass::combine_ops`]'s, so this host and the VS Code one offer the
/// same operations under the same tags (#342) — and `value` is exactly what
/// [`Handle::combine`] takes back.
#[wasm_bindgen(js_name = combineOps)]
pub fn combine_ops() -> Result<JsValue, JsValue> {
    to_js(&fieldglass::combine_ops())
}

/// An open file. Holds the bytes and the parsed message index, nothing else.
#[wasm_bindgen]
#[derive(Debug)]
pub struct Handle {
    session: Session,
}

/// Open a container from its bytes. The format is detected from the bytes.
///
/// The buffer is copied into linear memory; the caller may reuse or free its
/// own immediately.
#[wasm_bindgen]
pub fn open(bytes: &[u8]) -> Result<Handle, JsValue> {
    let session = Session::open(bytes.to_vec()).map_err(throw)?;
    Ok(Handle { session })
}

#[wasm_bindgen]
impl Handle {
    /// `"grib1"`, `"grib2"` or `"netcdf"`.
    pub fn format(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session.format())
    }

    /// Which of the two ways this container is addressed: `"messages"` or
    /// `"variables"`.
    ///
    /// **Ask this first.** GRIB is a stream of self-describing messages, so an
    /// index is the whole address and [`Handle::count`], [`Handle::message`]
    /// and [`Handle::decode`] are the calls. NetCDF holds named variables over
    /// shared dimensions, so a field is a variable, two of its axes, and where
    /// you are standing on the rest — [`Handle::variables`],
    /// [`Handle::dimensions`] and [`decodeSlice`](Handle::decode_slice). Asking one mode's
    /// question of the other throws `wrong_addressing` naming the call to make
    /// instead, rather than an index error that would blame the number when the
    /// question was wrong.
    pub fn addressing(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session.addressing())
    }

    /// How many messages the file holds. Zero for a variable container.
    pub fn count(&self) -> u32 {
        self.session.count()
    }

    /// The dataset's shared dimensions, in the order the file declares them.
    ///
    /// Shared is the point: two variables naming the same dimension are on the
    /// same axis, so a page offers one time slider for a file rather than one
    /// per variable. Empty for a message container.
    pub fn dimensions(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session.dimensions())
    }

    /// The variables a caller can decode a slice of.
    ///
    /// Renderable ones only — a variable of fewer than two dimensions has no
    /// raster to put on a map. Each carries the `index` [`decodeSlice`](Handle::decode_slice)
    /// takes. Empty for a message container.
    pub fn variables(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session.variables())
    }

    /// Decode one 2-D slice of a variable into the same field
    /// [`Handle::decode`] returns.
    ///
    /// `yDim` and `xDim` index into that variable's own `dims`, and
    /// `sliceIndices` holds **one entry per dimension** in the same declared
    /// order — so `sliceIndices[d]` is always the position on `dims[d]`, and
    /// the two horizontal entries are ignored rather than absent. A wrong
    /// length throws `invalid_option` rather than being guessed: silently
    /// defaulting the unstated axes to zero is how a viewer shows the first
    /// time step and labels it the last.
    ///
    /// The field that comes back is not special. `render`, `probe`, `contours`,
    /// `combine`, `warp` and `palette` take it exactly as they take a decoded
    /// message, which is why the addressing split stops here.
    #[wasm_bindgen(js_name = decodeSlice)]
    pub fn decode_slice(
        &self,
        variable: u32,
        y_dim: u32,
        x_dim: u32,
        slice_indices: &[u32],
        options: JsValue,
    ) -> Result<WasmField, JsValue> {
        let options: DecodeOptions = from_js(options)?;
        let field = self
            .session
            .decode_slice(variable, y_dim, x_dim, slice_indices, &options)
            .map_err(throw)?;
        Ok(WasmField { field })
    }

    /// One line through a variable: its values along `alongDim`, every other
    /// axis held at `sliceIndices` (#172). `options` is `{ dtype?: "auto" | "f32"
    /// | "f64" }`.
    ///
    /// The profile or time series a viewer plots beside a map when a user clicks
    /// a cell. `sliceIndices` names a position on every axis, the one being read
    /// along included and ignored — so a page passes the vector it already holds
    /// for the slice on screen, with the clicked cell written into its two
    /// horizontal positions.
    ///
    /// Returned as a plain object rather than a class with typed-array accessors,
    /// as `decodeSlice`'s field is: a line is one axis long, so there is no large
    /// buffer to hand over without copying. `values` is `{ dtype, data }`, read
    /// `mask` before a value, and `coordinates` is absent when the axis has none.
    ///
    /// Throws `wrong_addressing` for a message stream, `no_such_message` for a
    /// variable past the list, and `invalid_option` for an axis the variable does
    /// not have, a `sliceIndices` of the wrong length, or an index past its axis.
    #[wasm_bindgen(js_name = decodeLine)]
    pub fn decode_line(
        &self,
        variable: u32,
        along_dim: u32,
        slice_indices: &[u32],
        options: JsValue,
    ) -> Result<JsValue, JsValue> {
        let options: DecodeOptions = from_js(options)?;
        let line = self
            .session
            .decode_line(variable, along_dim, slice_indices, &options)
            .map_err(throw)?;
        to_js(&line)
    }

    /// One message's metadata, built on demand.
    ///
    /// Lazy on purpose: a thousand-message file should not serialise a thousand
    /// of these to open. Ask for the ones you are going to show.
    pub fn message(&self, index: u32) -> Result<JsValue, JsValue> {
        let info = self.session.message(index).map_err(throw)?;
        to_js(&info)
    }

    /// Decode one message. `options` is `{ dtype?: "auto" | "f32" | "f64" }`.
    ///
    /// The returned field is yours; free it when you are done.
    pub fn decode(&self, index: u32, options: JsValue) -> Result<WasmField, JsValue> {
        let options: DecodeOptions = from_js(options)?;
        let field = self.session.decode(index, &options).map_err(throw)?;
        Ok(WasmField { field })
    }

    /// Resample a field onto a geographic box without painting it.
    ///
    /// `options` is `{ bilinear?: boolean, bounds?: [latMin, latMax, lonMin,
    /// lonMax], width?: number, height?: number }` — a window at a pixel size,
    /// which is what a map view asks for (#465). Send `width` and `height`
    /// together or not at all; one alone throws `invalid_option`, as does a
    /// zero. With neither, the output is the source `ni × nj`, as before.
    pub fn warp(&self, field: &WasmField, options: JsValue) -> Result<JsValue, JsValue> {
        let options: WarpOptions = from_js(options)?;
        let out = self.session.warp(&field.field, &options).map_err(throw)?;
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

    /// The colour decision as data: `{ lut, t0, t1, span, scale, maskedRgba }`.
    ///
    /// `lut` is 256 RGBA entries with any reversal already applied — the CPU
    /// painter's own table. Upload it as a 256 × 1 `RGBA8` texture sampled
    /// `NEAREST` and pair it with [`glsl_snippet`].
    pub fn palette(&self, field: &WasmField, options: JsValue) -> Result<JsValue, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let palette = self
            .session
            .palette(&field.field, &options)
            .map_err(throw)?;
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
        Ok(object.into())
    }

    /// Paint a field to RGBA on the CPU — the fallback, and the oracle the
    /// shader path is checked against.
    ///
    /// `flipY` composes with the message's own scan order rather than replacing
    /// it, so `false` means **north up** and not "rows as stored": a grid
    /// scanning south-to-north is flipped for you. Pass the user's own request
    /// straight through; composing `grid().scan.jPositive` yourself would flip
    /// twice.
    #[wasm_bindgen(js_name = render)]
    pub fn render(
        &self,
        field: &WasmField,
        options: JsValue,
        flip_y: bool,
    ) -> Result<js_sys::Uint8Array, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let raster = self
            .session
            .render(&field.field, &options, flip_y)
            .map_err(throw)?;
        Ok(js_sys::Uint8Array::from(&raster.rgba[..]))
    }

    /// Sample one geographic point. `undefined` when the point is off the grid
    /// or the family cannot place it.
    pub fn probe(&self, field: &WasmField, lat: f64, lon: f64) -> Result<JsValue, JsValue> {
        match self.session.probe(&field.field, lat, lon) {
            Some(p) => to_js(&p),
            None => Ok(JsValue::UNDEFINED),
        }
    }

    /// Combine two aligned fields element by element — the difference map and
    /// its siblings (#579).
    ///
    /// `op` is one of the `value` tags [`combine_ops`] reports. The result is a
    /// field like any other, on **A's** placement, so `warp`, `palette`,
    /// `render`, `probe` and `contours` all take it; it is yours, so free it.
    ///
    /// Throws `invalid_option` for an op this build does not know, and
    /// `unsupported` when the two fields do not align cell for cell — the
    /// message names which property differs.
    pub fn combine(&self, a: &WasmField, b: &WasmField, op: &str) -> Result<WasmField, JsValue> {
        let op = fieldglass::op_from_wire(op).map_err(throw)?;
        let field = self
            .session
            .combine(&a.field, &b.field, op)
            .map_err(throw)?;
        Ok(WasmField { field })
    }

    /// Isolines in fractional grid coordinates. An empty `levels` asks for a
    /// nice set spanning the field's own range.
    pub fn contours(&self, field: &WasmField, levels: &[f64]) -> Result<JsValue, JsValue> {
        let out: Vec<Isoline> = self.session.contours(&field.field, levels).map_err(throw)?;
        to_js(&out)
    }

    /// The field as an `R32F` texture wants it: transformed by the palette's
    /// scale and **rebased by `t0`**, with the subtraction done in `f64`.
    ///
    /// Pass the same `options` you passed to [`Handle::palette`] — the palette
    /// is 256 entries and rebuilding it costs nothing next to the field.
    /// A shader that subtracts `t0` itself, in `f32`, loses the domain rather
    /// than the result; see `fieldglass::shader`.
    #[wasm_bindgen(js_name = shaderValues)]
    pub fn shader_values(
        &self,
        field: &WasmField,
        options: JsValue,
    ) -> Result<js_sys::Float32Array, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let palette = self
            .session
            .palette(&field.field, &options)
            .map_err(throw)?;
        let values = fieldglass::shader_values(&field.field, &palette);
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
        options: JsValue,
    ) -> Result<js_sys::Uint8Array, JsValue> {
        let options: PaletteOptions = from_js(options)?;
        let palette = self
            .session
            .palette(&field.field, &options)
            .map_err(throw)?;
        let mask = fieldglass::shader_mask(&field.field, &palette);
        Ok(js_sys::Uint8Array::from(&mask[..]))
    }
}

fn set(object: &js_sys::Object, key: &str, value: impl Into<JsValue>) -> Result<(), JsValue> {
    js_sys::Reflect::set(object, &JsValue::from_str(key), &value.into())?;
    Ok(())
}

/// One decoded field, owned by the caller.
#[wasm_bindgen]
#[derive(Debug)]
pub struct WasmField {
    field: Field,
}

#[wasm_bindgen]
impl WasmField {
    /// The values, as a `Float32Array` or a `Float64Array` depending on what
    /// the source supports — see the `dtype` accessor. A copy: the array does
    /// not alias linear memory.
    pub fn values(&self) -> JsValue {
        match self.field.values.as_f32() {
            Some(v) => js_sys::Float32Array::from(v).into(),
            // Every other width crosses as `Float64Array`: the accessor's job
            // is a lossless handoff, and `f64` is the only typed array that is
            // lossless for every width the API can hold.
            None => js_sys::Float64Array::from(&self.field.values.to_f64()[..]).into(),
        }
    }

    /// `"f32"` or `"f64"` — which typed array [`WasmField::values`] returned.
    pub fn dtype(&self) -> String {
        if self.field.values.as_f32().is_some() {
            "f32".to_string()
        } else {
            "f64".to_string()
        }
    }

    /// One byte per cell: `1` present, `0` absent. A separate array rather than
    /// `NaN` in the values, because `isnan()` is unreliable on some mobile GPUs
    /// and a `NaN` poisons linear filtering in a texture.
    pub fn mask(&self) -> js_sys::Uint8Array {
        js_sys::Uint8Array::from(&self.field.mask[..])
    }

    /// Grid columns (west-to-east point count of one row).
    pub fn ni(&self) -> u32 {
        self.field.ni
    }

    /// Grid rows. `values()` is `ni * nj` long, row-major.
    pub fn nj(&self) -> u32 {
        self.field.nj
    }

    /// Where the field sits on the Earth: `kind`, `boundsLonlat`, `proj4`,
    /// `x0`, `y0`, `dx`, `dy`, `periodicX`, `scan`.
    pub fn grid(&self) -> Result<JsValue, JsValue> {
        to_js(&self.field.georef)
    }

    /// `{ min, max, validCount }` over the present cells.
    pub fn stats(&self) -> Result<JsValue, JsValue> {
        to_js(&self.field.stats)
    }

    /// The parameter's name, e.g. `"Temperature"`. When no table in the crate
    /// resolves the message's parameter codes, this is `Parameter <codes>`
    /// naming the codes that went unresolved — `Parameter 209/10/0` for GRIB2,
    /// `Parameter 98/128/210` for GRIB1. See `fieldglass::api::Field` for the
    /// contract in full (#633).
    pub fn parameter(&self) -> String {
        self.field.parameter.clone()
    }

    /// The parameter's units as the originating table states them, e.g.
    /// `"K"`. Empty when the parameter did not resolve, or is dimensionless.
    pub fn units(&self) -> String {
        self.field.units.clone()
    }
}
