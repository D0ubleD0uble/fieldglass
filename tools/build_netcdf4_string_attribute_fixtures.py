#!/usr/bin/env python3
"""Build the variable-length string attribute fixtures and their oracle (#1549).

A NetCDF-4 attribute of type ``NC_STRING`` is an HDF5 variable-length string
(datatype class 9, type 1): the attribute holds one global-heap reference per
element, and the text lives in a global-heap collection. netCDF4-python writes
one through ``setncattr_string``; h5netcdf and h5py write every Python ``str``
attribute that way, so in their files ``units``, ``long_name`` and ``calendar``
are all ``NC_STRING``.

This writes two small files:

  * ``netcdf4_string_attributes.nc``, with the canonical Unidata ``netCDF4``
    library: ``NC_STRING`` global and variable attributes beside ``NC_CHAR``
    ones, a ``calendar = "noleap"`` time axis (the #1033 case) under a small
    ``temperature(time, lat, lon)`` map a host can draw, an empty
    string, non-ASCII UTF-8 text, a multi-element string array, and a variable
    with enough attributes that netCDF-C moves them to dense storage.
  * ``h5py_string_attributes.h5``, with ``h5py``, the h5netcdf / h5py style:
    a scalar variable-length string per Python ``str`` attribute, a one-element
    array, and a variable-length and a fixed-length string array.

The sibling ``netcdf4_string_attributes.oracle.json`` records every attribute
as ``netCDF4`` reads it back: a ``str``, or a list of ``str`` for an array.

Run from the repo root (needs ``netCDF4`` and ``h5py``), under a memory limit
as every HDF5 fixture builder is:

    ( ulimit -v 8000000; python3 tools/build_netcdf4_string_attribute_fixtures.py )
"""

from __future__ import annotations

import json
from pathlib import Path

import h5py
import netCDF4
import numpy as np

FIXTURES_DIR = Path("crates/fieldglass-netcdf/tests/fixtures")
NETCDF4_NAME = "netcdf4_string_attributes.nc"
H5PY_NAME = "h5py_string_attributes.h5"
ORACLE_NAME = "netcdf4_string_attributes.oracle.json"

TIME_VALUES = [0.0, 1.0, 2.0]
TEMPERATURE_VALUES = [270.5, 271.5, 272.5]
LAT_VALUES = [10.0, 20.0]
LON_VALUES = [100.0, 110.0, 120.0]

# More attributes than netCDF-C keeps in the object header (8), so the
# variable's attributes sit in a fractal heap and B-tree rather than inline.
DENSE_COUNT = 12


def build_netcdf4(path: Path) -> None:
    with netCDF4.Dataset(path, "w", format="NETCDF4") as f:
        f.createDimension("time", len(TIME_VALUES))

        time = f.createVariable("time", "f8", ("time",))
        time.units = "days since 2000-01-01"  # NC_CHAR beside the NC_STRING
        time.setncattr_string("calendar", "noleap")
        time.setncattr_string("standard_name", "time")
        time.setncattr_string("comment", "")

        # A small map per time step, so a host has a field whose time axis it
        # labels from `time`, and reads its calendar (#1033).
        f.createDimension("lat", len(LAT_VALUES))
        f.createDimension("lon", len(LON_VALUES))
        lat = f.createVariable("lat", "f8", ("lat",))
        lat.setncattr_string("units", "degrees_north")
        lon = f.createVariable("lon", "f8", ("lon",))
        lon.setncattr_string("units", "degrees_east")

        temperature = f.createVariable("temperature", "f4", ("time", "lat", "lon"))
        temperature.setncattr_string("units", "K")
        temperature.setncattr_string("long_name", "température de l’air")
        temperature.setncattr_string(
            "flag_meanings_list", ["good", "suspect, flagged", 'quoted "bad"']
        )

        dense = f.createVariable("dense", "f4", ("time",))
        for i in range(DENSE_COUNT):
            dense.setncattr_string(f"note_{i:02}", f"dense note {i}")

        f.setncattr_string("title", "fieldglass NC_STRING attribute fixture")
        f.history = "written by tools/build_netcdf4_string_attribute_fixtures.py"
        f.setncattr_string("keywords", ["one", "two"])

        time[:] = TIME_VALUES
        lat[:] = LAT_VALUES
        lon[:] = LON_VALUES
        temperature[:] = np.array(TEMPERATURE_VALUES, dtype="f4")[:, None, None] * np.ones(
            (len(LAT_VALUES), len(LON_VALUES)), dtype="f4"
        )
        dense[:] = np.array(TEMPERATURE_VALUES, dtype="f4")


def build_h5py(path: Path) -> None:
    vlen = h5py.string_dtype()
    # The fixed libver keeps the file stable across h5py releases.
    with h5py.File(path, "w", libver=("earliest", "v108")) as f:
        f.attrs["title"] = "fieldglass h5py string attribute fixture"
        f.attrs["one_element"] = np.array(["solo"], dtype=vlen)
        f.attrs["vlen_list"] = np.array(["x", "yz"], dtype=vlen)
        f.attrs["fixed_list"] = np.array([b"ab", b"cde"])

        time = f.create_dataset("time", data=np.array(TIME_VALUES))
        time.attrs["units"] = "days since 2000-01-01"
        time.attrs["calendar"] = "noleap"

        temperature = f.create_dataset(
            "temperature", data=np.array(TEMPERATURE_VALUES, dtype="f4")
        )
        temperature.attrs["units"] = "K"
        temperature.attrs["long_name"] = "air temperature"


def attributes(obj) -> dict:
    out = {}
    for name in obj.ncattrs():
        value = obj.getncattr(name)
        if isinstance(value, np.ndarray):
            value = value.tolist()
        if isinstance(value, list):
            value = [str(v) for v in value]
        out[name] = value
    return out


def oracle_of(path: Path) -> dict:
    with netCDF4.Dataset(path, "r") as f:
        return {
            "global": attributes(f),
            "variables": {name: attributes(v) for name, v in f.variables.items()},
        }


def main() -> int:
    if not FIXTURES_DIR.is_dir():
        raise SystemExit("run from the repo root")
    nc = FIXTURES_DIR / NETCDF4_NAME
    h5 = FIXTURES_DIR / H5PY_NAME
    build_netcdf4(nc)
    build_h5py(h5)
    data = {
        "source": (
            f"netCDF4 {netCDF4.__version__} "
            f"(libnetcdf {netCDF4.__netcdf4libversion__}, "
            f"HDF5 {netCDF4.__hdf5libversion__}), "
            f"h5py {h5py.__version__} (HDF5 {h5py.version.hdf5_version})"
        ),
        NETCDF4_NAME: oracle_of(nc),
        H5PY_NAME: oracle_of(h5),
    }
    (FIXTURES_DIR / ORACLE_NAME).write_text(
        json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    for path in (nc, h5):
        print(f"wrote {path} ({path.stat().st_size} B)")
    print(f"oracle: {data['source']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
