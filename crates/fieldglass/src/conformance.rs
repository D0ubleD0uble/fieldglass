//! The ADR-0006 conformance suite: one contract, satisfied by every host.
//!
//! ADR-0006 decision 3 asks for "fixtures and expected outputs for every
//! `Session` operation … in the `fieldglass` crate as data", with each host
//! running its own binding through them. This module is that data plus the two
//! pieces of machinery a runner needs: what each case *observes* about an
//! operation, and how an observation is compared with the recording.
//!
//! # What this is not
//!
//! It is not a second copy of `fieldglass-napi`'s characterisation golden
//! (#570). That golden pins *this build's* display output byte for byte over
//! the whole committed corpus, and it stays where it is, because it drives the
//! napi handles the extension actually calls. This suite pins the **contract
//! two bindings must both satisfy**: the wire shape of every DTO, the discrete
//! answers, the geolocated numbers to a tolerance, and the [`Error::code`] a
//! failing call reports. A host passes it by producing the same observations
//! through its own binding, in its own language.
//!
//! So the two differ in what a red run means. A diff in the golden says the
//! display path changed. A failure here says a *host* disagrees with the API,
//! or the API's wire shape moved.
//!
//! [`Error::code`]: crate::Error::code
//!
//! # The cases live in code, the expectations in data
//!
//! [`cases`] builds the case list; `conformance/suite.json` holds it together
//! with the observation each case produced when it was recorded. A runner
//! checks both halves — that the file still describes the cases the code names,
//! and that every observation still matches — so a case added without
//! re-recording fails rather than being silently skipped.
//!
//! # Exact and approximate, and why the split is where it is
//!
//! ADR-0009 measured cross-target agreement: `x86_64` glibc and
//! `wasm32-wasip1` disagree on 186 of 323,620 projected results and never by
//! more than 9.3 × 10⁻¹⁴ grid cells, while **every discrete decision agrees
//! exactly**. [`compare`] follows that finding rather than weakening
//! everything to a tolerance:
//!
//! * JSON integers — counts, lengths, raster dimensions, mask sums, RGBA bytes
//!   — are compared for equality.
//! * Strings, booleans and `null` are compared for equality, which is what
//!   pins every error code, every DTO field name and every `Some`/`None`-shaped
//!   answer.
//! * Only real-valued leaves take [`Tolerance`], and the default is eight
//!   orders of magnitude tighter than ADR-0009's own 10⁻⁵ grid cells.
//!
//! An observation is therefore built so that anything discrete crosses as a
//! JSON integer. A count emitted as `3.0` would be compared loosely, which is
//! the mistake this note exists to prevent.
//!
//! # Recording
//!
//! ```sh
//! FIELDGLASS_UPDATE_CONFORMANCE=1 cargo test -p fieldglass --features conformance conformance
//! ```
//!
//! That run fails on purpose after writing, for the reason the characterisation
//! golden does: a run that re-recorded has verified nothing.

use serde_json::{Value, json};

use crate::api::{Dtype, Field};
use crate::error::Error;
use crate::session::{DecodeOptions, PaletteOptions, Session, WarpOptions};

/// The recorded suite, as it ships in the crate.
///
/// A host that cannot link Rust reads this file directly — the wasm host's Node
/// runner does — so the format is plain JSON with no Rust in the loop.
pub const SUITE_JSON: &str = include_str!("../conformance/suite.json");

/// How close two real-valued leaves have to be.
///
/// `|observed - expected| <= absolute + relative * |expected|`. Both numbers
/// are recorded in the suite file so a host in another language reads the same
/// contract rather than inventing its own epsilon.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tolerance {
    /// Scale-free part, applied to `|expected|`.
    pub relative: f64,
    /// Floor, for expectations at or near zero.
    pub absolute: f64,
}

impl Default for Tolerance {
    /// ADR-0009 measured 9.3 × 10⁻¹⁴ grid cells of spread and set the planar
    /// golden's bound at 10⁻⁵ cells. This is tighter again by four orders of
    /// magnitude, which the measurement says the *discrete* half does not need
    /// at all and the continuous half clears comfortably. Tight on purpose: a
    /// loose tolerance here would hide a host that rounded, truncated, or lost
    /// a `f64` to a `f32` on the way across its boundary.
    fn default() -> Self {
        Self {
            relative: 1e-9,
            absolute: 1e-9,
        }
    }
}

/// Which operation a case exercises.
///
/// The list is `Session`'s, minus the operations no second host binds:
/// `fieldglass::render`'s pixel probe, overlay projection and CSV are called by
/// `fieldglass-napi` alone, so a suite case over them would have one runner and
/// prove nothing about agreement. The characterisation golden covers those.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    /// [`Session::open`] plus `format` and `count`.
    Open,
    /// [`Session::message`].
    Message,
    /// [`Session::decode`].
    Decode,
    /// [`Session::warp`].
    Warp,
    /// [`Session::palette`].
    Palette,
    /// [`Session::render`].
    Render,
    /// [`Session::probe`].
    Probe,
    /// [`Session::contours`].
    Contours,
}

/// Everything a runner needs to reproduce one call.
///
/// Every knob is stated, never defaulted: a case whose meaning changed when a
/// `Default` moved would fail on the wrong commit.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Args {
    /// Message index for the operations that take one.
    pub index: u32,
    /// Keep only this many leading bytes of the fixture before opening it.
    /// `None` opens the whole file. The corrupt-input cases use it, so the
    /// suite needs no committed broken fixture.
    pub truncate: Option<usize>,
    /// Which width to decode into. Typed rather than a loose string so a typo
    /// in a case fails to *parse* the suite; a runner input that came back as
    /// `Error::InvalidOption` would be indistinguishable from the API refusing
    /// a caller's option, which is a code the suite claims to have proved
    /// reachable.
    pub dtype: Option<Dtype>,
    /// [`WarpOptions::bilinear`].
    pub bilinear: Option<bool>,
    /// [`WarpOptions::bounds`], and the render window of nothing else.
    pub bounds: Option<[f64; 4]>,
    /// [`PaletteOptions::colormap`].
    pub colormap: Option<String>,
    /// [`PaletteOptions::reversed`].
    pub reversed: Option<bool>,
    /// [`PaletteOptions::scale`].
    pub scale: Option<String>,
    /// [`PaletteOptions::min`].
    pub range_min: Option<f64>,
    /// [`PaletteOptions::max`].
    pub range_max: Option<f64>,
    /// The `flip_y` argument of [`Session::render`].
    pub flip_y: Option<bool>,
    /// Latitude for [`Session::probe`].
    pub lat: Option<f64>,
    /// Longitude for [`Session::probe`].
    pub lon: Option<f64>,
    /// Explicit contour levels; empty asks for the automatic set.
    pub levels: Option<Vec<f64>>,
}

/// One case: an operation on a fixture, and what it produced.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Case {
    /// Unique, stable, and readable in a failure message.
    pub id: String,
    /// The fixture, relative to `crates/`. A Rust runner prefixes `../`, the
    /// Node runner prefixes `crates/`; neither has to know where the other
    /// runs from.
    pub fixture: String,
    /// Which operation.
    pub op: Op,
    /// The call's inputs.
    pub args: Args,
}

/// A case together with the observation recorded for it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedCase {
    /// The case itself.
    #[serde(flatten)]
    pub case: Case,
    /// What [`observe`] answered when this was recorded.
    pub expect: Value,
}

/// The suite as `conformance/suite.json` holds it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suite {
    /// How close a real-valued leaf has to be.
    pub tolerance: Tolerance,
    /// Every [`crate::Error`] code this build can report, sorted.
    ///
    /// Pinned here rather than only inside the cases so that a variant added,
    /// renamed or removed in `Error` is a diff in the shipped contract. The
    /// cases below then prove each of these codes is actually reachable
    /// through a host binding.
    pub error_codes: Vec<String>,
    /// The recorded cases.
    pub cases: Vec<RecordedCase>,
}

/// The shipped suite, parsed.
///
/// # Errors
///
/// When `conformance/suite.json` is not valid JSON for [`Suite`], which means
/// the recording and this build's types have diverged.
pub fn suite() -> Result<Suite, serde_json::Error> {
    serde_json::from_str(SUITE_JSON)
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// GRIB2 fixtures, relative to `crates/`.
const G2: &str = "fieldglass-grib2/tests/fixtures/";
/// GRIB1 fixtures, relative to `crates/`.
const G1: &str = "fieldglass-grib1/tests/fixtures/";

/// The fixtures every operation is run over, and why each is here.
///
/// One per *answer shape* rather than one per file: the point is that two
/// bindings agree about each shape, not that the corpus is enumerated (the
/// characterisation golden enumerates it). Between them these cover a
/// degrees-affine grid and a metres-affine one, a family with no row spacing at
/// all, a periodic grid whose contour seam nonetheless does not wrap, a grid
/// stored south-to-north, a synthesised grid, and a grid that cannot be placed.
const SUBJECTS: &[(&str, &str)] = &[
    // Plain lat/lon, simple packing: the ordinary case, and the one every
    // other answer is read against.
    ("latlon", "regular_latlon_surface.grib2"),
    // Gaussian rows are not uniformly spaced, so `dy` is absent. A host that
    // invented one would misplace every row but the middle.
    ("gaussian", "regular_gaussian_f32.grib2"),
    // A projected family: `axisUnits` is metres and the affine is in the
    // projection plane, not in degrees. Also `jScansPositively` — it and
    // `grib1_polar` are the two subjects whose render cases pin the row flip
    // `Session::render` composes (#573).
    ("lambert", "eta_lambert_msg0.grib2"),
    // The second projected family, and the one whose inverse is least like
    // Lambert's.
    ("tmerc", "transverse_mercator_ukv.grib2"),
    // Periodic in x, but its contour seam does not wrap and its rows are small
    // circles — the three-answers-not-one case #571 fixed.
    ("rotated", "rotated_latlon_surface.grib2"),
    // Dx = Dy = 0: the geometry places no point, so this is the refusal shape.
    // Decode still succeeds; it is warp and probe that have nothing to say.
    ("degenerate", "polar_stereographic_surface.grib2"),
];

/// GRIB1 subjects, kept separate because the fixture directory differs.
const SUBJECTS_G1: &[(&str, &str)] = &[
    // GRIB1, and a message whose points are stored `j`-consecutive — the
    // decoder transposes it, so `scan.jConsecutive` is descriptive and the
    // raster is already row-major by the time a host sees it.
    ("grib1_jcons", "j_consecutive_latlon.grib1"),
    // GRIB1 polar stereographic with real spacings: the projected family that
    // is not Lambert, the one `decode_and_colour` already geolocates, and the
    // second `jScansPositively` subject (see `lambert` above).
    ("grib1_polar", "cmc_wind_300_2010052400_p012.grib"),
];

/// The four geographic points every field is probed at.
///
/// Chosen so that each of the subjects above answers at least one of them with
/// a hit and at least one with a miss: `null` from `probe` is a recorded result
/// here, not an untested path. The two poles are included because they are
/// where a conformal inverse is least well conditioned.
const PROBE_POINTS: [(f64, f64); 4] = [(0.0, 0.0), (45.0, -100.0), (89.5, 10.0), (-89.5, -170.0)];

/// Every case the suite runs, in the order it records them.
///
/// Built in code rather than read from the file so that adding a case is a
/// source change a reviewer sees, and so a recording that has lost one fails
/// the case-list check instead of quietly covering less.
#[must_use]
pub fn cases() -> Vec<Case> {
    let mut out = Vec::new();
    let subjects: Vec<(String, String)> = SUBJECTS
        .iter()
        .map(|(tag, file)| ((*tag).to_string(), format!("{G2}{file}")))
        .chain(
            SUBJECTS_G1
                .iter()
                .map(|(tag, file)| ((*tag).to_string(), format!("{G1}{file}"))),
        )
        .collect();

    for (tag, fixture) in &subjects {
        let mut push = |suffix: &str, op: Op, args: Args| {
            out.push(Case {
                id: format!("{tag}/{suffix}"),
                fixture: fixture.clone(),
                op,
                args,
            });
        };

        push("open", Op::Open, Args::default());
        push("message", Op::Message, Args::default());

        // All three dtypes: `auto` is the contract's own rule (narrow only
        // what survives the round trip), and the two explicit widths are the
        // downcast a host asks for by name.
        for (name, dtype) in [
            ("auto", Dtype::Auto),
            ("f32", Dtype::F32),
            ("f64", Dtype::F64),
        ] {
            push(
                &format!("decode/{name}"),
                Op::Decode,
                Args {
                    dtype: Some(dtype),
                    ..Args::default()
                },
            );
        }

        // Both resamplings, and a window that is not the grid's own extent.
        for bilinear in [false, true] {
            push(
                &format!("warp/{}", if bilinear { "bilinear" } else { "nearest" }),
                Op::Warp,
                Args {
                    bilinear: Some(bilinear),
                    ..Args::default()
                },
            );
        }
        push(
            "warp/window",
            Op::Warp,
            Args {
                bilinear: Some(true),
                bounds: Some([-20.0, 40.0, -30.0, 60.0]),
                ..Args::default()
            },
        );

        push(
            "palette/viridis",
            Op::Palette,
            Args {
                colormap: Some("viridis".to_string()),
                reversed: Some(false),
                scale: Some("linear".to_string()),
                ..Args::default()
            },
        );
        // A reversed ramp over a manual range under the log scale: the three
        // knobs that transform the domain, all moved off their defaults at
        // once so a host that dropped one cannot match by accident.
        push(
            "palette/log_reversed_ranged",
            Op::Palette,
            Args {
                colormap: Some("plasma".to_string()),
                reversed: Some(true),
                scale: Some("log10".to_string()),
                range_min: Some(1.0),
                range_max: Some(1000.0),
                ..Args::default()
            },
        );

        // Both flips. `false` is the north-up default a host gets for free;
        // `true` composes with the message's own `jScansPositively` flag.
        for flip in [false, true] {
            push(
                &format!("render/flip_{flip}"),
                Op::Render,
                Args {
                    colormap: Some("viridis".to_string()),
                    reversed: Some(false),
                    scale: Some("linear".to_string()),
                    flip_y: Some(flip),
                    ..Args::default()
                },
            );
        }

        for (lat, lon) in PROBE_POINTS {
            push(
                &format!("probe/{lat}_{lon}"),
                Op::Probe,
                Args {
                    lat: Some(lat),
                    lon: Some(lon),
                    ..Args::default()
                },
            );
        }

        push(
            "contours/auto",
            Op::Contours,
            Args {
                levels: Some(Vec::new()),
                ..Args::default()
            },
        );
        push(
            "contours/explicit",
            Op::Contours,
            Args {
                levels: Some(vec![250.0, 275.0, 300.0]),
                ..Args::default()
            },
        );
    }

    // ---- The error cases, one per `Error` code -----------------------------
    //
    // Every code in `Suite::error_codes` has to be reachable through a call a
    // host can make, or "the codes are stable" is a claim about an enum rather
    // than about the API. These five are that proof.
    let latlon = format!("{G2}regular_latlon_surface.grib2");
    out.push(Case {
        // Not a container at all. Three bytes is short of the indicator
        // section, so detection recognises nothing — as against the `decode`
        // case below, where enough of the header survives to be recognised as
        // GRIB and only then fails to parse.
        id: "error/unsupported_format".to_string(),
        fixture: latlon.clone(),
        op: Op::Open,
        args: Args {
            truncate: Some(3),
            ..Args::default()
        },
    });
    out.push(Case {
        id: "error/no_such_message".to_string(),
        fixture: latlon.clone(),
        op: Op::Message,
        args: Args {
            index: 9_999,
            ..Args::default()
        },
    });
    out.push(Case {
        // The indicator and the section table survive; the data section does
        // not, so the file opens and the decode fails.
        id: "error/decode".to_string(),
        fixture: latlon.clone(),
        op: Op::Decode,
        args: Args {
            truncate: Some(200),
            dtype: Some(Dtype::Auto),
            ..Args::default()
        },
    });
    out.push(Case {
        id: "error/invalid_option".to_string(),
        fixture: latlon.clone(),
        op: Op::Palette,
        args: Args {
            colormap: Some("no-such-colormap".to_string()),
            reversed: Some(false),
            scale: Some("linear".to_string()),
            ..Args::default()
        },
    });
    out.push(Case {
        // HEALPix is a real grid this build decodes, but not one it can lay a
        // raster over: it has to be resampled onto a synthesised lat/lon grid
        // first, which `Session` does not do yet (#580). That is what
        // `unsupported` means — the operation is defined and this family
        // declines it — and it is the only fixture here that says so at
        // `decode`, so it doubles as this suite's HEALPix coverage.
        id: "error/unsupported".to_string(),
        fixture: format!("{G2}healpix_n4_ring.grib2"),
        op: Op::Decode,
        args: Args {
            dtype: Some(Dtype::Auto),
            ..Args::default()
        },
    });

    out
}

/// Every [`crate::Error`] code this build can produce, sorted.
///
/// Written out rather than derived, so that adding a variant is a decision
/// recorded in the suite file. `crate::error`'s own test walks the enum and
/// asserts this list is exactly what it yields.
#[must_use]
pub fn error_codes() -> Vec<String> {
    [
        "decode",
        "invalid_option",
        "no_such_message",
        "unsupported",
        "unsupported_format",
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect()
}

// ---------------------------------------------------------------------------
// Observation
// ---------------------------------------------------------------------------

/// Which elements of a buffer an observation samples.
///
/// Integer arithmetic on the length alone, so every host picks the same five
/// slots without being told which. Recording the index beside the value means a
/// length change moves the slots and shows up as a diff rather than as a silent
/// re-alignment.
#[must_use]
pub fn sample_indices(len: usize) -> Vec<usize> {
    if len == 0 {
        return Vec::new();
    }
    let mut out = vec![0, len / 4, len / 2, (len / 4) * 3, len - 1];
    out.sort_unstable();
    out.dedup();
    out
}

/// Serialise, or say loudly in the observation that it did not.
///
/// A host cannot panic here — wasm32 aborts rather than unwinding — so a
/// serialisation failure travels as data and fails the comparison instead.
///
/// It does **not** catch a non-finite float: `serde_json` writes those as
/// `null` rather than refusing them. That is what `real` exists for, and the
/// leaves this is used on (`Stats`, `Georef`, `MessageInfo`, `Probe`) come from
/// a decode that has already masked every non-finite cell.
fn value_of<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or_else(|e| json!({ "serialisationError": e.to_string() }))
}

/// A real-valued leaf, in the one spelling every runner can produce.
///
/// `None` is `null`. A **non-finite** value is the string `"nonFinite"`, not
/// `null`: `serde_json` turns a `NaN` or an infinity into `null` silently
/// (`impl From<f64> for Value` is `Number::from_f64(..).map_or(Value::Null, ..)`),
/// which would record "this cell is absent" for what is really "this build
/// produced a NaN" — two different failures with the same recording. The name
/// is deliberately not `"NaN"` / `"inf"`: JavaScript spells those differently,
/// and which non-finite it was is not something the hosts have to agree on.
fn real(v: Option<f64>) -> Value {
    match v {
        None => Value::Null,
        Some(x) if x.is_finite() => json!(x),
        Some(_) => json!("nonFinite"),
    }
}

/// A number that must compare exactly: emitted as a JSON integer.
fn count(n: usize) -> Value {
    json!(u64::try_from(n).unwrap_or(u64::MAX))
}

/// Strip the parts of a serialised `Georef` that are not part of the
/// host-facing contract.
///
/// `geometry` is `core`'s tagged enum. The schema describes it loosely on
/// purpose (`{"type":"object","required":["kind"]}`) because no host reads it
/// back, and enumerating eleven families' payloads here would pin `core`'s
/// internals as though they were API.
fn georef_value(georef: &crate::api::Georef) -> Value {
    let mut v = value_of(georef);
    if let Some(obj) = v.as_object_mut() {
        obj.remove("geometry");
    }
    v
}

/// What a decoded field contributes to an observation.
fn field_value(field: &Field) -> Value {
    let values = field.values.to_f64();
    let samples: Vec<Value> = sample_indices(values.len())
        .into_iter()
        .map(|i| {
            json!({
                "i": count(i),
                // A masked cell's value is not data — the DTO's own doc says
                // read the mask first — so it crosses as `null` rather than as
                // the zero the buffer happens to hold. `fieldglass-napi` puts
                // `f64::NAN` in that slot and the browser host reads whatever
                // the typed array holds, so this is the only spelling all three
                // runners can agree on.
                "v": real(
                    (field.mask.get(i).copied().unwrap_or(0) == 1)
                        .then(|| values.get(i).copied())
                        .flatten(),
                ),
                "m": count(usize::from(field.mask.get(i).copied().unwrap_or(0))),
            })
        })
        .collect();
    json!({
        "dtype": match field.values.dtype() {
            Dtype::F32 => "f32",
            Dtype::F64 => "f64",
            // `Values::dtype` answers only the two widths it holds; `Auto` is
            // a request, never an answer. Named rather than wildcarded so a
            // third width would fail here instead of being reported as one of
            // these two.
            Dtype::Auto => "auto",
        },
        "len": count(values.len()),
        "maskLen": count(field.mask.len()),
        "maskOnes": count(field.mask.iter().filter(|&&m| m == 1).count()),
        "ni": field.ni,
        "nj": field.nj,
        "parameter": field.parameter,
        "units": field.units,
        "stats": value_of(&field.stats),
        "georef": georef_value(&field.georef),
        "samples": samples,
    })
}

/// The observation for a failed call.
///
/// The message is deliberately reduced to "is there one": `code` is the stable
/// half of the contract and `message` is prose a reword should not redden.
fn error_value(e: &Error) -> Value {
    json!({ "error": { "code": e.code(), "hasMessage": !e.message().is_empty() } })
}

/// Decode options from a case's [`Args`]. Infallible: [`Args::dtype`] is the
/// typed enum, so there is no string left to reject.
fn decode_options(args: &Args) -> DecodeOptions {
    DecodeOptions {
        dtype: args.dtype.clone().unwrap_or_default(),
    }
}

/// Palette options from a case's [`Args`].
fn palette_options(args: &Args) -> PaletteOptions {
    PaletteOptions {
        colormap: args.colormap.clone(),
        reversed: args.reversed.unwrap_or(false),
        min: args.range_min,
        max: args.range_max,
        scale: args.scale.clone(),
    }
}

/// Run one case and report what it produced.
///
/// `bytes` is the fixture file. Every failure is an observation too — the
/// error cases in [`cases`] are how [`Suite::error_codes`] is proved reachable
/// — so this returns a value rather than a `Result`.
#[must_use]
pub fn observe(bytes: &[u8], case: &Case) -> Value {
    match run(bytes, case) {
        Ok(v) => v,
        Err(e) => error_value(&e),
    }
}

/// [`observe`]'s body, written with `?` so every operation's failure funnels
/// into the same observation.
fn run(bytes: &[u8], case: &Case) -> Result<Value, Error> {
    let truncated = match case.args.truncate {
        Some(n) => &bytes[..n.min(bytes.len())],
        None => bytes,
    };
    let session = Session::open(truncated.to_vec())?;

    let field =
        |index: u32| -> Result<Field, Error> { session.decode(index, &decode_options(&case.args)) };

    let value = match case.op {
        Op::Open => json!({
            "format": value_of(&session.format()),
            "count": session.count(),
        }),
        Op::Message => {
            let info = session.message(case.args.index)?;
            let mut v = value_of(&info);
            // The grid inside a `MessageInfo` is a `Georef` like any other, so
            // its `geometry` comes out for the same reason.
            if let Some(grid) = v.get_mut("grid")
                && let Some(obj) = grid.as_object_mut()
            {
                obj.remove("geometry");
            }
            v
        }
        Op::Decode => field_value(&field(case.args.index)?),
        Op::Warp => {
            let field = field(case.args.index)?;
            let options = WarpOptions {
                bilinear: case.args.bilinear.unwrap_or(true),
                bounds: case.args.bounds,
            };
            let warped = session.warp(&field, &options)?;
            let samples: Vec<Value> = sample_indices(warped.values.len())
                .into_iter()
                .map(|i| {
                    json!({
                        "i": count(i),
                        // `f32` widened, because a JS host reads a
                        // `Float32Array` and the two must be the same number.
                        // Masked as above: no value, not a stale one.
                        "v": real(
                            (warped.mask.get(i).copied().unwrap_or(0) == 1)
                                .then(|| warped.values.get(i).map(|&v| f64::from(v)))
                                .flatten(),
                        ),
                        "m": count(usize::from(warped.mask.get(i).copied().unwrap_or(0))),
                    })
                })
                .collect();
            json!({
                "width": warped.width,
                "height": warped.height,
                "bounds": warped.bounds,
                "len": count(warped.values.len()),
                "maskOnes": count(warped.mask.iter().filter(|&&m| m == 1).count()),
                "samples": samples,
            })
        }
        Op::Palette => {
            let field = field(case.args.index)?;
            let palette = session.palette(&field, &palette_options(&case.args))?;
            let lut: Vec<Value> = sample_indices(palette.lut.len())
                .into_iter()
                .map(|i| json!({ "i": count(i), "b": count(usize::from(palette.lut[i])) }))
                .collect();
            json!({
                "t0": real(Some(palette.t0)),
                "t1": real(Some(palette.t1)),
                "scale": palette.scale.as_str(),
                "lutLen": count(palette.lut.len()),
                "lutSamples": lut,
                "maskedRgba": palette.masked_rgba.iter().map(|&b| count(usize::from(b))).collect::<Vec<_>>(),
            })
        }
        Op::Render => {
            let field = field(case.args.index)?;
            let raster = session.render(
                &field,
                &palette_options(&case.args),
                case.args.flip_y.unwrap_or(false),
            )?;
            let pixels: Vec<Value> = sample_indices(raster.rgba.len() / 4)
                .into_iter()
                .map(|p| {
                    let k = p * 4;
                    json!({
                        "i": count(p),
                        "rgba": raster.rgba[k..k + 4]
                            .iter()
                            .map(|&b| count(usize::from(b)))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect();
            json!({
                "width": raster.width,
                "height": raster.height,
                "rgbaLen": count(raster.rgba.len()),
                // Alpha, not colour: the one per-pixel decision that says
                // whether the warp placed anything there.
                "opaque": count(raster.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).count()),
                "pixels": pixels,
            })
        }
        Op::Probe => {
            let field = field(case.args.index)?;
            let probe = session.probe(
                &field,
                case.args.lat.unwrap_or(0.0),
                case.args.lon.unwrap_or(0.0),
            );
            value_of(&probe)
        }
        Op::Contours => {
            let field = field(case.args.index)?;
            let levels = case.args.levels.clone().unwrap_or_default();
            let lines = session.contours(&field, &levels)?;
            json!({
                "levelCount": count(lines.len()),
                "levels": lines.iter().map(|l| real(Some(l.value))).collect::<Vec<_>>(),
                "segmentCounts": lines
                    .iter()
                    .map(|l| count(l.segments.len()))
                    .collect::<Vec<_>>(),
            })
        }
    };
    Ok(value)
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

/// Compare an observation with its recording, returning one line per
/// disagreement.
///
/// Objects must have exactly the same key set, which is what turns a renamed or
/// dropped DTO field into a failure rather than into silence. Arrays must have
/// the same length. Integers, strings, booleans and nulls compare exactly; only
/// real-valued leaves take `tolerance`. See the module docs for why that split
/// is where it is.
#[must_use]
pub fn compare(expected: &Value, observed: &Value, tolerance: Tolerance) -> Vec<String> {
    let mut out = Vec::new();
    diff("", expected, observed, tolerance, &mut out);
    out
}

fn diff(path: &str, expected: &Value, observed: &Value, tol: Tolerance, out: &mut Vec<String>) {
    let at = if path.is_empty() { "$" } else { path };
    match (expected, observed) {
        (Value::Object(a), Value::Object(b)) => {
            let missing: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
            let extra: Vec<&String> = b.keys().filter(|k| !a.contains_key(*k)).collect();
            if !missing.is_empty() {
                out.push(format!("{at}: missing keys {missing:?}"));
            }
            if !extra.is_empty() {
                out.push(format!("{at}: unexpected keys {extra:?}"));
            }
            for (k, av) in a {
                if let Some(bv) = b.get(k) {
                    diff(&format!("{path}.{k}"), av, bv, tol, out);
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                out.push(format!("{at}: length {} != {}", a.len(), b.len()));
                return;
            }
            for (i, (av, bv)) in a.iter().zip(b).enumerate() {
                diff(&format!("{path}[{i}]"), av, bv, tol, out);
            }
        }
        (Value::Number(a), Value::Number(b)) => {
            // An integer on either side is a discrete answer, and ADR-0009's
            // measurement says those agree exactly across targets. Comparing
            // one loosely would give away coverage the measurement says is
            // there.
            let discrete = !a.is_f64() || !b.is_f64();
            let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) else {
                out.push(format!("{at}: {a} != {b}"));
                return;
            };
            let ok = if discrete {
                a == b
            } else {
                (x - y).abs() <= tol.absolute + tol.relative * x.abs()
            };
            if !ok {
                out.push(format!("{at}: {x} != {y}"));
            }
        }
        (a, b) => {
            if a != b {
                out.push(format!("{at}: {a} != {b}"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/// Build a fresh [`Suite`] by running every case, given a way to read a
/// fixture.
///
/// `read` takes a `crates/`-relative fixture path. A runner supplies its own
/// because this crate's tests, the napi crate's tests and the Node runner all
/// start from different working directories.
///
/// # Errors
///
/// When a fixture cannot be read.
pub fn record(read: &dyn Fn(&str) -> Result<Vec<u8>, String>) -> Result<Suite, String> {
    let mut recorded = Vec::new();
    for case in cases() {
        let bytes = read(&case.fixture)?;
        let expect = observe(&bytes, &case);
        recorded.push(RecordedCase { case, expect });
    }
    Ok(Suite {
        tolerance: Tolerance::default(),
        error_codes: error_codes(),
        cases: recorded,
    })
}

/// Serialise a suite the way the committed file is written: pretty-printed,
/// two-space indented, one trailing newline.
///
/// # Errors
///
/// When the suite does not serialise. Not for a non-finite float — `serde_json`
/// writes those as `null`, which is why an observation puts every real-valued
/// leaf through a helper that names a non-finite before it gets here.
pub fn to_pretty_json(suite: &Suite) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string_pretty(suite)?;
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The comparator's own contract, since every other assertion in the suite
    /// is made through it. A comparator that returned "no differences" for
    /// everything would make the whole suite a no-op, so each rule is shown
    /// failing on purpose.
    #[test]
    fn the_comparator_rejects_what_it_should() {
        let tol = Tolerance::default();
        let base = json!({"a": 1, "b": 2.0, "c": "x", "d": [1, 2], "e": null});
        assert!(compare(&base, &base.clone(), tol).is_empty());

        // A renamed field is a missing key and an unexpected one.
        let renamed = json!({"a": 1, "bb": 2.0, "c": "x", "d": [1, 2], "e": null});
        assert_eq!(compare(&base, &renamed, tol).len(), 2);

        // An integer off by one is never within tolerance.
        assert_eq!(
            compare(&json!({"a": 1}), &json!({"a": 2}), tol).len(),
            1,
            "an integer count must compare exactly"
        );
        // Nor is an integer compared against a float of the same value: a host
        // that turned a count into a double changed the wire shape.
        assert_eq!(compare(&json!({"a": 1}), &json!({"a": 1.0}), tol).len(), 1);

        // A real-valued leaf inside the tolerance passes and outside it fails.
        assert!(compare(&json!({"b": 2.0}), &json!({"b": 2.0 + 1e-12}), tol).is_empty());
        assert_eq!(
            compare(&json!({"b": 2.0}), &json!({"b": 2.001}), tol).len(),
            1
        );

        // A string is never approximately equal, and neither is a null.
        assert_eq!(compare(&json!("x"), &json!("y"), tol).len(), 1);
        assert_eq!(compare(&json!(null), &json!(0), tol).len(), 1);

        // A shorter array is a length difference, reported once rather than
        // once per element.
        assert_eq!(compare(&json!([1, 2]), &json!([1]), tol).len(), 1);
    }

    /// Every host picks the same slots out of a buffer, and picks none out of
    /// an empty one.
    #[test]
    fn sample_indices_are_derived_from_the_length_alone() {
        assert!(sample_indices(0).is_empty());
        assert_eq!(sample_indices(1), vec![0]);
        assert_eq!(sample_indices(2), vec![0, 1]);
        assert_eq!(sample_indices(8), vec![0, 2, 4, 6, 7]);
        let big = sample_indices(1_000);
        assert_eq!(big, vec![0, 250, 500, 750, 999]);
    }

    /// The case list must have no duplicate ids: expectations are keyed by id,
    /// so a duplicate would silently drop a case.
    #[test]
    fn every_case_id_is_unique() {
        let mut ids: Vec<String> = cases().into_iter().map(|c| c.id).collect();
        let total = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), total, "duplicate conformance case ids");
    }
}
