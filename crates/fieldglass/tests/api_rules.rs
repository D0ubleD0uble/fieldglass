//! The rules ADR-0006 decision 2 states in prose, enforced (#573).
//!
//! > Every type on the `Session` surface: has no generics, lifetimes, or trait
//! > objects; carries fields as contiguous `Vec<f32>` / `Vec<f64>` plus a
//! > `Vec<u8>` mask, never `Vec<Option<f64>>`; uses strings only for labels; is
//! > `#[non_exhaustive]`; derives `serde::Serialize` / `Deserialize` and
//! > `schemars::JsonSchema`.
//!
//! # Why a source scan and not only trait bounds
//!
//! Half of these are compile-time properties and are checked that way below —
//! a missing `Serialize` fails [`is_wire_shaped`] to compile, and a lifetime
//! fails its `'static` bound. The other half are not visible to the type
//! system at all: `#[non_exhaustive]` has no trait, a `Vec<Option<f64>>` field
//! is a perfectly good `Serialize`, and neither says anything about a type
//! that was simply never registered here.
//!
//! **The last one is the failure mode that matters.** A rules test that walks a
//! list passes just as happily when the list is empty, or when a new type was
//! added and nobody added it. So [`declarations`] reads the crate's own source
//! — through `include_str!`, which resolves at compile time and so needs no
//! filesystem on any target this runs on — and
//! [`every_public_api_type_is_classified`](fn@every_public_api_type_is_classified)
//! fails when a public type in an API module is not in the table below.
//!
//! # The negative cases
//!
//! Every rule is also shown *failing*, in
//! [`the_scanner_rejects_a_non_conforming_type`](fn@the_scanner_rejects_a_non_conforming_type)
//! and [`the_schema_rule_rejects_an_optional_element_array`](fn@the_schema_rule_rejects_an_optional_element_array),
//! against types written here to break exactly one rule each. A gate nobody has
//! watched fail is a gate nobody knows is connected.

use std::collections::{BTreeMap, BTreeSet};

use fieldglass::{
    Addressing, AxisUnits, CombineOpInfo, DecodeOptions, DimensionInfo, Dtype, Error, Field,
    Georef, Isoline, LeftOutArray, Line, MessageInfo, PaletteOptions, PixelProbe, Probe, Projected,
    Raster, RenderOptions, ResolvedOptions, SourceFormat, Stats, TargetKind, Values, VariableInfo,
    WarpOptions, WarpTarget, Warped,
};

// ---------------------------------------------------------------------------
// The classification
// ---------------------------------------------------------------------------

/// What a public type in an API module is, and therefore which rules it owes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A DTO. Crosses a host boundary by serialisation, so it owes every rule
    /// in ADR-0006 decision 2.
    Wire,
    /// Public Rust vocabulary the display half is written in. A host reads
    /// these in Rust and converts; nothing serialises them, so they owe
    /// `#[non_exhaustive]`, the no-generics rule and the no-`Vec<Option<…>>`
    /// rule, but not the serde ones — and not the no-borrows rule either,
    /// because a `&'static` borrow is owned-equivalent and
    /// `ResolvedOptions::colormap` is a `Cow<'static, _>` of one.
    Engine,
    /// A type a host builds with **struct-literal syntax**, because it borrows
    /// and so cannot be deserialised or handed over by a constructor that owns
    /// its parts.
    ///
    /// The one rule that runs the other way: `#[non_exhaustive]` blocks literal
    /// syntax downstream, so such a type must *not* carry it. Every other
    /// constructible type on this surface is an option struct a host either
    /// deserialises or builds through `new`, which is why `#[non_exhaustive]`
    /// costs those nothing.
    Borrowed,
    /// A handle. Owns state, is never plain data, and is not serialised.
    Handle,
    /// The conformance harness's own data — the case list and the recording
    /// (`conformance/suite.json`). It is a `pub mod` under a default feature,
    /// so it is public surface and has to be classified; it is not part of the
    /// host contract, and pinning it with `#[non_exhaustive]` would stop a
    /// runner in this repository writing a `Case` literal.
    Suite,
}

/// Every public type the API modules declare, with the class it belongs to and
/// the reason it is not `Wire` where that is not obvious.
///
/// A type missing from here fails
/// [`every_public_api_type_is_classified`](fn@every_public_api_type_is_classified),
/// which is what stops this test from silently covering less over time.
const CLASSIFICATION: &[(&str, Class, &str)] = &[
    // --- api.rs: the DTOs #574 generates `native.ts` from -------------------
    ("SourceFormat", Class::Wire, ""),
    ("Dtype", Class::Wire, ""),
    ("AxisUnits", Class::Wire, ""),
    ("Georef", Class::Wire, ""),
    ("Values", Class::Wire, ""),
    ("Stats", Class::Wire, ""),
    ("Field", Class::Wire, ""),
    // A line through an array (#172): a profile or time series at a cell.
    ("Line", Class::Wire, ""),
    ("MessageInfo", Class::Wire, ""),
    // The second addressing mode (#662): how a container is addressed, and the
    // variables and dimensions that addressing names.
    ("Addressing", Class::Wire, ""),
    ("DimensionInfo", Class::Wire, ""),
    ("VariableInfo", Class::Wire, ""),
    // The arrays a container holds and would not read, one shape for every
    // container (#709).
    ("LeftOutArray", Class::Wire, ""),
    ("CombineOpInfo", Class::Wire, ""),
    ("Warped", Class::Wire, ""),
    ("Probe", Class::Wire, ""),
    ("Isoline", Class::Wire, ""),
    // --- error.rs -----------------------------------------------------------
    ("Error", Class::Wire, ""),
    // --- session.rs ---------------------------------------------------------
    ("DecodeOptions", Class::Wire, ""),
    ("WarpOptions", Class::Wire, ""),
    ("PaletteOptions", Class::Wire, ""),
    ("Raster", Class::Wire, ""),
    (
        "PlacedSlice",
        Class::Engine,
        "where one slice sits, for a host that paints it itself (#659); it \
         carries a `GridGeometry`, which is the engine's own shape rather than \
         anything a host serialises — placement reaches the wire as \
         `Field::georef`",
    ),
    (
        "Session",
        Class::Handle,
        "owns the parsed message index; a host holds it, never sends it",
    ),
    // --- render.rs ----------------------------------------------------------
    ("RenderOptions", Class::Wire, ""),
    (
        "Source",
        Class::Borrowed,
        "borrows the host's geometry for one call — the one lifetime on the \
         surface, and the reason it is neither a DTO nor #[non_exhaustive]: \
         `fieldglass-napi` writes the literal",
    ),
    (
        "ResolvedOptions",
        Class::Engine,
        "the parsed form of RenderOptions; a host's paint step reads it in Rust",
    ),
    (
        "TargetKind",
        Class::Engine,
        "which branch ResolvedOptions took",
    ),
    (
        "WarpTarget",
        Class::Engine,
        "the closed target vocabulary behind RenderOptions::projection",
    ),
    (
        "Projected",
        Class::Engine,
        "one projection stage's output, handed to the binding's own painter",
    ),
    (
        "PixelProbe",
        Class::Engine,
        "the pixel-probe result; napi converts it into its own DTO",
    ),
    // --- conformance.rs: the suite's own shape, not the host contract -------
    (
        "Tolerance",
        Class::Suite,
        "how close a real-valued leaf has to be",
    ),
    ("Op", Class::Suite, "which operation a case exercises"),
    ("Args", Class::Suite, "one call's inputs"),
    ("Case", Class::Suite, "one case"),
    ("RecordedCase", Class::Suite, "a case and what it produced"),
    ("Suite", Class::Suite, "the whole recording"),
];

/// Types re-exported from another crate at this crate's root, which the source
/// scan cannot see because they are declared elsewhere.
///
/// `Palette` is the one that matters: it is what [`fieldglass::Session::palette`]
/// returns, so it crosses to every host, and nothing here would have held it to
/// a rule. The rest arrive with it.
const FOREIGN_REEXPORTS: &[(&str, Class, &str)] = &[
    (
        "Scan",
        Class::Wire,
        "core's own type on `Georef::scan`; its schema is written by hand in \
         `api::scan_schema` because `core` cannot derive `JsonSchema`",
    ),
    (
        "Palette",
        Class::Wire,
        "the return of `Session::palette`, and the colour decision as data",
    ),
    (
        "ScaleMode",
        Class::Wire,
        "which transform a `Palette`'s domain is expressed in",
    ),
    (
        "Colormap",
        Class::Engine,
        "a table a host names by string or sends as its 768-byte lookup table, \
         and never receives by value; `colormaps()` hands out `&'static [Colormap]`",
    ),
    (
        "ColorTable",
        Class::Engine,
        "a parsed `.cpt`, read in Rust; a host keeps and sends only the lookup \
         table it compiles to, never the table itself (#236)",
    ),
    (
        "CptError",
        Class::Engine,
        "why a `.cpt` did not parse; a host shows its `Display` and nothing \
         serialises it",
    ),
    (
        "CombineOp",
        Class::Engine,
        "the closed combine vocabulary; a host names one by the string in \
         `CombineOpInfo::value` and never receives the enum",
    ),
    // The seams a host *implements*, rather than receives (#659). A host brings
    // its own `ObjectSource` — the addon reading a directory, a browser filling
    // one from a bucket — so it has to be able to name the trait, and
    // `fieldglass-wasm` depends on nothing else it could name it from.
    (
        "ObjectSource",
        Class::Engine,
        "the keyed-object seam `Session::open_store` takes; a host implements it \
         and never serialises one",
    ),
    (
        "ByteSource",
        Class::Engine,
        "the ranged seam `Session::open_source` takes, for the same reason",
    ),
    (
        "MemoryObjects",
        Class::Engine,
        "the `ObjectSource` a host that has already fetched everything wants; \
         built in Rust and handed to `Session::open_store`",
    ),
    (
        "FieldglassError",
        Class::Engine,
        "what every method of both seams returns, so a host implementing one has \
         to be able to name it; `Error` is what a host *receives*",
    ),
];

// ---------------------------------------------------------------------------
// The compile-time half
// ---------------------------------------------------------------------------

/// A wire type is owned, comparable, and round-trips through serde.
///
/// Every bound here is one of ADR-0006's rules made into something the
/// compiler checks: `'static` rejects a lifetime, `Sized` and the absence of a
/// type parameter reject a generic, and `DeserializeOwned` rejects a type that
/// can only be borrowed out of its input.
fn is_wire_shaped<T>()
where
    T: serde::Serialize
        + serde::de::DeserializeOwned
        + std::fmt::Debug
        + Clone
        + PartialEq
        + Send
        + Sync
        + Sized
        + 'static,
{
}

/// Engine types are owned Rust vocabulary: cloneable and printable, but not
/// serialised.
fn is_engine_shaped<T>()
where
    T: std::fmt::Debug + Clone + Sized,
{
}

/// Every wire type satisfies the bounds, or this test does not compile.
///
/// Deliberately not a loop over a list of names: the point is that the
/// *compiler* checked each one, and a name in a list is checked by nothing.
#[test]
fn every_wire_type_is_owned_and_round_trips() {
    is_wire_shaped::<SourceFormat>();
    is_wire_shaped::<Dtype>();
    is_wire_shaped::<AxisUnits>();
    is_wire_shaped::<Georef>();
    is_wire_shaped::<Values>();
    is_wire_shaped::<Stats>();
    is_wire_shaped::<Field>();
    is_wire_shaped::<MessageInfo>();
    is_wire_shaped::<CombineOpInfo>();
    is_wire_shaped::<Warped>();
    is_wire_shaped::<Probe>();
    is_wire_shaped::<Isoline>();
    is_wire_shaped::<Error>();
    is_wire_shaped::<DecodeOptions>();
    is_wire_shaped::<WarpOptions>();
    is_wire_shaped::<PaletteOptions>();
    is_wire_shaped::<Raster>();
    is_wire_shaped::<RenderOptions>();

    is_engine_shaped::<ResolvedOptions>();
    is_engine_shaped::<TargetKind>();
    is_engine_shaped::<WarpTarget>();
    is_engine_shaped::<Projected>();
    is_engine_shaped::<PixelProbe>();
}

/// Every option type a host builds can be built from **outside** this crate,
/// and its fields assigned afterwards.
///
/// `#[non_exhaustive]` blocks struct-literal syntax downstream, so a type with
/// no constructor leaves a caller only `Default::default()` followed by field
/// assignment — the pattern `clippy::field_reassign_with_default` exists to
/// discourage. This test is that rule's proof: it is an integration test, so it
/// *is* a downstream crate, and it does not compile if a constructor is
/// missing.
#[test]
fn every_option_type_is_constructible_from_outside_the_crate() {
    let decode = DecodeOptions::new(Dtype::F32);
    assert_eq!(decode.dtype, Dtype::F32);

    let mut warp = WarpOptions::new(false);
    warp.bounds = Some([-10.0, 10.0, -20.0, 20.0]);
    assert!(!warp.bilinear);

    let mut palette = PaletteOptions::new(Some("viridis"), Some("log10"));
    palette.reversed = true;
    palette.min = Some(1.0);
    palette.max = Some(1000.0);
    assert_eq!(palette.colormap.as_deref(), Some("viridis"));

    let mut render = RenderOptions::new("equirectangular", "bilinear");
    render.colormap = Some("plasma".to_string());
    render.flip_y = true;
    assert!(render.flip_y);
}

/// Every wire type survives a JSON round trip unchanged, from a document
/// written out here rather than from a value built in Rust.
///
/// Two reasons for the direction. The output DTOs are `#[non_exhaustive]` and
/// a host only ever *reads* them, so this test — a downstream crate — cannot
/// construct one, which is the rule working as intended. And starting from the
/// document is what pins the field names: a `rename_all` that moved, or a
/// field renamed in Rust, fails here, where a value built in Rust and compared
/// with itself would not notice.
#[test]
fn every_wire_type_round_trips_through_json() {
    fn round_trip<T>(name: &str, document: &str)
    where
        T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
    {
        let wanted: serde_json::Value =
            serde_json::from_str(document).expect("the document is JSON");
        let value: T = serde_json::from_str(document)
            .unwrap_or_else(|e| panic!("{name} does not read {document}: {e}"));
        let written = serde_json::to_value(&value).expect("serialises");
        assert_eq!(written, wanted, "{name}: serialising changed the document");
        let back: T = serde_json::from_value(written).expect("re-reads");
        assert_eq!(back, value, "{name}: the round trip changed the value");
    }

    round_trip::<SourceFormat>("SourceFormat", r#""grib2""#);
    round_trip::<Dtype>("Dtype", r#""auto""#);
    round_trip::<AxisUnits>("AxisUnits", r#""metres""#);
    round_trip::<Values>("Values", r#"{"dtype":"f32","data":[1.0,2.0]}"#);
    round_trip::<Stats>("Stats", r#"{"min":1.0,"max":2.0,"validCount":2}"#);
    round_trip::<Probe>(
        "Probe",
        r#"{"lat":1.0,"lon":2.0,"i":3.5,"j":4.5,"value":null}"#,
    );
    round_trip::<Isoline>("Isoline", r#"{"value":1.0,"segments":[[0.0,1.0,2.0,3.0]]}"#);
    round_trip::<Warped>(
        "Warped",
        r#"{"values":[1.0],"mask":[1],"width":1,"height":1,"bounds":[-1.0,1.0,-2.0,2.0]}"#,
    );
    round_trip::<Raster>("Raster", r#"{"rgba":[1,2,3,4],"width":1,"height":1}"#);
    round_trip::<Error>("Error", r#"{"code":"no_such_message","index":1,"count":0}"#);
    round_trip::<DecodeOptions>("DecodeOptions", r#"{"dtype":"f64"}"#);
    round_trip::<WarpOptions>(
        "WarpOptions",
        r#"{"bilinear":true,"bounds":null,"width":512,"height":384}"#,
    );
    round_trip::<PaletteOptions>(
        "PaletteOptions",
        r#"{"colormap":"viridis","colormapTable":null,"reversed":false,"min":null,"max":null,"scale":null}"#,
    );
    // The three the extension's declarations are generated from (#574). Their
    // documents are long, which is the point: every field name is pinned here.
    round_trip::<Georef>("Georef", GEOREF_JSON);
    // `GEOREF` stands in for the nested document, so the one `Georef` spelling
    // above is the only place its field names are written out.
    round_trip::<Field>("Field", &FIELD_JSON.replace("GEOREF", GEOREF_JSON));
    round_trip::<Line>("Line", LINE_JSON);
    round_trip::<MessageInfo>(
        "MessageInfo",
        &MESSAGE_INFO_JSON.replace("GEOREF", GEOREF_JSON),
    );
    round_trip::<CombineOpInfo>("CombineOpInfo", r#"{"value":"a_minus_b","label":"A − B"}"#);
    round_trip::<Addressing>("Addressing", r#""variables""#);
    round_trip::<DimensionInfo>("DimensionInfo", r#"{"name":"time","length":12}"#);
    round_trip::<VariableInfo>(
        "VariableInfo",
        r#"{"index":0,"name":"/g/sst","dims":[{"name":"lat","length":2}],"dtype":"float","units":"K","detectedYDim":0,"detectedXDim":1,"detectedTimeDim":null}"#,
    );
    round_trip::<LeftOutArray>(
        "LeftOutArray",
        r#"{"name":"PRODUCT/sst","reason":"unsupported section: codec bz2"}"#,
    );
    round_trip::<RenderOptions>("RenderOptions", RENDER_OPTIONS_JSON);

    assert_covers_every_wire_type("every_wire_type_round_trips_through_json", ROUND_TRIPPED);
}

/// A `Georef` as it crosses the wire. `geometry` is `core`'s tagged enum and is
/// described loosely to a schema consumer on purpose, so the smallest real
/// variant stands in for it here.
const GEOREF_JSON: &str = r#"{"geometry":{"kind":"unsupported","label":"whatever"},"kind":"latlon","label":"latlon","ni":2,"nj":2,"boundsLonlat":[-1.0,1.0,-2.0,2.0],"corners":[1.0,-1.0,-2.0,2.0],"pointsPerRow":null,"proj4":"+proj=longlat","axisUnits":"degrees","x0":0.0,"y0":1.0,"dx":1.0,"dy":-1.0,"periodicX":false,"scan":{"iNegative":false,"jPositive":true,"jConsecutive":false}}"#;

/// A `Field`, with the smallest raster that still has a mask and statistics.
const LINE_JSON: &str = r#"{"values":{"dtype":"f64","data":[6.0,18.0]},"mask":[1,1],"stats":{"min":6.0,"max":18.0,"validCount":2},"variable":"temperature","units":"K","dimension":"time","coordinates":[0.0,6.0],"coordinateUnits":"hours since 2020-01-01 00:00:00"}"#;
const FIELD_JSON: &str = r#"{"values":{"dtype":"f32","data":[1.0,2.0,3.0,4.0]},"mask":[1,1,1,0],"ni":2,"nj":2,"georef":GEOREF,"stats":{"min":1.0,"max":3.0,"validCount":3},"parameter":"Temperature","units":"K"}"#;

/// A `MessageInfo` with every optional field present, so none of them is pinned
/// only in its absent form.
const MESSAGE_INFO_JSON: &str = r#"{"index":0,"offsetBytes":0,"parameter":"Temperature","abbreviation":"2t","units":"K","level":"2 m above ground","levelType":"heightAboveGround","referenceTime":"2026-01-01T00:00:00Z","forecast":"+6h","packing":"grid_simple","grid":GEOREF,"sizeLabel":"N32","forecastHours":6,"p1Octet":null,"originatingCentre":"Centre 98","subCentre":null,"edition":2,"discipline":"Meteorological products","totalLengthBytes":1234,"productionStatus":"Operational products","dataType":"Analysis and forecast products"}"#;

/// A `RenderOptions` with every field stated. `width`/`height` carry real
/// numbers rather than `null`, so the document pins them as JSON *integers*: a
/// host reading `512.0` where the schema says `u32` is the drift this catches.
const RENDER_OPTIONS_JSON: &str = r#"{"projection":"equirectangular","projectionPreset":"atlantic","centerLat":0.0,"centerLon":0.0,"resampling":"bilinear","flipY":false,"rangeMin":null,"rangeMax":null,"boundsLatMin":null,"boundsLatMax":null,"boundsLonMin":null,"boundsLonMax":null,"colormap":"viridis","colormapTable":null,"reverseColormap":false,"scaleMode":"linear","width":512,"height":384}"#;

/// The wire types the round-trip test covers, so it can be held to the
/// classification rather than to whoever last edited the list.
const ROUND_TRIPPED: &[&str] = &[
    "SourceFormat",
    "Dtype",
    "AxisUnits",
    "Values",
    "Stats",
    "Probe",
    "Isoline",
    "Warped",
    "Raster",
    "Error",
    "DecodeOptions",
    "WarpOptions",
    "PaletteOptions",
    "Georef",
    "Field",
    "Line",
    "MessageInfo",
    "CombineOpInfo",
    "RenderOptions",
    "Addressing",
    "DimensionInfo",
    "VariableInfo",
    "LeftOutArray",
];

/// Fail unless `covered` is exactly the `Class::Wire` half of
/// [`CLASSIFICATION`].
///
/// The lists in this file are hand-written, which is the same fail-open the
/// preamble says the source scan exists to close: a wire type added to
/// `CLASSIFICATION` and to neither list would be checked by the scanner and by
/// nothing else.
fn assert_covers_every_wire_type(what: &str, covered: &[&str]) {
    let wanted: BTreeSet<&str> = CLASSIFICATION
        .iter()
        .filter(|(_, c, _)| *c == Class::Wire)
        .map(|(n, _, _)| *n)
        .collect();
    let got: BTreeSet<&str> = covered.iter().copied().collect();
    let missing: Vec<&&str> = wanted.difference(&got).collect();
    let extra: Vec<&&str> = got.difference(&wanted).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "{what} covers {got:?}; the Wire class is {wanted:?} (missing {missing:?}, extra {extra:?})"
    );
}

/// `Error::code()` reports exactly the codes the conformance suite pins, and a
/// variant that stopped being reachable, or one added without a code, fails
/// here.
#[test]
fn the_error_codes_are_the_ones_the_suite_pins() {
    let every_variant = [
        Error::UnsupportedFormat {
            detail: String::new(),
        },
        Error::Decode {
            detail: String::new(),
        },
        Error::NoSuchMessage { index: 0, count: 0 },
        Error::Unsupported {
            detail: String::new(),
        },
        Error::WrongAddressing {
            expected: String::new(),
            detail: String::new(),
        },
        Error::ShortRead {
            at: 0,
            got: 0,
            wanted: 0,
        },
        Error::InvalidOption {
            detail: String::new(),
        },
    ];
    let mut codes: Vec<String> = every_variant.iter().map(|e| e.code().to_string()).collect();
    codes.sort();
    codes.dedup();
    assert_eq!(
        codes,
        fieldglass::conformance::error_codes(),
        "the pinned code list and this build's `Error` have diverged"
    );

    // The count is the half a list cannot check: a *new* variant would still
    // let the list above match, because nothing forces it into the array —
    // which is how `WrongAddressing` (#662) went unpinned until #679. `Error`
    // is `#[non_exhaustive]`, so from outside the crate the match below can
    // only be written with a wildcard arm that fails rather than passes. The
    // exhaustive version lives in `error.rs`'s own tests, where the compiler
    // refuses a variant nobody listed.
    for e in &every_variant {
        let named = matches!(
            e,
            Error::UnsupportedFormat { .. }
                | Error::Decode { .. }
                | Error::NoSuchMessage { .. }
                | Error::Unsupported { .. }
                | Error::WrongAddressing { .. }
                | Error::ShortRead { .. }
                | Error::InvalidOption { .. }
        );
        assert!(named, "an Error variant this test does not name: {e:?}");
    }
}

// ---------------------------------------------------------------------------
// The source scan
// ---------------------------------------------------------------------------

/// One `pub struct` / `pub enum` the scanner found.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decl {
    /// The type's name.
    name: String,
    /// Whether the declaration carries generic parameters or a lifetime.
    generic: bool,
    /// The attributes above it, plus any the `api_type!` macro supplies.
    attrs: Vec<String>,
    /// The field and variant types inside the braces, as written.
    field_types: Vec<String>,
}

/// The API modules, by name and source.
///
/// `include_str!` rather than `std::fs`: this test also runs on
/// `wasm32-wasip1`, where the sandbox's view of the filesystem is whatever the
/// runner granted, and a scanner that silently found no files would be the
/// fail-open gate this whole file exists to avoid.
const MODULES: &[(&str, &str)] = &[
    ("api.rs", include_str!("../src/api.rs")),
    ("combine.rs", include_str!("../src/combine.rs")),
    ("error.rs", include_str!("../src/error.rs")),
    ("session.rs", include_str!("../src/session.rs")),
    ("render.rs", include_str!("../src/render.rs")),
    // A `pub mod` under a default feature, so its types are public surface
    // even though no host reads them.
    ("conformance.rs", include_str!("../src/conformance.rs")),
    // Declares no type today, and is here so that one added tomorrow is
    // classified rather than unscanned.
    ("shader.rs", include_str!("../src/shader.rs")),
];

/// Rules a declaration can break, as a message naming the type and the rule.
type Violations = Vec<String>;

/// Everything before the file's first top-level `#[cfg(test)]`.
///
/// Matched on the attribute at column zero rather than on `mod tests {`:
/// `render.rs` has four test modules and none of them is called `tests`, so a
/// name-based cut would have been a silent no-op there. Test modules declare
/// deliberately odd types (this file's own negative cases among them) and are
/// not part of the API.
fn without_tests(source: &str) -> &str {
    match source.find("\n#[cfg(test)]\n") {
        Some(i) => &source[..i],
        None => source,
    }
}

/// The attributes the `api_type!` macro adds to every type inside it, and the
/// line range that macro's invocation spans.
///
/// `api.rs` states the common derives once, in the macro, which is why the
/// declarations inside it look bare. A scanner that did not know this would
/// report all eleven DTOs as missing every derive.
fn macro_context(source: &str) -> (Vec<String>, Option<(usize, usize)>) {
    let lines: Vec<&str> = source.lines().collect();
    let mut attrs = Vec::new();
    let mut in_macro_body = false;
    for line in &lines {
        let t = line.trim();
        if t.starts_with("macro_rules! api_type") {
            in_macro_body = true;
            continue;
        }
        if in_macro_body {
            if t.starts_with("#[") {
                attrs.push(t.to_string());
            } else if t.starts_with("$item") {
                in_macro_body = false;
            }
        }
    }

    let mut span = None;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("api_type! {") {
            // The invocation's closing brace is the next line that is exactly
            // `}` at column zero.
            let end = lines
                .iter()
                .enumerate()
                .skip(i + 1)
                .find(|(_, l)| **l == "}")
                .map_or(lines.len(), |(j, _)| j);
            span = Some((i, end));
            break;
        }
    }
    (attrs, span)
}

/// Every `pub struct` / `pub enum` an API module declares.
fn declarations(source: &str) -> Vec<Decl> {
    let source = without_tests(source);
    let (macro_attrs, macro_span) = macro_context(source);
    let lines: Vec<&str> = source.lines().collect();

    let mut out = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("#[") {
            pending.push(t.to_string());
            continue;
        }
        if t.starts_with("//") || t.is_empty() {
            continue;
        }
        let Some(rest) = ["pub struct ", "pub enum ", "pub type ", "pub trait "]
            .iter()
            .find_map(|kw| t.strip_prefix(kw))
        else {
            pending.clear();
            continue;
        };
        // `macro_rules!` bodies contain `$item`, never a real declaration, so
        // anything reached here inside `api_type! { … }` is a real type that
        // inherits the macro's attributes.
        let inside_macro = macro_span.is_some_and(|(a, b)| i > a && i < b);
        let name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        let generic = rest[name.len()..].starts_with('<');
        let mut attrs = std::mem::take(&mut pending);
        if inside_macro {
            attrs.extend(macro_attrs.iter().cloned());
        }
        out.push(Decl {
            name,
            generic,
            attrs,
            field_types: field_types(&lines, i),
        });
    }
    out
}

/// The types written inside a declaration, including on its own line.
///
/// Deliberately shallow: it takes everything after the first `:` (a struct
/// field) or inside the first `(` (a tuple variant or a tuple struct) on a
/// line, which is enough for the substring rules below and needs no Rust
/// parser. It **starts at the declaration line**, because a tuple struct
/// carries its whole payload there — `pub struct Cells(pub Vec<Option<f64>>);`
/// is exactly the shape the rules exist to reject — and it stops on brace
/// balance rather than on an indented `}`, so a one-line or a payload-only
/// declaration does not run on into the next item's fields.
fn field_types(lines: &[&str], start: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut opened = false;
    for line in lines.iter().skip(start) {
        let t = line.trim();
        if !t.starts_with("//") && !t.starts_with("#[") && !t.is_empty() {
            if let Some((_, ty)) = t.split_once(": ") {
                out.push(ty.trim_end_matches(&[',', '}'][..]).trim().to_string());
            } else if let Some(open) = t.find('(')
                && let Some(close) = t.rfind(')')
                && close > open
            {
                out.push(t[open + 1..close].to_string());
            }
        }
        for c in line.chars() {
            match c {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        // A unit or tuple struct never opens a brace and ends at its `;`.
        if (opened && depth <= 0) || (!opened && t.ends_with(';')) {
            break;
        }
    }
    out
}

/// Check one declaration against the rules its class owes.
fn check(decl: &Decl, class: Class) -> Violations {
    let mut out = Vec::new();
    let name = &decl.name;
    // Anchored at the start of an attribute, not a substring of one: a
    // `#[cfg_attr(feature = "never-on", non_exhaustive)]` or a `#[deprecated]`
    // whose note happens to say "non_exhaustive" would otherwise satisfy the
    // rule in every real build.
    let attr = |name: &str| {
        decl.attrs
            .iter()
            .any(|a| a.trim_start_matches("#[").trim_start().starts_with(name))
    };
    // A derive, in either spelling: `serde::Serialize` and the imported
    // `Serialize` are the same derive and the rule is about the derive.
    let derives = |path: &str| {
        let bare = path.rsplit("::").next().unwrap_or(path);
        decl.attrs.iter().any(|a| {
            a.contains("derive")
                && (a.contains(path)
                    || a.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .any(|word| word == bare))
        })
    };

    // `Class::Borrowed` is the lifetime exemption: it is what such a type is
    // classified as, so there is no second list to keep in step with this one.
    if decl.generic && class != Class::Borrowed {
        out.push(format!(
            "{name}: declares generic parameters or a lifetime (ADR-0006: no generics, lifetimes, or trait objects)"
        ));
    }

    if class == Class::Handle {
        // A handle owns state; none of the plain-data rules apply to it.
        return out;
    }

    if class == Class::Suite {
        // Plain data, checked below, but not `#[non_exhaustive]`: a runner in
        // this repository writes `Case` and `Args` literals, which is the same
        // reason `Source` is exempt.
    } else if class == Class::Borrowed {
        // The rule that runs the other way: a host has to write the literal.
        if attr("non_exhaustive") {
            out.push(format!(
                "{name}: is #[non_exhaustive], which stops a host writing the \
                 struct literal it has no other way to build"
            ));
        }
    } else if !attr("non_exhaustive") {
        out.push(format!("{name}: is not #[non_exhaustive]"));
    }

    for ty in &decl.field_types {
        if ty.contains("Vec<Option<") {
            out.push(format!(
                "{name}: field type `{ty}` is Vec<Option<…>> (ADR-0006: contiguous values plus a separate u8 mask)"
            ));
        }
        if ty.contains("dyn ") || ty.contains("impl ") {
            out.push(format!("{name}: field type `{ty}` is a trait object"));
        }
        if class == Class::Wire && (ty.contains('&') || ty.contains('\'')) {
            out.push(format!("{name}: field type `{ty}` borrows"));
        }
    }

    if class == Class::Wire {
        for derive in ["serde::Serialize", "serde::Deserialize"] {
            if !derives(derive) {
                out.push(format!("{name}: does not derive {derive}"));
            }
        }
        if !derives("schemars::JsonSchema") {
            out.push(format!(
                "{name}: does not derive schemars::JsonSchema under the `schema` feature"
            ));
        }
        if !decl.attrs.iter().any(|a| a.contains("rename_all")) {
            out.push(format!(
                "{name}: states no serde rename_all, so its wire casing is accidental"
            ));
        }
    }
    out
}

/// Every public type an API module declares is in [`CLASSIFICATION`], and
/// every entry there is a real declaration.
///
/// This is the check that keeps the rest of this file honest. Without it, a
/// type added to `api.rs` tomorrow is checked by nothing and the suite is still
/// green.
#[test]
fn every_public_api_type_is_classified() {
    let declared: BTreeSet<String> = MODULES
        .iter()
        .flat_map(|(_, src)| declarations(src))
        .map(|d| d.name)
        .collect();
    let classified: BTreeSet<String> = CLASSIFICATION
        .iter()
        .map(|(n, _, _)| (*n).to_string())
        .collect();

    // Tied to the table rather than to a round number: the `stale` assertion
    // below already catches a declaration the scanner loses, and a floor with
    // slack in it would let a *second* one go missing first.
    assert_eq!(
        declared.len(),
        classified.len(),
        "the scanner found {} public types across {} API modules and the table \
         names {} — it stopped parsing, or the table has drifted",
        declared.len(),
        MODULES.len(),
        classified.len()
    );
    let unclassified: Vec<&String> = declared.difference(&classified).collect();
    assert!(
        unclassified.is_empty(),
        "public API types with no entry in CLASSIFICATION: {unclassified:?}"
    );
    let stale: Vec<&String> = classified.difference(&declared).collect();
    assert!(
        stale.is_empty(),
        "CLASSIFICATION names types that no longer exist: {stale:?}"
    );
}

/// Every declaration passes the rules its class owes.
#[test]
fn every_api_type_follows_the_rules() {
    let classes: BTreeMap<&str, Class> = CLASSIFICATION.iter().map(|(n, c, _)| (*n, *c)).collect();
    let mut violations = Violations::new();
    for (module, source) in MODULES {
        for decl in declarations(source) {
            let Some(class) = classes.get(decl.name.as_str()) else {
                continue; // reported by `every_public_api_type_is_classified`
            };
            violations.extend(
                check(&decl, *class)
                    .into_iter()
                    .map(|v| format!("{module}: {v}")),
            );
        }
    }
    assert!(
        violations.is_empty(),
        "{} ADR-0006 rule violation(s):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// The `Handle` class exempts a type from every rule, so the set of types in it
/// is pinned rather than left to whoever writes the next entry.
#[test]
fn only_the_session_is_a_handle() {
    let handles: Vec<&str> = CLASSIFICATION
        .iter()
        .filter(|(_, c, _)| *c == Class::Handle)
        .map(|(n, _, _)| *n)
        .collect();
    assert_eq!(
        handles,
        vec!["Session"],
        "`Handle` is checked by nothing, so a second one has to be a decision"
    );
}

/// The re-export table names exactly the foreign types this crate's root
/// re-exports.
///
/// A `pub use` of another crate's type is public API here and invisible to the
/// source scan, so the two are held in step: a re-export added without an entry
/// fails, and an entry left behind after a re-export goes fails too.
#[test]
fn every_foreign_reexport_is_classified() {
    const ROOTS: &[(&str, &str)] = &[
        ("lib.rs", include_str!("../src/lib.rs")),
        ("api.rs", include_str!("../src/api.rs")),
        // `combine.rs` re-exports `CombineOp` and the crate root re-exports it
        // again from there, so the root line names no foreign crate and this is
        // the only file the scan can see it in.
        ("combine.rs", include_str!("../src/combine.rs")),
    ];
    let mut found = BTreeSet::new();
    for (_, source) in ROOTS {
        for line in without_tests(source).lines() {
            let t = line.trim();
            // Only a `pub use` naming another crate: a re-export of this
            // crate's own module is already covered by the source scan.
            if !t.starts_with("pub use ") || !t.contains("fieldglass_core") {
                continue;
            }
            for name in t
                .trim_start_matches("pub use ")
                .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            {
                // A type, by convention: `UpperCamel`, and not a SCREAMING
                // constant. Values (`colormaps`, `PALETTE_LUT_LEN`) are not
                // types and owe none of these rules.
                let mut chars = name.chars();
                let upper_first = chars.next().is_some_and(char::is_uppercase);
                if upper_first && name.chars().any(char::is_lowercase) {
                    found.insert(name.to_string());
                }
            }
        }
    }
    let classified: BTreeSet<String> = FOREIGN_REEXPORTS
        .iter()
        .map(|(n, _, _)| (*n).to_string())
        .collect();
    assert_eq!(
        found, classified,
        "the foreign re-exports and FOREIGN_REEXPORTS have diverged"
    );
    for (name, _, reason) in FOREIGN_REEXPORTS {
        assert!(!reason.is_empty(), "{name} is re-exported with no reason");
    }
}

/// Every exemption states why.
///
/// An exemption with no reason is how a rule stops being a rule: the next
/// person adds one beside it and nobody can say what either was for.
#[test]
fn every_exemption_carries_a_reason() {
    for (name, class, reason) in CLASSIFICATION.iter().chain(FOREIGN_REEXPORTS) {
        if *class != Class::Wire {
            assert!(
                !reason.is_empty(),
                "{name} is exempt from the wire rules with no reason given"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The negative cases
// ---------------------------------------------------------------------------

/// A module that breaks one rule per type, written as source the scanner reads
/// the same way it reads `api.rs`.
///
/// Kept as text rather than as real Rust so each violation can be checked by
/// name: a compile-fail test would only say "this did not compile", and
/// rustdoc does not enforce *which* error a `compile_fail` doctest produced.
const NON_CONFORMING: &str = r#"
/// Generic, which no host binding can name.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GenericDto<T> {
    pub payload: T,
}

/// Missing `#[non_exhaustive]`, so adding a field is a breaking change nobody
/// notices until a downstream build fails.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct OpenDto {
    pub width: u32,
}

/// The engine's own shape, which costs a branch per element to cross a seam
/// and is not a typed array on the other side.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct OptionalCellsDto {
    pub values: Vec<Option<f64>>,
}

/// Borrows, so it cannot be handed to a host that outlives the call.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct BorrowingDto {
    pub label: &'static str,
}

/// No derives and no casing: nothing can be generated from it.
#[non_exhaustive]
pub struct BareDto {
    pub width: u32,
}

/// A tuple struct: its whole payload is on the declaration line, which a scan
/// that started one line later would never read.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Cells(pub Vec<Option<f64>>);

/// The same, written on one line, which is where a brace-counting scan can run
/// on into the next item.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct OneLiner { pub values: Vec<Option<f64>> }

/// A conforming type immediately after those two: if either ran on, this one's
/// clean fields would be read as theirs, or theirs as this one's.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AfterThem {
    pub values: Vec<f64>,
}

/// Borrows, so a host has to write the struct literal — and `#[non_exhaustive]`
/// is exactly what stops it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SealedBorrowingInput<'a> {
    pub geometry: &'a GridGeometry,
}
"#;

/// Each rule, shown rejecting a type that breaks exactly it.
///
/// The assertions name the rule *and* the type, so a scanner that reported
/// every type for every rule — which would also make this pass — fails on the
/// counts.
#[test]
fn the_scanner_rejects_a_non_conforming_type() {
    let decls = declarations(NON_CONFORMING);
    let found: BTreeSet<&str> = decls.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        found,
        BTreeSet::from([
            "GenericDto",
            "OpenDto",
            "OptionalCellsDto",
            "BorrowingDto",
            "BareDto",
            "SealedBorrowingInput",
            "Cells",
            "OneLiner",
            "AfterThem",
        ]),
        "the scanner did not find every type in the non-conforming module"
    );

    let report = |name: &str| -> Violations {
        let decl = decls
            .iter()
            .find(|d| d.name == name)
            .unwrap_or_else(|| panic!("{name}"));
        check(decl, Class::Wire)
    };

    // The conforming control. Without it, a `check` that returned a violation
    // for everything would pass every assertion below.
    let control = declarations(
        r#"
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GoodDto {
    pub width: u32,
    pub values: Vec<f64>,
    pub mask: Vec<u8>,
    pub label: String,
}
"#,
    );
    assert_eq!(control.len(), 1);
    assert!(
        check(&control[0], Class::Wire).is_empty(),
        "the control type must pass: {:?}",
        check(&control[0], Class::Wire)
    );

    // The declaration-line payloads, and the clean type after them. All three
    // together: a scanner that read one line too late reports nothing for the
    // first two, and one whose run-on swallowed a neighbour reports something
    // for the third.
    for name in ["Cells", "OneLiner"] {
        let v = report(name);
        assert_eq!(v.len(), 1, "{name}: {v:?}");
        assert!(v[0].contains("Vec<Option<"), "{name}: {v:?}");
    }
    assert!(
        report("AfterThem").is_empty(),
        "a clean type after a payload-on-the-declaration-line one must stay clean: {:?}",
        report("AfterThem")
    );

    let generic = report("GenericDto");
    assert_eq!(generic.len(), 1, "{generic:?}");
    assert!(generic[0].contains("generic parameters or a lifetime"));

    let open = report("OpenDto");
    assert_eq!(open.len(), 1, "{open:?}");
    assert!(open[0].contains("non_exhaustive"));

    let optional = report("OptionalCellsDto");
    assert_eq!(optional.len(), 1, "{optional:?}");
    assert!(optional[0].contains("Vec<Option<"));

    let borrowing = report("BorrowingDto");
    assert_eq!(borrowing.len(), 2, "{borrowing:?}");
    assert!(borrowing.iter().any(|v| v.contains("borrows")));
    assert!(borrowing.iter().any(|v| v.contains("serde::Deserialize")));

    // The rule that runs the other way. `SealedBorrowingInput` is exempt from
    // the lifetime rule the way `Source` is, and is reported for the one thing
    // that would actually stop a host using it.
    let sealed = {
        let decl = decls
            .iter()
            .find(|d| d.name == "SealedBorrowingInput")
            .expect("SealedBorrowingInput");
        check(decl, Class::Borrowed)
    };
    assert_eq!(sealed.len(), 1, "{sealed:?}");
    assert!(sealed[0].contains("#[non_exhaustive]"));

    let bare = report("BareDto");
    assert_eq!(bare.len(), 4, "{bare:?}");
    for rule in [
        "serde::Serialize",
        "serde::Deserialize",
        "schemars::JsonSchema",
        "rename_all",
    ] {
        assert!(bare.iter().any(|v| v.contains(rule)), "{bare:?}");
    }
}

// ---------------------------------------------------------------------------
// The schema half
// ---------------------------------------------------------------------------

/// Whether a generated schema describes an array whose elements may be null —
/// the JSON shape of a `Vec<Option<f64>>`, which is the one thing ADR-0006
/// names outright as not allowed to cross a seam.
///
/// Checked on the schema rather than on the source because a `type` alias, a
/// newtype, or a `serde(with = …)` can hide the Rust spelling while producing
/// exactly this JSON.
fn describes_an_optional_element_array(schema: &serde_json::Value) -> bool {
    fn walk(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Object(o) => {
                let is_array = o.get("type").and_then(|t| t.as_str()) == Some("array");
                if is_array && let Some(items) = o.get("items") {
                    let nullable = match items.get("type") {
                        Some(serde_json::Value::Array(kinds)) => {
                            kinds.iter().any(|k| k.as_str() == Some("null"))
                        }
                        _ => false,
                    };
                    if nullable {
                        return true;
                    }
                }
                o.values().any(walk)
            }
            serde_json::Value::Array(a) => a.iter().any(walk),
            _ => false,
        }
    }
    walk(schema)
}

/// No wire type's schema describes an array of possibly-null numbers, and none
/// carries a generic-mangled definition name.
#[test]
fn no_wire_schema_hides_an_optional_element_array() {
    fn check_schema<T: schemars::JsonSchema>(name: &str) {
        let schema = schemars::schema_for!(T);
        let json = serde_json::to_value(&schema).expect("the schema is JSON");
        assert!(
            !describes_an_optional_element_array(&json),
            "{name}: its schema describes an array of possibly-null elements"
        );
        // `schemars` names a generic instantiation `Foo_for_Bar`. A definition
        // spelled that way means a type parameter reached the wire.
        let text = json.to_string();
        assert!(
            !text.contains("_for_"),
            "{name}: its schema names a generic instantiation"
        );
    }

    check_schema::<Georef>("Georef");
    check_schema::<Field>("Field");
    check_schema::<Line>("Line");
    check_schema::<MessageInfo>("MessageInfo");
    check_schema::<CombineOpInfo>("CombineOpInfo");
    check_schema::<Warped>("Warped");
    check_schema::<Probe>("Probe");
    check_schema::<Isoline>("Isoline");
    check_schema::<Stats>("Stats");
    check_schema::<Values>("Values");
    check_schema::<Raster>("Raster");
    check_schema::<Error>("Error");
    check_schema::<RenderOptions>("RenderOptions");
    check_schema::<DecodeOptions>("DecodeOptions");
    check_schema::<WarpOptions>("WarpOptions");
    check_schema::<PaletteOptions>("PaletteOptions");
    check_schema::<SourceFormat>("SourceFormat");
    check_schema::<Dtype>("Dtype");
    check_schema::<AxisUnits>("AxisUnits");
    check_schema::<Addressing>("Addressing");
    check_schema::<DimensionInfo>("DimensionInfo");
    check_schema::<VariableInfo>("VariableInfo");
    check_schema::<LeftOutArray>("LeftOutArray");

    assert_covers_every_wire_type(
        "no_wire_schema_hides_an_optional_element_array",
        SCHEMA_CHECKED,
    );
}

/// The wire types the schema test covers — see [`assert_covers_every_wire_type`].
const SCHEMA_CHECKED: &[&str] = &[
    "Georef",
    "Field",
    "Line",
    "MessageInfo",
    "CombineOpInfo",
    "Warped",
    "Probe",
    "Isoline",
    "Stats",
    "Values",
    "Raster",
    "Error",
    "RenderOptions",
    "DecodeOptions",
    "WarpOptions",
    "PaletteOptions",
    "SourceFormat",
    "Dtype",
    "AxisUnits",
    "Addressing",
    "DimensionInfo",
    "VariableInfo",
    "LeftOutArray",
];

/// A type that carries the engine's own `Vec<Option<f64>>` across the seam.
/// Exists only to be rejected.
#[derive(schemars::JsonSchema)]
#[allow(dead_code)]
struct NonConformingSchema {
    values: Vec<Option<f64>>,
}

/// The schema rule, shown failing.
#[test]
fn the_schema_rule_rejects_an_optional_element_array() {
    let bad = serde_json::to_value(schemars::schema_for!(NonConformingSchema))
        .expect("the schema is JSON");
    assert!(
        describes_an_optional_element_array(&bad),
        "the rule must reject a Vec<Option<f64>> field, or it checks nothing"
    );

    // The control: the conforming shape the same data takes on the wire.
    let good = serde_json::to_value(schemars::schema_for!(Warped)).expect("the schema is JSON");
    assert!(!describes_an_optional_element_array(&good));
}

/// `Georef::scan` is `fieldglass_core::Scan`, described by hand because `core`
/// cannot derive `JsonSchema`. Its wire casing therefore has to be asserted
/// here rather than derived from an attribute the scanner can read.
#[test]
fn the_core_types_on_the_wire_are_camel_cased() {
    let scan = serde_json::to_value(fieldglass::Scan::new(true, false, true)).expect("serialises");
    let keys: BTreeSet<&str> = scan
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["iNegative", "jPositive", "jConsecutive"]),
        "Scan crosses the wire beside camelCase DTOs and must match them"
    );
}
