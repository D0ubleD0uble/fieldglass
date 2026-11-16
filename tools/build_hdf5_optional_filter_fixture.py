#!/usr/bin/env python3
"""Build ``hdf5_optional_filter_listing.h5``, the filter-mask cases of #1855.

The listing names a variable whose filter pipeline a read cannot undo. For a
dataset of several chunks it reads the first chunk's filter mask from the
chunk index, since libhdf5 skips a filter it lacks on every chunk. These two
datasets, ``float32`` [4, 6] in chunks of [2, 3] (four chunks, a fixed-array
index), are where a rule keyed on the pipeline message alone goes wrong.
Both read, since every chunk skipped its one filter (filter mask 1), and the
listing names neither:

  * ``plugin_optional`` — third-party filter 32099, which no build has, set
    optional (``H5Z_FLAG_OPTIONAL``, as h5py's ``allow_unknown_filter``
    does). libhdf5 skipped it on every chunk at write.
  * ``mandatory_skipped`` — scale-offset (6) set mandatory, every chunk
    written with ``write_direct_chunk`` and filter mask 1.

``0.25 * k`` for element ``k``, exact in float32.

Run from the repo root (needs ``h5py``, which the pre-commit venv has):

    python3 tools/build_hdf5_optional_filter_fixture.py
"""
from __future__ import annotations

from pathlib import Path

import h5py
import numpy as np
from h5py import h5d, h5p, h5s, h5t, h5z

OUT = Path("crates/fieldglass-netcdf/tests/fixtures/hdf5_optional_filter_listing.h5")

SHAPE = (4, 6)
CHUNK = (2, 3)
PLUGIN = 32099


def values() -> np.ndarray:
    return (0.25 * np.arange(24, dtype="f4")).reshape(SHAPE)


def main() -> None:
    v = values()
    with h5py.File(OUT, "w", libver="latest") as f:
        f.create_dataset(
            "plugin_optional",
            data=v,
            chunks=CHUNK,
            compression=PLUGIN,
            allow_unknown_filter=True,
            track_times=False,
        )
        dcpl = h5p.create(h5p.DATASET_CREATE)
        dcpl.set_chunk(CHUNK)
        dcpl.set_obj_track_times(False)
        dcpl.set_filter(h5z.FILTER_SCALEOFFSET, h5z.FLAG_MANDATORY, (0, 2))
        space = h5s.create_simple(SHAPE)
        mandatory = h5py.Dataset(
            h5d.create(f.id, b"mandatory_skipped", h5t.IEEE_F32LE, space, dcpl=dcpl)
        )
        for r in range(0, SHAPE[0], CHUNK[0]):
            for c in range(0, SHAPE[1], CHUNK[1]):
                block = np.ascontiguousarray(v[r : r + CHUNK[0], c : c + CHUNK[1]])
                mandatory.id.write_direct_chunk((r, c), block.astype("<f4").tobytes(), filter_mask=1)

    with h5py.File(OUT, "r") as f:
        for name, want in (("plugin_optional", (PLUGIN, h5z.FLAG_OPTIONAL)),
                           ("mandatory_skipped", (h5z.FILTER_SCALEOFFSET, h5z.FLAG_MANDATORY))):
            d = f[name]
            plist = d.id.get_create_plist()
            assert plist.get_nfilters() == 1, name
            assert plist.get_filter(0)[:2] == want, (name, plist.get_filter(0))
            masks = [d.id.get_chunk_info(i).filter_mask for i in range(d.id.get_num_chunks())]
            assert masks == [1, 1, 1, 1], (name, masks)
            assert np.array_equal(d[...], v), name
    data = OUT.read_bytes()
    print(f"wrote {OUT} ({len(data)} B), h5py {h5py.__version__}, "
          f"libhdf5 {h5py.version.hdf5_version}")


if __name__ == "__main__":
    main()
