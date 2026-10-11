#!/usr/bin/env python3
"""Build the HDF5 fixture behind #970: compressed chunks that decode past what
the reader holds for one chunk.

A chunk may legally be far larger than its dataset when the dataset is
extendable, and its dimensions are the file's own numbers. So a small file can
declare a chunk that decodes to gigabytes, and a valid file can hold one. The
reader refuses such a chunk before it reads or inflates anything, as a valid
file asked for too much rather than a corrupt one.

``hdf5_chunk_past_ceiling.h5`` holds four ``(4, 4)`` datasets with
``maxshape=(None, None)``, written by h5py with the default (earliest) format,
so each chunk index is a version-1 B-tree:

  * ``zstd_f4_past_field_cap`` — ``float32`` in one zstd chunk of
    ``(16384, 16384)``: 256 Mi values, past the 64 Mi one field holds, and
    1 GiB decoded.
  * ``zstd_f8_past_ceiling`` — ``float64`` in one zstd chunk of
    ``(5120, 8192)``: 40 Mi values, inside the field cap, but 320 MiB decoded,
    past the 256 MiB a chunk may decode to.
  * ``deflate_f8_bomb`` — the same chunk shape under gzip, holding a 1 KiB
    zlib stream: a hostile chunk, small on disk, that a reader would have to
    inflate before it could tell it is short.
  * ``deflate_f8_unwritten`` — the same, with nothing written, so no chunk is
    stored and every value is the fill value, 0.

The two zstd chunks are valid. Each is written with ``write_direct_chunk`` as
a frame of RLE blocks repeating the byte ``0x41``, with the frame content size
in its header, so a chunk that decodes to hundreds of megabytes is about 10 KB
(320 MiB) or 32 KB (1 GiB) on disk. Every value is the element whose bytes are
all ``0x41``: 12.078431129455566 as ``float32``, 2261634.5098039214 as
``float64``. libhdf5 reads them back, and the oracle records what it read.
libhdf5 does not check how long a chunk inflates to, so it reads the bomb's
chunk past its 1 KiB into memory it does not own: the values vary run to run
and the read sometimes crashes. The oracle says so instead of reading it.

Run from the repo root (needs ``h5py`` and ``hdf5plugin``, which the pre-commit
venv has; reading the oracle back makes libhdf5 allocate about 1.3 GB):

    python3 tools/build_hdf5_chunk_ceiling_fixture.py
"""
from __future__ import annotations

import json
import struct
import zlib
from pathlib import Path

import h5py
import hdf5plugin
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "hdf5_chunk_past_ceiling.h5"
BYTE = 0x41
# The largest block a zstd frame may hold (`Block_Maximum_Size`), and the
# window that allows it: 2^17 is window descriptor exponent 7, mantissa 0.
BLOCK = 128 * 1024
WINDOW_DESCRIPTOR = (17 - 10) << 3


def rle_zstd_frame(length: int) -> bytes:
    """A zstd frame (RFC 8878 §3.1.1) decoding to ``length`` bytes of ``BYTE``.

    The header carries the content size (``FCS_Field_Size`` 8 bytes, not single
    segment, so a window descriptor follows), which libhdf5's zstd filter needs
    to size its buffer. Each block is RLE: a 3-byte header and the one byte.
    """
    frame = bytearray(struct.pack("<I", 0xFD2FB528))
    frame.append(0b1100_0000)
    frame.append(WINDOW_DESCRIPTOR)
    frame += struct.pack("<Q", length)
    left = length
    while left:
        size = min(left, BLOCK)
        left -= size
        last = 1 if left == 0 else 0
        header = last | (1 << 1) | (size << 3)  # Block_Type 1 is RLE
        frame += header.to_bytes(3, "little")
        frame.append(BYTE)
    return bytes(frame)


def dataset(f: h5py.File, name: str, dtype: str, chunks: tuple[int, int], **kw):
    return f.create_dataset(
        name,
        shape=(4, 4),
        maxshape=(None, None),
        chunks=chunks,
        dtype=dtype,
        fillvalue=0,
        track_times=False,
        **kw,
    )


# libhdf5 does not check the length a chunk inflates to. It reads the bomb's
# chunk past the 1 KiB it inflated, into memory it does not own, so what it
# returns varies run to run and the read sometimes crashes. Recorded, not read.
BOMB_NOTE = (
    "not read: libhdf5 2.0.0 inflates the 1 KiB stream and reads the chunk's "
    "335544320 bytes past it, returning whatever memory follows or crashing "
    "(SIGSEGV), varying run to run"
)


def main() -> None:
    path = FIXTURES / NAME
    big_f4 = (16384, 16384)
    big_f8 = (5120, 8192)
    with h5py.File(path, "w") as f:
        d = dataset(f, "zstd_f4_past_field_cap", "f4", big_f4, compression=hdf5plugin.Zstd())
        d.id.write_direct_chunk((0, 0), rle_zstd_frame(big_f4[0] * big_f4[1] * 4))
        d = dataset(f, "zstd_f8_past_ceiling", "f8", big_f8, compression=hdf5plugin.Zstd())
        d.id.write_direct_chunk((0, 0), rle_zstd_frame(big_f8[0] * big_f8[1] * 8))
        d = dataset(f, "deflate_f8_bomb", "f8", big_f8, compression="gzip")
        d.id.write_direct_chunk((0, 0), zlib.compress(bytes([BYTE]) * 1024, 9))
        dataset(f, "deflate_f8_unwritten", "f8", big_f8, compression="gzip")

    oracle: dict = {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        f"hdf5plugin {hdf5plugin.version}, chunks written with write_direct_chunk",
        "note": "chunks decoding past 256 MiB or 64 Mi values; fieldglass refuses each "
        "stored one before reading it (#970)",
        "datasets": {},
    }
    with h5py.File(path, "r") as f:
        for name, d in f.items():
            entry: dict = {
                "dtype": str(d.dtype),
                "shape": list(d.shape),
                "chunks": list(d.chunks),
                "chunk_bytes": int(np.prod(d.chunks)) * d.dtype.itemsize,
            }
            if d.id.get_num_chunks():
                # Where the one stored chunk is, so a test can show it unread.
                info = d.id.get_chunk_info(0)
                entry["chunk_byte_offset"] = info.byte_offset
                entry["chunk_stored_bytes"] = info.size
            if name == "deflate_f8_bomb":
                entry["libhdf5"] = BOMB_NOTE
            else:
                entry["libhdf5_values"] = d[...].tolist()
            oracle["datasets"][name] = entry
    (FIXTURES / f"{NAME}.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {path} ({path.stat().st_size} B) + oracle")


if __name__ == "__main__":
    main()
