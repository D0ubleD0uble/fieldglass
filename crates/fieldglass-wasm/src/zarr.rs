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
//!
//! A sharded variable can be read in part (#1123): the plan's
//! `shardIndexes` name each shard's index, fetched as a range, and once those
//! are in the map, a second `planWindow` names the byte range of each inner
//! chunk in `innerChunks`. Both go into the map as `{ size, ranges }` under the
//! shard's key, so a window fetches the inner chunks it shows and not whole
//! shards.
//!
//! Every call is synchronous and reads only the map (ADR-0005): the network is
//! the Worker's, and so is the choice of which objects to keep between views.
//!
//! The rest of the variables surface (`variables`, `dimensions`,
//! `decodeSlice`, `decodeLine`) works on a store as on a NetCDF file, reading
//! whatever chunks the call covers.

use fieldglass::{DecodeOptions, PrefetchedObjects, Session, WindowRequest};
use js_sys::{Map, Object, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

use crate::{Handle, WasmField, from_js, object_brand, strict, throw, to_js};

/// Open a Zarr store from the objects a page has fetched for it.
///
/// `objects` is a `Map` or a plain object from each key — relative to the
/// store's root, as `t2m/c/0/1/2` or `t2m/0.1.2` — to its bytes. Both
/// editions, consolidated or not. Every buffer is copied into linear memory;
/// the caller may reuse its own at once.
///
/// **Part of an object** is `{ size, ranges: [{ offset, bytes }] }`: the
/// object's whole length (an HTTP response's `Content-Range` states it) and
/// each range fetched of it, by its first byte (#1123). That is how a sharded
/// variable is read in part: give each shard's index and the inner chunks
/// `planWindow` names this way, under the shard's key, all the ranges of one
/// shard in one entry. A read takes each range from one given range that holds
/// it. Throws `invalid_option` for a range past `size`, two sizes for one key,
/// and a key it does not know.
///
/// **Pass a key whose fetch came back 404 as `null`.** That says the store
/// holds no such object, and a chunk it never wrote is the array's fill value:
/// how a sparse store works, and how zarr-python leaves a chunk that is all
/// fill, coordinates included. A key that is **not passed at all** is one you
/// have not fetched, and a read that needs it throws rather than filling it:
/// `planWindow` and `decodeWindow` throw `invalid_option` naming every such
/// key of the coordinates or of the plan, and any other read (`decodeSlice`,
/// `decodeLine`, `axisValues`) throws `decode` naming the chunks it needed.
/// `undefined` counts as not passed, so a fetch helper that returns nothing
/// on a network error is refused rather than read as fill. An unconsolidated
/// v2 store also needs each array's and group's `.zattrs`, as bytes or as
/// `null` for a 404: opening without it throws `decode` naming it, since it
/// holds the dimension names and the CF scale and fill. Any other metadata
/// document is asked for speculatively and may be left out.
///
/// Throws `decode` when the objects hold no Zarr root document, or one that
/// does not parse, and `invalid_option` for a value that is not a `Uint8Array`,
/// `{ size, ranges }` or `null`. A `Map` and `Uint8Array`s made in another
/// realm (a `vm` context, an iframe) are read like those made here (#1244).
#[wasm_bindgen(js_name = openStore)]
pub fn open_store(
    #[wasm_bindgen(
        unchecked_param_type = "Map<string, Uint8Array | ObjectRanges | null> | Record<string, Uint8Array | ObjectRanges | null>"
    )]
    objects: JsValue,
) -> Result<Handle, JsValue> {
    let session = Session::open_store(read_objects(&objects)?).map_err(throw)?;
    Ok(Handle {
        session: Some(session),
    })
}

/// The JS map as the in-memory store the session reads: bytes for an object,
/// absent for `null`, and every key not in the map, or mapped to `undefined`,
/// one the host has not fetched (#1124).
fn read_objects(objects: &JsValue) -> Result<PrefetchedObjects, JsValue> {
    let entries: Vec<(JsValue, JsValue)> = if let Some(entries) = map_entries(objects)? {
        entries
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
    let mut store = PrefetchedObjects::new();
    for (key, value) in entries {
        let Some(key) = key.as_string() else {
            return Err(invalid(
                "every key of the store's objects must be a string".to_string(),
            ));
        };
        // Only `null` says "the store holds none". `undefined` is what a
        // helper returns when it fetched nothing, so it is not passed.
        if value.is_undefined() {
            continue;
        }
        if value.is_null() {
            store.insert_absent(key);
            continue;
        }
        if let Some(bytes) = uint8_bytes(&value)? {
            store.insert(key, bytes);
            continue;
        }
        let Some((size, ranges)) = object_ranges(&key, &value)? else {
            return Err(invalid(format!(
                "the object {key:?} must be a Uint8Array, `{{ size, ranges }}` for parts of \
                 it, or null for one the store does not hold"
            )));
        };
        // No ranges still states the size, so a read names the ranges it
        // lacks rather than the whole object.
        let ranges = if ranges.is_empty() {
            vec![(0, Vec::new())]
        } else {
            ranges
        };
        for (offset, bytes) in ranges {
            store
                .insert_range(key.clone(), size, offset, bytes)
                .map_err(|e| invalid(e.to_string()))?;
        }
    }
    Ok(store)
}

/// The parts of one object a page fetched by range (#1123), as
/// `{ size, ranges: [{ offset, bytes }] }`: `None` when `value` is not an
/// object of that shape at all, and a refusal when it is one with a key
/// missing, a key it does not know, or a number that is not a byte count.
fn object_ranges(key: &str, value: &JsValue) -> Result<Option<ObjectParts>, JsValue> {
    if !value.is_object()
        || js_sys::Array::is_array(value)
        || js_sys::ArrayBuffer::is_view(value)
        || !has_only_keys(value, &["size", "ranges"])
    {
        return Ok(None);
    }
    let size = byte_count(&js_sys::Reflect::get(value, &"size".into())?).ok_or_else(|| {
        invalid(format!(
            "the object {key:?} needs `size`, its whole length in bytes"
        ))
    })?;
    let ranges = js_sys::Reflect::get(value, &"ranges".into())?;
    if !js_sys::Array::is_array(&ranges) {
        return Err(invalid(format!(
            "the object {key:?} needs `ranges`, a list of `{{ offset, bytes }}`"
        )));
    }
    let mut out = Vec::new();
    for range in ranges.unchecked_into::<js_sys::Array>().iter() {
        let bad = || {
            invalid(format!(
                "each range of {key:?} must be `{{ offset, bytes }}`: a byte offset and a \
                 Uint8Array"
            ))
        };
        if !range.is_object()
            || js_sys::Array::is_array(&range)
            || !has_only_keys(&range, &["offset", "bytes"])
        {
            return Err(bad());
        }
        let offset =
            byte_count(&js_sys::Reflect::get(&range, &"offset".into())?).ok_or_else(bad)?;
        let bytes =
            uint8_bytes(&js_sys::Reflect::get(&range, &"bytes".into())?)?.ok_or_else(bad)?;
        out.push((offset, bytes));
    }
    Ok(Some((size, out)))
}

/// An object's size and each fetched range of it, by its first byte.
type ObjectParts = (u64, Vec<(u64, Vec<u8>)>);

/// Whether `value`'s own enumerable keys are exactly `keys`: a misspelt
/// `offest` is refused rather than read as a missing offset.
fn has_only_keys(value: &JsValue, keys: &[&str]) -> bool {
    let own: Vec<String> = Object::keys(value.unchecked_ref::<Object>())
        .iter()
        .filter_map(|k| k.as_string())
        .collect();
    own.len() == keys.len() && keys.iter().all(|k| own.iter().any(|o| o == k))
}

/// A JS number that is a byte count: a whole number from zero up to
/// `Number.MAX_SAFE_INTEGER`, past which a double no longer counts bytes
/// exactly.
fn byte_count(value: &JsValue) -> Option<u64> {
    let n = value.as_f64()?;
    // 2^53 - 1, `Number.MAX_SAFE_INTEGER`.
    let safe = 9_007_199_254_740_991.0;
    (n.is_finite() && n >= 0.0 && n.fract() == 0.0 && n <= safe).then_some(n as u64)
}

/// The shape `openStore` takes for part of an object (#1123).
#[wasm_bindgen(typescript_custom_section)]
const OBJECT_RANGES: &str = r#"
/**
 * Parts of one stored object a page fetched by range: a shard's index, or
 * some of its inner chunks (`WindowPlan.shardIndexes`, `WindowPlan.innerChunks`).
 */
export interface ObjectRanges {
  /** The object's whole length in bytes, as the response's `Content-Range` states it. */
  size: number;
  /** Each range fetched: its first byte, and its bytes. */
  ranges: { offset: number; bytes: Uint8Array }[];
}
"#;

/// The entries of `objects` when it is a `Map`, from this realm or another;
/// `None` when it is not one, so the caller reads it as a plain object.
///
/// Not `dyn_ref::<Map>`, which is `instanceof` and false for a `Map` made in a
/// `vm` context or an iframe: that map fell through and read as an object with
/// no entries, an empty store (#1244). `Map.prototype.forEach` called on the
/// value checks the value's internal slot, which holds across realms and which
/// `Symbol.toStringTag` cannot forge, so a look-alike throws `TypeError` and is
/// `None`. The brand ([`object_brand`], #1242) only spares the call for a value
/// that is plainly not a `Map`.
fn map_entries(objects: &JsValue) -> Result<Option<Vec<(JsValue, JsValue)>>, JsValue> {
    if object_brand(objects)? != "[object Map]" {
        return Ok(None);
    }
    let for_each: js_sys::Function =
        js_sys::Reflect::get(&Map::new(), &"forEach".into())?.unchecked_into();
    let out = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let sink = out.clone();
    let collect = Closure::<dyn FnMut(JsValue, JsValue)>::new(move |value, key| {
        sink.borrow_mut().push((key, value));
    });
    match for_each.call1(objects, collect.as_ref().unchecked_ref()) {
        Ok(_) => Ok(Some(out.take())),
        // Tagged `[object Map]` but not a Map. With keys of its own it is an
        // object of objects; with none it would read as an empty store.
        Err(_) if Object::keys(objects.unchecked_ref::<Object>()).length() == 0 => Err(invalid(
            "the store's objects are tagged as a Map but are not one".to_string(),
        )),
        Err(_) => Ok(None),
    }
}

/// The bytes of `value` when it is a `Uint8Array`, from this realm or another;
/// `None` when it is anything else.
///
/// Not `dyn_ref::<Uint8Array>`, which is `instanceof` and refused a view made in
/// another realm as "must be a Uint8Array" (#1244). Not the brand
/// ([`object_brand`], #1242) either: `Symbol.toStringTag` is a tag a page can
/// forge, and so is an own `byteLength`. The test is the getter of
/// `%TypedArray%.prototype[Symbol.toStringTag]`, taken from this realm's
/// intrinsics: it returns the view's internal element-type name for a typed
/// array of any realm and `undefined` for everything else, a `DataView`
/// included. Only `"Uint8Array"` passes, and the constructor then copies exactly
/// those bytes. A detached or out-of-bounds view makes the constructor throw;
/// that is caught and refused as `invalid_option`.
fn uint8_bytes(value: &JsValue) -> Result<Option<Vec<u8>>, JsValue> {
    if !value.is_object() {
        return Ok(None);
    }
    let proto =
        Object::get_prototype_of(&Object::get_prototype_of(&Uint8Array::new_with_length(0)));
    let descriptor = Object::get_own_property_descriptor(&proto, &js_sys::Symbol::to_string_tag());
    let getter: js_sys::Function =
        js_sys::Reflect::get(&descriptor, &"get".into())?.unchecked_into();
    if getter.call0(value)?.as_string().as_deref() != Some("Uint8Array") {
        return Ok(None);
    }
    let args = js_sys::Array::of1(value);
    match js_sys::Reflect::construct(&Uint8Array::new_with_length(0).constructor(), &args) {
        Ok(copy) => Ok(Some(copy.unchecked_into::<Uint8Array>().to_vec())),
        Err(_) => Err(invalid(
            "a Uint8Array in the store is detached or out of bounds".to_string(),
        )),
    }
}

fn invalid(detail: String) -> JsValue {
    throw(fieldglass::Error::invalid_option(detail))
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
        to_js(&self.session()?.coordinate_chunks(variable).map_err(throw)?)
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
    /// The answer is `{ rows, columns, ni, nj, chunks, shardIndexes,
    /// innerChunks }`: `chunks` is every object to fetch for `decodeWindow`,
    /// each once, and `columns` is one run, or two when the box crosses the
    /// seam of a global grid. A box that holds no cell is an empty plan, not an
    /// error.
    ///
    /// For a sharded variable `chunks` are its shards, and a page may read
    /// them in part (#1123). `shardIndexes` says where each shard's index is:
    /// its last `length` bytes (`Range: bytes=-length`) or its first, as
    /// `location` says. Pass each back as `{ size, ranges }` with the shard's
    /// size, open the store again and plan again: `innerChunks` then names the
    /// `offset` and `length` of every inner chunk the window intersects,
    /// leaving out those the index marks as never written (they are the fill
    /// value). Add those ranges to the same entries and decode. Both are
    /// `null` for a variable that is not sharded, and `innerChunks` is `null`
    /// until every index is in the map.
    ///
    /// A window is at least two cells each way, since one row or column cannot
    /// be placed: a box that holds one takes the next one too.
    ///
    /// Throws `invalid_option`, naming the keys, when the map was not passed a
    /// chunk of the coordinates `coordinateChunks` named (pass one that came
    /// back 404 as `null`: it reads as the fill value), and `unsupported` for a
    /// slice not placed on a regular lat/lon grid, as 1-D latitude and
    /// longitude coordinates place one. That refusal names a coordinate the
    /// store left out and why, and coordinate chunks passed as `null` whose
    /// fill value placed nothing. An index that fails its checksum, or names
    /// bytes outside its shard or over the index itself, throws `decode`.
    #[wasm_bindgen(js_name = planWindow, unchecked_return_type = "WindowPlan")]
    pub fn plan_window(
        &self,
        #[wasm_bindgen(unchecked_param_type = "WindowRequest")] request: JsValue,
    ) -> Result<JsValue, JsValue> {
        let request: WindowRequest = strict(request, "window request")?;
        to_js(&self.session()?.plan_window(&request).map_err(throw)?)
    }

    /// The field over a window's cells, read from the chunks `planWindow`
    /// names and no others. `request` as for `planWindow`; `options` as for
    /// `decodeSlice`.
    ///
    /// The field is a slice like any other, `ni` by `nj`, placed at the
    /// window's own corners, so `warp`, `palette`, `render`, `probe` and
    /// `contours` take it. It is yours; free it. Throws `invalid_option` for a
    /// box that holds no cell, and, naming them, for planned chunks the map
    /// was not passed, or for a shard passed by range, the index or inner chunk
    /// ranges it lacks. A planned chunk passed as `null` reads as the fill
    /// value.
    #[wasm_bindgen(js_name = decodeWindow)]
    pub fn decode_window(
        &self,
        #[wasm_bindgen(unchecked_param_type = "WindowRequest")] request: JsValue,
        #[wasm_bindgen(unchecked_optional_param_type = "DecodeOptions | null")] options: JsValue,
    ) -> Result<WasmField, JsValue> {
        let request: WindowRequest = strict(request, "window request")?;
        let options: DecodeOptions = from_js(options)?;
        let field = self
            .session()?
            .decode_window(&request, &options)
            .map_err(throw)?;
        Ok(WasmField { field: Some(field) })
    }
}
