#!/usr/bin/env python3
"""Build ``hdf5_scale_offset_listing.h5``, the file behind #1855.

The reader does not decode HDF5's scale-offset filter (id 6). A variable
stored through it is named undecodable, with the refusal a read gives, and
this file holds the cases where that rule could go wrong:

  * ``x`` — the dimension scale of axis 1, ``float32`` [6], under
    scale-offset. It is a coordinate variable a read cannot undo, so it is
    named undecodable but must still be the dimension ``x``.
  * ``y`` — the dimension scale of axis 0, ``float32`` [4], unfiltered.
  * ``field`` — ``float32`` [4, 6] on ``y`` and ``x`` under deflate. It reads,
    and keeps both named axes.
  * ``packed`` — ``float32`` [4, 6] on ``y`` and ``x`` under scale-offset,
    written. Named undecodable.
  * ``unwritten`` — ``float32`` [4, 6] under scale-offset in chunks of
    [2, 3], with no chunk written. A read gives the fill value, -1, without
    running a filter, so it is not named and reads.
  * ``skipped`` — ``float32`` [4, 6] under scale-offset in one chunk (a
    single-chunk index), written with ``write_direct_chunk`` and filter mask
    1, so the chunk skipped the filter and holds its values as they are. It
    is not named and reads.

The format is libver ``latest``, so a chunked dataset with one chunk gets a
single-chunk index, which stores that chunk's filter mask in the layout
message; the others get fixed-array indexes.

Run from the repo root (needs ``h5py``, which the pre-commit venv has):

    python3 tools/build_hdf5_scale_offset_listing_fixture.py
"""
from __future__ import annotations

from pathlib import Path

import h5py
import numpy as np

OUT = Path("crates/fieldglass-netcdf/tests/fixtures/hdf5_scale_offset_listing.h5")

SHAPE = (4, 6)


def values(base: float) -> np.ndarray:
    """``base`` plus 0.25 per element, exact in float32 and in two decimals."""
    return (base + 0.25 * np.arange(24, dtype="f4")).reshape(SHAPE)


def main() -> None:
    with h5py.File(OUT, "w", libver="latest") as f:
        x = f.create_dataset(
            "x",
            data=np.arange(6, dtype="f4") * 10,
            chunks=(6,),
            scaleoffset=2,
            track_times=False,
        )
        x.make_scale("x")
        y = f.create_dataset("y", data=np.arange(4, dtype="f4"), track_times=False)
        y.make_scale("y")
        for name, base, opts in (
            ("field", 0.0, {"compression": "gzip"}),
            ("packed", 100.0, {"scaleoffset": 2}),
        ):
            v = f.create_dataset(name, data=values(base), chunks=(2, 3), track_times=False, **opts)
            v.dims[0].attach_scale(y)
            v.dims[1].attach_scale(x)
        f.create_dataset(
            "unwritten",
            shape=SHAPE,
            dtype="f4",
            chunks=(2, 3),
            scaleoffset=2,
            fillvalue=-1.0,
            track_times=False,
        )
        skipped = f.create_dataset(
            "skipped",
            shape=SHAPE,
            dtype="f4",
            chunks=SHAPE,
            scaleoffset=2,
            track_times=False,
        )
        skipped.id.write_direct_chunk((0, 0), values(200.0).astype("<f4").tobytes(), filter_mask=1)

    with h5py.File(OUT, "r") as f:
        for name in ("x", "packed", "unwritten", "skipped"):
            assert f[name].scaleoffset == 2, name
        assert np.array_equal(f["field"][...], values(0.0))
        assert np.allclose(f["packed"][...], values(100.0), atol=0.005)
        assert np.array_equal(f["unwritten"][...], np.full(SHAPE, -1.0, dtype="f4"))
        assert np.array_equal(f["skipped"][...], values(200.0))
        assert np.array_equal(f["x"][...], np.arange(6, dtype="f4") * 10)
        dims = [f["packed"].dims[a][0].name for a in range(2)]
        assert dims == ["/y", "/x"], dims
    data = OUT.read_bytes()
    print(f"wrote {OUT} ({len(data)} B), h5py {h5py.__version__}, "
          f"libhdf5 {h5py.version.hdf5_version}")


if __name__ == "__main__":
    main()
