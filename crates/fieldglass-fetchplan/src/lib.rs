//! Manifests in, chunk plan out.
//!
//! A browser fetches weather fields by HTTP range. Every cloud-native
//! convention publishes a manifest saying where the bytes are — a wgrib2
//! `.idx` (an offset per message), an ECMWF `.index` (offset *and* length),
//! Zarr metadata (a chunk grid), kerchunk references (a chunk key to a url,
//! offset and length). They are dialects of one job. This crate reads them and
//! hands the host a plan: which bytes to fetch, and which chunk or message each
//! range is.
//!
//! ```
//! use fieldglass_fetchplan::{MessageManifest, NoResolver, Query, Wgrib2Idx};
//!
//! let sidecar = "\
//! 1:0:d=2026090400:PRMSL:mean sea level:anl:
//! 2:875084:d=2026090400:TMP:2 m above ground:anl:
//! ";
//! let idx = Wgrib2Idx::parse("gfs.t00z.pgrb2.0p25.f000", sidecar)?;
//! let hit = &idx.select(&Query::abbreviation("TMP"), &NoResolver)[0];
//!
//! // The host issues this against the object and checks what comes back.
//! assert_eq!(hit.range.http_range_header().as_deref(), Some("bytes=875084-"));
//! # Ok::<(), fieldglass_fetchplan::FetchPlanError>(())
//! ```
//!
//! # What this crate is not
//!
//! **It performs no I/O and never reads the clock.** Fetching lives in the
//! host (ADR-0005 decision 1), so the same planner serves the wasm host, a
//! future "Open URL…" in the extension, and a fixture test. Run discovery takes
//! `now` as a parameter for the same reason.
//!
//! **It depends on no format crate.** A planner that linked a decoder would
//! drag GRIB2's four codecs into a host that only wanted to know which bytes to
//! ask for. Matching a sidecar's `TMP` / `2 m above ground` to a WMO parameter
//! needs a table, so that is a trait — [`ParameterResolver`] — which the
//! `fieldglass` umbrella implements over the GRIB2 tables. Everything here is
//! syntax.
//!
//! **It hard-codes no bucket, model or URL.** The catalog of sources is data
//! the host owns; [`SourceSpec`] is one entry of it, and this crate validates
//! entries and expands key patterns.
//!
//! # A plan is a claim, not a fact
//!
//! A sidecar can be stale — NODD regenerates objects, and every offset after
//! the first change then points into the middle of some message — an
//! open-ended last range can over- or under-shoot, and an `n.m` sub-message
//! shares its offset with its sibling. So every [`PlanItem`] carries the
//! [`Expect`] its manifest line promised, and the consumer checks the bytes it
//! fetched before decoding them:
//!
//! ```
//! # use fieldglass_fetchplan::{PlanRange, Expect, Mismatch};
//! // A stale offset lands in packed data rather than on a message boundary.
//! let fetched = [0x1a, 0x0b, b'x', 0xff, 0, 0, 0, 0];
//! let err = Expect::default()
//!     .verify_envelope(&fetched, &PlanRange::OpenEnded { offset: 875_084 })
//!     .unwrap_err();
//! assert!(matches!(err, Mismatch::Magic { .. }));
//! ```
//!
//! [`Expect::verify_envelope`] is the half that needs no tables: the GRIB
//! magic, the edition, and the §0 total length against what was actually
//! fetched. The semantic half — that the parameter and level the message
//! decodes to are the ones the line promised — belongs with the decoder, and
//! reports through the same [`Mismatch::Field`] shape.
//!
//! # Dialects
//!
//! | Dialect | Type | Implements | Each item's [`Address`] | States a length? |
//! |---|---|---|---|---|
//! | wgrib2 `.idx` | [`Wgrib2Idx`] | [`MessageManifest`] | a message, and a field within it | no — the last range is open-ended |
//! | ECMWF `.index` | [`EcmwfIndex`] | [`MessageManifest`] | a message | yes — every range is exact |
//! | kerchunk references | [`KerchunkRefs`] | [`Manifest`] | a chunk of an array, by index | yes — every range is exact |
//! | Zarr v2 / v3 metadata | [`ArrayMetadata`] | — | — it *is* the addressing | — it states no ranges |
//!
//! # One plan, two ways to ask for part of it
//!
//! Every manifest is a [`Manifest`]: its [`items`](Manifest::items) are the
//! plan, and each [`PlanItem`] carries an [`Address`] saying which message or
//! which chunk the bytes are. A host can fetch a plan without knowing which
//! dialect wrote it (ADR-0010 decision 4, #685).
//!
//! What a request looks like differs, and that is the split. The first two rows
//! are a GRIB stream, a list of self-describing messages, so a request is
//! "which field do I want" and the answer is a [`Query`] over parameters,
//! levels and forecast steps: they implement [`MessageManifest`], the extension
//! trait that carries it. A reference document is an array store, where a
//! request is "which chunk of which variable", answered in indices by
//! [`KerchunkRefs::chunk_at`]; there is no parameter to match, so it is a
//! [`Manifest`] and nothing more. The split is the one `Session` draws between
//! a message index and a variable, for the same reason.
//!
//! [`ArrayMetadata`] reads a metadata document into the shared array model. It
//! is `fieldglass-zarr`'s, taken here with `default-features = false` (#686),
//! so this crate reads a `.zarray` without linking anything that could
//! decompress a chunk — `cargo tree` for it carries no decompressor. Before
//! that there were two readers of one document, with two `zarr_format` checks
//! and two vocabularies for refusing the same things.
//!
//! The arithmetic underneath is [`fieldglass_core::array`], because a store
//! walker and a kerchunk planner ask the same questions of the same shape and
//! differ only in where they read it from (ADR-0010 decision 1).

mod discovery;
mod ecmwf;
mod error;
mod kerchunk;
mod level;
mod manifest;
mod plan;
mod wgrib2;

pub use discovery::{Candidate, SourceSpec, candidates};
pub use ecmwf::EcmwfIndex;
pub use error::{Dialect, FetchPlanError, Mismatch};
pub use kerchunk::KerchunkRefs;
pub use level::{LevelSpec, Surface, parse_ecmwf_level, parse_ncep_level};
pub use manifest::{Manifest, MessageManifest, NoResolver, ParameterResolver, Query};
pub use plan::{Address, Expect, ParameterId, PlanItem, PlanRange};
pub use wgrib2::Wgrib2Idx;
// Re-exported from `core` rather than redefined: the chunk key encoding is
// array-model vocabulary, and a caller holding one from a store walker must be
// able to hand it to this crate (ADR-0010 decision 2).
pub use fieldglass_core::array::{ArrayError, ChunkGrid, ChunkKeyEncoding};
// The one reader of an array's metadata document lives in `fieldglass-zarr`
// (#686), taken here without its codecs. Re-exported so a caller planning a
// fetch needs no second manifest line to name what it planned against.
pub use fieldglass_zarr::ArrayMetadata;
