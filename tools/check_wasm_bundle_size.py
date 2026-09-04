#!/usr/bin/env python3
"""Fail when the wasm bundle's size stops matching what its README claims.

    python3 tools/check_wasm_bundle_size.py \\
        --build baseline=crates/fieldglass-wasm/pkg/web/fieldglass_wasm_bg.wasm \\
        --build '+simd128=crates/fieldglass-wasm/pkg/web-simd/fieldglass_wasm_bg.wasm'

A bundle-size gate is the only thing that keeps a wasm target honest: nothing
else fails when the download doubles (#462). The gate is not a separate ceiling
in a config file, though, because a ceiling and a documented figure drift apart —
the ceiling holds, the figure goes stale, and the README ends up quoting a number
from two releases ago. **The README table is the gate.** A change that moves the
bundle by more than the tolerance in either direction fails here until the table
is updated, so the documented size is the measured size by construction.

Both directions on purpose. An increase is the regression the gate exists for; a
decrease that nobody records loses the win, because the next increase is then
measured against a stale, generous number.

The tolerance (5% by default) is set well above toolchain noise — a rustc or
dependency bump moves this by well under a percent — and well below "it grew a
copy of the standard library".

Gzip is measured with Python's `gzip`, not the `gzip` binary: GNU and BSD `gzip`
disagree by a few hundred bytes on the same input, so a shell measurement would
put the gate and a maintainer's own reading of it a nudge apart. `mtime=0` keeps
the header from making the answer depend on the clock.
"""

from __future__ import annotations

import argparse
import gzip
import os
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_README = REPO / "crates" / "fieldglass-wasm" / "README.md"

# The table this checker reads. Anchored on the marker comment rather than on a
# heading, so the section can be reworded and moved without silently detaching
# the gate from the numbers.
TABLE_MARKER = "<!-- checked by tools/check_wasm_bundle_size.py -->"

DEFAULT_TOLERANCE = 0.05


class Failure(Exception):
    """A gate failure, reported to the user rather than as a traceback."""


def gzipped_size(data: bytes) -> int:
    """Bytes on the wire, for a server that serves the module gzipped."""
    return len(gzip.compress(data, compresslevel=9, mtime=0))


def parse_readme_table(readme: Path) -> dict[str, tuple[int, int]]:
    """`{build name: (raw bytes, gzipped bytes)}` as the README records them.

    The table is markdown a human reads, so the cells carry thousands
    separators and the build names carry backticks; both are stripped here.
    """
    text = readme.read_text(encoding="utf-8")
    if TABLE_MARKER not in text:
        raise Failure(
            f"{readme}: no {TABLE_MARKER} marker, so there is no table to check "
            "against. Put it on the line above the bundle-size table."
        )
    after = text.split(TABLE_MARKER, 1)[1]

    rows: dict[str, tuple[int, int]] = {}
    for line in after.splitlines():
        line = line.strip()
        if not line.startswith("|"):
            # The table ends at the first line that is not part of it, so a
            # later table in the README is not swept up by accident.
            if rows:
                break
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 3:
            continue
        name = cells[0].strip("`").strip()
        raw, gz = cells[1].replace(",", ""), cells[2].replace(",", "")
        if not (raw.isdigit() and gz.isdigit()):
            continue  # the header row and the `|---|` rule
        if int(raw) == 0 or int(gz) == 0:
            # `drift` divides by the recorded figure, and a zero would be a
            # traceback where the point is a readable failure. A recorded zero
            # is a broken row anyway.
            raise Failure(f"{readme}: build {name!r} records a size of zero")
        rows[name] = (int(raw), int(gz))

    if not rows:
        raise Failure(
            f"{readme}: the table after {TABLE_MARKER} has no rows whose second "
            "and third cells are byte counts."
        )
    return rows


def parse_build(spec: str) -> tuple[str, Path]:
    """`name=path` — the name has to match a README row."""
    name, _, path = spec.partition("=")
    if not name or not path:
        raise Failure(f"--build wants name=path, got {spec!r}")
    return name, Path(path)


def drift(measured: int, recorded: int) -> float:
    return abs(measured - recorded) / recorded


def check(
    builds: list[tuple[str, Path]], readme: Path, tolerance: float
) -> tuple[list[str], list[str]]:
    """Compare each build against its README row. `(report lines, failures)`."""
    recorded = parse_readme_table(readme)
    report = [
        "| Build | `.wasm` bytes | recorded | gzipped bytes | recorded | drift |",
        "|---|---:|---:|---:|---:|---:|",
    ]
    failures: list[str] = []

    for name, path in builds:
        if not path.is_file():
            # Loudly, not as a zero: a size gate that reports "0 bytes, well
            # under budget" when the build step failed is worse than no gate.
            failures.append(
                f"{name}: {path} does not exist. Build it first: "
                "crates/fieldglass-wasm/build.sh web"
            )
            continue
        if name not in recorded:
            failures.append(
                f"{name}: no row named {name!r} in {readme}. The README rows are: "
                f"{', '.join(sorted(recorded))}"
            )
            continue

        data = path.read_bytes()
        raw, gz = len(data), gzipped_size(data)
        want_raw, want_gz = recorded[name]
        raw_drift, gz_drift = drift(raw, want_raw), drift(gz, want_gz)
        worst = max(raw_drift, gz_drift)
        report.append(
            f"| {name} | {raw:,} | {want_raw:,} | {gz:,} | {want_gz:,} | {worst * 100:.1f}% |"
        )
        if worst > tolerance:
            failures.append(
                f"{name}: measured {raw:,} bytes ({gz:,} gzipped), README records "
                f"{want_raw:,} ({want_gz:,} gzipped) — {worst * 100:.1f}% off, over the "
                f"{tolerance * 100:.0f}% tolerance. Either this change should not have "
                f"moved the bundle, or {readme.name}'s table needs these numbers."
            )

    if not builds:
        failures.append("no --build was given, so nothing was measured")
    return report, failures


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--build",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="a `.wasm` to measure, named as its README row is (repeatable)",
    )
    parser.add_argument("--readme", type=Path, default=DEFAULT_README)
    parser.add_argument(
        "--tolerance",
        type=float,
        default=DEFAULT_TOLERANCE,
        help=f"fractional drift allowed either way (default {DEFAULT_TOLERANCE})",
    )
    args = parser.parse_args(argv)

    try:
        builds = [parse_build(spec) for spec in args.build]
        report, failures = check(builds, args.readme, args.tolerance)
    except Failure as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1

    text = "\n".join(["### wasm bundle size", "", *report])
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as handle:
            handle.write(f"{text}\n\n")

    for failure in failures:
        print(f"error: {failure}", file=sys.stderr)
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
