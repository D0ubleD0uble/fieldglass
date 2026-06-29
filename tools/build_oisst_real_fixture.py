#!/usr/bin/env python3
"""Build the *real* NOAA OISST v2.1 NetCDF-4 corpus fixture (issue #123).

This is a tiny window subset of a genuine operational analysis file:

    NOAA/NCEI 1/4° Daily Optimum Interpolation Sea Surface Temperature
    (OISST) v2.1, AVHRR, 2025-01-01
    s3://noaa-cdr-sea-surface-temp-optimum-interpolation-pds/
      data/v2.1/avhrr/202501/oisst-avhrr-v02r01.20250101.nc

OISST is a NOAA Climate Data Record produced by NOAA/NCEI — a work of the U.S.
Government, in the public domain (see fixtures/NOTICE.md). It complements the
GOES-16 satellite fixture with a regular-lat/lon analysis grid and, unlike the
geostationary GOES file, exercises:

  * the HDF5 chunked + **deflate + shuffle** value path on real data (GOES used
    deflate without shuffle),
  * CF unpacking with scalar ``valid_min`` / ``valid_max`` (GOES used the
    two-element ``valid_range``), plus ``scale_factor`` / ``add_offset`` /
    ``_FillValue`` over a real land/ocean mask,
  * a 4-D ``(time, zlev, lat, lon)`` variable with singleton ``time`` / ``zlev``,
  * dense HDF5 attribute storage: the fixture retains 25 global attributes (the
    source carries 37), still well past libhdf5's 8-attribute compact threshold,
    so the metadata spills into a fractal heap — the structure the #33 robustness
    work hardened.

The subset keeps a small coastal window (mixed land + ocean so the ``sst`` fill
mask is exercised) of the ``sst`` and ``ice`` fields plus the coordinate
variables; the raw on-disk ``int16`` codes are copied verbatim (auto-scaling
off) so the genuine CF attributes survive unchanged.

Run from the repo root (needs ``netCDF4`` + ``numpy``; downloads ~1.5 MB once)::

    python3 tools/build_oisst_real_fixture.py

It writes ``oisst_avhrr_v2.nc`` and ``oisst_avhrr_v2.nc.oracle.json`` next to the
other NetCDF fixtures. The committed fixture + oracle mean the Rust test suite
needs no network at runtime.
"""
from __future__ import annotations

import json
import urllib.request
from pathlib import Path

import netCDF4
import numpy as np

HERE = Path(__file__).resolve().parent.parent
FIXTURES = HERE / "crates" / "fieldglass-netcdf" / "tests" / "fixtures"

# Immutable object in the public NOAA OISST Climate Data Record archive.
S3_KEY = "data/v2.1/avhrr/202501/oisst-avhrr-v02r01.20250101.nc"
SOURCE_URL = (
    "https://noaa-cdr-sea-surface-temp-optimum-interpolation-pds.s3.amazonaws.com/"
    + S3_KEY
)
CACHE = Path("/tmp") / Path(S3_KEY).name

# Window of the 720x1440 global 1/4° grid: Hudson Bay (~58-66°N, ~278-286°E) on
# 2025-01-01. A January high-latitude scene so all three real behaviours appear
# together: land + sea-ice fill the sst mask (~1/3 of points), and the ice field
# carries genuine sea-ice concentrations (>0) over the rest. lat[i] = -89.875 +
# 0.25*i, lon[j] = 0.125 + 0.25*j.
ROW0, COL0, N = 592, 1112, 32

# Identity globals worth keeping. Window-dependent extent attributes
# (geospatial_lat/lon_min/max) are deliberately dropped — they describe the full
# grid and would be wrong for the subset; a `history` note records the subset.
KEEP_GLOBALS = [
    "Conventions", "title", "references", "source", "id", "naming_authority",
    "cdm_data_type", "product_version", "processing_level", "institution",
    "keywords", "keywords_vocabulary", "platform", "platform_vocabulary",
    "instrument", "instrument_vocabulary", "standard_name_vocabulary",
    "geospatial_lat_units", "geospatial_lat_resolution",
    "geospatial_lon_units", "geospatial_lon_resolution",
    "time_coverage_start", "time_coverage_end", "sensor",
]


def fetch_source() -> Path:
    if not CACHE.exists():
        print(f"  downloading {SOURCE_URL}")
        # SOURCE_URL is composed only from string literals, so a static analyzer
        # can constant-fold it to a fixed https:// URL — no caller-controlled
        # value ever reaches the fetch. We read via single-argument urlopen and
        # write the body ourselves rather than the two-argument urlretrieve: only
        # the former lets the analyzer see the URL as the proven constant it is.
        # The source object is ~1.5 MB, so buffering it in memory is fine.
        with urllib.request.urlopen(SOURCE_URL) as resp:
            CACHE.write_bytes(resp.read())
    return CACHE


def copy_attrs(src, dst, names=None):
    # _FillValue is set at creation (createVariable fill_value=), not after.
    for name in (names if names is not None else src.ncattrs()):
        if name == "_FillValue":
            continue
        if names is not None and name not in src.ncattrs():
            continue
        dst.setncattr(name, src.getncattr(name))


def sample_indices(n: int) -> list[int]:
    if n <= 5:
        return list(range(n))
    return sorted({0, 1, n // 2, n - 2, n - 1})


def packed_field_oracle(raw: np.ndarray, var) -> dict:
    """CF-unpack the windowed raw codes the way libnetcdf does: mask _FillValue
    and anything outside [valid_min, valid_max], then scale_factor / add_offset."""
    scale, offset = float(var.scale_factor), float(var.add_offset)
    fill = int(var._FillValue)
    vlo, vhi = int(var.valid_min), int(var.valid_max)
    flat = raw.reshape(-1)
    valid = (flat != fill) & (flat >= vlo) & (flat <= vhi)
    scaled = flat.astype("f8") * scale + offset
    present = scaled[valid]
    samples = {str(k): (float(scaled[k]) if valid[k] else None)
               for k in sample_indices(flat.size)}
    return {
        "scale_factor": scale, "add_offset": offset,
        "fill_value": fill, "valid_min": vlo, "valid_max": vhi,
        "units": str(var.units), "long_name": str(var.long_name),
        "present_count": int(valid.sum()), "missing_count": int((~valid).sum()),
        "min": round(float(present.min()), 6) if present.size else None,
        "max": round(float(present.max()), 6) if present.size else None,
        "mean": round(float(present.mean()), 6) if present.size else None,
        "scaled_samples": samples,
    }


def main() -> int:
    if not FIXTURES.is_dir():
        raise SystemExit("run from the repo root")
    src = netCDF4.Dataset(fetch_source())

    rs, cs = slice(ROW0, ROW0 + N), slice(COL0, COL0 + N)
    s_sst, s_ice = src.variables["sst"], src.variables["ice"]
    s_lat, s_lon = src.variables["lat"], src.variables["lon"]
    s_time, s_zlev = src.variables["time"], src.variables["zlev"]
    for v in (s_sst, s_ice):
        v.set_auto_maskandscale(False)
    sst_raw = np.asarray(s_sst[0, 0, rs, cs]).astype("i2")
    ice_raw = np.asarray(s_ice[0, 0, rs, cs]).astype("i2")
    lat_v = np.asarray(s_lat[rs]).astype("f4")
    lon_v = np.asarray(s_lon[cs]).astype("f4")
    time_v = np.asarray(s_time[:]).astype("f4")
    zlev_v = np.asarray(s_zlev[:]).astype("f4")

    out = FIXTURES / "oisst_avhrr_v2.nc"
    with netCDF4.Dataset(out, "w", format="NETCDF4") as d:
        copy_attrs(src, d, KEEP_GLOBALS)
        d.setncattr("history", "Hudson Bay window subset of the source OISST file "
                               "(see tools/build_oisst_real_fixture.py, NOTICE.md)")
        d.createDimension("time", 1)
        d.createDimension("zlev", 1)
        d.createDimension("lat", N)
        d.createDimension("lon", N)

        out_time = d.createVariable("time", "f4", ("time",))
        copy_attrs(s_time, out_time)
        out_time[:] = time_v
        out_zlev = d.createVariable("zlev", "f4", ("zlev",))
        copy_attrs(s_zlev, out_zlev)
        out_zlev[:] = zlev_v
        out_lat = d.createVariable("lat", "f4", ("lat",))
        copy_attrs(s_lat, out_lat)
        out_lat[:] = lat_v
        out_lon = d.createVariable("lon", "f4", ("lon",))
        copy_attrs(s_lon, out_lon)
        out_lon[:] = lon_v

        # Match the real OISST encoding: chunked + deflate + shuffle, so the
        # committed fixture exercises the HDF5 chunked + deflate + shuffle value
        # path on real data. Auto-scaling is OFF on write so the verbatim int16
        # codes keep their genuine CF attributes (otherwise netCDF4 would re-pack
        # them). oisst_real_world.rs fails loudly on any decode drift.
        for name, raw, src_var in (("sst", sst_raw, s_sst), ("ice", ice_raw, s_ice)):
            ov = d.createVariable(name, "i2", ("time", "zlev", "lat", "lon"),
                                  zlib=True, shuffle=True, complevel=4,
                                  chunksizes=(1, 1, 16, 16),
                                  fill_value=np.int16(src_var._FillValue))
            ov.set_auto_maskandscale(False)
            copy_attrs(src_var, ov)
            ov[0, 0, :, :] = raw

    # ---- oracle: structure, regular-grid geolocation, value samples ----
    lat0, lon0 = float(lat_v[0]), float(lon_v[0])
    dlat = float(lat_v[1] - lat_v[0])
    dlon = float(lon_v[1] - lon_v[0])
    oracle = {
        "source": (
            f"NOAA OISST v2.1 AVHRR 2025-01-01; {N}x{N} coastal window "
            f"(rows {ROW0}..{ROW0 + N}, cols {COL0}..{COL0 + N}) of "
            f"s3://noaa-cdr-sea-surface-temp-optimum-interpolation-pds/{S3_KEY}. "
            f"netCDF4 {netCDF4.__version__} "
            f"(libnetcdf {netCDF4.getlibversion().split()[0]}). Provenance in NOTICE.md."
        ),
        "dimensions": {"time": 1, "zlev": 1, "lat": N, "lon": N},
        "variables": sorted(["time", "zlev", "lat", "lon", "sst", "ice"]),
        # A few identity globals retained verbatim from the source — asserted so
        # the dense (fractal-heap) global-attribute parse is covered non-vacuously.
        "global_attrs": {k: str(src.getncattr(k))
                         for k in ("Conventions", "title", "institution")},
        "grid": {
            "lat0": round(lat0, 6), "lon0": round(lon0, 6),
            "dlat": round(dlat, 6), "dlon": round(dlon, 6),
            "lat_last": round(float(lat_v[-1]), 6),
            "lon_last": round(float(lon_v[-1]), 6),
        },
        "sst": packed_field_oracle(sst_raw, s_sst),
        "ice": packed_field_oracle(ice_raw, s_ice),
    }
    (FIXTURES / "oisst_avhrr_v2.nc.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n")
    src.close()
    so, io = oracle["sst"], oracle["ice"]
    print(f"  wrote {out.name} ({out.stat().st_size} bytes) + oracle")
    print(f"  sst: {so['present_count']} present / {so['missing_count']} missing"
          f" (land mask), range [{so['min']}, {so['max']}] {so['units']}")
    print(f"  ice: {io['present_count']} present / {io['missing_count']} missing")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
