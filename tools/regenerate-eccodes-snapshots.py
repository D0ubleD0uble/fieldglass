#!/usr/bin/env python3
"""Regenerate eccodes reference snapshots for the GRIB test fixtures.

For each fixture under ``crates/fieldglass-grib{1,2}/tests/fixtures/``, this
script invokes ``grib_dump -j`` (from the ``libeccodes-tools`` package) and
writes a curated subset of the result to ``{fixture}.eccodes.ref.json``. The
Rust integration tests ``crates/fieldglass-grib{1,2}/tests/eccodes_reference.rs``
read each snapshot and assert that our parser produces the same values for the
curated keys.

The snapshot is intentionally a *subset* of what eccodes emits — only the
fields our parser is expected to expose. Adding a field to an edition's
``keys`` and re-running this script is how you grow coverage; the Rust side
needs the matching arm in its ``assert_message_matches`` dispatch, or it fails
loudly on the unmapped key.

The two editions keep separate key lists because GRIB1 and GRIB2 are different
formats, not two spellings of one: GRIB1's identity is
``table2Version`` + ``indicatorOfParameter`` + centre where GRIB2 has
discipline/category/number, its grid is a data-representation type rather than
a template number, and its packing variants live in BDS flag bits that GRIB2
spends a whole section on.

Run this only after upgrading eccodes or adding a new fixture; the generated
``.eccodes.ref.json`` files are checked into git so the tests themselves have
zero external dependencies.

Usage:
    python3 tools/regenerate-eccodes-snapshots.py            # both editions
    python3 tools/regenerate-eccodes-snapshots.py --edition 1

Requires: ``grib_dump`` on PATH (Debian/Ubuntu: ``apt install
libeccodes-tools``).
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

# Curated subset of eccodes keys for GRIB2. Keep ordered by section for
# human-readable diffs when the snapshot regenerates.
GRIB2_KEYS: list[str] = [
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

# Curated subset for GRIB1, ordered by section: PDS, GDS, BDS. Every key here
# has a field in `fieldglass-grib1` behind it — the point is to compare two
# implementations of the same octets, so a key eccodes *derives* (`stepRange`,
# `isConstant`, the statistics) is deliberately absent.
GRIB1_KEYS: list[str] = [
    # Product Definition Section (§1)
    "editionNumber",
    "table2Version",
    "centre",
    "subCentre",
    "generatingProcessIdentifier",
    "indicatorOfParameter",
    "indicatorOfTypeOfLevel",
    "level",
    "timeRangeIndicator",
    "dataDate",
    "dataTime",
    "decimalScaleFactor",
    "GDSPresent",
    "bitmapPresent",
    # Grid Description Section (§2)
    "gridType",
    "Ni",
    "Nj",
    "Nx",
    "Ny",
    "numberOfDataPoints",
    "latitudeOfFirstGridPointInDegrees",
    "longitudeOfFirstGridPointInDegrees",
    "latitudeOfLastGridPointInDegrees",
    "longitudeOfLastGridPointInDegrees",
    "iDirectionIncrementInDegrees",
    "jDirectionIncrementInDegrees",
    "DxInMetres",
    "DyInMetres",
    "orientationOfTheGridInDegrees",
    "southPoleOnProjectionPlane",
    "earthIsOblate",
    "uvRelativeToGrid",
    "iScansNegatively",
    "jScansPositively",
    "jPointsAreConsecutive",
    "N",
    # The reduced grids' row-length list. It is the geometry every reduced
    # decode walks, and it is long — which is the point: an off-by-one in the
    # PL block shows up nowhere else in the metadata.
    "pl",
    "J",
    "K",
    "M",
    # Binary Data Section (§4)
    "packingType",
    "sphericalHarmonics",
    "complexPacking",
    "integerPointValues",
    "additionalFlagPresent",
    "bitsPerValue",
    # Octet 11 under its complex-packing name: the same octet that carries the
    # per-point width for simple packing is the first-order width for
    # second-order, and eccodes renames it accordingly.
    "widthOfFirstOrderValues",
    "binaryScaleFactor",
    "referenceValue",
    "matrixOfValues",
    "secondaryBitmapPresent",
    "secondOrderOfDifferentWidth",
    "generalExtended2ordr",
    "boustrophedonicOrdering",
    "twoOrdersOfSPD",
    "plusOneinOrdersOfSPD",
]


@dataclass(frozen=True)
class Edition:
    """One GRIB edition's fixture directory, key list and exemptions."""

    number: int
    crate: str
    patterns: tuple[str, ...]
    keys: list[str]
    # Fixtures eccodes cannot decode, so there is no snapshot to generate.
    # These are deliberately exceed-eccodes cases; their decode is
    # cross-checked against a different oracle in the Rust tests instead. Keep
    # in step with `NO_ECCODES_SNAPSHOT` in the matching
    # `tests/eccodes_reference.rs`, which the Rust side checks for staleness.
    undecodable: frozenset[str]


EDITIONS: tuple[Edition, ...] = (
    Edition(
        number=1,
        crate="fieldglass-grib1",
        patterns=("*.grib1", "*.grib"),
        keys=GRIB1_KEYS,
        undecodable=frozenset(
            {
                # True `matrixOfValues`: eccodes 2.34.1 aborts inside its own
                # secondary-bitmap accessor ("assertion failed: `m <=
                # secondary_len'").
                "hand_matrix_of_values.grib1",
            }
        ),
    ),
    Edition(
        number=2,
        crate="fieldglass-grib2",
        patterns=("*.grib2",),
        keys=GRIB2_KEYS,
        undecodable=frozenset(
            {
                # Local template 5.40010, which eccodes has no definition for
                # and errors on with "No final 7777".
                "png_local_40010.grib2",
            }
        ),
    ),
)


def grib_dump_json(path: Path) -> dict:
    """Run ``grib_dump -j`` and return the parsed JSON."""
    result = subprocess.run(
        ["grib_dump", "-j", str(path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    )
    return json.loads(result.stdout)


def grib_get_values(path: Path, key: str) -> list[str] | None:
    """One value of ``key`` per message via ``grib_get -p``, or ``None`` if
    eccodes cannot supply it for this file.

    ``grib_dump -j`` prints only what is in eccodes' *dump* namespace, and for
    GRIB2 that leaves out the whole §5 Data Representation group —
    ``dataRepresentationTemplateNumber``, ``referenceValue``,
    ``binaryScaleFactor``, ``decimalScaleFactor``, ``bitsPerValue``. They sat in
    the curated list producing nothing, so the section that decides how every
    value is packed had no cross-check at all (found while building the GRIB1
    counterpart, #475). ``grib_get`` reads the same keys through the accessor
    layer and does return them.
    """
    result = subprocess.run(
        ["grib_get", "-p", key, str(path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if result.returncode != 0:
        return None
    lines = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    # eccodes prints this in place of a value the message does not define.
    if not lines or any(line == "not_found" for line in lines):
        return None
    return lines


# What eccodes prints for a key whose octets hold the all-ones missing
# sentinel (`Nx` on a reduced grid, say). It is not a value, so the snapshot
# records `null` — the same thing the dump does when it omits a key, and the
# same thing the Rust side skips.
MISSING = "MISSING"


def typed(value: str) -> int | float | str:
    """``grib_get`` prints everything as text; the snapshot needs the JSON type
    the dump would have produced, so the Rust side's integer / float / string
    check picks the right comparison."""
    try:
        return int(value)
    except ValueError:
        pass
    try:
        return float(value)
    except ValueError:
        return value


def curate(path: Path, messages: list[list[dict]], keys: list[str]) -> list[dict]:
    """Flatten each message's ``[{key,value}, ...]`` into a dict, then keep
    only the curated keys (in declaration order), filling in any the dump
    omitted from ``grib_get``."""
    dumped = [{pair["key"]: pair["value"] for pair in kv_list} for kv_list in messages]
    missing = [k for k in keys if not any(k in kv for kv in dumped)]
    for key in missing:
        values = grib_get_values(path, key)
        if values is None or len(values) != len(dumped):
            continue
        for kv, value in zip(dumped, values):
            kv[key] = None if value == MISSING else typed(value)
    return [{k: kv[k] for k in keys if k in kv} for kv in dumped]


# How many points to record per message. Statistics alone cannot see a
# permutation — a scan-order bug leaves the min, max and mean untouched (it is
# how #285 survived a range oracle) — so the snapshot also pins a spread of
# individual points, which any reordering moves.
VALUE_SAMPLE_POINTS = 16


def is_missing(value) -> bool:
    """Whether eccodes emitted this array entry as a masked point.

    `grib_dump -j` writes the missing sentinel two ways in the same array —
    `null` for most entries and the raw `-1e+100` for others — so both are
    folded to one representation here, and the Rust side gets a single rule:
    `null` means the bitmap masks this point.
    """
    return value is None or (isinstance(value, (int, float)) and value <= -1e99)


def value_block(kv: dict) -> dict | None:
    """The value-level oracle for one message: how many points eccodes decoded,
    how many the bitmap masks, the statistics over the rest, and a sample of
    individual points.

    Returns ``None`` when eccodes decoded nothing to compare against — a
    spherical-harmonic GRIB2 message, say, whose values key it does not fill.
    """
    count = kv.get("numberOfDataPoints")
    values = kv.get("values")
    if not isinstance(values, list):
        # No array: keep the statistics if eccodes computed them, since they
        # still pin the magnitudes. `numberOfValues` is the coded count, which
        # for a bitmapped field is smaller than the field.
        if "average" not in kv:
            return None
        return {
            "count": count if isinstance(count, int) else kv.get("numberOfValues"),
            "numberOfMissing": kv.get("numberOfMissing"),
            "minimum": kv.get("minimum"),
            "maximum": kv.get("maximum"),
            "average": kv.get("average"),
        }

    n = len(values)
    step = max(1, n // VALUE_SAMPLE_POINTS)
    indices = sorted({0, n - 1, *range(0, n, step)})[:VALUE_SAMPLE_POINTS + 2]
    return {
        "count": n,
        "numberOfMissing": kv.get("numberOfMissing"),
        "minimum": kv.get("minimum"),
        "maximum": kv.get("maximum"),
        "average": kv.get("average"),
        "sample": [[i, None if is_missing(values[i]) else values[i]] for i in indices],
    }


def regenerate(edition: Edition, repo_root: Path) -> int:
    """Write every snapshot for one edition. Returns a process exit code."""
    fixtures = repo_root / "crates" / edition.crate / "tests" / "fixtures"
    grib_files = sorted(
        {path for pattern in edition.patterns for path in fixtures.glob(pattern)}
    )
    if not grib_files:
        print(f"No GRIB{edition.number} fixtures found in {fixtures}", file=sys.stderr)
        return 1

    shipped: set[str] = set()
    for grib in grib_files:
        if grib.name in edition.undecodable:
            print(f"skipping {grib.name} (eccodes cannot decode it)")
            continue
        dump = grib_dump_json(grib)
        curated = curate(grib, dump["messages"], edition.keys)
        values = [
            value_block({pair["key"]: pair["value"] for pair in kv_list})
            for kv_list in dump["messages"]
        ]
        # Suffix the whole filename rather than replacing an extension: GRIB1
        # fixtures carry both `.grib1` and `.grib`, and this keeps every
        # snapshot named `<fixture>.eccodes.ref.json` either way.
        ref_path = grib.with_name(grib.name + ".eccodes.ref.json")
        snapshot = {"messages": curated, "values": values}
        ref_path.write_text(json.dumps(snapshot, indent=2) + "\n", encoding="utf-8")
        print(f"wrote {ref_path.relative_to(repo_root)} ({len(curated)} msg)")
        for message in curated:
            shipped.update(k for k, v in message.items() if v is not None)

    # The failure that hid the missing §5 coverage: a curated key eccodes never
    # emits is dropped silently by `curate`, leaving the Rust dispatch arm dead
    # and the section unchecked. Say so here, where the curated list lives.
    never = [k for k in edition.keys if k not in shipped]
    if never:
        print(
            f"GRIB{edition.number}: {len(never)} curated key(s) reached no snapshot "
            f"and are checking nothing: {never}",
            file=sys.stderr,
        )
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--edition",
        type=int,
        choices=[e.number for e in EDITIONS],
        help="regenerate only this GRIB edition (default: both)",
    )
    args = parser.parse_args()

    # Both tools ship in the same package, and the fallback is not optional:
    # without `grib_get` the §5 keys silently vanish from the snapshots, which
    # is the exact failure this script now checks for at the end.
    missing_tools = [t for t in ("grib_dump", "grib_get") if shutil.which(t) is None]
    if missing_tools:
        print(
            f"{', '.join(missing_tools)} not on PATH. Install eccodes "
            "(Debian/Ubuntu: `apt install libeccodes-tools`) and re-run.",
            file=sys.stderr,
        )
        return 1

    repo_root = Path(__file__).resolve().parent.parent
    for edition in EDITIONS:
        if args.edition is not None and edition.number != args.edition:
            continue
        code = regenerate(edition, repo_root)
        if code != 0:
            return code
    return 0


if __name__ == "__main__":
    sys.exit(main())
