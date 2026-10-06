#!/usr/bin/env python3
"""Build the four HDF5 fixtures behind #837: an oversized chunk, a chunk
B-tree that names that chunk sixteen times, one that names it at sixteen
different origins, and one that names two different chunks at one origin.

  * ``hdf5_oversized_chunk.h5`` — written by ``h5py`` with ``libver='earliest'``
    (superblock v0, a **version-1 chunk B-tree**): a dataset ``v`` of shape
    ``(1,)``, ``uint8``, holding ``7``, with one gzip chunk of 16 Mi elements.
    An extendable dataset (``maxshape=(None,)``) may legally have a chunk far
    larger than its shape. The reader must place the one element without
    walking all 16 Mi of the chunk. Its oracle is h5py's own read.
  * ``hdf5_duplicate_chunk_records.h5`` — the same file with a hand-built chunk
    index appended: a level-0 B-tree v1 leaf of ``K = 4`` entries, each naming
    the one stored chunk at origin 0, under a level-1 root whose ``R = 4``
    children all point at that leaf, with the layout message repointed at the
    root. That is 16 chunk records for one chunk. A v1 B-tree's keys are
    strictly ordered, so the index is malformed. The copies are identical, so
    the value is not in doubt: libhdf5 reads ``[7]``, and so does the reader,
    which reads the chunk once instead of inflating 16 MB sixteen times (about
    10.6 s on the fuzz build before the fix).
  * ``hdf5_shared_chunk_records.h5`` — shape ``(16, 1)`` ``uint8`` in gzip
    chunks of ``(1, 16 Mi)`` with only ``[0, 0] = 7`` written, so one chunk is
    stored. A hand-built leaf then names that chunk at all sixteen origins
    ``(i, 0)``. Nothing in the format forbids two origins sharing storage, and
    libhdf5 reads sixteen 7s. The reader does too, inflating the chunk once
    rather than once per origin.
  * ``hdf5_shared_chunk_records_masks.h5`` and
    ``hdf5_shared_chunk_records_sizes.h5`` — the same, with record ``i``'s
    filter mask raised by ``2 * i`` (bits above the pipeline's one filter, which
    decode identically) or its stored size by ``i`` bytes (past the end of the
    zlib stream). libhdf5 reads sixteen 7s from both. The reader reads the
    first with the chunk inflated once, and refuses the second: one stored
    chunk with sixteen stated sizes is a malformed index (#888, ADR-0013).
  * ``hdf5_conflicting_chunk_records.h5`` — ``(2,)`` ``uint8`` ``[7, 9]`` in
    unfiltered chunks of one element, with a leaf naming chunk A at origin 0,
    then chunk B at origin 0, then chunk B at origin 1. Which value origin 0
    holds is ambiguous. libhdf5 2.0.0 reads ``[9, 9]``, the last record, and
    swapping the first two records makes it read ``[7, 9]``; the reader refuses
    the index instead (ADR-0013). The oracle records what libhdf5 read.
  * ``hdf5_conflicting_chunk_records_swapped.h5`` — the same with the first
    two records swapped (B at origin 0, then A at origin 0, then B at 1).
    libhdf5 reads ``[7, 9]``, so its answer depends on record order alone.
  * ``hdf5_off_grid_chunk_record.h5`` — ``(4,)`` ``uint8`` ``[1, 2, 3, 4]`` in
    chunks of two, with records at origins 0 and 1. Origin 1 is inside the
    shape but not a multiple of the chunk edge; libhdf5 refuses the read, and
    so does the reader. The oracle records libhdf5's error.
  * ``hdf5_outside_chunk_record.h5`` — ``[7, 9]`` in chunks of one, with a
    third record at origin 5, wholly outside the shape, whose address is past
    the end of the file. libhdf5 reads ``[7, 9]``, never looking it up, and
    the reader skips it unread: reading it would fail.

Both files are well under the NetCDF fuzz corpus's ``max_len``; the second is
copied into ``crates/fieldglass-netcdf/fuzz/corpus/parse/`` as a seed.

Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_duplicate_chunk_fixture.py
"""
from __future__ import annotations

import json
import struct
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
CORPUS = Path("crates/fieldglass-netcdf/fuzz/corpus/parse")
CHUNK = 16 * 1024 * 1024
K = 4  # entries in the appended leaf
R = 4  # children of the appended root, all the same leaf

# Superblock v0: signature (8), four version bytes, offset and length sizes,
# a reserved byte (8 in all), group K values (4), flags (4), base address (8),
# free-space address (8), then the end-of-file address.
EOF_ADDRESS_AT = 40


def build_oversized(path: Path) -> None:
    with h5py.File(path, "w", libver="earliest") as f:
        f.create_dataset(
            "v",
            data=np.array([7], dtype="u1"),
            maxshape=(None,),
            chunks=(CHUNK,),
            compression="gzip",
            track_times=False,
        )


def chunk_btree_address(raw: bytes) -> int:
    """The one chunk B-tree (signature TREE, node type 1) in the file."""
    found = [
        i for i in range(len(raw) - 5) if raw[i : i + 4] == b"TREE" and raw[i + 4] == 1
    ]
    assert len(found) == 1, f"expected one chunk B-tree, found {found}"
    return found[0]


def key(size: int, mask: int, *origin: int) -> bytes:
    # A chunk key: chunk size, filter mask, then rank + 1 offsets (the last is
    # the element-byte offset, always 0).
    return struct.pack(f"<II{len(origin) + 1}Q", size, mask, *origin, 0)


def node(level: int, entries: list[tuple[bytes, int]], last_key: bytes) -> bytes:
    # libhdf5 reads a v1 chunk B-tree node at its full allocated size, room for
    # 2K entries with the default K of 32: a 24-byte header, 64 children and 65
    # keys.
    node_bytes = 24 + 64 * 8 + 65 * len(last_key)
    undefined = 0xFFFF_FFFF_FFFF_FFFF
    out = b"TREE" + struct.pack("<BBHQQ", 1, level, len(entries), undefined, undefined)
    for k, child in entries:
        out += k + struct.pack("<Q", child)
    out += last_key
    return out + bytes(node_bytes - len(out))


def repoint(raw: bytearray, rank: int, old_root: int, root: int) -> None:
    """Point the Data Layout message (v3, chunked: version 3, class 2,
    dimensionality rank + 1, then the chunk B-tree address) at `root`."""
    pattern = bytes([3, 2, rank + 1]) + struct.pack("<Q", old_root)
    at = raw.find(pattern)
    assert at > 0 and raw.find(pattern, at + 1) == -1, "layout message not unique"
    raw[at + 3 : at + 11] = struct.pack("<Q", root)


def append(raw: bytearray, block: bytes) -> int:
    while len(raw) % 8:
        raw.append(0)
    at = len(raw)
    raw += block
    return at


def finish(raw: bytearray, path: Path) -> None:
    assert struct.unpack_from("<Q", raw, EOF_ADDRESS_AT)[0] <= len(raw), "EOF field"
    struct.pack_into("<Q", raw, EOF_ADDRESS_AT, len(raw))
    path.write_bytes(bytes(raw))


def build_duplicated(src: Path, dst: Path) -> None:
    raw = bytearray(src.read_bytes())
    assert struct.unpack_from("<Q", raw, EOF_ADDRESS_AT)[0] == len(raw), "EOF field"
    with h5py.File(src, "r") as f:
        info = f["v"].id.get_chunk_info(0)
        chunk_addr, chunk_size, mask = info.byte_offset, info.size, info.filter_mask
    old_root = chunk_btree_address(raw)
    record = key(chunk_size, mask, 0)
    leaf = append(raw, node(0, [(record, chunk_addr)] * K, key(0, 0, CHUNK)))
    root = append(raw, node(1, [(key(0, 0, 0), leaf)] * R, key(0, 0, CHUNK)))
    repoint(raw, 1, old_root, root)
    finish(raw, dst)


def build_shared(path: Path, vary: str = "") -> list:
    """The shared-storage file; ``vary`` makes record ``i`` differ from the
    stored chunk's own key by ``2 * i`` in the filter mask (bits above the one
    gzip filter, ``"mask"``) or by ``i`` bytes of stored size (``"size"``)."""
    src = path.with_suffix(".src.h5")
    with h5py.File(src, "w", libver="earliest") as f:
        v = f.create_dataset(
            "v",
            shape=(16, 1),
            dtype="u1",
            maxshape=(None, None),
            chunks=(1, CHUNK),
            compression="gzip",
            track_times=False,
        )
        v[0, 0] = 7
    with h5py.File(src, "r") as f:
        assert f["v"].id.get_num_chunks() == 1
        info = f["v"].id.get_chunk_info(0)
    raw = bytearray(src.read_bytes())
    src.unlink()
    old_root = chunk_btree_address(raw)
    def record(i: int) -> bytes:
        size = info.size + (i if vary == "size" else 0)
        mask = info.filter_mask | (2 * i if vary == "mask" else 0)
        return key(size, mask, i, 0)

    entries = [(record(i), info.byte_offset) for i in range(16)]
    leaf = append(raw, node(0, entries, key(0, 0, 16, 0)))
    repoint(raw, 2, old_root, leaf)
    finish(raw, path)
    with h5py.File(path, "r") as f:
        return f["v"][...].ravel().tolist()


def hand_index(
    path: Path, data: list[int], chunk: int, records, last: int
) -> "list | str":
    """A rank-1 ``uint8`` dataset ``data`` in unfiltered chunks of ``chunk``
    elements, with its chunk index replaced by one hand-built leaf.
    ``records(addresses)`` gives the leaf's ``(origin, address)`` entries from
    the stored chunks' addresses. Returns libhdf5's read, or its error text."""
    src = path.with_suffix(".src.h5")
    with h5py.File(src, "w", libver="earliest") as f:
        f.create_dataset(
            "v",
            data=np.array(data, dtype="u1"),
            maxshape=(None,),
            chunks=(chunk,),
            track_times=False,
        )
    with h5py.File(src, "r") as f:
        stored = [f["v"].id.get_chunk_info(i).byte_offset for i in range(f["v"].id.get_num_chunks())]
    raw = bytearray(src.read_bytes())
    src.unlink()
    old_root = chunk_btree_address(raw)
    entries = [(key(chunk, 0, origin), addr) for origin, addr in records(stored, len(raw))]
    leaf = append(raw, node(0, entries, key(0, 0, last)))
    repoint(raw, 1, old_root, leaf)
    finish(raw, path)
    try:
        with h5py.File(path, "r") as f:
            return f["v"][...].tolist()
    except OSError as e:
        return str(e).splitlines()[0]


def build_conflicting(path: Path, swapped: bool = False) -> list:
    def records(s, _end):
        a, b = s
        first = [(0, b), (0, a)] if swapped else [(0, a), (0, b)]
        return first + [(1, b)]

    return hand_index(path, [7, 9], 1, records, 2)


def write_oracle(path: Path, note: str, libhdf5) -> None:
    field = "libhdf5_values" if isinstance(libhdf5, list) else "libhdf5_error"
    (FIXTURES / f"{path.name}.oracle.json").write_text(
        json.dumps(
            {
                "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
                "libver='earliest', chunk B-tree leaf appended by hand",
                "note": note,
                field: libhdf5,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {path} ({path.stat().st_size} B); libhdf5: {libhdf5}")


def main() -> None:
    oversized = FIXTURES / "hdf5_oversized_chunk.h5"
    duplicated = FIXTURES / "hdf5_duplicate_chunk_records.h5"
    build_oversized(oversized)
    build_duplicated(oversized, duplicated)
    (CORPUS / duplicated.name).write_bytes(duplicated.read_bytes())

    with h5py.File(oversized, "r") as f:
        v = f["v"]
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
            "libver='earliest'",
            "note": "one element in one gzip chunk of 16 Mi elements, v1 chunk B-tree (#837)",
            "shape": list(v.shape),
            "chunks": list(v.chunks),
            "dtype": str(v.dtype),
            "values": v[...].tolist(),
        }
    (FIXTURES / f"{oversized.name}.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {oversized} ({oversized.stat().st_size} B) + oracle")

    shared = FIXTURES / "hdf5_shared_chunk_records.h5"
    shared_read = build_shared(shared)
    assert shared_read == [7] * 16, shared_read
    (CORPUS / shared.name).write_bytes(shared.read_bytes())
    (FIXTURES / f"{shared.name}.oracle.json").write_text(
        json.dumps(
            {
                "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
                "libver='earliest', chunk B-tree leaf appended by hand",
                "note": "one stored chunk named at origins (i, 0) for i < 16 (#837)",
                "shape": [16, 1],
                "libhdf5_values": shared_read,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {shared} ({shared.stat().st_size} B); libhdf5 reads {shared_read}; seed copied")

    for vary in ("mask", "size"):
        variant = FIXTURES / f"hdf5_shared_chunk_records_{vary}s.h5"
        variant_read = build_shared(variant, vary)
        assert variant_read == [7] * 16, (vary, variant_read)
        (FIXTURES / f"{variant.name}.oracle.json").write_text(
            json.dumps(
                {
                    "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
                    "libver='earliest', chunk B-tree leaf appended by hand",
                    "note": f"one stored chunk named at origins (i, 0) for i < 16, record i's "
                    f"{vary} varied by {'2 * i' if vary == 'mask' else 'i'} (#888)",
                    "shape": [16, 1],
                    "libhdf5_values": variant_read,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        print(f"wrote {variant} ({variant.stat().st_size} B); libhdf5 reads {variant_read}")
    masks = FIXTURES / "hdf5_shared_chunk_records_masks.h5"
    (CORPUS / masks.name).write_bytes(masks.read_bytes())

    conflicting = FIXTURES / "hdf5_conflicting_chunk_records.h5"
    libhdf5_read = build_conflicting(conflicting)
    (FIXTURES / f"{conflicting.name}.oracle.json").write_text(
        json.dumps(
            {
                "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
                "libver='earliest', chunk B-tree leaf appended by hand",
                "note": "records A@0, B@0, B@1 for [7, 9]; libhdf5 reads the last record at "
                "origin 0; fieldglass refuses the index (ADR-0013, #837)",
                "libhdf5_values": libhdf5_read,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {conflicting} ({conflicting.stat().st_size} B); libhdf5 reads {libhdf5_read}")

    swapped = FIXTURES / "hdf5_conflicting_chunk_records_swapped.h5"
    write_oracle(
        swapped,
        "records B@0, A@0, B@1 for [7, 9]: the committed conflict with its first two "
        "records swapped; fieldglass refuses it as it refuses that one (ADR-0013, #891)",
        build_conflicting(swapped, swapped=True),
    )
    off_grid = FIXTURES / "hdf5_off_grid_chunk_record.h5"
    write_oracle(
        off_grid,
        "[1, 2, 3, 4] in chunks of 2 with records at origins 0 and 1; origin 1 is inside "
        "the shape and off the chunk grid (ADR-0013 rule 3, #891)",
        hand_index(off_grid, [1, 2, 3, 4], 2, lambda s, _e: [(0, s[0]), (1, s[1])], 4),
    )
    outside = FIXTURES / "hdf5_outside_chunk_record.h5"
    write_oracle(
        outside,
        "[7, 9] in chunks of 1 plus a record at origin 5, outside the shape, whose address "
        "is past the end of the file (ADR-0013 rule 4, #891)",
        hand_index(
            outside, [7, 9], 1, lambda s, end: [(0, s[0]), (1, s[1]), (5, end + 65536)], 6
        ),
    )
    print(f"wrote {duplicated} ({duplicated.stat().st_size} B), {K * R} records; seed copied")


if __name__ == "__main__":
    main()
