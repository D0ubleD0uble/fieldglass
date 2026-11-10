#!/usr/bin/env python3
"""Build the no-degrees coordinate fixture (#1727).

``netcdf_values_no_degrees.nc`` is a classic (CDF-1) file whose 1-D
coordinates hold values no latitude or longitude can, read four ways:

  * ``bare(y, x)``: a projected ``y`` and ``x`` in meters that never had any
    attributes, so they are read as latitude and longitude from their names
    and the reader lost nothing;
  * ``attributed(lat, lon)``: a ``lat`` that states ``degrees_north`` and
    holds 0 to 2000, over a ``lon`` that is a real longitude;
  * ``section(time, lat)``: a time against that same ``lat``. A slice of time
    against latitude is not placed whatever its values, so they are not why.
    ``hovmoller(time, x)`` and ``profile(lat, time)`` are the same with only
    the longitude or only the latitude where placement would read it;
  * ``T2(south_north, west_east)``: a WRF Lambert domain whose 1-D
    ``south_north`` and ``west_east`` read as latitude and longitude by their
    ``axis`` and hold meters. WRF's own globals place it before the 1-D axes
    are read, so their values do not unplace it.

Run from the repo root (needs ``netCDF4`` and ``numpy``):

    python3 tools/build_netcdf_values_no_degrees_fixture.py

The build writes no timestamp or history, so it is reproducible byte for byte.
"""

from __future__ import annotations

from pathlib import Path

import netCDF4
import numpy as np

FIXTURES_DIR = Path("crates/fieldglass-netcdf/tests/fixtures")
NAME = "netcdf_values_no_degrees.nc"


def main() -> None:
    path = FIXTURES_DIR / NAME
    path.unlink(missing_ok=True)
    with netCDF4.Dataset(path, "w", format="NETCDF3_CLASSIC") as nc:
        # WRF's projection, as wrf_lambert.nc states it.
        nc.MAP_PROJ = np.int32(1)
        nc.TRUELAT1 = np.float32(30.0)
        nc.TRUELAT2 = np.float32(60.0)
        nc.STAND_LON = np.float32(-97.5)
        nc.MOAD_CEN_LAT = np.float32(38.5)
        nc.DX = np.float32(30000.0)
        nc.DY = np.float32(30000.0)

        for name, size in [
            ("y", 3),
            ("x", 4),
            ("lat", 3),
            ("lon", 4),
            ("time", 2),
            ("south_north", 3),
            ("west_east", 4),
        ]:
            nc.createDimension(name, size)

        def variable(name, dims, values, dtype="f8", **attrs):
            var = nc.createVariable(name, dtype, dims)
            for key, value in attrs.items():
                var.setncattr(key, value)
            var[:] = np.asarray(values, dtype=dtype)

        variable("y", ("y",), [-1000.0, 0.0, 1000.0])
        variable("x", ("x",), [-2000.0, -1000.0, 1000.0, 2000.0])
        variable("bare", ("y", "x"), np.arange(12.0).reshape(3, 4), "f4")

        variable(
            "lat",
            ("lat",),
            [0.0, 1000.0, 2000.0],
            units="degrees_north",
            standard_name="latitude",
        )
        variable(
            "lon",
            ("lon",),
            [0.0, 90.0, 180.0, 270.0],
            units="degrees_east",
            standard_name="longitude",
        )
        variable("attributed", ("lat", "lon"), np.arange(12.0).reshape(3, 4), "f4")

        variable(
            "time",
            ("time",),
            [0.0, 6.0],
            units="hours since 2000-01-01 00:00:00",
            standard_name="time",
        )
        variable("section", ("time", "lat"), np.arange(6.0).reshape(2, 3), "f4")
        variable("hovmoller", ("time", "x"), np.arange(8.0).reshape(2, 4), "f4")
        variable("profile", ("lat", "time"), np.arange(6.0).reshape(3, 2), "f4")

        variable("south_north", ("south_north",), [-30000.0, 0.0, 30000.0], axis="Y")
        variable(
            "west_east",
            ("west_east",),
            [-45000.0, -15000.0, 15000.0, 45000.0],
            axis="X",
        )
        rows, cols = np.mgrid[0:3, 0:4]
        variable("XLAT", ("south_north", "west_east"), 38.0 + 0.27 * rows, "f4")
        variable("XLONG", ("south_north", "west_east"), -98.0 + 0.34 * cols, "f4")
        variable("T2", ("south_north", "west_east"), 280.0 + rows + cols, "f4")
    print(f"wrote {path}")


if __name__ == "__main__":
    main()
