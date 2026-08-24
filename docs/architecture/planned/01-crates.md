# Planned — Level 1: crates

After milestones 7, 10, and 11. Compare with [`../01-crates.md`](../01-crates.md).

Two hosts instead of one, and two new pure crates below them. The rule from
today's diagram survives unchanged: no format crate depends on another, and
nothing below a host depends on a host. `fieldglass-wasm` is the second
consumer of the format crates and the reason the render orchestration moves
from `napi` into `core` (#464).

```mermaid
flowchart TD
    app["fieldglass-app<br/><i>browser map creator (external, private)</i>"]
    ext["VS Code extension<br/><i>TypeScript</i>"]
    napi["fieldglass-napi<br/><i>N-API boundary (Node addon)</i>"]
    wasm["fieldglass-wasm #460<br/><i>wasm-bindgen façade, publish = false</i>"]
    fetchplan["fieldglass-fetchplan #461<br/><i>manifests in, byte ranges out; no I/O</i>"]
    zarr["fieldglass-zarr #246<br/><i>bytes-in chunk decode: zstd, blosc, shuffle</i>"]
    grib1["fieldglass-grib1<br/><i>GRIB1 decode</i>"]
    grib2["fieldglass-grib2<br/><i>GRIB2 decode</i>"]
    netcdf["fieldglass-netcdf<br/><i>NetCDF classic + NetCDF-4 / HDF5</i>"]
    core["fieldglass-core<br/><i>traits, GridGeometry, projection, warp, render orchestration</i>"]
    verify["fieldglass-verify #205<br/><i>Verus proofs; own workspace, never shipped</i>"]

    ext --> napi
    app --> wasm
    napi --> grib1
    napi --> grib2
    napi --> netcdf
    napi --> zarr
    napi --> core
    wasm --> grib1
    wasm --> grib2
    wasm --> netcdf
    wasm --> zarr
    wasm --> fetchplan
    wasm --> core
    grib1 --> core
    grib2 --> core
    netcdf --> core
    zarr --> core
    fetchplan --> grib2
    verify -. proves .-> core
    verify -. proves .-> grib1
    verify -. proves .-> grib2
    verify -. proves .-> netcdf

    classDef planned stroke-dasharray: 6 4
    classDef external fill:none,stroke-dasharray: 2 3
    class wasm,fetchplan,zarr,verify planned
    class app,ext external
```

**What each new crate is for**

| Crate | Milestone | Depends on | Why it is its own crate |
| --- | --- | --- | --- |
| `fieldglass-wasm` | 11 | format crates, `core`, `fetchplan` | A cdylib for `wasm32-unknown-unknown`; DTO glue only once #464 lands. The host owns memory and fetching. |
| `fieldglass-fetchplan` | 11 | `grib2` (parameter tables for `.idx` vocabulary, #426) | Pure: reads `.idx` / `.index` / Zarr / kerchunk manifests and returns ranges. No I/O, no clock, so it is testable against fixtures and usable from both hosts. |
| `fieldglass-zarr` | 11 (re-scoped) | `core` | Chunk codecs only (zstd via `ruzstd`, blosc, shuffle, v3 shard inner chunks). Addressing lives in `fetchplan`; directory walking is the napi host's concern. |
| `fieldglass-verify` | 7 | `vstd` only; reads the kernel sources | Deliberately outside the workspace so `cargo build`, `cargo deny`, and the six-target cross-compile never see Verus. Already exists (#197); the proofs (#199–#204) are what is planned. |

**What moves.** `core` grows a `GridGeometry` enum and the render
orchestration (`ResolvedOptions`, warp targets, probe, contour and overlay
projection, CSV) that today sit in `napi` (#464). `napi` and `wasm` keep DTOs
and error mapping. That is the one structural change; everything else is
additive.

**What does not appear.** HTTP range (#247) and S3 (#252) are not crates: per
ADR-0005 the host fetches, so in the browser they are `fetch()` with a `Range`
header driven by `fetchplan`'s output, and in the extension they will be the
same call from TypeScript. `#114` (multi-GB files) is the same seam applied to
a local file.
