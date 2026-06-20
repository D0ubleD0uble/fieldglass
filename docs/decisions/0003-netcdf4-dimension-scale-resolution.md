# 0003 — NetCDF-4 dimension-scale resolution

**Status:** Accepted (2026-06-20). Scopes the dimension/coordinate half of
[#33](https://github.com/D0ubleD0uble/fieldglass/issues/33) and unblocks NetCDF-4
/ HDF5 rendering ([#169](https://github.com/D0ubleD0uble/fieldglass/issues/169)).

## Context

Decision [0002](0002-netcdf-slice-selection-and-rendering.md) renders a NetCDF
variable by picking a 2-D slice and synthesising a `"latlon"` `MessageMeta` from
the coordinate variables. That needs, per variable, its **ordered named
dimensions** and the **1-D lat/lon coordinate variables**. For the classic
backing those fall out of the header for free. For the NetCDF-4 / HDF5 backing
they do not, and that is the gap this note closes.

The raw HDF5 object model is already in place (#37–#40, all closed): the object-
header walker, root-group traversal, dataspace / datatype decoders, and the
attribute decoder. What is missing is the **NetCDF-4 semantic layer on top of
HDF5** — the dimension-scale convention that maps anonymous HDF5 datasets to
named, shared netCDF dimensions. Today `open_netcdf` returns `fully_parsed =
false` with empty dimensions / variables / attributes for an HDF5 file, and the
`DIMENSION_LIST` attribute that carries the mapping is **silently skipped**
(`hdf5/attribute.rs`: its datatype — variable-length of object references — is
one the decoder does not handle, so the attribute is dropped).

Without this, none of the four HDF5 corpus models in the 0.2.0 milestone
(MERRA-2, ERA5, CMIP6, GOES) can drive the slice picker. This is the bottleneck
for #169.

## The netCDF-4 dimension-scale convention

NetCDF-4 represents shared dimensions as **HDF5 dimension scales**
([NUG, NetCDF-4 File Format](https://docs.unidata.ucar.edu/nug/current/file_format_specifications.html);
[Unidata, "NetCDF-4 use of dimension scales"](https://www.unidata.ucar.edu/blogs/developer/en/entry/netcdf4_use_of_dimension_scales)).
The pieces we read:

- **`CLASS = "DIMENSION_SCALE"`** — a string attribute marking a dataset as a
  dimension scale. These correspond 1-to-1 with netCDF-4 dimensions.
- **`NAME`** — the dimension's name. For a dimension that also has coordinate
  values (a *coordinate variable*) this is the variable name. For a dimension
  with **no** coordinate variable, netCDF-4 writes a char dimension scale with
  empty contents and sets `NAME` to the exact string
  `"This is a netCDF dimension but not a netCDF variable."` (length carried by
  the dataspace, not the name). Such a dataset is a dimension, **not** a
  renderable variable.
- **`_Netcdf4Dimid`** — a scalar `int` giving the zero-based dimension id. It
  fixes dimension ordering when dimensions and coordinate variables are defined
  in different orders; we use it as the authoritative dimension index.
- **`DIMENSION_LIST`** — on every *variable* dataset, a **variable-length array
  of object references**, one element per axis in the variable's own dimension
  order. Each element references the dimension-scale dataset(s) attached to that
  axis (netCDF-4 attaches exactly one). Resolving each reference to a dataset,
  then to that dataset's `NAME`, yields the variable's **ordered dimension
  names** — the thing 0002's picker needs.
- **`REFERENCE_LIST`** — the inverse map (scale → {variable, axis}). Maintained
  by HDF5 but **not needed** here: `DIMENSION_LIST` already gives the forward
  mapping, so we read one direction, not both.
- **`_Netcdf4Coordinates`** — present on *multi-dimensional* coordinate
  variables (the curvilinear / 2-D-lat-lon case). Out of scope here; it belongs
  to the curvilinear follow-up ([#168](https://github.com/D0ubleD0uble/fieldglass/issues/168)).

## Decision

Resolve a single per-file **dimension table** at open time and use it to populate
the HDF5 branch of `DatasetMeta`, flipping `fully_parsed = true`.

Algorithm, over the root group's datasets (already enumerated by
`list_root_children`, which hands back each child's `object_header_address`):

1. **Find dimension scales.** A dataset is a dimension whenever it carries
   `CLASS = "DIMENSION_SCALE"`. Record `{ name (from NAME), length (from its
   dataspace), dimid (from _Netcdf4Dimid, else assign by discovery order),
   has_coordinate_values (false iff it is the pure-dimension char placeholder) }`,
   keyed by object-header address.
2. **Resolve each variable's dimensions.** For every dataset, decode its
   `DIMENSION_LIST` → for each axis take the referenced address → look up the
   dimension table → the dimension name. Result: an ordered `Vec<String>` of
   dimension names per variable, matching the dataset's rank.
3. **Classify each dataset** as: a *coordinate variable* (is a dimension scale
   **and** has real values — 1-D, named like its dimension), a *pure dimension*
   (the char placeholder — surfaced as a `DimensionMeta`, excluded from the
   variable/render list), or a plain *data variable*.

The coordinate variables then feed 0002's CF axis detection (`units` →
`standard_name`/`axis`) and geometry synthesis unchanged — this note makes the
HDF5 backing *look like* the classic backing to everything above the reader, so
#169 becomes wiring rather than new design.

### New HDF5 primitives required

`DIMENSION_LIST` cannot be decoded without three pieces the codebase lacks.
Each is small, spec-bounded, and reused beyond this feature:

- **Reference datatype (class 7).** An object reference is an `offset_size`-byte
  file address — the referenced object's header address, which matches
  `GroupChild::object_header_address` directly. Add to `datatype.rs`.
- **Variable-length datatype (class 9).** Decode the base type plus the
  vlen/sequence properties. Its on-disk element is a **global-heap ID**:
  `length (4) + global-heap collection address (offset_size) + object index (4)`.
  Add to `datatype.rs`.
- **Global-heap reader.** vlen data lives in a global-heap collection
  (signature `GCOL`); a global-heap ID indexes into it to retrieve the object
  bytes (here, the array of references). Add alongside the existing
  fractal/local-heap readers in `hdf5/heap.rs` (or a new `hdf5/global_heap.rs`).

A raw-bytes accessor is also needed: the current attribute path stringifies
values, but `DIMENSION_LIST` / `_Netcdf4Dimid` must be read as structured data.
Add a lower-level "attribute by name → (datatype, dataspace, raw data)" reader
rather than overloading the human-readable `Hdf5Attribute`.

### Module placement

- `hdf5/datatype.rs` — reference + variable-length classes.
- `hdf5/heap.rs` (or `hdf5/global_heap.rs`) — `GCOL` reader.
- `hdf5/dimensions.rs` (new) — the dimension table + per-variable resolution +
  dataset classification (the semantic layer; the only NetCDF-4-aware module).
- `hdf5/attribute.rs` — raw structured-attribute accessor.
- `reader.rs` — expose the dimension table / per-variable dimension names so the
  napi layer can build `DatasetMeta`; build it once and cache it on the reader.
- `napi/src/lib.rs` — fill the `NetcdfBacking::Hdf5` arm of `dataset_meta_from`;
  set `fully_parsed = true`.

## Scope and guardrails

- **Root group only**, matching the existing HDF5 value-decode scope. Variables
  in nested groups (some CMIP6 layouts) are a follow-up; flag, don't
  mis-resolve. The four 0.2.0 corpus models keep their renderable fields in the
  root group.
- **Follow the existing "subset we need, else a clear error" rule.** A
  global-heap collection that is indirect/filtered, a vlen that is not a simple
  sequence of references, or a `DIMENSION_LIST` whose datatype is unexpected →
  return an explicit unsupported error, never a silent misread (the same
  discipline `heap.rs` already documents).
- **Edge cases to handle explicitly:**
  - dimension with no coordinate variable (pure char placeholder) → a
    `DimensionMeta`, not a variable;
  - a coordinate variable is both a variable and a dimension scale — it appears
    in the variables list *and* defines a dimension;
  - `_Netcdf4Dimid` absent (older writers) → assign dimids by discovery order
    and proceed;
  - `DIMENSION_LIST` absent on a dataset → it shares no dimensions; fall back to
    anonymous per-axis dims sized from the dataspace, marked assumed;
  - more than one scale attached to an axis → take the first (netCDF-4 writes
    exactly one).

## Validation

- Cross-check against **netCDF4-python** (`ncdump -h`) as the oracle, per the
  repo's eccodes-style fixture discipline: for a tiny MERRA-2 / ERA5 / CMIP6 /
  GOES subset, assert the resolved dimension names + lengths, each variable's
  ordered dimension list, and the coordinate-variable classification match
  `ncdump`. Record provenance in `crates/fieldglass-netcdf/tests/fixtures/NOTICE.md`.
- Unit-test the new primitives in isolation: a reference datatype round-trip, a
  vlen-of-reference attribute resolved through a hand-built `GCOL`, and the
  pure-dimension `NAME` placeholder.
- After this lands, the README feature matrix's `Indicator / header section
  parsing` and `Tabular metadata viewer` NetCDF cells can drop the
  `✅ classic / 🚧 NetCDF-4` split (the #33 acceptance criterion) — done when the
  implementation lands, not in this note.

## Consequences

- #169 (HDF5 rendering) reduces to wiring: the reader presents HDF5 dimensions /
  coordinate variables the same way classic does, so 0002's axis detection and
  geometry synthesis apply without a branch.
- The new reference / vlen / global-heap primitives are general HDF5 building
  blocks, reusable for any future vlen attribute (e.g. string-array metadata),
  not one-offs for this feature.
- #33's tracker can close once this plus the metadata-viewer wiring land; the
  curvilinear `_Netcdf4Coordinates` path stays with #168.

## References

- NetCDF User's Guide, NetCDF-4 File Format / File Format Specifications:
  <https://docs.unidata.ucar.edu/nug/current/file_format_specifications.html>
- Unidata Developer's Blog, "NetCDF-4 use of dimension scales" and the
  "HDF5 Dimension Scales" series:
  <https://www.unidata.ucar.edu/blogs/developer/en/entry/netcdf4_use_of_dimension_scales>
- HDF5 File Format Specification v3 — "Datatype Message" (reference / variable-
  length classes) and "Global Heap".
- Builds on decision [0002](0002-netcdf-slice-selection-and-rendering.md); the
  curvilinear / 2-D-coordinate case is [#168](https://github.com/D0ubleD0uble/fieldglass/issues/168).
