#!/usr/bin/env python3
"""Generate the MARS short name table `verify_message` checks ECMWF records with.

Regenerate, naming each eccodes definitions tree read and its version:

    python3 tools/gen_mars_short_names.py \
        --definitions 2.34.1=/usr/share/eccodes/definitions \
        --definitions 2.45.0=<eccodes 2.45.0 source>/definitions \
        --definitions 2.48.0=<eccodes 2.48.0 source>/definitions \
        && cargo fmt

Writes `crates/fieldglass/src/fetchplan/mars_short_names.rs`.

# What it is for

An ECMWF `.index` names each message's parameter by its MARS short name
(`"param": "mx2t3"`), not by GRIB2 codes. `fieldglass::fetchplan::
verify_message` compares the name with the decoded message (#1116), and this
table is how: for each name, the eccodes concept blocks that define it.

# Source

eccodes' `shortName` concept, the files eccodes consults for an ECMWF
(centre 98) GRIB2 message: `definitions/grib2/localConcepts/ecmf/` and
`definitions/grib2/`, each `shortName.def` with its `.legacy.def` and, from
eccodes 2.38 on, `.lte33.def` (the encodings of GRIB2 tables version 33 and
older, which the committed ECMWF messages, at version 32, are). Apache-2.0;
the definitions are factual data from ECMWF's parameter database.

Three eccodes versions:

* **2.34.1**, the version the rest of the repo validates against
  (`gen_localconcepts_tables.py` reads the same install).
* **2.45.0**, because ECMWF's newer open-data products postdate 2.34.1. Its
  definitions give `2tl273`, `tpl01`, `tprg3` and five other AIFS ensemble
  probabilities a WMO encoding 2.34.1 does not know, only an older local one.
  A table from 2.34.1 alone would refuse those records when they are right.
* **2.48.0**, the newest read. It dropped `typeOfFirstFixedSurface = 101`
  from `swhg2` to `swhg8`, so a producer on it may write those on another
  surface; without it the table would refuse such a record.

The table therefore assumes a producer's definitions are no looser than the
union of these trees. A later eccodes that loosens a block again (drops a
condition, or adds an encoding) can make a right record fail until the table
is regenerated with that tree added.

# How eccodes names a message

A block looks like

    'mx2t3' = {
         discipline = 0 ;
         parameterCategory = 0 ;
         parameterNumber = 0 ;
         typeOfFirstFixedSurface = 103 ;
         ...
         typeOfStatisticalProcessing = 2 ;
         lengthOfTimeRange = 3 ;
         indicatorOfUnitForTimeRange = 1 ;
        }

A block matches a message when every condition in it holds. Several blocks,
of several names, can match one message: `2t` states the codes and the 2 m
surface, six conditions, and so matches `mx2t3`'s bytes too. eccodes names
the message after the matching block that states the most conditions
(`concept_evaluate` in `src/eccodes/accessor/Concept.cc`), so those bytes are
`mx2t3`, whose block states nine. eccodes 2.34.1 counts an array condition
once; from ECC-1992 (in 2.45.0 and 2.48.0) it counts it once per element, and
the count here follows the version each block was read from.

That ranking is within one set of files at a time, which this table calls a
pool. eccodes reads `shortName.def` (or, from 2.38, `shortName.lte33.def` for
a message of tables version 33 or older) from both directories as one pool,
and falls back to the `.legacy.def` pool only when nothing in it matches
(`grib2/parameters.def`). So each eccodes version gives two or three pools:

    2.34.1  shortName.def
    2.34.1  shortName.legacy.def
    2.45.0  shortName.def
    2.45.0  shortName.lte33.def
    2.45.0  shortName.legacy.def
    2.48.0  shortName.def
    2.48.0  shortName.lte33.def
    2.48.0  shortName.legacy.def

Each block records the pools that hold it. `verify_message` accepts a name
when, in some pool, one of its blocks may match and no block of another name
in that pool certainly matches with more conditions. Which pool a message
reads is not known here (it turns on the tables version, which the message
identity does not carry), so a name any version read would give the message
under any of its sets of files is accepted. A legacy pool is ranked on its own, not
only when its main pool has no match, which can accept a name eccodes would
not reach but never refuses one it would. `shortName.chemsplit.def`, read for
the chemistry split of MARS classes `mc` and `cr`, is not open data and is not
read.

# What is kept from a block

The conditions `MessageIdentity` carries as written: the parameter codes, the
product template, both fixed surfaces (type, scale factor, scaled value, or
`missing()`), the statistical process and the length of its time range with
its unit, and for templates 4.5 and 4.9 the probability type and limits.
These are what tell apart names that share codes: `10u` and `100u` by the
height, `msl` and `sp` and `str` and `ttr` by the surface type, `tp` and
`tpg25` by the probability. Only these kept conditions are ever compared.

Two kinds of names are not told apart. The wave-period heights `h1012` to
`h2530` state a period range the table does not keep, so they stay possible
on any `swh` message. And `tpl01`, `2tl273` and `10spg10` (and the other AIFS
probabilities with only a local encoding in 2.34.1) have no WMO block there,
so eccodes 2.34.1 names their WMO-encoded bytes `tp`, `2t` and `10si`, and
those names pass too.

Every other condition (a local tables version, a wave period range, a
chemical constituent) is counted but not kept. A block with one is never
certain to match, so it can widen what its own name accepts but never
refuses another name. The table can therefore fail to refuse a wrong record,
but never refuses a record eccodes would name so. The level is compared on
its own, from the `levtype`/`levelist` the record states.

# Which names

The parameters ECMWF publishes as open data, listed in `OPEN_DATA` below from
<https://www.ecmwf.int/en/forecasts/datasets/open-data> (read 2026-10-20):
IFS and AIFS, deterministic and ensemble, atmosphere and wave. The whole
concept is about 4,000 names, most of them never in an open-data `.index`, and
the table ships in the browser module, so it carries only these. A name not in
the table is not compared, which is what an ECMWF record got before.

A listed name no eccodes read defines is reported and left out, for the same
reason.

# Rivals

A name outside the list can still be what eccodes calls a message a listed
name also matches. From tables version 34 eccodes names a 3-hour maximum of
2 m temperature `max_2t`, not `mx2t3`, and `2t`'s block matches those bytes
with fewer conditions. So the table also carries, without their names, the
blocks of other names that could outrank a listed name's block: those with
the same parameter codes as a block of a listed name in the same pool, that
state more conditions than the fewest any such block states, and whose every
condition is kept (a block with one that is not can never be certain, so it
could never refuse). They are compared only as rivals: a record naming one is
still not compared.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# The trees the committed table was built from. Regenerating from others is
# refused, so a stale header cannot describe a different table.
VERSIONS = ("2.34.1", "2.45.0", "2.48.0")

# Versions from before ECC-1992, whose `concept_evaluate` counts an array
# condition once rather than once per element.
COUNTS_ARRAYS_ONCE = {"2.34.1"}

# The pools of one eccodes version, each the files eccodes reads together for
# a centre-98 message, local before WMO. A pool none of whose files a version
# has is skipped (`.lte33.def` arrived after 2.34.1).
POOL_FILES = (
    ("shortName.def", ("grib2/localConcepts/ecmf/shortName.def", "grib2/shortName.def")),
    (
        "shortName.lte33.def",
        ("grib2/localConcepts/ecmf/shortName.lte33.def", "grib2/shortName.lte33.def"),
    ),
    (
        "shortName.legacy.def",
        ("grib2/localConcepts/ecmf/shortName.legacy.def", "grib2/shortName.legacy.def"),
    ),
)

OUT = (
    Path(__file__).resolve().parent.parent
    / "crates/fieldglass/src/fetchplan/mars_short_names.rs"
)

# ECMWF open data, every stream and type the page lists (IFS `oper`, `enfo`,
# `wave`, `waef`, and AIFS single and ensemble), deduplicated.
OPEN_DATA = sorted(
    {
        # Pressure levels.
        "z", "t", "u", "v", "q", "w", "vo", "d", "gh", "r", "ws",
        # Soil levels.
        "vsw", "sot",
        # Single level.
        "msl", "sp", "2t", "2d", "skt", "10u", "10v", "100u", "100v", "10fg",
        "mx2t3", "mn2t3", "mx2t6", "mn2t6", "tcc", "hcc", "mcc", "lcc", "tcw",
        "tcwv", "cp", "tp", "tprate", "tciw", "tclw", "tcrw", "sf", "sd", "rsn",
        "asn", "mucape", "ptype", "ro", "ssr", "ssrd", "str", "strd", "ttr",
        "ewss", "nsss", "zos", "sithick", "sve", "svn", "lsm", "sdor", "slor",
        "rowe", "fscov", "10si", "100si",
        # Waves.
        "swh", "mwd", "mwp", "pp1d", "mp2", "cdww", "h1012", "h1214", "h1417",
        "h1721", "h2125", "h2530", "wmb",
        # Ensemble probabilities.
        "tpg1", "tpg5", "tpg10", "tpg20", "tpg25", "tpg50", "tpg100",
        "10fgg10", "10fgg15", "10fgg25", "ptsa_gt_1stdev", "ptsa_gt_1p5stdev",
        "ptsa_gt_2stdev", "ptsa_lt_1stdev", "ptsa_lt_1p5stdev",
        "ptsa_lt_2stdev", "swhg2", "swhg4", "swhg6", "swhg8", "tpl01", "tprl1",
        "tprg3", "tprg5", "2tl273", "10spg10", "10spg15", "mwpg8", "mwpg10",
        "mwpg12", "mwpg15",
    }
)

BLOCK = re.compile(r"^'([^']+)'\s*=\s*\{(.*?)\}", re.S | re.M)
PAIR = re.compile(r"(\w+)\s*=\s*([^;]+);")

TRIPLE = ("discipline", "parameterCategory", "parameterNumber")

# The eccodes keys kept, each with the `Key` variant `verify_message` reads it
# by, in the order a block lists them.
KEPT = {
    "discipline": "Discipline",
    "parameterCategory": "Category",
    "parameterNumber": "Number",
    "productDefinitionTemplateNumber": "ProductTemplate",
    "typeOfFirstFixedSurface": "FirstSurfaceType",
    "scaleFactorOfFirstFixedSurface": "FirstScaleFactor",
    "scaledValueOfFirstFixedSurface": "FirstScaledValue",
    "typeOfSecondFixedSurface": "SecondSurfaceType",
    "scaleFactorOfSecondFixedSurface": "SecondScaleFactor",
    "scaledValueOfSecondFixedSurface": "SecondScaledValue",
    "typeOfStatisticalProcessing": "StatisticalProcess",
    "indicatorOfUnitForTimeRange": "TimeRangeUnit",
    "lengthOfTimeRange": "TimeRangeLength",
    "probabilityType": "ProbabilityType",
    "scaleFactorOfLowerLimit": "LowerLimitScaleFactor",
    "scaledValueOfLowerLimit": "LowerLimitScaledValue",
}

# A `missing()` value, as the table writes it.
MISSING = "missing()"


def parse(path: Path) -> list[tuple[str, dict[str, str]]]:
    """Every block of one concept file, in file order."""
    text = path.read_text(encoding="utf-8")
    # Comments are whole lines starting with `#`.
    text = "\n".join(l for l in text.splitlines() if not l.lstrip().startswith("#"))
    blocks = []
    for name, body in BLOCK.findall(text):
        keys = {k: v.strip() for k, v in PAIR.findall(body)}
        blocks.append((name, keys))
    return blocks


def integer(value: str | None) -> int | None:
    """A key's value as an integer, or `None` for `missing()` and the like."""
    if value is None:
        return None
    try:
        return int(value)
    except ValueError:
        return None


def conditions(keys: dict[str, str], per_element: bool) -> int:
    """How many conditions eccodes counts for a block: one per key, and, where
    `per_element` (ECC-1992), one per element of an array value."""
    count = 0
    for value in keys.values():
        if per_element and value.startswith("["):
            count += len([v for v in re.split(r"[\s,]+", value.strip("[]")) if v])
        else:
            count += 1
    return count


def row(keys: dict[str, str], version: str) -> tuple | None:
    """The kept part of one block, or `None` when its codes do not read.

    `(conditions read, conditions stated)`, where each read condition is a
    `(Key variant, value)` pair and the value is an integer or `None` for
    `missing()`. A kept key whose value is neither (an array, an expression)
    is left unread, so it is counted and not compared.
    """
    codes = [integer(keys.get(k)) for k in TRIPLE]
    if any(c is None or not 0 <= c <= 255 for c in codes):
        return None
    read = []
    for key, value in keys.items():
        variant = KEPT.get(key)
        if variant is None:
            continue
        if value == MISSING:
            read.append((variant, None))
        elif (number := integer(value)) is not None and -(2**31) <= number < 2**31:
            read.append((variant, number))
    return (tuple(read), conditions(keys, version not in COUNTS_ARRAYS_ONCE))


def rust_block(read: tuple, stated: int, mask: int, width: int) -> str:
    conds = ", ".join(
        f"(Key::{k}, {'None' if v is None else f'Some({v})'})" for k, v in read
    )
    return f"Block {{ read: &[{conds}], conditions: {stated}, pools: 0b{mask:0{width}b} }}"


def render(
    table: dict[str, list[list]],
    rivals: list[list],
    pools: list[tuple[str, str]],
    missing: list[str],
) -> str:
    entries = []
    for name in sorted(table):
        rows = ", ".join(rust_block(read, stated, mask, len(pools)) for read, stated, mask in table[name])
        entries.append(f'    ("{name}", &[{rows}]),')
    rival_rows = "\n".join(
        f"    {rust_block(read, stated, mask, len(pools))}," for read, stated, mask in rivals
    )
    blocks = sum(len(v) for v in table.values())
    left_out = (
        "\n//! Listed but not defined by this eccodes, so not compared: "
        + ", ".join(f"`{m}`" for m in missing)
        + "."
        if missing
        else ""
    )
    pool_docs = "\n".join(
        f"/// * bit {i}: eccodes {version} `{kind}`" for i, (version, kind) in enumerate(pools)
    )
    variants = "\n".join(
        f"    /// `{key}`.\n    {variant},"
        for key, variant in KEPT.items()
    )
    return f'''//! MARS short names of ECMWF's open-data parameters — generated, do not edit.
//!
//! Do not edit: regenerate with `python3 tools/gen_mars_short_names.py`.
//! Source: eccodes {" and ".join(VERSIONS)}, their `shortName` concept for an
//! ECMWF GRIB2 message (`definitions/grib2/localConcepts/ecmf/` and
//! `definitions/grib2/`; Apache-2.0, factual data from ECMWF's parameter
//! database).
//!
//! {len(table)} names, {blocks} blocks, and {len(rivals)} blocks of other names
//! that can outrank them. Each block keeps the conditions the message identity
//! carries and counts every condition eccodes ranks it by; see the
//! generator's module docs for which and why.{left_out}

/// An eccodes key this table keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {{
{variants}
}}

/// One way a MARS short name is encoded: an eccodes concept block for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Block {{
    /// The conditions this table compares, each a key and the value the block
    /// requires, `None` for `missing()`.
    pub read: &'static [(Key, Option<i32>)],
    /// How many conditions the eccodes block states, read here or not: what
    /// eccodes ranks matching blocks by.
    pub conditions: u8,
    /// The pools that hold the block, a bit each (see [`POOLS`]).
    pub pools: u8,
}}

/// How many pools there are: the sets of concept files eccodes ranks blocks
/// within.
///
{pool_docs}
pub(crate) const POOLS: u32 = {len(pools)};

/// Every name the table carries, sorted, with its blocks in eccodes' lookup
/// order.
pub(crate) static NAMES: &[(&str, &[Block])] = &[
{chr(10).join(entries)}
];

/// Blocks of names not in [`NAMES`] that share parameter codes with a block of
/// one there, in the same pool, and could outrank it: compared only as
/// rivals, never as the name a record gives.
pub(crate) static RIVALS: &[Block] = &[
{rival_rows}
];

/// The blocks that define `name`, or `None` for a name this table does not
/// carry.
pub(crate) fn blocks(name: &str) -> Option<&'static [Block]> {{
    NAMES
        .binary_search_by(|(n, _)| (*n).cmp(name))
        .ok()
        .map(|i| NAMES[i].1)
}}
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--definitions",
        action="append",
        required=True,
        metavar="VERSION=DIR",
        help="an eccodes definitions directory and its version; once per version",
    )
    args = parser.parse_args()

    roots: dict[str, Path] = {}
    for spec in args.definitions:
        version, _, directory = spec.partition("=")
        if not directory:
            raise SystemExit(f"--definitions {spec!r}: expected VERSION=DIR")
        roots[version] = Path(directory)
    if tuple(sorted(roots)) != tuple(sorted(VERSIONS)):
        raise SystemExit(f"read exactly eccodes {', '.join(VERSIONS)}; got {', '.join(roots)}")

    wanted = set(OPEN_DATA)
    # Every block of every pool: (pool bit, name, codes, kept), file order.
    everything: list[tuple[int, str, tuple, tuple]] = []
    pools: list[tuple[str, str]] = []
    for version in VERSIONS:
        if not any((roots[version] / f).exists() for _, files in POOL_FILES for f in files):
            raise SystemExit(f"{roots[version]}: no shortName concept file found")
        for kind, files in POOL_FILES:
            paths = [roots[version] / f for f in files if (roots[version] / f).exists()]
            if not paths:
                continue
            bit = 1 << len(pools)
            pools.append((version, kind))
            for path in paths:
                for name, keys in parse(path):
                    kept = row(keys, version)
                    if kept is None:
                        if name in wanted:
                            raise SystemExit(
                                f"{path}: '{name}' has codes that do not read: {keys}"
                            )
                        continue
                    codes = tuple(integer(keys[k]) for k in TRIPLE)
                    everything.append((bit, name, codes, kept))

    def merge(rows: list[list], kept: tuple, bit: int) -> None:
        # Two blocks that differ only in unread keys keep one row.
        for existing in rows:
            if (existing[0], existing[1]) == kept:
                existing[2] |= bit
                return
        rows.append([kept[0], kept[1], bit])

    # name -> [[read, stated, pool mask]], first appearance first.
    table: dict[str, list[list]] = {}
    # The fewest conditions a listed name's block states, per pool and codes.
    fewest: dict[tuple[int, tuple], int] = {}
    for bit, name, codes, kept in everything:
        if name in wanted:
            merge(table.setdefault(name, []), kept, bit)
            fewest[(bit, codes)] = min(fewest.get((bit, codes), kept[1]), kept[1])
    # [[read, stated, pool mask]] of the blocks of other names that can
    # outrank a listed name's, and can be certain to match (module docs).
    rivals: list[list] = []
    for bit, name, codes, (read, stated) in everything:
        if (
            name not in wanted
            and stated > fewest.get((bit, codes), 255)
            and len(read) == stated
        ):
            merge(rivals, (read, stated), bit)

    every_row = [r for rows in table.values() for r in rows] + rivals
    used = {k for read, _, _ in every_row for k, _ in read}
    unused = [key for key, variant in KEPT.items() if variant not in used]
    if unused:
        # A `Key` no block reads would be dead code in the crate.
        raise SystemExit(f"no block reads {', '.join(unused)}: drop it from KEPT")
    if len(pools) > 8:
        raise SystemExit(f"{len(pools)} pools do not fit the table's u8 mask")
    missing = sorted(wanted - table.keys())
    for name in missing:
        print(f"not defined by any eccodes read: {name}", file=sys.stderr)

    text = render(table, rivals, pools, missing)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(
        f"wrote {OUT}: {len(table)} names, {len(rivals)} rival blocks, {len(pools)} pools",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
