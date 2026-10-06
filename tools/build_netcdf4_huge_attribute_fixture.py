#!/usr/bin/env python3
"""Build the NetCDF-4 / HDF5 fixtures behind #899: dense attribute and link
storage holding a *huge* fractal-heap object.

Once an object has more than 8 attributes (or a group more than 8 links),
libhdf5 moves them to dense storage: a fractal heap indexed by a version-2
B-tree. An attribute or link message larger than the heap's maximum managed
object size (4 KB for these heaps) is stored as a huge object, outside the
heap's blocks, and found through the heap's own huge-object B-tree.

  * ``netcdf4_huge_attributes.nc`` — written by netCDF-C through
    netCDF4-python (``format="NETCDF4"``): ten short global attributes and a
    5,600-byte ``history``, and one variable ``t(x)`` = ``[1, 2, 3, 4]`` with
    ten short attributes and a 5,000-byte ``comment``. Both attribute sets are
    dense and both long attributes are huge objects. A long ``history`` is how
    real files reach this: NCO and CDO append a line per command.
  * ``hdf5_huge_link_name.h5`` — written by h5py (``libver="latest"``): a group
    ``g`` with nine short subgroups and one whose name is 5,000 characters, so
    its link message is a huge object in the group's dense link storage.

Each has an ``.oracle.json`` holding what netCDF4-python / h5py read back.

Run from the repo root (needs ``netCDF4`` and ``h5py``):

    python3 tools/build_netcdf4_huge_attribute_fixture.py
"""
from __future__ import annotations

import json
from pathlib import Path

import h5py
import netCDF4
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")


def build_netcdf(path: Path) -> dict:
    with netCDF4.Dataset(path, "w", format="NETCDF4") as nc:
        for i in range(10):
            nc.setncattr(f"global_{i}", f"value {i}")
        nc.setncattr("history", "ncks -O " * 700)
        nc.createDimension("x", 4)
        t = nc.createVariable("t", "f4", ("x",))
        for i in range(10):
            t.setncattr(f"attr_{i}", f"value {i}")
        t.setncattr("comment", "a long comment. " * 312 + "end")
        t[:] = np.array([1, 2, 3, 4], dtype="f4")
    with netCDF4.Dataset(path) as nc:
        return {
            "source": f"netCDF4-python {netCDF4.__version__} (netCDF-C "
            f"{netCDF4.__netcdf4libversion__}, HDF5 {netCDF4.__hdf5libversion__})",
            "note": "dense attributes with one huge object at the root and one on t (#899)",
            "global_attributes": {k: nc.getncattr(k) for k in nc.ncattrs()},
            "variables": {
                "t": {
                    "attributes": {k: nc["t"].getncattr(k) for k in nc["t"].ncattrs()},
                    "values": nc["t"][:].tolist(),
                }
            },
        }


def build_links(path: Path) -> dict:
    long_name = "n" * 5000
    with h5py.File(path, "w", libver="latest") as f:
        g = f.create_group("g", track_order=False)
        for i in range(9):
            g.create_group(f"short_{i}")
        g.create_group(long_name)
    with h5py.File(path, "r") as f:
        names = sorted(f["g"].keys())
    return {
        "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
        "libver='latest'",
        "note": "dense link storage in g with one 5,000-character link name, a huge "
        "heap object (#899)",
        "g_links": names,
    }


def main() -> None:
    nc = FIXTURES / "netcdf4_huge_attributes.nc"
    links = FIXTURES / "hdf5_huge_link_name.h5"
    for path, oracle in ((nc, build_netcdf(nc)), (links, build_links(links))):
        (FIXTURES / f"{path.name}.oracle.json").write_text(
            json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
        )
        print(f"wrote {path} ({path.stat().st_size} B) + oracle")


if __name__ == "__main__":
    main()
