//! Works out which bytes of a remote weather file to fetch, from the index
//! published beside it.
//!
//! A browser fetches weather fields by HTTP range, and every cloud-native
//! convention publishes an index saying where the bytes are: a wgrib2 `.idx`
//! (an offset per message), an ECMWF `.index` (an offset and a length), Zarr
//! metadata (a chunk grid), and kerchunk references (a chunk key mapped to a
//! URL, offset and length). They're dialects of one job. This crate reads them
//! and hands the host a plan: which bytes to fetch, and which chunk or message
//! each range is.
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
//! assert_eq!(hit.range.http_range_header()?.as_deref(), Some("bytes=875084-"));
//! # Ok::<(), fieldglass_fetchplan::FetchPlanError>(())
//! ```
//!
//! # What this crate is not
//!
//! It does no I/O and never reads the clock. The host does the fetching, so
//! the same planner serves the browser host and a fixture test alike. Run
//! discovery takes `now` as a parameter for the same reason.
//!
//! It links no decoder. A planner that linked one would drag GRIB2's four
//! codecs into a host that only wanted to know which bytes to ask for.
//! Matching a sidecar's `TMP` and `2 m above ground` to a WMO parameter needs a
//! table, so that's a trait, [`ParameterResolver`], which the `fieldglass`
//! crate implements over the GRIB2 tables. Everything here is syntax.
//!
//! It names no bucket, model or URL. The catalog of sources is the host's
//! data. [`SourceSpec`] is one entry of it, and this crate checks entries and
//! expands their key patterns.
//!
//! # A plan is a claim, not a fact
//!
//! A sidecar can go stale. When a publisher regenerates an object, every
//! offset after the first change points into the middle of some message. An
//! open-ended last range can also overshoot or undershoot, and an `n.m`
//! sub-message shares its offset with its sibling. So every [`PlanItem`]
//! carries the [`Expectation`] its index line promised, and the consumer checks
//! the bytes it fetched before decoding them:
//!
//! ```
//! # use fieldglass_fetchplan::{PlanRange, Expectation, Mismatch};
//! // A stale offset lands in packed data rather than on a message boundary.
//! let fetched = [0x1a, 0x0b, b'x', 0xff, 0, 0, 0, 0];
//! let err = Expectation::default()
//!     .verify_envelope(&fetched, &PlanRange::OpenEnded { offset: 875_084 })
//!     .unwrap_err();
//! assert!(matches!(err, Mismatch::Magic { .. }));
//! ```
//!
//! [`Expectation::verify_envelope`] is the half that needs no tables: the GRIB
//! marker, the edition, and the section 0 total length against what was
//! actually fetched. The other half, that the parameter and level the message
//! decodes to are the ones the line promised, belongs with the decoder, and
//! reports through the same [`Mismatch::Field`] shape.
//!
//! # Dialects
//!
//! | Dialect | Type | Implements | Each item's [`Address`] | States a length? |
//! |---|---|---|---|---|
//! | wgrib2 `.idx` | [`Wgrib2Idx`] | [`MessageManifest`] | a message, and a field within it | no, so the last range is open-ended |
//! | ECMWF `.index` | [`EcmwfIndex`] | [`MessageManifest`] | a message | yes, every range is exact |
//! | kerchunk references | [`KerchunkRefs`] | [`Manifest`] | a chunk of an array, by index | yes, every range is exact |
//! | Zarr v2 / v3 metadata | [`ArrayMetadata`] | none | none; it *is* the addressing | none; it states no ranges |
//!
//! # One plan, two ways to ask for part of it
//!
//! Every index is a [`Manifest`]. Its [`items`](Manifest::items) are the plan,
//! and each [`PlanItem`] carries an [`Address`] saying which message or which
//! chunk the bytes are, so a host can fetch a plan without knowing which
//! dialect wrote it.
//!
//! What differs is how you ask for part of one. The first two rows are GRIB
//! streams, lists of self-describing messages, so a request is "which field do
//! I want", and the answer is a [`Query`] over parameters, levels and forecast
//! steps. Those dialects implement [`MessageManifest`], the extension trait
//! that carries it. A reference document describes an array store, where a
//! request is "which chunk of which variable", answered in indices by
//! [`KerchunkRefs::chunk_at`]. There's no parameter to match, so it's a
//! [`Manifest`] and nothing more. `Session` draws the same line between a
//! message index and a variable, for the same reason.
//!
//! [`ArrayMetadata`] reads a metadata document into the shared array model. It
//! comes from `fieldglass-zarr`, taken with `default-features = false`, so this
//! crate reads a `.zarray` without linking anything that could decompress a
//! chunk, and `cargo tree` for it shows no decompressor.
//!
//! The arithmetic underneath is [`fieldglass_core::array`], because a store
//! walker and a kerchunk planner ask the same questions of the same shape and
//! differ only in where they read it from.

mod discovery;
mod ecmwf;
mod error;
mod kerchunk;
mod kerchunk_objects;
mod level;
mod manifest;
mod plan;
mod wgrib2;

pub use discovery::{Candidate, MAX_CANDIDATES, SourceSpec, candidates};
pub use ecmwf::EcmwfIndex;
pub use error::{Dialect, FetchPlanError, Mismatch};
pub use kerchunk::KerchunkRefs;
pub use kerchunk_objects::KerchunkObjects;
pub use level::{LevelSpec, Surface, parse_ecmwf_level, parse_ncep_level};
pub use manifest::{Manifest, MessageManifest, NoResolver, ParameterResolver, Query};
pub use plan::{Address, Expectation, ParameterId, PlanItem, PlanRange};
pub use wgrib2::Wgrib2Idx;
// Re-exported from `core` rather than redefined: the chunk key encoding is
// array-model vocabulary, and a caller holding one from a store walker must be
// able to hand it to this crate (ADR-0010 decision 2).
pub use fieldglass_core::array::{ArrayError, ChunkGrid, ChunkKeyEncoding};
// The keyed seam `KerchunkObjects` presents, so a caller reading a reference
// document through it needs no `fieldglass-core` line of its own.
pub use fieldglass_core::bytes::{ByteSource, ObjectSource};
// The one reader of an array's metadata document lives in `fieldglass-zarr`
// (#686), taken here without its codecs. Re-exported so a caller planning a
// fetch needs no second manifest line to name what it planned against.
pub use fieldglass_zarr::ArrayMetadata;
