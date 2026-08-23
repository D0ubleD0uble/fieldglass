#!/usr/bin/env python3
"""Snapshot the WMO GRIB2 *code* tables as a JSON oracle for the table tests.

`tables_wmo.rs` is generated from WMO Code Tables 4.2, 4.4 and 4.5. The rest of
the GRIB2 code tables — discipline, production status, grid template, earth
shape, statistical process and friends — are hand-written in `tables.rs`,
because they are short and carry deliberately abbreviated labels. Hand-written
means unverified, and the parameter cross-check in #415 showed what unverified
costs: three entries naming the wrong quantity. Sweeping the code tables the
same way found six more, in Tables 1.3 and 4.10.

This snapshots those tables from the same pinned release the generator uses, so
`tests/wmo_code_tables.rs` can check every curated arm against them. Committed,
so the test suite needs no network.

Regenerate:

    python3 tools/gen_wmo_code_table_snapshot.py
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

# Run from the repo root like the other generators, so the output path resolves;
# that puts this file's own directory outside the import path.
sys.path.insert(0, str(Path(__file__).resolve().parent))

# Reuse the generator's pinned tag and download so the two cannot drift apart.
from gen_wmo_grib2_tables import WMO_TAG, clean, fetch_csvs, rows  # noqa: E402

OUT = Path("crates/fieldglass-grib2/tests/fixtures/wmo_code_tables.ref.json")

# WMO table number -> the `tables.rs` function that carries it. The comment is
# what the test reports, so keep it the human name of the table.
TABLES = {
    "0.0": "lookup_discipline",
    "1.2": "lookup_reference_time_significance",
    "1.3": "lookup_production_status",
    "1.4": "lookup_data_type",
    "3.1": "lookup_grid_template",
    "3.2": "lookup_earth_shape",
    "4.3": "lookup_generating_process_type",
    "4.4": "lookup_time_range_unit",
    "4.5": "lookup_fixed_surface",
    "4.6": "lookup_ensemble_type",
    "4.10": "lookup_statistical_process",
}


def main() -> int:
    csvs = fetch_csvs()
    out: dict[str, dict[str, str]] = {}
    for table, _fn in TABLES.items():
        major, minor = table.split(".")
        filename = f"GRIB2_CodeFlag_{major}_{minor}_CodeTable_en.csv"
        if filename not in csvs:
            raise SystemExit(f"{filename} not in the {WMO_TAG} release")
        entries: dict[str, str] = {}
        for row in rows(csvs[filename]):
            code = clean(row.get("CodeFlag", ""))
            if not code.isdigit():
                continue
            name = clean(row.get("MeaningParameterDescription_en", ""))
            if not name:
                continue
            entries[code] = name
        out[table] = dict(sorted(entries.items(), key=lambda kv: int(kv[0])))

    snapshot = {
        "source": f"wmo-im/GRIB2 release {WMO_TAG}",
        "note": "WMO GRIB2 code tables, the oracle for the hand-written lookups "
        "in tables.rs (#415). Regenerate with tools/gen_wmo_code_table_snapshot.py.",
        "tables": out,
    }
    OUT.write_text(json.dumps(snapshot, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    total = sum(len(v) for v in out.values())
    print(f"wrote {OUT} — {total} entries across {len(out)} code tables from {WMO_TAG}",
          file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
