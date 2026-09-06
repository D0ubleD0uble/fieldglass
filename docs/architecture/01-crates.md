# Architecture — Level 1: crates

Seven crates, one flow: a format crate parses its container and hands `core` the
same decoded field (`Vec<Option<f64>>` + grid geometry); `core` projects, warps,
and renders it; a host binds the result to its language. `fieldglass-core` owns
the shared traits and geometry and depends on nothing else in the workspace.

Both hosts now bind `fieldglass`, the host-neutral umbrella, but they are
shaped differently and that is the one asymmetry worth knowing about.
`fieldglass-wasm` touches nothing below it. `fieldglass-napi` still reaches the
format crates directly as well: #572 moved the display half — warp, probe,
contours, overlays and CSV — onto `fieldglass`, and what is left below is
decode, NetCDF, and the `MessageMeta` DTO the VS Code extension still reads.
Closing that gap is the rest of
[#464](https://github.com/D0ubleD0uble/fieldglass/issues/464), under
[ADR-0006](../decisions/0006-hosts-are-bindings-over-a-plain-data-api.md).

```mermaid
flowchart TD
    napi["fieldglass-napi<br/><i>N-API boundary (Node addon)</i>"]
    wasm["fieldglass-wasm<br/><i>wasm-bindgen façade (browser)</i>"]
    fieldglass["fieldglass<br/><i>Session, plain-data API types, shader, conformance suite</i>"]
    grib1["fieldglass-grib1<br/><i>GRIB1 decode</i>"]
    grib2["fieldglass-grib2<br/><i>GRIB2 decode</i>"]
    netcdf["fieldglass-netcdf<br/><i>NetCDF classic + NetCDF-4 / HDF5</i>"]
    core["fieldglass-core<br/><i>traits, GridGeometry, projection, warp, overlay, Palette</i>"]

    wasm --> fieldglass
    fieldglass --> grib1
    fieldglass --> grib2
    fieldglass --> core
    napi --> fieldglass
    napi --> grib1
    napi --> grib2
    napi --> netcdf
    napi --> core
    grib1 --> core
    grib2 --> core
    netcdf --> core
```

**Why it stays decoupled:** no format crate depends on another, and nothing
below a host depends on a host. A new decode path lands inside one format crate
and reuses `core`'s projection, warp, and overlay through the decoded field and
grid geometry, so it never ripples outward. Reprojection keys on grid type and
spacing alone, so a new field works the moment it decodes.

**Why a format crate re-exports the `core` types it names.** Depending on
`fieldglass-grib1` alone has to be enough to *use* it, and every fallible call
it makes returns `fieldglass_core::FieldglassError`. If the crate does not
re-export that name, a consumer has to add `fieldglass-core` to their own
manifest to write a `match` — and a manifest line written for one type is
written without `default-features = false`, which unifies `render` and `fs`
back on for everything in the graph, the browser included. So each format crate
re-exports exactly the `core` names that appear in its own public signatures:
the error type; `GridGeometry` for the two GRIB crates' `From` impls;
`ByteRange` / `ByteSource` for NetCDF's byte-access seam; and, in `grib2`, the
three parameter structs a §3 template hands back by value
(`LambertAzimuthalParams`, `TransverseMercatorParams`, `GeostationaryParams`).
The rule is *its own signatures*, not "whatever seems useful" — a re-export of
something the crate does not itself hand back is core's API surface leaking
through a second door, and `GridGeometry`'s own payload structs are the line:
they are core's API, reached by destructuring rather than by name.
`tests/crate-independence` is a package that depends on the three format crates
and deliberately not on `core`, so the rule is checked by `cargo test
--workspace` rather than remembered. That package catches a re-export that is
*removed*; `tools/check_format_crate_reexports.py` (pre-commit) catches one that
is never *added*, by reading each crate's own public signatures — `pub fn`
headers, public fields, enum payloads, trait items and `impl … for …` headers,
which is how `GridGeometry` enters both GRIB crates — and asking whether every
`fieldglass_core` name in them can be spelled from the format crate alone. Its
`ALLOWED_UNEXPORTED` list is where an exception has to be written down.

**A manifest is a claim about the crate too, and cargo checks nothing about
it.** A declared dependency nothing uses resolves, compiles, links and reports
success, so it passes `cargo clippy -D warnings`, `cargo test --workspace`,
`cargo deny check` and the six-target cross-compile alike — which is how
`serde` sat in all three format crates, and `thiserror` in `fieldglass-grib1`,
until [#538](https://github.com/D0ubleD0uble/fieldglass/issues/538). For a
published crate that is a statement about what it needs, made to the reader who
audits and vendors it, and it widens the advisory and licence surface
`cargo deny` holds the project to for nothing.
`tools/check_unused_dependencies.py` (pre-commit) asserts that every dependency
key of every package — including the three `fuzz/` crates and
`fieldglass-verify`, which are their own workspaces and which no `--workspace`
command ever sees — is spelled as an identifier in that package's own `.rs`
files, with comments and string literals stripped first, because this repo
names its dependencies in prose far more often than a grep can tell apart from
a use. Its `SKIPPED` map is where an exception is written down. It deliberately
does not check which *table* a dependency sits in: a normal dependency used
only from `tests/` is a real cost, but `tests/crate-independence` is that shape
on purpose, so the rule would need an exception on the only package it fires
on.

**The conformance suite is part of the API, not of any host.** ADR-0006
decision 3 puts the fixtures and expected outputs in `fieldglass`, as data, and
has each host run its own binding through them; `crates/fieldglass/conformance/suite.json`
is that data and `fieldglass::conformance` (feature `conformance`, on by
default) is the runner's machinery — the case list, the observation each
operation produces, and the comparator. Three runners replay it:
`crates/fieldglass/tests/conformance.rs` over `Session` (native, and
`wasm32-wasip1` in CI), `fieldglass-napi`'s `conformance_host` module over the
napi handles, and `crates/fieldglass-wasm/tests/node/conformance.mjs` over the
built browser bundle from Node. The third is the one that exercises a
*binding's* own work — serde-wasm-bindgen conversion, the typed arrays, the
`e.code` error mapping — and it is a CI step in the wasm job, not a manual
check.

Two rules that follow from the same record are checked beside it.
`crates/fieldglass/tests/api_rules.rs` holds every public type in the API
modules to ADR-0006 decision 2 (no generics or lifetimes, contiguous values
plus a `u8` mask, `#[non_exhaustive]`, the serde and schema derives, a stated
`rename_all`), reading the crate's own source through `include_str!` so a type
added without being classified fails rather than going unchecked, and rejecting
a deliberately non-conforming type beside it so the gate is known to be
connected. `tools/check_host_types.py` (pre-commit) is the other half of #464's
acceptance — no function outside a host crate takes a host DTO or returns a
host error type — which Rust cannot assert from inside the crate that would
have the dependency.

**Why `fieldglass` takes `core` with `default-features = false`.** It sits
between every host and `core`, so taking core's defaults there would re-enable
them for the browser by feature unification, and the wasm bundle would carry
`detect_format`'s `std::fs` — dead weight on a target with no filesystem. It
forwards the two optional surfaces it does want, `render` and `analysis`,
through features of its own (#552) rather than pinning them on the dependency,
so a host can decline one; `fs` is not forwarded at all, because ADR-0005 hands
every host bytes rather than a path.

**The umbrella's features, and what each is for.** `grib1` and `grib2` make the
decoders optional, since a decoder carries its own codecs — PNG, AEC and
JPEG 2000 arrive with GRIB2 alone — and a build that never opens that edition
should not link them: `grib1,render` resolves 12 crates against the default 34.
`Session` has one `Reader` variant per format feature and `lib.rs` refuses a
build with neither, so every dispatch stays exhaustive; `detect_from_bytes` is
never gated, because "this is GRIB1 and I cannot read it" is a different answer
from "I do not know what this is". `render` and `analysis` are independent of
each other — a values-first host wants contours without the painter — and
`render::contour_polylines`, which traces isolines and then projects them, is
the one member that needs both. `conformance` turns all four on, because the
suite is one recorded expectation per case over every format and both surfaces
and a partial build would have to skip cases to run at all. None of this is
visible to `--workspace`, where `fieldglass-napi`'s request unifies every
feature back on; the `cargo-clippy-umbrella-features` and
`cargo-doc-umbrella-features` hooks build the reduced sets instead.

`fs` is still not forwarded, and #552 deliberately did not add it even though
`planned/01-crates.md` lists it beside the other two: the umbrella does not
re-export `detect_format`, so an `fs` feature here would turn on a `core`
surface no consumer of *this* crate can reach. A CLI (#254) or PyO3 consumer
that starts from a path names `fieldglass-core` itself.

`fieldglass` does not depend
on `fieldglass-netcdf` yet: NetCDF reaches the browser with its own issue, and
an unused dependency here would be paid for in bundle size today. That is also
why the per-format features are two and not the four #552 named — `zarr` has no
crate (#246) and `netcdf` has no edge to make optional.

See [`planned/01-crates.md`](planned/01-crates.md) for where this is going —
`fieldglass-fetchplan` (#461), `fieldglass-zarr` (#246), and `napi` moving onto
`fieldglass` (#464).
