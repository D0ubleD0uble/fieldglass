#!/usr/bin/env python3
"""Build the projected-grid NetCDF test fixtures for issue #168 (decision 0004):

  * ``wrf_lambert.nc``       — a WRF ``wrfout``-style classic file: the Lambert
    projection lives in *global attributes* (``MAP_PROJ`` = 1, ``TRUELAT1/2``,
    ``STAND_LON``, ``MOAD_CEN_LAT``, ``DX``/``DY``) and the 2-D ``XLAT``/``XLONG``
    coordinate arrays are precomputed conveniences (and the verification oracle).
  * ``goes_geostationary.nc`` — a GOES ABI-style NetCDF-4 file: a CF
    ``grid_mapping`` variable ``goes_imager_projection`` carries the
    ``geostationary`` parameters and the 1-D ``x``/``y`` *radian* scan-angle
    coordinate variables are stored as scaled ``int16`` (the real GOES on-disk
    encoding), exercising CF ``scale_factor``/``add_offset``.

Both grids are **regular grids in a projected CRS** (Model A in decision 0004),
deliberately tiny so they stay byte-small in git. The coordinate geometry is
generated with *independent* NumPy implementations of the standard projection
formulas (Snyder Lambert Conformal Conic; GOES-R PUG fixed-grid), so the Rust
projectors reproducing them is a genuine cross-language cross-check rather than a
tautology.

Run from the repo root (needs ``netCDF4`` + ``numpy``)::

    python3 tools/build_netcdf_projected_fixtures.py

It writes the two ``.nc`` files and a sibling ``*.oracle.json`` for each, next to
the other NetCDF fixtures. See that directory's ``NOTICE.md`` for provenance.
"""
from __future__ import annotations

import json
import math
from pathlib import Path

import netCDF4
import numpy as np

HERE = Path(__file__).resolve().parent.parent
FIXTURES = HERE / "crates" / "fieldglass-netcdf" / "tests" / "fixtures"

# Spherical Earth radius used by the Fieldglass Lambert projector
# (`fieldglass_core::projection::EARTH_RADIUS_M`, the WMO shapeOfTheEarth = 6
# default). Real wrfout uses 6_370_000 m; the ~0.02 % difference is the same
# approximation the GRIB Lambert path already makes, so the fixture adopts the
# projector's constant to keep the cross-check about the projection math.
EARTH_R = 6_371_229.0
DEG = math.pi / 180.0


# ---------------------------------------------------------------------------
# Lambert Conformal Conic (Snyder, "Map Projections — A Working Manual", §15)
# ---------------------------------------------------------------------------
def _lambert_constants(latin1: float, latin2: float, lad: float):
    f1, f2, f0 = latin1 * DEG, latin2 * DEG, lad * DEG
    t1 = math.tan(math.pi / 4 + f1 / 2)
    t2 = math.tan(math.pi / 4 + f2 / 2)
    if abs(latin1 - latin2) < 1e-9:
        n = math.sin(f1)
    else:
        n = math.log(math.cos(f1) / math.cos(f2)) / math.log(t2 / t1)
    f = math.cos(f1) * t1**n / n
    rho0 = EARTH_R * f / math.tan(math.pi / 4 + f0 / 2) ** n
    return n, f, rho0


def lambert_forward(lat, lon, latin1, latin2, lad, lov):
    n, f, rho0 = _lambert_constants(latin1, latin2, lad)
    dlon = ((lon - lov + 180.0) % 360.0 - 180.0) * DEG
    rho = EARTH_R * f / math.tan(math.pi / 4 + lat * DEG / 2) ** n
    return rho * math.sin(n * dlon), rho0 - rho * math.cos(n * dlon)


def lambert_inverse(x, y, latin1, latin2, lad, lov):
    n, f, rho0 = _lambert_constants(latin1, latin2, lad)
    dy = rho0 - y
    rho = math.copysign(math.hypot(x, dy), n)
    theta = math.atan2(x, dy)
    lon = lov + (theta / n) / DEG
    lat = (2 * math.atan((EARTH_R * f / rho) ** (1 / n)) - math.pi / 2) / DEG
    return lat, lon


def build_wrf() -> None:
    """A tiny CONUS-like WRF mass grid. We pin the south-west corner (the first
    scanned point, south_north = west_east = 0) in geographic coordinates, walk
    +DX/+DY through projected space, and inverse-project to fill XLAT/XLONG."""
    latin1, latin2, lov, lad = 30.0, 60.0, -97.5, 38.5
    dx = dy = 30_000.0  # metres (deliberately coarse; this is a 6×5 toy grid)
    nx, ny = 6, 5
    sw_lat, sw_lon = 32.0, -100.0

    x0, y0 = lambert_forward(sw_lat, sw_lon, latin1, latin2, lad, lov)
    xlat = np.zeros((ny, nx), dtype="f4")
    xlong = np.zeros((ny, nx), dtype="f4")
    for j in range(ny):
        for i in range(nx):
            lat, lon = lambert_inverse(
                x0 + i * dx, y0 + j * dy, latin1, latin2, lad, lov
            )
            xlat[j, i] = lat
            xlong[j, i] = lon

    # A smooth, recognisable data field so a render is visually meaningful.
    t2 = (280.0 + 5.0 * np.cos(np.linspace(0, math.pi, ny))[:, None]
          + 3.0 * np.sin(np.linspace(0, math.pi, nx))[None, :]).astype("f4")

    path = FIXTURES / "wrf_lambert.nc"
    with netCDF4.Dataset(path, "w", format="NETCDF3_CLASSIC") as d:
        d.setncatts({
            "TITLE": " OUTPUT FROM WRF (synthetic fixture)",
            "MAP_PROJ": np.int32(1),          # 1 = Lambert Conformal
            "MAP_PROJ_CHAR": "Lambert Conformal",
            "TRUELAT1": np.float32(latin1),
            "TRUELAT2": np.float32(latin2),
            "STAND_LON": np.float32(lov),
            "MOAD_CEN_LAT": np.float32(lad),
            "DX": np.float32(dx),
            "DY": np.float32(dy),
        })
        d.createDimension("Time", None)
        d.createDimension("south_north", ny)
        d.createDimension("west_east", nx)
        v_lat = d.createVariable("XLAT", "f4", ("Time", "south_north", "west_east"))
        v_lat.setncatts({"units": "degree_north", "description": "LATITUDE, SOUTH IS NEGATIVE"})
        v_lat[0, :, :] = xlat
        v_lon = d.createVariable("XLONG", "f4", ("Time", "south_north", "west_east"))
        v_lon.setncatts({"units": "degree_east", "description": "LONGITUDE, WEST IS NEGATIVE"})
        v_lon[0, :, :] = xlong
        v_t2 = d.createVariable("T2", "f4", ("Time", "south_north", "west_east"))
        v_t2.setncatts({"units": "K", "description": "TEMP at 2 M"})
        v_t2[0, :, :] = t2

    oracle = {
        "projection": "lambert_conformal_conic",
        "map_proj": 1,
        "truelat1": latin1, "truelat2": latin2,
        "stand_lon": lov, "moad_cen_lat": lad,
        "dx": dx, "dy": dy, "nx": nx, "ny": ny,
        # Interior cells whose (XLAT, XLONG) the Rust Lambert inverse must map
        # back to (i, j). The grid edge (row 0 / col 0 / far edge) is skipped:
        # float32 storage nudges a boundary cell a hair outside the grid, which
        # the projector correctly rejects — not what this geolocation check is
        # about (the render warp samples the interior).
        "samples": [
            {"i": i, "j": j, "lat": float(xlat[j, i]), "lon": float(xlong[j, i])}
            for j in (1, ny // 2, ny - 2) for i in (1, nx // 2, nx - 2)
        ],
    }
    (FIXTURES / "wrf_lambert.nc.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n")
    print(f"  wrote {path.name} ({path.stat().st_size} bytes) + oracle")


# ---------------------------------------------------------------------------
# GOES-R ABI fixed grid (GOES-R PUG Vol. 3 §5.1.2.8; NOAA STAR)
# ---------------------------------------------------------------------------
def goes_scan_to_lonlat(x, y, lon0_deg, h, r_eq, r_pol):
    """Scan angles (rad) → geodetic (lat, lon) in degrees, or None off-disk.
    Independent NumPy transcription of the GOES-R PUG fixed-grid algorithm
    (sweep axis = x)."""
    lon0 = lon0_deg * DEG
    a = (math.sin(x) ** 2
         + math.cos(x) ** 2 * (math.cos(y) ** 2 + (r_eq**2 / r_pol**2) * math.sin(y) ** 2))
    b = -2.0 * h * math.cos(x) * math.cos(y)
    c = h * h - r_eq * r_eq
    disc = b * b - 4 * a * c
    if disc < 0:
        return None
    rs = (-b - math.sqrt(disc)) / (2 * a)
    sx = rs * math.cos(x) * math.cos(y)
    sy = -rs * math.sin(x)
    sz = rs * math.cos(x) * math.sin(y)
    lat = math.atan((r_eq**2 / r_pol**2) * sz / math.sqrt((h - sx) ** 2 + sy**2))
    lon = lon0 - math.atan(sy / (h - sx))
    return lat / DEG, lon / DEG


def build_goes() -> None:
    lon0 = -75.0                 # GOES-East sub-satellite longitude
    pph = 35_786_023.0           # perspective_point_height (above the ellipsoid)
    r_eq = 6_378_137.0           # GRS80 semi-major
    r_pol = 6_356_752.31414      # GRS80 semi-minor
    h = pph + r_eq               # Earth-centre → satellite distance
    nx, ny = 6, 6

    # A small near-nadir patch of the ABI fixed grid, in radians. y descends
    # north→south (negative step), as in real GOES files.
    x_rad = np.linspace(-0.02, 0.02, nx)
    y_rad = np.linspace(0.02, -0.02, ny)

    # Store x/y as scaled int16 (the real GOES encoding) so the reader exercises
    # CF scale_factor/add_offset. netCDF4 packs the float radians on write as
    # round((value - add_offset) / scale_factor); centre the offset so the
    # packed codes fit signed int16 (±32767).
    def scale_offset(arr):
        lo, hi = float(arr.min()), float(arr.max())
        return (hi - lo) / 60000.0, (hi + lo) / 2.0

    xs, xo = scale_offset(x_rad)
    ys, yo = scale_offset(y_rad)

    rad = np.zeros((ny, nx), dtype="f4")
    samples = []
    for j in range(ny):
        for i in range(nx):
            ll = goes_scan_to_lonlat(x_rad[i], y_rad[j], lon0, h, r_eq, r_pol)
            if ll is not None:
                samples.append({"i": i, "j": j, "x": float(x_rad[i]),
                                "y": float(y_rad[j]), "lat": ll[0], "lon": ll[1]})
            rad[j, i] = 100.0 + i + 10 * j

    path = FIXTURES / "goes_geostationary.nc"
    with netCDF4.Dataset(path, "w", format="NETCDF4") as d:
        d.setncatts({"title": "synthetic GOES ABI fixed-grid fixture",
                     "Conventions": "CF-1.7"})
        d.createDimension("y", ny)
        d.createDimension("x", nx)
        gm = d.createVariable("goes_imager_projection", "i4")
        gm.setncatts({
            "grid_mapping_name": "geostationary",
            "perspective_point_height": pph,
            "semi_major_axis": r_eq,
            "semi_minor_axis": r_pol,
            "longitude_of_projection_origin": lon0,
            "sweep_angle_axis": "x",
        })
        vx = d.createVariable("x", "i2", ("x",), contiguous=True)
        vx.setncatts({"units": "rad", "axis": "X",
                      "standard_name": "projection_x_coordinate",
                      "scale_factor": xs, "add_offset": xo})
        vx[:] = x_rad  # netCDF4 packs to int16 via scale_factor/add_offset
        vy = d.createVariable("y", "i2", ("y",), contiguous=True)
        vy.setncatts({"units": "rad", "axis": "Y",
                      "standard_name": "projection_y_coordinate",
                      "scale_factor": ys, "add_offset": yo})
        vy[:] = y_rad
        vr = d.createVariable("Rad", "f4", ("y", "x"), contiguous=True)
        vr.setncatts({"units": "W m-2 sr-1 um-1", "grid_mapping": "goes_imager_projection"})
        vr[:, :] = rad

    oracle = {
        "projection": "geostationary",
        "longitude_of_projection_origin": lon0,
        "perspective_point_height": pph,
        "semi_major_axis": r_eq, "semi_minor_axis": r_pol,
        "h_metres": h, "sweep_angle_axis": "x", "nx": nx, "ny": ny,
        "x_scale_factor": xs, "x_add_offset": xo,
        "y_scale_factor": ys, "y_add_offset": yo,
        "samples": samples,
    }
    (FIXTURES / "goes_geostationary.nc.oracle.json").write_text(json.dumps(oracle, indent=2) + "\n")
    print(f"  wrote {path.name} ({path.stat().st_size} bytes) + oracle")


def main() -> int:
    build_wrf()
    build_goes()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
