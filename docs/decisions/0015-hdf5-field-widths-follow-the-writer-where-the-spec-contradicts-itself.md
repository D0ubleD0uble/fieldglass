# 0015 — HDF5 field widths follow libhdf5 where the specification contradicts itself

**Status:** Accepted (2026-10-09). Answers #922.

## Context

An HDF5 superblock states Size of Offsets (O, the width of a file address) and
Size of Lengths (L, the width of a size or a heap offset) separately. Almost
every file uses 8 for both, so a field read at the wrong one of the two reads
correctly anyway. libhdf5 writes other sizes on request (`H5Pset_sizes`), and
an earliest-format file with O ≠ L failed to open: with (8, 4) the reader took
the root group's header address from four bytes too early.

Every field read on the earliest-format path, and every field elsewhere that
depends on O or L, was checked against the HDF5 File Format Specification
(version 3) and libhdf5 1.14.4's decoders. A group that tracks creation order
uses a link info message even in an earliest-format file, so that message is
on this path too. The two sources agree on all but two fields:

| Field | Specification | libhdf5 | Reader before | Reader now |
| --- | --- | --- | --- | --- |
| Symbol-table entry: link-name offset (superblock root entry and every `SNOD` entry) | O | L (`H5G_ent_decode`) | O | **L** |
| Group B-tree node key | L | L (`H5G_node_decode_key`) | O | L |
| Global heap: collection header and each object header | "16 bytes" (stated for L = 8) | `ALIGN8(8 + L)` (`H5HG_SIZEOF_HDR`, `H5HG_SIZEOF_OBJHDR`) | `8 + L` | **`ALIGN8(8 + L)`** |
| Object-header continuation: undefined address | all ones at O | all ones at O | 8-byte all ones only | all ones at O |
| Link info message: maximum creation index | 8 | 8 (`INT64DECODE`) | L | 8 |

The fields that were already right: the superblock's four addresses (O); the
symbol-table message's B-tree and heap addresses (O); the local heap's data
size and free-list offset (L) and data address (O); B-tree sibling and child
addresses (O); a chunk B-tree key (4 + 4 + 8 per dimension, fixed); the
continuation's address (O) and length (L); dataspace sizes and maxima (L); the
contiguous layout's address (O) and size (L) and the chunked layout's index
address (O); a global heap's collection and object sizes (L); a global heap ID
(4 + O + 4) and an object reference (O); the attribute info message's
maximum creation index (2 bytes); the fractal heap header, its indirect-block
and huge-object records, version-2 B-tree headers and records, and the
version-4 layout's chunk indexes. Data layout messages before version 3 are
refused as unsupported, as before.

The link-name offset is the one field where the two sources disagree outright.
It is an offset into the group's local heap. Every other local-heap offset in
the specification is L: the group B-tree key that indexes the same names, the
heap's own free-list offset and its data size. libhdf5 is the format's
reference writer, and every file with O ≠ L has the field at L.

The global heap is a gap rather than a contradiction. The specification sizes
both headers as 16 bytes, true only when L = 8, and says the object header's
reserved field aligns the next field on an 8-byte boundary.

## Decision

1. **Read the symbol-table entry's link-name offset at Size of Lengths.** This
   departs from the specification's layout table. It is a known divergence
   from the specification, not from libhdf5. The project's rule is to follow the
   specification where a reference library disagrees with it. That rule
   assumes the specification is consistent. Here its table contradicts its
   own treatment of every other heap offset, and following it would refuse
   every earliest-format file whose sizes differ, all of them written by
   libhdf5.
2. **Pad each global heap header to a multiple of eight,** as libhdf5 does.
   This fills a gap the specification leaves for L ≠ 8. It does not contradict
   the specification.
3. **Treat a dataspace maximum of all ones at Size of Lengths as unlimited.**
   This was already the reader's rule; the fixtures show that libhdf5 does not
   agree. With L = 4, libhdf5 writes `H5S_UNLIMITED` as `0xFFFF_FFFF` and
   reads it back as 4294967295, because it compares the decoded value with a
   64-bit all-ones constant. This is a known divergence from libhdf5. The
   specification gives the unlimited value as all bits set, and the writer
   meant unlimited.

The evidence is `tools/build_hdf5_size_fixtures.py`, the fixtures
`hdf5_sizes_o8_l4.h5` and `hdf5_sizes_o4_l8.h5` with their h5py oracles,
`tests/hdf5_offset_length_sizes.rs`, the unit tests
`root_entry_name_offset_is_length_sized` (superblock versions 0 and 1) and
`continuation_to_an_undefined_address_is_refused_at_any_offset_size`, and the
fixtures' `NOTICE.md` entry.

## Consequences

- Files with O ≠ L open, list, resolve their dimension scales and decode,
  including groups that track creation order in any file format. Files with
  O = L read the same as before.
- A file written to the specification's table rather than by libhdf5, with
  O ≠ L, would be misread. No such writer is known.
- When another field turns out to differ between the specification and
  libhdf5, check first whether the specification is consistent with itself.
  Only then apply the follow-the-specification rule.
