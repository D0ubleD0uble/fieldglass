#!/usr/bin/env python3
"""Build ``samples/latest_format.nc`` — a renderable "latest format" NetCDF-4 file.

#216 taught the HDF5 value decoder the version-4 / version-5 chunk indexes that
a recent libhdf5 writes ("latest format"): single-chunk, fixed-array,
extensible-array (filtered and not), implicit, and v2-B-tree. Nothing in the
sample corpus exercises any of them. ``samples/oisst.nc`` and ``samples/goes.nc``
are both chunked, but netCDF-4 writes them in the *default* format, whose chunk
index is the **version-1 B-tree** — the path that already shipped in 0.2.0.

The fixtures that do cover #216 (``hdf5_ea_chunk_index.h5`` and friends) are bare
HDF5: no dimension scales, no CF coordinates, so no renderable variable and
nothing for the UI to draw. Value decode is only reachable from the UI through a
render, so those fixtures cannot exercise it there.

This writes the missing file: a small global field carrying proper netCDF-4
dimension scales and CF axis attributes, under ``libver='latest'`` so libhdf5
picks the new indexes.

  * ``t2m``          — fixed shape + chunked  → **version-4 fixed-array** index
  * ``t2m_growable`` — unlimited leading dim + gzip/shuffle
                       → **version-5 filtered extensible-array** index

Both are renderable (lat/lon axes resolve), so opening the file in the viewer
drives decode end to end. It is a true regression check: on v0.2.0 both variables
fail with "HDF5 data layout message version 4 / 5 is not supported"; on master
they decode.

``samples/`` is gitignored (see ``tools/fetch_samples.sh``) — the corpus is built
locally, so this file is generated, not committed. Run from the repo root (needs
``h5py``):

    python3 tools/build_latest_format_sample.py
"""
from __future__ import annotations

from pathlib import Path

import h5py
import numpy as np

OUT = Path("samples/latest_format.nc")
NY, NX = 90, 180


def main() -> None:
    lat = np.linspace(-89.0, 89.0, NY)
    lon = np.linspace(-179.0, 179.0, NX)
    yy, xx = np.meshgrid(lat, lon, indexing="ij")
    # A smooth, obviously-global field: warm equator, cool poles, a gentle
    # zonal wave so a wrong row/column order is visible at a glance.
    field = (280.0 + 25.0 * np.cos(np.radians(yy)) + 3.0 * np.sin(np.radians(2.0 * xx))).astype("f4")

    OUT.parent.mkdir(exist_ok=True)
    with h5py.File(OUT, "w", libver="latest") as f:
        f.attrs["title"] = np.bytes_(b"fieldglass latest-format sample (#216)")
        f.attrs["Conventions"] = np.bytes_(b"CF-1.8")

        def scale(name, data, dimid, units, standard_name=None):
            """A netCDF-4 coordinate variable: a dimension scale + CF attributes."""
            d = f.create_dataset(name, data=data, track_times=False)
            d.attrs["_Netcdf4Dimid"] = np.int32(dimid)
            d.make_scale(name)
            d.attrs["units"] = np.bytes_(units.encode())
            if standard_name:
                d.attrs["standard_name"] = np.bytes_(standard_name.encode())
            return d

        d_lat = scale("lat", lat, 0, "degrees_north", "latitude")
        d_lon = scale("lon", lon, 1, "degrees_east", "longitude")
        d_time = scale("time", np.array([0.0]), 2, "hours since 2026-01-01 00:00:00", "time")

        # Fixed shape + chunked under libver='latest' → version-4 fixed-array index.
        t2m = f.create_dataset("t2m", data=field, chunks=(30, 60), track_times=False)
        t2m.attrs["units"] = np.bytes_(b"K")
        t2m.attrs["long_name"] = np.bytes_(b"2 metre temperature (fixed-array chunk index)")
        t2m.dims[0].attach_scale(d_lat)
        t2m.dims[1].attach_scale(d_lon)

        # Unlimited leading dim + filters → version-5 filtered extensible-array index.
        grow = f.create_dataset(
            "t2m_growable",
            shape=(1, NY, NX),
            maxshape=(None, NY, NX),
            chunks=(1, 30, 60),
            dtype="f4",
            compression="gzip",
            shuffle=True,
            track_times=False,
        )
        grow[0] = field
        grow.attrs["units"] = np.bytes_(b"K")
        grow.attrs["long_name"] = np.bytes_(b"2 metre temperature (extensible-array chunk index)")
        grow.dims[0].attach_scale(d_time)
        grow.dims[1].attach_scale(d_lat)
        grow.dims[2].attach_scale(d_lon)

    print(f"wrote {OUT} (h5py {h5py.__version__}, libhdf5 {h5py.version.hdf5_version}, libver='latest')")


if __name__ == "__main__":
    main()
