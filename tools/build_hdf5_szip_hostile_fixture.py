#!/usr/bin/env python3
"""Build the HDF5 fixture behind #975: szip chunks whose stated lengths are
far past what their bytes can hold.

A chunk's decoded length comes from its dimensions, which are the file's own
numbers, and #970 lets a filtered chunk be up to 256 MiB. Two szip paths took
that length on trust before #975:

  * ``szip_short_stream`` — ``float32`` under ``[shuffle, szip]`` in one chunk
    of ``(8192, 8192)``: 64 Mi values, exactly 256 MiB, the most a chunk may
    decode to. The stored chunk is szip's 4-byte size prefix, stating those
    268,435,456 bytes, and a 16-byte stream. CCSDS 121.0-B-3 caps what 16
    bytes can code far below that, so the reader refuses the chunk before it
    allocates the length, where it used to zero 256 MiB first.
  * ``szip_deflate_bomb`` — ``uint8`` under ``[szip, deflate]`` in one chunk
    of ``(2048, 2048)``, 4 MiB. The stored chunk is a 32 KB zlib stream of
    32 MiB of zeros. Reading undoes deflate first, and its output is szip's
    stream, whose length nothing records; the reader bounds it by the longest
    stream szip's parameters allow for 4 MiB, about 4.05 MiB, where it used
    to allow 33 times the chunk (132 MiB) and inflated all 32 MiB.

Both datasets are ``(4, 4)`` with ``maxshape=(None, None)``, so the chunk may
dwarf the dataset, and their chunks are written with ``write_direct_chunk``.
libhdf5 writes the filter pipeline and the szip ``cd_values`` itself; the
oracle records them, where each stored chunk lies, and what libhdf5 makes of
each read.

Run from the repo root (needs ``h5py`` with its bundled libaec, which the
pre-commit venv has; reading the oracle back makes libhdf5 allocate about
300 MB):

    python3 tools/build_hdf5_szip_hostile_fixture.py
"""
from __future__ import annotations

import json
import struct
import zlib
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "hdf5_szip_hostile.h5"

SHORT_CHUNK = (8192, 8192)
SHORT_CHUNK_BYTES = SHORT_CHUNK[0] * SHORT_CHUNK[1] * 4
# Sixteen bytes of stream: what they code does not matter, only that it is
# far less than the prefix states.
SHORT_STREAM = bytes(16)

BOMB_CHUNK = (2048, 2048)
BOMB_INFLATED = 32 << 20


def create(f: h5py.File, name: str, h5type, chunks: tuple[int, int], filters) -> h5py.Dataset:
    """A ``(4, 4)`` dataset of ``h5type`` that may grow without bound, with
    ``filters(dcpl)`` setting its pipeline in write order."""
    dcpl = h5py.h5p.create(h5py.h5p.DATASET_CREATE)
    dcpl.set_chunk(chunks)
    filters(dcpl)
    dcpl.set_fill_value(np.zeros((), dtype=h5type.dtype))
    dcpl.set_obj_track_times(False)
    space = h5py.h5s.create_simple((4, 4), (h5py.h5s.UNLIMITED, h5py.h5s.UNLIMITED))
    h5py.h5d.create(f.id, name.encode(), h5type, space, dcpl=dcpl)
    return f[name]


def short_stream_pipeline(dcpl) -> None:
    dcpl.set_shuffle()
    dcpl.set_szip(h5py.h5z.SZIP_NN_OPTION_MASK, 32)


def bomb_pipeline(dcpl) -> None:
    dcpl.set_szip(h5py.h5z.SZIP_EC_OPTION_MASK, 32)
    dcpl.set_deflate(9)


def libhdf5_read(d: h5py.Dataset) -> dict:
    """What libhdf5 returns for the whole dataset, or the error it raises."""
    try:
        return {"libhdf5_values": d[...].tolist()}
    except Exception as e:  # noqa: BLE001 - the oracle records any refusal
        return {"libhdf5_error": f"{type(e).__name__}: {e}"}


def main() -> None:
    path = FIXTURES / NAME
    with h5py.File(path, "w") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass szip chunks stating more than they hold")
        d = create(f, "szip_short_stream", h5py.h5t.IEEE_F32LE, SHORT_CHUNK, short_stream_pipeline)
        d.id.write_direct_chunk((0, 0), struct.pack("<I", SHORT_CHUNK_BYTES) + SHORT_STREAM)
        d = create(f, "szip_deflate_bomb", h5py.h5t.STD_U8LE, BOMB_CHUNK, bomb_pipeline)
        d.id.write_direct_chunk((0, 0), zlib.compress(bytes(BOMB_INFLATED), 9))

    oracle: dict = {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        "chunks written with write_direct_chunk",
        "note": "szip chunks whose stated length is far past what their bytes hold; "
        "fieldglass refuses each before allocating that length (#975)",
        "bomb_inflated_bytes": BOMB_INFLATED,
        "datasets": {},
    }
    with h5py.File(path, "r") as f:
        for name, d in f.items():
            plist = d.id.get_create_plist()
            info = d.id.get_chunk_info(0)
            entry: dict = {
                "dtype": str(d.dtype),
                "shape": list(d.shape),
                "chunks": list(d.chunks),
                "chunk_bytes": int(np.prod(d.chunks)) * d.dtype.itemsize,
                "pipeline": [plist.get_filter(i)[0] for i in range(plist.get_nfilters())],
                "szip_cd_values": [
                    list(plist.get_filter(i)[2])
                    for i in range(plist.get_nfilters())
                    if plist.get_filter(i)[0] == h5py.h5z.FILTER_SZIP
                ][0],
                "chunk_byte_offset": info.byte_offset,
                "chunk_stored_bytes": info.size,
            }
            entry.update(libhdf5_read(d))
            oracle["datasets"][name] = entry
    (FIXTURES / f"{NAME}.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {path} ({path.stat().st_size} B) + oracle")


if __name__ == "__main__":
    main()
