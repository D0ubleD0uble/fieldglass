#!/usr/bin/env python3
"""Build the HDF5 fixtures behind #920: files whose B-tree "K" values are not
the defaults, with nodes fuller than the defaults allow.

A version-1 B-tree node holds at most 2K entries. The superblock states K for
symbol-table nodes (Group Leaf Node K), group B-tree nodes (Group Internal Node
K) and, from version 1, chunk B-tree nodes (Indexed Storage Internal Node K). A
version-2 superblock states them in a B-tree 'K' Values message in its
superblock extension instead. A reader that capped nodes at the default K would
refuse these files; one that read K from the wrong place would too.

  * ``hdf5_btree_k_sb1.h5``: ``set_libver_bounds(EARLIEST, V18)``, which with
    a non-default indexed-storage K writes a version-1 superblock. Its root
    holds a chunked dataset ``v`` (100 one-element chunks in one chunk B-tree
    node), twelve empty groups, and a group ``wide`` holding 300 soft links,
    enough symbol-table nodes for one group B-tree node to pass 2 x 16.
  * ``hdf5_btree_k_sb2.h5``: ``set_libver_bounds(V18, V18)``, a version-2
    superblock with a superblock extension, holding the same ``v``. Groups here
    are new-style, so only the chunk B-tree uses K.

Both use ``H5Pset_sym_k(32, 8)`` and ``H5Pset_istore_k(64)``. h5py does not
wrap those two calls, so the builder makes them through ``ctypes`` on the
libhdf5 that h5py itself loaded, which is the instance whose property-list IDs
h5py hands out.

The oracle records what h5py reads back and the fullest node of each kind,
found by scanning the file for node signatures; the builder checks that each
passes the default 2K.

Run from the repo root on Linux (needs ``h5py``):

    python3 tools/build_hdf5_btree_k_fixtures.py
"""
from __future__ import annotations

import ctypes
import json
import struct
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")

#: (Group Leaf Node K, Group Internal Node K, Indexed Storage Internal Node K)
K = (8, 32, 64)
#: The specification's defaults, in the same order.
DEFAULT_K = (4, 16, 32)


def libhdf5() -> ctypes.CDLL:
    """The libhdf5 h5py loaded: a second copy would not know its IDs."""
    with open("/proc/self/maps", encoding="utf-8") as maps:
        for line in maps:
            path = line.split()[-1]
            name = path.rsplit("/", 1)[-1]
            if name.startswith("libhdf5") and not name.startswith("libhdf5_hl"):
                return ctypes.CDLL(path)
    raise RuntimeError("h5py has not loaded a libhdf5")


def write(path: Path, low: int) -> None:
    lib = libhdf5()
    fcpl = h5py.h5p.create(h5py.h5p.FILE_CREATE)
    fcpl.set_obj_track_times(False)
    leaf, internal, istore = K
    assert lib.H5Pset_sym_k(ctypes.c_int64(fcpl.id), ctypes.c_uint(internal), ctypes.c_uint(leaf)) >= 0
    assert lib.H5Pset_istore_k(ctypes.c_int64(fcpl.id), ctypes.c_uint(istore)) >= 0
    fapl = h5py.h5p.create(h5py.h5p.FILE_ACCESS)
    fapl.set_libver_bounds(low, h5py.h5f.LIBVER_V18)
    fid = h5py.h5f.create(str(path).encode(), h5py.h5f.ACC_TRUNC, fapl=fapl, fcpl=fcpl)

    gcpl = h5py.h5p.create(h5py.h5p.GROUP_CREATE)
    gcpl.set_obj_track_times(False)
    with h5py.File(fid) as f:
        f.create_dataset("v", data=np.arange(100, dtype="i4"), chunks=(1,), track_times=False)
        if low == h5py.h5f.LIBVER_EARLIEST:
            for i in range(12):
                h5py.h5g.create(f.id, f"g{i:02}".encode(), gcpl=gcpl)
            wide = h5py.h5g.create(f.id, b"wide", gcpl=gcpl)
            # Soft links: each takes a symbol-table entry and a heap name but
            # no object header, which keeps the file small.
            for i in range(300):
                wide.links.create_soft(f"s{i:03}".encode(), b"/v")


def fullest_nodes(data: bytes) -> dict[str, int]:
    """The most entries any node of each kind holds, by signature scan."""
    out = {"symbol_node": 0, "group_node": 0, "chunk_node": 0}
    for sig in (b"TREE", b"SNOD"):
        at = data.find(sig)
        while at >= 0:
            entries = struct.unpack_from("<H", data, at + 6)[0]
            kind = "symbol_node" if sig == b"SNOD" else ("group_node", "chunk_node")[data[at + 4]]
            out[kind] = max(out[kind], entries)
            at = data.find(sig, at + 1)
    return out


def oracle(path: Path, version: int) -> dict:
    data = path.read_bytes()
    assert data[8] == version, f"{path}: superblock version {data[8]}, expected {version}"
    if version == 1:
        stated = struct.unpack_from("<HH", data, 16) + struct.unpack_from("<H", data, 24)
        assert stated == K, f"{path}: superblock K {stated}"
    nodes = fullest_nodes(data)
    defaults = dict(zip(("symbol_node", "group_node", "chunk_node"), DEFAULT_K))
    for kind, entries in nodes.items():
        if version == 2 and kind != "chunk_node":
            continue
        assert entries > 2 * defaults[kind], f"{path}: fullest {kind} {entries} fits the default"

    with h5py.File(path, "r") as f:
        members = sorted(f.keys())
        values = [int(x) for x in f["v"][()]]
        wide = sorted(f["wide"].keys()) if "wide" in f else []
    return {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        f"superblock version {version}, H5Pset_sym_k({K[1]}, {K[0]}), H5Pset_istore_k({K[2]})",
        "note": "nodes fuller than the default B-tree K allows (#920)",
        "superblock_version": version,
        "k": {"group_leaf": K[0], "group_internal": K[1], "chunk_internal": K[2]},
        "fullest_nodes": nodes,
        "root_members": members,
        "wide_links": len(wide),
        "v": values,
    }


def main() -> None:
    for version, low in ((1, h5py.h5f.LIBVER_EARLIEST), (2, h5py.h5f.LIBVER_V18)):
        name = f"hdf5_btree_k_sb{version}.h5"
        path = FIXTURES / name
        write(path, low)
        result = oracle(path, version)
        (FIXTURES / f"{name}.oracle.json").write_text(
            json.dumps(result, indent=2) + "\n", encoding="utf-8"
        )
        print(f"wrote {path} ({path.stat().st_size} B): fullest nodes {result['fullest_nodes']}")


if __name__ == "__main__":
    main()
