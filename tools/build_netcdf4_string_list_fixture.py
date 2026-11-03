#!/usr/bin/env python3
"""Build the string-list CF attribute fixture and its oracle (#1570).

``netcdf4_string_lists.nc`` is written with the Unidata ``netCDF4`` library.
It holds the two cases #1570 settles:

  * attribute types. ``NC_STRING`` attributes (``setncattr_string``) beside
    ``NC_CHAR`` and numeric ones, so a reader can be held to the type netCDF-C
    reports for each;
  * CF attributes that CF reads as one string, stored as a list of strings. A
    time axis ``time`` whose ``calendar`` is ``["noleap", "360_day"]``, a
    second time axis ``step`` whose ``units`` are two strings, a variable
    ``pr`` whose ``units`` are two strings, and a variable ``tas`` whose
    ``units`` are a one-element list, which is the string itself.

The sibling ``netcdf4_string_lists.oracle.json`` records, for this file and
for ``netcdf4_string_attributes.nc`` (#1549, read here, not rewritten), every
attribute's type as netCDF-C's ``nc_inq_atttype`` reports it, named as
``ncdump`` names it, and its value as netCDF4-python reads it back. The type
is asked of the libnetcdf the ``netCDF4`` wheel loads, through ``ctypes``, as
netCDF4-python has no call that returns an attribute's type.

Run from the repo root (needs ``netCDF4``), under a memory limit as every HDF5
fixture builder is:

    ( ulimit -v 8000000; python3 tools/build_netcdf4_string_list_fixture.py )
"""

from __future__ import annotations

import ctypes
import glob
import json
import os
from pathlib import Path

import netCDF4
import numpy as np

FIXTURES_DIR = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "netcdf4_string_lists.nc"
ORACLE_NAME = "netcdf4_string_lists.oracle.json"
# Read for its types only; built by build_netcdf4_string_attribute_fixtures.py.
EXISTING = "netcdf4_string_attributes.nc"

TIME_VALUES = [0.0, 31.0, 59.0]
STEP_VALUES = [0.0, 6.0]
LAT_VALUES = [10.0, 20.0]
LON_VALUES = [100.0, 110.0, 120.0]

# netCDF-C's external type codes (netcdf.h), named as ncdump prints them.
NC_TYPE_NAMES = {
    1: "byte",
    2: "char",
    3: "short",
    4: "int",
    5: "float",
    6: "double",
    7: "ubyte",
    8: "ushort",
    9: "uint",
    10: "int64",
    11: "uint64",
    12: "string",
}
NC_GLOBAL = -1


def build(path: Path) -> None:
    with netCDF4.Dataset(path, "w", format="NETCDF4") as f:
        f.createDimension("time", len(TIME_VALUES))
        f.createDimension("step", len(STEP_VALUES))
        f.createDimension("lat", len(LAT_VALUES))
        f.createDimension("lon", len(LON_VALUES))

        time = f.createVariable("time", "f8", ("time",))
        time.setncattr_string("units", ["days since 2000-01-01"])
        # Two calendars: CF names one, and no reading of the times is right
        # for both.
        time.setncattr_string("calendar", ["noleap", "360_day"])

        step = f.createVariable("step", "f8", ("step",))
        step.setncattr_string("units", ["hours since 2000-01-01", "days since 2000-01-01"])
        step.calendar = "standard"  # NC_CHAR

        lat = f.createVariable("lat", "f8", ("lat",))
        lat.units = "degrees_north"  # NC_CHAR
        lon = f.createVariable("lon", "f8", ("lon",))
        lon.units = "degrees_east"

        tas = f.createVariable("tas", "f4", ("time", "lat", "lon"))
        tas.setncattr_string("units", ["K"])  # one element: the string itself
        tas.long_name = "air temperature"  # NC_CHAR
        tas.valid_range = np.array([150.0, 350.0], dtype="f4")  # NC_FLOAT

        pr = f.createVariable("pr", "f4", ("step", "lat", "lon"))
        pr.setncattr_string("units", ["kg m-2 s-1", "mm day-1"])
        pr.setncattr_string("long_name", "precipitation")

        f.setncattr_string("source", "tools/build_netcdf4_string_list_fixture.py")
        f.title = "fieldglass NC_STRING list fixture"  # NC_CHAR
        f.version = np.int32(1)  # NC_INT

        time[:] = TIME_VALUES
        step[:] = STEP_VALUES
        lat[:] = LAT_VALUES
        lon[:] = LON_VALUES
        grid = np.ones((len(LAT_VALUES), len(LON_VALUES)), dtype="f4")
        tas[:] = np.array([270.5, 271.5, 272.5], dtype="f4")[:, None, None] * grid
        pr[:] = np.array([0.0, 1.5], dtype="f4")[:, None, None] * grid


def libnetcdf() -> ctypes.CDLL:
    """The libnetcdf the netCDF4 wheel bundles, already loaded by the import."""
    site = Path(netCDF4.__file__).resolve().parent.parent
    found = sorted(glob.glob(str(site / "netcdf4.libs" / "libnetcdf*.so*")))
    if not found:
        raise SystemExit("no bundled libnetcdf found beside the netCDF4 package")
    lib = ctypes.CDLL(found[0])
    lib.nc_open.argtypes = [ctypes.c_char_p, ctypes.c_int, ctypes.POINTER(ctypes.c_int)]
    lib.nc_inq_varid.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.POINTER(ctypes.c_int)]
    lib.nc_inq_atttype.argtypes = [
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_char_p,
        ctypes.POINTER(ctypes.c_int),
    ]
    lib.nc_close.argtypes = [ctypes.c_int]
    return lib


def check(status: int, what: str) -> None:
    if status != 0:
        raise SystemExit(f"netCDF-C {what} failed with status {status}")


def value_of(obj, name: str):
    value = obj.getncattr(name)
    if isinstance(value, np.ndarray):
        value = value.tolist()
    if isinstance(value, np.generic):
        value = value.item()
    if isinstance(value, list):
        value = [v if isinstance(v, (int, float)) else str(v) for v in value]
    return value


def oracle_of(path: Path, lib: ctypes.CDLL) -> dict:
    ncid = ctypes.c_int()
    check(lib.nc_open(str(path).encode(), 0, ctypes.byref(ncid)), f"nc_open({path})")
    try:

        def attributes(obj, varid: int) -> dict:
            out = {}
            for name in obj.ncattrs():
                code = ctypes.c_int()
                check(
                    lib.nc_inq_atttype(ncid, varid, name.encode(), ctypes.byref(code)),
                    f"nc_inq_atttype({name})",
                )
                out[name] = {"type": NC_TYPE_NAMES[code.value], "value": value_of(obj, name)}
            return out

        with netCDF4.Dataset(path, "r") as f:
            variables = {}
            for name, v in f.variables.items():
                varid = ctypes.c_int()
                check(lib.nc_inq_varid(ncid, name.encode(), ctypes.byref(varid)), name)
                variables[name] = attributes(v, varid.value)
            return {"global": attributes(f, NC_GLOBAL), "variables": variables}
    finally:
        lib.nc_close(ncid)


def main() -> int:
    if not FIXTURES_DIR.is_dir():
        raise SystemExit("run from the repo root")
    path = FIXTURES_DIR / NAME
    build(path)
    lib = libnetcdf()
    data = {
        "source": (
            f"netCDF4 {netCDF4.__version__} "
            f"(libnetcdf {netCDF4.__netcdf4libversion__}, "
            f"HDF5 {netCDF4.__hdf5libversion__}); types from nc_inq_atttype"
        ),
        NAME: oracle_of(path, lib),
        EXISTING: oracle_of(FIXTURES_DIR / EXISTING, lib),
    }
    (FIXTURES_DIR / ORACLE_NAME).write_text(
        json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    print(f"wrote {path} ({os.path.getsize(path)} B)")
    print(f"oracle: {data['source']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
