# 0016 — An HDF5 listing reads each block for one object

**Status:** Accepted (2026-10-11). Answers #921. Widens decision 1 of
[ADR-0014](0014-hdf5-group-names-are-held-to-their-heap.md) from one group to
the whole listing.

## Context

ADR-0014 (#901), with #895 and #899, claims each block one structure reads (an
object header's chunks, one group's B-tree nodes, `SNOD`s and names, one
heap's objects) and refuses a second claim. Those claims did not span the
objects of a listing. A block that many objects named was read and copied once
per object, so the cost grew with the number of objects times the block:

| Shape (fixture in `crates/fieldglass-netcdf/tests/fixtures/`) | Before, release build |
| --- | --- |
| 256 headers continuing into one 256 KiB chunk (`hdf5_shared_continuation.h5`) | listed at 67 MB, the header memo full |
| 256 datasets whose Attribute Info names one dense store of 64 × 3 KB (`hdf5_shared_attribute_storage.h5`) | resolved at 103 MB; 403 MB at 4× the objects |
| 256 groups whose Link Info names one dense store of 64 links (`hdf5_shared_link_storage.h5`) | 16,449 objects listed at 36 MB |
| 128 groups sharing the root's local heap, each naming its 60,000-character name (`hdf5_shared_local_heap.h5`) | resolved at 25 MB; 92 MB at 4× |

A deep chain of nested groups had the same shape in its own memory: each level
held a copy of the path above it, O(depth²) bytes (600 groups of 200-character
names, 62 MB; 2,400 groups, 590 MB).

### What the specification and libhdf5 say

The specification implies one owner for each of these blocks without
forbidding a second. The Attribute Info and Link Info messages give the
address of "the fractal heap" storing the object's dense attributes or links,
and a group's link names are in "the group's local heap" (HDF5 File Format
Specification version 3, "Attribute Info Message", "Link Info Message",
"Local Heaps"). An object header's continuation chunks belong to that header.
No writer shares any of them.

Measured with libhdf5 2.0.0 (h5py 3.16.0), the builder being
`tools/build_hdf5_shared_block_fixtures.py`:

| Fixture | libhdf5 |
| --- | --- |
| `hdf5_shared_continuation.h5` | opens the first dataset, refuses the second ("incorrect number of chunks after deserializing object header chunk") |
| `hdf5_shared_attribute_storage.h5` | lists the 64 shared attributes on every dataset |
| `hdf5_shared_link_storage.h5` | lists the 64 shared members of every group |
| `hdf5_shared_local_heap.h5` | lists every group's one member |

## Decision

1. **A whole-file listing claims every block it reads, across all its objects,
   and refuses a second claim** (`Listing` in
   `crates/fieldglass-netcdf/src/hdf5.rs`, one `ClaimedRanges` of file ranges).
   The group walk claims object-header chunks (chunk 0 and each continuation,
   v1 and v2), each group's local heap and its data segment, group B-tree v1
   nodes and `SNOD`s, and for dense links the fractal-heap header, the B-tree
   v2 header and nodes and any huge objects. Metadata resolution claims every
   object's dense attribute storage the same way. One claim set spans all
   these structure families, so a block of one family overlapping another's
   is refused too. Fractal-heap direct and indirect blocks need no claim:
   each carries a back-pointer to its one heap (ADR-0013's amendment, #888).
2. **Refusing shared dense attribute and link stores and shared local heaps
   is a known divergence from libhdf5**, which reads them. Unlike ADR-0014's
   shared name, these files break no rule the specification states; this is a
   robustness policy, because reading the shared block once per object is what
   makes a small file expensive and no writer produces one. The shared
   continuation chunk is **not** a divergence: libhdf5 refuses it too.

   Because the file is valid, the refusal is **not** a parse error (#987). It
   is `ArrayError::SharedStorage`, which says the file is fine and reaches a
   host as `unsupported`, as decision 4 does for a listing past its cap.
   `ClaimedRanges::claim_store` raises it when a store claim repeats the exact
   block an earlier store claim holds. The claims are classed so:

   | Claim | Class |
   | --- | --- |
   | fractal-heap header, B-tree v2 header (a dense attribute or link store) claimed again | `SharedStorage`, "dense attribute or link storage" |
   | local heap header or data segment claimed again | `SharedStorage`, "local heap" |
   | object-header chunk, B-tree v1 node, `SNOD`, group member name, B-tree v2 node, huge or managed heap object | `Parse`: the specification or libhdf5 treats a second name for these as corrupt (ADR-0014) |
   | a store claim that overlaps anything but an identical earlier store claim | `Parse` |
3. **Exempt:** an object reached by two hard links is one object. Its header
   chunks are claimed on its first visit, and resolution describes it once and
   lists it under each name. A global-heap collection is legitimately shared
   (netCDF-C puts many variables' `DIMENSION_LIST` references in one); it is
   not claimed, and is scanned once per resolution instead, answering as a
   fresh scan would.
4. **A listing's own size is a policy cap, reported as such.** The group walk
   keeps one path buffer, so a chain costs its path once. The listing holds at
   most 1,048,576 objects (or links in one group) and 64 MiB of path-qualified
   names. A valid file can pass either, so passing one is
   `ArrayError::ListingTooLarge`, which says the file is fine and reaches a
   host as `unsupported`, as #847, #942 and #970 report a valid file asked for
   too much. `MAX_BTREE_NODES` (4,096 group B-tree node visits) is left a parse
   error here: it predates this decision as the guard on a cyclic tree, and
   whether a valid group can reach it is a separate question.

The evidence is the fixtures above, their `NOTICE.md` entry and
`crates/fieldglass-netcdf/tests/hdf5_shared_blocks.rs`, which checks each
refusal, how often the shared block is read and the listing's peak heap, that
the hard-linked `hdf5_hard_linked_dense_attributes.h5` lists both names, and
that `hdf5_deep_group_chain_past_name_cap.h5` is refused as too large rather
than corrupt (also through `Session` in
`crates/fieldglass/tests/hdf5_listing_cap.rs`).

## Consequences

- A listing's cost is bounded by the file's own storage: each shared shape is
  refused after reading the block at most twice (the second header reads a
  shared chunk before it is claimed), in under 3 ms and 5 MB at four times the
  objects. The 600-group chain lists in 1.5 ms at 3.8 MB.
- A file libhdf5 reads can be refused: one whose objects share a dense
  attribute or link store, or whose groups share a local heap. It is
  reported as unsupported, not corrupt.
- Opening a NetCDF-4 file costs a few more allocations for the claim sets and
  the memoised chunk ranges (+9 allocations, about 2 KB and 3% instructions on
  the perf corpus's `open` rows).
