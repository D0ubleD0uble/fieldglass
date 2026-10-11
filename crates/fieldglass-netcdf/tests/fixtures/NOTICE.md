# Test fixture provenance

## `netcdf_classic_dummy.nc`, `netcdf4_hdf5_dummy.nc`

Sourced from the Unidata `netcdf4-python` test corpus
(<https://github.com/Unidata/netcdf4-python/tree/master/test>) — `netcdf_dummy_file.nc`
and `issue1152.nc`, respectively. Used to exercise minimal CDF-1 classic and
NetCDF-4 / HDF5 backings.

## `ersst_v5_187001_cdf1.nc`

NOAA Extended Reconstructed Sea Surface Temperature (ERSST) v5, January 1870
monthly mean — a real published climate-science product. Sourced verbatim from
NOAA NCEI:

- URL: <https://www.ncei.noaa.gov/pub/data/cmb/ersst/v5/netcdf/ersst.v5.187001.nc>
- DOI: <https://doi.org/10.7289/V5T72FNM>
- Reference: Huang, B., et al. (2017), *Extended Reconstructed Sea Surface
  Temperature, Version 5 (ERSSTv5)*, J. Climate, 30, 8179–8205.
- License (per file metadata): "No constraints on data access or use."

The file is the un-modified upstream byte stream (`CDF\x01` magic, classic
CDF-1, 4 dimensions, 6 variables including `sst` and `ssta` at 2°×2°
resolution, 38 CF-1.6 / ACDD-1.3 global attributes).

## `ersst_v5_187001_cdf2.nc`, `ersst_v5_187001_cdf5.nc`

Re-encoded copies of `ersst_v5_187001_cdf1.nc` produced by the canonical
Unidata `netCDF4` Python library (which wraps `libnetcdf`'s `nccopy -k`):

```text
NETCDF3_64BIT_OFFSET   →  ersst_v5_187001_cdf2.nc   (CDF-2: 64-bit var begins)
NETCDF3_64BIT_DATA     →  ersst_v5_187001_cdf5.nc   (CDF-5: 64-bit nelems / dim lengths / vsize)
```

The values, dimensions, attributes, and variable structure are identical to
the upstream NOAA file — only the on-disk encoding differs. This lets the
header parser exercise the rare CDF-2 / CDF-5 width paths against real
model-derived content rather than hand-crafted bytes. Reproduced via the
`build_fixtures.py` script in this directory.

> CDF-5's *extended numeric types* (`UByte`, `UShort`, `UInt`, `Int64`,
> `UInt64`) are not exercised by these fixtures because the source CDF-1 file
> contains none — those types are covered by the unit tests in
> `crates/fieldglass-netcdf/src/classic.rs`.

## Value-decode oracles (`*.values.json`)

`netcdf_classic_dummy.nc.values.json` and `ersst_v5_187001_cdf1.nc.values.json`
are the value-decode targets for classic NetCDF value decode (#108). Each
records, per variable, what the canonical Unidata `netCDF4` library (which
wraps `libnetcdf`) decodes from the on-disk bytes: `nc_type`, shape,
dimensions, fill value, present/missing counts, value statistics, and a few
anchored samples in C (row-major / on-disk) order. Samples are the *raw*
on-disk values (fills included) so a decoder can match the exact sequence,
including masked positions. Once #108 reads each variable from its `begin`
offset, the decoded array must reproduce these numbers.

The two fixtures together cover the decode matrix: every `nc_type`
(`char` / `int` / `float` / `double`), every layout (scalar, fixed 1-D,
multi-dimensional, and unlimited-dimension *record* variables — empty here
since `numrecs = 0`), default fills (`crs` = `NC_FILL_INT`), explicit
`_FillValue`s (`z` = -9999.9), and real masked climate data (ERSST `sst`:
5032 of 16020 points are the -999 fill). The ERSST CDF-2 / CDF-5 fixtures
decode to byte-identical values, so the single CDF-1 oracle covers all three.

Regenerate with `python3 tools/regenerate-netcdf-oracles.py` from the repo
root (needs `netCDF4`); the committed JSON means the Rust suite needs no
netCDF4 at runtime. `tests/classic_value_targets.rs` pins the type/shape
matrix the decode builds on; the value numbers are checked once #108 lands.

## HDF5 deep-parse fixtures (`hdf5_v1_symboltable.h5`, `hdf5_v2_linkinfo.h5`)

Synthetic HDF5 files built with `h5py` (wraps libhdf5) as targets for the
NetCDF-4 / HDF5 deep-parse chain — object-header walker (#37), group/link
traversal (#38), dataspace + datatype decoders (#39), attribute decoder (#40),
and dataset value decode (#121), under the #33 umbrella. Built and
oracle-dumped by `tools/build_hdf5_fixtures.py` (run from the repo root; needs
`h5py`). `track_times=False` keeps object headers timestamp-free for
reproducibility.

The two files deliberately exercise the **two on-disk group layouts** #38 must
handle:

- `hdf5_v1_symboltable.h5` (`libver='earliest'`): superblock v0, **v1** object
  headers, **symbol-table** groups (local heap + B-tree v1 → `SNOD` nodes), no
  `OHDR` signature. The legacy layout. Also carries a chunked + gzip + shuffle
  dataset (`compressed`) whose chunk index is a **version-1 B-tree** (Data Layout
  v3) — the storage path #121 value decode reads end to end (B-tree chunk walk +
  filter-pipeline reverse). The v2 fixture's `chunked` dataset uses the newer
  version-4 chunk index instead, so the two cover both index styles.
- `hdf5_v2_linkinfo.h5` (`libver='v110'`): superblock v3, **v2** object headers
  (`OHDR`), **link-info** groups, a chunked + gzip + shuffle dataset (#121
  filter pipeline), and a 12-attribute dataset that forces **dense** attribute
  storage (fractal heap `FRHP` + B-tree v2, #40).

Both carry the same matrix: the datatype set (#39: signed int little- and
big-endian, `float32`, `float64`, fixed-length string), the dataspace set
(scalar, simple 1-D / 2-D, and an unlimited `H5S_UNLIMITED` max dim — stored
chunked, as HDF5 requires), global + per-dataset attributes (#40, numeric and
string), contiguous storage, and an unwritten dataset with an explicit fill
value (#121).

Each fixture has a sibling `*.h5.oracle.json` (the decode/parse target): the
superblock version, object-header style, raw layout markers (`OHDR` / `SNOD` /
`FRHP`), global attributes, the root-group child list (== `h5dump -n`), and per
dataset the datatype, dataspace (dims + max dims), storage layout + filters,
fill value, attributes, and value statistics + samples. These are what the
chain must reproduce; deep parsing isn't implemented yet, so they're staged
references, with `tests/hdf5_deep_parse_targets.rs` pinning the layout facts
verifiable today (superblock + `OHDR`/`SNOD`/`FRHP` markers).

> The bundled real NetCDF-4 file `netcdf4_hdf5_dummy.nc` remains the "library
> wrote it" example; these two add controlled coverage of both group layouts
> and the datatype / storage / attribute matrix.

## Multi-level B-tree v2 fixture (`hdf5_btreev2_multilevel.h5`)

A synthetic `h5py` (libhdf5) file whose `many_attrs` dataset carries 700 dense
attributes — enough that the attribute name-index **version-2 B-tree grows to
depth 2** (an internal-node tree, not a single leaf). It targets the multi-level
B-tree walk: the doubling-table heap support added for the GOES-16 file (#187)
handles a file's *storage*, but a metadata-heavy file's *index* spills into
internal B-tree nodes first, which the reader previously refused. A real
operational file (e.g. ERA5 / MERRA-2 / CMIP6, #123) hits this before it needs
child indirect heap blocks. Built and oracle-dumped by
`tools/build_hdf5_fixtures.py` (`track_times=False` for reproducibility).

Each attribute is `a{i:04d} -> int32 i`, so the sibling `*.h5.oracle.json` records
the rule, the attribute count, the measured B-tree depth, and a few sampled
values rather than dumping 700 entries; `tests/hdf5_attributes.rs` reads every
attribute back and checks it against the generated expectation. Part of #33.

## Child-indirect fractal-heap fixture (`hdf5_child_indirect.h5`)

A synthetic `h5py` (libhdf5) file whose `many_attrs` dataset carries 512 dense
attributes of `int32[256]` (≈1 KiB each). That much dense storage fills every
direct-block row of the attribute fractal heap's doubling table and spills into a
**child indirect block** — the rows beyond `max_direct_block_size`, which the
reader previously refused with a clean `child indirect fractal-heap blocks not
supported` error. It is the real-libhdf5 backstop for the step after the
multi-level B-tree fixture: the metadata-heaviest corpus files (#123) reach this
once their *storage*, not just their *index*, outgrows one indirect block's
direct rows.

libhdf5 fills the full grid of direct blocks before it allocates a child indirect
block (the exact heap geometry — starting / max-direct block size, table width —
is libhdf5-version dependent and recorded in the oracle), so this fixture is
necessarily larger (~575 KiB) than the others; the hand-built
`crates/fieldglass-netcdf/src/hdf5/heap.rs` unit tests pin the exact byte layout
at no cost. The sibling `*.h5.oracle.json` records the attribute rule / count,
sampled names, and the parsed fractal-heap geometry (`cur_rows`,
`max_dblock_rows`, and the indirect / direct block counts that confirm a child
indirect block is populated). Built by `tools/build_hdf5_fixtures.py`
(`track_times=False` for reproducibility); `tests/hdf5_attributes.rs` reads every
attribute back. Part of #33.

## Version-4 chunk-index fixture (`hdf5_v4_chunk_index.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` so libhdf5
selects the **version-4 data-layout message** and its newer chunk indexes, the
target for #216. It carries three chunked float datasets whose values are
`arange` (so the oracle is a rule):

- `single_chunk` — a 4×4 field in one 4×4 chunk, stored under the **Single
  Chunk** index (type 1), inline in the layout message with no external index.
- `fixed_array` — a fixed-shape 8×8 field in 4×4 chunks, stored under an
  **unfiltered Fixed Array** index (type 3, `FAHD` header + `FADB` data block).
- `single_chunk_filtered` — a gzip/shuffle single chunk. libhdf5 2.0 writes this
  with a data-layout message **version 5** (the unfiltered cases stay version 4),
  which post-dates the v3-spec format the reader decodes; it is a deliberate
  boundary case the reader must reject cleanly, tracked as a #216 follow-up.

The *filtered* Fixed Array is already covered by `hdf5_v2_linkinfo.h5`'s
`chunked` dataset. Built by `tools/build_hdf5_fixtures.py` (`track_times=False`
for reproducibility); `tests/hdf5_value_decode.rs` checks the decoded values
against `h5py`. Part of #216.

## Extensible-array chunk-index fixture (`hdf5_ea_chunk_index.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` whose two
datasets have one unlimited dimension, so libhdf5 indexes their chunks with a
version-4 **Extensible Array** (target for #216). Values are `arange`:

- `ea_direct` (150 chunks) spans super blocks 0–3, whose data-block addresses
  libhdf5 stores directly in the extensible-array index block (no secondary
  block); the data blocks grow in size (16, 32, 32, 64, … elements) per the
  super-block doubling rule.
- `ea_secondary` (280 chunks) is large enough that libhdf5 allocates a
  **secondary block** for super block 4, exercising the reader's walk from the
  index block's secondary-block pointer to the data-block addresses beyond the
  direct slots.

The `hdf5_v2_linkinfo.h5` `record` dataset already covers the simplest tier: a
single chunk addressed directly in the index block with no data blocks. Built by
`tools/build_hdf5_fixtures.py` (`track_times=False` for reproducibility);
`tests/hdf5_value_decode.rs` checks the decoded values against `h5py`. Part of
#216.

## Filtered extensible-array chunk-index fixture (`hdf5_ea_filtered.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` whose datasets
have one unlimited dimension **and** a filter (gzip + shuffle), so libhdf5
indexes their chunks with a version-4 **Extensible Array** whose elements are the
*filtered* form (client id 1: chunk address + on-disk size + filter mask, not the
address-only elements the unfiltered array stores). libhdf5 2.0 writes these
under a data-layout message **version 5**, which for the chunked class is encoded
byte-for-byte like version 4. Values are `arange`:

- `ea_filtered_iblock` (4 chunks) keeps every element directly in the
  extensible-array index block (no data blocks), covering the filtered
  index-block element read in isolation.
- `ea_filtered_direct` (150 chunks) spans super blocks 0–3 whose data blocks are
  addressed directly from the index block, covering filtered elements inside data
  blocks across the doubling walk.
- `ea_filtered_secondary` (280 chunks) is large enough that libhdf5 allocates a
  **secondary block** for super block 4, reached via the index block's
  secondary-block pointer.

The builder asserts every `EAHD` header in the file has client id 1, positively
confirming libhdf5 filtered the chunks (a client id 0 would mean the filter was
dropped and the fixture no longer exercises the filtered path). Built by
`tools/build_hdf5_fixtures.py` (`track_times=False` for reproducibility);
`tests/hdf5_value_decode.rs` checks the decoded values against `h5py`. Part of
#216.

## Implicit chunk-index fixture (`hdf5_implicit_index.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` whose datasets
are fixed-shape, unfiltered, and **early-allocated**, so libhdf5 indexes their
chunks with the version-4 **Implicit** index (target for #216): every chunk of
the chunk grid is stored contiguously from one base address with no on-disk
index structure. The high-level `h5py` API always defers allocation (which
yields a Fixed Array), so the datasets are created through the low-level API
with `set_alloc_time(ALLOC_TIME_EARLY)`. Values are `arange`:

- `implicit` (8×8 in 4×4 chunks) is a square 2×2 chunk grid of four whole
  chunks.
- `implicit_partial` (5×7 in 4×4 chunks) is a 2×2 grid whose right and bottom
  chunks hang past the dataset bounds, exercising edge-chunk clipping on
  scatter.

The builder asserts the file contains no `FAHD`/`EAHD`/… markers, positively
confirming libhdf5 chose the implicit index rather than a Fixed or Extensible
Array. Built by `tools/build_hdf5_fixtures.py` (`track_times=False` for
reproducibility); `tests/hdf5_value_decode.rs` checks the decoded values against
`h5py`. Part of #216.

## Version-2 B-tree chunk-index fixture (`hdf5_v2_btree_index.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` whose datasets
have **more than one unlimited dimension**, the case libhdf5 indexes with a
version-4 **version-2 B-tree** (chunk index type 5) rather than a fixed or
extensible array — both of which assume at most one growth dimension (target
for #216). Each B-tree record carries the chunk's scaled (chunk-grid) coordinate
directly, which the reader multiplies by the chunk edge to place the chunk.
Values are `arange`:

- `bt2` (4×4 in 2×2 chunks) is unfiltered, so its records are the type-10 form
  (address + scaled offsets).
- `bt2_multi` (8×8 in 2×2 chunks → 16 chunks) keeps several records per B-tree
  leaf.
- `bt2_filtered` (4×4 in 2×2 gzip+shuffle chunks) uses the filtered type-11
  form (address + on-disk size + filter mask + scaled offsets), whose size-field
  width the reader derives from the record width the B-tree advertises.

The builder asserts every chunk-index B-tree header (`BTHD`) is type 10 or 11
and that no `FAHD`/`EAHD` marker appears, positively confirming libhdf5 chose
the v2 B-tree rather than a Fixed or Extensible Array. Built by
`tools/build_hdf5_fixtures.py` (`track_times=False` for reproducibility);
`tests/hdf5_value_decode.rs` checks the decoded values against `h5py`. Part of
#216.

## Chunk-record fixtures (`hdf5_oversized_chunk.h5`, `hdf5_duplicate_chunk_records.h5`, `hdf5_shared_chunk_records*.h5`, `hdf5_conflicting_chunk_records*.h5`, `hdf5_off_grid_chunk_record.h5`, `hdf5_outside_chunk_record.h5`)

Nine small files for #837, each holding a `uint8` dataset `v` under a
version-1 chunk B-tree (`libver='earliest'`). Built by
`tools/build_hdf5_duplicate_chunk_fixture.py` with h5py 3.16.0 (libhdf5 2.0.0);
the build is reproducible byte for byte. `tests/hdf5_chunk_records.rs` decodes
all nine.

- `hdf5_oversized_chunk.h5` (19,814 bytes) is plain h5py output: shape `(1,)`
  holding `7`, `maxshape=(None,)`, one gzip chunk of 16 Mi elements. A chunk
  that large is legal on an extendable dataset. The oracle is h5py's read,
  `[7]`.
- `hdf5_duplicate_chunk_records.h5` (24,008 bytes) is that file with a chunk
  index appended by hand: a level-0 leaf of four entries, each naming the one
  stored chunk at origin 0, under a level-1 root whose four children all point
  at that leaf, with the layout message repointed at the root. That is 16
  records for one chunk. The nodes are padded to the 2,096 bytes libhdf5 reads
  a node at (room for 2K entries, K = 32). libhdf5 reads `[7]` and counts 16
  chunks. Fieldglass reads `[7]`, reading the chunk once. It also seeds the
  NetCDF fuzz corpus.
- `hdf5_shared_chunk_records.h5` (22,952 bytes) is shape `(16, 1)` in gzip
  chunks of `(1, 16 Mi)` with only `[0, 0] = 7` written, so one chunk is
  stored, and a hand-built leaf naming that chunk at all sixteen origins
  `(i, 0)`. The format does not forbid two origins sharing storage. libhdf5
  reads sixteen 7s, and so does Fieldglass, inflating the chunk once. It also
  seeds the NetCDF fuzz corpus.
- `hdf5_shared_chunk_records_masks.h5` and `hdf5_shared_chunk_records_sizes.h5`
  (22,952 bytes each) are that file with record `i`'s filter mask raised by
  `2i` (bits above the one gzip filter) or its stored size by `i` bytes (past
  the end of the zlib stream). libhdf5 reads sixteen 7s from both. Fieldglass
  reads the first with the chunk inflated once, and refuses the second.
  **Known divergence from libhdf5** (#888): one stored chunk stated at sixteen
  sizes would decode differently under another pipeline (fletcher32 reads its
  checksum from the end), and reading each is the per-record cost again.
  ADR-0013's amendment records it. The masks file seeds the NetCDF fuzz corpus.
- `hdf5_conflicting_chunk_records.h5` (5,600 bytes) holds `[7, 9]` in
  unfiltered chunks of one element, with a hand-built leaf naming chunk A at
  origin 0, chunk B at origin 0, and chunk B at origin 1. **Known divergence
  from libhdf5.** libhdf5 2.0.0 reads `[9, 9]`, the last record at origin 0,
  and `[7, 9]` when the first two records are swapped: its answer depends on
  record order (and, across leaves, on its tree search), not on anything the
  file states. The HDF5 File Format Specification ("Version 1 B-trees") says
  "the range of values represented by child[i] is indicated by key[i] and
  key[i+1]" and, for chunk trees, that "the chunk described by key[i] is the
  least chunk in child[i]". Two equal keys name an empty range, so the index
  is malformed, and Fieldglass refuses it. The oracle records libhdf5's
  read, and ADR-0013 records the decision.
- `hdf5_conflicting_chunk_records_swapped.h5` (5,600 bytes) is the same with
  the first two records swapped. libhdf5 reads `[7, 9]`, which is the evidence
  that its answer depends on record order alone. Fieldglass refuses it, as it
  refuses the committed order.
- `hdf5_off_grid_chunk_record.h5` (5,600 bytes) holds `[1, 2, 3, 4]` in
  unfiltered chunks of two, with records at origins 0 and 1. Origin 1 is
  inside the shape but not a multiple of the chunk edge. libhdf5 refuses the
  read ("bad coordinate offset", recorded in the oracle), and so does
  Fieldglass.
- `hdf5_outside_chunk_record.h5` (5,600 bytes) holds `[7, 9]` with a third
  record at origin 5, wholly outside the shape, addressed past the end of the
  file. libhdf5 reads `[7, 9]` without looking it up, and Fieldglass skips it
  unread.

## Huge heap object fixtures (`netcdf4_huge_attributes.nc`, `hdf5_huge_link_name.h5`, `hdf5_shared_huge_attribute.h5`, `hdf5_huge_only_heaps.h5`)

Four files for #899 and #907, built by `tools/build_netcdf4_huge_attribute_fixture.py`
(reproducible byte for byte). In each, dense storage (a fractal heap indexed
by a version-2 B-tree) holds a message larger than the heap's 4 KB maximum
managed size, which libhdf5 stores as a *huge* object found through the heap's
huge-object B-tree (type 1, unfiltered).

- `netcdf4_huge_attributes.nc` (23,423 bytes) is written by netCDF-C 4.9.3
  through netCDF4-python 1.7.4 (`format="NETCDF4"`, HDF5 1.14.6): ten short
  global attributes and a 5,600-byte `history`, and one variable `t(x)` =
  `[1, 2, 3, 4]` with ten short attributes and a 5,000-byte `comment`.
- `hdf5_huge_link_name.h5` (8,642 bytes) is written by h5py 3.16.0 (libhdf5
  2.0.0, `libver='latest'`): a group `g` with nine short subgroups and one
  whose name is 5,000 characters.

- `hdf5_shared_huge_attribute.h5` (8,133 bytes) is written by h5py with
  `libver='latest'`: a dataset `v` with nine small attributes and a
  5,000-byte one, a huge object. The builder then repoints one small
  attribute's name-index record at the huge object's heap ID, leaving the
  B-tree checksum stale (libhdf5 refuses the node on it; Fieldglass does not
  verify B-tree checksums). Two records then name one object, the shape that
  made a 1.9 MB file use 34 GB in review; Fieldglass reads the object once and
  refuses the second name.

- `hdf5_huge_only_heaps.h5` (209,787 bytes) is written by h5py with
  `libver='latest'` and holds fractal heaps of only huge objects, which
  libhdf5 gives no root block (#907): dataset `one` has a single 70,000-byte
  attribute (too big for its object header, so its attributes go dense at
  once), dataset `nine` has nine 5,000-byte attributes, and group `g` has
  nine links each named with 5,000 characters. Its oracle records each
  string as its first character and length.

All but the shared-attribute file have an `.oracle.json` holding what
netCDF4-python or h5py read back. `tests/hdf5_huge_objects.rs` checks the
attributes, `t`'s values and the links against them, and that the
shared-attribute file reads its huge object once;
`crates/fieldglass/tests/huge_heap_objects.rs` opens the NetCDF file and the
huge-only file through `Session`.

## Shared symbol-table fixtures (`hdf5_shared_group_name.h5`, `hdf5_shared_snod.h5`)

Two small files for #901, built by `tools/build_hdf5_shared_snod_fixture.py`
(reproducible byte for byte) from one h5py 3.16.0 file (libhdf5 2.0.0,
`libver='earliest'`): a root group of eight datasets, one named with 2,000
`L` characters, all in one symbol-table node (`SNOD`) of a version-1 group
B-tree.

- `hdf5_shared_group_name.h5` (7,448 bytes) points a second `SNOD` entry's
  name offset at the long name, so two members share one name in the local
  heap. h5py lists both. Fieldglass refuses the group, reading the name once.
- `hdf5_shared_snod.h5` (7,512 bytes) appends a group B-tree leaf whose two
  entries both name the one `SNOD`, and repoints the group at it. libhdf5
  refuses it (it reads the appended node at its full allocated size).
  Fieldglass refuses the second reference.

A valid group gives each node and name its own storage; the issue's version
of the second shape, 8,192 references to one `SNOD`, made a 197 KB file use
3.85 GB. `tests/hdf5_shared_snod.rs` checks both refusals through a
recording source.

## Local heap segment fixtures (`hdf5_local_heap_short.h5`, `hdf5_local_heap_unterminated.h5`)

Two files for #908, built by `tools/build_hdf5_local_heap_fixture.py`
(reproducible byte for byte) from one h5py 3.16.0 file (libhdf5 2.0.0,
default earliest format) with datasets `alpha` and `beta`. Its root local
heap's 88-byte data segment holds `alpha` at offset 8 and `beta` at 16. The
builder patches the heap header's data segment size (and sets the free-list
head to 1, `H5HL_FREE_NULL`):

- `hdf5_local_heap_short.h5` (2,072 bytes): size 16, so `beta`'s offset is at
  the segment's end. libhdf5 refuses it ("unable to offset into local heap
  data block"); so does Fieldglass.
- `hdf5_local_heap_unterminated.h5` (2,072 bytes): size 24, with `beta`'s
  terminator and padding overwritten (`betaxxxx`) and a terminator written at
  offset 24, the first byte past the segment. **Known divergence from
  libhdf5.** The HDF5 File Format Specification ("Local Heap") defines a name
  as a string in the heap's data segment, so one that runs past the
  segment's end is malformed. libhdf5 2.0.0 lists it anyway as `betaxxxx`;
  Fieldglass refuses it.

Each `.oracle.json` records libhdf5's outcome. `tests/hdf5_local_heap.rs`
checks both refusals and that no read starts at or past the segment's end.
ADR-0014 records the decision.

## Soft link fixtures (`hdf5_soft_links_earliest.h5`, `hdf5_soft_links_latest.h5`)

Two files for #914, built by `tools/build_hdf5_soft_link_fixture.py` with h5py
3.16.0 (libhdf5 2.0.0), reproducible byte for byte. Each root holds the
`float32` datasets `a` (`[0, 1, 2]`), `m` (`[10 … 13]`) and `z`
(`[20 … 24]`), a soft link `s` to `/a` and a dangling soft link `d` to
`/nope`: `hdf5_soft_links_earliest.h5` in the default (earliest) format, a
symbol-table group whose soft-link entries have cache type 2 and an undefined
header address, and `hdf5_soft_links_latest.h5` with `libver='latest'`, link
messages. In the earliest-format file the symbol-table node keeps its entries
sorted by name (`a`, `d`, `m`, `s`, `z`), so one hard link sits between the
soft links and one after both: a reader that stops at the first soft link
instead of skipping it lists only `a` (#919). Each oracle records what h5py lists and which members are
hard links; `tests/hdf5_soft_links.rs` checks that both list `a`, `m` and `z`
and decode all three.

## Userblock fixtures (`hdf5_userblock_{earliest,latest}.h5`, `hdf5_no_userblock_{earliest,latest}.h5`)

Four files for #936, built by `tools/build_hdf5_userblock_fixtures.py` with
h5py 3.16.0 (libhdf5 2.0.0), reproducible byte for byte. Each root holds a
contiguous `float32` `v` [4, 5] with a numeric and a fixed-length string
attribute, on the dimension scales `y` and `x` (so `DIMENSION_LIST` sends the
reader into the global heap), and a chunked, deflated `int16` `c` [6, 8]. The
`hdf5_userblock_*` files are written with `userblock_size=512`, and the builder
then writes a text header into the userblock; the `hdf5_no_userblock_*` twins
are the same content without one. `earliest` is the default format
(superblock version 0), `latest` is `libver='latest'` (version 3).

In the userblock files the signature is at byte 512 and the superblock's
stored Base Address is 512 too; every other address is relative to it. libhdf5
takes the signature's offset as the base when the stored field differs
(`H5F__super_read`), and the reader does the same without reading the field.
Each oracle records the signature offset, the stored base, the members, every
value and `v`'s dimension names as h5py reads them back.
`tests/hdf5_userblock.rs` checks that each userblock file lists and decodes
exactly as its twin and as the oracle, and the umbrella's
`tests/hdf5_userblock.rs` does the same through `Session::open`.

## Offset and length size fixtures (`hdf5_sizes_o8_l4.h5`, `hdf5_sizes_o4_l8.h5`)

Two files for #922, built by `tools/build_hdf5_size_fixtures.py` with h5py
3.16.0 (libhdf5 2.0.0), reproducible byte for byte. Both are in the earliest
format (`set_libver_bounds(EARLIEST, V18)`, superblock version 0), with Size of
Offsets and Size of Lengths set by `fcpl.set_sizes` to (8, 4) and (4, 8).
Without the explicit bounds h5py writes a version-2 superblock and no
version-1 B-tree, so the builder checks the version byte.

Each root holds 15 datasets, more than one symbol-table node holds, so the
group B-tree has several keys. `many_attrs` gets 30 attributes after later
objects are written, so its header needs a continuation (the oracle records
the chunk count). `v` is gzip-chunked with an unlimited first dimension, and
dimension scales `time` and `x` are attached, so its `DIMENSION_LIST` lives in
the global heap. A nested group `g` holds `inner`, and a group `t` that tracks
creation order holds `tracked`: its link info message has an 8-byte maximum
creation index, which the reader used to read at Size of Lengths. Each oracle
records what h5py reads: values, shapes, maxima, dimension labels and
attributes.

**Known divergence from libhdf5.** `time` was created with `maxshape=(None,)`.
In the (8, 4) file its maximum is stored as `0xFFFF_FFFF`, and h5py reads it
back as 4294967295 rather than unlimited. Fieldglass reads it as unlimited.
**Known divergence from the specification.** The specification's "Symbol
Table Entry" table gives the link-name offset as Size of Offsets; libhdf5 and
Fieldglass read it at Size of Lengths. ADR-0015 records both, with the
field-by-field audit. `tests/hdf5_offset_length_sizes.rs` checks the listing,
values, dimensions and attributes against the oracles.

## B-tree K fixtures (`hdf5_btree_k_sb1.h5`, `hdf5_btree_k_sb2.h5`)

Two files for #920, built by `tools/build_hdf5_btree_k_fixtures.py` with h5py
3.16.0 (libhdf5 2.0.0). `hdf5_btree_k_sb1.h5` is reproducible byte for byte.
`hdf5_btree_k_sb2.h5` is not: libhdf5 stamps the superblock extension's object
header with the creation time whatever `set_obj_track_times` says, so a rebuild
differs in those four timestamps and the header's checksum. Both are written
with `H5Pset_sym_k(32, 8)` and `H5Pset_istore_k(64)`, so a node may hold 16
symbol-table entries, 64 group B-tree children and 128 chunk B-tree children,
against 8, 32 and 64 at the defaults. h5py wraps neither call, so the builder
makes them through `ctypes` on the libhdf5 h5py loaded (found in
`/proc/self/maps`; a second copy of the library would not know h5py's
property-list IDs).

`hdf5_btree_k_sb1.h5` uses `set_libver_bounds(EARLIEST, V18)`; a non-default
indexed-storage K makes that a version-1 superblock, which states all three K
values. Its root holds `v` (100 `i4` values in one-element chunks, all in one
chunk B-tree node), twelve empty groups and `wide`, a group of 296 soft links,
so its symbol-table nodes hold up to 16 entries and its group B-tree node 36.
`hdf5_btree_k_sb2.h5` uses `set_libver_bounds(V18, V18)`: a version-2
superblock whose extension carries a B-tree 'K' Values message, holding the
same `v`. Its groups are new-style, so only the chunk node uses K.

Each oracle records what h5py reads and the fullest node of each kind, found
by scanning for node signatures; the builder checks each is past the default
2K. `tests/hdf5_btree_k.rs` reads both, then patches the version-1 file's K to
the smallest value that holds its fullest node, and to one less, to show each
of the three walkers caps at exactly 2K. All three counts are even, so the
smaller K is exactly full and a `>=` where `>` belongs would fail.

**Reading of the specification.** The superblock table says Group Leaf Node K
bounds "each leaf node of a group B-tree". libhdf5 uses it for symbol-table
nodes only, as the specification's "Symbol Table Nodes" section does, and
bounds every level of the group B-tree by Group Internal Node K. Fieldglass
does the same; the other reading would refuse libhdf5's own default files,
whose level-0 group nodes exceed 8 entries once a group has a few dozen
members.

**Known divergence from libhdf5.** libhdf5 refuses a version-0 or -1
superblock that states a zero Group Leaf or Group Internal Node K when the
file opens. Fieldglass refuses only a non-empty node bounded by a zero K, so
such a file whose symbol-table groups are all empty lists, as empty. No value
is misread.

## NetCDF-4 dimension-scale fixture (`netcdf4_dimscale.nc`)

A small NetCDF-4 file written with the canonical Unidata `netCDF4` library (which
wraps `libnetcdf` / libhdf5) as the target for dimension-scale resolution
(#174, under #33; decision 0003). Unlike the two `hdf5_*` fixtures — which are
pure `h5py` and carry **no** dimension scales — this lays down the real
`CLASS = "DIMENSION_SCALE"` / `DIMENSION_LIST` / `_Netcdf4Dimid` machinery, so it
exercises the semantic layer that maps HDF5 dimension scales to named netCDF
dimensions and resolves each variable's ordered dimension list. Built by
`tools/build_netcdf4_dimscale_fixture.py` (run from the repo root; needs
`netCDF4`).

It covers every classification the resolver makes: an **unlimited** dimension
with a coordinate variable (`time`), regular coordinate variables (`lat` /
`lon`), a **pure dimension** with no coordinate variable (`nv` — the
`"This is a netCDF dimension but not a netCDF variable."` placeholder), a
multi-dimensional **data variable** whose `DIMENSION_LIST` must resolve to
ordered names (`temperature(time, lat, lon)`), and a variable that references the
pure dimension (`lat_bnds(lat, nv)`).

The sibling `netcdf4_dimscale.nc.oracle.json` is `ncdump -h` in JSON form: per
dimension its length and unlimited flag; per variable its netCDF type, ordered
dimension names, and whether it is a coordinate variable. `nc_type` is the
canonical netCDF type name (matching the Rust reader's `NcType::name()`), not the
numpy alias. `tests/hdf5_dimension_scales.rs` pins the resolver against it.

## NetCDF-4 unsupported-datatype fixture (`netcdf4_unsupported_type.nc`)

A small NetCDF-4 file written with the canonical Unidata `netCDF4` library that
mixes variables the reader decodes with variables it does not, the target for
#550. `hdf5/datatype.rs` maps three HDF5 datatype classes onto `NcType` —
fixed-point, IEEE floating point, fixed-length string — and files carrying a
compound or variable-length variable alongside ordinary fields are routine:
station-record files, OMI / TROPOMI granules, anything written through
`nc_def_compound`. One such dataset used to fail metadata resolution for the
whole file. Built by `tools/build_netcdf4_unsupported_type_fixture.py` (run from
the repo root; needs `netCDF4`).

It carries, in this creation order:

- `station_info(station)` — an HDF5 **compound** datatype (class 6),
- `visits(station)` — an HDF5 **variable-length** datatype (class 9),
- `time(time)` — a plain `double` coordinate variable,
- `temperature(time)` — a plain `float` data variable,
- `station` — a pure dimension with no coordinate variable.

The two undecodable datasets are written **first** on purpose, so both take a
lower whole-file dataset index than `temperature`. That index space is the one
`decode_variable_raw` walks, so skipping a dataset must leave a hole rather
than close one; `tests/hdf5_unsupported_datatype.rs` proves it by decoding
`temperature` through the `decode_index` the resolver reports for it and
comparing the values against the oracle.

The sibling `netcdf4_unsupported_type.nc.oracle.json` is `ncdump -h` in JSON
form, as for `netcdf4_dimscale.nc`, plus a `decodable_datatype` flag per
variable (`false` for the compound and vlen ones) and the `time` /
`temperature` values. A user-defined type is reported by its kind
(`"compound"`, `"vlen"`, `"enum"`) where a plain variable reports its netCDF
type name.

## NetCDF-4 nested-group fixture (`netcdf4_grouped.nc`)

A small grouped NetCDF-4 file written with the canonical Unidata `netCDF4`
library, the target for nested-group resolution (#219). Where `netcdf4_dimscale.nc`
lives entirely in the root group, this lays out a Sentinel-5P-style tree so the
resolver must descend into groups and present objects with path-qualified names:

- a **root** dimension + coordinate variable (`time`) that stays bare-named,
- a nested group `/PRODUCT` with its own `scanline` / `ground_pixel` dimensions
  and variables (`/PRODUCT/latitude`, `/PRODUCT/longitude`, `/PRODUCT/qa_value`),
- `/PRODUCT/qa_value`, whose `DIMENSION_LIST` mixes the **ancestor** root `time`
  dimension with its own group's dimensions — the netCDF scoping rule that a
  dimension is visible to its group and all descendants,
- a **two-level** group `/PRODUCT/SUPPORT_DATA` whose `surface_altitude` variable
  is path-qualified through both levels.

The sibling `netcdf4_grouped.nc.oracle.json` path-qualifies every object exactly
as the Rust reader does (a root object keeps its bare name; a nested one is its
group's leading-slash path plus `/name`), with each variable's dimension names
qualified by the group that *defines* each dimension. Built by
`tools/build_netcdf4_grouped_fixture.py` (run from the repo root; needs `netCDF4`);
`tests/hdf5_nested_groups.rs` pins the resolver and the decode path against it.

## Projected-grid fixtures (`wrf_lambert.nc`, `wrf_polar.nc`, `wrf_mercator.nc`, `wrf_latlon.nc`, `goes_geostationary.nc`, `goes_geostationary_classic.nc`, `goes_geostationary_metres.nc`)

Targets for projected-grid geolocation (#168, #220, and #226; decision 0004) — regular
grids in a projected CRS, rendered through the analytic-inverse warp (Model A).
All are self-generated by `tools/build_netcdf_projected_fixtures.py` (run from
the repo root; needs `netCDF4` + `numpy`), so there is **no upstream provenance
or licensing constraint**. They are deliberately tiny toy grids; the official
NOAA GOES and a real `wrfout` subset belong to the bundled corpus (#123).

The coordinate geometry is generated with *independent* NumPy implementations of
the standard projection formulas — Snyder Lambert Conformal Conic, Snyder polar
stereographic, spherical Mercator, and the GOES-R PUG fixed-grid algorithm — so
the Rust projectors reproducing it is a genuine cross-language check rather than
a tautology.

- `wrf_lambert.nc` (classic NetCDF-3) is a WRF `wrfout`-style file: the Lambert
  projection lives in **global attributes** (`MAP_PROJ = 1`, `TRUELAT1` /
  `TRUELAT2`, `STAND_LON`, `MOAD_CEN_LAT`, `DX` / `DY`), and the 2-D `XLAT` /
  `XLONG` arrays are precomputed conveniences whose `(0, 0)` corner fixes the grid
  origin. The fixture adopts the projector's spherical Earth radius (6 371 229 m);
  real `wrfout` uses 6 370 000 m — the same ~0.02 % approximation the GRIB Lambert
  path already makes.
- `wrf_polar.nc` and `wrf_mercator.nc` (#220) are the same `wrfout` shape with
  `MAP_PROJ = 2` (polar stereographic: `DX`/`DY` true at `TRUELAT1`, oriented
  along `STAND_LON`, hemisphere from `TRUELAT1`'s sign) and `MAP_PROJ = 3`
  (Mercator: uniform projected metres, geolocated from the `XLAT`/`XLONG`
  corner coordinates alone). They adopt the same 6 371 229 m sphere as
  `wrf_lambert.nc`, so the same ~0.02 % radius approximation against real
  `wrfout` applies — the oracle cross-check exercises the projection math, not
  that constant.
- `wrf_latlon.nc` (#226) is the same `wrfout` shape with `MAP_PROJ = 6`
  (unrotated lat-lon: `POLE_LAT = 90`, `POLE_LON = 0`). An unrotated domain is a
  plain regular geographic grid, so `DX`/`DY` are **degrees** (not metres) and
  the grid is geolocated from the `XLAT`/`XLONG` corner coordinates alone, like
  the Mercator fixture. A rotated domain (`POLE_LAT != 90`) is deliberately not
  fixtured — its geolocation is deferred (source-only), so there is nothing to
  cross-check.
- `goes_geostationary.nc` (NetCDF-4 / HDF5) is a GOES ABI-style file: a CF
  `grid_mapping` variable `goes_imager_projection`
  (`grid_mapping_name = "geostationary"`, GRS80 ellipsoid, `sweep_angle_axis =
  "x"`, GOES-East sub-satellite longitude) and 1-D `x` / `y` *radian* scan-angle
  coordinate variables stored as **scaled `int16`** (the real GOES encoding),
  exercising CF `scale_factor` / `add_offset`.
- `goes_geostationary_classic.nc` (#844, classic NetCDF-3) is the same dataset,
  written by the same function with `format="NETCDF3_CLASSIC"`. It has no oracle
  of its own; it exists so a test can overwrite one grid-mapping attribute in
  place. A classic header stores each `f64` big-endian and carries no checksum,
  whereas the HDF5 object header checksums its attributes, so the NetCDF-4 file
  cannot be edited that way. `crates/fieldglass/tests/non_finite_geometry.rs`
  sets `semi_major_axis`, `semi_minor_axis` and `perspective_point_height` to
  values that describe no camera and checks the slice is not placed. The napi
  characterisation golden renders it identically to `goes_geostationary.nc`.
  The same test overwrites one axis's `units = "rad"` with `deg` and checks
  the mapping is declined (#966).
- `goes_geostationary_metres.nc` (#966, NetCDF-4) is the same grid with `x` /
  `y` in **metres**, stored unpacked as `f8` with `units = "m"`: the scan
  angles times `perspective_point_height`, which is PROJ's `+proj=geos`
  easting and northing and what satpy, pyresample and GDAL write. Its sibling
  `goes_geostationary_metres.nc.oracle.json` is **PROJ's own inverse** (the
  `proj` CLI, `+proj=geos +sweep=x`, PROJ 9.4.0 here) at every pixel, and the
  builder asserts it agrees with the NumPy fixed-grid transcription behind the
  radian oracle to 1e-9°. Building this one file needs PROJ installed.
  `tests/projected_grids.rs` checks the Rust projector against it, and
  `crates/fieldglass/tests/geostationary_units.rs` checks a `Session` places
  it where it places `goes_geostationary.nc`.

Each of the other six has a sibling `*.oracle.json` with the resolved projection parameters and
sampled `(i, j) ↔ (lat, lon)` geolocation. `tests/projected_grids.rs` resolves
the projection from the on-disk metadata and asserts the `fieldglass_core`
projector reproduces the oracle.

## Real GOES-16 ABI fixture (`goes16_abi_cmip.nc`)

The first **real operational** NetCDF-4 / HDF5 file in the corpus (#123) — a
small subset of a genuine NOAA GOES-16 ABI L2 Cloud & Moisture Imagery product:

- Product: ABI L2 CMIP, Mesoscale sector 1, band 13 (10.3 µm IR), GOES-East.
- Source object (immutable, in the public NOAA archive):
  `s3://noaa-goes16/ABI-L2-CMIPM/2023/001/18/`
  `OR_ABI-L2-CMIPM1-M6C13_G16_s20230011800281_e20230011800350_c20230011800425.nc`
- License: a work of the U.S. Government — **public domain**, no copyright. NOAA
  requests attribution to "NOAA/NESDIS, GOES-R Series."

Built by `tools/build_goes_real_fixture.py` (run from the repo root; needs
`netCDF4` + `numpy`; downloads the source object once). The script keeps a
`24 × 24` center window of the 500×500 grid plus the `goes_imager_projection`
grid mapping, the scaled-`int16` `x` / `y` scan-angle coordinates, and the
`CMI` / `DQF` fields; the dozens of ancillary scalar metadata variables are
dropped to keep the fixture byte-small. The raw on-disk `int16` / `int8` codes
are copied verbatim (auto-scaling off), so the genuine CF `scale_factor` /
`add_offset` / `valid_range` / `_FillValue` attributes and the real GRS80 /
sub-satellite-longitude projection parameters survive unchanged. `CMI` keeps the
real chunked + deflate storage, so the HDF5 value path decodes a real compressed
field end to end.

Unlike the synthetic `goes_geostationary.nc`, the attributes here are rich enough
that their dense storage spills into a fractal heap with an **indirect root
block** (a doubling table of direct blocks) — the structure real attribute-heavy
NetCDF-4 files use, exercised here for the first time.

The sibling `goes16_abi_cmip.nc.oracle.json` records the resolved projection
parameters, the `(i, j) ↔ (lat, lon)` geolocation (computed by an *independent*
NumPy transcription of the GOES-R PUG fixed-grid algorithm, so the Rust projector
reproducing it is a cross-language check), and the `CMI` brightness temperatures
`netCDF4` decodes (CF-unpacked, in Kelvin). `tests/goes_real_world.rs` asserts
the HDF5 backing, the dimension/variable resolution, the geolocation, and the
chunked-field value decode against it.

## CF packed-data fixture (`cf_packed_data.nc`)

Target for CF **data-variable** unpacking (#184): `scale_factor` / `add_offset`
+ `valid_range` applied to the rendered field, the way GOES `Rad`, MERRA-2, and
ERA5 store it as scaled `int16`. The companion projected fixtures above already
exercise the CF convention on *coordinate* arrays; this packs the data plane
itself.

Self-generated by `tools/build_netcdf_cf_packed_fixture.py` (run from the repo
root; needs `netCDF4` + `numpy`), so there is **no upstream provenance or
licensing constraint** — a tiny `3 × 4` toy grid. `temp(lat, lon)` is a scaled
`int16` carrying `scale_factor = 0.0625` (a power of two, hence exact in both
float32 and float64), `add_offset = 250`, `_FillValue = -9999`, and
`valid_range = [0, 10000]`, plus 1-D `lat`/`lon` coordinates.

The sibling `cf_packed_data.nc.oracle.json` records both the raw on-disk codes
(only `_FillValue` masked) and the physical values `netCDF4` produces with auto
mask+scale on — the CF unpacking the Rust decode + `unpack_cf_data` must
reproduce. `tests/cf_packed_data.rs` asserts both stages against it.

## CF `missing_value` fixtures (`missing_value_classic.nc`, `missing_value_nc4.nc`)

Target for `missing_value` masking: libnetcdf masks a point equal to either
`_FillValue` **or** the CF `missing_value` attribute. `temp(y, x)` is an `int16`
marking gaps with a distinct `_FillValue` and a scalar `missing_value`, points
hitting each. The same logical field is bundled in both on-disk encodings —
classic NetCDF-3 (`…_classic.nc`) and NetCDF-4 / HDF5 (`…_nc4.nc`) — so each
decode backing is exercised end-to-end.

Self-generated by `tools/build_netcdf_missing_value_fixture.py` (run from the
repo root; needs `netCDF4` + `numpy`), so there is **no upstream provenance or
licensing constraint** — a tiny `2 × 3` toy grid. The shared
`missing_value.oracle.json` records the masked array `netCDF4` produces with
auto-mask on; `tests/missing_value.rs` asserts both backings reproduce it.

## Real NOAA OISST v2.1 fixture (`oisst_avhrr_v2.nc`)

The second **real operational** NetCDF-4 / HDF5 file in the corpus (#123) — a
tiny window subset of a genuine NOAA/NCEI Optimum Interpolation Sea Surface
Temperature analysis:

- Product: OISST v2.1, AVHRR, daily 1/4° global, 2025-01-01.
- Source object (immutable, in the public NOAA CDR archive):
  `s3://noaa-cdr-sea-surface-temp-optimum-interpolation-pds/`
  `data/v2.1/avhrr/202501/oisst-avhrr-v02r01.20250101.nc`
- License: a NOAA Climate Data Record produced by NOAA/NCEI — a work of the
  U.S. Government, **public domain**, no copyright. Attribution: NOAA/NCEI.

Built by `tools/build_oisst_real_fixture.py` (run from the repo root; needs
`netCDF4` + `numpy`; downloads the source object once). The script keeps a
`32 × 32` Hudson Bay window (rows 592–624, cols 1112–1144) of the global grid —
a January high-latitude scene chosen so all three real behaviours appear at
once: land and sea ice fill the `sst` mask (~1/3 of the window), while the rest
carries near-freezing water and the `ice` field genuine sea-ice concentrations.
It retains the `sst` and `ice` fields plus the `time` / `zlev` / `lat` / `lon`
coordinate variables; the dozens of ancillary attributes that name the full grid
extent are dropped or noted as a window subset in `history`. The raw on-disk
`int16` codes are copied verbatim (auto-scaling off), so the genuine CF
`scale_factor` / `add_offset` / `valid_min` / `valid_max` / `_FillValue`
attributes survive unchanged, and `sst` / `ice` keep the real chunked + deflate
+ **shuffle** storage, so the HDF5 value path decodes a real compressed field end
to end.

It complements the geostationary `goes16_abi_cmip.nc` with a different slice of
the stack: a **regular 1/4° lat/lon** analysis grid (vs the GOES fixed scan
grid), the deflate + **shuffle** filter chain (GOES used deflate alone), CF
unpacking driven by scalar `valid_min` / `valid_max` (GOES used the two-element
`valid_range`), and a 4-D `(time, zlev, lat, lon)` variable with singleton
`time` / `zlev`. Its 25 retained global attributes still exceed libhdf5's
8-attribute compact threshold, so the metadata spills into **dense** storage
(fractal heap `FRHP` + B-tree v2 `BTHD`) — the layout the #33 robustness work
hardened, exercised here on a real file.

The sibling `oisst_avhrr_v2.nc.oracle.json` records the regular-grid geolocation
(corner + 0.25° spacing) and, per packed field, the masking + scaling `netCDF4`
produces (auto mask+scale on): present / missing counts, value statistics, and
anchored per-index samples. `tests/oisst_real_world.rs` asserts the HDF5
backing, the dimension / variable resolution, the regular-grid coordinates, and
the chunked + deflate + shuffle value decode against it.

## fletcher32 checksum fixture (`hdf5_fletcher32.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` carrying the
**fletcher32** checksum filter (id 3, target for #412). fletcher32 is not a
compressor: it appends a four-byte checksum to each stored chunk, which reading
verifies and strips.

The same 8×8 `float32` field (`arange(64) * 0.5`, 4×8 chunks) is written five
ways so the test compares them directly: `plain_deflate` (deflate alone, the
baseline), `f32_deflate`, `f32_only` (no compressor, so the stored chunk is the
element bytes verbatim plus four), `f32_shuffle_deflate`, and `f32_shuffle`
(shuffle + fletcher32, no compressor). A sixth dataset, `f32_odd`, is a 7-byte
`uint8` chunk that exercises the checksum's trailing-odd-byte branch, and is
also the only single-chunk dataset here (7 B raw, 11 B stored) — so it covers
the single-chunk index, whose chunk length comes from the layout message's
filtered size rather than from the chunk shape. The rest hold two chunks and go
through a fixed array.

`f32_shuffle` is the dataset that earns its place: it is the only one on which
a reader that returned the chunk unchanged is *visibly* wrong. The compressed
pipelines absorb the four extra bytes in the zlib decoder and decode correctly
anyway, whereas 132 bytes divides evenly by the 4-byte element and unshuffles
into 33 elements instead of 32, silently.

Built by `tools/build_hdf5_fixtures.py` (`build_fletcher32`, `track_times=False`
for reproducibility). The builder asserts that libhdf5 really did put filter 3
last in each pipeline and that `f32_only`'s stored chunk is exactly four bytes
longer than its raw chunk, so a libhdf5 that stopped applying the filter fails
the build rather than yielding a fixture that proves nothing. The checksum
vectors pinned in `hdf5/filter.rs` come from libhdf5 itself via the same file's
`fletcher32_oracle` helper. `tests/hdf5_fletcher32.rs` checks the decoded values
and the corruption reports. Part of #412.

## zstd filter fixture (`hdf5_zstd.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` carrying the
**zstd** filter (id 32015, target for #413) — the compressor netcdf-c >= 4.9
recommends for new climate archives.

The same 8x8 `float32` field (`arange(64) * 0.5`, 4x8 chunks) is written five
ways: `plain_deflate` (the comparison baseline, no zstd), `zstd`,
`shuffle_zstd`, `zstd_fletcher32`, and `zstd_high` (level 19, a different frame
layout from the default).

`shuffle_zstd` is the dataset that earns its place. netcdf-c writes shuffle
then zstd, so reading has to undo zstd *first*; it is the one dataset here on
which a reader that ran the pipeline in write order is visibly wrong, and the
builder asserts its pipeline is exactly `[2, 32015]` so that ordering claim
cannot quietly stop being tested.

Unlike every other builder in `tools/build_hdf5_fixtures.py`, this one needs
**`hdf5plugin`** in addition to `h5py`: libhdf5 cannot write filter 32015
without the plugin it supplies. The builder fails the build rather than emit a
fixture with the filter silently absent. `tests/hdf5_zstd.rs` checks the decoded
values and the corrupt-frame report. Part of #413.

## szip filter fixtures (`hdf5_szip.h5`, `hdf5_szip_hand.h5`)

Synthetic files carrying the **szip** filter (id 4, #421), built by
`build_szip` and `build_szip_hand` in `tools/build_hdf5_fixtures.py` with
**h5py 3.16.0, libhdf5 2.0.0 and libaec 1.1.4**. The h5py wheel bundles libaec
as libsz, so libhdf5 writes szip with no plugin; the builder reads the libaec
version out of the bundled library and records it in each oracle's `source`.
Each `*.oracle.json` holds, per dataset, the h5py read-back of **every** value
(the test compares all of them), the pipeline and `cd_values` the file really
carries, and each chunk's filter mask, file offset and stored size. The
builders are deterministic: rebuilding gives the same bytes.

`hdf5_szip.h5` (`libver='latest'`) is written by libhdf5 itself. `cd_values`
are `(mask, pixels per block, bits per pixel, pixels per scanline)`, and the
builder refuses to write a dataset whose values are not the ones listed:

| Dataset | Type | `cd_values` | What it covers |
| --- | --- | --- | --- |
| `i2_ppb16` | `<i2` | 169, 16, 16, 32 | Two chunks; the size-prefix test alters the first |
| `i2_ppb10` | `<i2` | 169, 10, 16, 40 | Block 10, a size HDF5 allows outside CCSDS's 8/16/32/64 |
| `i4be_ppb32` | `>i4` | 177, 32, 32, 64 | MSB (mask bit 16, set for big-endian data); byte planes |
| `f4_ppb18` | `<f4` | 169, 18, 32, 36 | Block 18; 32-bit byte planes |
| `f8_ppb8` | `<f8` | 169, 8, 64, 16 | 64-bit byte planes, two chunks |
| `f8_ppb18` | `<f8` | 169, 18, 64, 20 | 64-bit, and 20 pixels per scanline padded to 36 |
| `u1_ppb8` | `\|u1` | 169, 8, 8, 32 | 8-bit pixels |
| `i4_precision16` | `<i4`, precision 16 | 169, 8, 16, 16 | Pixel (2 bytes) narrower than the element (4), decision D2 |
| `shuffle_szip` | `<i4` | 169, 16, 32, 32 | Pipeline `[shuffle, szip]` |
| `pps_not_multiple` | `<i2` | 141, 16, 16, 25 | 25 pixels per scanline padded to 32; the `EC` mask |
| `partial_scanline` | `<i2` | 169, 8, 16, 1024 | 3,000 pixels end part-way through the third 1,024-pixel scanline |
| `f4_pps_capped` | `<f4`, 1-D | 169, 32, 32, 4096 | One 5,000-pixel chunk: libhdf5 caps a scanline at 128 blocks, so the last scanline holds 904 pixels and libsz pads it to 4,096 |
| `f8_pps_capped` | `<f8`, 1-D | 169, 8, 64, 1024 | The same at 64 bits: 1,500 pixels, the last scanline 476 of 1,024 |
| `incompressible` | `\|u1` | 141, 8, 8, 8 | Random bytes: szip does not shrink them, so libhdf5 stores the chunk as it is with filter-mask bit 0 set |

`i4_precision16` holds only non-negative values. The reader does not apply a
fixed-point precision below the element width, so a negative value in a
16-bit-precision `int32` (stored `ec ff 00 00` for -20) would read as 65516.
That is a datatype gap, not an szip one (#795), and is kept out of this
fixture.

`hdf5_szip_hand.h5` (`libver='earliest'`) holds chunks libhdf5 reads but its
writer never produces. Each chunk is compressed by the wheel's own libsz
(`SZ_BufftoBuffCompress` through `ctypes`, the call `H5Zszip.c` makes), given
the 4-byte size prefix, and stored with `write_direct_chunk`. libhdf5's read-back
is the oracle, and the builder checks it equals the source array.

| Dataset | Type | `cd_values` | How it was made |
| --- | --- | --- | --- |
| `rsi1_i2` | `<i2` | 169, 16, 16, **5** | Written with pixels per scanline 40, then patched to 5 in the stored filter message: one block per scanline (RSI 1), 11 of its 16 pixels padding |
| `rsi1_f4` | `<f4` | 169, 32, 32, **1** | Patched from 64 to 1: one pixel per 32-pixel block, the case where libsz decodes into a copy 32 times the output |
| `deflate_szip` | `\|u1` | 169, 8, 8, 64 | Pipeline `[deflate, szip]`. libhdf5 skips szip on every chunk here, since szip never shrinks a deflate stream, so the chunks are `szip(zlib(data))` written with filter mask 0. The prefix is the deflate stream's length (61), not the chunk's (64) |

libhdf5's `set_local` never picks a scanline shorter than a block: it uses the
whole chunk instead. The patch is why this file is `libver='earliest'`:
version-1 object headers carry no checksum, so the edited filter message stays
valid. The builder asserts each patched sequence occurs exactly once.

**Where the reader is stricter than libhdf5.** libhdf5 checks the size prefix
only in a debug-build `assert` (`H5Zszip.c`), and libsz returns success with
short output when a stream runs out. Setting the prefix of `i2_ppb16`'s first
chunk to 256 MiB + 1 (`MAX_DECOMPRESSED_CHUNK` + 1) reads back correctly through
h5py 3.16, after allocating that much; the same on `deflate_szip` also reads
back. The reader refuses both, before allocating. When only shuffle precedes
szip the prefix must equal the chunk's length; behind a length-changing filter
it may be at most the chunk's length plus an eighth plus 4 KiB (4,168 bytes
for `deflate_szip`), which covers deflate's, zstd's and fletcher32's growth,
and 33 times the chunk plus 4 KiB behind an szip (#813,
`hdf5_szip_growth.h5`).
A prefix of 511 or 513 on the 512-byte chunk fails in libhdf5 as well.
`a_chunk_whose_size_prefix_is_wrong_is_refused` and
`behind_deflate_the_prefix_is_bounded_by_the_chunk` in
`tests/hdf5_szip.rs` pin both; ADR-0012 decision 4 lists the divergence.

## szip growth fixture (`hdf5_szip_growth.h5`)

One `[szip, deflate]` chunk that libhdf5 reads, built by `build_szip_growth`
in `tools/build_hdf5_fixtures.py` with the same h5py, libhdf5 and libaec
(#813). `i2_growth` is a single 2,048-value `<i2` chunk (4,096 bytes) of
-1s (all bits set, the worst of a sweep of libsz over every 16-bit value),
`cd_values` 141, 32, 16, **1**: entropy coding without the NN
preprocessor, which would predict a constant chunk exactly, at 32 pixels per
block, with pixels per scanline patched from 2,048 to 1 the way
`hdf5_szip_hand.h5` patches its RSI-1 datasets (so `libver='earliest'`).
libsz pads each one-pixel scanline to a 32-pixel block, so the szip stream is
107,268 bytes, 26 times the chunk; deflate stores it in about a kilobyte, and
the chunk is written with `write_direct_chunk`. libhdf5's read-back is the
oracle, and the builder checks it equals the source and that the stream
still grows past the chunk plus an eighth plus 4 KiB.

The reader bounds deflate's output behind an szip by the chunk times 33 plus
4 KiB (139,264 bytes here). Any factor below 26 refuses this chunk, which is
what `a_codec_behind_szip_decodes_a_stream_26_times_its_chunk` in
`tests/hdf5_szip.rs` pins.

## szip long-stream fixture (`hdf5_szip_long_stream.h5`)

One chunk the reader must refuse, built by `build_szip_long_stream` in
`tools/build_hdf5_fixtures.py` with the same h5py, libhdf5 and libaec (#794).
`f8_long_stream` is a single 4x8 `<f8` chunk (256 bytes), `cd_values` 169, 8,
64, 8, szip alone, with the right size prefix, 256. Its stream is libsz's
encoding of 64 values, twice the chunk, stored with `write_direct_chunk`.

64-bit pixels are coded as byte planes laid out by the output's length, so
decoding 256 bytes of that stream puts bytes in the wrong places. libhdf5
reads the chunk without complaint and returns that: 60 of its 256 bytes differ
from the 32 values the chunk should hold. The oracle records libhdf5's
read-back as `values`, the intended values as `source_values`, and the count
as `wrong_bytes`; the builder fails if libhdf5 ever reads the chunk
correctly. Every length rule the reader applies passes here, so
`fieldglass_aec::sz::decompress` is what refuses it
(`AecError::TrailingInput`: a whole byte of stream left after the one the
output's length implies). `a_chunk_whose_stream_codes_more_pixels_is_refused`
in `tests/hdf5_szip.rs` pins it; ADR-0012 decision 4 lists the divergence.

## Anonymous-dimension fixture (`hdf5_phony_dims.h5`)

A synthetic `h5py` (libhdf5) file written with `libver='latest'` carrying **no
dimension scales at all**, so no dataset declares an axis and every one has to
be invented (#533).

Five datasets, shaped to rule out every rule simpler than netCDF-C's actual
per-axis reuse: `a_8x8` and `b_8x8` share a shape (the second must reuse the
first's pair); `a_8x8`'s two axes are both 8 long and still need two dimensions
(so length alone cannot deduplicate); `d_6x4` is `c_4x6` transposed (so whole
shapes cannot match either); and `e_1d7` is 1-D. Datasets are **created in an
order that differs from their alphabetical order**, because netCDF-C numbers
invented dimensions by name — a builder that agreed by accident would hide a
reader that numbered by discovery order instead.

The oracle is netCDF-C itself, read through netCDF4-python: `phony_dim_0..4` =
8, 8, 4, 6, 7, with `a_8x8`/`b_8x8` on `(0, 1)`, `c_4x6` on `(2, 3)`, `d_6x4` on
`(3, 2)` and `e_1d7` on `(4)`. `tests/hdf5_phony_dims.rs` pins all of it.
Built by `tools/build_hdf5_fixtures.py`. Part of #533.

## Fixed-point precision fixture (`hdf5_fixed_point_precision.h5`)

A synthetic file written by h5py 3.16.0 (libhdf5 2.0.0) with `libver='latest'`,
holding integers that do not fill their container (#795). The HDF5 file format
specification (version 3, IV.A.2.d, "Fixed-Point Bit Field Description" and
"Fixed-Point Property Description") gives a fixed-point type a bit offset and a
bit precision: the value is the `precision` bits starting `offset` bits above
the least significant bit, the bits below and above are padding (zeros or ones
per the lo_pad / hi_pad flags), and a signed value's sign bit is the top bit of
the precision. libhdf5 agrees with the spec here, so h5py's read-back is the
value oracle.

h5py's high-level API cannot ask for such a type, so `build_fixed_point_precision`
builds each one through `h5py.h5t` (copy a standard type, then `set_precision`,
`set_offset`, `set_pad`) and writes native values through `h5d`, letting
libhdf5 pack them. The datasets cover signed and unsigned, little- and
big-endian, 1-, 2-, 4- and 8-byte containers, offsets 0 to 8, negative values
down to each precision's minimum, padding written as ones, a chunked dataset
whose unwritten chunks read as a packed Fill Value message default (-7), and a
dataset masked by a `_FillValue` attribute of the packed type. Two root
attributes carry packed values too. The oracle records each type's offset,
precision, padding and sign, the values h5py reads back, and the stored bytes
of every contiguous dataset, so the packing is visible in the fixture itself.

`f32_prec24` is a 24-bit float in a 32-bit container (sign at bit 23, 7-bit
exponent at bit 16, 16-bit mantissa, bias 63). h5py reads it back as
[1.5, -2.25, 0.0]. The reader decodes only the IEEE binary32 / binary64
layouts and reports this one as unsupported rather than reading its container
as an IEEE `f32`.

The build is reproducible (`track_times` off). `tests/hdf5_fixed_point_precision.rs`
checks every dataset and attribute against the oracle. Part of #795.

## Curvilinear corpus (`rtofs_tripolar_arctic.nc`, `mirs_swath_n21.nc`)

The two-dimensional-coordinate corpus for #444, built by
`tools/build_netcdf_curvilinear_fixtures.py`. Both are windows of immutable
objects in public AWS Open Data buckets that need no credentials, so the
builder reproduces the same bytes on any future run — neither source is a
rolling operational file. Both are works of the U.S. Government (NOAA/NCEP and
NOAA/NESDIS) and are **not subject to copyright protection** in the United
States (17 U.S.C. § 105); NOAA distributes them without restriction.

The pair exists because "curvilinear" covers two different shapes of
irregularity, and an implementation can handle one and fail the other.

### `rtofs_tripolar_arctic.nc` — ocean tripolar

- Source: <https://noaa-nws-rtofs-pds.s3.amazonaws.com/rtofs.20240201/rtofs_glo_2ds_n000_ice.nc>
- Product: NCEP Global Real-Time Ocean Forecast System (RTOFS), global HYCOM,
  2-D surface ice fields, 2024-02-01 nowcast (`n000`).
- Subset: `[Y 3098:3298, X 1024:1284]` of the 3298 x 4500 source, plus
  `Latitude`, `Longitude`, `MT`, `Date` and three ice fields; deflate level 9.

South of about 47 °N the RTOFS mesh is an ordinary Mercator lat/lon. North of it
the grid is replaced by a **bipolar** patch whose two poles are placed over land
so that neither sits in the ocean, which is what lets the model run to the pole
without a singularity in water. The consequence for a reader is that a single
row of the array runs from 47 °N up over a pole and back down: the last row of
the *full* grid reaches 90 °N twice, at columns 1124 and ~3374.

The committed window is centred on the first of those. Every row in it varies in
latitude — 1.7° at the bottom edge rising to 5.0° at the row over the pole —
where a row of the regular mesh is flat to float precision. Its longitudes are
kept as the source writes them, **unnormalised**, spanning 74° to 1019°.

### `mirs_swath_n21.nc` — satellite swath

- Source: <https://noaa-nesdis-n21-pds.s3.amazonaws.com/NPR_MIRS_IMG_33min/2023/09/19/NPR-MIRS-IMG_33min_v11_n21_s202309191449310_e202309191523380_c202309191705326.nc>
- Product: NOAA-21 (JPSS-2) Microwave Integrated Retrieval System (MiRS)
  imagery, 33-minute granule, 2023-09-19.
- Subset: scanlines `[660:760]` of 768, all 96 fields of view, plus `Latitude`,
  `Longitude` and four retrieved fields; deflate level 9.

A cross-track microwave sounder geolocates every field of view individually, so
the swath is curvilinear by construction rather than by a polar patch. The
committed scanlines are the end of the descending pass, chosen because they
cross the antimeridian *and* converge on the south pole (reaching 85 °S) — the
two cases a naive reader gets wrong in opposite directions, one by unwrapping
longitude and one by assuming a row is a parallel. A benign mid-latitude window
of the same granule would satisfy every structural assertion and catch neither.

The retrieved fields are stored as `int16` with CF `scale_factor` and
`_FillValue` (`RR` carries no `_FillValue`, which is why the builder cannot
assume one), so the fixture exercises the unpacking seam alongside the geometry.

`tests/curvilinear_corpus.rs` reads both, and pins today's behaviour: the 2-D
coordinates are classifiable on their own attributes but never reach axis
detection, because that is offered only 1-D coordinate variables. #445 is what
changes it.

## `../fuzz_seeds/oom_large_fill_dataset.h5`

A byte-for-byte copy of the seed of the same name in this crate's fuzz corpus
(`fuzz/corpus/parse/`), read by `tests/whole_variable_budget.rs` (#847). It is
the 13 KB input on which the project's time-boxed CI fuzz run reported
out-of-memory: libFuzzer's mutation of a seed corpus drawn from this crate's
own test fixtures, whose second dataset declares a chunked 9,175,044 × 16 shape
of four-byte elements and stores no chunks. The fuzz directory is a separate
package, so `cargo package` leaves it out; the copy is what the published
crate's test reads (#926). It sits in `tests/fuzz_seeds/`, outside this
directory, so neither the `*.nc` and `*.h5` sweeps nor the napi display
golden's corpus walk treat a hostile seed as a fixture.

## Classic record-layout fixtures (`record_single_*.nc`, `record_mixed_cdf1.nc`)

Targets for the record stride of NetCDF classic (#204). The format
specification makes `recsize` the sum of the record variables' `vsize`, each
padded to 4 bytes, except that "when there is only one record variable and it
is of type character, byte, or short, no padding is used between record
slabs". It adds that `vsize` still includes the padding in that case, so
"readers should ignore vsize and assume no padding". The reader used to step by
`vsize`, and so failed on every file of that shape.

Self-generated by `tools/build_netcdf_record_layout_fixtures.py` (run from the
repo root; needs `netCDF4` + `numpy`), with netCDF4 1.7.4 over libnetcdf 4.9.3,
so there is **no upstream provenance or licensing constraint**. Each file has 3
records.

- `record_single_short_cdf1.nc`, `record_single_short_cdf2.nc`,
  `record_single_short_cdf5.nc`: one `short` record variable `s(time, x=3)`.
  `vsize` is 8 on disk and the records are 6 bytes apart, the last ending at
  the end of the file. One per version, because `vsize` and `begin` change
  width between them.
- `record_single_ubyte_cdf5.nc`: the same with a CDF-5 `ubyte`. The
  specification's sentence names `char`, `byte` and `short` only, the types
  narrower than 4 bytes when it was written, and says nothing of CDF-5's
  `ubyte` and `ushort`. libnetcdf packs them the same way, as this file shows,
  and the reader follows it. For every other type the question does not arise:
  a slab of 4- or 8-byte elements needs no padding.
- `record_mixed_cdf1.nc`: three record variables, `label` (`char`), `a`
  (`byte`) and `b` (`short`), plus a fixed `float` variable `f`. Every record
  slab is padded, so `recsize` is 12, and the `char` variable, which value
  decode rejects, still takes its place in each record.

The shared `record_layout.oracle.json` records the values `netCDF4` reads for
every numeric variable. `tests/classic_record_layout.rs` checks the decoded
values against it and the planned offsets against the specification.

## Large sparse fixture (`netcdf4_large_sparse.nc`)

Target for region reads (#939): a variable too large to decode whole, of which
a viewer must still draw one plane. `t2m(time = 120, lat = 721, lon = 1440)` is
float32, 124,588,800 values, about 2.5 GB decoded whole, chunked one time step
per chunk and deflated. Only time steps 7 and 119 are written, each row holding
its latitude rounded to a whole degree (step 119 adds 100), so the file is
40 KB; every other chunk is never allocated and reads as the `_FillValue`,
-999. The `time`, `lat` and `lon` coordinates are written in full.

Self-generated by `tools/build_netcdf_large_sparse_fixture.py` (run from the
repo root; needs `netCDF4` + `numpy`), with netCDF4 1.7.4 over libnetcdf 4.9.3
and HDF5 1.14.6, so there is **no upstream provenance or licensing
constraint**. `tests/region_reads.rs` checks plane, raw and time-series reads
against the values the script writes; the umbrella's
`tests/large_variable_plane.rs` draws a plane through `Session` and bounds the
memory it holds while doing so.

## Fill-only field-cap fixtures (`fill_only_past_field_cap.nc`, `fill_only_at_field_cap.nc`)

Targets for the one-field cap on a region read (#942). Each holds one chunked,
deflated float32 variable `t(y, x)` that stores no chunk, so every value reads
as the `_FillValue`, 1.5, and the file is 8 KB whatever its shape.
`fill_only_past_field_cap.nc` is 10000 × 10000: inside the 2 GiB
whole-variable budget, and its one plane is past `MAX_FIELD_POINTS` (64 Mi
values), so drawing it is refused. `fill_only_at_field_cap.nc` is 8192 × 8192,
exactly `MAX_FIELD_POINTS`, the largest plane a read may hold, and draws.

Self-generated by `tools/build_netcdf_fill_only_field_fixtures.py` (run from
the repo root; needs `netCDF4` + `numpy`), with netCDF4 1.7.4 over libnetcdf
4.9.3 and HDF5 1.14.6, so there is **no upstream provenance or licensing
constraint**. The umbrella's `tests/whole_variable_budget.rs` reads both
through `Session`, natively and under `wasm32-wasip1`.

## Hostile Fixed Array fixture (`hdf5_fixed_array_huge_count.h5`)

A Fixed Array chunk index whose entry count would size an allocation of
hundreds of gigabytes (#939 review). h5py (libver `latest`) writes a 5 × 7
float32 dataset `v` with 1 × 1 chunks; the dataspace is then patched to
131072 × 131072, and the `FAHD` header to `page_bits` 63 and 2^34 entries, which
equals the patched chunk grid. The object header's and the `FAHD`'s lookup3
checksums are recomputed, so nothing but the counts is wrong. The reader
refuses the chunk grid past `MAX_BTREE_NODES` chunks before it sizes anything
from it.

Self-generated by `tools/build_hdf5_fixed_array_count_fixture.py` (run from the
repo root; needs `h5py` + `numpy`), with h5py over HDF5 2.0.0, so there is
**no upstream provenance or licensing constraint**.
`tests/region_reads.rs::a_fixed_array_counting_more_chunks_than_an_index_may_hold_is_refused`
and the umbrella's `a_line_through_a_hostile_fixed_array_is_refused_not_allocated`
read it, the latter also under `wasm32-wasip1`.
