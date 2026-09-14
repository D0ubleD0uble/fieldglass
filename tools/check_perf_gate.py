#!/usr/bin/env python3
"""Fail when a gated performance number stops matching `docs/performance.md`.

    python3 tools/check_perf_gate.py --measured measured.json \\
        --instructions crates/fieldglass-perf/target/gungraun \\
        --wasm baseline=wasm.json --wasm +simd128=wasm-simd.json
    python3 tools/check_perf_gate.py ... --write      # re-record the tables

The shape is `tools/check_wasm_bundle_size.py`'s, for the reason given there:
**the document is the gate.** A threshold in a config file and a number in a doc
drift apart, so there is only the doc. A change that moves a gated number fails
here until the table is re-recorded, and `--write` is how it is re-recorded — so
the diff of `docs/performance.md` in a pull request is the performance review.

Both directions fail, on purpose. A regression is what the gate exists for; an
improvement nobody records is lost, because the next regression is then measured
against a stale, generous number.

**Tolerances.** The heap and I/O numbers are exact: dhat counts requested sizes
and the recording counts requested ranges, so the same code over the same bytes
gives the same numbers on every machine. Instruction counts and estimated cycles
come from Callgrind and move a little with the toolchain, so they get a relative
tolerance. wasm memory grows in 64 KiB pages and is exact for one build; it gets
one page either way, so a rustc bump that nudges the allocator across a page
boundary is not mistaken for a regression.

**Bounds are reported, not gated.** Every scenario's bound is evaluated and its
gap printed. The bounds section of the doc is derived from the doc's own table
and checked to still be that derivation: the table is what is compared with a
measurement, with each metric's tolerance, and the bounds follow from it. A
scenario that violates its bound today is a finding with its own issue; failing
the build on it would only teach people to edit the bound.

**Nothing is optional by default.** Every tier the table records must be
supplied. `--only` names the tiers a job measures when a workflow splits them —
the wasm tier is measured where the wasm bundles are built — and it is a list a
reviewer can read in the workflow, not a silent skip when an input is missing.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_DOC = REPO / "docs" / "performance.md"

TABLE_BEGIN = "<!-- perf-gate:table:begin — regenerate with tools/check_perf_gate.py --write -->"
TABLE_END = "<!-- perf-gate:table:end -->"
BOUNDS_BEGIN = "<!-- perf-gate:bounds:begin — regenerate with tools/check_perf_gate.py --write -->"
BOUNDS_END = "<!-- perf-gate:bounds:end -->"
DIGEST = re.compile(r"Corpus digest: `([0-9a-f]{64})`")

INSTRUCTION_TOLERANCE = 0.02
WASM_PAGE = 65536

# (key, heading, tier). The order is the table's.
COLUMNS = [
    ("cells", "Cells", "base"),
    ("width", "Value bytes", "base"),
    ("bytes", "Bytes read", "io"),
    ("bound", "Bound", "io"),
    ("requests", "Requests", "io"),
    ("blocks", "Allocations", "heap"),
    ("peak", "Peak heap", "heap"),
    ("instructions", "Instructions", "instructions"),
    ("cycles", "Est. cycles", "instructions"),
    ("wasm", "wasm memory", "wasm"),
    ("wasm_simd", "wasm +simd128 memory", "wasm"),
]
TIERS = ("io", "heap", "instructions", "wasm")
# Columns every run produces, whatever `--only` says: the `measure` binary
# always runs, and the cell count is what the bounds divide by.
ALWAYS = {"base"}
WASM_BUILDS = {"baseline": "wasm", "+simd128": "wasm_simd"}

# Per-cell floors, in bytes, for the operations whose floor does not depend on
# the decoded value width: the least any implementation could hold per cell, and
# why. The decoding operations' floors are in `floor_for`, because they follow
# the width `Dtype::Auto` chose.
FIXED_FLOORS = {
    "warp": (5, "f32 output (4) + mask (1) per output pixel"),
    "render": (4, "one RGBA pixel"),
    "contours": (0, "marching squares needs a row of state, not a cell's"),
}


class Failure(Exception):
    """A gate failure, reported to the user rather than as a traceback."""


# ── Inputs ────────────────────────────────────────────────────────────────────


def load_json(path: Path, what: str) -> dict:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except OSError as exc:
        raise Failure(f"{what}: cannot read {path}: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise Failure(f"{what}: {path} is not JSON: {exc}") from exc


def gungraun_metrics(root: Path, order: list[str]) -> dict[str, dict[str, int]]:
    """`{scenario id: {"instructions", "cycles"}}` from Gungraun's summaries.

    Gungraun names an `iter` benchmark by position, `scenario_N`, so the id is
    `order[N]` from the same catalogue. Every summary must map to an id and every
    id must have a summary: a partial run is a failure, not a smaller table.
    """
    summaries = sorted(root.rglob("summary.json"))
    if not summaries:
        raise Failure(f"no Gungraun summary.json under {root}; did the bench run with --save-summary=json?")
    out: dict[str, dict[str, int]] = {}
    for path in summaries:
        doc = load_json(path, "gungraun summary")
        match = re.fullmatch(r"scenario_(\d+)", str(doc.get("id", "")))
        if not match:
            continue
        index = int(match.group(1))
        if index >= len(order):
            raise Failure(f"{path}: benchmark {doc['id']} is past the catalogue's {len(order)} scenarios — a stale summary from an older corpus?")
        callgrind = doc["profiles"][0]["summaries"]["total"]["summary"]["Callgrind"]
        out[order[index]] = {
            "instructions": metric(callgrind["Ir"]),
            "cycles": metric(callgrind["EstimatedCycles"]),
        }
    missing = [scenario for scenario in order if scenario not in out]
    if missing:
        raise Failure(f"Gungraun has no summary for {len(missing)} scenarios, e.g. {missing[:3]}")
    return out


def metric(entry: dict) -> int:
    """The new side of a Gungraun metric, whichever shape it was saved in."""
    value = entry["metrics"]
    new = value.get("Left") or (value.get("Both") or [None])[0]
    if not new:
        raise Failure(f"a Gungraun metric has no new value: {entry}")
    number = new.get("Int", new.get("Float"))
    return int(round(number))


def measured_rows(
    measured: dict,
    instructions: dict[str, dict[str, int]] | None,
    wasm: dict[str, dict] | None,
) -> dict[str, dict[str, int | None]]:
    """One row per scenario, keyed as the table is."""
    rows: dict[str, dict[str, int | None]] = {}
    for scenario in measured["order"]:
        m = measured["scenarios"][scenario]
        row: dict[str, int | None] = {
            "bytes": m["bytes"] if m["bound_bytes"] is not None else None,
            "bound": m["bound_bytes"],
            "requests": m["requests"] if m["bound_bytes"] is not None else None,
            "blocks": m["blocks"],
            "peak": m["peak"],
            "cells": m["cells"],
            "width": m.get("width"),
        }
        if instructions is not None:
            row.update(instructions[scenario])
        if wasm is not None:
            for column in WASM_BUILDS.values():
                build = wasm.get(column)
                row[column] = build["scenarios"][scenario]["after"] if build and scenario in build["scenarios"] else None
        rows[scenario] = row
    return rows


# ── The table ─────────────────────────────────────────────────────────────────


def fmt(value: int | None) -> str:
    return "—" if value is None else f"{value:,}"


def parse_number(cell: str) -> int | None:
    cell = cell.strip()
    if cell in ("—", ""):
        return None
    digits = cell.replace(",", "")
    if not digits.isdigit():
        raise Failure(f"table cell {cell!r} is not a number")
    return int(digits)


def section(text: str, begin: str, end: str, doc: Path) -> str:
    if begin not in text or end not in text:
        raise Failure(f"{doc}: missing the {begin!r} … {end!r} markers")
    return text.split(begin, 1)[1].split(end, 1)[0]


def render_table(rows: dict[str, dict[str, int | None]]) -> str:
    head = "| Scenario | " + " | ".join(h for _, h, _ in COLUMNS) + " |"
    rule = "|---|" + "---:|" * len(COLUMNS)
    lines = [head, rule]
    for scenario, row in rows.items():
        cells = " | ".join(fmt(row.get(key)) for key, _, _ in COLUMNS)
        lines.append(f"| `{scenario}` | {cells} |")
    return "\n".join(lines)


def parse_table(body: str, doc: Path) -> dict[str, dict[str, int | None]]:
    rows: dict[str, dict[str, int | None]] = {}
    for line in body.splitlines():
        line = line.strip()
        if not line.startswith("| `"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) != len(COLUMNS) + 1:
            raise Failure(f"{doc}: row {cells[0]} has {len(cells) - 1} values, the table has {len(COLUMNS)} columns")
        scenario = cells[0].strip("`")
        if scenario in rows:
            # Otherwise the last copy wins, and a stale first copy passes review.
            raise Failure(f"{doc}: scenario {scenario} has more than one row")
        rows[scenario] = {key: parse_number(cell) for (key, _, _), cell in zip(COLUMNS, cells[1:])}
    if not rows:
        raise Failure(f"{doc}: the gated table has no rows")
    return rows


def compare(
    recorded: dict[str, dict[str, int | None]],
    measured: dict[str, dict[str, int | None]],
    tiers: set[str],
    instruction_tolerance: float,
) -> list[str]:
    failures: list[str] = []
    for scenario in sorted(set(recorded) ^ set(measured)):
        where = "the table but not the measurement" if scenario in recorded else "the measurement but not the table"
        failures.append(f"{scenario}: in {where} — the catalogue changed; re-record with --write")
    for scenario in recorded.keys() & measured.keys():
        for key, heading, tier in COLUMNS:
            if tier not in tiers | ALWAYS:
                continue
            want, got = recorded[scenario][key], measured[scenario].get(key)
            if want is None and got is None:
                continue
            if want is None or got is None:
                failures.append(f"{scenario}: {heading} recorded {fmt(want)}, measured {fmt(got)}")
                continue
            if tier == "instructions":
                ok = abs(got - want) <= instruction_tolerance * max(want, 1)
            elif tier == "wasm":
                ok = abs(got - want) <= WASM_PAGE
            else:
                ok = got == want
            if not ok:
                change = (got - want) / want * 100 if want else float("inf")
                failures.append(f"{scenario}: {heading} {got:,}, recorded {want:,} ({change:+.1f}%)")
    return failures


# ── Bounds ────────────────────────────────────────────────────────────────────


def pair(rows: dict, family_op: str, a: str, b: str) -> tuple[dict, dict] | None:
    family, op = family_op
    first, second = rows.get(f"{family}-{a}/{op}"), rows.get(f"{family}-{b}/{op}")
    return (first, second) if first and second else None


def families(rows: dict) -> list[tuple[str, str]]:
    seen = []
    for scenario in rows:
        name, op = scenario.split("/")
        family = name.rsplit("-", 1)[0]
        if (family, op) not in seen:
            seen.append((family, op))
    return seen


def floor_for(family: str, op: str, row: dict) -> tuple[int, str] | None:
    """The least per-cell heap an operation could hold, and the reason."""
    width = row.get("width")
    if op == "decode" and width:
        return width + 1, f"output value ({width}) + mask byte (1); unpacking writes straight into the output"
    if op in ("slice", "scrub") and width:
        if family == "netcdf-classic":
            return width + 1, f"output value ({width}) + mask (1); the plane is read in place"
        return width + 5, f"output value ({width}) + mask (1) + one decompressed f32 chunk element (4)"
    return FIXED_FLOORS.get(op)


def render_bounds(rows: dict[str, dict[str, int | None]], tiers: set[str]) -> str:
    """The bounds analysis, derived from the gated rows alone."""
    out: list[str] = []

    out += [
        "#### Bytes read",
        "",
        "Each row that reads more than its bound. The bound is stated beside every",
        "scenario in the table above; these are the ones over it.",
        "",
        "| Scenario | Bytes read | Bound | Over by |",
        "|---|---:|---:|---:|",
    ]
    over = [(s, r) for s, r in rows.items() if r["bound"] is not None and r["bytes"] > r["bound"]]
    out += [f"| `{s}` | {r['bytes']:,} | {r['bound']:,} | {r['bytes'] / max(r['bound'], 1):.1f}× |" for s, r in over] or ["| none | | | |"]

    out += [
        "",
        "#### Allocations against cell count",
        "",
        "The same operation at `S` and at `L` (four times the cells). The bound is",
        "that the count does not change.",
        "",
        "| Operation | `S` | `L` | Verdict |",
        "|---|---:|---:|---|",
    ]
    for family, op in families(rows):
        p = pair(rows, (family, op), "S", "L")
        if not p:
            continue
        s, l = p
        verdict = "holds" if s["blocks"] == l["blocks"] else f"grows {l['blocks'] - s['blocks']:+,}"
        out.append(f"| `{family}/{op}` | {s['blocks']:,} | {l['blocks']:,} | {verdict} |")

    out += [
        "",
        "#### Peak heap per cell",
        "",
        "`B` is the slope between `S` and `L`: `(peak(L) − peak(S)) / (cells(L) − cells(S))`,",
        "so a fixed overhead `C` cancels out. The floor is the least any implementation",
        "could hold per cell for that operation.",
        "",
        "| Operation | `B` today | `C` today | Floor `B` | Gap | Floor is |",
        "|---|---:|---:|---:|---:|---|",
    ]
    for family, op in families(rows):
        p = pair(rows, (family, op), "S", "L")
        if not p:
            continue
        s, l = p
        floor = floor_for(family, op, s)
        if floor is None:
            continue
        if l["cells"] == s["cells"]:
            continue
        slope = (l["peak"] - s["peak"]) / (l["cells"] - s["cells"])
        base = s["peak"] - slope * s["cells"]
        gap = f"{slope / floor[0]:.1f}×" if floor[0] else f"+{slope:.1f} B"
        out.append(f"| `{family}/{op}` | {slope:.1f} B | {base:,.0f} B | {floor[0]} B | {gap} | {floor[1]} |")

    out += [
        "",
        "#### Work against the variable, not the plane",
        "",
        "An open, a slice and a scrub at `D` (four times the variable, the same plane)",
        "against `S`. The bound is a ratio of 1: the planes nobody asked for cost nothing.",
        "",
        "| Operation | Peak heap `D`/`S` | Allocations `D`/`S` |"
        + (" Instructions `D`/`S` |" if "instructions" in tiers else ""),
        "|---|---:|---:|" + ("---:|" if "instructions" in tiers else ""),
    ]
    for family, op in families(rows):
        if op not in ("slice", "scrub", "open"):
            continue
        p = pair(rows, (family, op), "S", "D")
        if not p:
            continue
        s, d = p
        line = f"| `{family}/{op}` | {d['peak'] / s['peak']:.2f} | {d['blocks'] / s['blocks']:.2f} |"
        if "instructions" in tiers:
            line += f" {d['instructions'] / s['instructions']:.2f} |"
        out.append(line)

    if "instructions" in tiers:
        out += [
            "",
            "#### Work against cell count",
            "",
            "Instructions at `L` over `S`. Proportional work is a ratio of about 4",
            "(the cell ratio); spectral inputs grow coefficients, not cells.",
            "",
            "| Operation | Instructions `S` | Instructions `L` | `L`/`S` |",
            "|---|---:|---:|---:|",
        ]
        for family, op in families(rows):
            if op not in ("decode", "slice", "scrub", "warp", "render", "contours", "codec"):
                continue
            p = pair(rows, (family, op), "S", "L")
            if not p:
                continue
            s, l = p
            out.append(f"| `{family}/{op}` | {s['instructions']:,} | {l['instructions']:,} | {l['instructions'] / s['instructions']:.2f} |")

        out += [
            "",
            "#### Decode above its codec",
            "",
            "The share of a decode's instructions spent outside the decompressor, over",
            "the same bytes. What is left once the codec's ceiling is taken out.",
            "",
            "| Input | Decode | Codec alone | Outside the codec |",
            "|---|---:|---:|---:|",
        ]
        for scenario, row in rows.items():
            name, op = scenario.split("/")
            if op != "codec":
                continue
            decode = rows.get(f"{name}/decode") or rows.get(f"{name}/slice")
            if not decode:
                continue
            outside = 1 - row["instructions"] / decode["instructions"]
            out.append(f"| `{name}` | {decode['instructions']:,} | {row['instructions']:,} | {outside * 100:.0f}% |")

    return "\n".join(out)


# ── Main ──────────────────────────────────────────────────────────────────────


def splice(text: str, begin: str, end: str, body: str) -> str:
    head, rest = text.split(begin, 1)
    _, tail = rest.split(end, 1)
    return f"{head}{begin}\n{body}\n{end}{tail}"


def run(args: argparse.Namespace) -> tuple[str, list[str]]:
    tiers = set(args.only.split(",")) if args.only else set(TIERS)
    unknown = tiers - set(TIERS)
    if unknown:
        raise Failure(f"--only names unknown tiers {sorted(unknown)}; the tiers are {', '.join(TIERS)}")

    measured = load_json(args.measured, "--measured")
    instructions = None
    if "instructions" in tiers:
        if not args.instructions:
            raise Failure("the instructions tier needs --instructions <gungraun dir> (or leave it out of --only)")
        instructions = gungraun_metrics(args.instructions, measured["order"])
    wasm = None
    if "wasm" in tiers:
        wasm = {}
        for spec in args.wasm:
            label, _, path = spec.partition("=")
            if label not in WASM_BUILDS or not path:
                raise Failure(f"--wasm wants one of {sorted(WASM_BUILDS)}=path, got {spec!r}")
            wasm[WASM_BUILDS[label]] = load_json(Path(path), f"--wasm {label}")
        missing = set(WASM_BUILDS.values()) - set(wasm)
        if missing:
            raise Failure(f"the wasm tier needs both builds; missing {sorted(missing)}")
        for build in wasm.values():
            if build.get("digest") != measured["digest"]:
                raise Failure("a wasm measurement was taken over a different corpus than --measured")

    rows = measured_rows(measured, instructions, wasm)
    text = args.doc.read_text(encoding="utf-8")
    failures: list[str] = []

    if args.write:
        if tiers != set(TIERS):
            raise Failure("--write re-records every tier, so it cannot be combined with --only")
        text = splice(text, TABLE_BEGIN, TABLE_END, render_table(rows))
        text = splice(text, BOUNDS_BEGIN, BOUNDS_END, render_bounds(rows, tiers))
        text = DIGEST.sub(f"Corpus digest: `{measured['digest']}`", text)
        args.doc.write_text(text, encoding="utf-8")
        return f"re-recorded {len(rows)} scenarios in {args.doc}", []

    digest = DIGEST.search(text)
    if not digest:
        raise Failure(f"{args.doc}: no `Corpus digest: ` line")
    if digest.group(1) != measured["digest"]:
        failures.append(
            f"the corpus digest is {measured['digest']}, the doc records {digest.group(1)}: the generated "
            "inputs changed (a generator edit or a writer version), so every number below may move with them"
        )
    recorded = parse_table(section(text, TABLE_BEGIN, TABLE_END, args.doc), args.doc)
    failures += compare(recorded, rows, tiers, args.instruction_tolerance)
    # Derived from the recorded table, not from this measurement: the
    # instruction counts are allowed to wander inside their tolerance, and a
    # ratio of two of them would then never match exactly.
    want = render_bounds(recorded, set(TIERS)).strip()
    have = section(text, BOUNDS_BEGIN, BOUNDS_END, args.doc).strip()
    if want != have:
        failures.append("the bounds section is not what the table implies; re-record with --write")

    report = render_bounds(rows, tiers)
    return report, failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--measured", type=Path, required=True, help="the `measure` binary's JSON")
    parser.add_argument("--instructions", type=Path, help="Gungraun's output directory")
    parser.add_argument("--wasm", action="append", default=[], metavar="BUILD=PATH", help="wasm/measure.mjs JSON, per build")
    parser.add_argument("--doc", type=Path, default=DEFAULT_DOC)
    parser.add_argument("--only", help=f"comma-separated tiers this run measures ({', '.join(TIERS)})")
    parser.add_argument("--instruction-tolerance", type=float, default=INSTRUCTION_TOLERANCE)
    parser.add_argument("--write", action="store_true", help="re-record the doc from these measurements")
    args = parser.parse_args(argv)
    try:
        report, failures = run(args)
    except Failure as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    print(report)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(f"### Performance bounds\n\n{report}\n\n")
    for failure in failures:
        print(f"error: {failure}", file=sys.stderr)
    if failures:
        print(
            f"\n{len(failures)} gated number(s) moved. If the change is meant to move them, "
            "re-record with --write and say why in the pull request.",
            file=sys.stderr,
        )
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
