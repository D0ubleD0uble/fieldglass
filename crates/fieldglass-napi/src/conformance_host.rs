//! This host, run through the ADR-0006 conformance suite (#573).
//!
//! The suite lives in `fieldglass` as data (`conformance/suite.json`) and its
//! reference runner drives `Session`. This one drives **the `Handle` the
//! extension actually calls** — the same dispatch its JavaScript methods run,
//! through their Rust twins — and checks it produces the same observations.
//! That is the whole point of decision 3: a rule about "hosts are thin
//! bindings" is a rule about code review until a second implementation has to
//! agree with the first.
//!
//! # What this runner covers, and what it cannot yet
//!
//! Since #1073 this host's `Handle` has the browser module's shape for opening,
//! listing and decoding, so those ops are compared call for call:
//!
//! | suite op | napi call | compared |
//! |---|---|---|
//! | `open` | `open(bytes)`, `format()`, `addressing()`, `count()` | which format accepted the bytes, how it is addressed, and the message count |
//! | `message` | `message(i)` | the whole `MessageInfo`, key for key, as the addon hands it to JavaScript (#574) |
//! | `decode` | `decode(i, { dtype })` | width, raster shape, value count, mask sum, stats, the sampled cells |
//! | `render` | `renderGrid(i, …)` with `projection: "source"` | raster shape, RGBA length, opaque count, the sampled pixels, and the colour range it reports (#1156) |
//! | `variables` | `variables()` | each variable's name, axes, element type and detected image axes, in order |
//! | `dimensions` | `metadata().dimensions` | every dimension's name and length |
//! | `left_out` | `leftOut()` | every array left out, with its reason and `cause` (#1073) |
//! | `undrawable` | `undrawable()` | every array no map is drawn from, with its reason and `cause` (#1073) |
//! | `decode_slice` | `decodeSlice({ variable, yDim, xDim, sliceIndices }, { dtype })` | as `decode` |
//! | `decode_line` | `decodeLine({ variable, alongDim, sliceIndices }, { dtype })` | the line's values, mask, labels, coordinates and stats |
//! | `decode_display` | `decodeDisplay(i, { reduce })` | raster shape, reduction, the derived `Georef`, value count, mask sum, the sampled cells (#1082) |
//! | `render_display` | `renderDisplayGrid(display, stats, …)` with `projection: "source"`, the stats `decode` returns | as `render` |
//! | the error cases | the same calls | that the call fails, and on the same input |
//!
//! `variables()` is the API's own `VariableInfo` list on both hosts since
//! #1073, compared whole, `index` and `units` included, and a slice case's
//! `variable` is that position on both: no translation between the two.
//!
//! The `decode` row is the one that has caught a real host divergence: the
//! GRIB decode used to take the raw path while every other call resolved, so a
//! spectral or HEALPix message failed here and answered everywhere else. It
//! resolves now (#580), and those cases are the first in the suite to pin the
//! synthesised grid across two bindings.
//!
//! `warp`, `palette`, `probe`, `probe_message`, `full_detail`, `contours`,
//! `combine`, `warp_display` and `palette_display` are **not** compared:
//!
//! * the probes — this host's `probePixel` reads an output *pixel*; the suite
//!   probes a geographic point. Two different questions, not two answers to
//!   one.
//! * the rest — this host's pipeline does the warp and the palette inside
//!   `renderGrid`, returns contours as projected polylines, and combines inside
//!   the same call, so there is no warped raster, palette or combined *field*
//!   to compare. The characterisation golden records `renderGrid`, `probePixel`
//!   and `projectContours` over combined targets across the whole corpus, and
//!   `combine::tests` in `fieldglass` pins the alignment gate itself.
//!
//! The row `every_comparable_op_is_actually_compared` keeps that list honest:
//! it is the *only* place the skip is stated, so an op that becomes comparable
//! and is not added fails here.
//!
//! # Why the error mapping is checked loosely
//!
//! `Error::code()` is the stable half of the API contract. The `Handle` throws
//! an API refusal with its `code`, as the browser module does, but a few
//! refusals are this host's own prose (a variable index the NetCDF reader does
//! not list, a closed handle) and carry none. So this runner asserts that the
//! same input fails, not that it fails with the same code. That is recorded as
//! `THE_NAPI_ERROR_MAPPING_LOSES_THE_CODE` rather than left as a silent gap.

use super::*;
use fieldglass::conformance::{self, Args, Case, Op, RecordedCase, Tolerance, sample_indices};
use serde_json::{Value, json};

/// Some of this host's refusals are its own prose with no `code`, so a JS
/// caller cannot always branch on `fieldglass::Error::code()` the way the
/// wasm host's `e.code` lets one.
///
/// Named rather than described so the gap is greppable from the issue that
/// closes it.
const THE_NAPI_ERROR_MAPPING_LOSES_THE_CODE: &str =
    "some napi refusals are prose with no code; the suite compares failure, not the code";

/// The suite ops this runner compares, and the ops it skips with the reason.
///
/// Every op in the suite must appear in exactly one of the two lists, which is
/// what stops an op quietly falling out of coverage.
const COMPARED: &[Op] = &[
    Op::Open,
    Op::Message,
    Op::Decode,
    Op::Render,
    Op::Variables,
    Op::Dimensions,
    Op::LeftOut,
    Op::Undrawable,
    Op::DecodeSlice,
    Op::DecodeLine,
    // A field decoded to draw (#1082): `decodeDisplay`, and its render through
    // `renderDisplayGrid` in the source projection.
    Op::DecodeDisplay,
    Op::RenderDisplay,
];

/// Skipped ops and why — see the module docs for the long form.
const SKIPPED: &[(Op, &str)] = &[
    (Op::Warp, "napi has no warp-without-paint operation"),
    (Op::Palette, "napi has no palette-as-data operation"),
    (
        Op::Probe,
        "napi's `probePixel` reads a pixel, the suite probes a point",
    ),
    (
        Op::ProbeMessage,
        "napi's `probePixel` reads a pixel, the suite probes a point; its two-value spectral \
         probe (#637) is pinned by `lib.rs` tests and the characterisation golden's T383 rows",
    ),
    (
        Op::FullDetail,
        "napi's `probePixel` reads a pixel, the suite probes a point; its full-detail value \
         rides on the pixel probe, pinned with `ProbeMessage`'s",
    ),
    (
        Op::Contours,
        "napi returns projected polylines, not grid-space isolines",
    ),
    (
        Op::Combine,
        "napi has no combine-without-render operation; `renderGrid` paints a combined target \
         in the same call (#574)",
    ),
    (
        Op::WarpDisplay,
        "napi has no warp-without-paint operation; `renderDisplayGrid` warps inline on a \
         reprojection target, as `renderGrid` does",
    ),
    (
        Op::PaletteDisplay,
        "napi has no palette-as-data operation, for a display grid or any other",
    ),
];

/// Read a fixture the way the suite names one: relative to `crates/`.
fn read(fixture: &str) -> Vec<u8> {
    let path = format!("../{fixture}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
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
        range_center: None,
        bounds_lat_min: None,
        bounds_lat_max: None,
        bounds_lon_min: None,
        bounds_lon_max: None,
        colormap: args.colormap.clone(),
        colormap_table: args.colormap_table.clone(),
        reverse_colormap: args.reversed,
        scale_mode: args.scale.clone(),
        width: None,
        height: None,
    }
}

/// `decode`'s options for a case, as JavaScript would send them.
fn decode_options(args: &Args) -> Option<Value> {
    Some(
        args.decode_options
            .clone()
            .unwrap_or_else(|| json!({ "dtype": args.dtype })),
    )
}

/// `decodeDisplay`'s options for a case, as JavaScript would send them.
fn display_options(args: &Args) -> Option<Value> {
    Some(json!({ "reduce": args.reduce.unwrap_or(0), "dtype": args.dtype }))
}

/// A decoded field in the shape `fieldglass::conformance`'s field observation
/// takes, for the keys this host's object answers.
fn decoded_value(grid: &DecodedGrid) -> Value {
    let values: Vec<f64> = match &grid.values {
        napi::Either::A(v) => v.iter().map(|&x| f64::from(x)).collect(),
        napi::Either::B(v) => v.to_vec(),
    };
    let mask: &[u8] = &grid.mask;
    let samples: Vec<Value> = sample_indices(values.len())
        .into_iter()
        .map(|i| {
            let present = mask.get(i).copied().unwrap_or(0) == 1;
            json!({
                "i": i,
                // A masked cell's value is not data; read the mask first, the
                // way the DTO's own doc tells a host to.
                "v": present.then(|| values[i]),
                "m": usize::from(present),
            })
        })
        .collect();
    json!({
        "dtype": grid.dtype,
        "len": values.len(),
        "maskLen": mask.len(),
        "maskOnes": mask.iter().filter(|&&m| m == 1).count(),
        "ni": grid.ni,
        "nj": grid.nj,
        "stats": grid.stats,
        "samples": samples,
    })
}

/// A painted raster in the suite's render observation shape.
fn raster_value(raster: &RenderedGrid) -> Value {
    let rgba: &[u8] = &raster.rgba;
    let pixels: Vec<Value> = sample_indices(rgba.len() / 4)
        .into_iter()
        .map(|p| json!({ "i": p, "rgba": rgba[p * 4..p * 4 + 4].to_vec() }))
        .collect();
    json!({
        "width": raster.width,
        "height": raster.height,
        "rgbaLen": rgba.len(),
        "opaque": rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).count(),
        "pixels": pixels,
        // The range the render reports (#1156), which the browser host reports
        // too: in the source projection a pixel is a cell, so the counts are
        // the source grid's, as `Session::render`'s are.
        "range": raster.range,
    })
}

/// What this host answers for one case, in the suite's own observation shape.
///
/// `None` means "this op is not comparable through this binding"; a failure
/// becomes the error observation, exactly as the reference runner's does.
fn observe(case: &Case) -> Option<Value> {
    if !COMPARED.contains(&case.op) || !asks_the_same_question(case) {
        return None;
    }
    let bytes = read(&case.fixture);
    let bytes = match case.args.truncate {
        Some(n) => &bytes[..n.min(bytes.len())],
        None => &bytes[..],
    };

    let Ok(handle) = crate::Handle::from_vec(bytes.to_vec()) else {
        return Some(failed());
    };
    let args = &case.args;

    let observed = match case.op {
        Op::Open => (|| -> Result<Value, RenderError> {
            Ok(json!({
                "format": handle.format()?,
                "addressing": handle.addressing()?,
                "count": handle.count()?,
            }))
        })(),
        Op::Message => handle.message(args.index).map(|mut info| {
            // The suite leaves `geometry` out of every `Georef` it records: it
            // is `core`'s tagged enum and outside the host contract.
            if let Some(grid) = info.get_mut("grid").and_then(Value::as_object_mut) {
                grid.remove("geometry");
            }
            info
        }),
        Op::Decode => handle
            .decode(args.index, decode_options(args))
            .map(|grid| decoded_value(&grid)),
        Op::Render => handle
            .render_grid(json!(args.index), render_options(args))
            .map(|raster| raster_value(&raster)),
        Op::Variables => handle
            .variables()
            .and_then(|variables| Ok(to_js(serde_json::to_value(variables))?)),
        Op::Dimensions => handle
            .metadata()
            .map(|meta| axes(meta.dimensions.iter().map(|d| (d.name.as_str(), d.length)))),
        Op::LeftOut => handle.left_out(),
        Op::Undrawable => handle.undrawable(),
        Op::DecodeSlice => {
            let variable = args.variable;
            handle
                .decode_slice(
                    json!({
                        "variable": variable,
                        "yDim": args.y_dim,
                        "xDim": args.x_dim,
                        "sliceIndices": args.slice_indices,
                    }),
                    decode_options(args),
                )
                .map(|grid| decoded_value(&grid))
        }
        Op::DecodeLine => {
            let variable = args.variable;
            handle
                .decode_line(
                    json!({
                        "variable": variable,
                        "alongDim": args.along_dim,
                        "sliceIndices": args.slice_indices,
                    }),
                    decode_options(args),
                )
                .map(|line| line_value(&line))
        }
        Op::DecodeDisplay => handle
            .decode_display(args.index, display_options(args))
            .and_then(|display| display_value(&display)),
        Op::RenderDisplay => (|| -> Result<Value, RenderError> {
            let display = handle.decode_display(args.index, display_options(args))?;
            // The stats a caller holds from its own full decode: `decode`'s.
            let stats = if args.full_stats == Some(false) {
                None
            } else {
                Some(handle.decode(args.index, decode_options(args))?.stats)
            };
            let raster = handle.render_display_grid(&display, stats, &render_options(args))?;
            Ok(raster_value(&raster))
        })(),
        // `COMPARED` gates the entry, so nothing else reaches here. Written as
        // an explicit arm rather than a wildcard so that adding an op to
        // `COMPARED` without adding its adapter fails to compile.
        Op::Warp
        | Op::Palette
        | Op::Probe
        | Op::ProbeMessage
        | Op::FullDetail
        | Op::Contours
        | Op::Combine
        | Op::WarpDisplay
        | Op::PaletteDisplay => return None,
    };
    Some(observed.unwrap_or_else(|e| match e.api() {
        // An API refusal crosses with its code (#1073), so the code is
        // compared; this host's own prose has none, and is compared as failed.
        Some(api) => {
            json!({ "error": { "code": api.code(), "hasMessage": !api.message().is_empty() } })
        }
        None => failed(),
    }))
}

/// A line in `fieldglass::conformance::line_value`'s shape, less `dtype`: this
/// host hands every line back as numbers and reports no width, so it has no
/// honest answer to give. The library and browser runners still hold `dtype`
/// to the recording. Every other field is compared, values and coordinates
/// included.
fn line_value(line: &LineResult) -> Value {
    let real = |v: Option<f64>| match v {
        None => Value::Null,
        Some(x) if x.is_finite() => json!(x),
        Some(_) => json!("nonFinite"),
    };
    let points: Vec<Value> = line
        .values
        .iter()
        .zip(&line.mask)
        .map(|(&v, &m)| real((m == 1).then_some(v)))
        .collect();
    let valid = line.mask.iter().filter(|&&m| m == 1).count();
    json!({
        "len": line.values.len(),
        "maskLen": line.mask.len(),
        "maskOnes": valid,
        "variable": line.variable,
        "units": line.units,
        "dimension": line.dimension,
        "coordinates": line
            .coordinates
            .as_ref()
            .map(|c| c.iter().map(|&x| real(Some(x))).collect::<Vec<_>>()),
        "coordinateUnits": line.coordinate_units,
        "stats": { "min": line.min, "max": line.max, "validCount": valid },
        "points": points,
    })
}

/// A display grid in the suite's display observation shape.
fn display_value(display: &DisplayGrid) -> Result<Value, RenderError> {
    let samples: Vec<Value> = sample_indices(display.values.len())
        .into_iter()
        .map(|i| {
            let v = display.values[i];
            json!({ "i": i, "v": v, "m": usize::from(v.is_some()) })
        })
        .collect();
    let mut georef = display.grid()?;
    if let Some(obj) = georef.as_object_mut() {
        obj.remove("geometry");
    }
    Ok(json!({
        "len": display.values.len(),
        "maskOnes": display.values.iter().flatten().count(),
        "ni": display.ni()?,
        "nj": display.nj()?,
        "reduction": display.reduction()?,
        "georef": georef,
        "samples": samples,
    }))
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
/// **A short-serving source (#707, #709).** A case with `short_read` set opens
/// through `Session::open_source` over a source that serves fewer bytes than it
/// was asked for. This host's `open` takes the bytes — the addon reads the file
/// itself — so it has no transport and cannot be in a truncated-transfer state
/// at all; asking it that question would compare a whole-file decode with a
/// truncated one and call the difference a divergence. When this host gains a
/// source-taking open (#114's large files), the case becomes comparable and
/// this check should go.
///
/// Every other case is the same question since #1073: `decode`,
/// `decodeSlice` and `decodeDisplay` take the suite's `dtype`.
fn asks_the_same_question(case: &Case) -> bool {
    case.args.short_read.is_none()
}

/// The shape this runner records for a failed call: that it failed, and
/// nothing about how, because `THE_NAPI_ERROR_MAPPING_LOSES_THE_CODE`.
fn failed() -> Value {
    json!({ "failed": true })
}

/// Reduce a reference observation to the keys this runner produces, so the two
/// are compared on exactly the ground they share.
///
/// An error observation on either side collapses to `failed`. Everything else
/// is projected key by key — a key this host does not answer is dropped, and a
/// key it answers that the reference does not is a *failure*, not a skip.
///
/// Recursive, and through arrays element by element: a variable list is an
/// array of objects, and this host answers fewer keys on each element than the
/// API does (see the module docs). Arrays of different lengths are left whole,
/// so the comparison reports the length rather than a projection of it.
fn projected(reference: &Value, observed: &Value) -> Value {
    if reference.get("error").is_some() {
        // Compared whole when this host answered with a code, and as a bare
        // failure when it answered in its own words.
        return if observed.get("error").is_some() {
            reference.clone()
        } else {
            failed()
        };
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
        let Some(observed) = observe(case) else {
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
        .filter(|r| COMPARED.contains(&r.case.op) && asks_the_same_question(&r.case))
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
