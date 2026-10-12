#!/usr/bin/env python3
"""Generate the NetCDF classic `numrecs` fixtures (#1000, #1008).

A classic header states its record count, `numrecs`, before anything in the
file shows that many records are there. The reader used to build one byte
range per stated record before checking the ranges against the file, so a file
of a few hundred bytes could make it allocate gigabytes first. These files pin
that the check now comes first, and that a streaming file reads the whole
records it holds.

Each starts as the same small file written by libnetcdf (through the
`netCDF4` Python package): an unlimited `time` dimension, `x = 1`, a `double`
coordinate `time(time)`, a `byte` variable `v(time, x)` and a fixed `float`
variable `f(x)`, with 2 records written. `recsize` is 12 (8 for `time`, 1 for
`v` padded to 4). Then only the `numrecs` field is changed:

  * `numrecs_huge_cdf1.nc`, `numrecs_huge_cdf5.nc`: `numrecs` = 60,000,000.
    Both record variables then fit the whole-variable budget (60 M doubles is
    1.44 GB decoded, 60 M bytes 1.02 GB) and the 64 Mi-value field cap, so
    nothing refuses them before their ranges are planned. The file holds 2
    records, so a read of record 0 or 1 is valid and any read past them is
    refused. CDF-5 states `numrecs` in 64 bits, CDF-1 in 32.
  * `numrecs_streaming_cdf1.nc`, `numrecs_streaming_cdf5.nc`: `numrecs` is
    the specification's STREAMING value, all bits set (`0xFFFFFFFF` in CDF-1,
    `0xFFFFFFFFFFFFFFFF` in CDF-5), "indeterminate record count". libnetcdf
    writes no such file, so it is made the same way.
  * `numrecs_streaming_partial_cdf1.nc`, `numrecs_streaming_partial_cdf5.nc`:
    the streaming file with its last 5 bytes cut off, as a write interrupted
    part way through record 1 leaves it. Record 1 then has 7 of its 12 bytes,
    part of its `time` value and none of its `v`, so the file holds 1 whole
    record.

The patch is one fixed-width field at byte 4, after the magic. The build is
reproducible byte for byte. Run from the repo root (needs `netCDF4` +
`numpy`):

    python3 tools/build_netcdf_numrecs_fixtures.py
"""
from __future__ import annotations

import sys
import tempfile
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
HUGE = 60_000_000
# Bytes cut off the end of the streaming file for the partial-record files:
# fewer than the 12 of one record, more than the 4 of `v`'s padded slab.
CUT = 5

FORMATS = {
    "cdf1": ("NETCDF3_CLASSIC", 4),
    "cdf5": ("NETCDF3_64BIT_DATA", 8),
}


def base(fmt: str) -> bytes:
    """The libnetcdf-written file with its 2 real records."""
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "base.nc"
        with netCDF4.Dataset(path, "w", format=fmt) as d:
            d.createDimension("time", None)
            d.createDimension("x", 1)
            t = d.createVariable("time", "f8", ("time",), fill_value=False)
            t.units = "hours since 2020-01-01 00:00:00"
            v = d.createVariable("v", "i1", ("time", "x"), fill_value=False)
            f = d.createVariable("f", "f4", ("x",), fill_value=False)
            t[:] = np.array([0.0, 6.0])
            v[:] = np.array([[7], [-8]], dtype="i1")
            f[:] = np.array([0.5], dtype="f4")
        return path.read_bytes()


def with_numrecs(data: bytes, width: int, numrecs: int) -> bytes:
    stated = int.from_bytes(data[4 : 4 + width], "big")
    if stated != 2:
        raise SystemExit(f"expected numrecs 2 at byte 4, found {stated}")
    return data[:4] + numrecs.to_bytes(width, "big") + data[4 + width :]


def main() -> int:
    if not FIXTURES.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    for tag, (fmt, width) in FORMATS.items():
        data = base(fmt)
        for kind, numrecs in [("huge", HUGE), ("streaming", (1 << (8 * width)) - 1)]:
            path = FIXTURES / f"numrecs_{kind}_{tag}.nc"
            path.write_bytes(with_numrecs(data, width, numrecs))
            print(f"{path}: {path.stat().st_size} bytes, numrecs {numrecs:#x}")
        streaming = with_numrecs(data, width, (1 << (8 * width)) - 1)
        path = FIXTURES / f"numrecs_streaming_partial_{tag}.nc"
        path.write_bytes(streaming[:-CUT])
        print(f"{path}: {path.stat().st_size} bytes, streaming, cut short")
    return 0


if __name__ == "__main__":
    sys.exit(main())
