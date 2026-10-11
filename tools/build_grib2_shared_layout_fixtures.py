#!/usr/bin/env python3
"""Build GRIB2 fixtures in two product templates laid out as 4.0 (#959).

`fieldglass-grib2` reads 4.0, 4.8 and 4.11 into types of their own, and since
#959 reads every template that shares 4.0's octets 10-34 (4.1, 4.2, 4.5, 4.6,
4.7, 4.9, 4.10, 4.12, 4.15) through that common block, keeping the rest as
written. No fixture used any of them, so the octets those templates are read
from were checked only against hand-built bytes. These are real messages, one
of each kind:

- `ensemble_member_4_1.grib2`: template 4.1, an ensemble member at a point in
  time. Octets 35-37 are the ensemble block.
- `derived_max_4_12.grib2`: template 4.12, a derived forecast (the unweighted
  mean of 20 members) of a 0-6 h maximum. Octets 35-36 are the derived block,
  and 4.8's statistical block follows, so it is the one that shows the reader
  finds that block after the template's own octets.

Both start from the committed `regular_latlon_surface.grib2` (16 x 31, 2 m
temperature) with only §4 changed, so each stays small and the decode path is
the one that fixture already pins.

**Encode with the wheel, verify with the pin.** The eccodes PyPI wheel writes
the templates; the pinned eccodes 2.34.1 CLI on PATH reads them back here, and
`tools/regenerate-eccodes-snapshots.py` records its answers as the oracle.

Usage:  python3 tools/build_grib2_shared_layout_fixtures.py
Needs:  the `eccodes` PyPI wheel (encoding) and eccodes 2.34.1 on PATH (oracle).
Then:   python3 tools/regenerate-eccodes-snapshots.py --edition 2
"""

from __future__ import annotations

import pathlib
import subprocess
import sys

import eccodes as ec

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)
SOURCE = FIXTURES / "regular_latlon_surface.grib2"

# (output, template, keys set after the template number, keys the pin must
# read back). The read-back is asserted rather than printed: a builder whose
# key quietly failed to take would leave the layout untested while looking
# like it covered it.
BUILDS = [
    (
        "ensemble_member_4_1.grib2",
        1,
        {"typeOfEnsembleForecast": 3, "perturbationNumber": 7, "numberOfForecastsInEnsemble": 20},
        {
            "productDefinitionTemplateNumber": "1",
            "parameterCategory": "0",
            "parameterNumber": "0",
            "perturbationNumber": "7",
            "numberOfForecastsInEnsemble": "20",
        },
    ),
    (
        "derived_max_4_12.grib2",
        12,
        {
            "derivedForecast": 0,
            "numberOfForecastsInEnsemble": 20,
            "typeOfStatisticalProcessing": 2,
            "stepRange": "0-6",
        },
        {
            "productDefinitionTemplateNumber": "12",
            "derivedForecast": "0",
            "typeOfStatisticalProcessing": "2",
            "lengthOfTimeRange": "6",
            "stepRange": "0-6",
        },
    ),
]


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
    with SOURCE.open("rb") as f:
        source = ec.codes_grib_new_from_file(f)
    try:
        for name, template, keys, expected in BUILDS:
            h = ec.codes_clone(source)
            try:
                ec.codes_set(h, "productDefinitionTemplateNumber", template)
                for key, value in keys.items():
                    ec.codes_set(h, key, value)
                out = FIXTURES / name
                with out.open("wb") as f:
                    ec.codes_write(h, f)
            finally:
                ec.codes_release(h)
            for key, want in expected.items():
                got = oracle(out, key)
                if got != want:
                    print(f"{name}: {key} reads back {got!r}, expected {want!r}", file=sys.stderr)
                    return 1
            print(f"wrote {out.relative_to(FIXTURES.parents[3])} ({out.stat().st_size} bytes)")
    finally:
        ec.codes_release(source)
    return 0


if __name__ == "__main__":
    sys.exit(main())
