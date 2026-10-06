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
  * ``hdf5_shared_huge_attribute.h5`` — h5py (``libver="latest"``): a dataset
    ``v`` with nine small attributes and a 5,000-byte one, a huge object; then
    one small attribute's name-index record has its heap ID repointed at the
    huge one (checksums untouched; the reader does not verify B-tree
    checksums). Two records then name one huge object: a listing reads it once
    and refuses the second name, where 15,000 such records made a 1.9 MB file
    use 34 GB (#899 review). libhdf5 refuses the edited node on its checksum.

  * ``hdf5_huge_only_heaps.h5`` — h5py (``libver="latest"``): heaps holding
    only huge objects, which libhdf5 gives no root block (#907). Dataset
    ``one`` has a single 70,000-byte attribute (too big for its object header,
    so its attributes go dense at once); dataset ``nine`` has nine 5,000-byte
    attributes; group ``g`` has nine links, each named with 5,000 characters.

The first two and the last have an ``.oracle.json`` holding what netCDF4-python / h5py
read back.

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


def build_shared_huge(path: Path) -> None:
    import re

    with h5py.File(path, "w", libver="latest") as f:
        v = f.create_dataset("v", data=np.arange(4, dtype="i4"), track_times=False)
        for i in range(9):
            v.attrs[f"a{i}"] = np.int32(i)
        v.attrs["big"] = np.zeros(5000, dtype="u1")
    raw = bytearray(path.read_bytes())
    # Type-8 (attribute name) B-tree v2 leaves: "BTLF", version, type, then
    # 17-byte records whose first 8 bytes are the heap ID (type in bits 4-5).
    records = []
    for leaf in (m.start() for m in re.finditer(b"BTLF", raw)):
        if raw[leaf + 5] != 8:
            continue
        p = leaf + 6
        for _ in range(10):
            records.append(p)
            p += 17
    huge = [p for p in records if (raw[p] >> 4) & 3 == 1]
    managed = [p for p in records if (raw[p] >> 4) & 3 == 0]
    assert len(huge) == 1 and managed, (len(huge), len(managed))
    raw[managed[0] : managed[0] + 8] = raw[huge[0] : huge[0] + 8]
    path.write_bytes(bytes(raw))


def build_huge_only(path: Path) -> dict:
    with h5py.File(path, "w", libver="latest") as f:
        one = f.create_dataset("one", data=np.arange(12, dtype="f4").reshape(3, 4), track_times=False)
        one.attrs["big"] = np.bytes_(b"x" * 70000)
        nine = f.create_dataset("nine", data=np.arange(4, dtype="f4"), track_times=False)
        for i in range(9):
            nine.attrs[f"a{i}"] = np.bytes_(bytes([ord("a") + i]) * 5000)
        g = f.create_group("g", track_order=False)
        for i in range(9):
            g.create_group(chr(ord("a") + i) * 5000)
    with h5py.File(path, "r") as f:
        return {
            "source": f"h5py {h5py.__version__} (libhdf5 {h5py.version.hdf5_version}), "
            "libver='latest'",
            "note": "fractal heaps holding only huge objects, with no root block (#907); "
            "string attributes and link names are recorded as (first character, length)",
            "one": {k: [v.decode()[0], len(v)] for k, v in f["one"].attrs.items()},
            "nine": {k: [v.decode()[0], len(v)] for k, v in f["nine"].attrs.items()},
            "g_links": sorted([k[0], len(k)] for k in f["g"].keys()),
        }


def main() -> None:
    nc = FIXTURES / "netcdf4_huge_attributes.nc"
    links = FIXTURES / "hdf5_huge_link_name.h5"
    for path, oracle in ((nc, build_netcdf(nc)), (links, build_links(links))):
        (FIXTURES / f"{path.name}.oracle.json").write_text(
            json.dumps(oracle, indent=2) + "\n", encoding="utf-8"
        )
        print(f"wrote {path} ({path.stat().st_size} B) + oracle")
    only = FIXTURES / "hdf5_huge_only_heaps.h5"
    (FIXTURES / f"{only.name}.oracle.json").write_text(
        json.dumps(build_huge_only(only), indent=2) + "\n", encoding="utf-8"
    )
    print(f"wrote {only} ({only.stat().st_size} B) + oracle")
    shared = FIXTURES / "hdf5_shared_huge_attribute.h5"
    build_shared_huge(shared)
    print(f"wrote {shared} ({shared.stat().st_size} B)")


if __name__ == "__main__":
    main()
