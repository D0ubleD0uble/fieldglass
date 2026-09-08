//! Manifests in, byte ranges out.
//!
//! A browser fetches weather fields by HTTP range. Every cloud-native
//! convention publishes a manifest saying where the bytes are — a wgrib2
//! `.idx` (an offset per message), an ECMWF `.index` (offset *and* length),
//! Zarr metadata (a chunk grid), kerchunk references (a chunk key to a url,
//! offset and length). They are dialects of one job. This crate reads them and
//! hands the host a list of ranges.
//!
//! ```
//! use fieldglass_fetchplan::{Manifest, NoResolver, Query, Wgrib2Idx};
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
//! | Dialect | Type | Addressed by | States a length? |
//! |---|---|---|---|
//! | wgrib2 `.idx` | [`Wgrib2Idx`] | a query over parameters and levels | no — the last range is open-ended |
//! | ECMWF `.index` | [`EcmwfIndex`] | a query over parameters and levels | yes — every range is exact |
//! | kerchunk references | [`KerchunkRefs`] | a chunk of an array, by index | yes — every range is exact |
//! | Zarr v2 / v3 metadata | [`ZarrArrayMeta`] | — it *is* the addressing | — it states no ranges |
//!
//! # Two ways to address a container, not one
//!
//! The first two rows are a GRIB stream: a list of self-describing messages, so
//! a request is "which field do I want" and the answer is a
//! [`Query`] over parameters, levels and forecast steps. Both implement
//! [`Manifest`], which is what that shape is called here.
//!
//! The last two are an array store, where a request is "which chunk of which
//! variable", answered in indices. There is no parameter to match and no single
//! object to name — a reference document addresses as many objects as it likes
//! — so [`KerchunkRefs`] is deliberately **not** a [`Manifest`]: implementing it
//! would mean a `key()` picking one URL out of many and a query that never
//! matches. The split is the same one `Session` draws between a message index
//! and a variable, for the same reason.
//!
//! The trait shape is changing (ADR-0010 decision 4, #685): a plan item will
//! carry its own address — a message index or a chunk index — `Manifest` will
//! keep `items()` and `messages()` and lose `key()`, the query will move to a
//! `MessageManifest` extension trait, and `KerchunkRefs` will implement the base
//! trait. The chunk-grid arithmetic under [`ZarrArrayMeta`] moves to
//! `fieldglass-core` in #677.
//!
//! [`ZarrArrayMeta`] reads a metadata document into the shared array model.
//! The arithmetic itself — shape and chunk shape in, the key of the chunk
//! holding a region out — is [`fieldglass_core::array`], because a store
//! walker and a kerchunk planner ask the same questions of the same shape and
//! only differ in where they read it from (ADR-0010 decision 1). What stays
//! here is the reading. The codecs, the data type and the fill value are
//! `fieldglass-zarr`'s, so a host that only wants to know which object to
//! fetch does not link a decompressor to find out.

mod discovery;
mod ecmwf;
mod error;
mod kerchunk;
mod level;
mod manifest;
mod plan;
mod wgrib2;
mod zarr;

pub use discovery::{Candidate, SourceSpec, candidates};
pub use ecmwf::EcmwfIndex;
pub use error::{Dialect, FetchPlanError, Mismatch};
pub use kerchunk::KerchunkRefs;
pub use level::{LevelSpec, Surface, parse_ecmwf_level, parse_ncep_level};
pub use manifest::{Manifest, NoResolver, ParameterResolver, Query};
pub use plan::{Expect, ParameterId, PlanItem, PlanRange};
pub use wgrib2::Wgrib2Idx;
// Re-exported from `core` rather than redefined: the chunk key encoding is
// array-model vocabulary, and a caller holding one from a store walker must be
// able to hand it to this crate (ADR-0010 decision 2).
pub use fieldglass_core::array::{ArrayError, ChunkGrid, ChunkKeyEncoding};
pub use zarr::ZarrArrayMeta;
