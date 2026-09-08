# fieldglass-fetchplan

Manifests in, byte ranges out.

A browser fetches weather fields by HTTP range. Every cloud-native convention
publishes a manifest saying where the bytes are — a wgrib2 `.idx` (an offset per
message), an ECMWF `.index` (offset *and* length), Zarr metadata (a chunk grid),
kerchunk references (a chunk key to a url, offset and length). They are dialects
of one job. This crate reads them and hands the host a list of ranges.

```rust
use fieldglass_fetchplan::{Manifest, NoResolver, Query, Wgrib2Idx};

let sidecar = "\
1:0:d=2026090400:PRMSL:mean sea level:anl:
2:875084:d=2026090400:TMP:2 m above ground:anl:
";
let idx = Wgrib2Idx::parse("gfs.t00z.pgrb2.0p25.f000", sidecar)?;
let hit = &idx.select(&Query::abbreviation("TMP"), &NoResolver)[0];

// The host issues this against the object and checks what comes back.
assert_eq!(hit.range.http_range_header().as_deref(), Some("bytes=875084-"));
# Ok::<(), fieldglass_fetchplan::FetchPlanError>(())
```

## What it is not

**It performs no I/O and never reads the clock.** Fetching lives in the host
(ADR-0005), so the same planner serves the wasm host, a future "Open URL…" in
the VS Code extension, and a fixture test. Run discovery takes `now` as a
parameter for the same reason.

**It depends on no format crate.** A planner that linked a decoder would drag
GRIB2's four codecs into a host that only wanted to know which bytes to ask for.
(`fieldglass-zarr` is a *dev*-dependency, for the seam test below, and reaches
nothing that links this crate.)
Resolving a sidecar's `TMP` / `2 m above ground` to WMO codes needs a table, so
that is the `ParameterResolver` trait, which the `fieldglass` umbrella
implements. Everything here is syntax.

**It hard-codes no bucket, model or URL.** The catalog of sources is data the
host owns; `SourceSpec` is one entry of it, and this crate validates entries and
expands key patterns.

## A plan is a claim, not a fact

A sidecar can be stale — producers regenerate objects, and every offset after
the first change then points into the middle of some message. So every
`PlanItem` carries the `Expect` its manifest line promised, and the consumer
checks the bytes it fetched before decoding them:

```rust
# use fieldglass_fetchplan::{PlanRange, Expect, Mismatch};
// A stale offset lands in packed data rather than on a message boundary.
let fetched = [0x1a, 0x0b, b'x', 0xff, 0, 0, 0, 0];
let err = Expect::default()
    .verify_envelope(&fetched, &PlanRange::OpenEnded { offset: 875_084 })
    .unwrap_err();
assert!(matches!(err, Mismatch::Magic { .. }));
```

`Expect::verify_envelope` is the half that needs no tables: the GRIB magic, the
edition, and the §0 total length against what was actually fetched — both
editions, since §0 is the section they share. The semantic half, that the
parameter and level the message *decodes* to are the ones the line promised,
lives in `fieldglass::fetchplan::verify_message`, where a decoder already is.

## Dialects

| Dialect | Type | Addressed by | States a length? |
|---|---|---|---|
| wgrib2 `.idx` | `Wgrib2Idx` | a query over parameters and levels | no — the last range is open-ended |
| ECMWF `.index` | `EcmwfIndex` | a query over parameters and levels | yes — every range is exact |
| kerchunk references | `KerchunkRefs` | a chunk of an array, by index | yes — every range is exact |
| Zarr v2 / v3 metadata | `ZarrArrayMeta` | — it *is* the addressing | — it states no ranges |

### Two ways to address a container, not one

The first two rows are a GRIB stream: a list of self-describing messages, so a
request is "which field do I want" and the answer is a `Query` over parameters,
levels and forecast steps. Both implement `Manifest`.

The last two are an array store, where a request is "which chunk of which
variable", answered in indices. There is no parameter to match and no single
object to name — a reference document addresses as many objects as it likes — so
`KerchunkRefs` is deliberately **not** a `Manifest`: implementing it would mean a
`key()` picking one URL out of many and a query that never matches.

`ZarrArrayMeta` is the arithmetic under the last row. It reads a `.zarray` or a
`zarr.json` for the chunk grid and the key spelling and for nothing else — the
codecs, the data type and the fill value are `fieldglass-zarr`'s, so a host that
only wants to know which object to fetch does not link a decompressor to find
out. The two crates meet at one seam, and
[`tests/kerchunk_seam.rs`](tests/kerchunk_seam.rs) is that seam: a chunk index
in, a planned range out, those bytes decoded, and the values checked against
what the array was written from.

Of the reference-document spec, version 1 is read, with `{{name}}` substitution
against `templates`. A `gen` block generates references from jinja2 expressions
over a dimension; it is refused, naming itself, rather than half-read by
planning the `refs` beside it and silently omitting the rest.

Three parsing rules here came from reading real sidecars rather than a
specification, and each has a committed fixture behind it — see
[`tests/fixtures/NOTICE.md`](tests/fixtures/NOTICE.md):

- NAM packs `UGRD`/`VGRD` two fields to a message and numbers them `13.1`,
  `13.2`, sharing one offset. Two records, one fetch.
- HRRR writes `var discipline=0 center=7 local_table=1 parmcat=16 parm=201` in
  place of a short name for a parameter wgrib2's own tables do not name — the
  one case where a `.idx` line states WMO codes outright.
- GEFS writes no trailing colon, so the parser drops empty trailing fields
  rather than assuming a terminator.

A fourth came from a test failing against the real NBM sidecar: a plain
deterministic field is published beside five probabilistic ones under the same
abbreviation and level. An under-specified query returns all six —
this crate never guesses between them — and `Query::unqualified()` is how the
deterministic one is named, since it is distinguished by carrying nothing.

## Licence

MIT or Apache-2.0, at your option.
