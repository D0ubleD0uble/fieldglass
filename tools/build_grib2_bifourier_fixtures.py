#!/usr/bin/env python3
"""Build GRIB2 bi-Fourier (DRT 5.53) decode fixtures.

Bi-Fourier spectral packing (`bifourier_complex`, §3.61/62/63 + §5.53) has no
public sample data and the eccodes CLI cannot set a coefficient array, so each
fixture is a round-trip: we *encode* a chosen truncation geometry with an
arbitrary coefficient array using the eccodes **Python wheel**, then take the
value + metadata **oracle** from the pinned **CLI** eccodes 2.34.1.

Version split (recorded in tests/fixtures/NOTICE.md):
  * Encoder  — eccodes wheel (libeccodes 2.48.0); `python3` here must resolve to
    the interpreter that has `eccodes` + `numpy` installed (the repo's
    pre-commit venv does). The CLI has no way to set the `values` array.
  * Oracle   — CLI `grib_get_data` / `grib_dump` at eccodes 2.34.1 (the repo
    pin). Bi-Fourier's only known decode fix (ECC-1207) predates the pin, so
    2.34.1 is a valid value oracle. The CLI decodes the wheel-encoded bytes.

Regenerate:  python3 tools/build_grib2_bifourier_fixtures.py
Needs:       eccodes>=2 + numpy in this Python; grib_get_data on PATH (2.34.1).
"""

from __future__ import annotations

import math
import pathlib
import sys

import numpy as np

from eccodes_oracle import grib_get, grib_get_data_rows

try:
    import eccodes as ec
except ImportError:  # pragma: no cover - environment guard
    sys.exit(
        "eccodes Python module not found. Install the wheel into this "
        "interpreter (pip install eccodes numpy)."
    )

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)
SAMPLE = pathlib.Path.home() / (
    "Code/research/wx_sci/wxcode-rs/eccodes/samples/lambert_bf_grib2.tmpl"
)

RECTANGLE, ELLIPSE, DIAMOND = 77, 88, 99


def truncation(kind: int, ni: int, nj: int) -> tuple[list[int], list[int]]:
    """Port of eccodes' rectangle/ellipse/diamond truncation-limit tables."""
    it = [0] * (nj + 1)
    jt = [0] * (ni + 1)
    if kind == RECTANGLE:
        it = [ni] * (nj + 1)
        jt = [nj] * (ni + 1)
    elif kind == ELLIPSE:
        eps = 1e-10
        for j in range(1, nj):
            it[j] = int(ni / nj * math.sqrt(max(0.0, nj * nj - j * j)) + eps)
        if nj == 0:
            it[0] = ni
        else:
            it[0], it[nj] = ni, 0
        for i in range(1, ni):
            jt[i] = int(nj / ni * math.sqrt(max(0.0, ni * ni - i * i)) + eps)
        if ni == 0:
            jt[0] = nj
        else:
            jt[0], jt[ni] = nj, 0
    elif kind == DIAMOND:
        it = [-1] if nj == 0 else [ni - (j * ni) // nj for j in range(nj + 1)]
        jt = [-1] if ni == 0 else [nj - (i * nj) // ni for i in range(ni + 1)]
    else:
        raise ValueError(f"bad truncation type {kind}")
    return it, jt


def size_bif(kind: int, bi: int, bj: int) -> int:
    it, _ = truncation(kind, bi, bj)
    return sum(4 * (it[j] + 1) for j in range(bj + 1))


def build(name: str, *, bif_n, bif_m, bif_trunc, sub_n, sub_m, sub_trunc,
          keepaxes, bits, precision=2):
    """Encode one fixture and dump its pinned-CLI value oracle.

    `precision` is the unpacked-subset float width (1 = IEEE 32-bit, 2 = 64-bit;
    the `lambert_bf` sample defaults to 2, as ECMWF/ALADIN fields do).
    """
    nv = size_bif(bif_trunc, bif_n, bif_m)
    with open(SAMPLE, "rb") as f:
        h = ec.codes_grib_new_from_file(f)
    for key, val in {
        "biFourierResolutionParameterN": bif_n,
        "biFourierResolutionParameterM": bif_m,
        "biFourierTruncationType": bif_trunc,
        "biFourierResolutionSubSetParameterN": sub_n,
        "biFourierResolutionSubSetParameterM": sub_m,
        "biFourierSubTruncationType": sub_trunc,
        "biFourierPackingModeForAxes": 1 if keepaxes else 0,
        "unpackedSubsetPrecision": precision,
        "bitsPerValue": bits,
    }.items():
        ec.codes_set(h, key, val)
    # A varied coefficient field: decaying magnitude, alternating sign, so both
    # the exact IEEE subset and the quantised packed remainder are exercised.
    vals = np.array(
        [(120.0 / (1 + k)) * (1 if k % 3 else -1) for k in range(nv)],
        dtype=float,
    )
    ec.codes_set_values(h, vals)
    grib_path = FIXTURES / f"{name}.grib2"
    with open(grib_path, "wb") as f:
        ec.codes_write(h, f)
    ec.codes_release(h)

    # Oracle: pinned CLI value rows (drop the "Values" header line).
    ref = [line.strip() for line in grib_get_data_rows(grib_path)]
    (FIXTURES / f"{name}.eccodes.ref.txt").write_text("\n".join(ref) + "\n")

    ver = " ".join(
        grib_get(
            grib_path,
            ["packingType", "numberOfValues", "totalNumberOfValuesInUnpackedSubset"],
        )
    )
    print(f"  {name}: size_bif={nv}, oracle values={len(ref)}, [{ver}]")
    assert len(ref) == nv, f"{name}: oracle count {len(ref)} != size_bif {nv}"


def main() -> None:
    if not SAMPLE.exists():
        sys.exit(f"eccodes sample not found: {SAMPLE}")
    print(f"eccodes wheel {ec.codes_get_api_version()} (encoder); "
          f"oracle from CLI grib_get_data")
    build("bifourier_ellipse_keepaxes", bif_n=4, bif_m=4, bif_trunc=ELLIPSE,
          sub_n=2, sub_m=2, sub_trunc=RECTANGLE, keepaxes=True, bits=12)
    build("bifourier_diamond_no_axes", bif_n=5, bif_m=5, bif_trunc=DIAMOND,
          sub_n=2, sub_m=2, sub_trunc=RECTANGLE, keepaxes=False, bits=12)
    build("bifourier_rectangle_keepaxes", bif_n=3, bif_m=4, bif_trunc=RECTANGLE,
          sub_n=1, sub_m=1, sub_trunc=ELLIPSE, keepaxes=True, bits=10)
    # IEEE 32-bit unpacked subset (precision 1), to exercise the 4-byte path.
    build("bifourier_ellipse_ieee32", bif_n=4, bif_m=4, bif_trunc=ELLIPSE,
          sub_n=2, sub_m=2, sub_trunc=RECTANGLE, keepaxes=True, bits=12,
          precision=1)
    print("done")


if __name__ == "__main__":
    main()
