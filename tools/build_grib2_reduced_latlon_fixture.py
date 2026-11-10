#!/usr/bin/env python3
"""Build the GRIB2 reduced lat/lon fixture (#1760).

A reduced lat/lon grid is template 3.0 with `Ni` coded missing (all ones) and
§3's optional list of numbers of points, one entry per row (WMO FM 92 GRIB2,
§3 octets 11-12 and Code Table 3.11). ECMWF's wave model writes one. eccodes
2.34.1 ships a sample of it, `samples/reduced_ll_sfc_grib2.tmpl`, but that
sample is a 501-row, 313362-point constant field: too big to commit and no
value oracle. This builds a small one from it.

The grid is global at 10 degrees: 19 rows from 90N to 90S, each row's points
spread around the full circle. The two polar rows hold **no** points, the way
the wave grid's polar rows do, so the expansion's empty-row path is on the
decode path. The widest row is 36 points, so the raster is 36 x 19.

Values are `100*row + point`, so every stored point names its own row and
position, and a decode that drops, duplicates or shifts a point shows at that
point. A bitmap masks every point where `(7*row + point) % 11 == 0`, so the
mask is carried through the expansion too.

**Encode with the wheel, verify with the pin.** Setting `pl` needs
`codes_set_array`, which the CLI has no equivalent for, so the PyPI wheel writes
the message; eccodes 2.34.1 on PATH is then the oracle that says what was
written. The builder asserts the keys below against the pin rather than
printing them.

Usage:  python3 tools/build_grib2_reduced_latlon_fixture.py
Needs:  the `eccodes` PyPI wheel (encoding) and eccodes 2.34.1 on PATH (oracle).
"""

from __future__ import annotations

import math
import pathlib
import subprocess
import sys

import eccodes as ec
import numpy as np

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)
SAMPLE = pathlib.Path("/usr/share/eccodes/samples/reduced_ll_sfc_grib2.tmpl")
OUT = FIXTURES / "reduced_latlon_10deg.grib2"

ROWS = 19
STEP_DEG = 10.0
MISSING = 9999.0

EXPECTED = {
    "gridType": "reduced_ll",
    "gridDefinitionTemplateNumber": "0",
    "Ni": "MISSING",
    "Nj": "19",
    "PLPresent": "1",
    "interpretationOfNumberOfPoints": "1",
    "bitmapPresent": "1",
}


def row_widths() -> list[int]:
    """Points per row: `round(36 cos(lat))`, with no points on the poles."""
    widths = []
    for j in range(ROWS):
        lat = 90.0 - STEP_DEG * j
        widths.append(0 if abs(lat) == 90.0 else round(36 * math.cos(math.radians(lat))))
    return widths


def oracle(path: pathlib.Path, key: str) -> str:
    """Ask the *pinned* eccodes CLI what it reads back."""
    return subprocess.run(
        ["grib_get", "-p", key, str(path)],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def main() -> int:
    if not SAMPLE.is_file():
        raise SystemExit(f"{SAMPLE} is missing; install eccodes' sample files")
    widths = row_widths()
    total = sum(widths)
    values = []
    for j, n in enumerate(widths):
        for i in range(n):
            values.append(MISSING if (7 * j + i) % 11 == 0 else float(100 * j + i))

    with SAMPLE.open("rb") as source:
        handle = ec.codes_grib_new_from_file(source)
    try:
        ec.codes_set(handle, "Nj", ROWS)
        ec.codes_set(handle, "latitudeOfFirstGridPointInDegrees", 90.0)
        ec.codes_set(handle, "longitudeOfFirstGridPointInDegrees", 0.0)
        ec.codes_set(handle, "latitudeOfLastGridPointInDegrees", -90.0)
        ec.codes_set(handle, "longitudeOfLastGridPointInDegrees", 360.0 - STEP_DEG)
        ec.codes_set(handle, "jDirectionIncrementInDegrees", STEP_DEG)
        ec.codes_set(handle, "numberOfDataPoints", total)
        ec.codes_set_array(handle, "pl", np.array(widths, dtype=np.int64))
        ec.codes_set(handle, "bitmapPresent", 1)
        ec.codes_set(handle, "missingValue", MISSING)
        ec.codes_set(handle, "bitsPerValue", 16)
        ec.codes_set_values(handle, np.array(values, dtype=np.float64))
        OUT.write_bytes(ec.codes_get_message(handle))
    finally:
        ec.codes_release(handle)

    for key, want in EXPECTED.items():
        got = oracle(OUT, key)
        if got != want:
            raise SystemExit(f"{OUT.name}: eccodes reads {key}={got!r}, expected {want!r}")
    # §3's count is every point; §5's (`numberOfValues`) only the ones the
    # bitmap keeps.
    present = sum(1 for v in values if v != MISSING)
    for key, want in (("numberOfDataPoints", total), ("numberOfValues", present)):
        got = oracle(OUT, key)
        if got != str(want):
            raise SystemExit(f"{OUT.name}: eccodes reads {key}={got!r}, expected {want}")
    print(
        f"{OUT.name}: {ROWS} rows, {total} points, widest {max(widths)} "
        "(verified against the pin)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
