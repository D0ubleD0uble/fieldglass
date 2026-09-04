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

use fieldglass::{ContourLevel, DecodeOptions, Field, PaletteOptions, Session, WarpOptions};
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

/// An open file. Holds the bytes and the parsed message index, nothing else.
#[wasm_bindgen]
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
    /// `"grib1"` or `"grib2"`.
    pub fn format(&self) -> Result<JsValue, JsValue> {
        to_js(&self.session.format())
    }

    /// How many messages the file holds.
    pub fn count(&self) -> u32 {
        self.session.count()
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
    /// lonMax] }`. Output is the source `ni × nj` until #465 lets a caller size
    /// it.
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
    /// shader path is checked against. `flipY` emits rows bottom-to-top, which
    /// a grid scanning south-to-north needs for a north-up canvas.
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

    /// Isolines in fractional grid coordinates. An empty `levels` asks for a
    /// nice set spanning the field's own range.
    pub fn contours(&self, field: &WasmField, levels: &[f64]) -> Result<JsValue, JsValue> {
        let out: Vec<ContourLevel> = self.session.contours(&field.field, levels).map_err(throw)?;
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

    pub fn ni(&self) -> u32 {
        self.field.ni
    }

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

    pub fn parameter(&self) -> String {
        self.field.parameter.clone()
    }

    pub fn units(&self) -> String {
        self.field.units.clone()
    }
}
