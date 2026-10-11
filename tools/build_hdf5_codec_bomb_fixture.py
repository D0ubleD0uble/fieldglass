#!/usr/bin/env python3
"""Build the HDF5 fixture behind #981: a deflate and a zstd bomb, each in a
chunk whose own length bounds what the codec may write.

A chunk under ``[deflate]`` or ``[zstd]`` alone must decompress to exactly its
own length, so the reader stops either codec there. Before #981 the buffer it
stopped in still grew by doubling, and the step that reached the limit could
reserve nearly twice it:

  * ``deflate_bomb`` — ``uint8`` under ``[deflate]`` in one chunk of
    ``(1536, 2048)``, 3 MiB. The stored chunk is a 32 KB zlib stream of
    32 MiB of zeros.
  * ``zstd_bomb`` — ``uint8`` under ``[zstd]`` (filter 32015) in the same
    chunk. The stored chunk is libhdf5's own zstd frame (through
    ``hdf5plugin``) of 32 MiB of zeros, taken from a scratch file with
    ``read_direct_chunk``. Level 1, whose 512 KiB window keeps ``ruzstd``'s
    own buffer well under the chunk, so the largest allocation a read makes
    is the output's; level 3's 2 MiB window would put that buffer at 4 MiB.

3 MiB is deliberately not a power of two times where either buffer starts, so
the doubling overshoots it. Both datasets are ``(4, 4)`` with
``maxshape=(None, None)``, so the chunk may dwarf the dataset, and their chunks
are written with ``write_direct_chunk``. The oracle records the pipeline,
where each stored chunk lies, the bombs' inflated size and what libhdf5 makes
of each read.

Run from the repo root (needs ``h5py`` and ``hdf5plugin``, which the
pre-commit venv has; reading the oracle back makes libhdf5 allocate about
70 MB):

    python3 tools/build_hdf5_codec_bomb_fixture.py
"""
from __future__ import annotations

import json
import zlib
from pathlib import Path

import h5py
import hdf5plugin
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "hdf5_codec_bombs.h5"

CHUNK = (1536, 2048)
BOMB_INFLATED = 32 << 20
# The zstd frame is built as one chunk of this shape, BOMB_INFLATED bytes.
BOMB_SHAPE = (4096, 8192)
ZSTD_LEVEL = 1


def zstd_window_bytes(frame: bytes) -> int:
    """The window a zstd frame header declares (RFC 8878 section 3.1.1.1.2)."""
    assert frame[:4] == bytes.fromhex("28b52ffd"), "zstd magic"
    descriptor = frame[4]
    assert not descriptor & 0x20, "a single-segment frame has no window byte"
    exponent, mantissa = frame[5] >> 3, frame[5] & 7
    base = 1 << (10 + exponent)
    return base + (base // 8) * mantissa


def zstd_frame() -> bytes:
    """libhdf5's zstd frame of ``BOMB_INFLATED`` zero bytes, from an in-memory
    scratch file."""
    with h5py.File("scratch.h5", "w", driver="core", backing_store=False) as f:
        d = f.create_dataset(
            "z",
            data=np.zeros(BOMB_SHAPE, dtype="u1"),
            chunks=BOMB_SHAPE,
            **hdf5plugin.Zstd(clevel=ZSTD_LEVEL),
        )
        mask, frame = d.id.read_direct_chunk((0, 0))
        assert mask == 0, "zstd skipped"
        return frame


def create(f: h5py.File, name: str, **filters) -> h5py.Dataset:
    """A ``(4, 4)`` ``uint8`` dataset that may grow without bound, in chunks of
    ``CHUNK``, with ``filters`` as h5py's keyword arguments."""
    return f.create_dataset(
        name,
        shape=(4, 4),
        maxshape=(None, None),
        dtype="u1",
        chunks=CHUNK,
        fillvalue=0,
        track_times=False,
        **filters,
    )


def libhdf5_read(d: h5py.Dataset) -> dict:
    """What libhdf5 returns for the whole dataset, or the error it raises."""
    try:
        return {"libhdf5_values": d[...].tolist()}
    except Exception as e:  # noqa: BLE001 - the oracle records any refusal
        return {"libhdf5_error": f"{type(e).__name__}: {e}"}


def main() -> None:
    path = FIXTURES / NAME
    with h5py.File(path, "w") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass deflate and zstd bombs bounded by their chunk")
        d = create(f, "deflate_bomb", compression="gzip", compression_opts=9)
        d.id.write_direct_chunk((0, 0), zlib.compress(bytes(BOMB_INFLATED), 9))
        d = create(f, "zstd_bomb", **hdf5plugin.Zstd(clevel=ZSTD_LEVEL))
        d.id.write_direct_chunk((0, 0), zstd_frame())

    oracle: dict = {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        f"hdf5plugin {hdf5plugin.version}, chunks written with write_direct_chunk",
        "note": "deflate and zstd chunks that decompress far past their own length; "
        "fieldglass stops each at that length without reserving past it (#981)",
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
                "chunk_byte_offset": info.byte_offset,
                "chunk_stored_bytes": info.size,
            }
            if entry["pipeline"] == [32015]:
                entry["zstd_window_bytes"] = zstd_window_bytes(d.id.read_direct_chunk((0, 0))[1])
            entry.update(libhdf5_read(d))
            oracle["datasets"][name] = entry
    (FIXTURES / f"{NAME}.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {path} ({path.stat().st_size} B) + oracle")


if __name__ == "__main__":
    main()
