#!/usr/bin/env python3
"""Generate the CF packed-data fixture (`cf_packed_data.nc`) and its oracle.

Targets CF `scale_factor` / `add_offset` + `valid_range` unpacking of a **data**
variable (#184). The companion projected fixtures (#168) already exercise the CF
convention on *coordinate* arrays; this one packs the rendered field itself, as
GOES `Rad`, MERRA-2, and ERA5 do.

Self-generated (no upstream provenance or licensing constraint); a deliberately
tiny toy grid. The data variable is a scaled `int16` carrying `scale_factor`,
`add_offset`, `_FillValue` and `valid_range`, plus 1-D `lat`/`lon` coordinates.

The oracle records both the raw on-disk codes and the physical values the
canonical `netCDF4` library produces with auto mask+scale on (the CF unpacking
the Rust decode + `unpack_cf_data` must reproduce), so the test needs no netCDF4
at runtime. Run from the repo root (needs `netCDF4` + `numpy`):

    python3 tools/build_netcdf_cf_packed_fixture.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

FIXTURES = Path("crates/fieldglass-netcdf/tests/fixtures")
NC = FIXTURES / "cf_packed_data.nc"

# Packed int16 codes laid out (lat, lon) = (3, 4). Deliberately spans the
# masking cases: below valid_range, the inclusive bounds themselves, above
# valid_range, and the _FillValue sentinel.
PACKED = np.array(
    [
        [-50, 0, 2500, 10000],   # -50 < valid_min(0) -> masked
        [15000, -9999, 5000, 9999],  # 15000 > valid_max(10000), -9999 fill
        [1, 7500, 250, 10000],   # all in range
    ],
    dtype="i2",
)
# A power-of-two scale (2**-4) is exact in both float32 and float64, so the
# physical values are bit-identical whether unpacked in libnetcdf's float32
# domain or the Rust f64 path — the oracle is exact, not tolerance-bound. The
# lossy small-magnitude display case (GOES ≈ 6.7e-7) is covered by the
# `format_float` unit tests, not this fixture.
SCALE = np.float32(0.0625)
OFFSET = np.float32(250.0)
FILL = np.int16(-9999)
VALID_RANGE = np.array([0, 10000], dtype="i2")


def build() -> None:
    if NC.exists():
        NC.unlink()
    ny, nx = PACKED.shape
    with netCDF4.Dataset(NC, "w", format="NETCDF3_CLASSIC") as d:
        d.title = "Synthetic CF packed-data fixture for #184"
        d.createDimension("lat", ny)
        d.createDimension("lon", nx)

        lat = d.createVariable("lat", "f8", ("lat",))
        lat.units = "degrees_north"
        lat.standard_name = "latitude"
        lat[:] = np.linspace(10.0, 30.0, ny)

        lon = d.createVariable("lon", "f8", ("lon",))
        lon.units = "degrees_east"
        lon.standard_name = "longitude"
        lon[:] = np.linspace(-100.0, -70.0, nx)

        temp = d.createVariable("temp", "i2", ("lat", "lon"), fill_value=FILL)
        temp.units = "kelvin"
        temp.standard_name = "air_temperature"
        temp.scale_factor = SCALE
        temp.add_offset = OFFSET
        temp.valid_range = VALID_RANGE
        temp.set_auto_maskandscale(False)  # write the raw packed codes verbatim
        temp[:] = PACKED


def physical_oracle() -> dict:
    """Read back through libnetcdf's auto mask+scale — the CF physical values."""
    with netCDF4.Dataset(NC) as d:
        v = d.variables["temp"]  # auto mask+scale on by default
        arr = v[:]
        mask = np.ma.getmaskarray(arr)
        flat = np.asarray(arr.filled(np.nan)).reshape(-1)
        physical = [None if m else float(x) for x, m in zip(flat, mask.reshape(-1))]

        raw = d.variables["temp"]
        raw.set_auto_maskandscale(False)
        raw_flat = np.asarray(raw[:]).reshape(-1)
        # Only _FillValue is masked at the decode stage; valid_range masking is
        # applied later by unpack_cf_data, so the decode oracle keeps in-range
        # *and* out-of-range codes, nulling only the fill sentinel.
        decoded = [None if int(c) == int(FILL) else float(c) for c in raw_flat]
    return {
        "shape": list(PACKED.shape),
        "scale_factor": float(SCALE),
        "add_offset": float(OFFSET),
        "fill_value": float(FILL),
        "valid_range": [int(VALID_RANGE[0]), int(VALID_RANGE[1])],
        "decoded_raw": decoded,
        "physical": physical,
    }


def main() -> int:
    if not FIXTURES.is_dir():
        print("run from the repo root", file=sys.stderr)
        return 1
    build()
    doc = {
        "source": (
            f"netCDF4 {netCDF4.__version__} (libnetcdf "
            f"{netCDF4.getlibversion().split()[0]}). CF unpacking oracle for "
            "data-variable scale_factor/add_offset + valid_range (#184). "
            "Self-generated; provenance in NOTICE.md."
        ),
        "temp": physical_oracle(),
    }
    (FIXTURES / "cf_packed_data.nc.oracle.json").write_text(
        json.dumps(doc, indent=2) + "\n"
    )
    print(f"wrote {NC} and its oracle")
    return 0


if __name__ == "__main__":
    sys.exit(main())
