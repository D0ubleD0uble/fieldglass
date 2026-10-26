# 0016 — An HDF5 listing reads each block for one object

**Status:** Accepted (2026-10-11). Answers #921. Widens decision 1 of
[ADR-0014](0014-hdf5-group-names-are-held-to-their-heap.md) from one group to
the whole listing. Decision 5 added for #997, and decision 2's B-tree rows split by role for #1005 (2026-10-11).

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
   block an earlier store claim of the same kind holds. A B-tree v2 header's
   kind is the role it is read in (`heap::BTreeRole`: a heap's huge-object
   index, an attribute index, a link index), and its type is checked against
   that role before the claim (#1005). libhdf5 checks a v2 B-tree's type when
   it opens one, so one tree named in two roles is corrupt, not shared. A
   fractal heap has no type and libhdf5 opens it for any role, so its header's
   kind is the same in every role. The claims are classed so:

   | Claim | Class |
   | --- | --- |
   | fractal-heap header claimed again, in any role | `SharedStorage`, "dense attribute or link store" |
   | B-tree v2 header (a dense attribute or link index, or a heap's huge-object index) claimed again in the same role | `SharedStorage`, "dense attribute or link store" |
   | B-tree v2 header claimed again in another role, or whose type its role cannot read | `Parse` |
   | local heap header or data segment claimed again | `SharedStorage`, "local heap" |
   | object-header chunk, B-tree v1 node, `SNOD`, group member name, B-tree v2 node, huge or managed heap object | `Parse`: the specification or libhdf5 treats a second name for these as corrupt (ADR-0014) |
   | a store claim that overlaps anything but an identical earlier claim of the same kind (a local heap header laid on another heap's data segment is corrupt) | `Parse` |
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

5. **One metadata structure is read in at most 16 MiB** (#997). Claims stop a
   block being read twice; they do not bound one read. An object header's
   chunks, a fractal-heap object and a global-heap object are each read whole
   at a length the file states, and over a source read by range (#950) that
   length is bounded only by the size the host declares, so one field could
   ask for gigabytes. Each is now checked before the read against
   `MAX_METADATA_BYTES` (16 MiB, the classic header's `MAX_HEADER_BYTES`), in
   `crates/fieldglass-netcdf/src/hdf5/source.rs`:

   | Structure | Counted | Largest in the corpus | Why 16 MiB leaves room |
   | --- | --- | --- | --- |
   | object header | every chunk, prefix included: the v1 header size, the v2 chunk-0 size, each continuation | 2,163 bytes (a v2 chunk 0 in `samples/oisst.nc`); 1,792 in a v1 header | libhdf5 keeps a message under 64 KiB (`H5O_MESG_MAX_SIZE`) and sizes a new chunk to the message it adds (`H5O__alloc_new_chunk`); 16 MiB is 256 maximal messages |
   | fractal-heap object | a managed object, and a huge object's stored and decoded length | 70,025 bytes (`hdf5_huge_only_heaps.h5`); 5,629 from netCDF-C | in a NetCDF-4 file each is one dense attribute or link; a classic file's whole header is held to the same 16 MiB |
   | global-heap object | the one object a lookup reads; objects scanned past are skipped, not read | 8 bytes: every `DIMENSION_LIST` entry in the fixtures and `samples/` | libhdf5 sizes a collection to the object it is made for (`H5HG_insert`), so the format sets no ceiling |

   The largest values were measured by opening every committed NetCDF-4 and
   HDF5 fixture and the five NetCDF-4 files `tools/fetch_samples.sh` puts in
   `samples/`. A filtered fractal-heap direct block keeps its 64 MiB ceiling,
   now on its stored size as well as its decoded one. A header, huge-object
   or global-heap length is charged before it is narrowed to `usize`, so the
   refusal is the same on a 32-bit target.

   Every length is checked to end inside the file before it meets a cap
   (#1017): a header chunk, a heap object or block, a filtered chunk. Over a
   buffer the end of the file is the buffer's, so a length past it is a
   parse error, a corrupt file, and only a well-formed structure in the file
   can be refused as too large. Over a source read by range the end of the
   file is the size the host declares, and a well-formed oversized length
   still meets the cap.

   Where a filter pipeline stands between the stored bytes and a capped
   decoded size, the stored size may pass the cap by what the filters that
   ran can add to an object of the decoded size, and never by more than the
   cap again (#1016, `FilterPipeline::stored_limit`): fletcher32 adds its
   4-byte checksum, deflate at most zlib's `deflateBound` for any parameters
   (about an eighth, RFC 1951 §3.2.6; zlib's stored blocks on data that does
   not compress add 5 bytes per 65,535, §3.2.4), zstd `ZSTD_COMPRESSBOUND`
   (about 1/256), and szip what its parameters allow. This holds for a
   filtered chunk, a filtered heap block and a filtered huge heap object,
   and for what a codec may decode to behind those filters (ADR-0012
   decision 6), so a 256 MiB chunk under fletcher32 and deflate, the order
   netCDF-C writes, decodes as well as being read. Twice the cap holds every
   file libhdf5 writes: its szip never picks a scanline shorter than a block,
   which keeps a stream under twice its input, and the filter is optional, so
   a chunk szip would grow is stored unfiltered. Only an szip from a writer
   outside libhdf5's limits, up to 33 times, can pass it, and is refused as
   too large.

   libhdf5 lets a heap declare direct blocks of up to 2 GiB
   (`H5HF_MAX_DIRECT_SIZE_LIMIT`), and nothing in the format bounds a header
   chunk or a global-heap object, so a valid file can pass each cap. What
   passing one costs depends on what the structure is:

   - **One dense attribute or link** (a fractal-heap object) is skipped, and
     the file kept: the listing goes on without it, as it goes on without an
     attribute it cannot parse. One oversized attribute would otherwise make
     the whole file unreadable, even from a buffer in memory. Only a
     well-formed object is skipped: it is checked to lie in the file and
     claimed before its size is, so one past the end of the file, or a
     second record naming one already claimed, is still a parse error, as
     before. A `DIMENSION_LIST` skipped this way leaves its variable on
     anonymous `phony_dim_N` axes, as a variable with no `DIMENSION_LIST`
     has. Nothing reports the skip: the attributes `parse_bodies` drops are
     not reported either, and a host has no channel for either yet.
   - **Structure the listing stands on** is refused as
     `ArrayError::ListingTooLarge`, as in decision 4: an object header, which
     holds a dataset's layout and its compact attributes together; a
     global-heap object, the `DIMENSION_LIST` references a lookup resolves;
     and a filtered heap block, which holds many objects. The filtered block
     past 64 MiB was a parse error before.

   Two data-chunk reads are sized by fields the file states, and both are now
   checked before the chunk is planned or read. An unfiltered chunk is held to
   its own chunk's size (a parse error, refused after the read before). A
   filtered chunk's stored size, up to 4 GiB in a B-tree key, is held to the
   256 MiB decoded-chunk ceiling of #970 grown by its pipeline as above, as
   `ArrayError::StoredChunkTooLarge`. A chunk at the ceiling is stored past
   it when a filter grows it, as fletcher32 always does, so the ceiling alone
   refused valid files libhdf5 reads (#1016). A contiguous dataset's region
   is checked to end inside the file before its run list and output buffer
   are built: both are bounded only by the field cap, so a 2.7 KB file
   declaring 8,192 × 8,191 float32 values held 268 MB before its first read
   found the data missing (`crates/fieldglass/tests/contiguous_past_the_file.rs`,
   through the reader and `Session`; it now holds under 4 MiB).

The evidence is the fixtures above, their `NOTICE.md` entry and
`crates/fieldglass-netcdf/tests/hdf5_shared_blocks.rs`, which checks each
refusal, how often the shared block is read and the listing's peak heap, that
the hard-linked `hdf5_hard_linked_dense_attributes.h5` lists both names, and
that `hdf5_deep_group_chain_past_name_cap.h5` is refused as too large rather
than corrupt (also through `Session` in
`crates/fieldglass/tests/hdf5_listing_cap.rs`).

For decision 5, `declared_tests` in `crates/fieldglass-netcdf/src/hdf5.rs` rewrites
one length field of a committed fixture to claim gigabytes (a v1 header size, a
v1 and a v2 continuation, a v2 chunk-0 size, a global-heap object, an
unfiltered and a filtered chunk's B-tree key) and opens it through
`NetcdfReader::from_source` over `fieldglass_core::testing::Declared`, a source
that holds the file and declares a terabyte. Each is refused with no read
longer than 64 KiB; before the change each asked for the gigabytes in one read.
The same tests rewrite huge-object index records: `netcdf4_huge_attributes.nc`
still lists every variable, drops only its two oversized attributes, and
decodes; with `t`'s `DIMENSION_LIST` record pointed at one of them and the
`comment` record taken out of the index, `t` falls back to `phony_dim_0`;
`hdf5_huge_link_name.h5` resolves without its oversized link. Before the
change each file was refused. With the `comment` record left in, two records
name one object and the listing is a parse error; an oversized object past
the end of a buffer is a parse error too, not skipped. The managed-object
and filtered-block caps are checked in `heap.rs`. Opened from a buffer, each
rewritten header and chunk length runs past the end of the file and is a
parse error (#1017). A `float64` chunk cut to exactly 256 MiB and stored 4
bytes past it under fletcher32, or 20,491 bytes past it under deflate on data
that does not compress, reaches the read; a byte past what its filter can add
is too large (#1016).

## Consequences

- A file read by range costs no more to refuse than its fields take to read:
  no header, heap object or global-heap object is fetched past 16 MiB, however
  large the host says the file is.

- A listing's cost is bounded by the file's own storage: each shared shape is
  refused after reading the block at most twice (the second header reads a
  shared chunk before it is claimed), in under 3 ms and 5 MB at four times the
  objects. The 600-group chain lists in 1.5 ms at 3.8 MB.
- A file libhdf5 reads can be refused: one whose objects share a dense
  attribute or link store, or whose groups share a local heap. It is
  reported as unsupported, not corrupt.
- Opening a NetCDF-4 file costs a few more allocations for the claim sets and
  the memoized chunk ranges (+9 allocations, about 2 KB and 3% instructions on
  the perf corpus's `open` rows).
