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
    Addressing, Affine, AxisUnits, AxisValues, ColourRange, CombineOpInfo, ContourOptions,
    DecodeOptions, DimensionInfo, DisplayDecodeOptions, DisplayField, Dtype, Error, Field,
    FullDetail, Georef, GridSupport, Identification, Isoline, LeftOutArray, LeftOutCause, Line,
    LineRequest, MessageIdentity, MessageInfo, MessageProbe, OutOfRange, PaletteOptions,
    PixelProbe, Placement, Positive, Probe, Projected, RangeEnd, RangeRefusal, RangeRefusalReason,
    RangeSource, Raster, RenderOptions, ResolvedOptions, SliceRequest, SourceFormat,
    SpectralTruncation, StatisticalStep, Stats, SurfaceCodes, TargetKind, Values, VariableInfo,
    VectorOptions, VectorPairing, Vertical, WarpOptions, WarpTarget, Warped,
};
// The window planner's (#1083), behind `zarr`, a default feature.
use fieldglass::{ChunkRef, WindowPlan, WindowRequest};
// The planner's wire types (#1081). `fetchplan` is a default feature, so this
// suite names them unconditionally: a build without it is not one this file is
// meant to pass, and a compile error says so louder than a skipped check would.
use fieldglass::fetchplan::{
    Address, Candidate, CandidateOptions, Expect, LevelSpec, ParameterId, PlanItem, PlanRange,
    Query, SourceSpec, Surface,
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
    /// (`conformance/suite.json`). It is a `pub mod` under a published feature,
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
    // A grid's plane origin, step and units, all or none (#870).
    ("Affine", Class::Wire, ""),
    // Why a grid can or cannot be placed, beside the corners it explains (#776).
    ("Placement", Class::Wire, ""),
    // Which point operations a grid takes, asked ahead of the call (#945).
    ("GridSupport", Class::Wire, ""),
    // That a spectral field's map is band-limited below what it declares (#637).
    ("SpectralTruncation", Class::Wire, ""),
    ("Georef", Class::Wire, ""),
    ("Values", Class::Wire, ""),
    ("Stats", Class::Wire, ""),
    ("Field", Class::Wire, ""),
    // A field decoded to draw, possibly at a reduced resolution (#1082).
    ("DisplayField", Class::Wire, ""),
    // A line through an array (#172): a profile or time series at a cell.
    ("Line", Class::Wire, ""),
    ("MessageInfo", Class::Wire, ""),
    // The edition and the identification only that edition carries (#773).
    ("Identification", Class::Wire, ""),
    // What makes a GRIB message the same message in a changed file (#959),
    // and the two §4 groups it carries as written.
    ("MessageIdentity", Class::Wire, ""),
    ("SurfaceCodes", Class::Wire, ""),
    ("StatisticalStep", Class::Wire, ""),
    // The second addressing mode (#662): how a container is addressed, and the
    // variables and dimensions that addressing names.
    ("Addressing", Class::Wire, ""),
    ("DimensionInfo", Class::Wire, ""),
    ("VariableInfo", Class::Wire, ""),
    // How a vector plot is drawn: spacing, the components' convention, and the
    // speed the longest arrow stands for (#241).
    ("VectorOptions", Class::Wire, ""),
    (
        "VectorArrows",
        Class::Engine,
        "the arrows of a vector field plus their scale; a host converts the runs \
         on its way out, as it already does for a coastline's (#241)",
    ),
    // One axis of a variable with its coordinate values, for labelling a
    // cross-section's axes (#171).
    ("AxisValues", Class::Wire, ""),
    // Which way a vertical axis runs, so a host draws it the right way up
    // (#1043).
    ("Vertical", Class::Wire, ""),
    ("Positive", Class::Wire, ""),
    // Whether the two fields arrows are drawn from are a u/v pair (#1049).
    ("VectorPairing", Class::Wire, ""),
    // The arrays a container holds and would not read, one shape for every
    // container (#709).
    ("LeftOutArray", Class::Wire, ""),
    // Why an array is left out or no map, as a code a host words (#1129).
    ("LeftOutCause", Class::Wire, ""),
    ("CombineOpInfo", Class::Wire, ""),
    ("Warped", Class::Wire, ""),
    ("Probe", Class::Wire, ""),
    // A probe by message index: the shown value and, for a band-limited
    // spectral map, the full-detail one (#637).
    ("FullDetail", Class::Wire, ""),
    ("MessageProbe", Class::Wire, ""),
    ("Isoline", Class::Wire, ""),
    // The colour range a render used, and why one was refused (#1155).
    ("ColourRange", Class::Wire, ""),
    ("RangeSource", Class::Wire, ""),
    ("OutOfRange", Class::Wire, ""),
    ("RangeRefusal", Class::Wire, ""),
    ("RangeRefusalReason", Class::Wire, ""),
    ("RangeEnd", Class::Wire, ""),
    // --- error.rs -----------------------------------------------------------
    ("Error", Class::Wire, ""),
    // --- session.rs ---------------------------------------------------------
    ("DecodeOptions", Class::Wire, ""),
    ("DisplayDecodeOptions", Class::Wire, ""),
    // What a JS host's `decodeSlice` and `decodeLine` take (#1073).
    ("SliceRequest", Class::Wire, ""),
    ("LineRequest", Class::Wire, ""),
    ("WarpOptions", Class::Wire, ""),
    ("PaletteOptions", Class::Wire, ""),
    // The range contours are spaced over (#1155).
    ("ContourOptions", Class::Wire, ""),
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
    // --- session/window.rs: a lat/lon box over a variable (#1083) -----------
    ("WindowRequest", Class::Wire, ""),
    ("WindowPlan", Class::Wire, ""),
    ("ChunkRef", Class::Wire, ""),
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
    // --- fetchplan.rs: what the umbrella adds to the planner (#1081) --------
    // The clock and step a host sends to `candidates`.
    ("CandidateOptions", Class::Wire, ""),
    (
        "TableResolver",
        Class::Engine,
        "the GRIB2 tables inverted into a name index; a host holds one and \
         passes it to `select`, and nothing serialises it",
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
        "PrefetchedObjects",
        Class::Engine,
        "the `ObjectSource` a host fills as it fetches (#1124), which names keys \
         it was never given; built in Rust and handed to `Session::open_store`",
    ),
    (
        "FieldglassError",
        Class::Engine,
        "what every method of both seams returns, so a host implementing one has \
         to be able to name it; `Error` is what a host *receives*",
    ),
    // What a host names so it can depend on this crate alone (#574): the addon
    // dropped its direct `fieldglass-core` edge, and these are the pieces of
    // `core` it had been reaching for.
    (
        "GridGeometry",
        Class::Engine,
        "the grid `Georef::geometry` and `Source::geometry` carry; a host holds \
         one between calls and never serialises it — placement reaches the wire \
         as `Georef`",
    ),
    (
        "SpatialIndex",
        Class::Engine,
        "the cell-centre index a lookup `GridGeometry` wraps; a host asks it which \
         way a curvilinear grid's rows run, in Rust",
    ),
    (
        "Format",
        Class::Engine,
        "which container a buffer holds, asked before a host picks a handle; a \
         host maps it to its own string",
    ),
    (
        "ProjectedPolylines",
        Class::Engine,
        "the pixel-space runs `render::overlay_polylines` returns; a host \
         converts them on its way out",
    ),
];

/// The planner types `fieldglass::fetchplan` re-exports from
/// `fieldglass-fetchplan`, classified the way [`FOREIGN_REEXPORTS`] classifies
/// `core`'s (#1081).
///
/// The `Wire` ones cross to a host as they are, so they owe what a DTO owes:
/// they are in the schema, round-trip from a document, and pass the source scan
/// against the planner's own files
/// ([`every_planner_wire_type_follows_the_rules`](fn@every_planner_wire_type_follows_the_rules)).
/// A wire entry's third column is not a reason but an exemption, and only one
/// rule can be exempted: `#[non_exhaustive]`, named by [`PLANNER_EXHAUSTIVE`].
const PLANNER_REEXPORTS: &[(&str, Class, &str)] = &[
    // What a host receives from a plan, and sends back to verify it.
    ("PlanItem", Class::Wire, ""),
    ("PlanRange", Class::Wire, ""),
    ("Address", Class::Wire, ""),
    ("Expect", Class::Wire, ""),
    ("ParameterId", Class::Wire, ""),
    ("LevelSpec", Class::Wire, ""),
    ("Surface", Class::Wire, ""),
    // What a host sends to select, and the run discovery's input and output.
    ("Query", Class::Wire, ""),
    ("SourceSpec", Class::Wire, ""),
    ("Candidate", Class::Wire, ""),
    (
        "Wgrib2Idx",
        Class::Engine,
        "a parsed sidecar a host holds and asks; its items cross, it does not",
    ),
    (
        "EcmwfIndex",
        Class::Engine,
        "a parsed sidecar a host holds and asks; its items cross, it does not",
    ),
    (
        "Manifest",
        Class::Engine,
        "the trait both sidecars and the reference document implement",
    ),
    (
        "MessageManifest",
        Class::Engine,
        "the trait `select` is on; a host calls it, never sends it",
    ),
    (
        "ParameterResolver",
        Class::Engine,
        "the seam `TableResolver` implements, so a host can name it",
    ),
    (
        "NoResolver",
        Class::Engine,
        "the resolver that resolves nothing, for the purely syntactic path",
    ),
    (
        "FetchPlanError",
        Class::Engine,
        "what the planner fails with; a host receives it as `Error`, \
         `decode` or `invalid_option`",
    ),
    (
        "Mismatch",
        Class::Engine,
        "why fetched bytes are not what a plan promised; a host receives it as \
         `Error`, through `fetchplan::verify_envelope`",
    ),
    (
        "Dialect",
        Class::Engine,
        "which grammar a `FetchPlanError` names; reaches a host only as prose",
    ),
    // The chunk half (#687). No host binds it yet; #1081 binds the message half.
    (
        "KerchunkRefs",
        Class::Engine,
        "a parsed reference document; the chunk half, which no host binds yet",
    ),
    (
        "ArrayMetadata",
        Class::Engine,
        "an array's parsed metadata, read in Rust on the way to a chunk key",
    ),
    (
        "ArrayError",
        Class::Engine,
        "why array metadata did not read; reaches a host only as prose",
    ),
    (
        "ChunkGrid",
        Class::Engine,
        "chunk-grid arithmetic, used in Rust to spell a chunk key",
    ),
    (
        "ChunkKeyEncoding",
        Class::Engine,
        "how a chunk key is spelt, read in Rust",
    ),
];

/// Planner wire types exempt from `#[non_exhaustive]`, with the reason.
const PLANNER_EXHAUSTIVE: &[(&str, &str)] = &[(
    "ParameterId",
    "three octets, the identity GRIB2 gives a parameter (discipline, category, \
     number); it cannot grow, and `#[non_exhaustive]` would stop a caller \
     writing the triple it is",
)];

/// The planner's source files, for the source scan. Its types are declared
/// there, not here, so the scan reads them where they are.
const PLANNER_MODULES: &[(&str, &str)] = &[
    (
        "fetchplan/plan.rs",
        include_str!("../../fieldglass-fetchplan/src/plan.rs"),
    ),
    (
        "fetchplan/level.rs",
        include_str!("../../fieldglass-fetchplan/src/level.rs"),
    ),
    (
        "fetchplan/manifest.rs",
        include_str!("../../fieldglass-fetchplan/src/manifest.rs"),
    ),
    (
        "fetchplan/discovery.rs",
        include_str!("../../fieldglass-fetchplan/src/discovery.rs"),
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
    is_wire_shaped::<Affine>();
    is_wire_shaped::<Placement>();
    is_wire_shaped::<GridSupport>();
    is_wire_shaped::<SpectralTruncation>();
    is_wire_shaped::<Georef>();
    is_wire_shaped::<Values>();
    is_wire_shaped::<Stats>();
    is_wire_shaped::<Field>();
    is_wire_shaped::<DisplayField>();
    is_wire_shaped::<MessageInfo>();
    is_wire_shaped::<CombineOpInfo>();
    is_wire_shaped::<Warped>();
    is_wire_shaped::<Probe>();
    is_wire_shaped::<FullDetail>();
    is_wire_shaped::<MessageProbe>();
    is_wire_shaped::<Isoline>();
    is_wire_shaped::<ColourRange>();
    is_wire_shaped::<RangeSource>();
    is_wire_shaped::<OutOfRange>();
    is_wire_shaped::<RangeRefusal>();
    is_wire_shaped::<RangeRefusalReason>();
    is_wire_shaped::<RangeEnd>();
    is_wire_shaped::<Error>();
    is_wire_shaped::<DecodeOptions>();
    is_wire_shaped::<DisplayDecodeOptions>();
    is_wire_shaped::<SliceRequest>();
    is_wire_shaped::<LineRequest>();
    is_wire_shaped::<WarpOptions>();
    is_wire_shaped::<PaletteOptions>();
    is_wire_shaped::<ContourOptions>();
    is_wire_shaped::<Raster>();
    is_wire_shaped::<RenderOptions>();
    is_wire_shaped::<CandidateOptions>();
    is_wire_shaped::<WindowRequest>();
    is_wire_shaped::<WindowPlan>();
    is_wire_shaped::<ChunkRef>();

    // The planner's (#1081), declared in `fieldglass-fetchplan`.
    is_wire_shaped::<PlanItem>();
    is_wire_shaped::<PlanRange>();
    is_wire_shaped::<Address>();
    is_wire_shaped::<Expect>();
    is_wire_shaped::<ParameterId>();
    is_wire_shaped::<LevelSpec>();
    is_wire_shaped::<Surface>();
    is_wire_shaped::<Query>();
    is_wire_shaped::<SourceSpec>();
    is_wire_shaped::<Candidate>();

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

    let mut contours = ContourOptions::default();
    contours.min = Some(1.0);
    contours.scale = Some("log10".to_string());
    assert_eq!(contours.max, None);

    let mut render = RenderOptions::new("equirectangular", "bilinear");
    render.colormap = Some("plasma".to_string());
    render.flip_y = true;
    assert!(render.flip_y);

    let mut at = CandidateOptions::new(1_788_500_000);
    at.step_hours = 6;
    assert_eq!(at.now_unix_secs, 1_788_500_000);

    let mut window = WindowRequest::new(0, 1, 2, [0.0, 10.0, 350.0, 370.0], vec![0, 0, 0]);
    window.slice_indices[0] = 4;
    assert_eq!(window.bounds[3], 370.0);

    let mut query = Query::abbreviation("TMP").at_level_text("2 m above ground");
    query.unqualified = true;
    assert_eq!(query.abbreviation.as_deref(), Some("TMP"));
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
    // A Gaussian grid's affine: present, with a row step it cannot state.
    round_trip::<Affine>(
        "Affine",
        r#"{"x0":0.0,"y0":87.8638,"dx":2.8125,"dy":null,"units":"degrees"}"#,
    );
    round_trip::<Placement>("Placement", r#""predefined_unresolved""#);
    round_trip::<GridSupport>(
        "GridSupport",
        r#"{"contours":true,"vectors":true,"longCsv":true,"zonalMean":false}"#,
    );
    round_trip::<Identification>("Identification", r#"{"edition":"grib1","p1Octet":null}"#);
    round_trip::<Identification>(
        "Identification",
        r#"{"edition":"grib2","discipline":"Meteorological products","productionStatus":"Operational products","dataType":"Analysis and forecast products"}"#,
    );
    round_trip::<MessageIdentity>("MessageIdentity", MESSAGE_IDENTITY_GRIB1_JSON);
    round_trip::<MessageIdentity>("MessageIdentity", MESSAGE_IDENTITY_GRIB2_JSON);
    round_trip::<SurfaceCodes>(
        "SurfaceCodes",
        r#"{"surfaceType":100,"scaleFactor":0,"scaledValue":50000}"#,
    );
    round_trip::<StatisticalStep>(
        "StatisticalStep",
        r#"{"process":1,"lengthUnit":1,"length":6}"#,
    );
    round_trip::<SpectralTruncation>(
        "SpectralTruncation",
        r#"{"declared":7999,"truncatedTo":359}"#,
    );
    round_trip::<Values>("Values", r#"{"dtype":"f32","data":[1.0,2.0]}"#);
    round_trip::<Stats>("Stats", r#"{"min":1.0,"max":2.0,"validCount":2}"#);
    round_trip::<Probe>(
        "Probe",
        r#"{"lat":1.0,"lon":2.0,"i":3.5,"j":4.5,"value":null}"#,
    );
    round_trip::<FullDetail>(
        "FullDetail",
        r#"{"value":318.4,"truncation":{"declared":7999,"truncatedTo":359}}"#,
    );
    round_trip::<MessageProbe>(
        "MessageProbe",
        r#"{"lat":45.5,"lon":120.0,"i":240.0,"j":89.0,"value":325.6,"fullDetail":{"value":318.4,"truncation":{"declared":7999,"truncatedTo":359}}}"#,
    );
    round_trip::<Isoline>("Isoline", r#"{"value":1.0,"segments":[[0.0,1.0,2.0,3.0]]}"#);
    round_trip::<Warped>(
        "Warped",
        r#"{"values":[1.0],"mask":[1],"width":1,"height":1,"bounds":[-1.0,1.0,-2.0,2.0]}"#,
    );
    round_trip::<Raster>(
        "Raster",
        r#"{"rgba":[1,2,3,4],"width":1,"height":1,"range":{"usedMin":1.5,"usedMax":9.0,"rangeSource":"manual_min","center":null,"outOfRange":"clamp","below":3,"above":0,"present":40,"total":64}}"#,
    );
    round_trip::<ColourRange>(
        "ColourRange",
        r#"{"usedMin":1.5,"usedMax":9.0,"rangeSource":"manual_min","center":null,"outOfRange":"clamp","below":3,"above":0,"present":40,"total":64}"#,
    );
    // An empty field with an automatic range: no range, and not an error.
    round_trip::<ColourRange>(
        "ColourRange",
        r#"{"usedMin":null,"usedMax":null,"rangeSource":"auto","center":null,"outOfRange":"clamp","below":0,"above":0,"present":0,"total":4}"#,
    );
    round_trip::<RangeSource>("RangeSource", r#""manual_max""#);
    round_trip::<OutOfRange>("OutOfRange", r#""clamp""#);
    round_trip::<RangeRefusal>(
        "RangeRefusal",
        r#"{"reason":"inverted","end":"min","given":100000.0,"fieldMin":53502.3,"fieldMax":99919.6}"#,
    );
    round_trip::<RangeRefusalReason>("RangeRefusalReason", r#""log10_not_positive""#);
    round_trip::<RangeEnd>("RangeEnd", r#""both""#);
    round_trip::<Error>("Error", r#"{"code":"no_such_message","index":1,"count":0}"#);
    round_trip::<Error>(
        "Error",
        r#"{"code":"invalid_option","detail":"x","rangeRefusal":{"reason":"inverted","end":"min","given":100000.0,"fieldMin":53502.3,"fieldMax":99919.6}}"#,
    );
    round_trip::<Error>(
        "Error",
        r#"{"code":"invalid_option","detail":"x","rangeRefusal":null}"#,
    );
    round_trip::<DecodeOptions>("DecodeOptions", r#"{"dtype":"f64"}"#);
    round_trip::<DisplayDecodeOptions>("DisplayDecodeOptions", r#"{"reduce":1,"dtype":"auto"}"#);
    round_trip::<SliceRequest>(
        "SliceRequest",
        r#"{"variable":1,"yDim":2,"xDim":3,"sliceIndices":[4,0,0,0]}"#,
    );
    round_trip::<LineRequest>(
        "LineRequest",
        r#"{"variable":1,"alongDim":2,"sliceIndices":[4,0,0,0]}"#,
    );
    round_trip::<WarpOptions>(
        "WarpOptions",
        r#"{"bilinear":true,"bounds":null,"width":512,"height":384}"#,
    );
    round_trip::<ContourOptions>(
        "ContourOptions",
        r#"{"min":1.0,"max":null,"center":null,"scale":"log10"}"#,
    );
    round_trip::<PaletteOptions>(
        "PaletteOptions",
        r#"{"colormap":"viridis","colormapTable":null,"reversed":false,"min":null,"max":null,"center":0.0,"scale":null}"#,
    );
    // The three the extension's declarations are generated from (#574). Their
    // documents are long, which is the point: every field name is pinned here.
    round_trip::<Georef>("Georef", GEOREF_JSON);
    // `GEOREF` stands in for the nested document, so the one `Georef` spelling
    // above is the only place its field names are written out.
    round_trip::<Field>("Field", &FIELD_JSON.replace("GEOREF", GEOREF_JSON));
    round_trip::<DisplayField>(
        "DisplayField",
        &DISPLAY_FIELD_JSON.replace("GEOREF", GEOREF_JSON),
    );
    round_trip::<Line>("Line", LINE_JSON);
    round_trip::<MessageInfo>(
        "MessageInfo",
        &MESSAGE_INFO_JSON
            .replace("GEOREF", GEOREF_JSON)
            .replace("IDENTITY", MESSAGE_IDENTITY_GRIB2_JSON),
    );
    round_trip::<CombineOpInfo>("CombineOpInfo", r#"{"value":"a_minus_b","label":"A − B"}"#);
    round_trip::<Addressing>("Addressing", r#""variables""#);
    round_trip::<DimensionInfo>("DimensionInfo", r#"{"name":"time","length":12}"#);
    round_trip::<VariableInfo>(
        "VariableInfo",
        r#"{"index":0,"name":"/g/sst","dims":[{"name":"lat","length":2}],"dtype":"float","units":"K","detectedYDim":0,"detectedXDim":1,"detectedTimeDim":null}"#,
    );
    round_trip::<VectorOptions>(
        "VectorOptions",
        r#"{"spacing":6,"gridRelative":true,"referenceSpeed":25.0}"#,
    );
    round_trip::<AxisValues>(
        "AxisValues",
        r#"{"dimension":"time","length":3,"coordinates":[0.0,6.0,12.0],"units":"hours since 2020-01-01","vertical":null}"#,
    );
    round_trip::<Vertical>("Vertical", r#"{"positive":"down","logPressure":true}"#);
    round_trip::<Positive>("Positive", r#""up""#);
    round_trip::<VectorPairing>("VectorPairing", r#""not_a_pair""#);
    round_trip::<LeftOutArray>(
        "LeftOutArray",
        r#"{"name":"PRODUCT/sst","reason":"unsupported section: Zarr dtype \"<f2\"","cause":{"code":"unsupported_dtype","dtype":"<f2"}}"#,
    );
    // Every variant, because the tag spelling is per variant (#1129).
    round_trip::<LeftOutCause>(
        "LeftOutCause",
        r#"{"code":"unsupported_dtype","dtype":"compound"}"#,
    );
    round_trip::<LeftOutCause>("LeftOutCause", r#"{"code":"text"}"#);
    round_trip::<LeftOutCause>(
        "LeftOutCause",
        r#"{"code":"dimension_clash","dimension":"x"}"#,
    );
    round_trip::<LeftOutCause>("LeftOutCause", r#"{"code":"unsupported"}"#);
    round_trip::<LeftOutCause>("LeftOutCause", r#"{"code":"unreadable_metadata"}"#);
    round_trip::<LeftOutCause>("LeftOutCause", r#"{"code":"too_few_dimensions","rank":1}"#);
    round_trip::<LeftOutCause>("LeftOutCause", r#"{"code":"coordinate"}"#);
    round_trip::<RenderOptions>("RenderOptions", RENDER_OPTIONS_JSON);
    round_trip::<CandidateOptions>(
        "CandidateOptions",
        r#"{"nowUnixSecs":1788500000,"stepHours":6}"#,
    );
    // The window planner's (#1083): a plan over a store, and one over a file
    // read by range, whose `chunks` is null.
    round_trip::<WindowRequest>(
        "WindowRequest",
        r#"{"variable":0,"yDim":1,"xDim":2,"bounds":[55.0,75.0,-75.0,75.0],"sliceIndices":[2,0,0]}"#,
    );
    round_trip::<ChunkRef>(
        "ChunkRef",
        r#"{"array":"t2m","key":"t2m/c/2/0/2","index":[2,0,2]}"#,
    );
    round_trip::<WindowPlan>(
        "WindowPlan",
        r#"{"rows":[0,2],"columns":[[10,12],[0,3]],"ni":5,"nj":2,"chunks":[{"array":"t2m","key":"t2m/2.0.0","index":[2,0,0]}]}"#,
    );
    round_trip::<WindowPlan>(
        "WindowPlan",
        r#"{"rows":[1,3],"columns":[[1,4]],"ni":3,"nj":2,"chunks":null}"#,
    );

    // The planner's (#1081). Every variant of each tagged enum, because the tag
    // spelling is per variant.
    round_trip::<PlanRange>(
        "PlanRange",
        r#"{"kind":"exact","offset":76032983,"length":210}"#,
    );
    round_trip::<PlanRange>("PlanRange", r#"{"kind":"openEnded","offset":490148925}"#);
    round_trip::<PlanRange>("PlanRange", r#"{"kind":"whole"}"#);
    round_trip::<Address>(
        "Address",
        r#"{"kind":"message","index":106,"subIndex":null}"#,
    );
    round_trip::<Address>(
        "Address",
        r#"{"kind":"chunk","array":"temp","index":[1,0]}"#,
    );
    round_trip::<Address>("Address", r#"{"kind":"key","key":"temp/.zarray"}"#);
    round_trip::<ParameterId>("ParameterId", r#"{"discipline":0,"category":1,"number":1}"#);
    round_trip::<Surface>("Surface", r#""isobaric""#);
    round_trip::<Surface>("Surface", r#"{"named":"cloud ceiling"}"#);
    round_trip::<LevelSpec>("LevelSpec", LEVEL_SPEC_JSON);
    round_trip::<Expect>("Expect", &EXPECT_JSON.replace("LEVEL", LEVEL_SPEC_JSON));
    round_trip::<PlanItem>(
        "PlanItem",
        &PLAN_ITEM_JSON.replace("EXPECT", &EXPECT_JSON.replace("LEVEL", LEVEL_SPEC_JSON)),
    );
    round_trip::<Query>("Query", &QUERY_JSON.replace("LEVEL", LEVEL_SPEC_JSON));
    round_trip::<SourceSpec>("SourceSpec", SOURCE_SPEC_JSON);
    round_trip::<Candidate>(
        "Candidate",
        r#"{"key":"gfs.20260904/00/atmos/gfs.t00z.pgrb2.0p25.f000","cycleUnixSecs":1788480000,"stepHours":0}"#,
    );

    assert_covers_every_wire_type("every_wire_type_round_trips_through_json", ROUND_TRIPPED);
}

/// A level with a value, so `value` is pinned present as well as absent.
const LEVEL_SPEC_JSON: &str = r#"{"surface":"isobaric","value":2.0,"value2":null}"#;
/// An `Expect` with every field stated; `LEVEL` stands in for the level.
const EXPECT_JSON: &str = r#"{"abbreviation":"RH","parameter":{"discipline":0,"category":1,"number":1},"level":"2 mb","levelSpec":LEVEL,"forecast":"anl","referenceTime":"2026090400","totalLength":210,"qualifiers":["ENS=+1"],"dialect":"wgrib2_idx"}"#;
const PLAN_ITEM_JSON: &str = r#"{"key":"gfs.20260904/00/atmos/gfs.t00z.pgrb2.0p25.f000","range":{"kind":"exact","offset":76032983,"length":210},"address":{"kind":"message","index":106,"subIndex":null},"expect":EXPECT}"#;
/// A `Query` with every constraint stated.
const QUERY_JSON: &str = r#"{"abbreviation":"RH","parameter":{"discipline":0,"category":1,"number":1},"levelText":"2 mb","level":LEVEL,"forecast":"anl","qualifiers":["ENS=+1"],"unqualified":false}"#;
const SOURCE_SPEC_JSON: &str = r#"{"keyPattern":"gfs.{yyyy}{mm}{dd}/{HH}/atmos/gfs.t{HH}z.pgrb2.0p25.f{fff}","cycleHours":[0,6,12,18],"latencyMinutes":240,"maxCandidates":3}"#;

/// A `Georef` as it crosses the wire. `geometry` is `core`'s tagged enum and is
/// described loosely to a schema consumer on purpose, so the smallest real
/// variant stands in for it here.
const GEOREF_JSON: &str = r#"{"geometry":{"kind":"unsupported","label":"whatever"},"kind":"latlon","label":"latlon","ni":2,"nj":2,"boundsLonlat":[-1.0,1.0,-2.0,2.0],"corners":[1.0,-1.0,-2.0,2.0],"placement":"placed","reprojectable":true,"supports":{"contours":true,"vectors":true,"longCsv":true,"zonalMean":true},"pointsPerRow":null,"proj4":"+proj=longlat","affine":{"x0":0.0,"y0":1.0,"dx":1.0,"dy":-1.0,"units":"degrees"},"periodicX":false,"scan":{"iNegative":false,"jPositive":true,"jConsecutive":false}}"#;

/// A `Field`, with the smallest raster that still has a mask and statistics.
const LINE_JSON: &str = r#"{"values":{"dtype":"f64","data":[6.0,18.0]},"mask":[1,1],"stats":{"min":6.0,"max":18.0,"validCount":2},"variable":"temperature","units":"K","dimension":"time","coordinates":[0.0,6.0],"coordinateUnits":"hours since 2020-01-01 00:00:00","coordinateVertical":null}"#;
const FIELD_JSON: &str = r#"{"values":{"dtype":"f32","data":[1.0,2.0,3.0,4.0]},"mask":[1,1,1,0],"ni":2,"nj":2,"georef":GEOREF,"truncation":{"declared":7999,"truncatedTo":359},"stats":{"min":1.0,"max":3.0,"validCount":3},"parameter":"Temperature","units":"K"}"#;
/// A `DisplayField`: a `Field`'s layout with a reduction and no statistics.
const DISPLAY_FIELD_JSON: &str = r#"{"values":{"dtype":"f32","data":[1.0,2.0,3.0,4.0]},"mask":[1,1,1,0],"ni":2,"nj":2,"georef":GEOREF,"reduction":1,"truncation":null,"parameter":"Temperature","units":"K"}"#;

/// A GRIB1 `MessageIdentity` with every optional field present.
const MESSAGE_IDENTITY_GRIB1_JSON: &str = r#"{"edition":"grib1","tableVersion":128,"parameter":167,"localCentre":98,"levelType":1,"levelOctets":[0,0],"referenceTime":"2026-01-01T00:00:00Z","timeUnit":1,"p1":6,"p2":0,"timeRange":0,"gridType":0,"gridNumber":255,"dimensions":[360,181]}"#;

/// A GRIB2 `MessageIdentity` with every optional field present, a statistical
/// range among them.
const MESSAGE_IDENTITY_GRIB2_JSON: &str = r#"{"edition":"grib2","discipline":0,"productTemplate":11,"productOctets":"0d","ensembleType":3,"ensembleMember":7,"derivedForecast":2,"localCentre":7,"parameterCategory":1,"parameterNumber":8,"firstSurface":{"surfaceType":1,"scaleFactor":null,"scaledValue":null},"secondSurface":{"surfaceType":255,"scaleFactor":null,"scaledValue":null},"referenceTime":"2026-01-01T00:00:00Z","forecastTimeUnit":1,"forecastTime":0,"rangeEnd":"2026-01-01T06:00:00Z","statistics":[{"process":1,"lengthUnit":1,"length":6}],"gridTemplate":0,"dimensions":[1440,721],"dataPoints":1038240}"#;

/// A `MessageInfo` with every optional field present, so none of them is pinned
/// only in its absent form.
const MESSAGE_INFO_JSON: &str = r#"{"index":0,"offsetBytes":0,"parameter":"Temperature","abbreviation":"2t","units":"K","level":"2 m above ground","levelType":"heightAboveGround","referenceTime":"2026-01-01T00:00:00Z","forecast":"+6h","packing":"grid_simple","grid":GEOREF,"placement":"placed","reprojectable":true,"supports":{"contours":true,"vectors":true,"longCsv":true,"zonalMean":true},"sizeLabel":"T7999","truncation":{"declared":7999,"truncatedTo":359},"forecastHours":6,"originatingCentre":"Centre 98","subCentre":null,"identification":{"edition":"grib2","discipline":"Meteorological products","productionStatus":"Operational products","dataType":"Analysis and forecast products"},"identity":IDENTITY,"totalLengthBytes":1234,"uvRelativeToGrid":null}"#;

/// A `RenderOptions` with every field stated. `width`/`height` carry real
/// numbers rather than `null`, so the document pins them as JSON *integers*: a
/// host reading `512.0` where the schema says `u32` is the drift this catches.
const RENDER_OPTIONS_JSON: &str = r#"{"projection":"equirectangular","projectionPreset":"atlantic","centerLat":0.0,"centerLon":0.0,"resampling":"bilinear","flipY":false,"rangeMin":null,"rangeMax":null,"rangeCenter":null,"boundsLatMin":null,"boundsLatMax":null,"boundsLonMin":null,"boundsLonMax":null,"colormap":"viridis","colormapTable":null,"reverseColormap":false,"scaleMode":"linear","width":512,"height":384}"#;

/// The wire types the round-trip test covers, so it can be held to the
/// classification rather than to whoever last edited the list.
const ROUND_TRIPPED: &[&str] = &[
    "SourceFormat",
    "Dtype",
    "AxisUnits",
    "Affine",
    "Placement",
    "GridSupport",
    "Identification",
    "MessageIdentity",
    "SurfaceCodes",
    "StatisticalStep",
    "SpectralTruncation",
    "Values",
    "Stats",
    "Probe",
    "FullDetail",
    "MessageProbe",
    "Isoline",
    "ColourRange",
    "RangeSource",
    "OutOfRange",
    "RangeRefusal",
    "RangeRefusalReason",
    "RangeEnd",
    "Warped",
    "Raster",
    "Error",
    "DecodeOptions",
    "DisplayDecodeOptions",
    "SliceRequest",
    "LineRequest",
    "WarpOptions",
    "PaletteOptions",
    "ContourOptions",
    "Georef",
    "Field",
    "DisplayField",
    "Line",
    "MessageInfo",
    "CombineOpInfo",
    "RenderOptions",
    "Addressing",
    "DimensionInfo",
    "VariableInfo",
    "AxisValues",
    "Vertical",
    "Positive",
    "VectorPairing",
    "VectorOptions",
    "LeftOutArray",
    "LeftOutCause",
    "CandidateOptions",
    "WindowRequest",
    "WindowPlan",
    "ChunkRef",
    "PlanItem",
    "PlanRange",
    "Address",
    "Expect",
    "ParameterId",
    "LevelSpec",
    "Surface",
    "Query",
    "SourceSpec",
    "Candidate",
];

/// Fail unless `covered` is exactly the `Class::Wire` half of
/// [`CLASSIFICATION`] and [`PLANNER_REEXPORTS`].
///
/// The lists in this file are hand-written, which is the same fail-open the
/// preamble says the source scan exists to close: a wire type added to
/// `CLASSIFICATION` and to neither list would be checked by the scanner and by
/// nothing else.
fn assert_covers_every_wire_type(what: &str, covered: &[&str]) {
    let wanted: BTreeSet<&str> = CLASSIFICATION
        .iter()
        .chain(PLANNER_REEXPORTS)
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
        Error::invalid_option(String::new()),
    ];
    let mut codes: Vec<String> = every_variant.iter().map(|e| e.code().to_string()).collect();
    codes.sort();
    codes.dedup();
    // The pinned list is behind `conformance`, which is off by default (#927).
    // `cargo test --workspace` has it on, through `fieldglass-napi`'s
    // dev-dependency, and `tests/conformance.rs` refuses to compile in a run
    // without it, so the gate cannot lose this comparison silently.
    #[cfg(feature = "conformance")]
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
    /// The attributes on its fields and variants, as written.
    inner_attrs: Vec<String>,
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
    // The window planner (#1083), a child module of the session's.
    (
        "session/window.rs",
        include_str!("../src/session/window.rs"),
    ),
    ("render.rs", include_str!("../src/render.rs")),
    // Declares no type: the colour-range rule, over `api.rs`'s DTOs (#1155).
    ("colour_range.rs", include_str!("../src/colour_range.rs")),
    // A `pub mod` under a published feature, so its types are public surface
    // even though no host reads them.
    ("conformance.rs", include_str!("../src/conformance.rs")),
    // Declares no type today, and is here so that one added tomorrow is
    // classified rather than unscanned.
    ("shader.rs", include_str!("../src/shader.rs")),
    // What the umbrella adds to the planner: the resolver, and one option type
    // a host sends (#1081). The planner's own types are scanned from its files,
    // through `PLANNER_MODULES`.
    ("fetchplan.rs", include_str!("../src/fetchplan.rs")),
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

/// `source` with every attribute on one line.
///
/// rustfmt breaks a long attribute across lines —
/// `#[serde(\n    skip_serializing_if = "Option::is_none"\n)]` — and a scanner
/// that reads an attribute as the line it starts on would see only
/// `#[serde(` and miss what it says. So an attribute whose brackets do not
/// close on its first line takes the following lines until they do, joined
/// with a space.
fn join_attributes(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut open: Option<(String, i32)> = None;
    for line in source.lines() {
        let depth = |s: &str| {
            s.chars().fold(0i32, |d, c| match c {
                '[' => d + 1,
                ']' => d - 1,
                _ => d,
            })
        };
        if let Some((mut attr, d)) = open.take() {
            attr.push(' ');
            attr.push_str(line.trim());
            let d = d + depth(line);
            if d > 0 {
                open = Some((attr, d));
            } else {
                out.push_str(&attr);
                out.push('\n');
            }
            continue;
        }
        let d = depth(line);
        if line.trim_start().starts_with("#[") && d > 0 {
            open = Some((line.to_string(), d));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if let Some((attr, _)) = open {
        out.push_str(&attr);
    }
    out
}

/// Every `pub struct` / `pub enum` an API module declares.
fn declarations(source: &str) -> Vec<Decl> {
    let joined = join_attributes(without_tests(source));
    let source = joined.as_str();
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
            inner_attrs: inner_attrs(&lines, i),
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

/// The attributes written on a declaration's fields and variants, read the way
/// [`field_types`] reads their types: from the declaration line to the brace
/// that closes it.
fn inner_attrs(lines: &[&str], start: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut opened = false;
    for line in lines.iter().skip(start) {
        let t = line.trim();
        if opened && t.starts_with("#[") {
            out.push(t.to_string());
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
        // The wire contract is that every key of a returned object is present,
        // with `null` for a `None` (#574, decided 2026-09-29). A
        // `skip_serializing_if`, on the type or on one field, drops the key
        // instead, so the field reads `undefined` in JavaScript while the
        // declaration generated from the schema still says `T | null`: the
        // #288 shape, where a `!== null` guard fails open. Nothing else in the
        // pipeline would notice, because the schema does not record the skip.
        if decl
            .attrs
            .iter()
            .chain(&decl.inner_attrs)
            .any(|a| a.contains("skip_serializing"))
        {
            out.push(format!(
                "{name}: uses serde skip_serializing, which leaves a key out of the \
                 wire object instead of writing null (#574)"
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

/// Every `pub use` statement in `source`, joined onto one line.
///
/// A re-export list long enough for rustfmt to wrap spans several lines, and a
/// line-by-line scan would see only its first, which names no type.
fn pub_use_statements(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut open: Option<String> = None;
    for line in without_tests(source).lines() {
        let t = line.trim();
        let mut statement = match open.take() {
            Some(mut s) => {
                s.push(' ');
                s.push_str(t);
                s
            }
            None if t.starts_with("pub use ") => t.to_string(),
            None => continue,
        };
        if statement.ends_with(';') {
            out.push(std::mem::take(&mut statement));
        } else {
            open = Some(statement);
        }
    }
    out
}

/// The planner re-export table names exactly the types `fieldglass::fetchplan`
/// re-exports from `fieldglass-fetchplan` (#1081), and each one not on the wire
/// says why.
#[test]
fn every_planner_reexport_is_classified() {
    let source = include_str!("../src/fetchplan.rs");
    let mut found = BTreeSet::new();
    for statement in pub_use_statements(source) {
        if !statement.contains("fieldglass_fetchplan") {
            continue;
        }
        for name in statement
            .trim_start_matches("pub use ")
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        {
            // Types only, as in `every_foreign_reexport_is_classified`: the
            // re-exported functions owe none of these rules.
            let upper_first = name.chars().next().is_some_and(char::is_uppercase);
            if upper_first && name.chars().any(char::is_lowercase) {
                found.insert(name.to_string());
            }
        }
    }
    let classified: BTreeSet<String> = PLANNER_REEXPORTS
        .iter()
        .map(|(n, _, _)| (*n).to_string())
        .collect();
    assert!(
        found.len() > 10,
        "the scan found only {found:?}; it stopped reading the re-export list"
    );
    assert_eq!(
        found, classified,
        "the planner re-exports and PLANNER_REEXPORTS have diverged"
    );
}

/// Every planner wire type passes the rules a wire type owes, read from the
/// planner's own source: the derives, a stated casing, no `Vec<Option<…>>`, no
/// skipped key, and `#[non_exhaustive]` unless [`PLANNER_EXHAUSTIVE`] says why
/// not.
#[test]
fn every_planner_wire_type_follows_the_rules() {
    let decls: BTreeMap<String, (&str, Decl)> = PLANNER_MODULES
        .iter()
        .flat_map(|(module, src)| declarations(src).into_iter().map(move |d| (*module, d)))
        .map(|(module, d)| (d.name.clone(), (module, d)))
        .collect();
    let mut violations = Violations::new();
    for (name, class, _) in PLANNER_REEXPORTS {
        if *class != Class::Wire {
            continue;
        }
        let Some((module, decl)) = decls.get(*name) else {
            violations.push(format!("{name}: not declared in any of PLANNER_MODULES"));
            continue;
        };
        let exempt = PLANNER_EXHAUSTIVE.iter().any(|(n, _)| n == name);
        violations.extend(
            check(decl, Class::Wire)
                .into_iter()
                .filter(|v| !(exempt && v.ends_with("is not #[non_exhaustive]")))
                .map(|v| format!("{module}: {v}")),
        );
        if exempt && decl.attrs.iter().any(|a| a.contains("non_exhaustive")) {
            violations.push(format!(
                "{name}: is #[non_exhaustive] and also exempted from it; drop the exemption"
            ));
        }
    }
    for (name, reason) in PLANNER_EXHAUSTIVE {
        assert!(!reason.is_empty(), "{name} is exempted with no reason");
    }
    assert!(
        violations.is_empty(),
        "{} ADR-0006 rule violation(s) on the planner's wire types:\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// The planner's rule check is connected: a wire type with no schema derive is
/// reported, read from text the way the planner's files are.
#[test]
fn the_planner_scan_rejects_a_type_without_its_derives() {
    let bad = "#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]\n\
               #[serde(rename_all = \"camelCase\")]\n\
               #[non_exhaustive]\n\
               pub struct PlanItem {\n    pub key: String,\n}\n";
    let decl = declarations(bad).pop().expect("one declaration");
    let found = check(&decl, Class::Wire);
    assert!(
        found.iter().any(|v| v.contains("schemars::JsonSchema")),
        "{found:?}"
    );
}

/// Every exemption states why.
///
/// An exemption with no reason is how a rule stops being a rule: the next
/// person adds one beside it and nobody can say what either was for.
#[test]
fn every_exemption_carries_a_reason() {
    for (name, class, reason) in CLASSIFICATION
        .iter()
        .chain(FOREIGN_REEXPORTS)
        .chain(PLANNER_REEXPORTS)
    {
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

/// Leaves a key out of the wire object when it has nothing to say, so the
/// field is `undefined` in JavaScript under a declaration saying `T | null`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SkippingDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// The same skip, broken across lines the way rustfmt writes a long attribute:
/// a scanner that reads an attribute as its first line sees only `#[serde(`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WrappedSkippingDto {
    #[serde(
        skip_serializing_if = "Option::is_none"
    )]
    pub note: Option<String>,
}

/// And on the type itself, where it would drop every `None` field at once.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(
    rename_all = "camelCase",
    skip_serializing_if = "Option::is_none"
)]
#[non_exhaustive]
pub struct WrappedTypeSkippingDto {
    pub note: Option<String>,
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
            "SkippingDto",
            "WrappedSkippingDto",
            "WrappedTypeSkippingDto",
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

    for name in [
        "SkippingDto",
        "WrappedSkippingDto",
        "WrappedTypeSkippingDto",
    ] {
        let skipping = report(name);
        assert_eq!(skipping.len(), 1, "{name}: {skipping:?}");
        assert!(
            skipping[0].contains("skip_serializing"),
            "{name}: {skipping:?}"
        );
    }

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
    check_schema::<DisplayField>("DisplayField");
    check_schema::<Line>("Line");
    check_schema::<MessageInfo>("MessageInfo");
    check_schema::<CombineOpInfo>("CombineOpInfo");
    check_schema::<Warped>("Warped");
    check_schema::<Probe>("Probe");
    check_schema::<FullDetail>("FullDetail");
    check_schema::<MessageProbe>("MessageProbe");
    check_schema::<Isoline>("Isoline");
    check_schema::<ColourRange>("ColourRange");
    check_schema::<RangeSource>("RangeSource");
    check_schema::<OutOfRange>("OutOfRange");
    check_schema::<RangeRefusal>("RangeRefusal");
    check_schema::<RangeRefusalReason>("RangeRefusalReason");
    check_schema::<RangeEnd>("RangeEnd");
    check_schema::<Stats>("Stats");
    check_schema::<Values>("Values");
    check_schema::<Raster>("Raster");
    check_schema::<Error>("Error");
    check_schema::<RenderOptions>("RenderOptions");
    check_schema::<DecodeOptions>("DecodeOptions");
    check_schema::<DisplayDecodeOptions>("DisplayDecodeOptions");
    check_schema::<SliceRequest>("SliceRequest");
    check_schema::<LineRequest>("LineRequest");
    check_schema::<WarpOptions>("WarpOptions");
    check_schema::<PaletteOptions>("PaletteOptions");
    check_schema::<ContourOptions>("ContourOptions");
    check_schema::<SourceFormat>("SourceFormat");
    check_schema::<Dtype>("Dtype");
    check_schema::<AxisUnits>("AxisUnits");
    check_schema::<Affine>("Affine");
    check_schema::<Placement>("Placement");
    check_schema::<GridSupport>("GridSupport");
    check_schema::<Identification>("Identification");
    check_schema::<MessageIdentity>("MessageIdentity");
    check_schema::<SurfaceCodes>("SurfaceCodes");
    check_schema::<StatisticalStep>("StatisticalStep");
    check_schema::<SpectralTruncation>("SpectralTruncation");
    check_schema::<Addressing>("Addressing");
    check_schema::<DimensionInfo>("DimensionInfo");
    check_schema::<VariableInfo>("VariableInfo");
    check_schema::<AxisValues>("AxisValues");
    check_schema::<Vertical>("Vertical");
    check_schema::<Positive>("Positive");
    check_schema::<VectorPairing>("VectorPairing");
    check_schema::<VectorOptions>("VectorOptions");
    check_schema::<LeftOutArray>("LeftOutArray");
    check_schema::<LeftOutCause>("LeftOutCause");
    check_schema::<CandidateOptions>("CandidateOptions");
    check_schema::<WindowRequest>("WindowRequest");
    check_schema::<WindowPlan>("WindowPlan");
    check_schema::<ChunkRef>("ChunkRef");
    check_schema::<PlanItem>("PlanItem");
    check_schema::<PlanRange>("PlanRange");
    check_schema::<Address>("Address");
    check_schema::<Expect>("Expect");
    check_schema::<ParameterId>("ParameterId");
    check_schema::<LevelSpec>("LevelSpec");
    check_schema::<Surface>("Surface");
    check_schema::<Query>("Query");
    check_schema::<SourceSpec>("SourceSpec");
    check_schema::<Candidate>("Candidate");

    assert_covers_every_wire_type(
        "no_wire_schema_hides_an_optional_element_array",
        SCHEMA_CHECKED,
    );
}

/// The wire types the schema test covers — see [`assert_covers_every_wire_type`].
const SCHEMA_CHECKED: &[&str] = &[
    "Georef",
    "Field",
    "DisplayField",
    "Line",
    "MessageInfo",
    "CombineOpInfo",
    "Warped",
    "Probe",
    "FullDetail",
    "MessageProbe",
    "Isoline",
    "ColourRange",
    "RangeSource",
    "OutOfRange",
    "RangeRefusal",
    "RangeRefusalReason",
    "RangeEnd",
    "Stats",
    "Values",
    "Raster",
    "Error",
    "RenderOptions",
    "DecodeOptions",
    "DisplayDecodeOptions",
    "SliceRequest",
    "LineRequest",
    "WarpOptions",
    "PaletteOptions",
    "ContourOptions",
    "SourceFormat",
    "Dtype",
    "AxisUnits",
    "Affine",
    "Placement",
    "GridSupport",
    "Identification",
    "MessageIdentity",
    "SurfaceCodes",
    "StatisticalStep",
    "SpectralTruncation",
    "Addressing",
    "DimensionInfo",
    "VariableInfo",
    "AxisValues",
    "Vertical",
    "Positive",
    "VectorPairing",
    "VectorOptions",
    "LeftOutArray",
    "LeftOutCause",
    "CandidateOptions",
    "WindowRequest",
    "WindowPlan",
    "ChunkRef",
    "PlanItem",
    "PlanRange",
    "Address",
    "Expect",
    "ParameterId",
    "LevelSpec",
    "Surface",
    "Query",
    "SourceSpec",
    "Candidate",
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

// ---------------------------------------------------------------------------
// The checked-in schema (#574)
// ---------------------------------------------------------------------------

/// The host contract as one JSON Schema document, relative to this crate's
/// manifest directory (which cargo makes the working directory of a test).
///
/// `tools/gen_api_declarations.py` reads this file and writes the TypeScript
/// declarations both hosts ship, and its `--check` is the drift gate for that
/// half. This test is the drift gate for the other half: the file is what the
/// Rust types describe *now*.
const SCHEMA_PATH: &str = "schema/api.schema.json";

/// Set to rewrite [`SCHEMA_PATH`] rather than compare against it.
const UPDATE_SCHEMA_ENV: &str = "FIELDGLASS_UPDATE_SCHEMA";

/// The option types: what a host **sends**. Every other type in the schema is
/// what a host **receives**.
///
/// The direction decides how a field's optionality reads, and the difference is
/// real. A returned DTO always carries every key, with `null` for a Rust `None`
/// (#574, decided 2026-09-29), so it is described by `schemars`' *serialize*
/// contract, in which every field is required and an `Option` is `T | null`. An
/// option object is read with `#[serde(default)]`, so a key a host leaves out
/// is a default rather than an error: the *deserialize* contract, in which
/// those fields are not required.
///
/// The planner's `PlanItem` is the one type a host sends *back*: it hands the
/// item it was given to `verifyEnvelope`. It is registered as returned, because
/// what a host passes back is the object it received, with every key present.
const SENT: &[&str] = &[
    "DecodeOptions",
    "DisplayDecodeOptions",
    "SliceRequest",
    "LineRequest",
    "WarpOptions",
    "PaletteOptions",
    "ContourOptions",
    "RenderOptions",
    "VectorOptions",
    "CandidateOptions",
    "Query",
    "SourceSpec",
];

/// Wire types that are deliberately not in the schema, with the reason.
///
/// Both are `core`'s, and `core` does not derive `JsonSchema` (the reason
/// `Georef::scan` has a hand-written schema). Neither crosses as serialised
/// JSON: the browser host builds its palette object field by field, with a
/// `Uint8Array` lookup table, and `ScaleMode` crosses only as the string in it.
const NOT_IN_SCHEMA: &[(&str, &str)] = &[
    (
        "Palette",
        "core's type; the wasm host builds its palette object by hand, with typed arrays",
    ),
    (
        "ScaleMode",
        "core's type; crosses only as the `scale` string of the palette object",
    ),
];

/// The schema's top-level key that lists the root types by the umbrella feature
/// a host needs to bind them: `{"always": [...], "fetchplan": [...], "zarr":
/// [...]}`, where `always` is every build.
///
/// The browser package ships two entries (#1158): the default one, without the
/// planner and Zarr stores, and `@fieldglass/wasm/full` with both. Each entry's
/// `.d.ts` declares only what it contains, so `tools/gen_api_declarations.py`
/// splits the declarations along this list: the default file holds what the
/// `always` roots reach, and a feature's file what its own roots reach beyond
/// that. Nested definitions such as `Dialect` follow the roots that reach them.
///
/// The list is not taken on trust: each root is held to the module its type is
/// declared in ([`feature_of_module`]).
const FEATURE_ROOTS_KEY: &str = "x-fieldglass-feature-roots";

/// The feature whose module declares `T`: `"fetchplan"`, `"zarr"`, or
/// `"always"` for one every build has. Each root's registration is held to it,
/// so a planner or Zarr type registered as one every build binds fails instead
/// of reaching the default package's declarations, and the reverse.
///
/// The planner's types are declared in `fieldglass-fetchplan` or the umbrella's
/// `fetchplan` module, both behind `fetchplan`; Zarr's in `fieldglass-zarr` or
/// the umbrella's `session::window`, behind `zarr`. Every-build types come only
/// from the crates [`ALWAYS_CRATES`] names. A type from any other crate answers
/// `"unknown crate"`, which no registration matches, so a type from a crate the
/// umbrella gates behind a new feature cannot land in the default package's
/// declarations by falling through.
///
/// The path is [`std::any::type_name`]'s, whose exact text Rust does not
/// promise. A toolchain that changed it fails the registration loudly, which is
/// the safe direction.
fn feature_of_module<T: ?Sized>() -> &'static str {
    let path = std::any::type_name::<T>();
    let krate = path.split("::").next().unwrap_or_default();
    match krate {
        "fieldglass_fetchplan" => "fetchplan",
        "fieldglass_zarr" => "zarr",
        "fieldglass" if path.starts_with("fieldglass::fetchplan::") => "fetchplan",
        "fieldglass" if path.starts_with("fieldglass::session::window::") => "zarr",
        k if ALWAYS_CRATES.contains(&k) => "always",
        _ => "unknown crate",
    }
}

/// The crates whose wire types every build of a host binds: the umbrella
/// (outside its `fetchplan` and `session::window` modules), core, and the three
/// readers the browser module always links. A crate added here is a claim that
/// no feature of the browser module gates it.
const ALWAYS_CRATES: &[&str] = &[
    "fieldglass",
    "fieldglass_core",
    "fieldglass_grib1",
    "fieldglass_grib2",
    "fieldglass_netcdf",
];

/// [`feature_of_module`] tells the three kinds apart, so the registration check
/// in [`api_schema`] can fail. Without the `zarr` answer a Zarr type registered
/// for every build would pass it.
#[test]
fn the_feature_of_a_root_is_read_off_its_module() {
    assert_eq!(feature_of_module::<PlanItem>(), "fetchplan");
    assert_eq!(feature_of_module::<Candidate>(), "fetchplan");
    assert_eq!(feature_of_module::<CandidateOptions>(), "fetchplan");
    assert_eq!(feature_of_module::<WindowPlan>(), "zarr");
    assert_eq!(feature_of_module::<WindowRequest>(), "zarr");
    assert_eq!(feature_of_module::<MessageInfo>(), "always");
    assert_eq!(feature_of_module::<Error>(), "always");
    // A Zarr crate's own type is Zarr's, and a crate on no list matches no
    // registration rather than defaulting to every build.
    assert_eq!(
        feature_of_module::<fieldglass_zarr::ZarrStore<fieldglass_core::MemoryObjects>>(),
        "zarr"
    );
    assert_eq!(feature_of_module::<serde_json::Value>(), "unknown crate");
}

/// Rebuild every object with its keys in sorted order.
///
/// Not cosmetic. `serde_json`'s map keeps insertion order when any crate in
/// the build turns on its `preserve_order` feature, and sorts by key otherwise,
/// and feature unification means `cargo test -p fieldglass` and `cargo test
/// --workspace` need not agree on which. A file one of them wrote and the other
/// called drift would be a gate that fails at random.
fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let sorted: BTreeMap<String, serde_json::Value> =
                map.into_iter().map(|(k, v)| (k, canonical(v))).collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonical).collect())
        }
        other => other,
    }
}

/// The schema of every wire type, as the file on disk should hold it, the
/// names of the types it was built from, and the names of the definitions the
/// returned (serialize-contract) generator produced.
///
/// Two generators, one per direction (see [`SENT`]), feeding one `$defs` table.
/// A type reachable from both directions must describe the same way in both, or
/// one name would mean two shapes. That is asserted rather than resolved,
/// because the fix is a decision about the type, not about the schema.
fn api_schema() -> (serde_json::Value, Vec<&'static str>, BTreeSet<String>) {
    use schemars::generate::SchemaSettings;

    let mut returned = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let mut sent = SchemaSettings::draft2020_12()
        .for_deserialize()
        .into_generator();
    let mut roots: Vec<&'static str> = Vec::new();
    // The roots by the feature a host needs to bind them; see
    // [`FEATURE_ROOTS_KEY`].
    let mut by_feature: BTreeMap<&'static str, Vec<&'static str>> = BTreeMap::new();

    macro_rules! register {
        ($generator:ident [$feature:literal]: $($t:ident),* $(,)?) => {$(
            $generator.subschema_for::<$t>();
            roots.push(stringify!($t));
            assert_eq!(
                feature_of_module::<$t>(), $feature,
                "{} ({}) is registered for feature {:?}, but the module that declares \
                 it belongs to another",
                stringify!($t), std::any::type_name::<$t>(), $feature,
            );
            by_feature.entry($feature).or_default().push(stringify!($t));
        )*};
    }
    register!(returned ["always"]:
        SourceFormat, Dtype, AxisUnits, Affine, Placement, GridSupport, Georef, Values, Stats, Field, DisplayField, Line,
        MessageInfo, Identification, MessageIdentity, SurfaceCodes, StatisticalStep, Addressing, DimensionInfo, VariableInfo, AxisValues, Vertical, Positive, VectorPairing, LeftOutArray, LeftOutCause,
        CombineOpInfo, Warped, Probe, MessageProbe, FullDetail, SpectralTruncation, Isoline,
        Error, Raster, ColourRange, RangeSource, OutOfRange, RangeRefusal, RangeRefusalReason, RangeEnd,
    );
    register!(returned ["fetchplan"]:
        PlanItem, PlanRange, Address, Expect, ParameterId, LevelSpec, Surface, Candidate,
    );
    register!(returned ["zarr"]: WindowPlan, ChunkRef);
    register!(sent ["always"]:
        DecodeOptions, DisplayDecodeOptions, SliceRequest, LineRequest, WarpOptions, PaletteOptions, ContourOptions, RenderOptions, VectorOptions,
    );
    register!(sent ["fetchplan"]: CandidateOptions, Query, SourceSpec);
    register!(sent ["zarr"]: WindowRequest);
    for names in by_feature.values_mut() {
        names.sort_unstable();
    }

    let mut defs = sent.take_definitions(true);
    let returned_defs = returned.take_definitions(true);
    let returned_names: BTreeSet<String> = returned_defs.keys().cloned().collect();
    for (name, schema) in returned_defs {
        if let Some(other) = defs.get(&name) {
            assert_eq!(
                other, &schema,
                "{name} is both sent and returned, and its two contracts describe \
                 different shapes: one name would mean two types in the declarations"
            );
        }
        defs.insert(name, schema);
    }

    let document = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Fieldglass host API",
        "description": "Generated from the wire types of the `fieldglass` crate by \
            `FIELDGLASS_UPDATE_SCHEMA=1 cargo test -p fieldglass --test api_rules`; do not \
            edit by hand. `tools/gen_api_declarations.py` turns it into the TypeScript both \
            hosts ship. A returned type is the serialize contract (every key present, null \
            for none); an option type a host sends is the deserialize contract (a key left \
            out takes its default).",
        "$defs": defs,
        FEATURE_ROOTS_KEY: by_feature,
    });
    (canonical(document), roots, returned_names)
}

/// The checked-in schema is exactly what the Rust types describe, and it
/// describes every wire type.
///
/// With `FIELDGLASS_UPDATE_SCHEMA` set it rewrites the file and then **fails**,
/// the rule the conformance recording follows: a run that re-recorded has
/// verified nothing, and a variable left set in an environment must not turn
/// the gate into a no-op.
#[test]
fn the_checked_in_schema_is_what_the_types_describe() {
    let (schema, roots, _) = api_schema();
    let text = serde_json::to_string_pretty(&schema).expect("the schema serialises") + "\n";

    assert_covers_every_wire_type("the_checked_in_schema_is_what_the_types_describe", &roots);
    for (name, reason) in NOT_IN_SCHEMA {
        assert!(
            !reason.is_empty(),
            "{name} is left out of the schema with no reason"
        );
        assert!(
            FOREIGN_REEXPORTS
                .iter()
                .any(|(n, c, _)| n == name && *c == Class::Wire),
            "{name} is excused from the schema but is not a foreign wire type"
        );
    }
    let defs = schema["$defs"].as_object().expect("the schema has $defs");
    for root in &roots {
        assert!(
            defs.contains_key(*root),
            "{root} has no definition in the schema"
        );
    }
    for sent in SENT {
        assert!(
            roots.contains(sent),
            "{sent} is listed as sent but is not in the schema"
        );
    }

    if std::env::var_os(UPDATE_SCHEMA_ENV).is_some() {
        std::fs::create_dir_all("schema").expect("the schema directory is creatable");
        std::fs::write(SCHEMA_PATH, &text).expect("the schema is writable");
        panic!(
            "{SCHEMA_PATH} rewritten ({} definitions). Unset {UPDATE_SCHEMA_ENV}, run \
             `python3 tools/gen_api_declarations.py` to regenerate the declarations, and \
             run again to verify.",
            defs.len()
        );
    }

    let on_disk = std::fs::read_to_string(SCHEMA_PATH).unwrap_or_default();
    assert!(
        on_disk == text,
        "{SCHEMA_PATH} is not what the wire types describe. Regenerate it with \
         `{UPDATE_SCHEMA_ENV}=1 cargo test -p fieldglass --test api_rules`, then run \
         `python3 tools/gen_api_declarations.py`."
    );
}

/// Every property of every object a returned definition describes, `Type.field`,
/// that the schema does not list as `required`.
///
/// Inside an internally tagged enum (`#[serde(tag = "…")]`, the shape `Error`
/// takes) each variant is an object whose tag property holds a `const`, and the
/// path names it, `Type[code=Variant].field`, so a finding says which variant.
///
/// Walks the whole definition, so an object nested inline, such as a struct
/// variant of an enum, is held to the same rule as the top level.
fn optional_returned_fields(name: &str, schema: &serde_json::Value) -> Vec<String> {
    fn walk(path: &str, v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                // An internally tagged variant names itself by its tag's const.
                let tagged = map
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .and_then(|props| {
                        props.iter().find_map(|(key, sub)| {
                            sub.get("const")
                                .and_then(|c| c.as_str())
                                .map(|value| format!("{path}[{key}={value}]"))
                        })
                    });
                let path = tagged.as_deref().unwrap_or(path);
                if let Some(props) = map.get("properties").and_then(|p| p.as_object()) {
                    let required: BTreeSet<&str> = map
                        .get("required")
                        .and_then(|r| r.as_array())
                        .map(|r| r.iter().filter_map(|k| k.as_str()).collect())
                        .unwrap_or_default();
                    for key in props.keys() {
                        if !required.contains(key.as_str()) {
                            out.push(format!("{path}.{key}"));
                        }
                    }
                }
                for (key, child) in map {
                    // A property's own schema is reached through `properties`,
                    // so name the path after the property rather than the key.
                    if key == "properties" {
                        if let Some(props) = child.as_object() {
                            for (field, sub) in props {
                                walk(&format!("{path}.{field}"), sub, out);
                            }
                        }
                    } else {
                        walk(path, child, out);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    walk(path, item, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(name, schema, &mut out);
    out
}

/// Every field of a returned type is `required` in the schema (#817).
///
/// A returned DTO carries every key, with `null` for a `None` (#574, decided
/// 2026-09-29), and the TypeScript both hosts ship is generated from this
/// schema, so a field the schema leaves out of `required` becomes an optional
/// property: `undefined` in JavaScript where a `!== null` guard expects `null`,
/// the #288 shape. The source scanner refuses `skip_serializing` by name, but a
/// field can drop out of `required` other ways (a hand-written `JsonSchema`, an
/// attribute the scanner does not know), and the schema is what the
/// declarations are built from, so this checks the schema itself. A type only a
/// host sends ([`SENT`]) never reaches the returned generator and is not walked:
/// it is the deserialize contract. A type both sent and returned is walked,
/// since it is returned, and `api_schema` already holds its two shapes equal.
#[test]
fn every_returned_field_is_required_in_the_schema() {
    let (schema, _, returned_names) = api_schema();
    let defs = schema["$defs"].as_object().expect("the schema has $defs");
    let mut optional = Vec::new();
    for name in &returned_names {
        optional.extend(optional_returned_fields(name, &defs[name.as_str()]));
    }
    assert!(
        optional.is_empty(),
        "returned wire fields the schema makes optional, so the generated TypeScript \
         would read them as `undefined` rather than `null` (#574): {optional:?}"
    );
}

/// The rule above catches what it is for: a returned field that skips
/// serialising its `None`, described by the serialize contract the way
/// [`api_schema`] describes every returned type.
#[test]
fn the_required_rule_rejects_a_skipped_field() {
    use schemars::generate::SchemaSettings;

    #[derive(serde::Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    struct Returned {
        kept: Option<u32>,
        #[serde(skip_serializing_if = "Option::is_none")]
        skipped: Option<u32>,
    }
    #[derive(serde::Serialize, schemars::JsonSchema)]
    #[serde(rename_all = "camelCase")]
    #[allow(dead_code)]
    enum Tagged {
        Variant {
            #[serde(skip_serializing_if = "Option::is_none")]
            inner: Option<u32>,
        },
    }

    let mut generator = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let schema = serde_json::to_value(generator.root_schema_for::<Returned>())
        .expect("the schema serialises");
    assert_eq!(
        optional_returned_fields("Returned", &schema),
        ["Returned.skipped"]
    );
    let schema =
        serde_json::to_value(generator.root_schema_for::<Tagged>()).expect("the schema serialises");
    let found = optional_returned_fields("Tagged", &schema);
    assert!(
        found.iter().any(|f| f.ends_with(".inner")),
        "a struct variant's skipped field is found: {found:?}"
    );

    // An internally tagged enum, the shape `Error` takes: the path names the
    // variant through its tag.
    #[derive(serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "code", rename_all = "camelCase")]
    #[allow(dead_code)]
    enum Coded {
        First {
            kept: u32,
        },
        Second {
            #[serde(skip_serializing_if = "Option::is_none")]
            dropped: Option<u32>,
        },
    }
    let schema =
        serde_json::to_value(generator.root_schema_for::<Coded>()).expect("the schema serialises");
    assert_eq!(
        optional_returned_fields("Coded", &schema),
        ["Coded[code=second].dropped"]
    );
}
