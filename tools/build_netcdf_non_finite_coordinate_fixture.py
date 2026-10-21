#!/usr/bin/env python3
"""Generate `non_finite_coordinates.nc`: 1-D coordinates holding NaN or infinity.

CF (section 1.3, after the NUG) requires a coordinate variable's values to be
strictly monotonic and allows it no missing values. A NaN or an infinity
breaks both, so the slices these coordinates span cannot be placed. Before
#1172 an all-NaN latitude came back as a lat/lon placement with NaN corners,
and a NaN in the middle of a latitude was placed from its finite ends as if
nothing were wrong.

Every value is written as itself. Only `lat_fill` carries a `_FillValue`, so a
reader that masks fill values still sees every NaN.

  * `all_nan(lat_nan, lon)`        every latitude NaN, as a coordinate read from
                                   chunks a store never wrote is when its fill
                                   value is NaN
  * `hole(lat_hole, lon)`          finite ends, NaN in the middle
  * `infinite_end(lat, lon_inf)`   a longitude ending in +infinity
  * `placed(lat, lon)`             finite coordinates, the control
  * `masked(lat_fill, lon)`        a latitude whose `_FillValue` (-999) sits in
                                   the middle: masked as it is read, and refused
                                   the way the NaN is

NetCDF-3 classic, 3 x 4 float32 fields. Run from the repo root (needs
`netCDF4` + `numpy`):

    python3 tools/build_netcdf_non_finite_coordinate_fixture.py
"""
from __future__ import annotations

import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

FIXTURE = Path("crates/fieldglass-netcdf/tests/fixtures/non_finite_coordinates.nc")

LATITUDES = {
    "lat": [10.0, 20.0, 30.0],
    "lat_nan": [np.nan, np.nan, np.nan],
    "lat_hole": [10.0, np.nan, 30.0],
}
LONGITUDES = {
    "lon": [0.0, 90.0, 180.0, 270.0],
    "lon_inf": [0.0, 90.0, 180.0, np.inf],
}
FIELDS = [
    ("all_nan", "lat_nan", "lon"),
    ("hole", "lat_hole", "lon"),
    ("infinite_end", "lat", "lon_inf"),
    ("placed", "lat", "lon"),
    ("masked", "lat_fill", "lon"),
]
FILL = -999.0


def coordinate(d, name: str, values: list[float], units: str, standard: str) -> None:
    d.createDimension(name, len(values))
    # `fill_value=False`: no `_FillValue`, so nothing masks the NaN.
    v = d.createVariable(name, "f8", (name,), fill_value=False)
    v.units = units
    v.standard_name = standard
    v[:] = np.array(values, dtype="f8")


def main() -> int:
    if not FIXTURE.parent.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    if FIXTURE.exists():
        FIXTURE.unlink()
    with netCDF4.Dataset(FIXTURE, "w", format="NETCDF3_CLASSIC") as d:
        d.title = "Synthetic 1-D coordinates holding NaN or infinity (#1172)"
        d.Conventions = "CF-1.8"
        for name, values in LATITUDES.items():
            coordinate(d, name, values, "degrees_north", "latitude")
        d.createDimension("lat_fill", 3)
        v = d.createVariable("lat_fill", "f8", ("lat_fill",), fill_value=FILL)
        v.units = "degrees_north"
        v.standard_name = "latitude"
        v.set_auto_mask(False)  # write the sentinel verbatim
        v[:] = np.array([10.0, FILL, 30.0], dtype="f8")
        for name, values in LONGITUDES.items():
            coordinate(d, name, values, "degrees_east", "longitude")
        for name, y, x in FIELDS:
            v = d.createVariable(name, "f4", (y, x), fill_value=False)
            v.units = "K"
            v[:] = np.arange(12, dtype="f4").reshape(3, 4) + 270.0
    print(f"wrote {FIXTURE}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
