#!/usr/bin/env python3
"""Generate the large, sparse NetCDF-4 fixture for region reads (#939).

`t2m(time, lat, lon)` is float32, 120 x 721 x 1440: a 0.25 degree global grid,
five days hourly, 124,588,800 values. Decoded whole that is about 2.5 GB (16
bytes of output and 4 stored per value), past the reader's 2 GiB whole-variable
budget, which is the point: a viewer must still draw one plane of it, reading
that plane and the one chunk under it.

The file is small because almost none of it is stored. Each chunk is one time
step, deflated, and only two are written:

  * time step 7: the value at row j is the latitude rounded to a whole degree,
    the same along the row, so it deflates to a few kilobytes;
  * time step 119: the same plus 100, so the last chunk of the index is real.

Every other chunk is never allocated and reads as the `_FillValue`, -999,
which CF masks. The time, latitude and longitude coordinates are written in
full, so the slice places as a regular global grid.

Run from the repo root (needs `netCDF4` + `numpy`):

    python3 tools/build_netcdf_large_sparse_fixture.py
"""
from __future__ import annotations

import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
OUT = FIXTURES / "netcdf4_large_sparse.nc"

NT, NY, NX = 120, 721, 1440
FILL = np.float32(-999.0)
WRITTEN = {7: 0.0, 119: 100.0}


def main() -> int:
    if not FIXTURES.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    if OUT.exists():
        OUT.unlink()
    lat = np.linspace(90.0, -90.0, NY)
    lon = np.arange(NX) * 0.25
    plane = np.repeat(np.round(lat).astype("f4")[:, None], NX, axis=1)
    with netCDF4.Dataset(OUT, "w", format="NETCDF4") as d:
        d.title = "Synthetic large sparse field for region reads (#939)"
        d.createDimension("time", NT)
        d.createDimension("lat", NY)
        d.createDimension("lon", NX)
        t = d.createVariable("time", "f8", ("time",))
        t.units = "hours since 2020-01-01 00:00:00"
        t.standard_name = "time"
        t[:] = np.arange(NT, dtype="f8")
        y = d.createVariable("lat", "f8", ("lat",))
        y.units = "degrees_north"
        y.standard_name = "latitude"
        y[:] = lat
        x = d.createVariable("lon", "f8", ("lon",))
        x.units = "degrees_east"
        x.standard_name = "longitude"
        x[:] = lon
        v = d.createVariable(
            "t2m",
            "f4",
            ("time", "lat", "lon"),
            zlib=True,
            complevel=9,
            chunksizes=(1, NY, NX),
            fill_value=FILL,
        )
        v.units = "K"
        for step, offset in WRITTEN.items():
            v[step] = plane + np.float32(offset)
    print(f"{OUT}: {OUT.stat().st_size} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
