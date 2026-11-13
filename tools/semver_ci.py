#!/usr/bin/env python3
"""The two decisions behind the *Public API* check that a PR must not change.

    python3 -I tools/semver_ci.py guard
    python3 -I tools/semver_ci.py baseline
    python3 -I tools/semver_ci.py packages <base commit>

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

**packages** (`semver.yml`, #1712). Prints the crates the check must cover,
one per line, and fails when the PR's manifests would weaken the check. The
list is every workspace member the base commit publishes, read from the base
commit's own manifests, not the PR's. The job passes each one to
cargo-semver-checks as `--package` next to `--workspace`. The job fails when
the PR sets a crate on that list to `publish = false`, drops it from the
workspace, renames it, or takes away its library target: the tool skips the
last three without a word, and an un-published crate would drop off the next
PR's base list. Un-publishing or removing a published crate is a release
decision for the owner, and no PR passes with one. It also fails when any
workspace manifest in the PR declares `cargo-semver-checks` metadata
(`[package.metadata.cargo-semver-checks]` or
`[workspace.metadata.cargo-semver-checks]`), which is where the tool reads lint
overrides such as `function_missing = "allow"`, and when any tracked
`Cargo.toml` is a symlink, which could change without the job's path filter
seeing it. Both sides are read with `cargo metadata --no-deps`, the same data
the tool reads; the base tree is checked out by git into a temporary
directory, through a scratch index, and read from there.
Run it from the PR's checkout. The base must be a full commit SHA.

All three are tested by `tools/test_semver_ci.py`, which `semver.yml` runs in
a job of its own.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
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


# ------------------------------------------------------------- packages


# The key cargo-semver-checks reads its configuration from, under
# `[package.metadata]` and `[workspace.metadata]`.
SEMVER_METADATA_KEY = "cargo-semver-checks"
# The target kinds cargo-semver-checks treats as a library it can check.
LIB_KINDS = frozenset({"lib", "rlib", "dylib", "cdylib", "staticlib"})
SHA = re.compile(r"^(?:[0-9a-f]{40}|[0-9a-f]{64})$")
CRATE_NAME = re.compile(r"^[A-Za-z0-9_-]+$")


class PackagesError(Exception):
    """The crate list cannot be decided; the message says why."""


def cargo_metadata(root: Path) -> dict:
    """`cargo metadata --no-deps` for the workspace at `root`, run from there.

    Run from `root` so that cargo reads that tree's `.cargo/config.toml`, not
    the one in the directory the script was started from.
    """
    try:
        out = subprocess.run(
            ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"],
            cwd=root,
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        ).stdout
    except (OSError, subprocess.CalledProcessError) as err:
        detail = getattr(err, "stderr", "") or ""
        raise PackagesError(f"cargo metadata failed in {root}: {err}\n{detail}") from err
    meta = json.loads(out)
    if not isinstance(meta, dict) or not isinstance(meta.get("packages"), list):
        raise PackagesError(f"cargo metadata in {root} did not list packages")
    return meta


def workspace_members(meta: dict) -> dict[str, dict]:
    """The workspace's own packages, by name."""
    ids = set(meta.get("workspace_members") or [])
    return {p["name"]: p for p in meta["packages"] if p.get("id") in ids}


def is_published(package: dict) -> bool:
    """`publish = false` shows as an empty list; anything else may publish."""
    return package.get("publish") != []


def has_lib(package: dict) -> bool:
    return any(
        kind in LIB_KINDS
        for target in package.get("targets") or []
        for kind in target.get("kind") or []
    )


def semver_metadata(meta: dict) -> list[str]:
    """Every place a workspace manifest declares cargo-semver-checks config."""
    found = []
    # `cargo metadata` puts `[workspace.metadata]` at the top level, as
    # `metadata`; the tool reads the same field.
    workspace = meta.get("metadata")
    if isinstance(workspace, dict) and SEMVER_METADATA_KEY in workspace:
        root = meta.get("workspace_root", "the workspace root")
        found.append(f"[workspace.metadata.{SEMVER_METADATA_KEY}] in {root}/Cargo.toml")
    for name, package in sorted(workspace_members(meta).items()):
        metadata = package.get("metadata")
        if isinstance(metadata, dict) and SEMVER_METADATA_KEY in metadata:
            found.append(
                f"[package.metadata.{SEMVER_METADATA_KEY}] in {name} "
                f"({package.get('manifest_path')})"
            )
    return found


def decide_packages(base: dict, head: dict) -> tuple[bool, list[str], list[str]]:
    """Decide the crate list. Returns (passes, crates to check, lines to print).

    `base` and `head` are `cargo metadata` output for the base commit and the
    PR's checkout.
    """
    base_members = workspace_members(base)
    head_members = workspace_members(head)
    crates = sorted(name for name, p in base_members.items() if is_published(p))
    lines = []
    errors = []
    if not crates:
        errors.append(
            "the base commit publishes no workspace crate, so there is nothing "
            "to check against"
        )
    for name in crates:
        if not CRATE_NAME.match(name):
            errors.append(
                f"the base commit has a crate named '{name}', which is not a "
                "plain crate name"
            )
    lines.append("Crates the base commit publishes, all checked: " + ", ".join(crates))
    for name in crates:
        package = head_members.get(name)
        if package is None:
            errors.append(
                f"{name} is published on the base branch but is not a workspace "
                "member in this PR (dropped from `members`, excluded or "
                "renamed), so cargo-semver-checks would skip it"
            )
            continue
        if has_lib(base_members[name]) and not has_lib(package):
            errors.append(
                f"{name} has a library target on the base branch but none in "
                "this PR, so cargo-semver-checks would skip it"
            )
        if not is_published(package):
            # Checked anyway below, but once merged the next PR's base would
            # no longer list it, so un-publishing must not merge quietly.
            errors.append(
                f"{name} is published on the base branch but `publish = false` "
                "in this PR. Un-publishing a crate is a release decision for "
                "the owner, not something a PR can do on its own"
            )
    for where in semver_metadata(head):
        errors.append(
            f"{where} configures cargo-semver-checks, which can turn its lints "
            "off. No manifest may declare it"
        )
    for error in errors:
        lines.append(f"::error::{error}.")
    return not errors, crates, lines


def extract_tree(root: Path, rev: str, dest: Path) -> None:
    """Write the tree of commit `rev` into `dest/tree`.

    git writes the files itself, through a scratch index in `dest`, so the
    repository's own index and working tree are not touched.
    """
    env = {**os.environ, "GIT_INDEX_FILE": str(dest / "index")}
    for var in ("GIT_DIR", "GIT_WORK_TREE"):
        env.pop(var, None)
    try:
        for cmd in (
            ["read-tree", f"{rev}^{{commit}}"],
            ["checkout-index", "--all", f"--prefix={dest / 'tree'}/"],
        ):
            subprocess.run(
                ["git", "-C", str(root), *cmd], check=True, capture_output=True, env=env
            )
    except (OSError, subprocess.CalledProcessError) as err:
        raise PackagesError(f"could not read the base commit {rev}: {err}") from err


def symlinked_manifests(root: Path) -> list[str]:
    """Tracked `Cargo.toml` paths that are symlinks.

    The job's path filter keys on the path that changed. A manifest that is
    a symlink can change through its target, which the filter does not see.
    """
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "ls-files", "--stage", "-z"],
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        ).stdout
    except (OSError, subprocess.CalledProcessError) as err:
        raise PackagesError(f"could not list the tracked files: {err}") from err
    found = []
    for record in out.split("\0"):
        if not record:
            continue
        meta, _, path = record.partition("\t")
        name = path.rsplit("/", 1)[-1]
        if meta.split(" ")[0] == "120000" and name.lower() == "cargo.toml":
            found.append(path)
    return sorted(found)


def packages_main(root: Path, base_rev: str) -> int:
    if not SHA.match(base_rev):
        print(
            f"::error::the base must be a full commit SHA, got '{base_rev}'",
            file=sys.stderr,
        )
        return 1
    try:
        with tempfile.TemporaryDirectory(prefix="semver-base-") as tmp:
            extract_tree(root, base_rev, Path(tmp))
            base = cargo_metadata(Path(tmp) / "tree")
        head = cargo_metadata(root)
        links = symlinked_manifests(root)
    except (PackagesError, ValueError) as err:
        print(f"::error::{err}", file=sys.stderr)
        return 1
    ok, crates, lines = decide_packages(base, head)
    for path in links:
        ok = False
        lines.append(
            f"::error::{path} is a symlink. A manifest must be a regular file, "
            "so that a change to it starts the Public API job."
        )
    print("\n".join(lines), file=sys.stderr)
    if not ok:
        return 1
    print("\n".join(crates))
    return 0


def main(argv: list[str]) -> int:
    if argv == ["guard"]:
        return guard_main(dict(os.environ))
    if argv == ["baseline"]:
        return baseline_main(Path.cwd())
    if len(argv) == 2 and argv[0] == "packages":
        return packages_main(Path.cwd(), argv[1])
    print(__doc__.split("\n\n")[1], file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
