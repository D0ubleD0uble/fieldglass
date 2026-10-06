# 0014 — An HDF5 symbol-table group's nodes and names are read once, inside their heap

**Status:** Accepted (2026-10-06). Answers #901 and #908.

## Context

A group written in HDF5's earliest file format lists its members through a
version-1 B-tree whose leaves name symbol-table nodes (`SNOD`). Each `SNOD`
entry names a member by an offset into the group's local heap, whose header
states the heap's data segment address and size.

The reader followed every reference: a `SNOD` named from 8,192 leaf entries,
its entries all naming one 60 KB name, made a 197 KB file use 3.85 GB (#901).
And it read the data segment's address but threw the size away, so a name
offset past the segment's end read whatever bytes followed (#908).

### What the specification and libhdf5 say

The HDF5 File Format Specification ("Local Heap") defines a link name as a
string at an offset into the heap's data segment, of the stated size. A
group B-tree node has one parent and an `SNOD` one referring entry; names are
unique in a group.

Measured with libhdf5 2.0.0 (h5py 3.16.0) on hand-built files:

| File | libhdf5 | Fixture (`crates/fieldglass-netcdf/tests/fixtures/`) |
| --- | --- | --- |
| two `SNOD` entries naming one name | lists both | `hdf5_shared_group_name.h5` |
| a B-tree leaf naming one `SNOD` twice | refuses (reads the node at its full allocated size) | `hdf5_shared_snod.h5` |
| a name offset at the data segment's end | refuses ("unable to offset into local heap data block") | `hdf5_local_heap_short.h5` |
| a name whose terminator falls past the segment's end | lists it | `hdf5_local_heap_unterminated.h5` |

## Decision

`symbol_table_links` (`crates/fieldglass-netcdf/src/hdf5/group.rs`):

1. **Claims each group B-tree node and each `SNOD` by its range of the file,
   and each member name by its range of the heap** (`ClaimedRanges`), and
   refuses a second claim. A valid group gives each its own storage
   (libhdf5 allocates nodes at full capacity and pads names to 8 bytes).
   Refusing two entries that share a name is a divergence from libhdf5,
   which lists both; a name is unique in a group, so the index is malformed.
2. **Holds every name to the heap's data segment.** An offset at or past the
   segment's size is refused before anything is read, as libhdf5 refuses it.
   The name scan reads no further than the segment's end, so a name whose
   terminator falls past it is refused. **This is a known divergence from
   libhdf5**, which lists that name; the specification puts the whole string
   inside the segment.

The evidence for both divergences is each fixture above, its oracle or the
builder that records libhdf5's outcome (`tools/build_hdf5_shared_snod_fixture.py`,
`tools/build_hdf5_local_heap_fixture.py`), `tests/hdf5_shared_snod.rs`,
`tests/hdf5_local_heap.rs`, and the fixtures' `NOTICE.md` entries.

## Consequences

- Listing a symbol-table group reads each node and name once, and no name
  from outside its heap, so the cost is bounded by the group's own storage.
- A file libhdf5 lists can be refused: one whose group shares a name between
  two members, or names a member with a string running out of its heap. No
  writer produces either.
