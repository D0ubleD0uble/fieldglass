#!/usr/bin/env python3
"""Build the curvilinear (2-D coordinate) NetCDF corpus fixtures (issue #444).

Every geolocated grid Fieldglass reads today is described by a *formula*: a
lat/lon box and a spacing, or a CF grid mapping with projection parameters. A
curvilinear grid is described by a *list* — two auxiliary coordinate variables
``lat(y, x)`` and ``lon(y, x)`` giving the position of every cell, with no
formula behind them and no grid mapping to recover one from. ADR-0004 calls
this "Model B" and defers it; #445 implements it and #218 renders it. This
script commits the corpus those two need, which #444 splits out because the
licence and size work is what re-scoped #123.

Two files, because the shape of the irregularity differs and an implementation
can pass one while failing the other:

``rtofs_tripolar_arctic.nc`` — **ocean tripolar.** A window of NCEP's Global
Real-Time Ocean Forecast System (RTOFS), a global HYCOM run. South of about
47 degN the grid is an ordinary Mercator lat/lon; north of it the mesh is
replaced by a *bipolar* patch whose two poles sit over land so that neither is
in the ocean. The consequence is that a single row of the array runs from
47 degN up over the pole and back down, and the committed window (centred on
the pole at column 1124) has latitudes from 82 degN to 89.98 degN with the fold
running through it. Longitudes in the source are **not normalised** — this
window spans 74 deg to 1019 deg — which is its own trap for a reader that
assumes [-180, 180].

``mirs_swath_n21.nc`` — **satellite swath.** A window of a NOAA-21 MiRS
imagery granule: a microwave sounder's cross-track scan, 96 fields of view per
scanline, geolocated per pixel. The committed scanlines are the end of the
descending pass, chosen because they cross the antimeridian *and* converge on
the south pole, so a reader that unwraps longitude naively or assumes rows are
parallels fails here rather than on a benign mid-latitude window. Its fields
are ``int16`` with CF ``scale_factor`` and ``_FillValue``, so the fixture
exercises the unpacking seam as well as the geometry.

Both sources are works of the U.S. Government (NOAA/NCEP and NOAA/NESDIS) and
carry no copyright; both objects are immutable archive members rather than
rolling operational files, so this script reproduces the same bytes on any
future run. See ``NOTICE.md`` for provenance and the licence note.

Run from the repo root (needs ``netCDF4``, ``numpy`` and ``requests``;
downloads ~66 MB once, cached in the system temp directory)::

    python3 tools/build_netcdf_curvilinear_fixtures.py
"""

from __future__ import annotations

import json
import tempfile
from pathlib import Path

import netCDF4
import numpy as np
import requests

HERE = Path(__file__).resolve().parent.parent
FIXTURES = HERE / "crates" / "fieldglass-netcdf" / "tests" / "fixtures"
CACHE = Path(tempfile.gettempdir())

# --- Sources ---------------------------------------------------------------
# Both are immutable objects in AWS Open Data buckets that need no credentials.
# The RTOFS NetCDF products are *not* in the operational NOMADS directory for
# long (a few days); the AWS mirror keeps them from 2024-01-27 onward, which is
# why a 2024 nowcast is pinned here rather than a recent run.
RTOFS_URL = (
    "https://noaa-nws-rtofs-pds.s3.amazonaws.com/"
    "rtofs.20240201/rtofs_glo_2ds_n000_ice.nc"
)
MIRS_URL = (
    "https://noaa-nesdis-n21-pds.s3.amazonaws.com/"
    "NPR_MIRS_IMG_33min/2023/09/19/"
    "NPR-MIRS-IMG_33min_v11_n21_s202309191449310_e202309191523380"
    "_c202309191705326.nc"
)

# --- Windows ---------------------------------------------------------------
# RTOFS: 200 rows x 260 columns centred on the bipolar patch's pole at column
# 1124 of the last row. Small enough to commit, large enough that the fold runs
# through it (the last row's latitude varies by more than a degree of standard
# deviation, where a row south of 47 degN is constant to float precision).
RTOFS_ROW0, RTOFS_ROWS = 3098, 200
RTOFS_COL0, RTOFS_COLS = 1024, 260
RTOFS_VARS = ("ice_coverage", "ice_thickness", "ice_temperature")

# MiRS: the last 100 scanlines of the granule, all 96 fields of view. Scanlines
# 701-736 of the source are the ones whose own row spans the antimeridian.
MIRS_SCAN0, MIRS_SCANS = 660, 100
MIRS_VARS = ("TPW", "RR", "SIce", "TSkin")

# How many sampled cells the oracle records. The full coordinate arrays are in
# the fixture; the oracle exists so a test can pin specific cells by hand
# without re-deriving them, and so a *reader* bug shows as a mismatch against a
# reading taken with an independent library rather than with itself.
ORACLE_SAMPLES = 24


def fetch(url: str) -> Path:
    """Download ``url`` into the temp cache once, returning the local path."""
    path = CACHE / Path(url).name
    if path.exists():
        print(f"cached {path}")
        return path
    print(f"downloading {url}")
    # `requests` rather than `urlopen`: it mounts adapters for http and https
    # only, so a `file://` or `ftp://` URL is refused up front instead of
    # reading a local file.
    response = requests.get(url, timeout=120)
    response.raise_for_status()
    path.write_bytes(response.content)
    return path


def fill_value_of(var: netCDF4.Variable) -> float | None:
    """`_FillValue` if the variable declares one, else `None`.

    Not every MiRS field carries one — `RR` does not — and `getncattr` raises
    rather than answering for a missing attribute, so this is the guard that
    keeps the two kinds of variable on one code path.
    """
    if "_FillValue" in var.ncattrs():
        return var.getncattr("_FillValue")
    return None


def copy_attrs(src: netCDF4.Variable, dst: netCDF4.Variable) -> None:
    """Carry every attribute across verbatim, `_FillValue` excepted.

    `_FillValue` cannot be assigned after creation, so it is passed to
    `createVariable` instead; copying it here would raise.
    """
    for name in src.ncattrs():
        if name != "_FillValue":
            dst.setncattr(name, src.getncattr(name))


def sample_oracle(lat: np.ndarray, lon: np.ndarray) -> list[dict[str, float]]:
    """Sampled `(y, x) -> (lat, lon)` readings, corners always included.

    The corners are the cells a windowing bug moves first; the interior stride
    catches a transposition that happens to fix the corners.
    """
    ny, nx = lat.shape
    cells = [(0, 0), (0, nx - 1), (ny - 1, 0), (ny - 1, nx - 1)]
    stride = max(1, (ny * nx) // (ORACLE_SAMPLES - len(cells)))
    for flat in range(0, ny * nx, stride):
        cells.append((flat // nx, flat % nx))
    seen: dict[tuple[int, int], None] = {}
    for cell in cells:
        seen.setdefault(cell, None)
    return [
        {"y": y, "x": x, "lat": float(lat[y, x]), "lon": float(lon[y, x])}
        for y, x in sorted(seen)
    ]


def build_rtofs() -> None:
    src_path = fetch(RTOFS_URL)
    out = FIXTURES / "rtofs_tripolar_arctic.nc"
    rows = slice(RTOFS_ROW0, RTOFS_ROW0 + RTOFS_ROWS)
    cols = slice(RTOFS_COL0, RTOFS_COL0 + RTOFS_COLS)

    with netCDF4.Dataset(src_path) as src, netCDF4.Dataset(out, "w") as dst:
        lat = np.asarray(src["Latitude"][rows, cols])
        lon = np.asarray(src["Longitude"][rows, cols])
        assert lat.std(axis=1).max() > 1.0, "window does not contain the fold"

        dst.createDimension("MT", 1)
        dst.createDimension("Y", RTOFS_ROWS)
        dst.createDimension("X", RTOFS_COLS)
        for name, data in (("Latitude", lat), ("Longitude", lon)):
            var = dst.createVariable(name, "f4", ("Y", "X"), zlib=True, complevel=9)
            copy_attrs(src[name], var)
            var[:] = data
        for name in ("MT", "Date"):
            var = dst.createVariable(name, "f8", ("MT",))
            copy_attrs(src[name], var)
            var[:] = src[name][:1]
        for name in RTOFS_VARS:
            source = src[name]
            var = dst.createVariable(name, "f4", ("MT", "Y", "X"), zlib=True, complevel=9)
            copy_attrs(source, var)
            var[:] = np.asarray(source[0:1, rows, cols])

        for name in src.ncattrs():
            dst.setncattr(name, src.getncattr(name))
        dst.setncattr(
            "history",
            f"window [{rows.start}:{rows.stop}, {cols.start}:{cols.stop}] of "
            f"{Path(RTOFS_URL).name} for Fieldglass fixture (#444)",
        )
        oracle = {
            "source_url": RTOFS_URL,
            "window": {
                "y": [rows.start, rows.stop],
                "x": [cols.start, cols.stop],
            },
            "shape": {"y": RTOFS_ROWS, "x": RTOFS_COLS},
            "coordinates_attribute": src[RTOFS_VARS[0]].coordinates,
            "lat_range": [float(lat.min()), float(lat.max())],
            "lon_range": [float(lon.min()), float(lon.max())],
            "samples": sample_oracle(lat, lon),
        }

    write_oracle(out, oracle)
    print(f"wrote {out} ({out.stat().st_size} bytes)")


def build_mirs() -> None:
    src_path = fetch(MIRS_URL)
    out = FIXTURES / "mirs_swath_n21.nc"
    scans = slice(MIRS_SCAN0, MIRS_SCAN0 + MIRS_SCANS)

    with netCDF4.Dataset(src_path) as src, netCDF4.Dataset(out, "w") as dst:
        lat = np.asarray(src["Latitude"][scans, :])
        lon = np.asarray(src["Longitude"][scans, :])
        fovs = lat.shape[1]
        assert (lon.max(axis=1) - lon.min(axis=1)).max() > 180.0, (
            "window does not cross the antimeridian"
        )

        dst.createDimension("Scanline", MIRS_SCANS)
        dst.createDimension("Field_of_view", fovs)
        for name, data in (("Latitude", lat), ("Longitude", lon)):
            source = src[name]
            var = dst.createVariable(
                name,
                "f4",
                ("Scanline", "Field_of_view"),
                zlib=True,
                complevel=9,
                fill_value=fill_value_of(source),
            )
            copy_attrs(source, var)
            var[:] = data
        for name in MIRS_VARS:
            source = src[name]
            var = dst.createVariable(
                name,
                source.dtype,
                ("Scanline", "Field_of_view"),
                zlib=True,
                complevel=9,
                fill_value=fill_value_of(source),
            )
            copy_attrs(source, var)
            # `set_auto_maskandscale(False)` keeps the stored integers as they
            # are: the point of carrying `scale_factor` across is that the
            # fixture exercises Fieldglass's own unpacking, not netCDF4's.
            source.set_auto_maskandscale(False)
            var.set_auto_maskandscale(False)
            var[:] = np.asarray(source[scans, :])

        for name in src.ncattrs():
            dst.setncattr(name, src.getncattr(name))
        dst.setncattr(
            "history",
            f"scanlines [{scans.start}:{scans.stop}] of {Path(MIRS_URL).name} "
            "for Fieldglass fixture (#444)",
        )
        oracle = {
            "source_url": MIRS_URL,
            "window": {"scanline": [scans.start, scans.stop]},
            "shape": {"scanline": MIRS_SCANS, "field_of_view": fovs},
            "coordinates_attribute": src[MIRS_VARS[0]].coordinates,
            "lat_range": [float(lat.min()), float(lat.max())],
            "lon_range": [float(lon.min()), float(lon.max())],
            "samples": sample_oracle(lat, lon),
        }

    write_oracle(out, oracle)
    print(f"wrote {out} ({out.stat().st_size} bytes)")


def write_oracle(fixture: Path, oracle: dict[str, object]) -> None:
    path = fixture.with_suffix(fixture.suffix + ".oracle.json")
    with path.open("w", encoding="utf-8") as handle:
        json.dump(oracle, handle, indent=2, sort_keys=True)
        handle.write("\n")
    print(f"wrote {path} ({path.stat().st_size} bytes)")


if __name__ == "__main__":
    build_rtofs()
    build_mirs()
