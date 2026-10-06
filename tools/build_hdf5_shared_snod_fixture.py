#!/usr/bin/env python3
"""Build the two HDF5 fixtures behind #901: a symbol-table group whose index
names one stored block from two places.

Both start from one h5py file (``libver="earliest"``: superblock v0, a
version-1 group B-tree of ``SNOD`` symbol-table nodes and a local heap of
names) whose root holds eight datasets, one named with 2,000 ``L`` characters,
so all eight fit in one ``SNOD``.

  * ``hdf5_shared_group_name.h5`` points a second ``SNOD`` entry's name offset
    at the long name, so two members share one name in the local heap.
  * ``hdf5_shared_snod.h5`` appends a version-1 B-tree leaf whose two entries
    both name the one ``SNOD``, and repoints the group at it.

A valid group gives each node and each name its own storage. Before #901 the
reader read the shared one once per name: the issue's version of the second
shape, 8,192 references to one ``SNOD`` whose entries all named one 60 KB
name, made a 197 KB file use 3.85 GB. Version-1 group structures carry no
checksums. h5py lists the first file with two members of the long name; it
refuses the second, reading the appended node at libhdf5's full node size.

Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_shared_snod_fixture.py
"""
from __future__ import annotations

import struct
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
LONG = "L" * 2000


def base() -> bytearray:
    path = FIXTURES / "hdf5_shared_snod.src.h5"
    with h5py.File(path, "w", libver="earliest") as f:
        f.create_dataset(LONG, data=np.zeros(1, dtype="u1"), track_times=False)
        for i in range(7):
            f.create_dataset(f"d{i}", data=np.zeros(1, dtype="u1"), track_times=False)
    raw = bytearray(path.read_bytes())
    path.unlink()
    return raw


def snod(raw: bytearray) -> int:
    found = [i for i in range(len(raw) - 4) if raw[i : i + 4] == b"SNOD"]
    assert len(found) == 1, found
    return found[0]


def entries(raw: bytearray, at: int) -> list[int]:
    """The entry offsets of the SNOD at `at`: 8-byte header, then 40-byte
    entries (name offset, header address, cache type, reserved, scratch)."""
    count = struct.unpack_from("<H", raw, at + 6)[0]
    return [at + 8 + 40 * i for i in range(count)]


def build_shared_name(path: Path) -> None:
    raw = base()
    es = entries(raw, snod(raw))
    assert len(es) == 8
    # The long name's offset in the local heap's data segment, whose address
    # follows the heap's signature, version, reserved bytes, data size and
    # free-list offset.
    heap = raw.find(b"HEAP")
    data = struct.unpack_from("<Q", raw, heap + 24)[0]
    long_at = raw.find(LONG.encode()) - data
    offsets = [struct.unpack_from("<Q", raw, e)[0] for e in es]
    assert long_at in offsets, (long_at, offsets)
    target = next(e for e, o in zip(es, offsets) if o != long_at)
    struct.pack_into("<Q", raw, target, long_at)
    path.write_bytes(bytes(raw))


def build_shared_snod(path: Path) -> None:
    raw = base()
    at = snod(raw)
    trees = [i for i in range(len(raw) - 5) if raw[i : i + 4] == b"TREE" and raw[i + 4] == 0]
    assert len(trees) == 1, trees
    old = struct.pack("<Q", trees[0])
    while len(raw) % 8:
        raw.append(0)
    leaf = len(raw)
    undefined = 0xFFFF_FFFF_FFFF_FFFF
    node = b"TREE" + struct.pack("<BBHQQ", 0, 0, 2, undefined, undefined)
    for key in (0, 8):
        node += struct.pack("<QQ", key, at)
    node += struct.pack("<Q", 16)
    raw += node
    # The group's B-tree address is in its symbol-table message and in the
    # superblock's cached root entry; point both at the new leaf.
    n = 0
    start = 0
    while (i := raw.find(old, start, leaf)) != -1:
        raw[i : i + 8] = struct.pack("<Q", leaf)
        n += 1
        start = i + 8
    assert n >= 1, "no reference to the group B-tree"
    struct.pack_into("<Q", raw, 40, len(raw))  # superblock v0 end-of-file address
    path.write_bytes(bytes(raw))


def main() -> None:
    for name, build in (
        ("hdf5_shared_group_name.h5", build_shared_name),
        ("hdf5_shared_snod.h5", build_shared_snod),
    ):
        path = FIXTURES / name
        build(path)
        print(f"wrote {path} ({path.stat().st_size} B)")


if __name__ == "__main__":
    main()
