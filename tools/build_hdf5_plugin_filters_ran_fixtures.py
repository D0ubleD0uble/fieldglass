#!/usr/bin/env python3
"""Build the three fixtures behind #1855 whose third-party filter ran.

h5py with hdf5plugin, and netCDF-C, write a third-party filter optional
(``H5Z_FLAG_OPTIONAL``), and with the plugin present it runs on every chunk.
The listing must still name such a variable undecodable, since a read here
refuses it, which a rule keyed on the optional flag got wrong:

  * ``hdf5_lz4_chunks.h5``: ``lz4`` — ``float32`` [4, 6] in chunks of
    [2, 3] (four chunks, a fixed-array index) under hdf5plugin's LZ4 (filter
    id 32004). ``lz4_unlimited`` is the same with an unlimited first
    dimension, so an extensible array indexes it, and ``lz4_btree`` the same
    in the earliest file format, so a version-1 B-tree indexes it.
  * ``netcdf4_bzip2_chunks.nc``: ``bz`` — ``float32`` [4, 6] on ``y`` and
    ``x`` in chunks of [2, 3] under netCDF-C's bzip2 (filter id 307),
    written by netCDF4-python with ``compression="bzip2"``.
  * ``hdf5_lz4_first_chunk_skipped.h5``: ``fa_first_skipped``,
    ``ea_first_skipped`` and ``bt_first_skipped``, laid out as ``lz4``,
    ``lz4_unlimited`` and ``lz4_btree``, with the first chunk rewritten by
    ``write_direct_chunk`` unfiltered and filter mask 1. libhdf5 skips an
    optional filter on a chunk it fails on, as a compressor can on a chunk it
    cannot shrink, so the first chunk alone does not say the filter never
    ran: the other three ran it.

Every chunk of the first two files has filter mask 0: the filter ran. Each
dataset reads back as ``0.25 * k`` through the library that wrote it.

Run from the repo root (needs ``h5py``, ``hdf5plugin`` and ``netCDF4``, which
the pre-commit venv has):

    python3 tools/build_hdf5_plugin_filters_ran_fixtures.py
"""
from __future__ import annotations

from pathlib import Path

import h5py
import hdf5plugin
import netCDF4
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
LZ4 = FIXTURES / "hdf5_lz4_chunks.h5"
BZIP2 = FIXTURES / "netcdf4_bzip2_chunks.nc"
FIRST_SKIPPED = FIXTURES / "hdf5_lz4_first_chunk_skipped.h5"

SHAPE = (4, 6)
CHUNKS = (2, 3)
LZ4_ID = 32004
BZIP2_ID = 307


def values() -> np.ndarray:
    """0.25 per element, exact in float32."""
    return (0.25 * np.arange(24, dtype="f4")).reshape(SHAPE)


def chunk_masks(dset: h5py.Dataset) -> list[int]:
    """Each written chunk's filter mask."""
    return [dset.id.get_chunk_info(i).filter_mask for i in range(dset.id.get_num_chunks())]


def check_ran(dset: h5py.Dataset, filter_id: int, masks: list[int] | None = None) -> None:
    """The filter is in the pipeline, optional, and ran on all four chunks,
    or on those `masks` gives 0."""
    plist = dset.id.get_create_plist()
    filters = [plist.get_filter(i) for i in range(plist.get_nfilters())]
    assert any(f[0] == filter_id and f[1] & 1 for f in filters), filters
    want = masks or [0, 0, 0, 0]
    assert chunk_masks(dset) == want, chunk_masks(dset)


def main() -> None:
    layouts = (
        ("latest", "lz4", SHAPE),
        ("latest", "lz4_unlimited", (None, SHAPE[1])),
        ("earliest", "lz4_btree", SHAPE),
    )
    for libver, name, maxshape in layouts:
        mode = "w" if name == "lz4" else "a"
        with h5py.File(LZ4, mode, libver=libver) as f:
            f.create_dataset(
                name,
                data=values(),
                chunks=CHUNKS,
                maxshape=maxshape,
                track_times=False,
                **hdf5plugin.LZ4(),
            )

    skipped = (
        ("latest", "fa_first_skipped", SHAPE),
        ("latest", "ea_first_skipped", (None, SHAPE[1])),
        ("earliest", "bt_first_skipped", SHAPE),
    )
    for libver, name, maxshape in skipped:
        mode = "w" if name == "fa_first_skipped" else "a"
        with h5py.File(FIRST_SKIPPED, mode, libver=libver) as f:
            dset = f.create_dataset(
                name,
                data=values(),
                chunks=CHUNKS,
                maxshape=maxshape,
                track_times=False,
                **hdf5plugin.LZ4(),
            )
            first = np.ascontiguousarray(values()[: CHUNKS[0], : CHUNKS[1]])
            dset.id.write_direct_chunk((0, 0), first.tobytes(), filter_mask=1)

    with netCDF4.Dataset(BZIP2, "w", format="NETCDF4") as nc:
        nc.createDimension("y", SHAPE[0])
        nc.createDimension("x", SHAPE[1])
        bz = nc.createVariable("bz", "f4", ("y", "x"), compression="bzip2", chunksizes=CHUNKS)
        bz[:] = values()

    with h5py.File(LZ4, "r") as f:
        for _, name, _ in layouts:
            check_ran(f[name], LZ4_ID)
            assert np.array_equal(f[name][...], values()), name
    with h5py.File(FIRST_SKIPPED, "r") as f:
        for _, name, _ in skipped:
            check_ran(f[name], LZ4_ID, [1, 0, 0, 0])
            assert np.array_equal(f[name][...], values()), name
    with h5py.File(BZIP2, "r") as f:
        check_ran(f["bz"], BZIP2_ID)
    with netCDF4.Dataset(BZIP2) as nc:
        assert np.array_equal(nc["bz"][:], values())
    for path in (LZ4, BZIP2, FIRST_SKIPPED):
        print(f"wrote {path} ({path.stat().st_size} B)")
    print(f"h5py {h5py.__version__}, hdf5plugin {hdf5plugin.version}, "
          f"netCDF4 {netCDF4.__version__} (netCDF-C {netCDF4.__netcdf4libversion__}), "
          f"libhdf5 {h5py.version.hdf5_version}")


if __name__ == "__main__":
    main()
