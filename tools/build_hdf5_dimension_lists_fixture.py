#!/usr/bin/env python3
"""Build ``hdf5_dimension_lists.h5``, the file behind #1698.

A root of nine variables ``v0`` to ``v8``, each ``float32`` [2, 3] on the
dimension scales ``y`` and ``x``, so the file holds 18 ``DIMENSION_LIST``
references, each a global-heap object. libhdf5 writes all 18 into one
collection.

``crates/fieldglass/tests/dimension_lists_in_many_collections.rs`` moves each
reference into a collection of its own, appended to the file, alongside
65,534 other objects: 1,179,630 objects in the collections the references
name, past the 1,048,576 the reader once held across every collection. That
is a valid file, since each collection's objects have distinct indices, and
0.6.0 opened it. 18 is the fewest references that pass the budget that way:
a collection holds at most 65,535 objects with distinct indices, and 16 of
them hold 1,048,560.

The format is the default (superblock version 0, version-1 object headers),
whose headers carry no checksum, so the test can rewrite a reference in place.

Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_dimension_lists_fixture.py
"""
from __future__ import annotations

from pathlib import Path

import h5py
import numpy as np

OUT = Path("crates/fieldglass-netcdf/tests/fixtures/hdf5_dimension_lists.h5")


def main() -> None:
    with h5py.File(OUT, "w") as f:
        scales = []
        for name, n in (("y", 2), ("x", 3)):
            scale = f.create_dataset(name, data=np.arange(n, dtype="f8"), track_times=False)
            scale.make_scale(name)
            scales.append(scale)
        for k in range(9):
            v = f.create_dataset(
                f"v{k}",
                data=(np.arange(6, dtype="f4").reshape(2, 3) + 10 * k),
                track_times=False,
            )
            for axis, scale in enumerate(scales):
                v.dims[axis].attach_scale(scale)
    with h5py.File(OUT, "r") as f:
        for k in range(9):
            dims = [f[f"v{k}"].dims[a][0].name for a in range(2)]
            assert dims == ["/y", "/x"], dims
    data = OUT.read_bytes()
    assert data.count(b"GCOL") == 1, "one global-heap collection"
    print(f"wrote {OUT} ({len(data)} B), h5py {h5py.__version__}, "
          f"libhdf5 {h5py.version.hdf5_version}")


if __name__ == "__main__":
    main()
