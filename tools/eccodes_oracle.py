"""Shared eccodes CLI-oracle helpers for the GRIB2 §5 fixture builders.

The pinned decode oracle is eccodes 2.34.1 (see ``tests/fixtures/NOTICE.md``).
Every §5 fixture builder shells out to the same ``grib_get`` / ``grib_get_data``
/ ``grib_set`` commands to snapshot eccodes' decode as the value/metadata
oracle; those wrappers — and the oracle policy they encode (the pinned version,
the missing-value sentinel) — live here instead of being copy-pasted into each
builder, so a policy change is a one-line edit rather than a six-file sweep.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

#: The pinned eccodes CLI version these oracles assume. ``NOTICE.md`` records
#: why (and the cases where a newer wheel is used for value generation instead).
PINNED_VERSION = "2.34.1"

#: The token ``grib_get_data -m <sentinel>`` prints for a masked/missing point.
MISSING_SENTINEL = "9999"


def grib_get(path: Path, keys: list[str]) -> list[str]:
    """Whitespace-split output of ``grib_get -p <keys>`` for ``path``."""
    out = subprocess.run(
        ["grib_get", "-p", ",".join(keys), str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    return out.stdout.split()


def decoded_values(path: Path) -> list[float | None]:
    """Decoded grid values via ``grib_get_data -m <sentinel>``.

    Float-with-sentinel mode: each ``lat lon value`` row's value column is
    parsed to ``float``, and a masked point (the sentinel) becomes ``None``.
    """
    out = subprocess.run(
        ["grib_get_data", "-m", MISSING_SENTINEL, str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    vals: list[float | None] = []
    for line in out.stdout.strip().splitlines()[1:]:
        v = line.split()[2]
        vals.append(None if v == MISSING_SENTINEL else float(v))
    return vals


def grib_get_data_rows(path: Path) -> list[str]:
    """Raw ``grib_get_data`` rows for ``path`` — the header line and any blank
    lines dropped, no missing-value substitution.

    Raw-string mode: for callers that keep the value as a string or parse the
    ``lat lon value`` row themselves (the bi-Fourier and matrix builders).
    """
    out = subprocess.run(
        ["grib_get_data", str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    return [line for line in out.stdout.splitlines()[1:] if line.strip()]


def grib_set(path_in: Path, path_out: Path, sets: list[str], *, repack: bool = True) -> None:
    """Apply each ``key=value`` in ``sets`` from ``path_in`` to ``path_out``.

    ``repack=True`` passes ``-r`` (re-pack the data section). Pass
    ``repack=False`` for a pure metadata flip that must *not* re-pack — e.g.
    setting the boustrophedonic flag on an already-second-order message, where
    eccodes reorders the stored data on decode instead.
    """
    args = ["grib_set"]
    if repack:
        args.append("-r")
    args += sum((["-s", s] for s in sets), [])
    args += [str(path_in), str(path_out)]
    subprocess.run(args, capture_output=True, text=True, check=True)
