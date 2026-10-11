#!/usr/bin/env python3
"""Build the HDF5 fixtures behind #936: files that begin with a userblock.

An HDF5 file may begin with a userblock, arbitrary bytes a power of two from
512 up, with the superblock after it. Every address in the file is then
relative to the superblock's base address, the offset of its signature
(libhdf5 ``H5F__super_read``).

Four files, one root each holding the same content, written by h5py:

  * ``hdf5_userblock_earliest.h5`` — ``userblock_size=512``, default format
    (superblock version 0, symbol-table groups).
  * ``hdf5_userblock_latest.h5`` — ``userblock_size=512``, ``libver="latest"``
    (superblock version 3, link messages).
  * ``hdf5_no_userblock_earliest.h5`` and ``hdf5_no_userblock_latest.h5`` —
    the twins without a userblock.

Each root holds ``v`` (contiguous ``float32`` [4, 5], with a numeric and a
string attribute, its dimensions the scales ``y`` and ``x``) and ``c``
(chunked, deflated ``int16`` [6, 8], so a chunk index is walked). Attaching
the scales writes ``DIMENSION_LIST``, variable-length references the reader
follows into the global heap. The
userblock is filled with a text header and the rest with ``0x00``, as a tool
embedding one would.

The oracle records the signature offset, the superblock's stored base address,
the member names and every value, read back by h5py.

Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_userblock_fixtures.py
"""
from __future__ import annotations

import json
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
SIGNATURE = b"\x89HDF\r\n\x1a\n"
USERBLOCK = 512
HEADER = b"fieldglass #936: a userblock before the HDF5 superblock\n"


def build(path: Path, libver: str | None, userblock: int) -> dict:
    kwargs: dict = {}
    if libver:
        kwargs["libver"] = libver
    if userblock:
        kwargs["userblock_size"] = userblock
    with h5py.File(path, "w", **kwargs) as f:
        v = f.create_dataset(
            "v", data=np.arange(20, dtype="f4").reshape(4, 5) * 0.5, track_times=False
        )
        v.attrs["scale"] = np.float64(2.5)
        v.attrs["units"] = np.bytes_(b"K")
        for axis, (name, n) in enumerate((("y", 4), ("x", 5))):
            scale = f.create_dataset(name, data=np.arange(n, dtype="f8"), track_times=False)
            scale.make_scale(name)
            v.dims[axis].attach_scale(scale)
        f.create_dataset(
            "c",
            data=(np.arange(48, dtype="i2").reshape(6, 8) - 20),
            chunks=(3, 4),
            compression="gzip",
            track_times=False,
        )
    if userblock:
        # libhdf5 leaves the userblock zeroed; a tool writes its header there.
        with path.open("r+b") as raw:
            raw.write(HEADER)
    data = path.read_bytes()
    signature = data.find(SIGNATURE)
    version = data[signature + 8]
    if version in (0, 1):
        o = data[signature + 13]
        base_at = signature + (24 if version == 0 else 28)
    else:
        o = data[signature + 9]
        base_at = signature + 12
    stored_base = int.from_bytes(data[base_at : base_at + o], "little")
    with h5py.File(path, "r") as f:
        assert f.userblock_size == userblock, (f.userblock_size, userblock)
        members = sorted(f.keys())
        values = {k: f[k][()].astype("f8").ravel().tolist() for k in members}
        v_dims = [f["v"].dims[i][0].name.lstrip("/") for i in range(2)]
        units = f["v"].attrs["units"]
        units = units.decode() if isinstance(units, bytes) else str(units)
        scale = float(f["v"].attrs["scale"])
    return {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        f"libver={libver or 'default (earliest)'}, userblock_size={userblock}",
        "note": "root with contiguous v [4, 5] on scales y and x, and chunked, deflated "
        "c [6, 8] (#936)",
        "signature_offset": signature,
        "superblock_version": version,
        "stored_base_address": stored_base,
        "members": members,
        "values": values,
        "v_dims": v_dims,
        "v_units": units,
        "v_scale": scale,
    }


def main() -> None:
    for name, libver, userblock in (
        ("hdf5_userblock_earliest.h5", None, USERBLOCK),
        ("hdf5_userblock_latest.h5", "latest", USERBLOCK),
        ("hdf5_no_userblock_earliest.h5", None, 0),
        ("hdf5_no_userblock_latest.h5", "latest", 0),
    ):
        path = FIXTURES / name
        oracle = build(path, libver, userblock)
        (FIXTURES / f"{name}.oracle.json").write_text(
            json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
        )
        print(
            f"wrote {path} ({path.stat().st_size} B): signature at "
            f"{oracle['signature_offset']}, superblock v{oracle['superblock_version']}, "
            f"stored base {oracle['stored_base_address']}"
        )


if __name__ == "__main__":
    main()
