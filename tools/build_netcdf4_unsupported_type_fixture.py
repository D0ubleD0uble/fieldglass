#!/usr/bin/env python3
"""Build the NetCDF-4 unsupported-datatype fixture and its oracle (#550).

`crates/fieldglass-netcdf/src/hdf5/datatype.rs` decodes the three datatype
classes NetCDF-4 climate data is written in — fixed-point, IEEE floating point,
and fixed-length string. Compound, enum, variable-length, opaque and array
datatypes are outside that subset. A file that mixes the two is ordinary: a
station-record file, an OMI or TROPOMI granule, anything netCDF-C wrote through
``nc_def_compound``. Before #550 one such dataset failed metadata resolution for
the *whole* file, so the editor showed nothing rather than everything but the
one variable.

This writes the smallest file that reproduces that mix with the canonical
Unidata ``netCDF4`` library (which lays down the real dimension-scale
machinery), plus a sibling oracle JSON the Rust test pins against:

  * ``station_info(station)`` — an HDF5 **compound** datatype (class 6),
  * ``visits(station)`` — an HDF5 **variable-length** datatype (class 9),
  * ``time(time)`` — a plain ``double`` coordinate variable,
  * ``temperature(time)`` — a plain ``float`` data variable,
  * ``station`` — a pure dimension (no coordinate variable).

The two undecodable datasets are written **first** so that both sit at a lower
whole-file dataset index than ``temperature``: skipping them must not shift
``temperature``'s decode index, which is what the Rust test checks by decoding
its values through that index.

Run from the repo root (needs ``netCDF4``):

    python3 tools/build_netcdf4_unsupported_type_fixture.py
"""

from __future__ import annotations

import json
from pathlib import Path

import netCDF4
import numpy as np

FIXTURES_DIR = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "netcdf4_unsupported_type.nc"

TIME_VALUES = [0.0, 6.0, 12.0, 18.0]
TEMPERATURE_VALUES = [270.5, 271.5, 272.5, 273.5]

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


def build(path: Path) -> None:
    with netCDF4.Dataset(path, "w", format="NETCDF4") as f:
        f.createDimension("time", len(TIME_VALUES))
        f.createDimension("station", 3)  # pure dimension: no coordinate variable

        # The two datatypes outside the decoded subset, and their variables,
        # created before the plain ones so they take the lower dataset indices.
        compound = f.createCompoundType(
            np.dtype([("id", "i4"), ("lat", "f8"), ("lon", "f8")]), "station_record"
        )
        sequence = f.createVLType(np.int32, "int_sequence")

        info = f.createVariable("station_info", compound, ("station",))
        info.long_name = "station identity and position"

        visits = f.createVariable("visits", sequence, ("station",))
        visits.long_name = "observation counts per station"

        time = f.createVariable("time", "f8", ("time",))
        time.units = "hours since 2021-03-01 00:00:00"
        time.standard_name = "time"
        time.axis = "T"

        temperature = f.createVariable("temperature", "f4", ("time",))
        temperature.units = "K"
        temperature.standard_name = "air_temperature"

        info[:] = np.array(
            [(1, 10.0, 20.0), (2, 11.0, 21.0), (3, 12.0, 22.0)],
            dtype=compound.dtype_view,
        )
        visits[0] = np.array([1, 2], dtype="i4")
        visits[1] = np.array([3], dtype="i4")
        visits[2] = np.array([4, 5, 6], dtype="i4")
        time[:] = TIME_VALUES
        temperature[:] = np.array(TEMPERATURE_VALUES, dtype="f4")

        f.title = "fieldglass NetCDF-4 unsupported-datatype fixture"


def nc_type_of(var: netCDF4.Variable) -> str:
    """The netCDF type name, or the user-defined type's kind for a compound /
    vlen / enum variable — which is exactly the set the Rust reader declines."""
    dtype = var.datatype
    if isinstance(dtype, netCDF4.CompoundType):
        return "compound"
    if isinstance(dtype, netCDF4.VLType):
        return "vlen"
    if isinstance(dtype, netCDF4.EnumType):
        return "enum"
    return NC_TYPE_NAME[dtype.name]


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
                {"name": name, "length": len(dim), "unlimited": dim.isunlimited()}
                for name, dim in f.dimensions.items()
            ],
            "variables": [
                {
                    "name": name,
                    "nc_type": nc_type_of(var),
                    "dimensions": list(var.dimensions),
                    "is_coordinate": name in coord_names,
                    # The three classes `hdf5/datatype.rs` decodes; everything
                    # else is what #550 skips and reports rather than failing on.
                    "decodable_datatype": nc_type_of(var)
                    not in ("compound", "vlen", "enum"),
                }
                for name, var in f.variables.items()
            ],
            "values": {
                "time": list(TIME_VALUES),
                "temperature": list(TEMPERATURE_VALUES),
            },
        }


def main() -> int:
    if not FIXTURES_DIR.is_dir():
        raise SystemExit("run from the repo root")
    path = FIXTURES_DIR / NAME
    build(path)
    data = oracle(path)
    (FIXTURES_DIR / f"{NAME}.oracle.json").write_text(
        json.dumps(data, indent=2) + "\n", encoding="utf-8"
    )
    size = path.stat().st_size
    print(f"wrote {path} ({size} B) + oracle [{data['format']}, {data['source']}]")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
