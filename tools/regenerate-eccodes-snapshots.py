#!/usr/bin/env python3
"""Regenerate eccodes reference snapshots for the GRIB2 test fixtures.

For each ``*.grib2`` file under ``crates/fieldglass-grib2/tests/fixtures/``,
this script invokes ``grib_dump -j`` (from the ``libeccodes-tools`` package)
and writes a curated subset of the result to
``{fixture}.eccodes.ref.json``. The Rust integration test
``crates/fieldglass-grib2/tests/eccodes_reference.rs`` reads each snapshot
and asserts that ``fieldglass-grib2`` produces the same values for the
curated keys.

The snapshot is intentionally a *subset* of what eccodes emits — only the
fields that our parser is expected to expose. Adding a field to
``CURATED_KEYS`` and re-running this script is how you grow coverage.

Run this only after upgrading eccodes or adding a new fixture; the
generated ``.eccodes.ref.json`` files are checked into git so the test
itself has zero external dependencies.

Usage:
    python3 tools/regenerate-eccodes-snapshots.py

Requires: ``grib_dump`` on PATH (Debian/Ubuntu: ``apt install
libeccodes-tools``).
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

# Fixtures eccodes cannot decode, so there is no snapshot to generate. These
# are deliberately exceed-eccodes cases (e.g. local template 5.40010, which
# eccodes has no definition for and errors on with "No final 7777"); their
# decode is cross-checked against a different oracle in the Rust tests instead.
ECCODES_UNDECODABLE: frozenset[str] = frozenset(
    {
        "png_local_40010.grib2",
    }
)

# Curated subset of eccodes keys. Keep ordered by section for human-readable
# diffs when the snapshot regenerates.
CURATED_KEYS: list[str] = [
    # §0 Indicator
    "discipline",
    "editionNumber",
    "totalLength",
    # §1 Identification
    "centre",
    "subCentre",
    "significanceOfReferenceTime",
    "dataDate",
    "dataTime",
    "productionStatusOfProcessedData",
    "typeOfProcessedData",
    # §3 Grid Definition
    "gridDefinitionTemplateNumber",
    "shapeOfTheEarth",
    "numberOfDataPoints",
    "Ni",
    "Nj",
    "latitudeOfFirstGridPointInDegrees",
    "longitudeOfFirstGridPointInDegrees",
    "latitudeOfLastGridPointInDegrees",
    "longitudeOfLastGridPointInDegrees",
    "iDirectionIncrementInDegrees",
    "jDirectionIncrementInDegrees",
    # §4 Product Definition
    "productDefinitionTemplateNumber",
    "parameterCategory",
    "parameterNumber",
    "typeOfGeneratingProcess",
    "indicatorOfUnitOfTimeRange",
    "forecastTime",
    "typeOfFirstFixedSurface",
    "scaleFactorOfFirstFixedSurface",
    "scaledValueOfFirstFixedSurface",
    # §5 Data Representation
    "dataRepresentationTemplateNumber",
    "referenceValue",
    "binaryScaleFactor",
    "decimalScaleFactor",
    "bitsPerValue",
    # §6 Bit-Map
    "bitMapIndicator",
]


def grib_dump_json(path: Path) -> dict:
    """Run ``grib_dump -j`` and return the parsed JSON."""
    result = subprocess.run(
        ["grib_dump", "-j", str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def curate(messages: list[list[dict]]) -> list[dict]:
    """Flatten each message's ``[{key,value}, ...]`` into a dict, then keep
    only the curated keys (in declaration order)."""
    out: list[dict] = []
    for kv_list in messages:
        kv = {pair["key"]: pair["value"] for pair in kv_list}
        out.append({k: kv[k] for k in CURATED_KEYS if k in kv})
    return out


def main() -> int:
    if shutil.which("grib_dump") is None:
        print(
            "grib_dump not on PATH. Install eccodes "
            "(Debian/Ubuntu: `apt install libeccodes-tools`) and re-run.",
            file=sys.stderr,
        )
        return 1

    repo_root = Path(__file__).resolve().parent.parent
    fixtures = repo_root / "crates" / "fieldglass-grib2" / "tests" / "fixtures"
    grib_files = sorted(fixtures.glob("*.grib2"))
    if not grib_files:
        print(f"No .grib2 fixtures found in {fixtures}", file=sys.stderr)
        return 1

    for grib in grib_files:
        if grib.name in ECCODES_UNDECODABLE:
            print(f"skipping {grib.name} (eccodes cannot decode it)")
            continue
        dump = grib_dump_json(grib)
        curated = curate(dump["messages"])
        ref_path = grib.with_suffix(".grib2.eccodes.ref.json")
        ref_path.write_text(json.dumps({"messages": curated}, indent=2) + "\n")
        print(f"wrote {ref_path.relative_to(repo_root)} ({len(curated)} msg)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
