//! The fetch planner, in the browser (#1081).
//!
//! The sequence `docs/architecture/planned/04-hosts.md` draws: the Worker
//! fetches a sidecar's text, the planner turns it into byte ranges plus what
//! the bytes should be, the Worker fetches one range and checks the bytes
//! against the plan before it opens them, then checks the message it opened
//! against the field the plan named. Every call here is synchronous and does no
//! I/O; the network is the Worker's.
//!
//! Like the rest of this crate it forwards and converts, nothing more. The
//! plan items, queries and sources are `fieldglass-fetchplan`'s own serde types,
//! declared in `api.fetchplan.generated.d.ts` from the same schema as
//! everything else, so there is no second spelling of any of them here.

use std::sync::OnceLock;

use fieldglass::api::MessageInfo;
use fieldglass::fetchplan::{
    CandidateOptions, EcmwfIndex, PlanItem, PlanRange, Query, SourceSpec, TableResolver, Wgrib2Idx,
};
use wasm_bindgen::prelude::*;

use crate::{strict, throw, to_js, unreadable};

/// The table-backed resolver, built once per Worker.
///
/// Its first `resolve` inverts the GRIB2 parameter tables into a name index
/// (tens of milliseconds natively), so it is kept rather than rebuilt for each
/// sidecar. NCEP's tables, because every NOAA `.idx` is written in them; an
/// ECMWF `.index` names its parameters in MARS short names, which no table here
/// carries, so those select by the sidecar's own words.
fn resolver() -> &'static TableResolver {
    static RESOLVER: OnceLock<TableResolver> = OnceLock::new();
    RESOLVER.get_or_init(TableResolver::ncep)
}

/// Read a required argument: unlike an options object, `null` is not "all
/// defaults" but a mistake, and is refused as one. An array is refused too, as
/// serde would read it by position (#1221).
fn required<T: serde::de::DeserializeOwned>(value: JsValue, what: &str) -> Result<T, JsValue> {
    crate::refuse_non_object(&value, what)?;
    serde_wasm_bindgen::from_value(value).map_err(|e| unreadable(what, &e))
}

#[derive(Debug)]
enum Sidecar {
    Wgrib2(Wgrib2Idx),
    Ecmwf(EcmwfIndex),
}

/// A parsed GRIB sidecar: a wgrib2 `.idx` or an ECMWF `.index`.
///
/// Holds the parsed records and nothing else; free it when you are done.
#[wasm_bindgen]
#[derive(Debug)]
pub struct MessageManifest {
    /// `None` once closed.
    sidecar: Option<Sidecar>,
}

impl MessageManifest {
    /// The parsed sidecar, or the refusal a closed manifest gives.
    fn manifest(&self) -> Result<&dyn fieldglass::fetchplan::MessageManifest, JsValue> {
        Ok(
            match self
                .sidecar
                .as_ref()
                .ok_or_else(|| super::closed("manifest"))?
            {
                Sidecar::Wgrib2(idx) => idx,
                Sidecar::Ecmwf(idx) => idx,
            },
        )
    }
}

/// Parse a wgrib2 `.idx` sidecar.
///
/// `key` is the object the sidecar describes, as you will fetch it; every plan
/// item carries it back unchanged, and nothing here derives or alters one.
/// Throws `decode`, naming the line, for text that is not a `.idx`.
#[wasm_bindgen(js_name = parseWgrib2Idx)]
pub fn parse_wgrib2_idx(key: &str, text: &str) -> Result<MessageManifest, JsValue> {
    let idx = Wgrib2Idx::parse(key, text).map_err(|e| throw(e.into()))?;
    Ok(MessageManifest {
        sidecar: Some(Sidecar::Wgrib2(idx)),
    })
}

/// Parse an ECMWF `.index` sidecar (JSON lines). `key` as for
/// `parseWgrib2Idx`. Its parameters are MARS short names (`2t`), which no
/// table here resolves to WMO codes, so select them by `abbreviation`.
#[wasm_bindgen(js_name = parseEcmwfIndex)]
pub fn parse_ecmwf_index(key: &str, text: &str) -> Result<MessageManifest, JsValue> {
    let idx = EcmwfIndex::parse(key, text).map_err(|e| throw(e.into()))?;
    Ok(MessageManifest {
        sidecar: Some(Sidecar::Ecmwf(idx)),
    })
}

#[wasm_bindgen]
impl MessageManifest {
    /// Release the parsed sidecar now: the same as `free()`, and as `[Symbol.dispose]`, so
    /// `using` releases it at the end of its block (#1073). Any call on it
    /// afterwards throws, except `close()` itself, which is harmless twice.
    pub fn close(&mut self) {
        self.sidecar = None;
    }

    /// Every record, in the sidecar's order. A `.idx` states no length, so the
    /// last record's range is `openEnded`.
    #[wasm_bindgen(unchecked_return_type = "PlanItem[]")]
    pub fn items(&self) -> Result<JsValue, JsValue> {
        to_js(&self.manifest()?.items())
    }

    /// One item per GRIB message: the records of an `n.m` message share one
    /// fetch, so they collapse onto the first.
    #[wasm_bindgen(unchecked_return_type = "PlanItem[]")]
    pub fn messages(&self) -> Result<JsValue, JsValue> {
        to_js(&self.manifest()?.messages())
    }

    /// The records a query selects, through the GRIB2 parameter tables.
    ///
    /// `query` is `{ abbreviation?, levelText?, forecast?, qualifiers?,
    /// unqualified?, parameter?, level? }`: the first five in the sidecar's
    /// own words, the last two by meaning, which is what matches on either
    /// dialect. Every stated key must hold, and an empty query selects every
    /// record. **Every match comes back**, so check the length: four ensemble
    /// members are four items, never one guessed for you. An unknown key, at
    /// the top or inside `level` or `parameter`, throws `invalid_option`
    /// rather than being ignored, since dropping a misspelt one would widen
    /// the query.
    #[wasm_bindgen(unchecked_return_type = "PlanItem[]")]
    pub fn select(
        &self,
        #[wasm_bindgen(unchecked_optional_param_type = "Query | null")] query: JsValue,
    ) -> Result<JsValue, JsValue> {
        let query: Query = if query.is_undefined() || query.is_null() {
            Query::default()
        } else {
            strict(query, "query")?
        };
        to_js(&self.manifest()?.select(&query, resolver()))
    }
}

/// The value of the HTTP `Range` header that fetches `range`, or `null` when
/// the whole object is wanted and no header should be sent.
///
/// Throws for an `exact` range of no bytes or whose end passes 2^64, which has
/// no header; the planner refuses both when it reads a sidecar, so only a
/// range built by hand gets here. `null` would have the host fetch the whole
/// object.
#[wasm_bindgen(js_name = httpRangeHeader, unchecked_return_type = "string | null")]
pub fn http_range_header(
    #[wasm_bindgen(unchecked_param_type = "PlanRange")] range: JsValue,
) -> Result<JsValue, JsValue> {
    let range: PlanRange = required(range, "range")?;
    let header = range.http_range_header().map_err(|e| throw(e.into()))?;
    Ok(header.map_or(JsValue::NULL, |h| JsValue::from_str(&h)))
}

/// Check fetched bytes against what a plan item promised: the GRIB magic, the
/// edition, and the length the message declares against what arrived. Returns
/// that declared length, which is where the message ends in an open-ended
/// fetch.
///
/// Pass the item back whole, as `select` returned it. Throws `short_read` when
/// fewer bytes arrived than the item asked for or the message declares (worth
/// a retry), and `decode` when the bytes are not the promised message, which is
/// what a stale sidecar looks like (worth refetching the sidecar).
///
/// It assumes a successful ranged response, so check the HTTP status first:
/// 206 for a ranged request, 200 for a whole-object one. A 200 to a ranged
/// request means the server ignored `Range` and sent the whole object. An
/// error page is not a message either, and passed in here it reads as one of
/// the two codes above by its length, sending the caller after the wrong fix.
#[wasm_bindgen(js_name = verifyEnvelope)]
pub fn verify_envelope(
    #[wasm_bindgen(unchecked_param_type = "PlanItem")] item: JsValue,
    bytes: &[u8],
) -> Result<f64, JsValue> {
    let item: PlanItem = required(item, "plan item")?;
    let declared = fieldglass::fetchplan::verify_envelope(&item, bytes).map_err(throw)?;
    // A GRIB2 message can declare up to 2^64 bytes, and a JS number holds an
    // integer exactly only to 2^53. A declared length is returned only when it
    // is at most what arrived in the `Uint8Array`; anything longer has already
    // been refused, as `short_read` from an open range or as `decode` from an
    // exact one whose length disagrees. So this conversion is exact for every
    // value that reaches it.
    Ok(declared as f64)
}

/// Check an opened message against what a plan item promised: the parameter,
/// the level, the reference time and the forecast window, and on a wgrib2
/// `.idx` line its probability (`prob >0.254`), ensemble member (`ENS=+1`),
/// derived forecast (`ens mean`), percentile (`10% level`), and the aerosol or
/// chemical constituent and size and wavelength ranges of the chemistry and
/// aerosol templates 4.40–4.48 (`aerosol=Dust dry`, `aerosol_size <1e-05`).
/// An ECMWF `.index` line's MARS parameter name is checked for ECMWF's
/// open-data parameters, its `step` against the end of the message's window,
/// its `number` against the ensemble member, and its `type` (`cf`, `pf`, `em`,
/// `es`, `ep`, `fc`, `an`) against the product template. Wording the grammar
/// does not read is not compared. A message on a product template the reader
/// does not parse is checked on its reference time alone. The second half of trusting
/// a plan, after
/// `verifyEnvelope`: the bytes are a whole message, and this says whether they
/// are the field the sidecar named or a valid message that is simply another
/// one, which is what a sidecar regenerated against a newer object points at.
///
/// Pass the item back whole, as `select` returned it, and `info` as the
/// handle's `message(0)` returned it. The sidecar's words are compared by
/// meaning, not as text: `2 mb` is the message's 200 Pa, and an ECMWF `sfc` is
/// any single-level field. Throws `decode`, naming the field and both sides,
/// when they disagree (worth refetching the sidecar), and `invalid_option` for
/// an argument that does not read.
#[wasm_bindgen(js_name = verifyMessage)]
pub fn verify_message(
    #[wasm_bindgen(unchecked_param_type = "PlanItem")] item: JsValue,
    #[wasm_bindgen(unchecked_param_type = "MessageInfo")] info: JsValue,
) -> Result<(), JsValue> {
    let item: PlanItem = required(item, "plan item")?;
    let info: MessageInfo = required(info, "message info")?;
    fieldglass::fetchplan::verify_message(&item.expectation, &info).map_err(|m| {
        throw(fieldglass::Error::Decode {
            detail: format!("the message is not the one the plan promised: {m}"),
        })
    })
}

/// Check a catalog entry before it is used. Throws `invalid_option` naming what
/// is wrong: no cycle hours, an hour past 23, hours out of order, a
/// `maxCandidates` above 1024, or a key placeholder the planner does not know.
#[wasm_bindgen(js_name = validateSource)]
pub fn validate_source(
    #[wasm_bindgen(unchecked_param_type = "SourceSpec")] source: JsValue,
) -> Result<(), JsValue> {
    let source: SourceSpec = required(source, "source")?;
    source.validate().map_err(|e| throw(e.into()))
}

/// The runs of `source` that should have posted by `options.nowUnixSecs`,
/// newest first, each with the object key it should be under.
///
/// A candidate is a guess: walk the list and fetch until one succeeds. The
/// clock is yours, in whole seconds: `Math.floor(Date.now() / 1000)`, since a
/// fractional second is refused. The planner reads no clock of its own.
///
/// Validates `source` first, as `validateSource` does, and throws
/// `invalid_option` for the same reasons, including a `maxCandidates` above
/// 1024.
#[wasm_bindgen(unchecked_return_type = "Candidate[]")]
pub fn candidates(
    #[wasm_bindgen(unchecked_param_type = "SourceSpec")] source: JsValue,
    #[wasm_bindgen(unchecked_param_type = "CandidateOptions")] options: JsValue,
) -> Result<JsValue, JsValue> {
    let source: SourceSpec = required(source, "source")?;
    let options: CandidateOptions = strict(options, "candidate options")?;
    let out = fieldglass::fetchplan::candidates_at(&source, &options).map_err(throw)?;
    to_js(&out)
}
