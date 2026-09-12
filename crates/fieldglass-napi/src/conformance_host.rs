//! This host, run through the ADR-0006 conformance suite (#573).
//!
//! The suite lives in `fieldglass` as data (`conformance/suite.json`) and its
//! reference runner drives `Session`. This one drives **the napi handles the
//! extension actually calls** and checks they produce the same observations.
//! That is the whole point of decision 3: a rule about "hosts are thin
//! bindings" is a rule about code review until a second implementation has to
//! agree with the first.
//!
//! # What this runner covers, and what it cannot yet
//!
//! `fieldglass-napi` binds a *different shape* from the browser host, and #464
//! is what closes the gap. It has no `Session`; its three handles decode by
//! index, probe by pixel rather than by latitude and longitude, and describe a
//! message with `MessageMeta` rather than with `MessageInfo`. So the runner
//! takes the operations whose answers are directly comparable today:
//!
//! | suite op | napi call | compared |
//! |---|---|---|
//! | `open` | `Grib1Handle::from_bytes` / `Grib2Handle::from_bytes`, `messages()` | which format accepted the bytes, and the message count |
//! | `decode` | `decode_grid(i)` | raster shape, value count, mask sum, the sampled cells |
//! | `render` | `render_grid(i, …)` with `projection: "source"` | raster shape, RGBA length, opaque count, the sampled pixels |
//! | `variables` | `NetcdfHandle::variables()` | each variable's name, axes, element type and detected image axes, in order |
//! | `dimensions` | `NetcdfHandle::metadata().dimensions` | every dimension's name and length |
//! | `decode_slice` | `NetcdfHandle::render_slice(…)` with `projection: "source"` | the slice's raster shape and how many of its cells are present |
//! | the error cases | the same calls | that the call fails, and on the same input |
//!
//! The variable ops (#679) reach NetCDF through the handle the extension uses
//! for it, which answers in its own shape, so two things the API answers are
//! left out, each on purpose:
//!
//! * a variable's `units` — this host typesets them for display (ADR-0007) and
//!   the API hands over the file's own spelling, so the two differ by design;
//! * a variable's `index` — this host numbers variables by their place in the
//!   file, the API by their place in the list it offers. Both lists are
//!   compared in order, so the list position is checked anyway.
//!
//! And `decode_slice` is compared through a render because this host has no
//! call that hands back a slice's values: in the source projection a present
//! cell paints one opaque pixel and a masked one paints none, so the raster
//! shape and the opaque count are the slice's shape and presence, and nothing
//! about the numbers in it. The characterisation golden pins those.
//!
//! The `decode` row is the one that has caught a real host divergence:
//! `decode_grid` used to take the raw path while every other call on the
//! handles resolved, so a spectral or HEALPix message failed here and answered
//! everywhere else. It resolves now (#580), and those cases are the first in
//! the suite to pin the synthesised grid across two bindings.
//!
//! `message`, `warp`, `palette`, `probe` and `contours` are **not** compared,
//! and each for a reason that is a task rather than an oversight:
//!
//! * `message` — napi answers `MessageMeta`, whose field names differ. #574
//!   deletes it, and the comparison becomes free at that point.
//! * `probe` — napi probes an output *pixel*; the suite probes a geographic
//!   point. Two different questions, not two answers to one.
//! * `warp`, `palette`, `contours`, `combine` — napi exposes no operation with
//!   these shapes; its render does the warp inline, its contours come back as
//!   projected polylines, and its combine paints in the same call, so there is
//!   no combined *field* to compare. That last one is covered instead by the
//!   characterisation golden, which records `renderGridCombined`,
//!   `probeCombined` and `projectContoursCombined` over the whole corpus, and
//!   by `combine::tests` in `fieldglass` for the alignment gate itself.
//!
//! Writing an adapter for those would mean writing the code #464 is deleting.
//! The row `every_comparable_op_is_actually_compared` keeps that list honest:
//! it is the *only* place the skip is stated, so an op that becomes comparable
//! and is not added fails here.
//!
//! # Why the error mapping is checked loosely
//!
//! `Error::code()` is the stable half of the API contract, and this host
//! **drops it**: `IntoNapi` renders the error's `Display` into a
//! `napi::Error`'s reason and the code goes nowhere. So the strongest thing
//! this runner can assert today is that the same input fails, not that it fails
//! with the same code. That is recorded as
//! [`THE_NAPI_ERROR_MAPPING_LOSES_THE_CODE`] rather than left as a silent gap,
//! and it is what a host binding has to fix to pass the suite in full.

use super::*;
use fieldglass::conformance::{self, Args, Case, Op, RecordedCase, Tolerance, sample_indices};
use serde_json::{Value, json};

/// This host maps an API error onto a `napi::Error`, which carries a reason
/// string and no code. A JS caller therefore cannot branch on
/// `fieldglass::Error::code()` the way the wasm host's `e.code` lets one.
///
/// Named rather than described so the gap is greppable from the issue that
/// closes it (#574 collapses this crate onto `fieldglass::Session`, at which
/// point the mapping has one place to live).
const THE_NAPI_ERROR_MAPPING_LOSES_THE_CODE: &str =
    "napi::Error carries a reason, not a code; the suite compares failure, not the code";

/// The suite ops this runner compares, and the ops it skips with the reason.
///
/// Every op in the suite must appear in exactly one of the two lists, which is
/// what stops an op quietly falling out of coverage.
const COMPARED: &[Op] = &[
    Op::Open,
    Op::Decode,
    Op::Render,
    Op::Variables,
    Op::Dimensions,
    Op::DecodeSlice,
];

/// Skipped ops and why — see the module docs for the long form.
const SKIPPED: &[(Op, &str)] = &[
    (
        Op::Message,
        "napi answers MessageMeta, not MessageInfo (#574)",
    ),
    (Op::Warp, "napi has no warp-without-paint operation"),
    (Op::Palette, "napi has no palette-as-data operation"),
    (Op::Probe, "napi probes a pixel, the suite probes a point"),
    (
        Op::Contours,
        "napi returns projected polylines, not grid-space isolines",
    ),
    (
        Op::Combine,
        "napi has no combine-without-render operation; `renderGridCombined` \
         paints in the same call (#574)",
    ),
];

/// Read a fixture the way the suite names one: relative to `crates/`.
fn read(fixture: &str) -> Vec<u8> {
    let path = format!("../{fixture}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// One open file, through whichever handle accepted the bytes.
enum Handle {
    Grib1(Grib1Handle),
    Grib2(Grib2Handle),
    // Boxed: a NetCDF handle carries its dataset view and two caches, several
    // times the size of a GRIB one.
    Netcdf(Box<NetcdfHandle>),
}

impl Handle {
    /// Open exactly the way the extension does: take the handle for the format
    /// the bytes declare, and report a failure rather than falling through to
    /// another reader.
    fn open(bytes: &[u8]) -> Result<(Self, &'static str), String> {
        match fieldglass_core::detect_from_bytes(bytes) {
            fieldglass_core::Format::Grib1 => Grib1Handle::from_bytes(bytes.to_vec().into())
                .map(|h| (Self::Grib1(h), "grib1"))
                .map_err(|e| e.to_string()),
            fieldglass_core::Format::Grib2 => Grib2Handle::from_bytes(bytes.to_vec().into())
                .map(|h| (Self::Grib2(h), "grib2"))
                .map_err(|e| e.to_string()),
            fieldglass_core::Format::NetCdf => NetcdfHandle::from_bytes(bytes.to_vec().into())
                .map(|h| (Self::Netcdf(Box::new(h)), "netcdf"))
                .map_err(|e| e.to_string()),
            other => Err(format!("not a container this host opens: {other:?}")),
        }
    }

    /// How many messages the file holds. `None` for NetCDF: this host opens
    /// it with a handle that has no message list at all, rather than one that
    /// answers zero, so there is no count to compare.
    fn count(&self) -> Option<usize> {
        match self {
            Self::Grib1(h) => Some(h.messages().len()),
            Self::Grib2(h) => Some(h.messages().len()),
            Self::Netcdf(_) => None,
        }
    }

    /// Which of the two ways the file is addressed. On this host that is which
    /// handle opened it, which is exactly the choice the extension routes on.
    fn addressing(&self) -> &'static str {
        match self {
            Self::Grib1(_) | Self::Grib2(_) => "messages",
            Self::Netcdf(_) => "variables",
        }
    }

    fn decode_grid(&self, index: u32) -> napi::Result<DecodedGrid> {
        match self {
            Self::Grib1(h) => h.decode_grid(index),
            Self::Grib2(h) => h.decode_grid(index),
            Self::Netcdf(_) => Err(no_messages()),
        }
    }

    fn render_grid(&self, index: u32, options: RenderOptions) -> napi::Result<RenderedGrid> {
        match self {
            Self::Grib1(h) => h.render_grid(index, options),
            Self::Grib2(h) => h.render_grid(index, options),
            Self::Netcdf(_) => Err(no_messages()),
        }
    }
}

/// What asking a message question of the NetCDF handle comes to on this host:
/// it has no such call, so the question fails, which is what the API's
/// `wrong_addressing` says too.
fn no_messages() -> napi::Error {
    napi::Error::from_reason("a NetCDF handle holds variables, not messages")
}

/// Asking a variable question of a GRIB handle, the other way round.
fn no_variables() -> Value {
    failed()
}

/// The suite's palette knobs, as this host's `RenderOptions` states them.
///
/// Every field is written out, matching the suite's own rule: a case whose
/// meaning moved when a default moved would fail on the wrong commit.
fn render_options(args: &Args) -> RenderOptions {
    RenderOptions {
        projection: "source".to_string(),
        projection_preset: None,
        center_lat: None,
        center_lon: None,
        resampling: "nearest".to_string(),
        flip_y: args.flip_y.unwrap_or(false),
        range_min: args.range_min,
        range_max: args.range_max,
        bounds_lat_min: None,
        bounds_lat_max: None,
        bounds_lon_min: None,
        bounds_lon_max: None,
        colormap: args.colormap.clone(),
        reverse_colormap: args.reversed,
        scale_mode: args.scale.clone(),
        width: None,
        height: None,
    }
}

/// What this host answers for one case, in the suite's own observation shape.
///
/// `None` means "this op is not comparable through this binding"; a failure
/// becomes the error observation, exactly as the reference runner's does.
///
/// `expect` is the recorded observation, consulted for one thing only — see
/// [`asks_the_same_question`].
fn observe(case: &Case, expect: &Value) -> Option<Value> {
    if !COMPARED.contains(&case.op) || !asks_the_same_question(case, expect) {
        return None;
    }
    let bytes = read(&case.fixture);
    let bytes = match case.args.truncate {
        Some(n) => &bytes[..n.min(bytes.len())],
        None => &bytes[..],
    };

    let (handle, format) = match Handle::open(bytes) {
        Ok(v) => v,
        Err(_) => return Some(failed()),
    };

    match case.op {
        Op::Open => {
            let mut open = json!({ "format": format, "addressing": handle.addressing() });
            if let Some(count) = handle.count() {
                open["count"] = json!(count);
            }
            Some(open)
        }
        Op::Decode => {
            let Ok(grid) = handle.decode_grid(case.args.index) else {
                return Some(failed());
            };
            let values: &[f64] = &grid.values;
            let mask: &[u8] = &grid.mask;
            let samples: Vec<Value> = sample_indices(values.len())
                .into_iter()
                .map(|i| {
                    json!({
                        "i": i,
                        // A masked cell is `f64::NAN` on this side and has no
                        // value at all on the API's; read the mask first, the
                        // way the DTO's own doc tells a host to.
                        "v": (mask.get(i).copied().unwrap_or(0) == 1).then(|| values[i]),
                        "m": usize::from(mask.get(i).copied().unwrap_or(0)),
                    })
                })
                .collect();
            Some(json!({
                "len": values.len(),
                "maskOnes": mask.iter().filter(|&&m| m == 1).count(),
                "ni": grid.width,
                "nj": grid.height,
                "samples": samples,
            }))
        }
        Op::Render => {
            let Ok(raster) = handle.render_grid(case.args.index, render_options(&case.args)) else {
                return Some(failed());
            };
            let rgba: &[u8] = &raster.rgba;
            let pixels: Vec<Value> = sample_indices(rgba.len() / 4)
                .into_iter()
                .map(|p| json!({ "i": p, "rgba": rgba[p * 4..p * 4 + 4].to_vec() }))
                .collect();
            Some(json!({
                "width": raster.width,
                "height": raster.height,
                "rgbaLen": rgba.len(),
                "opaque": rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).count(),
                "pixels": pixels,
            }))
        }
        Op::Variables => {
            let Handle::Netcdf(h) = &handle else {
                return Some(no_variables());
            };
            let variables: Vec<Value> = h
                .variables()
                .iter()
                .map(|v| {
                    json!({
                        "name": v.name,
                        "dims": axes(v.dims.iter().map(|d| (d.name.as_str(), d.length))),
                        "dtype": v.nc_type,
                        "detectedYDim": v.detected_y_dim,
                        "detectedXDim": v.detected_x_dim,
                    })
                })
                .collect();
            Some(Value::Array(variables))
        }
        Op::Dimensions => {
            let Handle::Netcdf(h) = &handle else {
                return Some(no_variables());
            };
            let meta = h.metadata();
            Some(axes(
                meta.dimensions.iter().map(|d| (d.name.as_str(), d.length)),
            ))
        }
        Op::DecodeSlice => {
            let Handle::Netcdf(h) = &handle else {
                return Some(no_variables());
            };
            let args = &case.args;
            let (Some(position), Some(y_dim), Some(x_dim), Some(indices)) = (
                args.variable,
                args.y_dim,
                args.x_dim,
                args.slice_indices.clone(),
            ) else {
                return Some(failed());
            };
            // The API's `variable` is a position in the list it offers; this
            // host's `render_slice` takes the variable's place in the file.
            // The two lists are the same variables in the same order (the
            // `variables` cases check that), so the position translates.
            let Some(variable) = h
                .variables()
                .get(position as usize)
                .map(|v| v.variable_index)
            else {
                return Some(failed());
            };
            let Ok(raster) = h.render_slice(
                u32::try_from(variable).unwrap_or(u32::MAX),
                y_dim,
                x_dim,
                indices,
                render_options(args),
            ) else {
                return Some(failed());
            };
            let rgba: &[u8] = &raster.rgba;
            Some(json!({
                "ni": raster.width,
                "nj": raster.height,
                "len": rgba.len() / 4,
                // Source projection, nearest resampling: one pixel per cell,
                // opaque exactly where the cell is present.
                "maskOnes": rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).count(),
            }))
        }
        // `COMPARED` gates the entry, so nothing else reaches here. Written as
        // an explicit arm rather than a wildcard so that adding an op to
        // `COMPARED` without adding its adapter fails to compile.
        Op::Message | Op::Warp | Op::Palette | Op::Probe | Op::Contours | Op::Combine => None,
    }
}

/// A list of named axes in the shape `DimensionInfo` serialises to.
///
/// This host carries a length as `f64`, because napi hands JavaScript a number;
/// the suite compares a length as an integer, so it crosses back as one. A
/// length here is a dimension's size, never fractional and far below 2^53.
fn axes<'a>(axes: impl Iterator<Item = (&'a str, f64)>) -> Value {
    Value::Array(
        axes.map(|(name, length)| json!({ "name": name, "length": length as u64 }))
            .collect(),
    )
}

/// Whether this host's call is an answer to the *same question* the case asks.
///
/// Two, and both are real differences in surface rather than in numbers.
///
/// **A short-serving source (#707, #709).** A case with `short_read` set opens
/// through `Session::open_source` over a source that serves fewer bytes than it
/// was asked for. This host's handles take a JS `Buffer` — the addon reads the
/// file itself — so it has no transport and cannot be in a truncated-transfer
/// state at all; asking it that question would compare a whole-file decode with
/// a truncated one and call the difference a divergence. The same reason
/// `short_read` was unreachable through `Session` before #709 gave it a
/// source-taking constructor. When this host gains one (#659's directory store,
/// #114's large files), the case becomes comparable and this arm should go.
///
/// **A narrowed decode.** A real difference in surface too:
/// `decode_grid` has no `dtype` knob and always answers `Float64Array`, while
/// the suite's `decode` cases ask for each of `auto`, `f32` and `f64`. Where
/// the API narrowed to `f32`, the two calls decoded the same field to different
/// widths on purpose, and comparing them would be comparing a downcast with the
/// thing it was cast from.
///
/// So a decode case is comparable when the API answered `f64` — which covers
/// every `dtype: "f64"` case and every `auto` case over a field that did not
/// narrow — or when it failed, where the width never arose.
fn asks_the_same_question(case: &Case, expect: &Value) -> bool {
    if case.args.short_read.is_some() {
        return false;
    }
    if case.op != Op::Decode {
        return true;
    }
    if expect.get("error").is_some() {
        return true;
    }
    expect.get("dtype").and_then(Value::as_str) == Some("f64")
}

/// The shape this runner records for a failed call: that it failed, and
/// nothing about how, because [`THE_NAPI_ERROR_MAPPING_LOSES_THE_CODE`].
fn failed() -> Value {
    json!({ "failed": true })
}

/// Reduce a reference observation to the keys this runner produces, so the two
/// are compared on exactly the ground they share.
///
/// An error observation on either side collapses to [`failed`]. Everything else
/// is projected key by key — a key this host does not answer is dropped, and a
/// key it answers that the reference does not is a *failure*, not a skip.
///
/// Recursive, and through arrays element by element: a variable list is an
/// array of objects, and this host answers fewer keys on each element than the
/// API does (see the module docs). Arrays of different lengths are left whole,
/// so the comparison reports the length rather than a projection of it.
fn projected(reference: &Value, observed: &Value) -> Value {
    if reference.get("error").is_some() {
        return failed();
    }
    match (reference, observed) {
        (Value::Array(r), Value::Array(o)) if r.len() == o.len() => {
            Value::Array(r.iter().zip(o).map(|(r, o)| projected(r, o)).collect())
        }
        (Value::Object(r), Value::Object(o)) => {
            let mut out = serde_json::Map::new();
            for (key, observed) in o {
                if let Some(v) = r.get(key) {
                    out.insert(key.clone(), projected(v, observed));
                }
            }
            Value::Object(out)
        }
        _ => reference.clone(),
    }
}

/// Every comparable case, through this host's own binding.
#[test]
fn this_host_agrees_with_the_conformance_suite() {
    let suite = conformance::suite().expect("the shipped suite parses");
    let mut compared = 0usize;
    let mut failures = Vec::new();

    for RecordedCase { case, expect } in &suite.cases {
        let Some(observed) = observe(case, expect) else {
            continue;
        };
        compared += 1;
        let reference = projected(expect, &observed);
        // The reference side has already been reduced to this host's keys, so
        // any remaining difference is a real disagreement about a value both
        // bindings answer.
        for line in conformance::compare(&reference, &observed, suite.tolerance) {
            failures.push(format!("{}: {line}", case.id));
        }
    }

    // The exact number, derived from the suite rather than a floor: a case
    // that stops reaching this binding — because `COMPARED` shrank, or because
    // `asks_the_same_question` narrowed — is a coverage loss, and a floor with
    // slack in it is how that goes unnoticed.
    let comparable = suite
        .cases
        .iter()
        .filter(|r| COMPARED.contains(&r.case.op) && asks_the_same_question(&r.case, &r.expect))
        .count();
    assert_eq!(
        compared, comparable,
        "the runner reached {compared} of the {comparable} comparable cases"
    );
    assert!(
        comparable >= 30,
        "only {comparable} of the suite's cases are comparable through this \
         binding, which means the adapter stopped rather than that the suite shrank"
    );
    assert!(
        failures.is_empty(),
        "{} disagreement(s) between this host and the conformance suite:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Every op in the suite is either compared here or skipped with a reason, and
/// no op is in both lists.
///
/// The check that keeps the table in the module docs from drifting into
/// fiction: an op added to the suite and to neither list fails here.
#[test]
fn every_comparable_op_is_actually_compared() {
    let suite = conformance::suite().expect("the shipped suite parses");
    let skipped: Vec<Op> = SKIPPED.iter().map(|(op, _)| *op).collect();
    for RecordedCase { case, .. } in &suite.cases {
        let in_compared = COMPARED.contains(&case.op);
        let in_skipped = skipped.contains(&case.op);
        assert!(
            in_compared != in_skipped,
            "{:?} (case {}) is in {} of the two lists",
            case.op,
            case.id,
            if in_compared { "both" } else { "neither" }
        );
    }
    for (op, reason) in SKIPPED {
        assert!(!reason.is_empty(), "{op:?} is skipped with no reason");
    }
    assert!(
        !THE_NAPI_ERROR_MAPPING_LOSES_THE_CODE.is_empty(),
        "the error-mapping gap must stay stated"
    );
}

/// The comparison would notice if this host's numbers moved.
///
/// `projected` drops keys, which is exactly the operation that could quietly
/// reduce the runner to comparing nothing. This asserts it keeps the shared
/// keys, drops only the unshared ones, and that a perturbed value still fails.
#[test]
fn the_projection_keeps_what_both_hosts_answer() {
    let reference = json!({"width": 4, "height": 5, "rgbaLen": 80, "extra": 1});
    let observed = json!({"width": 4, "height": 5, "rgbaLen": 80});
    let kept = projected(&reference, &observed);
    assert_eq!(kept, observed, "a shared key must survive the projection");

    let moved = json!({"width": 4, "height": 6, "rgbaLen": 80});
    assert_eq!(
        conformance::compare(&kept, &moved, Tolerance::default()).len(),
        1,
        "a moved raster height must be a disagreement"
    );

    // An error on the reference side collapses to `failed`, and a host that
    // *succeeded* where the API failed is then a disagreement rather than a
    // silent pass.
    let errored = json!({"error": {"code": "unsupported", "hasMessage": true}});
    assert_eq!(projected(&errored, &observed), failed());
    assert!(!conformance::compare(&failed(), &observed, Tolerance::default()).is_empty());

    // Through an array of objects, element by element: the unshared key goes,
    // a moved shared value still fails, and a list of a different length is
    // not projected down to agreement.
    let reference = json!([{"name": "sst", "units": "K"}, {"name": "ice", "units": "%"}]);
    let observed = json!([{"name": "sst"}, {"name": "ice"}]);
    assert_eq!(projected(&reference, &observed), observed);
    let renamed = json!([{"name": "sst"}, {"name": "ssta"}]);
    assert_eq!(
        conformance::compare(
            &projected(&reference, &renamed),
            &renamed,
            Tolerance::default()
        )
        .len(),
        1,
        "a moved element value must be a disagreement"
    );
    let shorter = json!([{"name": "sst"}]);
    assert!(
        !conformance::compare(
            &projected(&reference, &shorter),
            &shorter,
            Tolerance::default()
        )
        .is_empty(),
        "a shorter list must not project down to agreement"
    );
}
