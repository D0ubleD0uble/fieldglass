# 0006 — Hosts are derived bindings over a plain-data API

**Status:** Accepted (2026-08-24). Shapes #464 and #460 in milestone 11; informs
the PyO3 and CLI hosts named in ROADMAP "New host surfaces" and #254.

## Context

Fieldglass has one host today, `fieldglass-napi`, and milestone 11 adds a
second, `fieldglass-wasm`. The roadmap names a third (PyO3), #254 a fourth (a
CLI), and a C ABI would serve R, Julia, Go, and GDAL from one surface. Each is
a thin layer by intent: the format crates are pure byte-in, values-out
engines, and ADR-0005 keeps decode synchronous so any host can call it.

A review of the napi crate on 2026-08-24 found why "thin" did not hold. Every
host binding is four things: bulk buffers in and out (file bytes, values,
mask, RGBA), small plain data (message metadata, georeferencing, options,
probe results), an error mapping, and method forwarding. In napi the second
and fourth fused with the engine: `MessageMeta`, a `#[napi(object)]` with ~65
`Option` fields, is the struct the nine warp setups, probe, contours, and CSV
compute from, and every helper returns `napi::Result`. The extension reads 16
of those fields. Written the same way, a wasm host would reimplement ~3,900
lines against its own DTOs, and a third host a third time.

The bulk-buffer part is genuinely per host: `Buffer` and `Float64Array` in
Node, `Uint8Array` views over linear memory in wasm, numpy in Python, raw
pointers in C. It is also small, on the order of three functions. Everything
else can be derived if the types it is derived from are plain enough.

Two things this record does not cover, because earlier decisions do: where
bytes come from (ADR-0005, which also owns #114's large-file case), and
whether decode can yield partial results as bytes arrive (evaluated for
milestone 11 and dropped; see decision 4).

## Decision

### 1. One host-neutral API crate, and every host is a binding of it

A new umbrella crate, `fieldglass`, holds a `Session` type with the operations
a host needs — `open`, `count`, `message(i)`, `decode(i, opts)`, `warp`,
`render`, `probe`, `contours`, `overlay`, `csv` — taking `&[u8]` and returning
owned plain data. It depends on `core` and the format crates, and it is the
crate a Rust user reaches for; the format crates stay independently usable.
No host type appears in it.

`GridGeometry`, the typed enum over grid families, lives in `core` behind the
`render` feature so the format crates can convert into it (`From<GridTemplate>`
in grib2, `From<GridDescription>` in grib1, the CF and WRF resolvers in
netcdf). `Session` builds the API DTOs (`Message`, `Georef`, `Field`, …) as views of
it; a host derives its own types from those, never from `core`, and nothing
in Rust reads a DTO back. Dependencies point down only: `core` knows no DTO,
`fieldglass` knows no host.

`fieldglass-napi` and `fieldglass-wasm` keep only the four per-host parts:
buffer conversion, error mapping, forwarding, and packaging. They depend on
`fieldglass` and nothing below it. A host that does none of the engine's work
is the acceptance test for this record.

### 2. API types follow rules a test can enforce

Every type on the `Session` surface:

- has no generics, lifetimes, or trait objects;
- carries fields as contiguous `Vec<f32>` / `Vec<f64>` plus a `Vec<u8>` mask,
  never `Vec<Option<f64>>`; the element type follows the source (f32 only
  when the packing's precision provably fits, f64 otherwise), and a narrower
  type is a downcast the host asks for by name, never a default;
- uses strings only for labels (`kind`, `units`, `parameter`, `proj4`);
- is an enum with stable discriminants where it enumerates, and is
  `#[non_exhaustive]`;
- derives `serde::Serialize` / `Deserialize` and `schemars::JsonSchema`.

`Error` is one enum with a stable `code()` and a `message()`.

The rules are what make derivation possible: napi's `serde-json` feature
returns any `Serialize` type, `serde-wasm-bindgen` or `tsify` derives the JS
side, `pythonize` the Python side, and `#[repr(C)]` is compatible with all of
them. TypeScript declarations are generated from the JSON schema rather than
kept by hand in `native.ts`.

### 3. A conformance suite is part of the API, not of any host

Fixtures and expected outputs for every `Session` operation live in the
`fieldglass` crate as data. Each host runs its own binding through them.
Adding a host is then a checklist: implement the buffer handoff, map the
error, pass the suite.

The same rule covers presentation. Colour, scale, and mask handling are
decided in Rust and exported as data (a `Palette`: LUT, transformed domain,
scale kind, masked colour); a GPU path consumes that data through a shader
snippet the package ships, and never re-derives it. The CPU painter reads the
same `Palette`, so it is the oracle a host's GPU output is tested against
rather than a second implementation.

### 4. What was rejected

**Keep hand-writing each host.** The status quo, and it works for one host.
Rejected because the second host is filed and the third and fourth are named;
the cost is linear in hosts and paid in the code most likely to drift.

**uniffi.** Generates Python, Swift, Kotlin, and Ruby from one definition and
would be the answer for a mobile host. Rejected as the primary mechanism
because it has no JavaScript or wasm story, so the two hosts Fieldglass
actually has would still be hand-written. Not precluded for a future host.

**One `dispatch(json) -> json` entry point.** The minimum possible binding,
and the right shape for a CLI or an MCP tool. Rejected as *the* API because it
loses typed handles and makes bulk buffers awkward; the serde-derived layer in
decision 2 yields it for free where it is wanted.

**Async or callback-driven hosts.** Out of scope here; ADR-0005 keeps the
seam synchronous and this record inherits that.

**Push-style streaming decode.** The engine yielding rows as bytes arrive was
evaluated for the browser host and dropped: only 5.0 is row-addressable, the
complex (5.2/5.3), PNG, and CCSDS packings are sequential or all-or-nothing,
JPEG 2000 has no row access in `rust-j2k`, and a bitmap is not row-addressable
without a prefix count. The models the browser host targets are 5.2/5.3. The
*pull* form of the same need fits this record without change: reduced-
resolution decode (#463) and a windowed, sized warp (#465) both return less
than the whole field by value, and a `decode_rows(i, range)` for 5.0 would be
the same shape if it were ever wanted.

## Consequences

**What this makes cheap.** A CLI (#254) and an MCP surface become
`serde_json::to_string(session.op(args))`. PyO3 is `#[pyclass]` on `Session`
plus numpy for the three buffers. A C ABI is `#[repr(C)]` on types that
already qualify, plus cbindgen. wasm (#460) is the first host written as a
derived binding rather than the second written by hand.

**What this costs.** Small data crosses the seam by value and, where derived,
by serialisation. That is a copy the hand-written napi DTOs did not pay. It
is negligible per message; the one place it could show is a file with
thousands of messages listed eagerly, which is why `message(i)` is lazy and
why a host may keep one hand-written fast path if a benchmark, not a guess,
says it needs one. `#[non_exhaustive]` and stable discriminants also put a
compatibility obligation on the API that the napi DTOs never carried; that
obligation arrives with the first public npm or PyPI package anyway. A fifth
workspace crate is one more entry in the release bump, the inter-crate `=`
pins, and `codecov.yml`.

**What this asks of #464.** Its unit of work is the `fieldglass` crate, the
`Session` and `GridGeometry` types, and the rules above, not a line count.
Acceptance: no function outside a host crate takes a host DTO or returns a
host error type; every API type passes the rules test; `native.ts` is
generated; napi and wasm both pass the conformance suite.

**What this asks of #460.** `Field` and `Georef` are the first types designed
under decision 2: contiguous values whose type follows the source, a separate
`u8` mask, scalar georeferencing, strings only for `kind` and `proj4`. A
field decoded at reduced resolution (#463) is display-only and cannot be
passed to `probe`, `csv`, `contours`, or `stats`; the type system says so.

**What this asks of #464 before it moves anything.** A characterisation
snapshot of render, probe, contour, and CSV output on master, since the
existing oracles stop at decoded values. That snapshot seeds the conformance
suite in decision 3.

## When to revisit

- **A host needs the engine to call back into it mid-operation** — a progress
  callback, or push-style streaming for a packing that turns out to support
  it. Pull-shaped partial results (a window, a row range, a reduced level)
  already fit; a callback does not, and the fix is a second seam beside this
  one, not a wider version of it.
- **The serialisation copy shows up in a profile** on a real host, not a
  synthetic list of 10,000 messages. Then a typed fast path for that one
  operation, in that one host, is allowed and should be recorded here.
- **Two hosts need incompatible views of the same type.** If Python wants
  `xarray`-shaped dimension metadata that no JS host can use, the view layer
  has grown a per-host branch and the DTO rules should be revisited rather
  than bent.
- **The C ABI arrives.** `#[repr(C)]` and explicit ownership are stricter
  than serde; if the rules in decision 2 turn out not to be enough, extend
  them before writing the first C header, not after.
