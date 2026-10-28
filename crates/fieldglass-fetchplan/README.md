# fieldglass-fetchplan

Works out which bytes of a remote weather file to fetch, from the index
published beside it: a wgrib2 `.idx`, an ECMWF `.index`, kerchunk references or
Zarr metadata. This crate is usable on its own, and is part of
[Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

Depend on it directly if you're writing a program that fetches part of a
remote GRIB file or Zarr store itself, such as a web page or a download tool,
and wants a plan without linking any decoder. To fetch and decode in one place,
use [`fieldglass`](https://crates.io/crates/fieldglass) with its `fetchplan`
feature, or the browser package `@fieldglass/wasm/full`.

## What it reads

A forecast file can be hundreds of megabytes when you only want one field of
it, and a Zarr store can be thousands of objects when you only want one
corner. So publishers put a small index beside the data that says where each
piece starts. These indexes come in a few dialects, but they all do the same
job. This crate reads them and hands you a plan: which bytes to fetch, and
which field or chunk each range is. Here it finds the 2 m temperature in a GFS
sidecar:

```rust
use fieldglass_fetchplan::{MessageManifest, NoResolver, Query, Wgrib2Idx};

let sidecar = "\
1:0:d=2026090400:PRMSL:mean sea level:anl:
2:875084:d=2026090400:TMP:2 m above ground:anl:
";
let idx = Wgrib2Idx::parse("gfs.t00z.pgrb2.0p25.f000", sidecar)?;
let hit = &idx.select(&Query::abbreviation("TMP"), &NoResolver)[0];

// The host issues this against the object and checks what comes back.
assert_eq!(hit.range.http_range_header()?.as_deref(), Some("bytes=875084-"));
# Ok::<(), fieldglass_fetchplan::FetchPlanError>(())
```

| Index | Type | Each item points at | States a length? |
|---|---|---|---|
| wgrib2 `.idx` | `Wgrib2Idx` | a message, and a field within it | No, so the last range is open-ended |
| ECMWF `.index` | `EcmwfIndex` | a message | Yes, every range is exact |
| kerchunk references | `KerchunkRefs` | a chunk of an array, by index | Yes, every range is exact |
| Zarr v2 and v3 metadata | `ArrayMetadata` | nothing; it is the chunk grid itself | No ranges at all |

Every index gives you the same kind of plan, a list of `PlanItem`s, so a
program can fetch one without knowing which dialect wrote it. What differs is
how you ask for part of it. A GRIB file is a list of fields, so you ask for a
field with a `Query` over its parameter, level and forecast step. A kerchunk
document describes an array store, so you ask for a chunk by its position with
`KerchunkRefs::chunk_at`. `KerchunkObjects` goes one step further and presents
a reference document, with a `ByteSource` for each file it points into, as a
store that `fieldglass-zarr` reads as it is. Of the kerchunk format, version 1
is read, including `{{name}}` substitution from its `templates`.

It also helps a program find the newest model run. `candidates` takes a
source's key pattern and cycle times, and the current time you pass it, and
returns the runs that should have posted by now, newest first. A fetch still
has to find out which of them actually has.

What it doesn't do:

- It doesn't read a **kerchunk `gen` block**, which generates references from
  templates. It refuses the document and names the block, instead of planning
  the references beside it and leaving the rest out.
- It doesn't **guess** between matches. When a query fits several fields, such
  as NBM's plain forecast and its five probabilities under one name and level,
  you get all of them. `Query::unqualified()` picks the plain one.

## A plan is a claim, not a fact

An index can be out of date. When a publisher regenerates a file, every offset
after the first change points into the middle of some other message. So every
`PlanItem` carries an `Expectation` of what its index line promised, and you
check the bytes you fetched before decoding them:

```rust
# use fieldglass_fetchplan::{PlanRange, Expectation, Mismatch};
// A stale offset lands in packed data rather than on a message boundary.
let fetched = [0x1a, 0x0b, b'x', 0xff, 0, 0, 0, 0];
let err = Expectation::default()
    .verify_envelope(&fetched, &PlanRange::OpenEnded { offset: 875_084 })
    .unwrap_err();
assert!(matches!(err, Mismatch::Magic { .. }));
```

`Expectation::verify_envelope` checks what needs no tables: that the bytes
start with the GRIB marker, the edition, and that the message's stated length
matches what you fetched, for both editions. Checking that the field decodes
to the parameter and level the line promised needs a decoder, so that lives in
`fieldglass::fetchplan::verify_message`.

## Feature flags

| Feature | Default | What it adds |
|---|---|---|
| `schema` | off | JSON Schema for the types a program sends and receives, such as a plan item, a query and a source. The browser package's TypeScript types are generated from it. |

## What it is not

It doesn't fetch anything or read the clock. Your program does the fetching
and passes the time in, so the same planner works in a browser, in a native
program and in a test at a fixed instant.

It links no decoder. It reads Zarr metadata through `fieldglass-zarr` with that
crate's compression code turned off, and it takes no GRIB reader at all.
Turning `TMP` and `2 m above ground` into WMO codes needs tables, so that's the
`ParameterResolver` trait, which `fieldglass` implements.

It names no bucket, model or URL. The list of sources is your program's data.
`SourceSpec` is one entry of it, and this crate checks entries and expands
their key patterns.

## How it is checked

The parser is held to real, unmodified sidecars from NOAA (GFS, GEFS, HRRR,
NAM, NBM) and ECMWF. Three of its rules came from reading them: NAM numbers
two fields that share one message `13.1` and `13.2`, HRRR writes WMO codes in
place of a short name wgrib2 doesn't know, and GEFS leaves off the trailing
colon. The kerchunk and Zarr plans are checked by decoding the bytes they
point at.

## Related crates

| Crate | Use it to |
|---|---|
| [`fieldglass`](https://crates.io/crates/fieldglass) | Fetch, check and decode a planned field |
| [`fieldglass-grib2`](https://crates.io/crates/fieldglass-grib2) | Read the GRIB2 messages a plan fetches |
| [`fieldglass-zarr`](https://crates.io/crates/fieldglass-zarr) | Read the Zarr chunks a plan fetches |

## License

MIT OR Apache-2.0

## AI disclosure

Much of this crate's code, tests and documentation was written with AI
assistance (Anthropic's Claude), under the maintainer's direction and review.

### How do I maintain quality?

I treat the AI as a fast contributor whose work I don't take on trust:

- Fieldglass is designed against the specifications and reference libraries are
  used to check against the resulting behaviors. If behaviors differ between
  Fieldglass and a reference, Fieldglass follows the specification and records
  why.
- Every example comes from as close to real-world files as possible while also
  trying not to bloat the size of our libraries with data. In
  `fieldglass-fetchplan`, the sidecars are real ones from NOAA and ECMWF,
  committed unmodified, and the Zarr and kerchunk files are written with
  `zarr-python` and `numcodecs`.
- I take a personal hand in testing each feature and delivery before a new tag
  is released. I maintain a testing plan in which I test each change.
- I have many strict automatic checks for formatting, lints, the test suite,
  dependency and security scans, and fuzzing with damaged or hostile files to
  make sure we refuse them safely.

### Where do I currently notice gaps?

- I dislike the current documentation. There's a lot of it, which would be fine
  if it made the docs more readable but instead it's hard to understand. I am
  undertaking efforts to rewrite the user-facing documentation with my own
  words. This file has been rewritten by me.
- While I have been ensuring proper functionality, behaviors, and
  larger-picture dependency graphs, I haven't been reading over every line of
  code. As the shape of things starts to become a bit more stable, I hope to
  review more of the code closely, but to be totally transparent this is more
  of a 'desire' than a reality currently.
