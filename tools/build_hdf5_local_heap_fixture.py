#!/usr/bin/env python3
"""Build the two HDF5 fixtures behind #908: a symbol-table group whose names
are read against their local heap's data segment size.

Both start from one h5py file (default, earliest format) with datasets
``alpha`` and ``beta``, whose root local heap has an 88-byte data segment
holding ``alpha`` at offset 8 and ``beta`` at offset 16. The heap header's
data segment size is then patched (and its free-list head set to 1,
``H5HL_FREE_NULL``):

  * ``hdf5_local_heap_short.h5`` — size 16, so ``beta``'s offset is at the end
    of the segment. libhdf5 refuses it ("unable to offset into local heap data
    block"); so does Fieldglass.
  * ``hdf5_local_heap_unterminated.h5`` — size 24, with ``beta``'s terminator
    and padding overwritten so the name runs to offset 24, the segment's end.
    The HDF5 File Format Specification ("Local Heap") puts a name inside the
    segment; libhdf5 2.0.0 accepts this one anyway and lists ``betaxxxx``.
    Fieldglass refuses it: a known divergence.

Each has an ``.oracle.json`` recording libhdf5's outcome.

Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_local_heap_fixture.py
"""
from __future__ import annotations

import json
import struct
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")


def base() -> tuple[bytearray, int, int]:
    src = FIXTURES / "hdf5_local_heap.src.h5"
    with h5py.File(src, "w") as f:
        f.create_dataset("alpha", data=np.arange(3, dtype="f4"), track_times=False)
        f.create_dataset("beta", data=np.arange(3, dtype="f4"), track_times=False)
    raw = bytearray(src.read_bytes())
    src.unlink()
    heap = raw.find(b"HEAP")
    size, _free, data = struct.unpack_from("<QQQ", raw, heap + 8)
    assert size == 88, size
    assert raw[data + 8 : data + 14] == b"alpha\0" and raw[data + 16 : data + 21] == b"beta\0"
    return raw, heap, data


def libhdf5_outcome(path: Path) -> dict:
    try:
        with h5py.File(path, "r") as f:
            return {"libhdf5_names": sorted(f.keys())}
    except Exception as e:  # libhdf5 refused the file
        return {"libhdf5_error": str(e).splitlines()[0]}


def main() -> None:
    raw, heap, data = base()
    short = bytearray(raw)
    struct.pack_into("<QQ", short, heap + 8, 16, 1)

    unterminated = bytearray(raw)
    struct.pack_into("<QQ", unterminated, heap + 8, 24, 1)
    # Fill beta's terminator and padding, and terminate it at offset 24, the
    # first byte past the shrunken segment (originally free space).
    unterminated[data + 20 : data + 24] = b"xxxx"
    unterminated[data + 24] = 0

    for name, body, note in (
        ("hdf5_local_heap_short.h5", short, "data segment size 16: beta's offset 16 is at its end"),
        (
            "hdf5_local_heap_unterminated.h5",
            unterminated,
            "data segment size 24: beta's name runs to offset 24, past its end",
        ),
    ):
        path = FIXTURES / name
        path.write_bytes(bytes(body))
        oracle = {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
            "local heap header patched (#908)",
            "note": note,
            **libhdf5_outcome(path),
        }
        (FIXTURES / f"{name}.oracle.json").write_text(
            json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
        )
        print(f"wrote {path} ({path.stat().st_size} B): {oracle}")


if __name__ == "__main__":
    main()
