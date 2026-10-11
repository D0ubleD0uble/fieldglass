#!/usr/bin/env python3
"""Build the fixtures behind #989: one zstd frame whose declared window is far
larger than its chunk, in an HDF5 file and in a Zarr store.

A zstd decoder keeps the frame's whole window before it can return a byte
(RFC 8878 section 3.1.1.1.2), so the window, not the chunk, sets how much it
holds. ``ruzstd`` buffers everything it decodes until it holds more than the
window, so a frame stating 64 MiB in a 4 KiB chunk had it hold about 128 MiB
before the reader's output limit saw a byte. The reader now refuses a window
past ``max(chunk, 8 MiB)`` from the frame header.

The frame is built by hand, since no zstd writer at hand states a window this
far past its content:

  * magic, then a frame header descriptor of 0 (no content size, not
    single-segment, no dictionary, no checksum);
  * a window descriptor of 0x80, exponent 16 and mantissa 0, so
    ``Window_Size = 2 ** 26``, 64 MiB, which is ``MAX_ZSTD_WINDOW``: the
    reader accepted it before #989;
  * ``BLOCKS`` RLE blocks of 128 KiB of zeros, the largest a block may be,
    each a 3-byte header and one byte, the last one flagged.

It decodes to 128 MiB, past the window, so the decoder has to buffer more
than the window before any of it can be read out. Both fixtures hold it in
one 4 KiB chunk, a ``(64, 64)`` ``uint8`` array:

  * ``hdf5_zstd_wide_window.h5``: dataset ``zstd_wide_window`` under
    ``[zstd]`` (filter 32015, through ``hdf5plugin``), the chunk written with
    ``write_direct_chunk``, plus an oracle of the pipeline, where the chunk
    lies, the frame's header and what libhdf5 makes of a read;
  * ``zstd_wide_window/`` in the Zarr fixtures: a v2 array with the ``zstd``
    compressor and the frame as chunk ``0.0``.

Run from the repo root (needs ``h5py`` and ``hdf5plugin``, which the
pre-commit venv has):

    python3 tools/build_zstd_wide_window_fixtures.py
"""
from __future__ import annotations

import json
from pathlib import Path

import h5py
import hdf5plugin
import numpy as np

NETCDF_FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
H5_NAME = "hdf5_zstd_wide_window.h5"
ZARR_STORE = Path("crates/fieldglass-zarr/tests/fixtures/zstd_wide_window")

SHAPE = (64, 64)
MAGIC = bytes.fromhex("28b52ffd")
# Exponent 16, mantissa 0: 2 ** (10 + 16) bytes, 64 MiB.
WINDOW_DESCRIPTOR = 0x80
WINDOW = 64 << 20
BLOCK = 128 << 10
BLOCKS = 1024
RLE = 1


def wide_window_frame() -> bytes:
    """A zstd frame stating a 64 MiB window, of ``BLOCKS`` RLE blocks of
    zeros (RFC 8878 sections 3.1.1 and 3.1.1.2)."""
    out = bytearray(MAGIC)
    out += bytes([0x00, WINDOW_DESCRIPTOR])
    for i in range(BLOCKS):
        last = 1 if i == BLOCKS - 1 else 0
        header = (BLOCK << 3) | (RLE << 1) | last
        out += header.to_bytes(3, "little") + b"\x00"
    return bytes(out)


def window_bytes(frame: bytes) -> int:
    """The window a frame header declares (RFC 8878 section 3.1.1.1.2)."""
    assert frame[:4] == MAGIC, "zstd magic"
    assert not frame[4] & 0x20, "a single-segment frame has no window byte"
    exponent, mantissa = frame[5] >> 3, frame[5] & 7
    base = 1 << (10 + exponent)
    return base + (base // 8) * mantissa


def build_hdf5(frame: bytes) -> None:
    path = NETCDF_FIXTURES / H5_NAME
    with h5py.File(path, "w") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass zstd frame stating a window far past its chunk")
        d = f.create_dataset(
            "zstd_wide_window",
            shape=SHAPE,
            dtype="u1",
            chunks=SHAPE,
            fillvalue=0,
            track_times=False,
            **hdf5plugin.Zstd(clevel=1),
        )
        d.id.write_direct_chunk((0, 0), frame)

    with h5py.File(path, "r") as f:
        d = f["zstd_wide_window"]
        plist = d.id.get_create_plist()
        info = d.id.get_chunk_info(0)
        stored = d.id.read_direct_chunk((0, 0))[1]
        try:
            values = d[...]
            read = {"libhdf5_values_all_zero": bool((values == 0).all())}
        except Exception as e:  # noqa: BLE001 - the oracle records any refusal
            read = {"libhdf5_error": f"{type(e).__name__}: {e}"}
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
            f"hdf5plugin {hdf5plugin.version}, the chunk written with write_direct_chunk",
            "note": "a hand-built zstd frame stating a 64 MiB window in a 4 KiB chunk; "
            "fieldglass refuses the window from the frame header (#989)",
            "dtype": str(d.dtype),
            "shape": list(d.shape),
            "chunks": list(d.chunks),
            "chunk_bytes": int(np.prod(d.chunks)) * d.dtype.itemsize,
            "pipeline": [plist.get_filter(i)[0] for i in range(plist.get_nfilters())],
            "chunk_byte_offset": info.byte_offset,
            "chunk_stored_bytes": info.size,
            "zstd_single_segment": bool(stored[4] & 0x20),
            "zstd_window_bytes": window_bytes(stored),
            "zstd_decoded_bytes": BLOCK * BLOCKS,
            **read,
        }
    (NETCDF_FIXTURES / f"{H5_NAME}.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {path} ({path.stat().st_size} B) + oracle")


def build_zarr(frame: bytes) -> None:
    ZARR_STORE.mkdir(parents=True, exist_ok=True)
    zarray = {
        "zarr_format": 2,
        "shape": list(SHAPE),
        "chunks": list(SHAPE),
        "dtype": "|u1",
        "compressor": {"id": "zstd", "level": 1},
        "fill_value": 0,
        "order": "C",
        "filters": None,
    }
    (ZARR_STORE / ".zarray").write_text(json.dumps(zarray, indent=2) + "\n", encoding="utf-8")
    (ZARR_STORE / "0.0").write_bytes(frame)
    print(f"wrote {ZARR_STORE}")


def main() -> None:
    frame = wide_window_frame()
    assert window_bytes(frame) == WINDOW
    build_hdf5(frame)
    build_zarr(frame)


if __name__ == "__main__":
    main()
