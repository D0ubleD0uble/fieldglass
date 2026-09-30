#!/usr/bin/env python3
"""Build the T383 spectral fixtures and their band-limited-map oracle (#637).

A spectral map is synthesised only up to the wavenumbers its 0.5-degree raster
can carry (T359), and a probe evaluates the full sum. This script makes one
fixture per GRIB edition whose truncation is past that limit, and records what
an independent synthesis (pyshtools) says both of those should be.

Steps, each checkable on its own:

1. **Fixtures.** The committed T63 `spectral_simple` fixtures are re-truncated
   to J = K = M = 383 and given 147,840 synthetic coefficients with the
   `eccodes` PyPI wheel (`codes_set_values`, which the pinned 2.34.1 CLI cannot
   do for a spectral field). The coefficients are seeded normal draws with a
   flat spectrum, so the band that truncation removes (n = 360..383) carries
   about an eighth of the field's variance and removing it is visible; the
   (0,0) term is 280, a temperature-like mean. 8 bits per value keeps each
   fixture at ~148 KB.
2. **Coefficients as eccodes decodes them.** The pinned eccodes 2.34.1 CLI
   (`grib_get_data`) prints each fixture's coefficients. Those, not the values
   written in step 1, feed the oracle, so the oracle and the Rust decoder start
   from the same quantised numbers and a decode error cannot hide.
3. **pyshtools synthesis.** ECMWF's complex coefficients X_{n,m} (normalised
   (1/2) int Pbar^2 dmu = 1, no Condon-Shortley phase) map to pyshtools' real
   orthonormal ones as C_{n,0} = sqrt(4 pi) X_{n,0} and, for m > 0,
   C_{n,m} = sqrt(2) sqrt(4 pi) Re X_{n,m}, S_{n,m} = -sqrt(2) sqrt(4 pi) Im X_{n,m}.
   The mapping is checked first against the committed T63 render oracle, which
   was computed from the ECMWF formula directly
   (`build_grib2_spectral_render_oracle.py`); the script refuses to write
   anything if the two disagree.

Output, per edition, `spectral_simple_t383.truncation.oracle.json`:

* `map`: the field band-limited to n <= 359 on the 5-degree grid (37 x 72,
  latitudes 90..-90, longitudes 0..355), latitude-major. Every one of those
  points is a node of the 0.5-degree synthesis grid, so the Rust test reads the
  map there.
* `points`: the full sum (`full`, n <= 383) and the band-limited sum
  (`truncated`, n <= 359) at a handful of points, some on the synthesis grid
  and some off it, for the exact probe.

Two more oracles, for `fieldglass-core`'s transform past where plain `f64`
holds (about T1810), from coefficients both sides generate with the same
64-bit LCG so nothing large is committed:

* `sht_t3000_extended_range.oracle.json`: the full sum at T3000 in 80-bit
  `long double`, by the same recurrence in wider arithmetic.
* `sht_t2500_pyshtools.oracle.json`: the full sum at T2500 by pyshtools
  (`MakeGridPoint`, Holmes-Featherstone scaling), an independent algorithm
  inside its documented range of about degree 2800.

Regenerate (needs numpy, pyshtools, the `eccodes` wheel >= 2.48, and the pinned
eccodes 2.34.1 CLI on PATH):

    python3 tools/build_spectral_truncation_oracle.py

or only the two `fieldglass-core` oracles, which need numpy and pyshtools:

    python3 tools/build_spectral_truncation_oracle.py --core-only
"""

from __future__ import annotations

import json
import math
import pathlib
import subprocess
import sys

import numpy as np
import pyshtools

ROOT = pathlib.Path(__file__).resolve().parent.parent
T = 383
BAND_LIMIT = 359
BITS = 8
SEED = 637

EDITIONS = [
    ROOT / "crates/fieldglass-grib2/tests/fixtures",
    ROOT / "crates/fieldglass-grib1/tests/fixtures",
]
SOURCES = {
    EDITIONS[0]: "spectral_simple_t63.grib2",
    EDITIONS[1]: "spectral_simple_t63.grib1",
}
TARGETS = {
    EDITIONS[0]: "spectral_simple_t383.grib2",
    EDITIONS[1]: "spectral_simple_t383.grib1",
}
ORACLE = "spectral_simple_t383.truncation.oracle.json"

GRID_LATS = [90.0 - 5.0 * i for i in range(37)]
GRID_LONS = [5.0 * j for j in range(72)]

# On the 0.5-degree synthesis grid (the Session probe snaps to these), then off
# it (the reader's point evaluation takes any point).
POINTS = [
    (90.0, 0.0),
    (-90.0, 0.0),
    (0.0, 0.0),
    (45.5, 120.0),
    (-30.0, 359.5),
    (12.5, 200.5),
    (-67.0, 88.0),
    (37.123, 14.321),
    (-61.7, 250.05),
    (0.26, 179.99),
    (-89.9, 10.0),
    (89.95, 300.0),
]


def coefficients() -> np.ndarray:
    rng = np.random.default_rng(SEED)
    values = rng.normal(0.0, 0.1, (T + 1) * (T + 2))
    # m = 0 is the first block of T + 1 (re, im) pairs; a real field has no
    # imaginary part there.
    values[1 : 2 * (T + 1) : 2] = 0.0
    values[0] = 280.0
    return values


def build_fixture(directory: pathlib.Path) -> pathlib.Path:
    import eccodes  # only the fixtures need it; `--core-only` runs without

    source = directory / SOURCES[directory]
    target = directory / TARGETS[directory]
    with source.open("rb") as f:
        handle = eccodes.codes_grib_new_from_file(f)
    try:
        for key in ("J", "K", "M"):
            eccodes.codes_set(handle, key, T)
        eccodes.codes_set(handle, "bitsPerValue", BITS)
        eccodes.codes_set_values(handle, coefficients())
        with target.open("wb") as f:
            eccodes.codes_write(handle, f)
    finally:
        eccodes.codes_release(handle)
    return target


def eccodes_coefficients(path: pathlib.Path) -> np.ndarray:
    # The pinned CLI. It prints "Wrong number of points" on stderr for every
    # spectral message, which is its geoiterator declining, not a decode error.
    out = subprocess.run(
        ["grib_get_data", str(path)], check=True, capture_output=True, text=True, encoding="utf-8"
    ).stdout.splitlines()
    assert out[0].strip() == "Value", out[0]
    return np.array([float(v) for v in out[1:]])


def to_pyshtools(raw: np.ndarray, t: int) -> np.ndarray:
    cilm = np.zeros((2, t + 1, t + 1))
    scale = math.sqrt(4.0 * math.pi)
    idx = 0
    for m in range(t + 1):
        for n in range(m, t + 1):
            re, im = raw[idx], raw[idx + 1]
            if m == 0:
                cilm[0, n, 0] = scale * re
            else:
                cilm[0, n, m] = math.sqrt(2.0) * scale * re
                cilm[1, n, m] = -math.sqrt(2.0) * scale * im
            idx += 2
    assert idx == len(raw), f"consumed {idx} of {len(raw)}"
    return cilm


def evaluate(cilm: np.ndarray, lat: float, lon: float, lmax: int) -> float:
    return float(pyshtools.expand.MakeGridPoint(cilm, lat, lon, lmax=lmax, norm=4, csphase=1))


def check_mapping() -> None:
    """The coefficient mapping, against the T63 oracle computed from ECMWF's
    formula directly. Refuse to write anything if they disagree."""
    fixtures = EDITIONS[0]
    raw = np.loadtxt(fixtures / "spectral_simple_t63.eccodes.ref.txt")
    want = np.loadtxt(fixtures / "spectral_render_t63.oracle.txt")
    cilm = to_pyshtools(raw, 63)
    got = np.array([evaluate(cilm, lat, lon, 63) for lat in GRID_LATS for lon in GRID_LONS])
    worst = float(np.max(np.abs(got - want)))
    assert worst < 1e-6, f"pyshtools disagrees with the ECMWF-formula T63 oracle by {worst}"
    print(f"mapping checked against the T63 formula oracle: max |diff| = {worst:.2e}")


EXTENDED_T = 3000
EXTENDED_POINTS = [(60.0, 120.0), (80.0, 10.0), (-70.0, 300.0), (37.5, 200.0), (0.25, 45.0)]
EXTENDED_OUT = ROOT / "crates/fieldglass-core/tests/fixtures/sht_t3000_extended_range.oracle.json"


def lcg_coefficients(t: int) -> list[float]:
    """The coefficients `sht_extended_range.rs` builds: a 64-bit LCG, its top
    53 bits as a uniform value in [-1, 1). Spelled the same on both sides so
    nothing large has to be committed."""
    mask = (1 << 64) - 1
    state = (0x9E3779B97F4A7C15 ^ t) & mask
    out = []
    for _ in range((t + 1) * (t + 2)):
        state = (state * 6364136223846793005 + 1442695040888963407) & mask
        out.append((state >> 11) / float(1 << 52) - 1.0)
    return out


def extended_range_oracle() -> None:
    """The full sum at T3000 in x87 80-bit `long double`, whose 15-bit exponent
    holds every sectoral term at these latitudes (cos(80°)^3000 is 1e-2282)
    without the underflow that wrecks the `f64` recurrence past T1927. Columns
    are stepped together, one `n - m` at a time, so this is numpy vector work
    rather than a 4.5-million-step Python loop. Independent of pyshtools, whose
    own Legendre routine is documented as accurate to about degree 2800."""
    ld = np.longdouble
    # x87 extended has a 15-bit exponent (maxexp 16384). A platform whose
    # `long double` is plain `double` (MSVC, some ARM ABIs) would underflow
    # exactly as the `f64` recurrence does and write a wrong oracle.
    assert np.finfo(ld).maxexp >= 16384, f"long double is too narrow: {np.finfo(ld)}"
    t = EXTENDED_T
    raw = np.array(lcg_coefficients(t), dtype=ld)
    ms = np.arange(t + 1)
    offset = 2 * (ms * (t + 1) - ms * (ms - 1) // 2)
    points = []
    for lat, lon in EXTENDED_POINTS:
        # The inputs `f64` gives the Rust side, widened, so the two differ only
        # in the arithmetic.
        mu = ld(math.sin(math.radians(lat)))
        s = ld(math.sqrt(max(0.0, 1.0 - float(mu) * float(mu))))
        lam = ld(math.radians(lon))
        factors = np.sqrt((2 * ms[:-1].astype(ld) + 3) / (2 * ms[:-1].astype(ld) + 2)) * s
        pmm = np.concatenate([[ld(1)], np.cumprod(factors)])
        re = pmm * raw[offset]
        im = pmm * raw[offset + 1]
        p2 = pmm[:-1].copy()
        p1 = np.sqrt(2 * ms[:-1].astype(ld) + 3) * mu * p2
        re[:-1] += p1 * raw[offset[:-1] + 2]
        im[:-1] += p1 * raw[offset[:-1] + 3]
        for k in range(2, t + 1):
            m = ms[: t + 1 - k].astype(ld)
            n = m + k
            a = np.sqrt((2 * n + 1) * (2 * n - 1) / ((n - m) * (n + m)))
            b = np.sqrt((2 * n + 1) * (n + m - 1) * (n - m - 1) / ((2 * n - 3) * (n - m) * (n + m)))
            p = a * mu * p1[: t + 1 - k] - b * p2[: t + 1 - k]
            idx = offset[: t + 1 - k] + 2 * k
            re[: t + 1 - k] += p * raw[idx]
            im[: t + 1 - k] += p * raw[idx + 1]
            p2, p1 = p1[: t + 1 - k], p
        mm = ms[1:].astype(ld)
        value = re[0] + 2 * np.sum(re[1:] * np.cos(mm * lam) - im[1:] * np.sin(mm * lam))
        points.append({"lat": lat, "lon": lon, "value": float(value)})
    EXTENDED_OUT.parent.mkdir(parents=True, exist_ok=True)
    EXTENDED_OUT.write_text(
        json.dumps({"truncation": t, "points": points}, indent=1) + "\n", encoding="utf-8"
    )
    print(f"wrote {EXTENDED_OUT.relative_to(ROOT)}: {points}")


PYSHTOOLS_T = 2500
PYSHTOOLS_LATS = [89.0, 80.0, 60.0, 37.5, 0.25, -70.0]
PYSHTOOLS_LONS = [10.0, 120.0, 300.0]
PYSHTOOLS_OUT = ROOT / "crates/fieldglass-core/tests/fixtures/sht_t2500_pyshtools.oracle.json"


def pyshtools_high_degree_oracle() -> None:
    """The full sum at T2500 on a small grid, by pyshtools rather than by any
    column recurrence of ours: an algorithm-independent check of the transform
    past where plain `f64` holds (about T1810), inside pyshtools' documented
    range (about degree 2800). Latitudes are latitude-major, as the Rust grid
    transform returns them."""
    t = PYSHTOOLS_T
    cilm = to_pyshtools(np.array(lcg_coefficients(t)), t)
    values = [evaluate(cilm, lat, lon, t) for lat in PYSHTOOLS_LATS for lon in PYSHTOOLS_LONS]
    PYSHTOOLS_OUT.write_text(
        json.dumps(
            {
                "truncation": t,
                "pyshtools": pyshtools.__version__,
                "lats": PYSHTOOLS_LATS,
                "lons": PYSHTOOLS_LONS,
                "values": values,
            },
            indent=1,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {PYSHTOOLS_OUT.relative_to(ROOT)}: {values}")


def main() -> None:
    extended_range_oracle()
    pyshtools_high_degree_oracle()
    if "--core-only" in sys.argv[1:]:
        return
    check_mapping()
    for directory in EDITIONS:
        fixture = build_fixture(directory)
        raw = eccodes_coefficients(fixture)
        assert len(raw) == (T + 1) * (T + 2), len(raw)
        cilm = to_pyshtools(raw, T)
        field_map = [evaluate(cilm, lat, lon, BAND_LIMIT) for lat in GRID_LATS for lon in GRID_LONS]
        points = [
            {
                "lat": lat,
                "lon": lon,
                "full": evaluate(cilm, lat, lon, T),
                "truncated": evaluate(cilm, lat, lon, BAND_LIMIT),
            }
            for lat, lon in POINTS
        ]
        oracle = {
            "fixture": fixture.name,
            "truncation": T,
            "bandLimit": BAND_LIMIT,
            "gridLats": GRID_LATS,
            "gridLons": GRID_LONS,
            "map": field_map,
            "points": points,
        }
        out = directory / ORACLE
        out.write_text(json.dumps(oracle, indent=1) + "\n", encoding="utf-8")
        removed = [abs(p["full"] - p["truncated"]) for p in points]
        print(
            f"wrote {fixture.relative_to(ROOT)} ({fixture.stat().st_size} bytes) and "
            f"{out.relative_to(ROOT)}: map {min(field_map):.3f}..{max(field_map):.3f}, "
            f"|full - truncated| at the points {min(removed):.3f}..{max(removed):.3f}"
        )


if __name__ == "__main__":
    main()
