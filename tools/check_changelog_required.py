#!/usr/bin/env python3
"""Fail a pull request that changes what ships without a CHANGELOG entry.

    python3 tools/check_changelog_required.py --base REF --head REF \\
        [--author LOGIN] [--labels JSON-LIST] [--root DIR]

A pull request that touches a shipped file must change the words of
CHANGELOG.md's `## [Unreleased]` section, or carry the `no-changelog` label.
A pull request opened by Dependabot is exempt: its updates are listed at
release time. (Owner decision on #1873, 2026-11-19.)

**What ships** is `SHIPPED` below, less `UNSHIPPED`: the crates' code and
manifests, the files a crate package carries into its build (`build.rs`, the
host API `schema/`, the `conformance/` data `include_str!` reads), the npm
and napi `package.json` files, and the extension's non-test sources, its
`media/` and its icon. The root `Cargo.toml` counts, since its workspace
version, dependencies and lints are every crate's. Left out: tests, examples,
fuzz crates, READMEs, lockfiles, the root `package.json` (dev tooling only),
and the two crates that never ship, `fieldglass-perf` and
`fieldglass-verify` (both `publish = false`, neither in the .vsix). Some
paths do affect what ships but are deliberately not counted: `Cargo.lock`,
the README and NOTICE in `crates/fieldglass-wasm/npm/`,
`crates/fieldglass-wasm/pack.sh` and `crates/fieldglass-wasm/build.sh`,
`crates/fieldglass-napi/dts-header.d.ts`, `extension/tsconfig.json`,
`extension/.vscodeignore`, the crate READMEs and `release.yml`.

**What counts as changing `[Unreleased]`** is a difference in its words:
both sides are split on whitespace and compared as word lists, so re-wrapping
or re-indenting the section does not count, and neither does an edit to a
released section. A section missing on one side reads as empty.

**Which CHANGELOG is the base.** The one at the merge base of `--base` and
`--head`, as `git diff base...head` reads the changed files. Master gaining an
entry after the branch point is then not mistaken for the PR's own.

The changed files are `git diff --name-only --no-renames`, so a shipped file
renamed away still counts as touching a shipped path.

The labels come in as a JSON list, so a label name with a comma or a space
stays one name. Exit status: 0 pass, 1 the rule fails, 2 bad arguments or a
git error.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_notes import UNRELEASED, section  # noqa: E402

LABEL = "no-changelog"
EXEMPT_AUTHORS = frozenset({"dependabot[bot]"})

# `*` is one path segment; `**` is any number of them, at least one file.
SHIPPED = (
    "Cargo.toml",
    "crates/*/Cargo.toml",
    "crates/*/build.rs",
    "crates/*/src/**",
    "crates/*/schema/**",
    "crates/*/conformance/**",
    "crates/*/package.json",
    "crates/*/npm/package.json",
    "extension/package.json",
    "extension/src/**",
    "extension/media/**",
    "extension/icon.png",
)

UNSHIPPED_CRATES = ("fieldglass-perf", "fieldglass-verify")
UNSHIPPED = (
    "extension/src/test/**",
    *(f"crates/{crate}/**" for crate in UNSHIPPED_CRATES),
)


def glob_regex(pattern: str) -> re.Pattern[str]:
    """`pattern` as a regex over a repository-relative POSIX path."""
    out = []
    for part in re.split(r"(\*\*|\*)", pattern):
        if part == "**":
            out.append(r".+")
        elif part == "*":
            out.append(r"[^/]+")
        else:
            out.append(re.escape(part))
    return re.compile("".join(out))


_SHIPPED = [glob_regex(p) for p in SHIPPED]
_UNSHIPPED = [glob_regex(p) for p in UNSHIPPED]


def shipped(path: str) -> bool:
    """Whether a change to `path` changes something a release ships."""
    return any(r.fullmatch(path) for r in _SHIPPED) and not any(
        r.fullmatch(path) for r in _UNSHIPPED
    )


def _words(changelog: str) -> list[str]:
    return (section(changelog, UNRELEASED) or "").split()


def unreleased_changed(base: str, head: str) -> bool:
    """Whether the words of `[Unreleased]` differ between the two texts."""
    return _words(base) != _words(head)


def verdict(
    files: list[str], base: str, head: str, labels: list[str], author: str
) -> list[str]:
    """The rule's failures for one pull request; empty when it passes."""
    hits = sorted(f for f in files if shipped(f))
    if not hits or author in EXEMPT_AUTHORS or LABEL in labels:
        return []
    if unreleased_changed(base, head):
        return []
    listed = "\n".join(f"  {f}" for f in hits)
    return [
        "This pull request changes shipped files but not CHANGELOG.md's "
        f"`## [{UNRELEASED}]` section:\n{listed}\n"
        f"Add an entry there (CONTRIBUTING.md says how), or, for a change "
        f"with nothing to tell a user, add the `{LABEL}` label and re-run "
        "this job."
    ]


class GitError(Exception):
    pass


def _git(root: Path, *args: str) -> str:
    done = subprocess.run(
        ["git", "-C", str(root), *args], capture_output=True, text=True, encoding="utf-8"
    )
    if done.returncode != 0:
        raise GitError(f"git {' '.join(args)}: {done.stderr.strip()}")
    return done.stdout


def _changelog_at(root: Path, rev: str) -> str:
    try:
        return _git(root, "show", f"{rev}:CHANGELOG.md")
    except GitError:
        # Absent at that revision. `rev` itself was resolved already.
        return ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--base", required=True, help="the base branch's commit")
    parser.add_argument("--head", required=True, help="the pull request's head commit")
    parser.add_argument("--author", default="", help="the pull request author's login")
    parser.add_argument("--labels", default="[]", help="the PR's label names, a JSON list")
    parser.add_argument("--root", default=".", help="the repository")
    args = parser.parse_args(argv)

    try:
        labels = json.loads(args.labels)
    except json.JSONDecodeError as err:
        print(f"--labels is not JSON: {err}", file=sys.stderr)
        return 2
    if not isinstance(labels, list) or not all(isinstance(x, str) for x in labels):
        print("--labels must be a JSON list of strings", file=sys.stderr)
        return 2

    root = Path(args.root)
    try:
        head = _git(root, "rev-parse", "--verify", f"{args.head}^{{commit}}").strip()
        merge_base = _git(root, "merge-base", args.base, head).strip()
        files = _git(root, "diff", "--name-only", "--no-renames", merge_base, head).splitlines()
    except GitError as err:
        print(err, file=sys.stderr)
        return 2

    problems = verdict(
        files,
        _changelog_at(root, merge_base),
        _changelog_at(root, head),
        labels=labels,
        author=args.author,
    )
    for problem in problems:
        print(problem, file=sys.stderr)
    if problems:
        return 1
    hits = sum(shipped(f) for f in files)
    if not hits:
        why = "no shipped file changed"
    elif args.author in EXEMPT_AUTHORS:
        why = f"{hits} shipped file(s) changed; {args.author} is exempt"
    elif LABEL in labels:
        why = f"{hits} shipped file(s) changed; labeled {LABEL}"
    else:
        why = f"{hits} shipped file(s) changed, with a CHANGELOG entry"
    print(f"OK: {why}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
