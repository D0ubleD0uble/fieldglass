#!/usr/bin/env python3
"""Generate crates/fieldglass-grib1/src/tables_ecmwf.rs from the eccodes
ECMWF local parameter tables (2.98.128 / 2.98.129).

Line format: `<code> <abbrev> <name> (<units>)`. Units are the final
balanced-parenthesis group; the name is everything between the abbrev and
that group. `~` abbreviations / units mean "unset" -> empty string.
"""
import re
import sys

SRC = "/usr/share/eccodes/definitions/grib1"


def split_units(rest: str):
    """Return (name, units). Units = last balanced (...) group at end."""
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
                name = rest[:i].strip()
                units = rest[i + 1:-1].strip()
                return name, units
    return rest, ""


def parse_table(version: int):
    entries = []
    with open(f"{SRC}/2.98.{version}.table") as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            parts = line.split(None, 2)
            if len(parts) < 2:
                continue
            try:
                code = int(parts[0])
            except ValueError:
                continue
            abbrev = parts[1]
            rest = parts[2] if len(parts) == 3 else ""
            name, units = split_units(rest)
            if abbrev == "~":
                abbrev = ""
            if units == "~":
                units = ""
            entries.append((code, name, abbrev, units))
    return entries


def rs_escape(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def emit_fn(fn_name: str, version: int, entries) -> str:
    lines = [
        f"/// ECMWF local parameter table {version} (centre 98). Generated from",
        f"/// eccodes' `definitions/grib1/2.98.{version}.table`.",
        f"fn {fn_name}(id: u8) -> Option<ParameterEntry> {{",
        "    let (name, abbreviation, units) = match id {",
    ]
    for code, name, abbrev, units in entries:
        lines.append(
            f'        {code} => ("{rs_escape(name)}", "{rs_escape(abbrev)}", "{rs_escape(units)}"),'
        )
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


def main():
    t128 = parse_table(128)
    t129 = parse_table(129)
    header = '''//! ECMWF GRIB1 local parameter tables (versions 128 and 129).
//!
//! WMO ON388 Table 2 (the international table, versions 1-3) only covers
//! parameter ids 1-127 with fixed meanings; centres redefine the full
//! 1-254 space in local tables 128+. ECMWF (originating centre 98) uses
//! table 128 for the bulk of IFS / ERA5 surface and single-level fields
//! (2t, 10u, 10v, msl, z, …). Table 129, in eccodes 2.34.1, holds the
//! gradient counterparts of those fields. Without these, ECMWF GRIB1 fields
//! all show as "Unknown".
//!
//! Data generated from eccodes' `definitions/grib1/2.98.128.table` and
//! `2.98.129.table` (Apache-2.0; the parameter definitions are factual data
//! from the ECMWF parameter database). Regenerate after an eccodes upgrade:
//! `python3 tools/gen_ecmwf_tables.py > crates/fieldglass-grib1/src/tables_ecmwf.rs && cargo fmt`.

use crate::tables::ParameterEntry;

/// Look up an ECMWF local-table parameter. Returns `None` when the table
/// version is not an ECMWF local table we carry, or the id is undefined in
/// it (so the caller can fall back to "Unknown").
pub fn lookup(table_version: u8, id: u8) -> Option<ParameterEntry> {
    match table_version {
        128 => ecmwf_128(id),
        129 => ecmwf_129(id),
        _ => None,
    }
}
'''
    out = (
        header
        + "\n"
        + emit_fn("ecmwf_128", 128, t128)
        + "\n\n"
        + emit_fn("ecmwf_129", 129, t129)
        + "\n"
    )
    sys.stdout.write(out)
    sys.stderr.write(f"128: {len(t128)} entries, 129: {len(t129)} entries\n")


if __name__ == "__main__":
    main()
