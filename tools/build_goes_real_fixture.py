#!/usr/bin/env python3
"""Build the *real* GOES-16 ABI NetCDF-4 corpus fixture (issue #123).

Unlike ``goes_geostationary.nc`` — a synthetic file whose geometry is generated
from independent projection formulas to cross-check the Rust projector — this
fixture is a tiny **window subset of a genuine operational GOES-16 file**:

    NOAA GOES-16 ABI L2 Cloud & Moisture Imagery, Mesoscale, band 13 (10.3 um IR)
    s3://noaa-goes16/ABI-L2-CMIPM/2023/001/18/
      OR_ABI-L2-CMIPM1-M6C13_G16_s20230011800281_e20230011800350_c20230011800425.nc

It is the first real NetCDF-4 / HDF5 file in the corpus, so it exercises the
whole stack on genuine data: HDF5 object-header + dimension-scale resolution, a
CF ``geostationary`` grid mapping with the real GRS80 / sub-satellite-longitude
parameters, scaled ``int16`` ``x`` / ``y`` scan-angle coordinates, and the
``CMI`` brightness-temperature field stored as unsigned ``int16`` with CF
``scale_factor`` / ``add_offset`` / ``valid_range`` / ``_FillValue``.

The source object is in the public ``noaa-goes16`` S3 bucket; GOES data is a
work of the U.S. Government and carries no copyright (see fixtures/NOTICE.md).
The subset keeps a small center window plus the projection, coordinate, and a
few identity variables; the dozens of ancillary scalar metadata variables are
dropped to keep the fixture byte-small in git.

The geolocation oracle is computed by an *independent* NumPy transcription of
the GOES-R PUG fixed-grid algorithm, so the Rust projector reproducing it is a
genuine cross-language check rather than a tautology.

Run from the repo root (needs ``netCDF4`` + ``numpy``; downloads ~350 KB once)::

    python3 tools/build_goes_real_fixture.py

It writes ``goes16_abi_cmip.nc`` and ``goes16_abi_cmip.nc.oracle.json`` next to
the other NetCDF fixtures. The committed fixture + oracle mean the Rust test
suite needs no network at runtime.
"""
from __future__ import annotations

import json
import math
import urllib.request
from pathlib import Path

import netCDF4
import numpy as np

HERE = Path(__file__).resolve().parent.parent
FIXTURES = HERE / "crates" / "fieldglass-netcdf" / "tests" / "fixtures"

# Immutable historical object in the public NOAA GOES-16 archive.
S3_KEY = (
    "ABI-L2-CMIPM/2023/001/18/"
    "OR_ABI-L2-CMIPM1-M6C13_G16_s20230011800281_e20230011800350_c20230011800425.nc"
)
SOURCE_URL = f"https://noaa-goes16.s3.amazonaws.com/{S3_KEY}"
CACHE = Path("/tmp") / Path(S3_KEY).name

# Center window of the 500x500 mesoscale grid: fully populated IR brightness
# temperatures (no fill), small enough to stay a few KB on disk.
ROW0, COL0, N = 238, 238, 24

# Globals worth keeping for a recognisable identity in the metadata view.
KEEP_GLOBALS = [
    "Conventions", "institution", "project", "production_site", "dataset_name",
    "platform_ID", "instrument_type", "scene_id", "spatial_resolution",
    "title", "summary", "keywords_vocabulary", "license",
    "time_coverage_start", "time_coverage_end",
    "timeline_id", "production_data_source",
]
DEG = math.pi / 180.0


# ---------------------------------------------------------------------------
# GOES-R ABI fixed grid (GOES-R PUG Vol. 3 §5.1.2.8; NOAA STAR) — independent
# NumPy transcription, sweep axis = x.
# ---------------------------------------------------------------------------
def goes_scan_to_lonlat(x, y, lon0_deg, h, r_eq, r_pol):
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


def fetch_source() -> Path:
    if not CACHE.exists():
        print(f"  downloading {SOURCE_URL}")
        # SOURCE_URL is composed only from string literals, so a static analyzer
        # can constant-fold it to a fixed https:// URL — no caller-controlled
        # value ever reaches the fetch. We read via single-argument urlopen and
        # write the body ourselves rather than the two-argument urlretrieve: only
        # the former lets the analyzer see the URL as the proven constant it is.
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


def main() -> int:
    if not FIXTURES.is_dir():
        raise SystemExit("run from the repo root")
    src_path = fetch_source()
    src = netCDF4.Dataset(src_path)

    rs, cs = slice(ROW0, ROW0 + N), slice(COL0, COL0 + N)
    sx = src.variables["x"]
    sy = src.variables["y"]
    scmi = src.variables["CMI"]
    sdqf = src.variables["DQF"]
    gm = src.variables["goes_imager_projection"]

    # Raw (unscaled) on-disk codes for the windowed coordinates + fields.
    for v in (sx, sy, scmi, sdqf):
        v.set_auto_mask(False)
        v.set_auto_scale(False)
    x_raw = np.asarray(sx[cs]).astype("i2")
    y_raw = np.asarray(sy[rs]).astype("i2")
    cmi_raw = np.asarray(scmi[rs, cs]).astype("i2")
    dqf_raw = np.asarray(sdqf[rs, cs]).astype("i1")

    out = FIXTURES / "goes16_abi_cmip.nc"
    with netCDF4.Dataset(out, "w", format="NETCDF4") as d:
        copy_attrs(src, d, KEEP_GLOBALS)
        d.setncattr("history", "center-window subset of the source GOES-16 file "
                               "(see tools/build_goes_real_fixture.py, NOTICE.md)")
        d.createDimension("y", N)
        d.createDimension("x", N)

        out_gm = d.createVariable("goes_imager_projection", "i4")
        copy_attrs(gm, out_gm)

        # We copy the raw on-disk int16/int8 codes verbatim and re-attach the
        # source CF attributes, so auto-scaling must be OFF on write — otherwise
        # netCDF4 would treat the codes as physical values and re-pack them.
        out_x = d.createVariable("x", "i2", ("x",))
        out_x.set_auto_maskandscale(False)
        copy_attrs(sx, out_x)
        out_x[:] = x_raw
        out_y = d.createVariable("y", "i2", ("y",))
        out_y.set_auto_maskandscale(False)
        copy_attrs(sy, out_y)
        out_y[:] = y_raw

        # Match the real GOES encoding: chunked + zlib, so the committed fixture
        # exercises the HDF5 chunked + deflate value path on real data. The
        # checked-in .nc + oracle are the source of truth; goes_real_world.rs
        # fails loudly if a regenerated fixture decodes to anything else.
        out_cmi = d.createVariable("CMI", "i2", ("y", "x"), zlib=True,
                                   complevel=1, chunksizes=(12, 12),
                                   fill_value=np.int16(scmi._FillValue))
        out_cmi.set_auto_maskandscale(False)
        copy_attrs(scmi, out_cmi)
        out_cmi[:, :] = cmi_raw
        out_dqf = d.createVariable("DQF", "i1", ("y", "x"), zlib=True,
                                   complevel=1, chunksizes=(12, 12),
                                   fill_value=np.int8(sdqf._FillValue))
        out_dqf.set_auto_maskandscale(False)
        copy_attrs(sdqf, out_dqf)
        out_dqf[:, :] = dqf_raw

    # ---- oracle: projection params, geolocation, value samples, structure ----
    lon0 = float(gm.longitude_of_projection_origin)
    pph = float(gm.perspective_point_height)
    r_eq = float(gm.semi_major_axis)
    r_pol = float(gm.semi_minor_axis)
    h = pph + r_eq
    xs_f, xo_f = float(sx.scale_factor), float(sx.add_offset)
    ys_f, yo_f = float(sy.scale_factor), float(sy.add_offset)
    x_rad = x_raw.astype("f8") * xs_f + xo_f
    y_rad = y_raw.astype("f8") * ys_f + yo_f

    geo_samples = []
    for j in (0, N // 2, N - 1):
        for i in (0, N // 2, N - 1):
            ll = goes_scan_to_lonlat(x_rad[i], y_rad[j], lon0, h, r_eq, r_pol)
            if ll is not None:
                geo_samples.append({"i": i, "j": j,
                                    "x": float(x_rad[i]), "y": float(y_rad[j]),
                                    "lat": ll[0], "lon": ll[1]})

    cmi_scale, cmi_off = float(scmi.scale_factor), float(scmi.add_offset)
    vlo, vhi = (int(v) for v in scmi.valid_range)
    flat = cmi_raw.reshape(-1)
    valid = (flat >= vlo) & (flat <= vhi)
    scaled = flat.astype("f8") * cmi_scale + cmi_off
    present = scaled[valid]
    cmi_samples = {str(k): (float(scaled[k]) if valid[k] else None)
                   for k in sample_indices(flat.size)}

    oracle = {
        "source": (
            f"NOAA GOES-16 ABI L2 CMIP Mesoscale band 13; center {N}x{N} window "
            f"of s3://noaa-goes16/{S3_KEY}. netCDF4 {netCDF4.__version__} "
            f"(libnetcdf {netCDF4.getlibversion().split()[0]}). Provenance in NOTICE.md."
        ),
        "projection": "geostationary",
        "longitude_of_projection_origin": lon0,
        "perspective_point_height": pph,
        "semi_major_axis": r_eq,
        "semi_minor_axis": r_pol,
        "h_metres": h,
        "sweep_angle_axis": str(gm.sweep_angle_axis),
        "nx": N, "ny": N,
        "x_scale_factor": xs_f, "x_add_offset": xo_f,
        "y_scale_factor": ys_f, "y_add_offset": yo_f,
        "geolocation_samples": geo_samples,
        "cmi": {
            "scale_factor": cmi_scale, "add_offset": cmi_off,
            "valid_range": [vlo, vhi], "units": str(scmi.units),
            "present_count": int(valid.sum()), "missing_count": int((~valid).sum()),
            "min_k": round(float(present.min()), 6), "max_k": round(float(present.max()), 6),
            "mean_k": round(float(present.mean()), 6),
            "scaled_samples": cmi_samples,
        },
        "dimensions": {"y": N, "x": N},
        "variables": sorted(
            ["goes_imager_projection", "x", "y", "CMI", "DQF"]
        ),
    }
    (FIXTURES / "goes16_abi_cmip.nc.oracle.json").write_text(
        json.dumps(oracle, indent=2) + "\n")
    src.close()
    print(f"  wrote {out.name} ({out.stat().st_size} bytes) + oracle")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
