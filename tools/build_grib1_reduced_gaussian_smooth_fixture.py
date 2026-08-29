#!/usr/bin/env python3
"""Build a GRIB1 reduced Gaussian fixture whose field actually varies.

The crate already has `reduced_gg_n32.grib1`, and it is deliberately a
*constant* field (`grib_set -d 285.5`): all-values-equal-the-reference is a real
`grid_simple` edge case, and pinning it is the whole point of that fixture. But
a constant field has no isolines, so it cannot show that contouring works on a
reduced grid — and it was the only GRIB1 reduced Gaussian file in the repo, so
"contours draw for GRIB1 reduced grids" had nothing to be tested against. A
tester turning contours on saw nothing and could not tell a fixed decoder from a
broken one.

This builds the sibling: same N32 grid, same `PL` list, a smooth analytic field
in its place.

**Why smooth rather than a ramp.** The GRIB2 octahedral fixture uses a sawtooth
(`index mod 50`) because its job is to be an exact value oracle — a decode that
drops or reorders a point cannot hide behind it. This fixture's job is the
opposite: it is read by eye. A smooth field draws isolines a person can judge as
following the map, and it is *more* diagnostic for the row-widening bug than a
sawtooth is, because rows are laid out so that a mis-widened row breaks an
otherwise continuous contour into a visible kink. The zonal term gives east-west
bands, so a row placed wrong breaks a band; the `sin(2*lon)` term puts two
crests around the globe, so a purely longitudinal error cannot hide behind
straight zonal lines the way it could in a lat-only field.

**Encode with the wheel, verify with the pin.** Values come from the wheel's
geoiterator so they line up with the grid's own point ordering rather than an
ordering this script assumed; eccodes 2.34.1 on PATH is then the oracle for what
was written, as `NOTICE.md` requires.

Usage:  python3 tools/build_grib1_reduced_gaussian_smooth_fixture.py
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
    "crates/fieldglass-grib1/tests/fixtures"
)

# The same stock N32 sample `reduced_gg_n32.grib1` is cut from, so the two
# fixtures differ in exactly one thing: the field.
SAMPLE = pathlib.Path("/usr/share/eccodes/samples/reduced_gg_pl_32_grib1.tmpl")
OUT = FIXTURES / "reduced_gg_n32_smooth.grib1"

# 16 bits resolves the field to about 1 mK over its ~50 K span — far finer than
# any contour interval a reader would pick, so the isolines are limited by the
# grid, not by the packing. It puts the message at about 12 kB.
BITS_PER_VALUE = 16

# What the pin must say about the result. Asserted rather than printed: a
# builder that quietly wrote a regular grid, or lost the `PL` list, would leave
# the reduced path untested while looking like it had covered it.
EXPECTED = {"gridType": "reduced_gg", "N": "32", "Nj": "64", "numberOfDataPoints": "6114"}


def field(lat_deg: float, lon_deg: float) -> float:
    """A smooth planetary-wave temperature field, in kelvin.

    Zonal bands from the `sin^2` term (about 248 K at the poles, 288 K at the
    equator) with a two-crest wave riding on them, tapered by `cos(lat)` so the
    wave vanishes at the poles rather than fighting the convergence of the
    meridians there.
    """
    lat = math.radians(lat_deg)
    lon = math.radians(lon_deg)
    return 288.0 - 40.0 * math.sin(lat) ** 2 + 10.0 * math.cos(lat) * math.sin(2.0 * lon)


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

    with SAMPLE.open("rb") as source:
        handle = ec.codes_grib_new_from_file(source)
    try:
        # Take the coordinates from the grid itself: the reduced layout is a
        # different number of points per row, so the point order is the one
        # thing this script must not guess at.
        points = ec.codes_grib_get_data(handle)
        values = np.array([field(p.lat, p.lon) for p in points], dtype=np.float64)

        ec.codes_set(handle, "bitsPerValue", BITS_PER_VALUE)
        ec.codes_set_values(handle, values)
        OUT.write_bytes(ec.codes_get_message(handle))
    finally:
        ec.codes_release(handle)

    for key, want in EXPECTED.items():
        got = oracle(OUT, key)
        if got != want:
            raise SystemExit(f"{OUT.name}: eccodes reads {key}={got!r}, expected {want!r}")

    # The field must actually vary, which is the entire reason this fixture
    # exists; a builder that wrote a constant would reproduce the gap it closes.
    spread = float(values.max() - values.min())
    if spread < 1.0:
        raise SystemExit(f"{OUT.name}: field spans only {spread:.3f} K; it must vary")

    print(
        f"{OUT.name}: {len(values)} points, "
        f"{values.min():.2f}..{values.max():.2f} K, "
        f"{OUT.stat().st_size} bytes (verified against the pin)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
