# Planned — Level 4: hosts

New altitude. The guarded diagrams stop at the N-API boundary because there is
one host. Milestone 11 adds a second, and the interesting design is in what
each host does *around* the Rust, not inside it.

## Who does what

ADR-0005 fixes the split: the host fetches and owns memory; Rust is a pure
function of bytes. Both hosts follow it, but they differ in what they cache
and in how a file arrives.

```mermaid
flowchart LR
    subgraph vscode["VS Code extension (today)"]
        fs["workspace.fs.readFile"] --> nb["napi handle<br/>reader + decode cache"]
        nb --> canvas["RGBA → canvas<br/>overlays, probe, contours"]
    end
    subgraph browser["fieldglass-app (planned, milestone 11)"]
        cat["sources.json<br/>(catalog is data)"] --> plan["fetchplan #461<br/>.idx → ranges"]
        plan --> fetch["fetch() with Range<br/>public bucket, CORS"]
        fetch --> wb["wasm handle #460<br/>no cache"]
        wb --> gpu["Float32 texture + mask<br/>colour in shader"]
        wb -. CPU fallback .-> canvas2["RGBA → canvas"]
    end
    classDef planned stroke-dasharray: 6 4
    class cat,plan,fetch,wb,gpu,canvas2 planned
```

## One field, from a bucket to a texture

The sequence the browser runs for a single GRIB2 message. Every arrow into
Rust is synchronous; every network wait is in the Worker's JavaScript.

```mermaid
sequenceDiagram
    participant A as app (main thread)
    participant W as Worker (JS)
    participant P as fetchplan (wasm)
    participant B as bucket (HTTPS)
    participant H as wasm handle
    participant G as GPU

    A->>W: want(source, run, field, level)
    W->>B: GET model.grib2.idx
    B-->>W: idx text
    W->>P: Wgrib2Idx::select(field, level)
    P-->>W: PlanItem { range: OpenEnded(offset) }
    W->>B: GET model.grib2 (Range: bytes=offset-)
    B-->>W: one GRIB2 message
    W->>H: open(bytes)
    W->>H: decode(0, { reduce })
    H-->>W: Field { values, mask, georef, stats }
    W-->>A: Field (transfer, zero-copy)
    A->>G: upload R32F + R8 mask, proj4 → mesh
    Note over A,G: restyle = uniform change, no re-decode
```

Notes that shape the two hosts:

- **Memory.** wasm linear memory never shrinks. The façade therefore holds no
  decode cache; the app decides which `Field`s to keep for an animation and
  drops the rest. napi keeps its caches because the extension's repaint loop
  depends on them.
- **Failure.** The wasm crate builds with `panic = "abort"`, so a decoder
  panic ends the Worker. The app treats the Worker as disposable and restarts
  it; the fuzz targets keep that rare.
- **Sizing.** Until #465 the warp output is the source `ni × nj`; after it the
  app asks for a window at a pixel size, which is what a map view needs.
- **Zoom.** For 5.40 (JPEG 2000) fields #463 lets the app decode at a lower
  wavelet level when the view is coarser than the grid; the returned georef is
  derived, not the message's GDS.
- **The extension later.** "Open URL…" in VS Code (#247) is the same
  `fetchplan` call from TypeScript with `fetch()` in the extension host, then
  the existing napi path. Nothing new in Rust.
