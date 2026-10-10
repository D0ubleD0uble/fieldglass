#!/usr/bin/env python3
"""Generate `hdf5_fixed_array_huge_count.h5`, a hostile Fixed Array chunk index
(#939 review).

h5py (libver='latest') writes a 5 x 7 float32 dataset `v` with 1 x 1 chunks,
indexed by a version-4 Fixed Array. The file is then patched:

  * the dataspace's dimensions (current and maximum) become 131072 x 131072;
  * the Fixed Array header (`FAHD`) states page_bits = 63 and an entry count of
    131072^2 = 2^34, which equals the new chunk grid, so every consistency check
    the reader made before the fix passes;
  * every checksum the patch touched (the object header's and the `FAHD`'s,
    Jenkins lookup3 as HDF5 uses) is recomputed, so the file is valid as far
    as its checksums go.

The reader used to size its record list from that entry count, 687 GB, before
reading one. A region read of one row has nothing else to stop it, since a
region read is not held to the whole-variable budget. The fix refuses a chunk
grid past `MAX_BTREE_NODES` chunks before anything is sized from it.

Run from the repo root (needs `h5py` + `numpy`):

    python3 tools/build_hdf5_fixed_array_count_fixture.py
"""
from __future__ import annotations

import struct
import sys
from pathlib import Path

import h5py  # type: ignore
import numpy as np

OUT = Path("crates/fieldglass-netcdf/tests/fixtures/hdf5_fixed_array_huge_count.h5")
BIG = 131072
MASK = 0xFFFFFFFF


def _rot(x: int, k: int) -> int:
    return ((x << k) | (x >> (32 - k))) & MASK


def lookup3(data: bytes, initval: int = 0) -> int:
    """Bob Jenkins' lookup3 `hashlittle`, the checksum HDF5 writes."""
    length = len(data)
    a = b = c = (0xDEADBEEF + length + initval) & MASK
    i = 0

    def mix(a: int, b: int, c: int) -> tuple[int, int, int]:
        a = (a - c) & MASK; a ^= _rot(c, 4); c = (c + b) & MASK
        b = (b - a) & MASK; b ^= _rot(a, 6); a = (a + c) & MASK
        c = (c - b) & MASK; c ^= _rot(b, 8); b = (b + a) & MASK
        a = (a - c) & MASK; a ^= _rot(c, 16); c = (c + b) & MASK
        b = (b - a) & MASK; b ^= _rot(a, 19); a = (a + c) & MASK
        c = (c - b) & MASK; c ^= _rot(b, 4); b = (b + a) & MASK
        return a, b, c

    def final(a: int, b: int, c: int) -> tuple[int, int, int]:
        c ^= b; c = (c - _rot(b, 14)) & MASK
        a ^= c; a = (a - _rot(c, 11)) & MASK
        b ^= a; b = (b - _rot(a, 25)) & MASK
        c ^= b; c = (c - _rot(b, 16)) & MASK
        a ^= c; a = (a - _rot(c, 4)) & MASK
        b ^= a; b = (b - _rot(a, 14)) & MASK
        c ^= b; c = (c - _rot(b, 24)) & MASK
        return a, b, c

    while length > 12:
        a = (a + int.from_bytes(data[i:i + 4], "little")) & MASK
        b = (b + int.from_bytes(data[i + 4:i + 8], "little")) & MASK
        c = (c + int.from_bytes(data[i + 8:i + 12], "little")) & MASK
        a, b, c = mix(a, b, c)
        length -= 12
        i += 12
    if length == 0:
        return c
    tail = data[i:i + length] + bytes(12 - length)
    a = (a + int.from_bytes(tail[0:4], "little")) & MASK
    b = (b + int.from_bytes(tail[4:8], "little")) & MASK
    c = (c + int.from_bytes(tail[8:12], "little")) & MASK
    a, b, c = final(a, b, c)
    return c


def checksum_end(data: bytearray, start: int, limit: int = 8192) -> int | None:
    """Where the checksum of the structure starting at `start` sits, found by
    trying each length until the stored value matches."""
    for end in range(start + 4, min(len(data) - 4, start + limit)):
        if lookup3(bytes(data[start:end])) == int.from_bytes(data[end:end + 4], "little"):
            return end
    return None


def main() -> int:
    if not OUT.parent.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    with h5py.File(OUT, "w", libver="latest") as f:
        d = f.create_dataset("v", shape=(5, 7), chunks=(1, 1), dtype="f4")
        d[...] = np.arange(35, dtype="f4").reshape(5, 7)
    data = bytearray(OUT.read_bytes())
    checksummed = []
    for sig in (b"OHDR", b"FAHD"):
        at = data.find(sig)
        while at != -1:
            end = checksum_end(data, at)
            if end:
                checksummed.append((at, end))
            at = data.find(sig, at + 1)
    fahd = data.find(b"FAHD")
    dims = struct.pack("<QQ", 5, 7)
    at = data.find(dims)
    while at != -1:
        data[at:at + 16] = struct.pack("<QQ", BIG, BIG)
        at = data.find(dims, at + 16)
    data[fahd + 7] = 63  # page bits
    data[fahd + 8:fahd + 16] = struct.pack("<Q", BIG * BIG)  # max entries
    for start, end in checksummed:
        data[end:end + 4] = struct.pack("<I", lookup3(bytes(data[start:end])))
    OUT.write_bytes(data)
    print(f"{OUT}: {len(data)} bytes, checksums fixed at {checksummed}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
