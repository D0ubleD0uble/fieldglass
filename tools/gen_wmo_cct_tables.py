#!/usr/bin/env python3
"""Generate the originating-centre and sub-centre tables from the WMO CCT.

Source: the machine-readable CSVs published on tagged releases of
`wmo-im/CCT` (MIT). Pinned rather than tracking `master`, like
`gen_wmo_grib2_tables.py`: regenerating from the same tag must be
byte-identical, and moving to a new tag is a reviewable diff.

Writes four files from one download:

  * `crates/fieldglass-grib1/src/tables_cct.rs` — centres, from **C-1**.
  * `crates/fieldglass-grib2/src/tables_cct.rs` — centres, from **C-11**.
  * `crates/fieldglass-core/src/cct_tables.rs` — sub-centres, from **C-12**.
  * `crates/fieldglass-grib2/tests/fixtures/wmo_cct.ref.json` — the oracle
    the table tests check the shipped tables against.

Regenerate:

    python3 tools/gen_wmo_cct_tables.py && cargo fmt

Four things about the upstream data are worth knowing before editing.

**GRIB1 and GRIB2 do not share a centre table.** C-1 carries the GRIB1
octet-5 assignments, C-11 the GRIB2 ones. They agree on every code the two
editions share, but 11 names differ in spelling or punctuation, C-11 defines 70
codes above 255 that a one-octet GRIB1 field cannot express, and code 255 is
`Missing value` in C-1 but `Not to be used` in C-11. So they are generated
separately, from their own file, into their own crate. eccodes makes the same
split (`definitions/common/c-1.table` and `c-11.table`).

**`)` means "same as the row above".** The printed manual braces a run of
codes that share one name; the CSV transcribes the closing brace as a lone
`)` in the name column. 19 codes in each file carry it — codes 1-3 are all
Melbourne, 4-6 all Moscow, 10-11 both Cairo. Emitting the cell verbatim would
name centre 3 `)`. `parse_centres` carries the last real name forward, which
is what eccodes' transcription does too.

**Sub-centres are namespaced by their originating centre.** C-12 keys on the
pair, and 51 of its 104 distinct sub-centre codes mean different things under
different centres — sub-centre 4 is NCEP's Environmental Modeling Center and
NASA's Goddard Space Flight Center. A flat `sub_centre -> name` table would be
wrong about half the time, so the generated lookup takes both.

**A few WMO names are less informative than the curated ones they replace.**
WMO publishes `Beijing (RSMC)` where the curated table said
`Beijing (RSMC) - CMA`, and ASCII-fies `Norrköping` to `Norrkoping`. #440 asks
that no currently-curated id come out *less* complete than it went in, so
`OVERRIDES` below restores those — and only those. Each is checked against the
CCT text it was written for by `tests/wmo_cct_tables.rs`, so an upstream rename
fails the build instead of leaving a stale override in place.
"""
from __future__ import annotations

import csv
import io
import json
import re
import tarfile
import urllib.request
from pathlib import Path

# Pinned upstream. CCT releases land roughly twice a year; bump deliberately
# and re-read the diff (see docs/planning/standards-watch-list.md).
CCT_TAG = "v2026-06-01"
SOURCE_URL = "https://github.com/wmo-im/CCT/archive/refs/tags/v2026-06-01.tar.gz"

GRIB1_OUT = Path("crates/fieldglass-grib1/src/tables_cct.rs")
GRIB2_OUT = Path("crates/fieldglass-grib2/src/tables_cct.rs")
CORE_OUT = Path("crates/fieldglass-core/src/cct_tables.rs")
ORACLE_OUT = Path("crates/fieldglass-grib2/tests/fixtures/wmo_cct.ref.json")

# The lone `)` the CSV uses for "this code shares the name above".
CONTINUATION = ")"

# Names that mark unassigned code space. Emitting them would turn a clean
# "unknown centre" into a confident, useless label. `Missing value` and
# `Not to be used` are deliberately *not* here: both say something true about
# the code, and both read better than the numeric fallback.
SKIP_NAME_PREFIXES = ("reserved",)

# Curated names #440 replaces that carry detail WMO does not publish, keyed by
# the CCT text they were written against. The value is what ships; the key is
# what the test re-checks against the pinned tag, so an upstream rename surfaces
# instead of silently leaving a stale override behind.
#
# Two kinds only:
#   * the operating agency's acronym, which is how these centres are referred
#     to in practice (`Offenbach (RSMC)` is DWD);
#   * a diacritic WMO's ASCII transcription drops.
# Anything beyond that belongs upstream, not here.
OVERRIDES: dict[int, tuple[str, str]] = {
    1: ("Melbourne", "Melbourne (WMC)"),
    2: ("Melbourne", "Melbourne (WMC)"),
    3: ("Melbourne", "Melbourne (WMC)"),
    4: ("Moscow", "Moscow (WMC)"),
    5: ("Moscow", "Moscow (WMC)"),
    6: ("Moscow", "Moscow (WMC)"),
    38: ("Beijing (RSMC)", "Beijing (RSMC) - CMA"),
    39: ("Beijing (RSMC)", "Beijing (RSMC) - CMA"),
    40: ("Seoul", "Seoul - KMA"),
    54: ("Montreal (RSMC)", "Montreal (RSMC) - CMC"),
    78: ("Offenbach (RSMC)", "Offenbach (RSMC) - DWD"),
    82: ("Norrkoping", "Norrköping - SMHI"),
    85: ("Toulouse (RSMC)", "Toulouse (RSMC) - Météo-France"),
    86: ("Helsinki", "Helsinki - FMI"),
    88: ("Oslo", "Oslo - MET Norway"),
    94: ("Copenhagen", "Copenhagen - DMI"),
}


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


def clean(value: str) -> str:
    """Collapse the whitespace CSV cells sometimes carry across line breaks."""
    return re.sub(r"\s+", " ", value or "").strip()


def usable(name: str) -> bool:
    return bool(name) and not name.lower().startswith(SKIP_NAME_PREFIXES)


def parse_centres(text: str, code_column: str, name_column: str) -> dict[int, str]:
    """Read one centre table, resolving the `)` continuation rows.

    Rows whose code cell is not a plain integer are range headers
    (`00010-00025: Centres in Region I`) or `Not applicable`; they carry no
    assignment and are skipped — but note they are skipped *without* clearing
    the carried name, because upstream puts them between a run and its `)`.
    """
    out: dict[int, str] = {}
    carried: str | None = None
    for row in csv.DictReader(io.StringIO(text)):
        code, name = clean(row[code_column]), clean(row[name_column])
        if name == CONTINUATION:
            name = carried or ""
        elif name:
            carried = name
        if not code.isdigit() or not usable(name):
            continue
        out[int(code)] = name
    return out


def parse_sub_centres(text: str) -> dict[tuple[int, int], str]:
    """Read C-12 as {(centre, sub_centre): name}.

    Every sub-centre 0 row is dropped, not just the centre-less "No sub-centre"
    one: GRIB writes 0 to mean the field is absent, so the generated lookup
    answers `None` for it under every centre. WMO does name 0 under centre 82,
    and keeping it would put a row in the oracle that nothing can ever return.
    """
    out: dict[tuple[int, int], str] = {}
    for row in csv.DictReader(io.StringIO(text)):
        centre = clean(row["CodeFigure_OriginatingCentres"])
        sub = clean(row["CodeFigure_SubCentres"])
        name = clean(row["Name_SubCentres_en"])
        if not centre.isdigit() or not sub.isdigit() or not usable(name):
            continue
        if int(sub) == 0:
            continue
        out[(int(centre), int(sub))] = name
    return out


def apply_overrides(centres: dict[int, str]) -> dict[int, str]:
    """Swap in the curated names, failing loudly if upstream moved under one."""
    out = dict(centres)
    for code, (expected, replacement) in OVERRIDES.items():
        if code not in out:
            continue
        if out[code] != expected:
            raise SystemExit(
                f"override for centre {code} expected {expected!r} in {CCT_TAG}, "
                f"found {out[code]!r} — re-review it against the new text"
            )
        out[code] = replacement
    return out


def rs(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def pattern(codes: list[int]) -> str:
    """Render one match pattern over a run of consecutive codes.

    Three or more become an inclusive range: clippy's `manual_range_patterns`
    rejects `1 | 2 | 3`, and `1..=3` is what the curated table this replaces
    already read.
    """
    if len(codes) >= 3:
        return f"{codes[0]}..={codes[-1]}"
    return " | ".join(str(c) for c in codes)


def render_centres(
    centres: dict[int, str], *, edition: str, table: str, source: str, width: str
) -> str:
    """Emit a `lookup_centre` returning `Option<&'static str>`.

    Consecutive codes sharing a name collapse into one `|` arm, which is both
    how the curated table read and a large part of why the file is short.
    """
    arms: list[tuple[list[int], str]] = []
    for code in sorted(centres):
        name = centres[code]
        if arms and arms[-1][1] == name and arms[-1][0][-1] == code - 1:
            arms[-1][0].append(code)
        else:
            arms.append(([code], name))
    body = "\n".join(f"        {pattern(codes)} => {rs(name)}," for codes, name in arms)
    return f'''//! Originating centres, generated from WMO Common Code Table {table}.
//!
//! Do not edit: regenerate with `python3 tools/gen_wmo_cct_tables.py`.
//! Source: `wmo-im/CCT` `{CCT_TAG}` (MIT), file `{source}`.
//!
//! {edition} has its own assignments; the generator's module docs say why this
//! is not shared with the other edition. {len(centres)} codes.

/// Look up an originating/generating centre name (WMO Common Code Table {table}).
///
/// `None` for codes the table does not assign; callers render the numeric id.
pub fn lookup_centre(centre: {width}) -> Option<&'static str> {{
    let name = match centre {{
{body}
        _ => return None,
    }};
    Some(name)
}}
'''


def render_sub_centres(subs: dict[tuple[int, int], str]) -> str:
    centres = sorted({c for c, _ in subs})
    outer: list[str] = []
    for centre in centres:
        inner = "\n".join(
            f"            {sub} => {rs(subs[(c, sub)])},"
            for c, sub in sorted(subs)
            if c == centre
        )
        outer.append(f"        {centre} => match sub_centre {{\n{inner}\n            _ => return None,\n        }},")
    body = "\n".join(outer)
    return f'''//! Sub-centres, generated from WMO Common Code Table C-12.
//!
//! Do not edit: regenerate with `python3 tools/gen_wmo_cct_tables.py`.
//! Source: `wmo-im/CCT` `{CCT_TAG}` (MIT), file `C12.csv`.
//!
//! Shared by both GRIB editions: C-12 keys on originating-centre codes, and
//! every centre it names sits below 256, where the C-1 and C-11 assignments
//! agree. {len(subs)} pairs across {len(centres)} centres.

/// Look up a sub-centre name (WMO Common Code Table C-12).
///
/// Keyed on the **pair**, not on `sub_centre` alone: 51 of the 104 sub-centre
/// codes C-12 defines mean different things under different centres — 4 is
/// NCEP's Environmental Modeling Center and NASA's Goddard Space Flight
/// Center. A flat table would be wrong about half the time.
///
/// `None` for a pair the table does not assign, and always for `sub_centre`
/// 0, which GRIB uses to mean "no sub-centre". WMO does list a name against
/// 0 for one centre (82, Norrköping), but a file setting the field to 0 is
/// declaring the field absent, so 0 is answered `None` under every centre.
pub fn lookup_sub_centre(centre: u16, sub_centre: u16) -> Option<&'static str> {{
    if sub_centre == 0 {{
        return None;
    }}
    let name = match centre {{
{body}
        _ => return None,
    }};
    Some(name)
}}
'''


def main() -> int:
    csvs = fetch_csvs()
    for required in ("C01.csv", "C11.csv", "C12.csv"):
        if required not in csvs:
            raise SystemExit(f"{required} not in the {CCT_TAG} release")

    raw1 = parse_centres(csvs["C01.csv"], "Octet5GRIB1_Octet6BUFR3", "OriginatingGeneratingCentres_en")
    raw2 = parse_centres(csvs["C11.csv"], "GRIB2_BUFR4", "OriginatingGeneratingCentre_en")
    # A one-octet GRIB1 centre field cannot carry a code above 255. C-1 assigns
    # none, but assert rather than trust: a future tag that did would otherwise
    # generate a table arm no GRIB1 file can ever reach.
    over = sorted(c for c in raw1 if c > 0xFF)
    if over:
        raise SystemExit(f"C-1 assigned codes above 255, which GRIB1 cannot encode: {over}")
    subs = parse_sub_centres(csvs["C12.csv"])

    centres1, centres2 = apply_overrides(raw1), apply_overrides(raw2)
    GRIB1_OUT.write_text(
        render_centres(
            centres1,
            edition="GRIB1 (PDS octet 5)",
            table="C-1",
            source="C01.csv",
            width="u8",
        )
    )
    GRIB2_OUT.write_text(
        render_centres(
            centres2,
            edition="GRIB2 (§1 octets 6-7)",
            table="C-11",
            source="C11.csv",
            width="u16",
        )
    )
    CORE_OUT.write_text(render_sub_centres(subs))

    # The oracle carries the *unmodified* upstream text plus the overrides, so
    # the test can check both that the table matches WMO and that every
    # difference from it is a declared override rather than a typo.
    ORACLE_OUT.write_text(
        json.dumps(
            {
                "tag": CCT_TAG,
                "grib1_centres": {str(k): v for k, v in sorted(raw1.items())},
                "grib2_centres": {str(k): v for k, v in sorted(raw2.items())},
                "sub_centres": {f"{c}/{s}": n for (c, s), n in sorted(subs.items())},
                "overrides": {str(k): list(v) for k, v in sorted(OVERRIDES.items())},
            },
            indent=1,
            ensure_ascii=False,
            sort_keys=False,
        )
        + "\n"
    )
    print(
        f"{CCT_TAG}: {len(centres1)} GRIB1 centres, {len(centres2)} GRIB2 centres, "
        f"{len(subs)} sub-centre pairs, {len(OVERRIDES)} overrides"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
