#!/usr/bin/env python3
"""Generate the NCEP local GRIB2 parameter table from wgrib2's `gribtable.dat`.

Regenerate:

    python3 tools/gen_ncep_tables.py && cargo fmt

Add `--crosscheck` to also rebuild the committed comparison snapshot, which
needs the eccodes *tools* on PATH. The test suite reads the snapshot, so it
needs neither wgrib2 nor eccodes at runtime.

# Why wgrib2 and not eccodes

eccodes ships NCEP local concepts too, under `localConcepts/kwbc`, and the
generator behind #424/#425 reads them without a line of new code — 313 entries,
no ambiguity, no placeholders. It was the obvious candidate, so it was measured
rather than assumed:

| source | routable entries | ambiguous | placeholders |
|---|---|---|---|
| eccodes `kwbc` @ 2.34.1 | 313 | 0 | 0 |
| wgrib2 `gribtable.dat` @ v3.8.0 | **479** | 0 | 0 |

wgrib2 is a strict superset — every triple eccodes defines, wgrib2 defines too —
and it carries 166 more. It also carries NCEP's *own* abbreviations, which are
uppercase (`SNOHF`, `MDIV`, `CNVHR`) where eccodes lowercases them by ECMWF
convention. Those uppercase forms are what NCO's own product listings print and
what `tables.rs` already uses for the curated entries, so they are the ones a
reader of a GFS or HRRR file expects.

The two agree on what each triple *means*: of the 313 they share, 310 have the
same abbreviation once case is folded. The three that differ are eccodes
preferring an alternative name — `mconv` for `MDIV`, `tsec` for `RTSEC`,
`elevhtml` for `ELEV` — and NCO's documentation is with wgrib2 on all three.
That comparison is what `--crosscheck` snapshots and
`tests/ncep_local_parameters.rs` asserts; two independent transcriptions of the
same table agreeing is what makes either trustworthy, and #415 is the reminder
of what an unverified table costs.

# The file format

`wgrib2/gribtables/ncep/gribtable.dat` is a C initialiser list, one brace record
per line, produced by wgrib2's own `get_gribtab.sh` from the NCO GRIB2
documentation web pages (which have no machine-readable form). Its columns, per
that script:

    {discipline, masterTableVersion, minVersion, maxVersion, centre,
     localTableVersion, category, number, "ABBREV", "Name", "Units"}

`get_gribtab.sh` writes centre 7 and local table version 1 for every entry with a
parameter code at or above 192, and centre 0 / version 0 for the WMO master
entries. So `centre == 7` is exactly the local set, which is what this reads.

# What is emitted, and what is not

  * **Only centre 7.** The 1,230 rows at centre 0 are WMO master parameters, and
    routing them through the centre-local seam would shadow the generated master
    table for NCEP files alone.

    They are not nothing, though — quite the opposite. WMO publishes no short
    names, so 1,346 of the 1,387 generated master parameters currently show an
    empty abbreviation, and these 1,230 rows would fill almost all of it with
    the forms every NCEP product listing uses. That is a change to the *master*
    table's short-name column for every centre, not a local table, so it is a
    separate issue rather than a quiet widening of this one — but the source is
    already downloaded here when it happens.
  * **Only codes 192-254**, and never a component of 255. One row is dropped by
    this rule: `(255, 255, 255) IMGD "Image data"`, which is the all-missing
    sentinel rather than a parameter.
  * **Ungated on the local table version.** wgrib2 records `1` for every NCEP
    entry, but that is its own bookkeeping rather than a claim about the wire,
    and eccodes' concepts gate none of them. Every NCEP sample in the tree —
    GFS, HRRR, NAM, RAP, NBM, MRMS — does carry `localTablesVersion = 1`, so
    gating would work today; not gating also works if a producer ever writes 0,
    and matches what eccodes answers.

The same source family adds MRMS (`gribtables/mrms/`, centre 161), KMA, NESDIS
and USAF tables in the same format, each keyed to its own centre. They plug into
the same seam and this parser reads them unchanged; they are deliberately not
shipped here.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import urllib.request
from pathlib import Path

# The pinned wgrib2 release. Bump deliberately and re-read the diff.
WGRIB2_TAG = "v3.8.0"
SOURCE_URL = (
    "https://raw.githubusercontent.com/NOAA-EMC/wgrib2/"
    "v3.8.0/wgrib2/gribtables/ncep/gribtable.dat"
)

# WMO Common Code Table C-11 code for NCEP, and the eccodes concept directory
# the cross-check reads.
NCEP_CENTRE = 7
ECCODES_VERSION = "2.34.1"
ECCODES_CONCEPTS = "kwbc"

# WMO reserves these for local use in each of discipline, category and number;
# 255 is "missing" and deliberately outside. Mirrors `tables::LOCAL_USE`.
LOCAL_USE = range(192, 255)

OUT = Path("crates/fieldglass-grib2/src/tables_ncep.rs")
CROSSCHECK = Path("crates/fieldglass-grib2/tests/fixtures/ncep_eccodes_crosscheck.json")
# The same committed message the localConcepts oracle drives; see
# `tools/gen_localconcepts_tables.py`.
ORACLE_BASE = Path("crates/fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2")

RECORD = re.compile(
    r"^\{\s*(-?\d+),\s*(-?\d+),\s*(-?\d+),\s*(-?\d+),\s*(-?\d+),\s*(-?\d+),"
    r"\s*(-?\d+),\s*(-?\d+),\s*\"([^\"]*)\",\s*\"([^\"]*)\",\s*\"([^\"]*)\"\},?\s*$"
)


def fetch_table() -> str:
    """Download the pinned `gribtable.dat`.

    Single-argument `urlopen` on a module-level literal so semgrep can
    constant-fold the URL and see it is not attacker-controlled.
    """
    with urllib.request.urlopen(SOURCE_URL) as response:  # noqa: S310 - pinned literal
        # The file is pure ASCII at v3.8.0; decode strictly so a future release
        # that is not fails here rather than silently substituting characters.
        return response.read().decode("utf-8")


def parse(text: str) -> dict[tuple[int, int, int], tuple[str, str, str]]:
    """Read the brace records into `{triple: (abbrev, name, units)}`.

    Every line must parse. A record shape wgrib2 changes under us is a table
    silently missing rows, which is the failure this refuses to have.
    """
    out: dict[tuple[int, int, int], tuple[str, str, str]] = {}
    for number, line in enumerate(text.splitlines(), start=1):
        if not line.strip():
            continue
        match = RECORD.match(line)
        if match is None:
            raise SystemExit(f"{SOURCE_URL}:{number}: unrecognised record: {line!r}")
        discipline, _mv, _mn, _mx, centre, _ltv, category, num = (
            int(value) for value in match.groups()[:8]
        )
        abbrev, name, units = match.groups()[8:]
        if centre != NCEP_CENTRE:
            continue
        triple = (discipline, category, num)
        if not routable(triple):
            continue
        if triple in out:
            raise SystemExit(f"{triple} is defined twice; the table is no longer unambiguous")
        out[triple] = (abbrev, name, units)
    return out


def routable(triple: tuple[int, int, int]) -> bool:
    return any(component in LOCAL_USE for component in triple) and 255 not in triple


def rs(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render(entries: dict[tuple[int, int, int], tuple[str, str, str]]) -> str:
    arms = "\n".join(
        f"        ({t[0]}, {t[1]}, {t[2]}) => "
        f"({rs(entries[t][0])}, {rs(entries[t][1])}, {rs(entries[t][2])}),"
        for t in sorted(entries)
    )
    return f'''//! NCEP local GRIB2 parameters, generated from wgrib2.
//!
//! Do not edit: regenerate with `python3 tools/gen_ncep_tables.py`.
//! Source: wgrib2 {WGRIB2_TAG}, `wgrib2/gribtables/ncep/gribtable.dat`, itself
//! produced by wgrib2's `get_gribtab.sh` from the NCO GRIB2 documentation.
//! Both are works of the U.S. federal government and in the public domain.
//!
//! {len(entries)} parameters, none gated on the local table version and none
//! ambiguous. See the generator's module docs for why wgrib2 rather than
//! eccodes, and for what the centre-0 rows in the same file are not.

/// The local table versions this centre gates entries on: none, and it cannot
/// be otherwise — [`lookup`] below takes no version to gate on. That is the
/// structural reason, not a claim about today's data; gating NCEP would mean
/// changing the signature, which changes this line with it (#476).
pub(crate) const GATED_VERSIONS: &[u8] = &[];

/// Look up a local NCEP parameter, or `None` when this table does not define
/// the triple.
pub(crate) fn lookup(
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str, &'static str)> {{
    let entry = match (discipline, category, number) {{
{arms}
        _ => return None,
    }};
    Some(entry)
}}
'''


def build_crosscheck(entries: dict[tuple[int, int, int], tuple[str, str, str]]) -> dict:
    """Ask eccodes what it calls each triple, as an independent transcription.

    eccodes reads the same NCO tables through an entirely separate pipeline —
    its own `localConcepts/kwbc` definition files — so agreement is evidence
    about the *table*, not about either parser. Collected the same way as the
    localConcepts oracles: rewrite §1 `centre` and the triple on a committed
    message, then read the keys back, one `grib_get` pass per key over a
    concatenation so it costs three calls rather than three per triple.
    """
    if not ORACLE_BASE.is_file():
        raise SystemExit(f"oracle base {ORACLE_BASE} is missing")
    work = sorted(entries)
    import tempfile

    with tempfile.TemporaryDirectory(prefix="fg-ncep-") as workdir:
        joined = Path(workdir) / "all.grib2"
        one = Path(workdir) / "one.grib2"
        with joined.open("wb") as sink:
            for index, triple in enumerate(work):
                if index % 200 == 0:
                    print(f"  building {index}/{len(work)}", file=sys.stderr)
                one.unlink(missing_ok=True)
                subprocess.run(
                    [
                        "grib_set",
                        "-s",
                        f"centre={NCEP_CENTRE},localTablesVersion=1,discipline={triple[0]},"
                        f"parameterCategory={triple[1]},parameterNumber={triple[2]}",
                        str(ORACLE_BASE),
                        str(one),
                    ],
                    check=True,
                    capture_output=True,
                )
                sink.write(one.read_bytes())
        fields = []
        for key in ("shortName", "name", "units"):
            print(f"  reading {key}", file=sys.stderr)
            lines = subprocess.run(
                ["grib_get", "-p", key, str(joined)],
                check=True,
                capture_output=True,
                text=True,
                encoding="utf-8",
            ).stdout.split("\n")
            if lines and lines[-1] == "":
                lines.pop()
            if len(lines) != len(work):
                raise SystemExit(
                    f"grib_get returned {len(lines)} {key} values for {len(work)} messages"
                )
            fields.append(lines)

    resolved = {
        f"{t[0]}/{t[1]}/{t[2]}": [short, name, units]
        for t, short, name, units in zip(work, *fields)
    }
    return {
        "wgrib2": WGRIB2_TAG,
        "eccodes": ECCODES_VERSION,
        "eccodesConcepts": ECCODES_CONCEPTS,
        "centreCode": NCEP_CENTRE,
        "resolved": resolved,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--crosscheck", action="store_true", help="rebuild the eccodes comparison snapshot"
    )
    args = parser.parse_args()

    entries = parse(fetch_table())
    OUT.write_text(render(entries), encoding="utf-8")
    print(f"wgrib2 {WGRIB2_TAG}: {len(entries)} NCEP local parameters -> {OUT}", file=sys.stderr)

    if args.crosscheck:
        CROSSCHECK.write_text(
            json.dumps(build_crosscheck(entries), indent=1, ensure_ascii=False) + "\n",
            encoding="utf-8",
        )
        print(f"wrote {CROSSCHECK}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
