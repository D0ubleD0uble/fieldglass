#!/usr/bin/env python3
"""Generate crates/fieldglass-grib2/src/tables_wmo.rs from the WMO GRIB2
master code tables.

Source: the machine-readable CSVs published on tagged releases of
`wmo-im/GRIB2` (MIT). We pin a tag rather than tracking `master` so the
generated file is reproducible: regenerating from the same tag must be
byte-identical, and moving to a new tag is a reviewable diff.

Covers, from that one download:

  * **Table 4.2** — parameter by discipline/category/number, all 60
    discipline/category files (~1.4k parameters).
  * **Table 4.5** — fixed surface types.
  * **Table 4.4** — indicator of unit of time range.

Regenerate:

    python3 tools/gen_wmo_grib2_tables.py && cargo fmt

The file is written directly rather than piped from stdout. A redirected stdout
takes its encoding from the platform locale, which on a machine that is neither
UTF-8 nor `C`/`POSIX` silently substitutes the non-ASCII this table carries —
14 lines of it at v37 — and quietly breaks the byte-reproducibility the pinned
tag exists to give (#451).

Two things about the upstream data that are worth knowing before editing:

  * **There is no abbreviation column.** WMO publishes names and units, not
    short names, so every entry here has only those two. The third column is
    joined on at lookup time from a second upstream — wgrib2's centre-0 rows,
    generated separately by `gen_wmo_short_names.py` (#469) — precisely so this
    file stays byte-reproducible from its own pinned WMO tag. Centre-specific
    short names are separate again (#424-#426).
  * **`Status` has typos upstream** — `Operationaal`, `Oprational`,
    `Operation`, `operational` all appear alongside `Operational` in v37. Any
    filter on that column must fold case and tolerate them, which is one
    reason this script does not filter on it: deprecated entries stay
    displayable (a file in the wild may still use one), and the status is
    emitted as a comment instead.

Units are copied verbatim. Upstream is not internally consistent about them
(`m s-1` and `m/s` both appear, as do `J kg-1` and `J/kg`), and a prettifier
that rewrote them into the curated table's `m s⁻¹` style would be a second
source of truth for no correctness gain -- and would mangle strings like
`(m2 s sr eV/nuc)-1`. What WMO publishes is what we show.
"""
from __future__ import annotations

import csv
import io
import re
import sys
import tarfile
import urllib.request
from pathlib import Path

# Pinned upstream. Bump deliberately, and re-run the spot-check test: WMO
# fast-track updates land twice a year (see docs/planning/standards-watch-list.md).
WMO_TAG = "v37"
OUT = Path("crates/fieldglass-grib2/src/tables_wmo.rs")
SOURCE_URL = "https://github.com/wmo-im/GRIB2/archive/refs/tags/v37.tar.gz"

# Rows whose "name" is one of these carry no information a reader wants; they
# mark unassigned code space. Emitting them would turn a clean "unknown
# parameter" into a confident, useless label.
SKIP_NAME_PREFIXES = ("reserved",)
SKIP_NAMES = {"missing"}


def fetch_csvs() -> dict[str, str]:
    """Download the pinned release tarball and return {basename: text}.

    Single-argument `urlopen` on a module-level literal so semgrep can
    constant-fold the URL and see it is not attacker-controlled.
    """
    with urllib.request.urlopen(SOURCE_URL) as response:  # noqa: S310 - pinned literal
        payload = response.read()
    out: dict[str, str] = {}
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:gz") as tar:
        for member in tar.getmembers():
            name = Path(member.name).name
            if not member.isfile() or not name.endswith(".csv"):
                continue
            handle = tar.extractfile(member)
            if handle is None:
                continue
            out[name] = handle.read().decode("utf-8-sig")
    return out


def rows(text: str) -> list[dict[str, str]]:
    return list(csv.DictReader(io.StringIO(text)))


def clean(value: str) -> str:
    """Collapse the whitespace CSV cells sometimes carry across line breaks."""
    return re.sub(r"\s+", " ", value or "").strip()


def usable(name: str) -> bool:
    lowered = name.lower()
    return bool(name) and lowered not in SKIP_NAMES and not lowered.startswith(SKIP_NAME_PREFIXES)


def units_of(row: dict[str, str]) -> str:
    """Units, with WMO's placeholder for "dimensionless" normalised to empty.

    A bare `-` means the quantity has no units; rendering it literally would
    put a stray dash in the units column.
    """
    unit = clean(row.get("UnitComments_en", ""))
    return "" if unit == "-" else unit


def rs(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def parse_parameters(csvs: dict[str, str]) -> dict[tuple[int, int], list[tuple]]:
    """Table 4.2, keyed by (discipline, category) -> sorted entries."""
    tables: dict[tuple[int, int], list[tuple]] = {}
    pattern = re.compile(r"^GRIB2_CodeFlag_4_2_(\d+)_(\d+)_CodeTable_en\.csv$")
    for filename, text in sorted(csvs.items()):
        match = pattern.match(filename)
        if not match:
            continue
        key = (int(match.group(1)), int(match.group(2)))
        entries = []
        for row in rows(text):
            code = clean(row.get("CodeFlag", ""))
            # Range rows ("192-254") describe reserved spans, not parameters.
            if not code.isdigit():
                continue
            name = clean(row.get("MeaningParameterDescription_en", ""))
            if not usable(name):
                continue
            entries.append((int(code), name, units_of(row), clean(row.get("Status", ""))))
        if entries:
            tables[key] = sorted(entries)
    return tables


def parse_code_table(csvs: dict[str, str], filename: str) -> list[tuple]:
    """A flat code table (4.4, 4.5) -> sorted (code, name) entries."""
    entries = []
    for row in rows(csvs[filename]):
        code = clean(row.get("CodeFlag", ""))
        if not code.isdigit():
            continue
        name = clean(row.get("MeaningParameterDescription_en", ""))
        if not usable(name):
            continue
        unit = units_of(row)
        # 4.5 records the level's unit separately; the curated labels carry it
        # in parentheses, so match that shape.
        label = f"{name} ({unit})" if unit and unit.lower() not in {"numeric", "code table"} else name
        entries.append((int(code), label))
    return sorted(entries)


def category_fn_name(discipline: int, category: int) -> str:
    return f"discipline_{discipline}_category_{category}"


def emit_parameter_table(tables: dict[tuple[int, int], list[tuple]]) -> list[str]:
    out: list[str] = []
    out.append("/// WMO Code Table 4.2 — parameter by discipline, category, and number.")
    out.append("///")
    out.append("/// Returns `(name, units)`; WMO publishes no short name, so the")
    out.append("/// abbreviation is the caller's to supply (see `tables::lookup_parameter`).")
    out.append("pub(crate) fn parameter(")
    out.append("    discipline: u8,")
    out.append("    category: u8,")
    out.append("    number: u8,")
    out.append(") -> Option<(&'static str, &'static str)> {")
    out.append("    match (discipline, category) {")
    for (discipline, category) in sorted(tables):
        fn = category_fn_name(discipline, category)
        out.append(f"        ({discipline}, {category}) => {fn}(number),")
    out.append("        _ => None,")
    out.append("    }")
    out.append("}")
    out.append("")

    for (discipline, category), entries in sorted(tables.items()):
        fn = category_fn_name(discipline, category)
        out.append(f"/// Discipline {discipline}, parameter category {category}.")
        out.append(f"fn {fn}(number: u8) -> Option<(&'static str, &'static str)> {{")
        out.append("    Some(match number {")
        for code, name, unit, status in entries:
            suffix = "  // deprecated" if status.lower().startswith(("deprecat",)) else ""
            out.append(f"        {code} => ({rs(name)}, {rs(unit)}),{suffix}")
        out.append("        _ => return None,")
        out.append("    })")
        out.append("}")
        out.append("")
    return out


def emit_code_table(fn: str, doc: str, entries: list[tuple]) -> list[str]:
    out = [f"/// {doc}", f"pub(crate) fn {fn}(value: u8) -> Option<&'static str> {{"]
    out.append("    Some(match value {")
    for code, name in entries:
        out.append(f"        {code} => {rs(name)},")
    out.append("        _ => return None,")
    out.append("    })")
    out.append("}")
    out.append("")
    return out


def main() -> int:
    csvs = fetch_csvs()
    parameters = parse_parameters(csvs)
    surfaces = parse_code_table(csvs, "GRIB2_CodeFlag_4_5_CodeTable_en.csv")
    time_units = parse_code_table(csvs, "GRIB2_CodeFlag_4_4_CodeTable_en.csv")

    count = sum(len(v) for v in parameters.values())
    lines = [
        "//! WMO GRIB2 master code tables — generated, do not edit by hand.",
        "//!",
        f"//! Generated from the `wmo-im/GRIB2` release **{WMO_TAG}** CSVs by",
        "//! `tools/gen_wmo_grib2_tables.py`. Those tables are published by the World",
        "//! Meteorological Organization under the MIT licence; the parameter",
        "//! definitions themselves are factual data from WMO Manual on Codes (FM 92",
        "//! GRIB), Code Tables 4.2, 4.4, and 4.5.",
        "//!",
        f"//! {count} parameters across {len(parameters)} discipline/category tables,",
        f"//! {len(surfaces)} fixed surface types, {len(time_units)} time-range units.",
        "//!",
        "//! Deprecated entries are kept and marked: a file in the wild may still use",
        "//! one, and naming it is better than reporting an unknown parameter. Entries",
        "//! WMO marks reserved, reserved-for-local-use, or missing are omitted, so the",
        "//! caller can fall back cleanly instead of showing a placeholder as a name.",
        "//!",
        "//! Regenerate:",
        "//!",
        "//! ```text",
        "//! python3 tools/gen_wmo_grib2_tables.py && cargo fmt",
        "//! ```",
        "",
    ]
    lines += emit_parameter_table(parameters)
    lines += emit_code_table(
        "fixed_surface",
        "WMO Code Table 4.5 — fixed surface types, with units where WMO gives them.",
        surfaces,
    )
    lines += emit_code_table(
        "time_range_unit",
        "WMO Code Table 4.4 — indicator of unit of time range.",
        time_units,
    )
    OUT.write_text("\n".join(lines).rstrip("\n") + "\n", encoding="utf-8")
    print(
        f"generated {count} parameters / {len(surfaces)} surfaces / "
        f"{len(time_units)} time units from wmo-im/GRIB2 {WMO_TAG} -> {OUT}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
