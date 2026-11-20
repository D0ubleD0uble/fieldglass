#!/usr/bin/env python3
"""Fail when the workflows that build the shipped wasm module pin it differently.

Also holds the size gate's Rust (`RUSTUP_TOOLCHAIN`, ci.yml) to the release's
(release.yml): both must be set, exact versions, and equal (#1811). And it holds
every Rust job in release.yml to that one pin (#1877): the pin is set for the
whole workflow, and every `toolchain:` input reads it, so the native modules and
the crates are built by the same recorded release as the wasm package, not by
whatever `stable` is on the day of the tag. The rest of this docstring is about
binaryen.

    python3 tools/check_binaryen_pin.py

`wasm-opt` decides the bytes of the shipped WebAssembly module. `ci.yml` runs
it to produce the bundle the size gate measures against the README table
(#462); `release.yml` runs it to produce the module inside the npm package
(#466); `perf.yml` runs it to produce the bundles whose linear memory the
performance gate records (#743). Those are three files with the same pin written
three times, which is a pin waiting to drift.

**What drift would cost.** Nothing loud. A release built with a different
optimizer produces a module the size gate never saw — so the figure the README
documents, and that a maintainer reads as the download size, describes a build
nobody shipped. Every check would still pass: the gate compares CI's build to
the README and never looks at the release's, and the release never looks at the
README. The two would simply stop being the same artifact, quietly, and the
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

# The files that install binaryen. Listed rather than globbed: a new
# workflow that pins it should be added here deliberately, and a glob would let
# one that pins it *differently* pass by never being compared.
PINNING_WORKFLOWS = ("ci.yml", "perf.yml", "release.yml")

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


# The two jobs that must build with the same Rust (#1749, #1811): ci.yml's `wasm`
# job, whose size gate measures the module, and release.yml's `publish-wasm-npm`
# job, which ships it and reads the workflow-wide pin. perf.yml and fieldglass-perf's rust-toolchain.toml pin
# 1.98.0 on purpose (nothing from them ships) and semver.yml pins its own
# compiler for rustdoc JSON, so none of those is compared.
TOOLCHAIN_WORKFLOWS = ("ci.yml", "release.yml")
TOOLCHAIN_KEY = "RUSTUP_TOOLCHAIN"
EXACT_VERSION = re.compile(r"\d+\.\d+\.\d+")


def toolchain_values(text: str) -> list[str]:
    """Every `RUSTUP_TOOLCHAIN: value` in this workflow, unquoted, in order.

    Tolerates single or double quotes and a trailing `# comment`. A commented-out
    line is not a pin.
    """
    values = []
    for raw in re.findall(rf"^\s*{TOOLCHAIN_KEY}:[ \t]*(.*?)\s*$", text, re.MULTILINE):
        value = re.sub(r"\s+#.*$", "", raw).strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
            value = value[1:-1].strip()
        values.append(value)
    return values


def toolchain_problems(texts: dict[str, str]) -> list[str]:
    """Why these workflows do not pin one exact Rust release, or `[]` if they do.

    A missing pin is a problem, not agreement, and so is a channel name such as
    `stable`: the size gate would then measure whatever the day's compiler is.
    """
    problems: list[str] = []
    pinned: dict[str, str] = {}
    for name, text in texts.items():
        values = toolchain_values(text)
        if not values:
            problems.append(f"{name}: {TOOLCHAIN_KEY} is not set")
            continue
        if len(set(values)) > 1:
            problems.append(
                f"{name}: {TOOLCHAIN_KEY} is set to more than one value: {sorted(set(values))}"
            )
            continue
        if not EXACT_VERSION.fullmatch(values[0]):
            problems.append(
                f"{name}: {TOOLCHAIN_KEY} is {values[0]!r}, not an exact version such as 1.99.1"
            )
            continue
        pinned[name] = values[0]
    if len(set(pinned.values())) > 1:
        detail = ", ".join(f"{n}={v}" for n, v in pinned.items())
        problems.append(
            f"{TOOLCHAIN_KEY} disagrees between the workflows ({detail}); bump them together"
        )
    return problems


# The workflow whose every Rust job must build with the pin (#1877).
RELEASE_WORKFLOW = "release.yml"
PINNED_TOOLCHAIN_INPUT = "${{ env.RUSTUP_TOOLCHAIN }}"


def workflow_env_keys(text: str) -> list[str]:
    """The keys of the workflow-level `env:` block, the one at column 0.

    A job's or a step's `env:` is indented, so it is not read: a pin set on one
    job leaves the others on whatever the runner has.
    """
    keys: list[str] = []
    inside = False
    for line in text.splitlines():
        if not inside:
            inside = line.rstrip() == "env:"
            continue
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if not line.startswith(" "):
            break
        match = re.match(r"^  ([A-Za-z_][A-Za-z0-9_]*):", line)
        if match:
            keys.append(match.group(1))
    return keys


def toolchain_inputs(text: str) -> list[str]:
    """Every `toolchain:` input in this workflow, unquoted, in order.

    That is the input of `dtolnay/rust-toolchain`, which installs a toolchain
    and makes it the default. A commented-out line is not read.
    """
    values = []
    for raw in re.findall(r"^\s*toolchain:[ \t]*(.*?)\s*$", text, re.MULTILINE):
        value = re.sub(r"\s+#.*$", "", raw).strip()
        if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
            value = value[1:-1].strip()
        values.append(value)
    return values


def release_toolchain_problems(text: str) -> list[str]:
    """Why release.yml does not build every Rust job with its pin, or `[]`.

    `toolchain_problems` already holds the value to an exact version equal to
    ci.yml's. This adds where it applies: workflow-wide, and in every
    `toolchain:` input. A job left on `toolchain: stable` would install the
    day's compiler and make it the default, and only `RUSTUP_TOOLCHAIN` in the
    environment keeps cargo on the pin, so both are required.
    """
    problems: list[str] = []
    if TOOLCHAIN_KEY not in workflow_env_keys(text):
        problems.append(
            f"{RELEASE_WORKFLOW}: {TOOLCHAIN_KEY} is not set in the workflow-level env:, "
            f"so a job without its own would build with whatever Rust the runner has"
        )
    inputs = toolchain_inputs(text)
    if not inputs:
        problems.append(f"{RELEASE_WORKFLOW}: no toolchain: input found; the checker cannot see the Rust setup")
    for value in inputs:
        if value != PINNED_TOOLCHAIN_INPUT:
            problems.append(
                f"{RELEASE_WORKFLOW}: a toolchain: input is {value!r}, "
                f"not {PINNED_TOOLCHAIN_INPUT!r}"
            )
    return problems


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
                f"{key} is not set in: {', '.join(missing)} — every one of these workflows "
                f"builds the module the size gate, the performance gate and the npm package "
                f"share, so each must pin it",
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

    toolchain_texts = {}
    for name in TOOLCHAIN_WORKFLOWS:
        path = WORKFLOWS / name
        if not path.exists():
            print(f"missing workflow: {path}", file=sys.stderr)
            return 1
        toolchain_texts[name] = path.read_text(encoding="utf-8")
    problems = toolchain_problems(toolchain_texts)
    problems += release_toolchain_problems(toolchain_texts[RELEASE_WORKFLOW])
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1

    version = pins[PINNING_WORKFLOWS[0]]["BINARYEN_VERSION"]
    print(f"binaryen pin OK — {', '.join(PINNING_WORKFLOWS)} agree on {version}")
    rust = toolchain_values(toolchain_texts[TOOLCHAIN_WORKFLOWS[0]])[0]
    print(f"rust toolchain pin OK — {', '.join(TOOLCHAIN_WORKFLOWS)} agree on {rust}")
    count = len(toolchain_inputs(toolchain_texts[RELEASE_WORKFLOW]))
    print(f"release toolchain OK — {RELEASE_WORKFLOW} sets it workflow-wide and all {count} Rust setups read it")
    return 0


if __name__ == "__main__":
    sys.exit(main())
