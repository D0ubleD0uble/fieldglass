#!/usr/bin/env python3
"""Build the numeric-calendar fixture (`netcdf_numeric_calendar.nc`, #1645).

CF 4.4.1 makes a time's ``calendar`` attribute a string. This file breaks that
on purpose, in the two shapes a numeric attribute takes in a classic NetCDF
file:

  * ``time(time)`` has ``int calendar = 360``, one number;
  * ``time2(time2)`` has ``int calendar = 1, 2``, a list of numbers.

Both are ``days since 2000-01-01`` with the values ``[0, 360]``, so on the
standard calendar the second would read 2000-12-26, and on ``360_day`` it
would be 2001-01-01. A reader cannot tell which calendar a number means, so
Fieldglass shows the numbers and says why. ``t(time, lat, lon)`` and
``u(time2, lat, lon)`` are drawn over them.

Self-generated (no upstream provenance or licensing constraint). Run from the
repo root (needs ``netCDF4`` and ``numpy``):

    python3 tools/build_netcdf_numeric_calendar_fixture.py
"""

from __future__ import annotations

from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

NC = Path("crates/fieldglass-netcdf/tests/fixtures/netcdf_numeric_calendar.nc")

TIME_VALUES = [0.0, 360.0]
LAT_VALUES = [10.0, 20.0]
LON_VALUES = [30.0, 40.0]

# (time variable, its numeric calendar, the field drawn over it)
TIMES = [
    ("time", np.int32(360), "t"),
    ("time2", np.array([1, 2], dtype="i4"), "u"),
]


def build() -> None:
    if NC.exists():
        NC.unlink()
    with netCDF4.Dataset(NC, "w", format="NETCDF3_CLASSIC") as d:
        d.title = "fieldglass numeric calendar fixture (#1645)"
        d.createDimension("lat", len(LAT_VALUES))
        d.createDimension("lon", len(LON_VALUES))
        lat = d.createVariable("lat", "f8", ("lat",))
        lat.units = "degrees_north"
        lat.standard_name = "latitude"
        lat[:] = LAT_VALUES
        lon = d.createVariable("lon", "f8", ("lon",))
        lon.units = "degrees_east"
        lon.standard_name = "longitude"
        lon[:] = LON_VALUES
        for name, calendar, field in TIMES:
            d.createDimension(name, len(TIME_VALUES))
            time = d.createVariable(name, "f8", (name,))
            time.units = "days since 2000-01-01"
            time.standard_name = "time"
            time.calendar = calendar
            time[:] = TIME_VALUES
            data = d.createVariable(field, "f4", (name, "lat", "lon"))
            data.units = "K"
            data[:] = np.arange(8, dtype="f4").reshape(2, 2, 2) + 270.0


def check() -> None:
    with netCDF4.Dataset(NC) as d:
        for name, calendar, _ in TIMES:
            got = d[name].getncattr("calendar")
            assert np.array_equal(np.atleast_1d(got), np.atleast_1d(calendar)), (name, got)
            assert d[name].getncattr("calendar").dtype == np.int32, name


if __name__ == "__main__":
    build()
    check()
    print(f"wrote {NC} ({NC.stat().st_size} bytes)")
