#!/usr/bin/env python3
"""Snapshot GRIB1 Code Tables 3, 4 and 5 as the authorities give them (#869).

`fieldglass-grib1` names GRIB1 level types (Code Table 3), time units (Table 4)
and time-range indicators (Table 5) from hand-written match arms in
`src/tables.rs` and `src/reader.rs`. Hand-written means unverified: Table 3 sat
one code off for codes 0-9, so every surface field read "Cloud base level".
This writes the two oracles `tests/code_tables.rs` holds them to:

  * `crates/fieldglass-grib1/tests/fixtures/code_tables.eccodes.ref.json` —
    what the pinned eccodes (2.34.1, the CLI on PATH and its definitions under
    /usr/share/eccodes) *decodes* for every code 0-255 of each table, read back
    from real GRIB1 messages stamped with each centre. A decode, not a parse of
    the `.table` files, so the oracle includes which file eccodes selects:
    `section.1.def` reads Tables 3 and 5 from `grib1/local/<centre>/` first and
    the master table second, and Table 4 from the master only.
  * `crates/fieldglass-grib1/tests/fixtures/code_tables.on388.ref.json` — NCEP's
    Office Note 388 Table 3, parsed from the NCO page. eccodes 2.34.1 ships no
    NCEP (`kwbc`) Table 3, so ON388 is the only source for NCEP's local level
    types (126 and the 204-254 specials) and the second, independent
    transcription of the WMO codes. NOAA publications are public domain.

Regenerate (needs eccodes 2.34.1 and network access):

    python3 tools/gen_grib1_code_table_snapshot.py

Each code is stamped into the template's octets directly (centre 5, level type
10, unit of time 18, time-range indicator 21) and every message goes through one
`grib_dump -O`. A code eccodes does not name prints as `Unknown code table
entry` and is omitted.
"""

from __future__ import annotations

import html
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.request import urlopen

SAMPLE = Path("/usr/share/eccodes/samples/GRIB1.tmpl")
PINNED_VERSION = "2.34.1"
ROOT = Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates" / "fieldglass-grib1" / "tests" / "fixtures"
ECCODES_OUT = FIXTURES / "code_tables.eccodes.ref.json"
ON388_OUT = FIXTURES / "code_tables.on388.ref.json"
ON388_URL = "https://www.nco.ncep.noaa.gov/pmb/docs/on388/table3.html"

# Centres stamped: 74 (UK Met Office) has no local tables, so it reads the
# master tables as any WMO centre does; 7 is NCEP, which eccodes also reads from
# the master; 34 (JMA) and 98 (ECMWF) ship a local Table 3 and Table 5.
CENTRES = [74, 7, 34, 98]

# Byte offsets in the message: Section 0 is 8 octets, then PDS octet n is at
# 8 + n - 1.
OCTET_CENTRE = 8 + 5 - 1
OCTET_LEVEL_TYPE = 8 + 10 - 1
OCTET_TIME_UNIT = 8 + 18 - 1
OCTET_TIME_RANGE = 8 + 21 - 1

KEYS = {
    "indicatorOfTypeOfLevel": "3",
    "unitOfTimeRange": "4",
    "timeRangeIndicator": "5",
}
LINE = re.compile(r"^\s*\d+(?:-\d+)?\s+(\w+) = (\d+) \[(.*) \((grib1/[^)]*)\) \]$")


def eccodes_reads(centre: int) -> dict[str, dict[str, str]]:
    """Every name eccodes decodes for codes 0-255 of Tables 3, 4 and 5."""
    template = bytearray(SAMPLE.read_bytes())
    messages = bytearray()
    for code in range(256):
        message = bytearray(template)
        message[OCTET_CENTRE] = centre
        message[OCTET_LEVEL_TYPE] = code
        message[OCTET_TIME_UNIT] = code
        message[OCTET_TIME_RANGE] = code
        messages += message
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "stamped.grib1"
        path.write_bytes(bytes(messages))
        dumped = subprocess.run(
            ["grib_dump", "-O", str(path)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=True,
        ).stdout

    tables: dict[str, dict[str, str]] = {table: {} for table in KEYS.values()}
    seen = {table: 0 for table in KEYS.values()}
    for line in dumped.splitlines():
        match = LINE.match(line)
        if not match or match.group(1) not in KEYS:
            continue
        key, code, title = match.group(1), match.group(2), match.group(3).strip()
        table = KEYS[key]
        seen[table] += 1
        if title != "Unknown code table entry":
            tables[table][code] = title
    # One line per message per key, or the dump did not line up with the codes.
    assert all(n == 256 for n in seen.values()), f"centre {centre}: {seen}"
    return tables


def cell_text(cell: str) -> str:
    return re.sub(r"\s+", " ", html.unescape(re.sub(r"<.*?>", "", cell, flags=re.S))).strip()


def on388_table3() -> dict[str, dict[str, object]]:
    """ON388 Table 3 and 3a: code -> meaning and octet contents.

    The page has two tables. Table 3 (100-201) is `code | meaning | contents of
    octets 11 and 12 (one cell, or one per octet for a layer) | abbreviation`.
    Table 3a (0-99 and NCEP's special levels 204-254) is `code | meaning |
    abbreviation`, with no contents column; where one of its levels carries a
    value, the meaning says so in words. Ranges (`10-19`) and the `0-99`
    pointer row are code space, not assignments, and are skipped, as are rows
    whose meaning is "reserved".
    """
    page = urlopen(ON388_URL).read().decode("latin-1")  # noqa: S310 - fixed https URL
    names: dict[str, dict[str, object]] = {}
    for row in re.findall(r"<tr.*?>(.*?)</tr>", page, flags=re.S | re.I):
        cells = [cell_text(c) for c in re.findall(r"<t[dh].*?>(.*?)</t[dh]>", row, flags=re.S | re.I)]
        if len(cells) < 2 or not re.fullmatch(r"\d+", cells[0]):
            continue
        code, meaning = str(int(cells[0])), cells[1]
        if meaning.lower() == "reserved":
            continue
        names[code] = {"meaning": meaning, "contents": cells[2:-1]}
    return names


def main() -> None:
    version_line = subprocess.run(["grib_dump", "-V"], capture_output=True, text=True, encoding="utf-8").stdout
    assert PINNED_VERSION in version_line, f"eccodes {PINNED_VERSION} required, found {version_line!r}"

    oracle = {
        "eccodes": PINNED_VERSION,
        "note": "grib_dump -O over GRIB1.tmpl stamped with each centre and each code 0-255 in "
        "PDS octets 10 (Table 3), 18 (Table 4) and 21 (Table 5); the decoded title per named "
        "code. Written by tools/gen_grib1_code_table_snapshot.py.",
        "centres": {str(c): eccodes_reads(c) for c in CENTRES},
    }
    ECCODES_OUT.write_text(json.dumps(oracle, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")

    on388 = on388_table3()
    ON388_OUT.write_text(
        json.dumps(
            {
                "source": ON388_URL,
                "note": "NCEP Office Note 388, Table 3 and 3a: meaning and octet-11/12 contents per "
                "assigned code (contents is empty for Table 3a, which has no such column). "
                "Written by tools/gen_grib1_code_table_snapshot.py.",
                "3": on388,
            },
            indent=1,
            ensure_ascii=False,
        )
        + "\n",
        encoding="utf-8",
    )
    print(f"wrote {ECCODES_OUT.name}: centres {CENTRES}", file=sys.stderr)
    print(f"wrote {ON388_OUT.name}: {len(on388)} codes", file=sys.stderr)


if __name__ == "__main__":
    main()
