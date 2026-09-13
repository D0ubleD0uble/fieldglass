#!/usr/bin/env python3
"""Generate the GRIB1 centre-local parameter tables, and their eccodes oracle.

Writes two files from the pinned eccodes (2.34.1, the CLI on PATH and its
definitions under /usr/share/eccodes):

  * `crates/fieldglass-grib1/src/tables_local.rs` — every ECMWF local Table 2
    (`definitions/grib1/2.98.<version>.table`), one function per version and a
    `lookup(centre, table_version, id)` over them.
  * `crates/fieldglass-grib1/tests/fixtures/local_tables.eccodes.ref.json` —
    what eccodes *decodes* for every id of every one of those tables, read back
    from real GRIB1 messages rather than from the text files the Rust is
    generated from, so the test that compares the two is not checking this
    script against itself.

Regenerate:

    python3 tools/gen_grib1_local_tables.py && cargo fmt

**Why ECMWF only (#601).** eccodes ships local tables for centres 34, 46, 82,
233 and 253 as well, but in layouts its own codetable reader does not parse:
`1 1 PRES Pressure [hPa]` (the abbreviation comes out as `1`, the units as
`unknown`), and `1 pres PRES Pressure Pa` with the units bare at the end of the
name (so `Sea surface temperature (LAKE) K` reports units `LAKE`). Generating
those would put eccodes' misreadings into the message table as if they were
names, where `Parameter 82/128/1` is at least honest. The ECMWF tables are all
`<code> <abbrev> <name> (<units>)`, which eccodes and this script read alike.

**Which table eccodes selects.** `grib1/section.1.def`: at a `table2Version` of
128 or more, the table is the originating centre's — except that a message from
another centre whose sub-centre is 98 reads ECMWF's. Below 128 it is the WMO
table for every centre. `lookup_parameter` in `tables.rs` implements the same
rule, and the oracle includes the sub-centre case.

**Two deliberate differences from eccodes' reading**, both covered by the test:

  * `~` (eccodes' "unset") becomes an empty string.
  * Units that themselves contain parentheses, `Cloud fraction ((0 - 1))`, are
    split at the last *balanced* group, giving `Cloud fraction` and `(0 - 1)`.
    eccodes splits at the last `(`, giving `Cloud fraction (` and `0 - 1)`.

The file is written directly rather than piped from stdout, and every read and
write names its encoding (#451).
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

DEFINITIONS = Path("/usr/share/eccodes/definitions/grib1")
SAMPLE = Path("/usr/share/eccodes/samples/GRIB1.tmpl")
PINNED_VERSION = "2.34.1"
CENTRE_ECMWF = 98
ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "crates" / "fieldglass-grib1" / "src" / "tables_local.rs"
ORACLE = ROOT / "crates" / "fieldglass-grib1" / "tests" / "fixtures" / "local_tables.eccodes.ref.json"

#: Non-ECMWF originators whose messages eccodes reads against ECMWF's tables
#: because their sub-centre is 98, and centres that must *not* reach ECMWF's
#: tables. `(centre, sub_centre, table_version)`.
SELECTION_CASES = [
    (80, 98, 128),  # Rome, produced for it by ECMWF: ECMWF's 128.
    (80, 0, 128),  # Rome on its own: no table.
    (7, 98, 128),  # Any other centre with sub-centre 98 likewise.
    (98, 98, 128),  # ECMWF naming itself as sub-centre changes nothing.
]


def split_units(rest: str) -> tuple[str, str]:
    """Return (name, units): units are the final balanced (...) group."""
    rest = rest.strip()
    if not rest.endswith(")"):
        return rest, ""
    depth = 0
    for i in range(len(rest) - 1, -1, -1):
        c = rest[i]
        if c == ")":
            depth += 1
        elif c == "(":
            depth -= 1
            if depth == 0:
                return rest[:i].strip(), rest[i + 1 : -1].strip()
    return rest, ""


def versions() -> list[int]:
    found = []
    for path in DEFINITIONS.glob(f"2.{CENTRE_ECMWF}.*.table"):
        version = int(path.name.split(".")[2])
        if version >= 128:
            found.append(version)
    return sorted(found)


def parse_table(version: int) -> list[tuple[int, str, str, str]]:
    entries = []
    text = (DEFINITIONS / f"2.{CENTRE_ECMWF}.{version}.table").read_text(encoding="utf-8")
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        parts = line.split(None, 2)
        if len(parts) < 2:
            continue
        try:
            code = int(parts[0])
        except ValueError:
            continue
        abbrev = "" if parts[1] == "~" else parts[1]
        name, units = split_units(parts[2] if len(parts) == 3 else "")
        entries.append((code, name, abbrev, "" if units == "~" else units))
    return entries


def rs_escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def emit_fn(version: int, entries) -> str:
    lines = [
        f"/// ECMWF local parameter table {version} (centre 98), from eccodes'",
        f"/// `definitions/grib1/2.98.{version}.table`.",
        f"fn ecmwf_{version}(id: u8) -> Option<ParameterEntry> {{",
        "    let (name, abbreviation, units) = match id {",
    ]
    for code, name, abbrev, units in entries:
        lines.append(f'        {code} => ("{rs_escape(name)}", "{rs_escape(abbrev)}", "{rs_escape(units)}"),')
    lines += [
        "        _ => return None,",
        "    };",
        "    Some(ParameterEntry {",
        "        name,",
        "        abbreviation,",
        "        units,",
        "    })",
        "}",
    ]
    return "\n".join(lines)


def render(tables: dict[int, list]) -> str:
    counts = ", ".join(f"{v} ({len(e)})" for v, e in tables.items())
    arms = "\n".join(f"        ({CENTRE_ECMWF}, {v}) => ecmwf_{v}(id)," for v in tables)
    header = f"""//! GRIB1 centre-local parameter tables — GENERATED by `tools/gen_grib1_local_tables.py`.
//!
//! Do not edit by hand: re-run the script instead.
//!
//! WMO ON388 Table 2 (the international table, versions 1-127) fixes ids
//! 1-127; a centre redefines the whole 1-254 id space in local tables 128+.
//! This carries every ECMWF local table eccodes {PINNED_VERSION} ships. Tables
//! and entry counts: {counts}.
//!
//! Other centres' local tables are not carried: eccodes ships some, in layouts
//! its own reader misparses, and the script explains why generating them would
//! label fields wrongly rather than not at all.
//!
//! Data from eccodes' `definitions/grib1/2.98.<version>.table` (Apache-2.0; the
//! parameter definitions are factual data from the ECMWF parameter database).

use crate::tables::ParameterEntry;

/// Look up `id` in the local table `table_version` of `centre`, the centre
/// whose tables the message reads (see `tables::lookup_parameter` for how that
/// is chosen). `None` when no such table is carried or it leaves `id` undefined.
pub(crate) fn lookup(centre: u8, table_version: u8, id: u8) -> Option<ParameterEntry> {{
    match (centre, table_version) {{
{arms}
        _ => None,
    }}
}}
"""
    return header + "\n" + "\n\n".join(emit_fn(v, e) for v, e in tables.items()) + "\n"


def eccodes_reads(centre: int, sub_centre: int, version: int) -> dict[str, list[str]]:
    """What eccodes decodes for ids 0-255 of one (centre, sub-centre, version).

    One `grib_set` per table: `grib_filter` does not reload a codetable when
    `table2Version` changes mid-run, but does follow `indicatorOfParameter`.
    Ids eccodes leaves undefined, which it prints as the id itself in all three
    places, are omitted.
    """
    out: dict[str, list[str]] = {}
    with tempfile.TemporaryDirectory() as tmp:
        message = Path(tmp) / "m.grib1"
        rules = Path(tmp) / "rules"
        subprocess.run(
            [
                "grib_set",
                "-s",
                f"centre={centre},subCentre={sub_centre},table2Version={version}",
                str(SAMPLE),
                str(message),
            ],
            check=True,
        )
        rules.write_text(
            "".join(
                f"set indicatorOfParameter={i};\n"
                'print "[indicatorOfParameter]|[indicatorOfParameter:s]|[parameterName]|[parameterUnits]";\n'
                for i in range(256)
            ),
            encoding="utf-8",
        )
        printed = subprocess.run(
            ["grib_filter", str(rules), str(message)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=True,
        ).stdout
    for line in printed.splitlines():
        code, abbrev, name, units = line.split("|")
        if abbrev == code and name == code and units == code:
            continue
        out[code] = [abbrev, name, units]
    return out


def main() -> None:
    version_line = subprocess.run(["grib_get_data", "-V"], capture_output=True, text=True, encoding="utf-8").stdout
    assert PINNED_VERSION in version_line, f"eccodes {PINNED_VERSION} required, found {version_line!r}"

    tables = {v: parse_table(v) for v in versions()}
    OUT.write_text(render(tables), encoding="utf-8")

    oracle = {
        "eccodes": PINNED_VERSION,
        "note": "grib_filter over GRIB1.tmpl stamped with each centre/subCentre/table2Version: "
        "[abbreviation, parameterName, parameterUnits] per defined id. "
        "Written by tools/gen_grib1_local_tables.py.",
        "tables": {},
    }
    cases = [(CENTRE_ECMWF, 0, v) for v in tables] + SELECTION_CASES
    for centre, sub_centre, version in cases:
        oracle["tables"][f"{centre}/{sub_centre}/{version}"] = eccodes_reads(centre, sub_centre, version)
    ORACLE.write_text(json.dumps(oracle, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")

    total = sum(len(e) for e in tables.values())
    print(f"wrote {OUT.name}: {len(tables)} tables, {total} entries", file=sys.stderr)
    print(f"wrote {ORACLE.name}: {len(cases)} tables", file=sys.stderr)


if __name__ == "__main__":
    main()
