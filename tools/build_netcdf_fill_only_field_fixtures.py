#!/usr/bin/env python3
"""Generate the fill-only field-cap fixtures (#942).

Two NetCDF-4 files, each one chunked float32 variable `t(y, x)` that stores no
chunk, so every value reads as the fill value 1.5 and the file is a few
kilobytes whatever the shape:

  * `fill_only_past_field_cap.nc`: 10000 x 10000, 100 M values. Its whole
    read (100 M x 20 bytes, 2.0 GB) is inside the reader's 2 GiB
    whole-variable budget, and its one plane is the whole variable, past the
    64 Mi values one field may hold. Drawing it must be refused, not
    attempted.
  * `fill_only_at_field_cap.nc`: 8192 x 8192, exactly 64 Mi values: the
    largest plane a read is allowed. Drawing it must complete, on the
    browser's 32-bit target too.

No coordinate variables, so the axes are anonymous and nothing but the field
itself is read. Run from the repo root (needs `netCDF4` + `numpy`):

    python3 tools/build_netcdf_fill_only_field_fixtures.py
"""
from __future__ import annotations

import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
FILL = np.float32(1.5)


def build(path: Path, n: int, chunk: int) -> None:
    if path.exists():
        path.unlink()
    with netCDF4.Dataset(path, "w", format="NETCDF4") as d:
        d.title = "Synthetic fill-only field for the field cap (#942)"
        d.createDimension("y", n)
        d.createDimension("x", n)
        v = d.createVariable(
            "t", "f4", ("y", "x"), zlib=True, chunksizes=(chunk, chunk), fill_value=FILL
        )
        v.units = "K"
    print(f"{path}: {path.stat().st_size} bytes")


def main() -> int:
    if not FIXTURES.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    build(FIXTURES / "fill_only_past_field_cap.nc", 10000, 1000)
    build(FIXTURES / "fill_only_at_field_cap.nc", 8192, 1024)
    return 0


if __name__ == "__main__":
    sys.exit(main())
