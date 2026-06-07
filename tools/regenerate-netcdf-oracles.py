#!/usr/bin/env python3
"""Regenerate value oracles for the classic-NetCDF test fixtures.

For each classic fixture under
``crates/fieldglass-netcdf/tests/fixtures/`` this writes a sibling
``{fixture}.values.json`` capturing, per variable, the on-disk decode that
the canonical Unidata ``netCDF4`` library (which wraps ``libnetcdf``)
produces: type, shape, fill value, present/missing counts, value statistics,
and a handful of anchored samples in C (row-major / on-disk) order.

These are the *value-decode targets* for NetCDF classic value decode (#108):
once that lands, the decoder reads each variable from its ``begin`` offset and
must reproduce these numbers. Like the eccodes snapshots, the oracles are
committed so the Rust test suite needs no netCDF4 at runtime.

Run from the repo root (needs ``netCDF4``; see fixtures/NOTICE.md):

    python3 tools/regenerate-netcdf-oracles.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

import netCDF4  # type: ignore
import numpy as np

# Fixtures to snapshot. ERSST CDF-2/CDF-5 are byte-for-byte re-encodes of the
# CDF-1 values, so one oracle covers all three (asserted by the tests).
FIXTURES = [
    "netcdf_classic_dummy.nc",
    "ersst_v5_187001_cdf1.nc",
]


def sample_indices(n: int) -> list[int]:
    """Anchored flat indices spanning an n-element array (C order)."""
    if n == 0:
        return []
    if n <= 5:
        return list(range(n))
    return sorted({0, 1, n // 2, n - 2, n - 1})


def variable_oracle(v: "netCDF4.Variable") -> dict:
    v.set_auto_mask(False)  # raw on-disk values, fills included
    raw = np.asarray(v[:])
    flat = raw.reshape(-1)
    nc_type = v.dtype
    out: dict = {
        "nc_type": str(np.dtype(nc_type)),
        "shape": list(v.shape),
        "dimensions": list(v.dimensions),
        "count": int(flat.size),
    }
    fill = getattr(v, "_FillValue", None)
    if fill is not None:
        out["fill_value"] = float(fill) if np.issubdtype(flat.dtype, np.number) else None

    if flat.dtype.kind == "S":  # NC_CHAR: store the text, not numeric stats
        out["text"] = b"".join(flat.tolist()).decode("latin-1").rstrip("\x00")
        return out

    if flat.size == 0:
        out["present_count"] = 0
        out["missing_count"] = 0
        return out

    if fill is not None:
        present = flat[flat != fill]
        missing = int((flat == fill).sum())
    else:
        present = flat
        missing = 0
    out["present_count"] = int(present.size)
    out["missing_count"] = missing
    if present.size:
        out["min"] = round(float(present.min()), 8)
        out["max"] = round(float(present.max()), 8)
        out["mean"] = float(present.mean())
    # Samples are raw values (fills included) so the decoder can match the
    # exact on-disk sequence, including masked positions.
    out["samples"] = {str(i): float(flat[i]) for i in sample_indices(flat.size)}
    return out


def main() -> int:
    fixtures_dir = Path("crates/fieldglass-netcdf/tests/fixtures")
    if not fixtures_dir.is_dir():
        print(f"run from the repo root; {fixtures_dir} not found", file=sys.stderr)
        return 1
    for name in FIXTURES:
        path = fixtures_dir / name
        ds = netCDF4.Dataset(path)
        doc = {
            "source": (
                f"netCDF4 {netCDF4.__version__} (libnetcdf "
                f"{netCDF4.getlibversion().split()[0]}). Value-decode oracle for "
                f"NetCDF classic value decode (#108). Provenance in NOTICE.md."
            ),
            "data_model": ds.data_model,
            "variables": {n: variable_oracle(v) for n, v in ds.variables.items()},
        }
        ds.close()
        out_path = fixtures_dir / f"{name}.values.json"
        out_path.write_text(json.dumps(doc, indent=2) + "\n")
        print(f"wrote {out_path} ({len(doc['variables'])} variables)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
