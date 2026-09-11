# 0010 — A common array model, and containers as drivers over it

**Status:** Accepted (2026-09-07). Shapes #677–#687 in milestone 12; informs
#658 and #659 in milestone 11, and every host named in ADR-0006.

## Context

The question that forced this record was small. #660 put the Zarr chunk-grid
arithmetic (`ZarrArrayMeta`, `ChunkKeyEncoding`) in `fieldglass-fetchplan`
because kerchunk needed it first. The store walker (#658) needs the same
arithmetic to spell a chunk's key, and it belongs in `fieldglass-zarr`. Two
crates need one piece of code and neither is its home.

Looking for the home turned up a shape rather than a slot:

- `fieldglass-core` models grids and nothing above them. It has no type for a
  dimension, an attribute, a variable, a group or a chunk. The NetCDF crate
  carries a private neutral view (`DatasetView`, `DimView`, `VarView`) that is
  the closest thing to an array model in the tree, and the umbrella carries a
  second copy as DTOs (`DimensionInfo`, `VariableInfo`). A Zarr walker would
  have been the third.
- The same Zarr metadata document is parsed three times: once in
  `fieldglass-zarr` for codecs, dtype and fill value, once in
  `fieldglass-fetchplan` for shape, chunks and key spelling, with two
  `node_type` checks, two `chunk_grid.name == "regular"` checks and two error
  enums.
- `fieldglass-fetchplan` is 5,000 lines of which about half is wgrib2 and
  ECMWF sidecar grammar, NCEP level strings, run discovery and a GRIB §0 check.
  Its `Manifest` trait promises one object key per manifest and a parameter
  `Query`, which is why kerchunk could not implement it. Its one consumer is
  the umbrella, behind a feature both hosts turn off.
- `ByteSource` models exactly one object, has no identity (ADR-0005 decision 2
  asked for one), and is used by NetCDF classic alone; the HDF5 reader still
  reads a whole in-memory slice.
- The conformance suite has 280 cases, none of them NetCDF, and no operation
  for the Variables addressing mode two hosts now share (#662).

Three independent lineages solved the same problem with the same layering:
netCDF-Java's Common Data Model with one I/O service provider per format
(Panoply's own architecture, and the project's parity target); GDAL's one
raster model, one driver per format, and a virtual filesystem every driver
reads through; and the Zarr ecosystem, where the v3 array model is what
kerchunk, VirtualiZarr and Icechunk project every other container onto. The
layering is the idea taken from them. No code and no names are.

The maintainer's framing of the project has also moved since the crates were
laid out: from a VS Code extension to general-use meteorological decoders for
as many native platforms as possible, where someone can take *just* a GRIB2
reader or *just* an HDF5 reader and get a lightweight, professional-grade
library. Any shared layer has to respect that.

## Decision

### 1. A common array model lives in `core`, on the parsing surface

`fieldglass_core::array` holds what every container that stores named arrays
has in common: a regular chunk grid (shape, chunk shape, the chunk containing
a point, the chunks covering a region, the extent of a ragged edge chunk), the
encoding that spells a chunk's key, named dimensions, typed attributes with a
neutral value, an array description, and a group tree that resolves to
path-qualified names. The CF unpacking rule (`scale_factor`, `add_offset`,
`_FillValue`, `missing_value`, `valid_range`) is stated once against those
attributes. (#677, #678)

It is a module, not a crate, for the same reason `GridGeometry` is in `core`:
the format crates take `core` with `default-features = false` and convert into
it, and a second always-on crate between them and `core` would buy nothing
today. It is ungated because it is pure arithmetic and plain structs, and it
adds **no dependency**. If a consumer ever wants the array model without the
geometry, the module lifts into its own crate; the module boundary is drawn so
that lift is mechanical.

### 2. The neutral model is Zarr v3's array model

The chunk key encoding moves with the chunk grid and not with the codecs. In
the Zarr v3 specification `chunk_grid` and `chunk_key_encoding` are extension
points of the *array*; the codec chain is a separate one. Key spelling is
array-model vocabulary, and it is what any keyed store of any container uses:
a kerchunk document over a GRIB archive spells its keys this way.

The same model describes NetCDF-4's chunked layout (a regular chunk grid with
an index) and a classic variable (one chunk per record), which is why one set
of types can carry all three readers. Adopting the v3 model is adopting the
lingua franca the cloud tools already converged on, not one format's quirk.

Everything is written from the specifications — the Zarr v3 core and v2
storage specs, the kerchunk reference-document format, the HDF5 and NetCDF
file format documents — and named in Fieldglass's own terms. Not from any
library's source, and depending on none of them.

### 3. Container crates are drivers over the model

Each format crate reads its container into three things: the array model
(for containers that hold named arrays), `GridGeometry` (for a field that can
be placed), and values. `fieldglass-zarr` owns the store walker (#658), the
one parser of an array's metadata document, and the codecs, with the codecs
behind a default-on `codecs` feature so a consumer that wants only the
metadata parser links no decompressor (#686). `fieldglass-netcdf`'s view is
rebuilt on the model (#684). GRIB is a stream of self-describing messages and
does not produce the model; it keeps `Addressing::Messages`.

The `planned/01-crates.md` sentence that directory walking is the napi host's
concern is superseded: walking a store is reading a container, and the host's
job is to hand over the objects (decision 5).

**Amended by #658 (2026-09-11): what a driver presents.** The decision above
says what a driver reads *into*. It left open what it hands *up*, and a
reader's own methods would have made every consumer of the model — CF
placement, a host's variable list — learn each container separately. A driver
of a container with named arrays presents `fieldglass_core::array::ArraySource`:
its `Group` tree and a raw region read, with CF applied once above it from the
array's attributes (`read_region_physical`). The model types stay plain
structs; this is a trait because it is IO, one rung above `ByteSource` and
`ObjectSource`, and every IO seam here is one.

It also settles where Zarr sits. A Zarr store is not a third file format beside
GRIB and NetCDF: it is a layout of chunks under keys plus the codecs they were
written through, so `ZarrStore` reads through `ObjectSource` and implements
`ArraySource`, as the NetCDF readers will over `ByteSource`. Two consequences
follow from putting it there rather than beside them. A kerchunk reference
document plus the ranges a host fetched is itself an `ObjectSource`, so a NetCDF
or GRIB archive described by one reads through `ZarrStore` unchanged; and
`Session` needs one arm for every container of arrays, not one per container.

### 4. `fieldglass-fetchplan` is manifests in, chunk plan out

The crate keeps every parser it has. What changes is the shape of its answer:
a `PlanItem` says which message or which chunk index it is; `Manifest` keeps
`items()` and the provided `messages()` and loses `key()`; the GRIB-only
`select(query, resolver)` moves to a `MessageManifest` extension trait; and
`KerchunkRefs` implements `Manifest` (#685). Kerchunk stays in the crate: a
reference document is a manifest, and manifests are what the crate is for.

For the metadata parser it takes `fieldglass-zarr` with default features off.
The crate's "no format crate" rule was a rule about weight — a planner must
not link a decoder's codecs — and the codecs are now behind a feature, so the
weight is not linked and the dev-dependency cycle around the seam test
disappears. The umbrella re-exports the kerchunk and chunk-grid surface it
currently omits (#687).

### 5. The storage seam gains keys

`ObjectSource` sits beside `ByteSource` in `fieldglass_core::bytes`: get an
object by key, list keys under a prefix, advisory prefetch, synchronous reads,
with an in-memory implementation over a map (#680). ADR-0005 stands: the host
fetches, and a remote store is the host filling an `ObjectSource`, not a Rust
transport. The napi host may implement it over `std::fs`, because napi is the
host. Two ADR-0005 leftovers ride alongside: `ByteSource` gains the identity
decision 2 required (#681), and the HDF5 reader migrates onto it as classic
did (#682).

### 6. Every format crate is a first-class standalone deliverable

A crate that depends on `fieldglass-grib2` alone must get a lightweight,
professional-grade GRIB2 reader and nothing else: no JSON parser, no
decompressor, no gated surface it did not ask for. The shared layers in
decisions 1 and 5 are held to that: they add no dependency to `core`, and
`cargo tree -p fieldglass-grib2` is the check named in each issue.

The stability promise does not change. The four crates on crates.io keep it,
and the umbrella joins them when it is published. A proposal to make
`fieldglass` the only semver surface and publish the others as internal was
considered and declined by the maintainer: the standalone readers are the
product, not a build detail of the umbrella.

### 7. What was rejected, and why

- **`fieldglass-zarr` depends on `fieldglass-fetchplan`.** The literal reading
  of the crates' own docs. It links 2,500 lines of GRIB sidecar grammar into
  anyone decoding a Zarr chunk and creates a cycle with the seam test.
- **Duplicate the arithmetic in both crates.** Forbidden by the project's
  conventions, and the split that made #602 hoist #542's transpose.
- **Move all Zarr knowledge into `fieldglass-zarr`, kerchunk included.** The
  most literal "Zarr crate", but kerchunk is a manifest and its consumers are
  fetch planners. It would have made the browser link codecs to plan a fetch.
- **A new `fieldglass-array` crate now.** Deferred, not rejected. A module in
  `core` has the same dependency footprint today and no consumer has asked for
  the model without the geometry.
- **Parse Zarr metadata in `core`.** It would put `serde_json` into every
  GRIB-only build, against decision 6. The parser lives in the Zarr crate and
  fetchplan takes it feature-reduced.

## Consequences

**What this makes cheap.** The #658 walker is a traversal over two metadata
spellings on top of types that already exist, with a seam to read through. A
Zarr store reaches `Session` the way NetCDF does, as `Addressing::Variables`,
and the PyO3 and CLI hosts (#254) see one model for every container that has
named arrays. The three copies of the dimension and variable types become one.
A kerchunk document over a GRIB archive is planned and decoded with no reader
change.

**What it costs.** `core` carries Zarr-specification vocabulary, with the
justification in decision 2 written where a reader of the crate docs will see
it. The NetCDF view is refactored under a byte-identical oracle. The
architecture drift guard hard-codes six crate suffixes and has to derive its
list from the workspace before the new edges can be drawn (#683).

**Order of work.** #677 first, since #658, #685 and #686 sit on it. #678 →
#684 → #679. #680, #681 and #682 are independent of everything above. #683 is
the documentation of all of it and lands first.

## When to revisit

- **A container whose chunk grid is not regular.** Zarr v3 allows extension
  chunk grids; a rectilinear one would add a variant to the model, and a fully
  irregular one would be a different model. Neither exists in the data
  Fieldglass targets today.
- **A consumer wants the array model without the geometry.** Lift the module
  into its own crate. The boundary is drawn for it.
- **A format crate's dependency footprint grows anyway.** Decision 6 is a
  rule, and rules that are only in prose drift. If `cargo tree` on a format
  crate ever shows a crate the reader did not need, the check belongs in a
  pre-commit tool beside `check_unused_dependencies.py`, not in review.
- **The stability promise is revisited at 1.0** (conventions, *Versioning*).
  Decision 6 records the position taken now; per-crate versioning then is
  compatible with it.
