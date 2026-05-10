#!/usr/bin/env python3
"""Reproduce the CDF-2 / CDF-5 ERSST test fixtures from the upstream CDF-1
NOAA file. Run from this directory:

    python3 build_fixtures.py

Requires the canonical Unidata `netCDF4` Python bindings (which wrap
`libnetcdf`). The script downloads the upstream NCEI file once and re-encodes
it into each on-disk classic variant. See NOTICE.md for provenance.
"""
from __future__ import annotations

import os
import sys
import urllib.request
from pathlib import Path

import netCDF4

UPSTREAM = "https://www.ncei.noaa.gov/pub/data/cmb/ersst/v5/netcdf/ersst.v5.187001.nc"
HERE = Path(__file__).resolve().parent
SRC = HERE / "ersst_v5_187001_cdf1.nc"


def fetch_upstream() -> None:
    if SRC.exists():
        return
    print(f"downloading {UPSTREAM} -> {SRC}")
    urllib.request.urlretrieve(UPSTREAM, SRC)


def transcode(src: Path, dst: Path, fmt: str) -> None:
    with netCDF4.Dataset(src, "r") as s, netCDF4.Dataset(dst, "w", format=fmt) as d:
        d.setncatts({k: s.getncattr(k) for k in s.ncattrs()})
        for name, dim in s.dimensions.items():
            d.createDimension(name, len(dim) if not dim.isunlimited() else None)
        for name, v in s.variables.items():
            nv = d.createVariable(name, v.dtype, v.dimensions)
            nv.setncatts({k: v.getncattr(k) for k in v.ncattrs()})
            nv[:] = v[:]
    print(f"  {fmt:<22} {dst.name:<32} {os.path.getsize(dst)} bytes")


def main() -> int:
    fetch_upstream()
    transcode(SRC, HERE / "ersst_v5_187001_cdf2.nc", "NETCDF3_64BIT_OFFSET")
    transcode(SRC, HERE / "ersst_v5_187001_cdf5.nc", "NETCDF3_64BIT_DATA")
    return 0


if __name__ == "__main__":
    sys.exit(main())
