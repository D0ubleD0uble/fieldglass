#!/usr/bin/env python3
"""Generate the CF `missing_value` masking fixtures and their oracle.

libnetcdf masks a value equal to either `_FillValue` **or** the CF
`missing_value` attribute. Fieldglass decode previously masked only
`_FillValue`, so a field that marks gaps with `missing_value` (common in older
COARDS / GrADS / satellite products) rendered its sentinels as real data.

`temp(y, x)` is an `int16` carrying a distinct `_FillValue` and a scalar
`missing_value`, with points hitting each. It is written in both on-disk
encodings so each decode backing is exercised end-to-end:

  * `missing_value_classic.nc` — NetCDF-3 classic (the classic decoder)
  * `missing_value_nc4.nc`     — NetCDF-4 / HDF5 (the HDF5 decoder)

The sibling oracle records the values `netCDF4` produces with auto-mask on (the
masked array), so reproducing it is a cross-tool check. No `scale_factor` is
set — this targets masking only. Run from the repo root (needs `netCDF4` +
`numpy`):

    python3 tools/build_netcdf_missing_value_fixture.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")

FILL = np.int16(-9999)
MISSING = np.int16(-8888)
# (y, x) = (2, 3). Position 1 is the _FillValue, positions 3 and 5 the
# missing_value; the rest are real data.
PACKED = np.array([[10, -9999, 20], [-8888, 30, -8888]], dtype="i2")


def build(path: Path, fmt: str) -> None:
    if path.exists():
        path.unlink()
    ny, nx = PACKED.shape
    with netCDF4.Dataset(path, "w", format=fmt) as d:
        d.title = "Synthetic CF missing_value masking fixture"
        d.createDimension("y", ny)
        d.createDimension("x", nx)
        v = d.createVariable("temp", "i2", ("y", "x"), fill_value=FILL)
        v.units = "kelvin"
        v.missing_value = MISSING
        v.set_auto_maskandscale(False)  # write raw codes verbatim
        v[:] = PACKED


def oracle() -> dict:
    # Read back through libnetcdf's auto mask — the masked physical array.
    with netCDF4.Dataset(FIXTURES / "missing_value_classic.nc") as d:
        v = d.variables["temp"]  # auto mask on by default
        arr = v[:]
        mask = np.ma.getmaskarray(arr).reshape(-1)
        flat = np.asarray(arr.filled(0)).reshape(-1)
        values = [None if m else float(x) for x, m in zip(flat, mask)]
    return {
        "shape": list(PACKED.shape),
        "fill_value": float(FILL),
        "missing_value": float(MISSING),
        "values": values,
    }


def main() -> int:
    if not FIXTURES.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    build(FIXTURES / "missing_value_classic.nc", "NETCDF3_CLASSIC")
    build(FIXTURES / "missing_value_nc4.nc", "NETCDF4")
    doc = {
        "source": (
            f"netCDF4 {netCDF4.__version__} (libnetcdf "
            f"{netCDF4.getlibversion().split()[0]}). CF missing_value masking "
            "oracle. Self-generated; provenance in NOTICE.md."
        ),
        "temp": oracle(),
    }
    (FIXTURES / "missing_value.oracle.json").write_text(
        json.dumps(doc, indent=2) + "\n"
    )
    print("wrote missing_value_classic.nc, missing_value_nc4.nc and the oracle")
    return 0


if __name__ == "__main__":
    sys.exit(main())
