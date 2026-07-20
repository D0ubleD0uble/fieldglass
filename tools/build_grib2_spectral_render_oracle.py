#!/usr/bin/env python3
"""Generate the inverse spherical-harmonic-transform oracle for #303.

eccodes cannot synthesise a grid from spectral coefficients (its geoiterator
returns "not yet implemented"), so there is no eccodes oracle. Instead this
computes the grid field DIRECTLY from ECMWF's definitive spectral definition
(https://confluence.ecmwf.int/display/UDOC/How+to+access+the+data+values+of+a+spherical+harmonic+field+in+GRIB+-+ecCodes+GRIB+FAQ):

    A(lambda, mu) = sum_{m=-T}^{T} sum_{n=|m|}^{T} X_{n,m} Pbar_n^m(mu) e^{i m lambda}
    with mu = sin(latitude),  X_{n,-m} = conj(X_{n,m}) / (-1)^m,
    and the normalisation  (1/2) integral_{-1}^{1} [Pbar_n^m(mu)]^2 dmu = 1.

Collapsing the +/-m pairs for a real field gives the implemented form:

    A = sum_n X_{n,0} Pbar_n^0
        + 2 sum_{m>=1} sum_n Pbar_n^m [Re(X_{n,m}) cos(m lambda) - Im(X_{n,m}) sin(m lambda)]

The fully-normalised associated Legendre recurrence was cross-validated three
ways during development: (a) the exact analytic cases Pbar_0^0=1,
Pbar_1^0=sqrt(3)*mu, Pbar_2^0=sqrt(5)*(3mu^2-1)/2; (b) the field synthesised
from single coefficients (constant, sqrt3*sin(lat), sqrt6*cos(lat)cos(lon));
(c) an independent pyshtools 'ortho' synthesis, which reproduces this field to
~5e-8 once the ECMWF *complex* coefficients X_{n,m} are mapped to pyshtools
*real* coefficients (m=0 direct; m>0 multiplies by sqrt(2), imag part negated)
and scaled by sqrt(4*pi). Pure numpy — no runtime deps beyond numpy.

Input coefficients: the committed spectral-decode oracle
`spectral_simple_t63.eccodes.ref.txt` (eccodes 2.34.1 `grib_get_data`, the
4160 T63 coefficients in ECMWF m-major (real, imag) order).

Output: `spectral_render_t63.oracle.txt` — the field on the fixed regular
lat/lon grid defined by GRID_LATS x GRID_LONS below, row-major (lat outer,
lon inner), one value per line. The Rust test rebuilds the identical grid.

Regenerate:  python3 tools/build_grib2_spectral_render_oracle.py
"""

from __future__ import annotations

import pathlib

import numpy as np

T = 63
FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)
COEFFS_REF = FIXTURES / "spectral_simple_t63.eccodes.ref.txt"
OUT = FIXTURES / "spectral_render_t63.oracle.txt"

# The fixed test grid: 5-degree regular lat/lon, latitudes 90..-90 (37) and
# longitudes 0..355 (72). Poles included (the recurrence handles mu = +/-1).
GRID_LATS = [90.0 - 5.0 * i for i in range(37)]
GRID_LONS = [5.0 * j for j in range(72)]


def plm_bar(mu: float) -> np.ndarray:
    """Fully-normalised associated Legendre Pbar[n, m], (1/2)∫Pbar^2 dmu = 1."""
    p = np.zeros((T + 1, T + 1))
    s = np.sqrt(max(0.0, 1.0 - mu * mu))
    p[0, 0] = 1.0
    for m in range(1, T + 1):
        p[m, m] = np.sqrt((2 * m + 1) / (2 * m)) * s * p[m - 1, m - 1]
    for m in range(0, T + 1):
        if m < T:
            p[m + 1, m] = np.sqrt(2 * m + 3) * mu * p[m, m]
        for n in range(m + 2, T + 1):
            a = np.sqrt((2 * n + 1) * (2 * n - 1) / ((n - m) * (n + m)))
            b = np.sqrt(
                (2 * n + 1) * (n + m - 1) * (n - m - 1)
                / ((2 * n - 3) * (n - m) * (n + m))
            )
            p[n, m] = a * mu * p[n - 1, m] - b * p[n - 2, m]
    return p


def load_coeffs() -> np.ndarray:
    raw = np.loadtxt(COEFFS_REF)
    cilm = np.zeros((2, T + 1, T + 1))
    idx = 0
    for m in range(T + 1):
        for n in range(m, T + 1):
            cilm[0, n, m] = raw[idx]
            cilm[1, n, m] = raw[idx + 1]
            idx += 2
    assert idx == len(raw), f"consumed {idx} of {len(raw)}"
    return cilm


def main() -> None:
    cilm = load_coeffs()
    out = []
    for lat in GRID_LATS:
        p = plm_bar(np.sin(np.deg2rad(lat)))
        for lon in GRID_LONS:
            lam = np.deg2rad(lon)
            a = float(np.dot(cilm[0, :, 0], p[:, 0]))  # m = 0
            for m in range(1, T + 1):
                cm, sm = np.cos(m * lam), np.sin(m * lam)
                col = p[m:, m]
                a += 2.0 * float(
                    np.dot(col, cilm[0, m:, m] * cm - cilm[1, m:, m] * sm)
                )
            out.append(a)
    OUT.write_text("\n".join(f"{v:.9e}" for v in out) + "\n")
    arr = np.array(out)
    print(f"wrote {OUT.name}: {len(out)} values "
          f"({len(GRID_LATS)}x{len(GRID_LONS)} grid)")
    print(f"field min/max/mean = {arr.min():.3f} / {arr.max():.3f} / {arr.mean():.3f}")


if __name__ == "__main__":
    main()
