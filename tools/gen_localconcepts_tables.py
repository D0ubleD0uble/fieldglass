#!/usr/bin/env python3
"""Generate a centre-local GRIB2 parameter table from eccodes' localConcepts.

Source: `definitions/grib2/localConcepts/<centre>/` in an eccodes install
(Apache-2.0; the parameter definitions are factual data from the centre's
parameter database). Pinned to eccodes **2.34.1**, the version the rest of the
repo validates against — bump deliberately and re-read the diff.

Centre-parameterised: ECMWF (#424) and DWD (#425) both come from here.

Regenerate:

    python3 tools/gen_localconcepts_tables.py ecmf && cargo fmt
    python3 tools/gen_localconcepts_tables.py edzw && cargo fmt

Add `--oracle` to also rebuild the committed cross-check snapshot, which needs
the eccodes *tools* on PATH and takes a couple of minutes for ECMWF. The test suite reads
the snapshot, so it needs no eccodes at runtime.

# What the concept files are, and what is safe to take from them

Each centre ships four files — `shortName.def`, `name.def`, `units.def`,
`paramId.def` — with one block per parameter. The first three are parallel:
same order, same key sets, so they join by position. `paramId.def` is not
always — DWD ships 1,757 of its blocks against 1,704 short names — so it joins
by key set instead, and it feeds no emitted field either way. A block looks
like:

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

| bucket | ecmf @ 2.34.1 | edzw @ 2.34.1 | what happens |
|---|---|---|---|
| triple only | 2,813 | 213 | emitted, matches any local table version |
| triple + `localTablesVersion` | 13 | 0 | emitted, gated on that version |
| triple + §4 keys | 48 | 100 | **skipped** |
| triple claimed by several blocks | 6 | 50 | **skipped** |
| placeholder name | 642 | 254 | **skipped** |

Skipping is not a compromise, it is the behaviour that agrees with eccodes:
given a message whose §4 keys do not match, eccodes reports `unknown` too. The
table is therefore silent exactly where eccodes is silent, which is what the
`--oracle` cross-check asserts.

Two more exclusions, both upstream of the buckets above:

  * **Codes outside 192-254.** The dispatch seam only offers local code space to
    a centre, so a block on a standard triple would be dead data. 47 ecmf and
    983 edzw blocks are lost this way (15 and 517 distinct triples); they are
    names we do not gain, never names we get wrong. DWD loses more than half
    its file here and another 254 triples to `DUMMY_n` placeholders, which is
    why its table is a thirteenth the size of ECMWF's from a source half as
    long. (The survey in `docs/planning/parameter-table-sources.md`
    quotes 40 and 1,069 for the same buckets. It counted from the concept files
    differently — this generator's numbers are the ones the output is built
    from, and the ones to trust.)
  * **Any component of 255**, which WMO assigns to "missing" rather than to a
    centre.

`~` in `shortName` or `units` is eccodes' "unset" marker and becomes an empty
string, the same convention `gen_ecmwf_tables.py` follows for GRIB1. DWD writes
`''` for the same thing, which parses to an empty string already — the two
spellings are indistinguishable in the emitted table, which is why the oracle
reads one key per pass rather than splitting a joined line (see
`build_oracle`).
"""
from __future__ import annotations

import argparse
import collections
import json
import re
import subprocess
import sys
import tempfile
import textwrap
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
# `dummy_` is DWD's equivalent: 508 blocks named `DUMMY_1` … `DUMMY_508`, short
# name and long name identical, no units, filling out whole local categories.
SKIP_NAME_PREFIXES = ("reserved", "dummy_")
SKIP_NAMES = {"experimental product"}

# The keys that identify a parameter rather than constrain a message.
TRIPLE = ("discipline", "parameterCategory", "parameterNumber")
# The one §1 key our dispatch seam carries, so a block constraining it is still
# resolvable (`Originator::local_tables_version`).
VERSION_KEY = "localTablesVersion"

# A committed GRIB2 message to drive the oracle against. Any message works —
# the oracle rewrites §1 and every emitted triple is claimed by exactly one
# concept block, so eccodes has nothing else it could match. That was checked,
# not assumed: rebuilding both snapshots against `regular_latlon_surface.grib2`,
# which sits on a different `typeOfFirstFixedSurface`, reproduces all 3,039
# entries unchanged. This one is already in the tree, so rebuilding the snapshot
# needs nothing from `samples/`.
ORACLE_BASE = Path("crates/fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2")

# WMO Common Code Table C-11 codes for the centres whose directories we read.
# eccodes keys `localConcepts/<dir>` off §1 `centre`, so the oracle has to set
# it; `tables_local.rs` keys its dispatch off the same numbers.
CENTRE_CODES = {"ecmf": 98, "edzw": 78}

Block = collections.namedtuple("Block", "short name units param_id version qualifiers")


def parse_concepts(centre: str) -> tuple[list[Block], list[tuple[int, int, int] | None]]:
    """Read a centre's concept files into one list of blocks, plus their triples.

    `shortName.def`, `name.def` and `units.def` are joined positionally and the
    join is checked block by block: they carry the three fields we emit, so a
    silent mis-join there would ship a name against the wrong triple.

    `paramId.def` is joined on the key set instead, because it is *not* always
    parallel to the other three — DWD ships 1,757 paramId blocks against 1,704
    short names at 2.34.1, the extra 53 all constraining §4 keys. We emit no
    paramId, so it stays a diagnostic rather than a hard requirement.
    """
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

    short, name, units = read("shortName.def"), read("name.def"), read("units.def")
    lengths = {len(short), len(name), len(units)}
    if len(lengths) != 1:
        raise SystemExit(f"{centre}: shortName/name/units disagree on length: {lengths}")

    # paramId by key set. A key set several paramId blocks claim is ambiguous,
    # so it resolves to nothing rather than to whichever came last.
    by_keys: dict[frozenset[tuple[str, str]], str | None] = {}
    for param_id, keys in read("paramId.def"):
        signature = frozenset(keys.items())
        by_keys[signature] = None if signature in by_keys else param_id

    blocks = []
    for index, (short_name, keys) in enumerate(short):
        # The files are parallel by construction, but "by construction" is how
        # a silently mis-joined table happens; check rather than trust.
        for other, label in ((name, "name"), (units, "units")):
            if other[index][1] != keys:
                raise SystemExit(f"{centre}: {label}.def block {index} has different keys")
        blocks.append(
            Block(
                short=unset(short_name),
                name=name[index][0],
                units=unset(units[index][0]),
                param_id=by_keys.get(frozenset(keys.items())),
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
    gated_summary = (
        f"{len(gated)} gated on the centre's local table version"
        if gated
        else "none gated on the centre's local table version"
    )
    # Wrapped here rather than left to rustfmt, which does not reflow doc
    # comments — the counts change with every eccodes bump, and a paragraph
    # whose line breaks drift with a number is a needless diff.
    coverage_note = textwrap.fill(
        f"{len(ungated)} parameters on the triple alone, {gated_summary}. "
        f"{len(skipped)} further triples are deliberately absent: eccodes resolves "
        "them using §4 keys this seam does not carry, so it reports `unknown` for "
        "them too unless those keys match. See the generator's module docs for the "
        "full rule.",
        width=76,
        initial_indent="//! ",
        subsequent_indent="//! ",
    )
    # A centre that gates nothing gets no `gated` helper at all: an empty match
    # is an unreachable expression, which `-D warnings` rejects, and a dead
    # function is worse documentation than its absence.
    if gated:
        version_param = "local_tables_version: u8"
        gated_call = """    // Version-gated entries win: they are the more specific rule, exactly as
    // they are for eccodes, which matches the block constraining the most keys.
    if let Some(entry) = gated(local_tables_version, discipline, category, number) {
        return Some(entry);
    }
"""
        gated_arms = "\n".join(
            f"        ({version}, {t[0]}, {t[1]}, {t[2]}) => "
            f"({rs(gated[(version, t)].short)}, {rs(gated[(version, t)].name)}, "
            f"{rs(gated[(version, t)].units)}),"
            for version, t in sorted(gated)
        )
        gated_fn = f'''
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
    else:
        # Kept in the signature so the dispatch seam calls every centre table
        # the same way, and so adding a gated entry later is a regeneration
        # rather than a call-site change.
        version_param = "_local_tables_version: u8"
        gated_call = ""
        gated_fn = ""

    # The versions a sweep has to visit to see every entry. Derived from the
    # emitted rows rather than restated, so it cannot fall out of step with them
    # (#476) — a hand-kept copy is exactly the enumeration that goes stale.
    gated_versions = ", ".join(str(v) for v in sorted({v for v, _ in gated}))

    return f'''//! {centre.upper()} local GRIB2 parameters, generated from eccodes.
//!
//! Do not edit: regenerate with `python3 tools/gen_localconcepts_tables.py {centre}`.
//! Source: eccodes {ECCODES_VERSION}, `definitions/grib2/localConcepts/{centre}/`
//! (Apache-2.0; the definitions are factual data from the centre's parameter
//! database).
//!
{coverage_note}

/// The local table versions this centre gates entries on, beyond the ungated
/// ones every version sees. A caller that wants to reach every entry — the unit
/// sweep in `tests/unit_notation.rs` is the one that does — visits version 0
/// plus each of these.
pub(crate) const GATED_VERSIONS: &[u8] = &[{gated_versions}];

/// Look up a local {centre.upper()} parameter, or `None` when this table does not
/// define the triple for that local table version.
pub(crate) fn lookup(
    {version_param},
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str, &'static str)> {{
{gated_call}    let entry = match (discipline, category, number) {{
{ungated_arms}
        _ => return None,
    }};
    Some(entry)
}}
{gated_fn}'''


def build_oracle(centre: str, ungated, gated) -> dict:
    """Ask eccodes itself what each emitted triple resolves to.

    A genuinely independent transcription: this drives eccodes' own concept
    engine through `grib_set`/`grib_get`, where the table above comes from
    parsing the definition text. The two agreeing is what makes the parser
    trustworthy; either alone is not.

    The base message is ECMWF's, so §1 `centre` is rewritten along with the
    triple: it is what selects the `localConcepts` directory, and leaving it
    alone answers `unknown unknown unknown` for every DWD triple — a snapshot
    that would look like a full run and assert nothing.

    Each field is read in its own `grib_get` pass rather than as one
    `-p shortName,name,units` line. The joined line cannot be split back into
    three: names and units both contain spaces, and an unset field prints as an
    empty string in one centre's files (`edzw` writes `''`) and as `~` in
    another's (`ecmf` writes `~`), so reconstructing it needs a rule for which
    marker eccodes will use. One key per pass has no such rule.

    GRIB messages concatenate, so the passes run over every triple's message at
    once and cost three `grib_get` calls rather than three per triple.
    """
    if not ORACLE_BASE.is_file():
        raise SystemExit(f"oracle base {ORACLE_BASE} is missing")
    if centre not in CENTRE_CODES:
        raise SystemExit(f"no C-11 centre code known for {centre!r}; add one to CENTRE_CODES")
    work = sorted(
        [(0, t) for t in ungated] + [(v, t) for v, t in gated],
    )
    with tempfile.TemporaryDirectory(prefix="fg-oracle-") as workdir:
        joined = Path(workdir) / "all.grib2"
        with joined.open("wb") as sink:
            for index, (version, triple) in enumerate(work):
                if index % 500 == 0:
                    print(f"  building {index}/{len(work)}", file=sys.stderr)
                one = Path(workdir) / "one.grib2"
                one.unlink(missing_ok=True)
                subprocess.run(
                    [
                        "grib_set",
                        "-s",
                        f"centre={CENTRE_CODES[centre]},localTablesVersion={version},"
                        f"discipline={triple[0]},parameterCategory={triple[1]},"
                        f"parameterNumber={triple[2]}",
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
            # `stdout` ends with a newline, so the split leaves one empty tail.
            if lines and lines[-1] == "":
                lines.pop()
            if len(lines) != len(work):
                raise SystemExit(
                    f"grib_get returned {len(lines)} {key} values for {len(work)} messages"
                )
            fields.append(lines)

    out: dict[str, list[str]] = {}
    for (version, triple), short, name, units in zip(work, *fields):
        if "unknown" in (short, name, units):
            raise SystemExit(
                f"eccodes resolved {centre} {version}/{triple} to "
                f"{(short, name, units)!r}. Either the centre code is wrong or the "
                "generator emitted a triple eccodes cannot reach from §1 alone; do "
                "not commit this snapshot."
            )
        out[f"{version}/{triple[0]}/{triple[1]}/{triple[2]}"] = [short, name, units]
    return {
        "centre": centre,
        "centreCode": CENTRE_CODES[centre],
        "eccodes": ECCODES_VERSION,
        "resolved": out,
    }


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
