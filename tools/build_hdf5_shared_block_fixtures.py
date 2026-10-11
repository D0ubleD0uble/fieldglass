#!/usr/bin/env python3
"""Build the HDF5 fixtures behind #921: one stored block named by many objects
or groups, and a deep chain of nested groups.

Each file starts from h5py and is patched so that many objects name one block.
A valid file gives each object its own storage, so each shape reads the shared
block once per object unless a listing refuses the second reference.

  * ``hdf5_shared_continuation.h5`` (``libver="earliest"``): 256 datasets whose
    object headers each turn their free-space (NIL) message into a
    continuation to one appended 256 KiB chunk of NIL messages.
  * ``hdf5_shared_attribute_storage.h5`` (``libver="latest"``): dataset
    ``owner`` holds 64 attributes of 3,000 bytes in dense storage (a fractal
    heap and a B-tree v2). 256 datasets each point their Attribute Info
    message at ``owner``'s heap and B-tree, and drop their one compact
    attribute.
  * ``hdf5_shared_link_storage.h5`` (``libver="latest"``): group ``owner``
    holds 64 hard links to one dataset, each named with 1,000 characters, in
    dense storage. 256 empty groups each point their Link Info message at
    ``owner``'s heap and B-tree.
  * ``hdf5_shared_local_heap.h5`` (``libver="earliest"``): the root holds a
    dataset named with 60,000 ``L`` characters and 128 groups. Each group
    points its symbol-table message at the root's local heap, and its one
    member at the long name and the long-named dataset.
  * ``hdf5_deep_group_chain.h5`` (``libver="latest"``): 600 nested groups,
    each named with 200 characters, and one dataset at the bottom. This one is
    a valid file and is not patched.
  * ``hdf5_hard_linked_dense_attributes.h5`` (``libver="latest"``): dataset
    ``v`` with twelve attributes in dense storage, also linked as ``w``. A
    valid file, not patched: one object reached by two names is still one
    object, and must not read as two sharing one store.
  * ``hdf5_deep_group_chain_past_name_cap.h5`` (``libver="latest"``): 100
    nested groups named with 360 characters, and at the bottom a dataset
    ``v`` with 2,000 more hard links to it. A valid file, not patched; its
    2,001 names of about 36 KB each total 72 MB, past the 64 MiB a listing
    holds.

Every checksum a patch touches (version-2 object headers) is recomputed.
h5py reads the valid two; ``NOTICE.md`` records what it makes of the others.

``--scale K`` multiplies the object counts (and the chain depth) by ``K``,
for measuring how a reader's cost grows; the committed files use ``K = 1``.

Run from the repo root (needs ``h5py`` and ``numpy``):

    python3 tools/build_hdf5_shared_block_fixtures.py [--scale K] [--out DIR]
"""
from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

import h5py
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_hdf5_fixed_array_count_fixture import lookup3  # noqa: E402

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
UNDEFINED = 0xFFFF_FFFF_FFFF_FFFF

MSG_NIL = 0x0000
MSG_LINK_INFO = 0x0002
MSG_ATTRIBUTE = 0x000C
MSG_CONTINUATION = 0x0010
MSG_SYMBOL_TABLE = 0x0011
MSG_ATTRIBUTE_INFO = 0x0015


def addr(obj) -> int:
    return h5py.h5o.get_info(obj.id).addr


def set_eof(raw: bytearray) -> None:
    """Superblock version 0 end-of-file address (offsets of 8 bytes)."""
    assert raw[8] == 0, "superblock v0 expected"
    struct.pack_into("<Q", raw, 40, len(raw))


def v1_messages(raw: bytearray, at: int) -> list[tuple[int, int, int]]:
    """(message type, body offset, body size) of a version-1 header's chunk 0."""
    assert raw[at] == 1, "version-1 object header expected"
    count = struct.unpack_from("<H", raw, at + 2)[0]
    out = []
    pos = at + 16
    for _ in range(count):
        mtype, size = struct.unpack_from("<HH", raw, pos)
        out.append((mtype, pos + 8, size))
        pos += 8 + size
    return out


class Ohdr:
    """A version-2 object header's chunk 0: its messages and checksum."""

    def __init__(self, raw: bytearray, at: int) -> None:
        assert raw[at : at + 4] == b"OHDR", hex(at)
        flags = raw[at + 5]
        pos = at + 6
        if flags & 0x20:
            pos += 16
        if flags & 0x10:
            pos += 4
        width = 1 << (flags & 0x03)
        size = int.from_bytes(raw[pos : pos + width], "little")
        pos += width
        self.raw, self.at = raw, at
        self.checksum_at = pos + size
        header = 6 if flags & 0x04 else 4
        self.messages: list[tuple[int, int, int, int]] = []
        while pos + header <= self.checksum_at:
            mtype = raw[pos]
            msize = struct.unpack_from("<H", raw, pos + 1)[0]
            self.messages.append((mtype, pos, pos + header, msize))
            pos += header + msize

    def find(self, mtype: int) -> tuple[int, int, int]:
        """(type offset, body offset, body size) of the first `mtype`."""
        hits = [(p, b, s) for t, p, b, s in self.messages if t == mtype]
        assert hits, f"no message {mtype:#x} at {self.at:#x}"
        return hits[0]

    def reseal(self) -> None:
        image = bytes(self.raw[self.at : self.checksum_at])
        struct.pack_into("<I", self.raw, self.checksum_at, lookup3(image))


def h5(path: Path, libver: str, build) -> bytearray:
    with h5py.File(path, "w", libver=libver) as f:
        build(f)
    raw = bytearray(path.read_bytes())
    return raw


def shared_continuation(path: Path, scale: int) -> None:
    count = 256 * scale
    names = [f"d{i:05d}" for i in range(count)]
    addrs: dict[str, int] = {}

    def build(f):
        for n in names:
            d = f.create_dataset(n, data=np.zeros(1, dtype="u1"), track_times=False)
            addrs[n] = addr(d)

    raw = h5(path, "earliest", build)
    while len(raw) % 8:
        raw.append(0)
    chunk_at = len(raw)
    # Four NIL messages of 65,528 bytes: 256 KiB of message data.
    nil = struct.pack("<HHB3x", MSG_NIL, 65528, 0) + bytes(65528)
    chunk = nil * 4
    raw += chunk
    for n in names:
        nils = [(b, s) for t, b, s in v1_messages(raw, addrs[n]) if t == MSG_NIL]
        assert nils and nils[0][1] >= 16, n
        body, size = nils[0]
        struct.pack_into("<H", raw, body - 8, MSG_CONTINUATION)
        raw[body : body + size] = struct.pack("<QQ", chunk_at, len(chunk)) + bytes(size - 16)
    set_eof(raw)
    path.write_bytes(bytes(raw))


def shared_attribute_storage(path: Path, scale: int) -> None:
    count = 256 * scale
    names = [f"v{i:05d}" for i in range(count)]
    addrs: dict[str, int] = {}

    def build(f):
        owner = f.create_dataset("owner", data=np.zeros(1, dtype="u1"), track_times=False)
        for i in range(64):
            owner.attrs[f"a{i:02d}"] = np.bytes_(b"x" * 3000)
        addrs["owner"] = addr(owner)
        for n in names:
            d = f.create_dataset(n, data=np.zeros(1, dtype="u1"), track_times=False)
            d.attrs["x"] = np.int32(0)
            addrs[n] = addr(d)

    raw = h5(path, "latest", build)
    _, body, size = Ohdr(raw, addrs["owner"]).find(MSG_ATTRIBUTE_INFO)
    info = bytes(raw[body : body + size])
    # version, flags, then the heap and name-index B-tree addresses.
    assert info[1] == 0 and size == 18, (info[:2], size)
    for n in names:
        # The one compact attribute brought an Attribute Info message with both
        # addresses undefined. It now names `owner`'s storage, and the
        # attribute itself becomes free space.
        header = Ohdr(raw, addrs[n])
        _, body, size = header.find(MSG_ATTRIBUTE_INFO)
        assert size == len(info), (n, size)
        raw[body : body + size] = info
        at, _, _ = header.find(MSG_ATTRIBUTE)
        raw[at] = MSG_NIL
        header.reseal()
    path.write_bytes(bytes(raw))


def shared_link_storage(path: Path, scale: int) -> None:
    count = 256 * scale
    names = [f"g{i:05d}" for i in range(count)]
    addrs: dict[str, int] = {}

    def build(f):
        target = f.create_dataset("t", data=np.zeros(1, dtype="u1"), track_times=False)
        owner = f.create_group("owner")
        for i in range(64):
            owner[f"{i:02d}" + "n" * 998] = target
        addrs["owner"] = addr(owner)
        for n in names:
            addrs[n] = addr(f.create_group(n))

    raw = h5(path, "latest", build)
    _, body, size = Ohdr(raw, addrs["owner"]).find(MSG_LINK_INFO)
    info = bytes(raw[body : body + size])
    # version, flags, then the heap and name-index B-tree addresses.
    assert info[1] == 0 and size == 18, (info[:2], size)
    for n in names:
        header = Ohdr(raw, addrs[n])
        _, body, size = header.find(MSG_LINK_INFO)
        assert size == len(info), (n, size)
        raw[body : body + size] = info
        header.reseal()
    path.write_bytes(bytes(raw))


def shared_local_heap(path: Path, scale: int) -> None:
    count = 128 * scale
    long = "L" * 60000
    names = [f"g{i:05d}" for i in range(count)]
    addrs: dict[str, int] = {}

    def build(f):
        addrs["long"] = addr(f.create_dataset(long, data=np.zeros(1, dtype="u1"), track_times=False))
        for n in names:
            g = f.create_group(n)
            g["s"] = h5py.SoftLink("/x")
            addrs[n] = addr(g)

    raw = h5(path, "earliest", build)
    root_heap = None
    root = struct.unpack_from("<Q", raw, 64)[0]  # root symbol-table entry: header address
    for t, body, _ in v1_messages(raw, root):
        if t == MSG_SYMBOL_TABLE:
            root_heap = struct.unpack_from("<Q", raw, body + 8)[0]
    assert root_heap is not None
    assert raw[root_heap : root_heap + 4] == b"HEAP"
    data = struct.unpack_from("<Q", raw, root_heap + 24)[0]
    long_at = raw.find(long.encode()) - data
    assert long_at >= 0
    for n in names:
        table = [b for t, b, _ in v1_messages(raw, addrs[n]) if t == MSG_SYMBOL_TABLE]
        assert len(table) == 1, n
        btree, heap = struct.unpack_from("<QQ", raw, table[0])
        struct.pack_into("<Q", raw, table[0] + 8, root_heap)
        # The group's one leaf node names its one SNOD, whose one entry is the
        # soft link: name offset, header address, cache type, reserved,
        # scratch-pad. Make it a hard link to the long-named dataset, named by
        # the long name in the root's heap.
        assert raw[btree : btree + 4] == b"TREE"
        snod = struct.unpack_from("<Q", raw, btree + 8 + 16 + 8)[0]
        assert raw[snod : snod + 4] == b"SNOD", n
        assert struct.unpack_from("<H", raw, snod + 6)[0] == 1, n
        struct.pack_into("<QQI", raw, snod + 8, long_at, addrs["long"], 0)
    path.write_bytes(bytes(raw))


def deep_group_chain(path: Path, scale: int) -> None:
    depth = 600 * scale

    def build(f):
        g = f
        for i in range(depth):
            g = g.create_group(f"{i:05d}" + "g" * 195)
        g.create_dataset("v", data=np.arange(4, dtype="f4"), track_times=False)

    h5(path, "latest", build)


def deep_chain_past_name_cap(path: Path, scale: int) -> None:
    del scale  # sized to pass the reader's name cap, whatever the scale

    def build(f):
        g = f
        for i in range(100):
            g = g.create_group(f"{i:03d}" + "n" * 357)
        v = g.create_dataset("v", data=np.arange(4, dtype="f4"), track_times=False)
        for i in range(2000):
            g[f"h{i:04d}"] = v

    h5(path, "latest", build)


def hard_linked_dense_attributes(path: Path, scale: int) -> None:
    del scale  # one object, whatever the scale

    def build(f):
        v = f.create_dataset("v", data=np.arange(4, dtype="f4"), track_times=False)
        for i in range(12):
            v.attrs[f"a{i:02d}"] = np.int32(i)
        f["w"] = v

    h5(path, "latest", build)


BUILDERS = {
    "hdf5_shared_continuation.h5": shared_continuation,
    "hdf5_shared_attribute_storage.h5": shared_attribute_storage,
    "hdf5_shared_link_storage.h5": shared_link_storage,
    "hdf5_shared_local_heap.h5": shared_local_heap,
    "hdf5_deep_group_chain.h5": deep_group_chain,
    "hdf5_hard_linked_dense_attributes.h5": hard_linked_dense_attributes,
    "hdf5_deep_group_chain_past_name_cap.h5": deep_chain_past_name_cap,
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--scale", type=int, default=1)
    parser.add_argument("--out", type=Path, default=FIXTURES)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    for name, build in BUILDERS.items():
        path = args.out / name
        build(path, args.scale)
        print(f"wrote {path} ({path.stat().st_size} B)")


if __name__ == "__main__":
    main()
