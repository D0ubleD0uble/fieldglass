#!/usr/bin/env python3
"""Rebuild the GRIB2 5.42 CCSDS flag fixtures (#756).

eccodes hands libaec the unsigned reference-subtracted value as an n-bit
pattern, and libaec's default build never writes RSI padding, so a 5.42 message
whose ``ccsdsFlags`` carries SIGNED or PAD_RSI still holds an unsigned,
unpadded stream. fieldglass decodes those to the source field; eccodes 2.34.1
does not. These fixtures pin that behaviour.

Each output is the source fixture re-flagged with the pinned eccodes CLI
(``grib_set -r -s ccsdsFlags=N``), under
``crates/fieldglass-grib2/tests/fixtures``:

- ``ecmwf_ccsds_latlon.grib2`` (12-bit)          -> ``ccsds_flags13_12bit.grib2``
- ``ccsds_regular_latlon_24bit.grib2`` (24-bit)  -> ``ccsds_flags13_24bit.grib2``
- ``ecmwf_ccsds_latlon.grib2``                   -> ``ccsds_flags36_12bit.grib2``
- ``ecmwf_ccsds_latlon.grib2``                   -> ``ccsds_flags46_12bit.grib2``

Flag values: SIGNED = 1, 3BYTE = 2, MSB = 4, PREPROCESS = 8, PAD_RSI = 32.

The value oracle is the SOURCE fixture's ``*_expected.json``, not eccodes'
decode of the new file: for the flag-13 and flag-36 outputs this script copies
the source oracle to ``<output>_expected.json`` with only ``ccsdsFlags`` and
the ``source`` note changed. (flag 46 has no oracle: it is expected to fail.)
The ``.eccodes.ref.json`` metadata snapshots are written by
``tools/regenerate-eccodes-snapshots.py``. See
``crates/fieldglass-grib2/tests/fixtures/NOTICE.md``.

Usage:
    python3 tools/build_grib2_ccsds_flag_fixtures.py

Requires ``grib_set`` from eccodes 2.34.1 on PATH.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

PINNED_ECCODES = "2.34.1"

FIXTURES = (
    Path(__file__).resolve().parent.parent
    / "crates"
    / "fieldglass-grib2"
    / "tests"
    / "fixtures"
)

# (source, output, ccsdsFlags)
BUILDS = (
    ("ecmwf_ccsds_latlon.grib2", "ccsds_flags13_12bit.grib2", 13),
    ("ccsds_regular_latlon_24bit.grib2", "ccsds_flags13_24bit.grib2", 13),
    ("ecmwf_ccsds_latlon.grib2", "ccsds_flags36_12bit.grib2", 36),
    ("ecmwf_ccsds_latlon.grib2", "ccsds_flags46_12bit.grib2", 46),
)


def eccodes_version() -> str:
    out = subprocess.run(
        ["grib_set", "-V"],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return (out.stdout + out.stderr).split()[-1]


def write_oracle(src: str, dst: str, flags: int) -> None:
    """Copy the source fixture's value oracle for a re-flagged output."""
    oracle = json.loads(
        (FIXTURES / src.replace(".grib2", "_expected.json")).read_text(encoding="utf-8")
    )
    oracle["section5"]["ccsdsFlags"] = flags
    oracle["source"] = (
        f"Value oracle copied from {src}'s eccodes 2.34.1 oracle, NOT from "
        f"eccodes' decode of {dst} (eccodes 2.34.1 decodes ccsdsFlags={flags} "
        "wrongly). Provenance in NOTICE.md."
    )
    out = FIXTURES / dst.replace(".grib2", "_expected.json")
    out.write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {out.name}")


def main() -> int:
    if shutil.which("grib_set") is None:
        print("grib_set not on PATH (apt install libeccodes-tools)", file=sys.stderr)
        return 1
    version = eccodes_version()
    if version != PINNED_ECCODES:
        print(f"need eccodes {PINNED_ECCODES}, found {version}", file=sys.stderr)
        return 1
    for src, dst, flags in BUILDS:
        subprocess.run(
            [
                "grib_set",
                "-r",
                "-s",
                f"ccsdsFlags={flags}",
                str(FIXTURES / src),
                str(FIXTURES / dst),
            ],
            check=True,
        )
        print(f"wrote {dst} (ccsdsFlags={flags}) from {src}")
        if flags != 46:
            write_oracle(src, dst, flags)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
