#!/usr/bin/env python3
"""Snapshot eccodes' GRIB2 Table 4.2 into a JSON oracle for the table tests.

`crates/fieldglass-grib2/src/tables_wmo.rs` is generated from the WMO CSVs;
this snapshots the *same* table as eccodes ships it, so the two can be
compared. Two independent transcriptions of one standard agreeing is worth a
great deal more than either one alone — it is what caught three curated
entries whose triples were GRIB1 ON388 codes copied onto GRIB2
discipline/category/number (#415).

Following the same rule as the `.eccodes.ref.json` message snapshots: the
output is committed, so the test suite needs no eccodes at runtime. eccodes is
needed only to regenerate.

Line format in `definitions/grib2/tables/<version>/4.2.<disc>.<cat>.table` is
`<code> <shortName> <name> (<units>)`. Units are the last *balanced*
parenthesis group, which matters — several space-weather fluxes carry nested
parentheses (`Proton flux (differential) ((m2 s sr eV)-1)`), and a
non-balanced match silently folds half the unit into the name.

Usage (ECCODES_DEFINITIONS may point at any eccodes checkout or install):

    ECCODES_DEFINITIONS=~/path/to/eccodes/definitions \\
        python3 tools/gen_eccodes_parameter_snapshot.py
"""
from __future__ import annotations

import json
import os
import re
import sys
from pathlib import Path

# eccodes ships one directory per WMO table version; take the highest, which is
# the closest thing it has to the WMO tag we generate from.
DEFAULT_DEFINITIONS = "/usr/share/eccodes/definitions"
OUT = Path("crates/fieldglass-grib2/tests/fixtures/eccodes_parameters.ref.json")


def split_units(rest: str) -> tuple[str, str]:
    """Split `name (units)` on the last balanced parenthesis group."""
    rest = rest.strip()
    if not rest.endswith(")"):
        return rest, ""
    depth = 0
    for i in range(len(rest) - 1, -1, -1):
        if rest[i] == ")":
            depth += 1
        elif rest[i] == "(":
            depth -= 1
            if depth == 0:
                return rest[:i].strip(), rest[i + 1 : -1].strip()
    return rest, ""


def main() -> int:
    definitions = Path(os.environ.get("ECCODES_DEFINITIONS", DEFAULT_DEFINITIONS))
    tables = definitions / "grib2" / "tables"
    if not tables.is_dir():
        raise SystemExit(f"no eccodes tables at {tables}; set ECCODES_DEFINITIONS")

    versions = sorted(
        (int(p.name) for p in tables.iterdir() if p.is_dir() and p.name.isdigit()),
    )
    if not versions:
        raise SystemExit(f"no numbered table versions under {tables}")
    version = versions[-1]

    entries: dict[str, dict[str, str]] = {}
    pattern = re.compile(r"^4\.2\.(\d+)\.(\d+)\.table$")
    for path in sorted((tables / str(version)).iterdir()):
        match = pattern.match(path.name)
        if not match:
            continue
        discipline, category = int(match.group(1)), int(match.group(2))
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = line.split(None, 2)
            if len(parts) < 3 or not parts[0].isdigit():
                continue
            name, units = split_units(parts[2])
            entries[f"{discipline}/{category}/{parts[0]}"] = {"name": name, "units": units}

    snapshot = {
        "source": f"eccodes definitions/grib2/tables/{version}/4.2.*.table",
        "table_version": version,
        "note": "WMO Code Table 4.2 as eccodes transcribes it — the independent "
        "oracle for the WMO-CSV-generated tables_wmo.rs (#415).",
        "parameters": dict(sorted(entries.items(), key=lambda kv: [int(x) for x in kv[0].split("/")])),
    }
    OUT.write_text(json.dumps(snapshot, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} — {len(entries)} parameters from eccodes table version {version}",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
