# fieldglass-fetchplan

Manifests in, byte ranges out.

A browser fetches weather fields by HTTP range. Every cloud-native convention
publishes a manifest saying where the bytes are — a wgrib2 `.idx` (an offset per
message), an ECMWF `.index` (offset *and* length), Zarr metadata, kerchunk
references. They are dialects of one job. This crate reads them and hands the
host a list of ranges.

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

| Dialect | Type | States a length? | Sub-messages |
|---|---|---|---|
| wgrib2 `.idx` | `Wgrib2Idx` | no — the last range is open-ended | `n.m`, sharing one offset |
| ECMWF `.index` | `EcmwfIndex` | yes — every range is exact | none |

Zarr v2/v3 and kerchunk are the same seam over chunk-grid arithmetic and land
with the codec crate, so both halves are tested against the same fixtures.

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
