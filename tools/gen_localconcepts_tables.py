#!/usr/bin/env python3
"""Generate a centre-local GRIB2 parameter table from eccodes' localConcepts.

Source: `definitions/grib2/localConcepts/<centre>/` in an eccodes install
(Apache-2.0; the parameter definitions are factual data from the centre's
parameter database). Pinned to eccodes **2.34.1**, the version the rest of the
repo validates against — bump deliberately and re-read the diff.

Centre-parameterised because #425 wants the same reader for DWD. Note that DWD
does *not* currently fit the dispatch seam this feeds (see
`docs/planning/parameter-table-sources.md`); the parser is shared, the decision
about what to emit is not.

Regenerate:

    python3 tools/gen_localconcepts_tables.py ecmf && cargo fmt

Add `--oracle` to also rebuild the committed cross-check snapshot, which needs
the eccodes *tools* on PATH and takes a couple of minutes. The test suite reads
the snapshot, so it needs no eccodes at runtime.

# What the concept files are, and what is safe to take from them

Each centre ships four parallel files — `shortName.def`, `name.def`,
`units.def`, `paramId.def` — with one block per parameter, in the same order,
carrying identical key sets. A block looks like:

    'lcc' = {
         localTablesVersion = 1 ;
         discipline = 0 ;
         parameterCategory = 6 ;
         parameterNumber = 193 ;
        }

eccodes resolves a message by finding the block whose keys *all* match it. Our
dispatch seam keys on the originating centre, its sub-centre, the local table
version, and the parameter triple — so a block that also constrains §4 keys
(`typeOfFirstFixedSurface`, `typeOfStatisticalProcessing`, …) cannot be
resolved here, and emitting it as an unconditional triple would name a
parameter the file may not actually carry.

So blocks are emitted only when what they constrain is what we can check:

| bucket | ecmf @ 2.34.1 | what happens |
|---|---|---|
| triple only | 2,813 | emitted, matches any local table version |
| triple + `localTablesVersion` | 13 | emitted, gated on that version |
| triple + §4 keys | 48 | **skipped** |
| triple claimed by several blocks | 6 | **skipped** |
| placeholder name | 642 | **skipped** |

Skipping is not a compromise, it is the behaviour that agrees with eccodes:
given a message whose §4 keys do not match, eccodes reports `unknown` too. The
table is therefore silent exactly where eccodes is silent, which is what the
`--oracle` cross-check asserts.

Two more exclusions, both upstream of the buckets above:

  * **Codes outside 192-254.** The dispatch seam only offers local code space to
    a centre, so a block on a standard triple would be dead data. 40 ecmf blocks
    are lost this way; they are names we do not gain, never names we get wrong.
  * **Any component of 255**, which WMO assigns to "missing" rather than to a
    centre.

`~` in `shortName` or `units` is eccodes' "unset" marker and becomes an empty
string, the same convention `gen_ecmwf_tables.py` follows for GRIB1.
"""
from __future__ import annotations

import argparse
import collections
import json
import re
import subprocess
import sys
from pathlib import Path

# The eccodes the repo validates against. `gen_ecmwf_tables.py` reads the same
# install for the GRIB1 tables.
ECCODES_VERSION = "2.34.1"
DEFINITIONS = Path("/usr/share/eccodes/definitions")

# WMO reserves these for local use in each of discipline, category and number;
# 255 is "missing" and deliberately outside. Mirrors `tables::LOCAL_USE`.
LOCAL_USE = range(192, 255)

# Names that carry no information a reader wants. Emitting them turns a clean
# "unknown parameter" into a confident, useless label — `Experimental product`
# is 635 identical entries with no short name and no units, so a file using one
# would show that instead of the code that at least identifies it. Same rule,
# and same reason, as `gen_wmo_grib2_tables.py`.
SKIP_NAME_PREFIXES = ("reserved",)
SKIP_NAMES = {"experimental product"}

# The keys that identify a parameter rather than constrain a message.
TRIPLE = ("discipline", "parameterCategory", "parameterNumber")
# The one §1 key our dispatch seam carries, so a block constraining it is still
# resolvable (`Originator::local_tables_version`).
VERSION_KEY = "localTablesVersion"

# A committed GRIB2 message to drive the oracle against. Any ECMWF message
# works — the oracle rewrites its triple — and this one is already in the tree,
# so rebuilding the snapshot needs nothing from `samples/`.
ORACLE_BASE = Path("crates/fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2")

Block = collections.namedtuple("Block", "short name units param_id version qualifiers")


def parse_concepts(centre: str) -> list[Block]:
    """Read the four parallel concept files for `centre` into one list."""
    base = DEFINITIONS / "grib2" / "localConcepts" / centre
    if not base.is_dir():
        raise SystemExit(f"no localConcepts for {centre!r} under {base}")

    def read(filename: str) -> list[tuple[str, dict[str, str]]]:
        text = (base / filename).read_text(encoding="utf-8", errors="replace")
        out = []
        for match in re.finditer(r"'((?:[^'\\]|\\.)*)'\s*=\s*\{([^}]*)\}", text):
            keys = {
                key: value.strip()
                for key, value in re.findall(r"(\w+)\s*=\s*([^;\n]+);", match.group(2))
            }
            out.append((match.group(1), keys))
        return out

    short, name = read("shortName.def"), read("name.def")
    units, param = read("units.def"), read("paramId.def")
    lengths = {len(short), len(name), len(units), len(param)}
    if len(lengths) != 1:
        raise SystemExit(f"{centre}: concept files disagree on length: {lengths}")

    blocks = []
    for index, (short_name, keys) in enumerate(short):
        # The files are parallel by construction, but "by construction" is how
        # a silently mis-joined table happens; check rather than trust.
        for other, label in ((name, "name"), (units, "units"), (param, "paramId")):
            if other[index][1] != keys:
                raise SystemExit(f"{centre}: {label}.def block {index} has different keys")
        blocks.append(
            Block(
                short=unset(short_name),
                name=name[index][0],
                units=unset(units[index][0]),
                param_id=param[index][0],
                version=keys.get(VERSION_KEY),
                qualifiers=frozenset(k for k in keys if k not in TRIPLE and k != VERSION_KEY),
            )
        )
    return blocks, [tuple(int(keys[k]) for k in TRIPLE) if all(
        keys.get(k, "").isdigit() for k in TRIPLE) else None for _, keys in short]


def unset(value: str) -> str:
    """eccodes writes `~` for "no value"."""
    return "" if value == "~" else value


def routable(triple: tuple[int, int, int] | None) -> bool:
    return (
        triple is not None
        and any(component in LOCAL_USE for component in triple)
        and 255 not in triple
    )


def select(blocks, triples):
    """Split the blocks into what we can emit and what we must not.

    Returns `(ungated, gated, skipped)` where `ungated` maps a triple to one
    entry, `gated` maps `(version, triple)` to one, and `skipped` records why
    each dropped triple was dropped so the caller can report it.
    """
    by_triple = collections.defaultdict(list)
    for block, triple in zip(blocks, triples):
        if routable(triple):
            by_triple[triple].append(block)

    ungated, gated, skipped = {}, {}, {}
    for triple, candidates in by_triple.items():
        if len(candidates) > 1:
            skipped[triple] = f"{len(candidates)} blocks claim it"
            continue
        block = candidates[0]
        lowered = block.name.lower()
        if lowered in SKIP_NAMES or lowered.startswith(SKIP_NAME_PREFIXES):
            skipped[triple] = f"placeholder name {block.name!r}"
        elif block.qualifiers:
            skipped[triple] = "constrains " + ", ".join(sorted(block.qualifiers))
        elif block.version is None:
            ungated[triple] = block
        else:
            gated[(int(block.version), triple)] = block
    return ungated, gated, skipped


def rs(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render(centre: str, ungated, gated, skipped) -> str:
    def arm(triple, block):
        return (
            f"        ({triple[0]}, {triple[1]}, {triple[2]}) => "
            f"({rs(block.short)}, {rs(block.name)}, {rs(block.units)}),"
        )

    ungated_arms = "\n".join(arm(t, ungated[t]) for t in sorted(ungated))
    gated_arms = "\n".join(
        f"        ({version}, {t[0]}, {t[1]}, {t[2]}) => "
        f"({rs(gated[(version, t)].short)}, {rs(gated[(version, t)].name)}, "
        f"{rs(gated[(version, t)].units)}),"
        for version, t in sorted(gated)
    )
    return f'''//! {centre.upper()} local GRIB2 parameters, generated from eccodes.
//!
//! Do not edit: regenerate with `python3 tools/gen_localconcepts_tables.py {centre}`.
//! Source: eccodes {ECCODES_VERSION}, `definitions/grib2/localConcepts/{centre}/`
//! (Apache-2.0; the definitions are factual data from the centre's parameter
//! database).
//!
//! {len(ungated)} parameters on the triple alone and {len(gated)} gated on the
//! centre's local table version. {len(skipped)} further triples are deliberately
//! absent: eccodes resolves them using §4 keys this seam does not carry, so it
//! reports `unknown` for them too unless those keys match. See the generator's
//! module docs for the full rule.

/// Look up a {centre.upper()} local parameter, or `None` when this table does not
/// define the triple for that local table version.
pub(crate) fn lookup(
    local_tables_version: u8,
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str, &'static str)> {{
    // Version-gated entries win: they are the more specific rule, exactly as
    // they are for eccodes, which matches the block constraining the most keys.
    if let Some(entry) = gated(local_tables_version, discipline, category, number) {{
        return Some(entry);
    }}
    let entry = match (discipline, category, number) {{
{ungated_arms}
        _ => return None,
    }};
    Some(entry)
}}

/// The entries {centre.upper()} defines only for a particular revision of its own
/// table.
fn gated(
    local_tables_version: u8,
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str, &'static str)> {{
    let entry = match (local_tables_version, discipline, category, number) {{
{gated_arms}
        _ => return None,
    }};
    Some(entry)
}}
'''


def build_oracle(centre: str, ungated, gated) -> dict:
    """Ask eccodes itself what each emitted triple resolves to.

    A genuinely independent transcription: this drives eccodes' own concept
    engine through `grib_set`/`grib_get`, where the table above comes from
    parsing the definition text. The two agreeing is what makes the parser
    trustworthy; either alone is not.
    """
    if not ORACLE_BASE.is_file():
        raise SystemExit(f"oracle base {ORACLE_BASE} is missing")
    out: dict[str, list[str]] = {}
    work = [(0, t, b) for t, b in ungated.items()] + [(v, t, b) for (v, t), b in gated.items()]
    for index, (version, triple, _block) in enumerate(sorted(work)):
        if index % 500 == 0:
            print(f"  oracle {index}/{len(work)}", file=sys.stderr)
        settings = (
            f"localTablesVersion={version},discipline={triple[0]},"
            f"parameterCategory={triple[1]},parameterNumber={triple[2]}"
        )
        subprocess.run(
            ["grib_set", "-s", settings, str(ORACLE_BASE), "/tmp/fg_oracle.grib2"],
            check=True,
            capture_output=True,
        )
        got = subprocess.run(
            ["grib_get", "-p", "shortName,name,units", "/tmp/fg_oracle.grib2"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        out[f"{version}/{triple[0]}/{triple[1]}/{triple[2]}"] = got
    return {"centre": centre, "eccodes": ECCODES_VERSION, "resolved": out}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("centre", nargs="?", default="ecmf")
    parser.add_argument("--oracle", action="store_true", help="rebuild the cross-check snapshot")
    args = parser.parse_args()

    blocks, triples = parse_concepts(args.centre)
    ungated, gated, skipped = select(blocks, triples)

    out = Path(f"crates/fieldglass-grib2/src/tables_{args.centre}.rs")
    out.write_text(render(args.centre, ungated, gated, skipped), encoding="utf-8")
    print(
        f"{args.centre} @ eccodes {ECCODES_VERSION}: {len(ungated)} ungated + "
        f"{len(gated)} version-gated emitted, {len(skipped)} skipped",
        file=sys.stderr,
    )

    if args.oracle:
        oracle = Path(f"crates/fieldglass-grib2/tests/fixtures/localconcepts_{args.centre}.ref.json")
        oracle.write_text(
            json.dumps(build_oracle(args.centre, ungated, gated), indent=1, ensure_ascii=False)
            + "\n",
            encoding="utf-8",
        )
        print(f"wrote {oracle}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
