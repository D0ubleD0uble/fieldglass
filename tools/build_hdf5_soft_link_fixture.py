#!/usr/bin/env python3
"""Build the HDF5 fixtures behind #914: a root group holding soft links, in
the earliest and the latest file format.

Each root holds datasets ``a``, ``m`` and ``z``, a soft link ``s`` to ``/a``
and a dangling soft link ``d`` to ``/nope``, written by h5py. A symbol-table
node keeps its entries sorted by name (``a``, ``d``, ``m``, ``s``, ``z``), so a
hard link sits between the two soft links and another after both: a reader
that stops at the first soft link, instead of skipping it, lists only ``a``
(#919).

  * ``hdf5_soft_links_earliest.h5`` — the default format: a symbol-table
    group, whose soft-link entries have cache type 2 and an undefined header
    address.
  * ``hdf5_soft_links_latest.h5`` — ``libver="latest"``: link messages.

The oracle records what h5py lists and which members are hard links. The
reader lists hard links in both formats.

Run from the repo root (needs ``h5py``):

    python3 tools/build_hdf5_soft_link_fixture.py
"""
from __future__ import annotations

import json
from pathlib import Path

import h5py
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")


def build(path: Path, libver: str | None) -> dict:
    kwargs = {"libver": libver} if libver else {}
    with h5py.File(path, "w", **kwargs) as f:
        f.create_dataset("a", data=np.arange(3, dtype="f4"), track_times=False)
        f.create_dataset("m", data=np.arange(10, 14, dtype="f4"), track_times=False)
        f.create_dataset("z", data=np.arange(20, 25, dtype="f4"), track_times=False)
        f["s"] = h5py.SoftLink("/a")
        f["d"] = h5py.SoftLink("/nope")
    with h5py.File(path, "r") as f:
        members = sorted(f.keys())
        hard = sorted(k for k in members if isinstance(f.get(k, getlink=True), h5py.HardLink))
    return {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        f"libver={libver or 'default (earliest)'}",
        "note": "root with datasets a, m and z, soft link s -> /a and dangling soft link "
        "d -> /nope (#914, #919)",
        "members": members,
        "hard_links": hard,
    }


def main() -> None:
    for name, libver in (
        ("hdf5_soft_links_earliest.h5", None),
        ("hdf5_soft_links_latest.h5", "latest"),
    ):
        path = FIXTURES / name
        oracle = build(path, libver)
        (FIXTURES / f"{name}.oracle.json").write_text(
            json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
        )
        print(f"wrote {path} ({path.stat().st_size} B): {oracle['members']} hard {oracle['hard_links']}")


if __name__ == "__main__":
    main()
