#!/usr/bin/env python3
"""Generate the NetCDF classic record-layout fixtures and their oracle (#204).

The classic format interleaves record variables: record `r` of a record
variable starts at `begin + r * recsize`. The spec defines `recsize` as the
sum of the record variables' `vsize`, each padded to a multiple of 4 bytes,
with one exception:

    In the special case when there is only one record variable and it is of
    type character, byte, or short, no padding is used between record slabs
    [...] the vsize field is computed to include padding to the next multiple
    of 4 bytes. In this case, readers should ignore vsize and assume no
    padding. Writers should store vsize as if padding were included.

The files here pin both sides of that rule, written by libnetcdf through the
`netCDF4` Python package:

  * `record_single_short_cdf1.nc`, `..._cdf2.nc`, `..._cdf5.nc` hold one
    `short` record variable of 3 values per record. Its `vsize` is 8 on disk
    and its records are 6 bytes apart. All three versions, because the width
    of `vsize` and `begin` differs between them.
  * `record_single_ubyte_cdf5.nc` is the same with a CDF-5 `ubyte`, which the
    spec's sentence predates. libnetcdf packs it the same way.
  * `record_mixed_cdf1.nc` has three record variables, a `char`, a `byte`
    and a `short`, plus a fixed variable. Each record slab is padded, so
    `recsize` is 12, and the `char` variable, which value decode rejects,
    still takes its place in every record.

The oracle records the values `netCDF4` reads back, so the test needs no
netCDF4 at runtime. Run from the repo root (needs `netCDF4` + `numpy`):

    python3 tools/build_netcdf_record_layout_fixtures.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
NUMRECS = 3

SINGLE = {
    "record_single_short_cdf1.nc": ("NETCDF3_CLASSIC", "i2"),
    "record_single_short_cdf2.nc": ("NETCDF3_64BIT_OFFSET", "i2"),
    "record_single_short_cdf5.nc": ("NETCDF3_64BIT_DATA", "i2"),
    "record_single_ubyte_cdf5.nc": ("NETCDF3_64BIT_DATA", "u1"),
}
MIXED = "record_mixed_cdf1.nc"


def build_single(path: Path, fmt: str, dtype: str) -> None:
    if path.exists():
        path.unlink()
    with netCDF4.Dataset(path, "w", format=fmt) as d:
        d.createDimension("time", None)
        d.createDimension("x", 3)
        # No _FillValue attribute: every stored value is data.
        v = d.createVariable("s", dtype, ("time", "x"), fill_value=False)
        v[:] = (np.arange(NUMRECS * 3).reshape(NUMRECS, 3) + 1).astype(dtype)


def build_mixed(path: Path) -> None:
    if path.exists():
        path.unlink()
    with netCDF4.Dataset(path, "w", format="NETCDF3_CLASSIC") as d:
        d.createDimension("time", None)
        d.createDimension("n", 3)
        d.createDimension("one", 1)
        fixed = d.createVariable("f", "f4", ("n",), fill_value=False)
        fixed[:] = np.array([0.5, 1.5, 2.5], dtype="f4")
        label = d.createVariable("label", "S1", ("time", "n"), fill_value=False)
        label[:] = np.array([list(b"abc"), list(b"def"), list(b"ghi")], dtype="u1").view("S1")
        a = d.createVariable("a", "i1", ("time", "n"), fill_value=False)
        a[:] = (np.arange(NUMRECS * 3).reshape(NUMRECS, 3) - 4).astype("i1")
        b = d.createVariable("b", "i2", ("time", "one"), fill_value=False)
        b[:] = np.array([[-300], [301], [302]], dtype="i2")


def values(path: Path) -> dict:
    out = {}
    with netCDF4.Dataset(path) as d:
        for name, v in d.variables.items():
            if v.dtype.kind == "S":
                continue
            arr = np.asarray(v[:]).reshape(-1)
            out[name] = {"shape": list(v.shape), "values": [float(x) for x in arr]}
    return out


def main() -> int:
    if not FIXTURES.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    for name, (fmt, dtype) in SINGLE.items():
        build_single(FIXTURES / name, fmt, dtype)
    build_mixed(FIXTURES / MIXED)
    doc = {
        "source": (
            f"netCDF4 {netCDF4.__version__} (libnetcdf "
            f"{netCDF4.getlibversion().split()[0]}). NetCDF classic record "
            "layout oracle. Self-generated; provenance in NOTICE.md."
        ),
        "files": {name: values(FIXTURES / name) for name in [*SINGLE, MIXED]},
    }
    (FIXTURES / "record_layout.oracle.json").write_text(
        json.dumps(doc, indent=2) + "\n", encoding="utf-8"
    )
    print("wrote the record-layout fixtures and record_layout.oracle.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
