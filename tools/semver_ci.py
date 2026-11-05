#!/usr/bin/env python3
"""The two decisions behind the *Public API* check that a PR must not change.

    python3 -I tools/semver_ci.py guard
    python3 -I tools/semver_ci.py baseline

Always run it with `-I`. Without it Python puts `tools/` first on the module
path, so a PR could add an unguarded `tools/json.py` and change what this
script does without touching it (#1699 review).

**guard** (`.github/workflows/semver-guard.yml`, #1699). `semver.yml` runs on
`pull_request`, so GitHub runs the PR's own copy of it, and a PR could edit
the check to pass. The guard runs on `pull_request_target` instead: GitHub
takes the workflow and this script from the base branch, so a PR cannot
change the copy that judges it. It fails a PR that touches one of `GUARDED`
unless the PR carries the `semver-change` label. A rename or deletion of a
guarded file counts. The label does not make a change safe; it makes it
visible in review. Reads `GH_TOKEN`, `REPO`, `PR_NUMBER`, `CHANGED_FILES` (the
event's count, required) and `LABELS` (a JSON list) from the environment, and lists the
PR's files through the API, every page. The API lists at most 3,000 files; a
PR with more than it returned fails closed.

**baseline** (`semver.yml`). Prints the tag the check measures against. That
is the latest `vX.Y.Z` tag, unless `tools/semver-baseline` exists, in which
case it is the tag named there. The file is for the one patch that fixes a
shipped break (RELEASING.md "Breaking changes", ADR-0018 §5). It must name one
existing `vX.Y.Z` tag; anything else is an error, never a quiet fallback to
the latest tag. The tag goes to stdout, an explanation to stderr.

Both are tested by `tools/test_semver_ci.py`, which `semver.yml` runs in a
job of its own.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from collections.abc import Callable, Iterable
from pathlib import Path

LABEL = "semver-change"

# Every file that decides what the Public API check measures, or that judges
# a change to one of them. This script is here because both workflows run it.
GUARDED = (
    ".github/workflows/semver.yml",
    ".github/workflows/semver-guard.yml",
    "tools/semver-release-type",
    "tools/semver-baseline",
    "tools/semver_ci.py",
)

BASELINE_FILE = "tools/semver-baseline"
TAG = re.compile(r"^v(\d+)\.(\d+)\.(\d+)$")

Runner = Callable[[list[str]], str]


def run(cmd: list[str]) -> str:
    """Run a command and return its stdout; raise on a non-zero exit."""
    return subprocess.run(cmd, check=True, capture_output=True, text=True, encoding="utf-8").stdout


# ---------------------------------------------------------------- guard


def fetch_pr_files(repo: str, number: str, runner: Runner = run) -> list[dict]:
    """Every file of a pull request, from every page of the API's listing."""
    out = runner(
        [
            "gh",
            "api",
            "--paginate",
            "--slurp",
            f"repos/{repo}/pulls/{number}/files?per_page=100",
        ]
    )
    pages = json.loads(out)
    if not isinstance(pages, list) or not all(isinstance(p, list) for p in pages):
        raise ValueError("the API did not return a list of pages of files")
    files = [entry for page in pages for entry in page]
    if not all(isinstance(e, dict) for e in files):
        raise ValueError("the API returned a file entry that is not an object")
    return files


def guarded_paths(files: Iterable[dict]) -> list[str]:
    """The guarded paths a PR touches, sorted. A rename counts from either end."""
    touched = set()
    for entry in files:
        for key in ("filename", "previous_filename"):
            path = entry.get(key)
            if path in GUARDED:
                touched.add(path)
    return sorted(touched)


def guard(
    files: list[dict], labels: list[str], expected: int | None
) -> tuple[bool, list[str]]:
    """Decide the guard. Returns (passes, lines to print)."""
    labeled = LABEL in labels
    touched = guarded_paths(files)
    truncated = expected is not None and len(files) < expected
    lines = [f"The PR changes {len(files)} file(s)."]
    if truncated:
        lines.append(
            f"The API listed {len(files)} of the PR's {expected} files, so a "
            "guarded file may be among the rest."
        )
    if touched:
        lines.append("Guarded files it touches: " + ", ".join(touched))
    if not touched and not truncated:
        lines.append("It touches no guarded file.")
        return True, lines
    if labeled:
        lines.append(
            f"::warning::This PR carries the '{LABEL}' label, so the guard "
            "passes. Review the change to the Public API check itself."
        )
        return True, lines
    lines.append(
        "::error::This PR changes how the Public API check works, or may: "
        + ", ".join(touched or ["(files the API did not list)"])
        + f". That needs the '{LABEL}' label, which is the owner's call. "
        "Add the label to re-run this check, or drop the change. See "
        "RELEASING.md \"Breaking changes\"."
    )
    return False, lines


def guard_main(env: dict[str, str], runner: Runner = run) -> int:
    try:
        repo = env["REPO"]
        number = env["PR_NUMBER"]
        labels = json.loads(env.get("LABELS") or "[]")
        # Without the event's count a truncated listing would go unnoticed,
        # so a missing or non-integer count is an error, not "unknown".
        expected = int(env["CHANGED_FILES"])
        if expected < 0:
            raise ValueError(f"CHANGED_FILES is negative: {expected}")
    except (KeyError, ValueError) as err:
        print(f"::error::semver guard: bad environment: {err}")
        return 1
    if not isinstance(labels, list):
        print("::error::semver guard: LABELS is not a JSON list")
        return 1
    try:
        files = fetch_pr_files(repo, number, runner)
    except (subprocess.CalledProcessError, ValueError, TypeError) as err:
        print(f"::error::semver guard: could not list the PR's files: {err}")
        return 1
    ok, lines = guard(files, labels, expected)
    print("\n".join(lines))
    return 0 if ok else 1


# ------------------------------------------------------------- baseline


class BaselineError(Exception):
    """The baseline cannot be decided; the message says why."""


def read_baseline_file(text: str) -> str:
    """The tag named in `tools/semver-baseline`. `#` lines are comments."""
    names = [
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]
    if len(names) != 1:
        raise BaselineError(
            f"{BASELINE_FILE} must name exactly one tag, found {len(names)} "
            "non-comment lines"
        )
    if not TAG.match(names[0]):
        raise BaselineError(
            f"{BASELINE_FILE} must name a vX.Y.Z tag, got '{names[0]}'"
        )
    return names[0]


def latest_tag(tags: Iterable[str]) -> str | None:
    """The highest vX.Y.Z tag, compared as numbers. Other tags are ignored."""
    versions = []
    for tag in tags:
        m = TAG.match(tag.strip())
        if m:
            versions.append((tuple(int(g) for g in m.groups()), tag.strip()))
    return max(versions)[1] if versions else None


def resolve_baseline(root: Path, runner: Runner = run) -> tuple[str, str]:
    """(tag, where it came from). Raises BaselineError."""
    tags = runner(["git", "-C", str(root), "tag", "--list", "v[0-9]*"]).split()
    path = root / BASELINE_FILE
    if path.exists():
        tag = read_baseline_file(path.read_text(encoding="utf-8"))
        if tag not in tags:
            raise BaselineError(
                f"{BASELINE_FILE} names '{tag}', which is not a tag in this "
                "repository"
            )
        return tag, f"named in {BASELINE_FILE}, in place of the latest tag"
    tag = latest_tag(tags)
    if tag is None:
        raise BaselineError("no vX.Y.Z tag found to use as the baseline")
    return tag, "the latest vX.Y.Z tag"


def baseline_main(root: Path, runner: Runner = run) -> int:
    try:
        tag, source = resolve_baseline(root, runner)
    except BaselineError as err:
        print(f"::error::{err}", file=sys.stderr)
        return 1
    except subprocess.CalledProcessError as err:
        print(f"::error::could not list the tags: {err}", file=sys.stderr)
        return 1
    print(f"Baseline {tag} ({source})", file=sys.stderr)
    print(tag)
    return 0


def main(argv: list[str]) -> int:
    if argv == ["guard"]:
        return guard_main(dict(os.environ))
    if argv == ["baseline"]:
        return baseline_main(Path.cwd())
    print(__doc__.split("\n\n")[1], file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
# smoke test of the semver guard; this PR is closed unmerged
