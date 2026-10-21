#!/usr/bin/env python3
"""Generate the MARS short name table `verify_message` checks ECMWF records with.

Regenerate, naming each eccodes definitions tree read and its version:

    python3 tools/gen_mars_short_names.py \
        --definitions 2.34.1=/usr/share/eccodes/definitions \
        --definitions 2.45.0=<eccodes 2.45.0 source>/definitions \
        && cargo fmt

Writes `crates/fieldglass/src/fetchplan/mars_short_names.rs`.

# What it is for

An ECMWF `.index` names each message's parameter by its MARS short name
(`"param": "mx2t3"`), not by GRIB2 codes. `fieldglass::fetchplan::
verify_message` compares the name with the decoded message (#1116), and this
table is how: for each name, the eccodes concept blocks that define it.

# Source

eccodes' `shortName` concept, the files eccodes consults for an ECMWF
(centre 98) GRIB2 message: `definitions/grib2/localConcepts/ecmf/` and then
`definitions/grib2/`, each `shortName.def` with its `.legacy.def` and, from
eccodes 2.38 on, `.lte33.def` (the encodings of GRIB2 tables version 33 and
older, which the committed ECMWF messages, at version 32, are). Apache-2.0;
the definitions are factual data from ECMWF's parameter database.

Two eccodes versions, and the union of their blocks:

* **2.34.1**, the version the rest of the repo validates against
  (`gen_localconcepts_tables.py` reads the same install).
* **2.45.0**, because ECMWF's newer open-data products postdate 2.34.1. Its
  definitions give `2tl273`, `tpl01`, `tprg3` and five other AIFS ensemble
  probabilities a WMO encoding 2.34.1 does not know, only an older local one.
  A table from 2.34.1 alone would refuse those records when they are right.

The union keeps an encoding either version defines, so it accepts what either
eccodes would name the parameter. Bump either deliberately and re-read the
diff.

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

and eccodes names a message after a block when every key in it matches. One
name can have several blocks (older and newer encodings of one parameter), and
a message carrying the name may match any of them.

# What is kept from a block

The parameter codes, and the two keys that tell apart parameters sharing them
and that `MessageIdentity` carries as written: the statistical process (Code
Table 4.10) and the length of its time range with its unit. `2t`, `mx2t3`,
`mn2t3`, `mx2t6` and `mn2t6` are all discipline 0, category 0, number 0; the
process and the length are what separate them.

Every other key (the fixed surfaces, a chemical constituent, a probability
limit, a product template) is dropped. Dropping a key widens what the block
accepts, never narrows it, so a message the full block would match is still
accepted: the table can fail to refuse a wrong record, but never refuses a
right one. The level is compared on its own, from the `levtype`/`levelist`
the record states.

# Which names

The parameters ECMWF publishes as open data, listed in `OPEN_DATA` below from
<https://www.ecmwf.int/en/forecasts/datasets/open-data> (read 2026-10-20):
IFS and AIFS, deterministic and ensemble, atmosphere and wave. The whole
concept is about 4,000 names, most of them never in an open-data `.index`, and
the table ships in the browser module, so it carries only these. A name not in
the table is not compared, which is what an ECMWF record got before.

A listed name no eccodes read defines is reported and left out, for the same
reason.
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# The trees the committed table was built from. Regenerating from others is
# refused, so a stale header cannot describe a different table.
VERSIONS = ("2.34.1", "2.45.0")

# eccodes' own lookup order for a centre-98 message: the local files, then the
# WMO master files. A file a version does not have is skipped (`.lte33.def`
# arrived after 2.34.1).
CONCEPT_FILES = (
    "grib2/localConcepts/ecmf/shortName.def",
    "grib2/localConcepts/ecmf/shortName.lte33.def",
    "grib2/localConcepts/ecmf/shortName.legacy.def",
    "grib2/shortName.def",
    "grib2/shortName.lte33.def",
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


def row(keys: dict[str, str]) -> tuple | None:
    """The kept part of one block, or `None` when its codes do not read."""
    codes = [integer(keys.get(k)) for k in TRIPLE]
    if any(c is None or not 0 <= c <= 255 for c in codes):
        return None
    process = integer(keys.get("typeOfStatisticalProcessing"))
    length = integer(keys.get("lengthOfTimeRange"))
    unit = integer(keys.get("indicatorOfUnitForTimeRange"))
    # A length is only a length with its unit.
    time_range = (unit, length) if unit is not None and length is not None else None
    return (*codes, process, time_range)


def rust(value) -> str:
    return "None" if value is None else f"Some({value})"


def render(table: dict[str, list[tuple]], missing: list[str]) -> str:
    arms = []
    for name in sorted(table):
        rows = ", ".join(
            "Block { "
            f"discipline: {d}, category: {c}, number: {n}, "
            f"statistical_process: {rust(p)}, "
            f"time_range: {'None' if t is None else f'Some(({t[0]}, {t[1]}))'} }}"
            for d, c, n, p, t in table[name]
        )
        arms.append(f'        "{name}" => &[{rows}],')
    blocks = sum(len(v) for v in table.values())
    left_out = (
        "\n//! Listed but not defined by this eccodes, so not compared: "
        + ", ".join(f"`{m}`" for m in missing)
        + "."
        if missing
        else ""
    )
    return f'''//! MARS short names of ECMWF's open-data parameters — generated, do not edit.
//!
//! Do not edit: regenerate with `python3 tools/gen_mars_short_names.py`.
//! Source: eccodes {" and ".join(VERSIONS)}, the union of their `shortName`
//! concept for an ECMWF GRIB2 message (`definitions/grib2/localConcepts/ecmf/`
//! and `definitions/grib2/`; Apache-2.0, factual data from ECMWF's parameter
//! database).
//!
//! {len(table)} names, {blocks} blocks. Each block keeps the parameter codes, the
//! statistical process and the time range's length; every other key of the
//! eccodes block is dropped, which widens what it accepts. See the
//! generator's module docs for why.{left_out}

/// One way a MARS short name is encoded: what an eccodes concept block for it
/// requires of a message, as far as this table keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Block {{
    /// Discipline (Code Table 0.0).
    pub discipline: u8,
    /// Parameter category (Code Table 4.1).
    pub category: u8,
    /// Parameter number (Code Table 4.2).
    pub number: u8,
    /// The statistical process (Code Table 4.10), when the block states one.
    pub statistical_process: Option<u8>,
    /// The time range as `(unit, length)` (Code Table 4.4), when the block
    /// states one.
    pub time_range: Option<(u8, u32)>,
}}

/// The blocks that define `name`, in eccodes' lookup order, or `None` for a
/// name this table does not carry.
pub(crate) fn blocks(name: &str) -> Option<&'static [Block]> {{
    Some(match name {{
{chr(10).join(arms)}
        _ => return None,
    }})
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
    table: dict[str, list[tuple]] = {}
    for version in VERSIONS:
        read = 0
        for rel in CONCEPT_FILES:
            path = roots[version] / rel
            if not path.exists():
                continue
            read += 1
            for name, keys in parse(path):
                if name not in wanted:
                    continue
                kept = row(keys)
                if kept is None:
                    raise SystemExit(f"{path}: '{name}' has codes that do not read: {keys}")
                rows = table.setdefault(name, [])
                # Two blocks that differ only in dropped keys keep one row.
                if kept not in rows:
                    rows.append(kept)
        if read == 0:
            raise SystemExit(f"{roots[version]}: no shortName concept file found")
    missing = sorted(wanted - table.keys())
    for name in missing:
        print(f"not defined by any eccodes read: {name}", file=sys.stderr)

    text = render(table, missing)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(text, encoding="utf-8")
    print(f"wrote {OUT}: {len(table)} names", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
