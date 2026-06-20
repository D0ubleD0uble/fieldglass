#!/usr/bin/env python3
"""Build the NetCDF-4 dimension-scale resolution fixture and its oracle (#174).

Decision 0003 resolves NetCDF-4's HDF5 dimension-scale convention into named,
shared dimensions and per-variable ordered dimension lists. The two hand-built
``hdf5_*`` fixtures are pure ``h5py`` and carry **no** dimension scales, so they
can't exercise the semantic layer. This writes a small but representative
NetCDF-4 file with the canonical Unidata ``netCDF4`` library (which lays down the
real ``CLASS=DIMENSION_SCALE`` / ``DIMENSION_LIST`` / ``_Netcdf4Dimid`` machinery)
plus a sibling oracle JSON for the Rust test to pin against — ``ncdump -h`` in
JSON form.

It deliberately covers every classification the resolver must make:

  * an **unlimited** dimension with a coordinate variable (``time``),
  * regular dimensions with coordinate variables (``lat`` / ``lon``),
  * a **pure dimension** with no coordinate variable (``nv``) — the
    ``"This is a netCDF dimension but not a netCDF variable."`` placeholder,
  * a multi-dimensional **data variable** whose ``DIMENSION_LIST`` must resolve
    to ordered names (``temperature(time, lat, lon)``),
  * a data variable that references the pure dimension (``lat_bnds(lat, nv)``).

Run from the repo root (needs ``netCDF4``):

    python3 tools/build_netcdf4_dimscale_fixture.py
"""
from __future__ import annotations

import json
from pathlib import Path

import netCDF4
import numpy as np

FIXTURES_DIR = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "netcdf4_dimscale.nc"


def build(path: Path) -> None:
    with netCDF4.Dataset(path, "w", format="NETCDF4") as f:
        f.createDimension("time", None)  # unlimited
        f.createDimension("lat", 3)
        f.createDimension("lon", 4)
        f.createDimension("nv", 2)  # pure dimension: no coordinate variable

        time = f.createVariable("time", "f8", ("time",))
        time.units = "hours since 2020-01-01 00:00:00"
        time.standard_name = "time"
        time.axis = "T"

        lat = f.createVariable("lat", "f8", ("lat",))
        lat.units = "degrees_north"
        lat.standard_name = "latitude"

        lon = f.createVariable("lon", "f8", ("lon",))
        lon.units = "degrees_east"
        lon.standard_name = "longitude"

        temp = f.createVariable(
            "temperature", "f4", ("time", "lat", "lon"), fill_value=np.float32(-9999.0)
        )
        temp.units = "K"
        temp.standard_name = "air_temperature"

        bnds = f.createVariable("lat_bnds", "f8", ("lat", "nv"))

        time[:] = [0.0, 6.0]
        lat[:] = [-10.0, 0.0, 10.0]
        lon[:] = [0.0, 90.0, 180.0, 270.0]
        temp[:] = np.arange(2 * 3 * 4, dtype="f4").reshape(2, 3, 4)
        bnds[:] = np.array([[-15, -5], [-5, 5], [5, 15]], dtype="f8")

        f.title = "fieldglass NetCDF-4 dimension-scale fixture"


# numpy dtype name -> the netCDF type name the Rust reader reports
# (`NcType::name()`), so the oracle pins the canonical netCDF type, not numpy's.
NC_TYPE_NAME = {
    "int8": "byte",
    "uint8": "ubyte",
    "int16": "short",
    "uint16": "ushort",
    "int32": "int",
    "uint32": "uint",
    "int64": "int64",
    "uint64": "uint64",
    "float32": "float",
    "float64": "double",
}


def oracle(path: Path) -> dict:
    with netCDF4.Dataset(path, "r") as f:
        coord_names = {n for n in f.variables if n in f.dimensions}
        return {
            "source": (
                f"netCDF4 {netCDF4.__version__} "
                f"(libnetcdf {netCDF4.__netcdf4libversion__}, "
                f"HDF5 {netCDF4.__hdf5libversion__})"
            ),
            "format": f.data_model,
            "dimensions": [
                {
                    "name": name,
                    "length": len(dim),
                    "unlimited": dim.isunlimited(),
                }
                for name, dim in f.dimensions.items()
            ],
            "variables": [
                {
                    "name": name,
                    "nc_type": NC_TYPE_NAME[var.dtype.name],
                    "dimensions": list(var.dimensions),
                    "is_coordinate": name in coord_names,
                }
                for name, var in f.variables.items()
            ],
        }


def main() -> int:
    if not FIXTURES_DIR.is_dir():
        raise SystemExit("run from the repo root")
    path = FIXTURES_DIR / NAME
    build(path)
    data = oracle(path)
    (FIXTURES_DIR / f"{NAME}.oracle.json").write_text(json.dumps(data, indent=2) + "\n")
    size = path.stat().st_size
    print(f"wrote {path} ({size} B) + oracle [{data['format']}, {data['source']}]")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
