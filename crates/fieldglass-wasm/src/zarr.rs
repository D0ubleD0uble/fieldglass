//! Zarr stores in the browser (#1083).
//!
//! A store is many objects rather than one run of bytes, so it opens from a
//! map of the objects the page has fetched, by their keys from the store's
//! root. The Worker builds that map up in three fetches, each planned by the
//! store itself:
//!
//! 1. the metadata: `zarr.json` for v3, `.zmetadata` for a consolidated v2
//!    store (an unconsolidated one also needs each array's `.zarray` and
//!    `.zattrs`);
//! 2. `handle.coordinateChunks(variable)`: the coordinate arrays of the
//!    variable's axes, which a box is matched against;
//! 3. `handle.planWindow(request).chunks`: the variable's chunks a box and a
//!    time step touch.
//!
//! Then `openStore(objects)` once more and `handle.decodeWindow(request)`.
//! Every call is synchronous and reads only the map (ADR-0005): the network is
//! the Worker's, and so is the choice of which objects to keep between views.
//!
//! The rest of the variables surface (`variables`, `dimensions`,
//! `decodeSlice`, `decodeLine`) works on a store as on a NetCDF file, reading
//! whatever chunks the call covers.

use fieldglass::{DecodeOptions, MemoryObjects, Session, WindowRequest};
use js_sys::{Map, Object, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::{Handle, WasmField, from_js, strict, throw, to_js};

/// Open a Zarr store from the objects a page has fetched for it.
///
/// `objects` is a `Map` or a plain object from each key — relative to the
/// store's root, as `t2m/c/0/1/2` or `t2m/0.1.2` — to its bytes. Both
/// editions, consolidated or not. Every buffer is copied into linear memory;
/// the caller may reuse its own at once.
///
/// **A key that is not there reads as absent**, and an absent chunk is the
/// array's fill value: that is how a sparse store works, and it is also what a
/// chunk you forgot to fetch looks like. Pass every key a plan names. A key
/// whose fetch came back 404 can be left out or passed as `null`.
///
/// Throws `decode` when the objects hold no Zarr root document, or one that
/// does not parse, and `invalid_option` for a value that is not a `Uint8Array`.
#[wasm_bindgen(js_name = openStore)]
pub fn open_store(
    #[wasm_bindgen(
        unchecked_param_type = "Map<string, Uint8Array | null> | Record<string, Uint8Array | null>"
    )]
    objects: JsValue,
) -> Result<Handle, JsValue> {
    let session = Session::open_store(read_objects(&objects)?).map_err(throw)?;
    Ok(Handle { session })
}

/// The JS map as the in-memory store the session reads.
fn read_objects(objects: &JsValue) -> Result<MemoryObjects, JsValue> {
    let entries: Vec<(JsValue, JsValue)> = if let Some(map) = objects.dyn_ref::<Map>() {
        let mut out = Vec::with_capacity(map.size() as usize);
        map.for_each(&mut |value, key| out.push((key, value)));
        out
    } else if objects.is_object() {
        Object::entries(objects.unchecked_ref::<Object>())
            .iter()
            .map(|pair| {
                let pair: js_sys::Array = pair.unchecked_into();
                (pair.get(0), pair.get(1))
            })
            .collect()
    } else {
        return Err(invalid(
            "the store's objects must be a Map or an object of key to Uint8Array".to_string(),
        ));
    };
    let mut store = MemoryObjects::new();
    for (key, value) in entries {
        let Some(key) = key.as_string() else {
            return Err(invalid(
                "every key of the store's objects must be a string".to_string(),
            ));
        };
        if value.is_null() || value.is_undefined() {
            continue;
        }
        let Some(bytes) = value.dyn_ref::<Uint8Array>() else {
            return Err(invalid(format!(
                "the object {key:?} must be a Uint8Array, or null for one the store does not hold"
            )));
        };
        store.insert(key, bytes.to_vec());
    }
    Ok(store)
}

fn invalid(detail: String) -> JsValue {
    throw(fieldglass::Error::InvalidOption { detail })
}

#[wasm_bindgen]
impl Handle {
    /// The objects holding the coordinate arrays of `variable`'s axes, in the
    /// order of its axes: `[{ array, key, index }]`. Fetch them before
    /// `planWindow`, which matches a box against them. The time axis is among
    /// them, so the same fetch labels the steps. `null` for a NetCDF file,
    /// which holds its coordinates already.
    ///
    /// Throws `wrong_addressing` for a message stream and `no_such_message`
    /// for a variable past the list.
    #[wasm_bindgen(js_name = coordinateChunks, unchecked_return_type = "ChunkRef[] | null")]
    pub fn coordinate_chunks(&self, variable: u32) -> Result<JsValue, JsValue> {
        to_js(&self.session.coordinate_chunks(variable).map_err(throw)?)
    }

    /// Which cells a box holds at one position on the other axes, and which
    /// chunks of the variable hold them.
    ///
    /// `request` is `{ variable, yDim, xDim, bounds, sliceIndices }`: the
    /// arguments `decodeSlice` takes plus `bounds`, `[latMin, latMax, lonMin,
    /// lonMax]`. A cell is in the box when its centre is. A box over the
    /// antimeridian runs past 180 (`[.., 170, 190]`) rather than wrapping. Every
    /// key is required and an unknown one throws `invalid_option`.
    ///
    /// The answer is `{ rows, columns, ni, nj, chunks }`: `chunks` is every
    /// object to fetch for `decodeWindow`, each once, and `columns` is one run,
    /// or two when the box crosses the seam of a global grid. A box that holds
    /// no cell is an empty plan, not an error.
    ///
    /// A window is at least two cells each way, since one row or column cannot
    /// be placed: a box that holds one takes the next one too.
    ///
    /// Throws `invalid_option`, naming the keys, when the map lacks a chunk of
    /// the coordinates `coordinateChunks` named, and `unsupported` for a slice
    /// not placed on a regular lat/lon grid, as 1-D latitude and longitude
    /// coordinates place one. A sharded array's plan names whole shards.
    #[wasm_bindgen(js_name = planWindow, unchecked_return_type = "WindowPlan")]
    pub fn plan_window(
        &self,
        #[wasm_bindgen(unchecked_param_type = "WindowRequest")] request: JsValue,
    ) -> Result<JsValue, JsValue> {
        let request: WindowRequest = strict(request, "window request")?;
        to_js(&self.session.plan_window(&request).map_err(throw)?)
    }

    /// The field over a window's cells, read from the chunks `planWindow`
    /// names and no others. `request` as for `planWindow`; `options` as for
    /// `decodeSlice`.
    ///
    /// The field is a slice like any other, `ni` by `nj`, placed at the
    /// window's own corners, so `warp`, `palette`, `render`, `probe` and
    /// `contours` take it. It is yours; free it. Throws `invalid_option` for a
    /// box that holds no cell.
    #[wasm_bindgen(js_name = decodeWindow)]
    pub fn decode_window(
        &self,
        #[wasm_bindgen(unchecked_param_type = "WindowRequest")] request: JsValue,
        #[wasm_bindgen(unchecked_param_type = "DecodeOptions | null | undefined")] options: JsValue,
    ) -> Result<WasmField, JsValue> {
        let request: WindowRequest = strict(request, "window request")?;
        let options: DecodeOptions = from_js(options)?;
        let field = self
            .session
            .decode_window(&request, &options)
            .map_err(throw)?;
        Ok(WasmField { field })
    }
}
