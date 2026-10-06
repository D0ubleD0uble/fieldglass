# 0013 — An HDF5 chunk index names each chunk once

**Status:** Accepted (2026-10-06). Answers #837; amended 2026-10-06 (#888).

## Context

A chunked HDF5 dataset finds its chunks through an index: a version-1 B-tree in
files written with the earliest file-format setting, or one of the version-4
indexes in newer ones. Each index entry gives a chunk's
element-space origin, its address, its stored size and its filter mask.

The reader used to read and inflate every entry it found. Nothing stopped two
entries from naming the same chunk, so a 24 KB file whose B-tree names one
16 MB gzip chunk sixteen times cost sixteen inflates of 16 MB: about 10.6 s on
the fuzz build, with up to a million entries allowed. The work was the number
of entries times the chunk size, which neither the per-variable element cap nor
the fuzz target's element gate sees. And `scatter_chunk` walked every element of
every chunk, so a chunk far larger than its dataset cost its full size even
though only one element landed.

The question is what a reader should do with an index that names a chunk more
than once, or names one off the chunk grid, or outside the dataset's shape.

### What the specification says

The HDF5 File Format Specification, "Version 1 B-trees": "The range of values
represented by child[i] is indicated by key[i] and key[i+1]", and "in chunk
trees (node type 1) the chunk described by key[i] is the least chunk in
child[i]". A chunk key's offsets are "the offset of the chunk within the
dataset". Two equal keys name an empty range, so an index that repeats an
origin is malformed. A version-2 B-tree record carries the chunk-grid
coordinate itself, and the fixed and extensible arrays derive the origin from
the entry's position, so neither of those can state an off-grid origin.

### What libhdf5 does

Measured with libhdf5 2.0.0 on hand-built files
(`tools/build_hdf5_duplicate_chunk_fixture.py` builds the committed ones):

| Index | libhdf5 reads | Fixture (`crates/fieldglass-netcdf/tests/fixtures/`) |
| --- | --- | --- |
| one chunk named 16 times, identically | the value written | `hdf5_duplicate_chunk_records.h5` |
| one chunk named at 16 different origins | the chunk at every origin | `hdf5_shared_chunk_records.h5` |
| `[7, 9]`; records A@0, B@0, B@1 | `[9, 9]` | `hdf5_conflicting_chunk_records.h5` |
| the same with A@0 and B@0 swapped | `[7, 9]` | `hdf5_conflicting_chunk_records_swapped.h5` |
| an origin not a multiple of the chunk edge | refuses ("bad coordinate offset") | `hdf5_off_grid_chunk_record.h5` |
| a record wholly outside the shape | ignores it | `hdf5_outside_chunk_record.h5` |

Each fixture's `.oracle.json` records libhdf5 2.0.0's read or its error, and
`tests/hdf5_chunk_records.rs` decodes each one.

For conflicting records its answer is the last record in index order within a
leaf, and across leaves whatever its key-guided search reaches. That answer
comes from how libhdf5 walks the tree. The file itself does not state it.

## Decision

The reader checks every index's records against the chunk grid before reading
any chunk (`chunks_in_shape` in `crates/fieldglass-netcdf/src/hdf5/values.rs`):

1. **Identical records** (same origin, address, size and filter mask) are read
   once. The value is not in doubt, and libhdf5 agrees.
2. **Records at one origin naming different storage are refused.** The value
   is ambiguous and the specification calls the index malformed. **This is a
   known divergence from libhdf5**, which reads an order-dependent value. The
   evidence is `tests/fixtures/hdf5_conflicting_chunk_records.h5` and
   `hdf5_conflicting_chunk_records_swapped.h5` (the same records in another
   order, read differently), their oracles (libhdf5's reads),
   `tests/hdf5_chunk_records.rs` and the fixtures' `NOTICE.md` entries.
3. **An origin inside the shape but off the chunk grid is refused**, as libhdf5
   refuses it.
4. **A record wholly outside the shape is skipped without being read**, as
   libhdf5 never looks it up, whether or not its origin is on the grid. A chunk
   larger than the shape is legal (an extendable dataset's, for example) and is
   not refused.
5. **Records at different origins naming the same storage** are legal, and
   libhdf5 reads them. The chunk is read and reversed once and placed at every
   origin that names it.

The check runs on every index, including the ones whose origins cannot repeat
by construction, because it is one sort over records that each cost a read.

Separately, `scatter_chunk` walks only the part of each chunk inside the shape,
one contiguous run along the last dimension at a time.

## Consequences

- The cost of a chunked decode is the distinct stored chunks times the chunk
  size, plus copying each in-shape origin's part once, rather than the number
  of index entries times the chunk size.
- A file libhdf5 opens can be refused: one whose index names two different
  chunks at one origin. No writer produces such a file, and the value libhdf5
  would show for it is an accident of record order.
- Rule 1 makes the B-tree walk's order irrelevant to the result. The walk visits
  internal nodes' children last-first, and nothing depends on that.

## Amendment (2026-10-06, #888)

Rule 5 grouped records by address, stored size and filter mask together. Two
fields of that key do not change the work. The filter mask's bits above the
pipeline's own filters are never read by the pipeline, and a gzip chunk's
stored size can run past the end of its zlib stream. So records naming one
chunk with masks `0, 2, 4, …` or sizes `s, s + 1, s + 2, …` each counted as a
different chunk and were each inflated: a 65 KB file of 1,024 such records
took 3.2 s, the cost #837 was meant to remove.

| Index | libhdf5 reads | Fixture |
| --- | --- | --- |
| one chunk at 16 origins, masks differing only above the pipeline's filters | the chunk at every origin | `hdf5_shared_chunk_records_masks.h5` |
| one chunk at 16 origins, stored sizes `s` to `s + 15` | the chunk at every origin | `hdf5_shared_chunk_records_sizes.h5` |

Rule 5 now reads:

5. **Records at different origins naming the same storage** are legal, and
   libhdf5 reads them. A stored chunk is its **address**. The filter mask is
   compared only over the filters the pipeline has. The chunk is read and
   reversed once and placed at every origin that names it.
6. **Records naming one address with different stored sizes, or with
   different masks over the pipeline's filters, are refused.** Each would
   decode the same bytes differently: a stored size moves fletcher32's
   checksum, a mask bit skips a filter. Reading each is the records-times-chunk
   cost again. **This is a second known divergence from libhdf5**, which reads
   `tests/fixtures/hdf5_shared_chunk_records_sizes.h5` as sixteen 7s; the
   fixture, its oracle and `tests/hdf5_chunk_records.rs` are the evidence. No
   writer shares storage between chunks at all.

The fractal heap had the same pattern: nothing stopped every direct-block
entry of an indirect block naming one filtered direct block, decoded once per
entry. A direct block's own Block Offset field is now checked against the heap
offset the doubling table names it at, as an indirect block's already was, so
a block matches at most one entry.

Rules 1 and 2 compare storage the same way: two records at one origin that
differ only in mask bits above the pipeline's filters are identical, not
conflicting.

With this, the cost of a chunked decode is the distinct stored addresses
times the chunk size, plus each in-shape origin's copy.
