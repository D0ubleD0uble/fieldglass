# Planned — Level 4: hosts

New altitude. The guarded diagrams stop at the N-API boundary because there is
one host. Milestone 11 adds a second, and the interesting design is in what
each host does *around* the Rust, not inside it.

## Who does what

ADR-0005 fixes the split: the host fetches and owns memory; Rust is a pure
function of bytes. ADR-0006 fixes what a host *is*: a binding over
`fieldglass::Session` that hand-writes only the buffer handoff and the error
mapping. Both hosts follow both, but they differ in what they cache and in
how a file arrives.

```mermaid
flowchart LR
    subgraph vscode["VS Code extension (today)"]
        fs["workspace.fs.readFile"] --> nb["napi handle<br/>reader + decode cache"]
        nb --> canvas["RGBA → canvas<br/>overlays, probe, contours"]
    end
    subgraph browser["fieldglass-app (planned, milestone 11)"]
        cat["sources.json<br/>(catalog is data)"] --> plan["fetchplan in wasm, built behind a feature (#1081)<br/>.idx → ranges + expect"]
        cat --> zplan["Zarr windows in wasm, built behind a feature (#1083)<br/>store metadata → chunk keys"]
        plan --> fetch["fetch() with Range<br/>public bucket, CORS"]
        zplan --> fetch
        fetch --> wb["wasm handle<br/>no cache"]
        wb --> gpu["values + mask textures<br/>+ Palette LUT texture<br/>shipped shader snippet"]
        wb -. CPU fallback .-> canvas2["RGBA → canvas"]
    end
    classDef planned stroke-dasharray: 6 4
    class cat,fetch,gpu,canvas2 planned
```

## One JavaScript API

The two JavaScript hosts bind `Session` under one shape (#1073). Each exports a
`Handle` from `open(bytes)` (a GRIB or NetCDF file) or `openStore(…)` (a Zarr
store: a directory in Node, the fetched objects in the browser), and an
operation both hosts have is spelt the same and takes the same arguments in
both, the camelCase of its `Session` name:

| | both hosts |
|---|---|
| open | `open(bytes)`, `openStore(…)`, `combineOps()` |
| ask first | `format()`, `addressing()` |
| list | `count()`, `message(index)`, `variables()`, `leftOut()`, `undrawable()` |
| decode | `decode(index, { dtype })`, `decodeSlice({ variable, yDim, xDim, sliceIndices }, { dtype })`, `decodeLine({ variable, alongDim, sliceIndices }, { dtype })`, `decodeDisplay(index, { reduce, dtype })` |
| release | `close()`, which is also `[Symbol.dispose]`, so `using` works |

The return types are each host's own, because the bulk buffers are (ADR-0006
decision 1). Beyond the table the browser module has the GPU path (`warp`,
`palette`, `shaderValues`, …) over fields the page holds, and the addon has the
extension's map pipeline (`renderGrid`, `probePixel`, `projectContours`, …)
over a memo, addressed by a target: a message index, a slice, or `{ a, b, op }`.
No name means two operations: the browser's `probe` reads a point of a field,
so the addon's pixel read is `probePixel`. Arguments are objects rather than
positional runs, parameters are camelCase, bytes cross as `Uint8Array`, and no
Rust doc link reaches either `.d.ts`. `tools/check_js_api.py` holds each host's
generated declarations to this in CI.

**Synchronous, with the async names reserved.** The bytes-in path stays the
primary one (ADR-0005), and fieldglass-app uses it. The asynchronous layer
(#1085) lands beside it, purely additively, under names reserved now:

* `load(source, { signal })`, beside `open(bytes)`: the library-side fetch,
  resolving to the same `Handle`;
* `<name>Async(…the same arguments, { signal })` beside each long call
  (`decodeAsync`, `decodeSliceAsync`, `renderGridAsync`, `warpAsync`, …),
  resolving to what the synchronous call returns. On the addon these run as
  napi `AsyncTask`s off the extension host's main thread.

`{ signal }` follows WHATWG: an already-aborted signal rejects at once, and an
abort rejects with `signal.reason`. Neither host may declare a reserved name
before #1085; the same checker fails if one does.

## One window of a Zarr store

A store is many objects, and the browser fetches only the ones a view needs.
The store says which, in three rounds, each synchronous and reading only the
objects already handed over (#1083):

```mermaid
sequenceDiagram
    participant W as Worker (JS)
    participant B as bucket (HTTPS)
    participant H as wasm handle

    W->>B: GET zarr.json (or .zmetadata)
    W->>H: openStore(objects)
    W->>H: coordinateChunks(variable)
    H-->>W: ChunkRef[] for the coordinate arrays
    W->>B: GET each coordinate chunk
    W->>H: openStore(objects + coordinates)
    W->>H: planWindow({ variable, yDim, xDim, bounds, sliceIndices })
    H-->>W: WindowPlan { rows, columns, chunks }
    W->>B: GET each planned chunk
    W->>H: openStore(objects + chunks)
    W->>H: decodeWindow(request)
    H-->>W: Field over the window, placed at its own corners
```

A cell is in the box when its centre is, and a box across the seam of a global
grid is two column runs read as one window. The plan's keys are spelled by the
code the read fetches with (`ZarrStore::chunk_keys`), so fetching exactly them
is enough. A key the Worker does not pass reads as the fill value, as an absent
chunk does. Windows are planned over slices placed on a regular lat/lon grid,
as 1-D latitude and longitude coordinates place them; any other placement is
refused. Built behind `fieldglass-wasm`'s `zarr` feature, and shipped in the
package's `@fieldglass/wasm/full` entry (#1158).

## One field, from a bucket to a texture

The sequence the browser runs for a single GRIB2 message. Every arrow into
Rust is synchronous; every network wait is in the Worker's JavaScript.

```mermaid
sequenceDiagram
    participant A as app (main thread)
    participant W as Worker (JS)
    participant P as fetchplan (via fieldglass, in wasm)
    participant B as bucket (HTTPS)
    participant H as wasm handle
    participant G as GPU

    A->>W: want(source, run, field, level)
    W->>B: GET model.grib2.idx
    B-->>W: idx text
    W->>P: parseWgrib2Idx(key, idx).select(query)
    P-->>W: PlanItem { range, address, expect }
    W->>P: httpRangeHeader(item.range)
    W->>B: GET model.grib2 (Range: bytes=offset-end)
    B-->>W: one GRIB2 message
    W->>P: verifyEnvelope(item, bytes)
    Note over P: GRIB magic, edition and §0 length against the plan
    W->>H: open(bytes)
    W->>H: message(0)
    H-->>W: MessageInfo
    W->>P: verifyMessage(item, info)
    Note over P: parameter, level, reference time and forecast window against the plan
    W->>H: decode(0, { reduce, dtype })
    H-->>W: Field { values, mask, georef, stats }
    W->>H: palette(opts)
    H-->>W: Palette { lut, t0, t1, scale, masked, range }
    W-->>A: Field + Palette (one copy out of wasm memory, then transfer)
    A->>G: upload values + R8 mask + 256×1 LUT, proj4 → mesh
    Note over A,G: restyle = new Palette (1 KB of LUT) plus one pass counting the cells for its range (#1156), no re-decode
    Note over A,G: shader = shipped snippet: normalise, NEAREST LUT lookup
```

Notes that shape the two hosts:

- **Memory.** wasm linear memory never shrinks. The façade therefore holds no
  decode cache; the app decides which `Field`s to keep for an animation and
  drops the rest. napi keeps its caches because the extension's repaint loop
  depends on them.
- **Failure.** The wasm crate builds with `panic = "abort"`, so a decoder
  panic ends the Worker. The app treats the Worker as disposable and restarts
  it; the fuzz targets keep that rare.
- **Sizing.** `warp` outputs the source `ni × nj` unless the caller names a
  `width` × `height`, which is what a map view needs (#465). The render options
  carry the same pair for the two lat/lon-box targets, where the *unnamed*
  default is not `ni × nj` but that shape raised to a 720-pixel long edge (#514),
  or the window's shape for a coordinate-lookup grid (#515). There the size also
  reaches the overlay and contour projections and the pixel probe, so a host that
  sets it for one must set it for all of them.
- **Zoom.** For 5.40 (JPEG 2000) fields #463 lets the app decode at a lower
  wavelet level when the view is coarser than the grid; the returned georef is
  derived, not the message's GDS, and the field is display-only: probe, CSV,
  contours, and stats always use the full-resolution decode. The decode itself
  exists (`Grib2Reader::decode_message_raster_with`; 39.9 ms → 10.5 ms → 2.8 ms
  on the committed 451×337 RAP fixture at reduction 0 / 1 / 2), but no host
  reaches it yet — see the decision recorded under *Decode options* in
  [`02-trait-seams.md`](02-trait-seams.md).
- **Precision.** `Field.values` is f64 unless the packing provably fits f32;
  the R32F texture upload is a downcast the app asks for explicitly.
- **Colour.** Decided once, in Rust: `Palette` feeds both the CPU painter
  and the GPU lookup, and the app's `readPixels` is tested against `render()`.
  The tolerance is one LUT entry, not zero: `Palette::normalise` narrows to
  `f32` and that moves the index for about six positions in a million against
  `Palette::index`, the exact byte the painter emits.
  **That bound holds only if the shader is handed a rebased domain.** A shader
  that subtracts and divides raw `f32` values against raw `f32` bounds loses the
  domain, not just the last bit: once the `f32` gap at `t0` passes one lookup
  step — about `t1 - t0 < 255 · |t0| · 2⁻²³` — the error is unbounded within the
  ramp, measured at 127 entries for a manual range of 1.0 over values near
  `1e7`. Geopotential in m²/s², pressure in Pa and radiances all reach that
  regime under a tight manual range, so the app uploads `v − t0` and the
  cancellation stays in `f64`. `t0`/`t1` are `f64` for this reason. Both the
  bound and the breakdown are pinned by
  `crates/fieldglass-core/tests/palette_golden.rs`.
- **Trust.** A `.idx` range is a claim. `verifyEnvelope` checks the fetched
  bytes against it (magic, edition, length) before they are opened, and throws
  `short_read` for a short transfer and `decode` for bytes that are not the
  message, which is what a stale sidecar produces (#1081). It assumes a
  successful ranged response, so the Worker checks the HTTP status first. The
  planner calls are built behind `fieldglass-wasm`'s `fetchplan` feature and
  ship in the package's second entry, `@fieldglass/wasm/full`, beside Zarr; the
  default entry leaves both out (#1158). `verifyMessage` then checks the opened message's
  parameter, level, reference time and forecast window against the sidecar
  line, comparing values rather than wording (`2 mb` is a message coded at
  200 Pa), and throws `decode` for a valid message that is another field
  (#1093). An ECMWF line's MARS name is checked against eccodes' definition of
  it for ECMWF's open-data parameters, so `mx2t3` is not `mn2t3` (#1116). A
  wgrib2 line's probability, ensemble member, derived forecast and percentile
  are checked against the product template's block, so a probability line is
  not the amount it is a probability of (#1117). A message on a product
  template the reader does not parse (for example the chemistry and aerosol
  templates 4.40–4.48) is checked on its reference time alone (#1126).
- **The extension later.** "Open URL…" in VS Code (#247) is the same
  `fetchplan` call from TypeScript with `fetch()` in the extension host, then
  the existing napi path. Nothing new in Rust.
- **Other hosts.** PyO3 (numpy for the three buffers), a CLI and MCP surface
  (`serde_json` of the same DTOs, #254), and a C ABI (`#[repr(C)]` on types
  that already qualify) are each one more binding of `Session`, not a new
  layer in this diagram.
