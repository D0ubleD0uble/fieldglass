#!/usr/bin/env python3
"""Fail when the two workflows pin different versions of binaryen.

    python3 tools/check_binaryen_pin.py

`wasm-opt` decides the bytes of the shipped WebAssembly module. `ci.yml` runs
it to produce the bundle the size gate measures against the README table
(#462); `release.yml` runs it to produce the module inside the npm package
(#466). Those are two files with the same pin written twice, which is a pin
waiting to drift.

**What drift would cost.** Nothing loud. A release built with a different
optimiser produces a module the size gate never saw — so the figure the README
documents, and that a maintainer reads as the download size, describes a build
nobody shipped. Every check would still pass: the gate compares CI's build to
the README and never looks at the release's, and the release never looks at the
README. The two would simply stop being the same artefact, quietly, and the
first symptom would be a bug report about a size or a codegen difference nobody
could reproduce.

The alternative to this check is a composite action shared by both workflows.
That is the better factoring and it is deliberately not taken here: it moves the
pin out of the file a reader of either workflow is looking at, and the version
and its checksum sitting inline beside the `curl` is the thing that makes those
steps auditable. Thirty lines of checker buys the safety without the
indirection.

Checks both the version and the SHA-256, because a matching version with a
mismatched checksum is the worse of the two failures: one of the workflows would
be verifying a tarball against the wrong digest and refusing to run at all,
which reads as a network flake.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = REPO_ROOT / ".github" / "workflows"

# The two files that install binaryen. Listed rather than globbed: a new
# workflow that pins it should be added here deliberately, and a glob would let
# one that pins it *differently* pass by never being compared.
PINNING_WORKFLOWS = ("ci.yml", "release.yml")

KEYS = ("BINARYEN_VERSION", "BINARYEN_SHA256")


def pins_from_text(text: str) -> dict[str, str]:
    """Every `BINARYEN_*: value` this workflow sets, by key.

    A plain regex rather than a YAML parse: the values are scalars on their own
    line in both files, and reading them textually keeps this runnable with the
    standard library alone, like the other checkers here.
    """
    found: dict[str, str] = {}
    for key in KEYS:
        matches = re.findall(rf"^\s*{key}:\s*(\S+)\s*$", text, re.MULTILINE)
        if len(set(matches)) > 1:
            raise ValueError(f"{key} is set to more than one value: {sorted(set(matches))}")
        if matches:
            found[key] = matches[0]
    return found


def main() -> int:
    pins: dict[str, dict[str, str]] = {}
    for name in PINNING_WORKFLOWS:
        path = WORKFLOWS / name
        if not path.exists():
            print(f"missing workflow: {path}", file=sys.stderr)
            return 1
        try:
            pins[name] = pins_from_text(path.read_text(encoding="utf-8"))
        except ValueError as e:
            print(f"{name}: {e}", file=sys.stderr)
            return 1

    failed = False
    for key in KEYS:
        values = {name: found.get(key) for name, found in pins.items()}
        missing = [name for name, value in values.items() if value is None]
        if missing:
            print(
                f"{key} is not set in: {', '.join(missing)} — both workflows build the "
                f"module that the size gate and the npm package share, so both must pin it",
                file=sys.stderr,
            )
            failed = True
            continue
        if len(set(values.values())) > 1:
            print(f"{key} disagrees between the workflows:", file=sys.stderr)
            for name, value in values.items():
                print(f"  {name}: {value}", file=sys.stderr)
            failed = True

    if failed:
        return 1

    version = pins[PINNING_WORKFLOWS[0]]["BINARYEN_VERSION"]
    print(f"binaryen pin OK — {', '.join(PINNING_WORKFLOWS)} agree on {version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
