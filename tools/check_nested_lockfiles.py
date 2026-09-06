#!/usr/bin/env python3
"""Fail if a nested crate's `Cargo.lock` no longer agrees with its manifest.

    python3 tools/check_nested_lockfiles.py

Four crates in this tree declare a bare `[workspace]` and so form workspaces of
their own: the three `fuzz/` crates, kept out so the stable gates never try to
compile a nightly-only libFuzzer target, and `crates/fieldglass-verify`, kept out
so a published crate never carries `vstd`. Each therefore has its own
`Cargo.lock`, and that is the blind spot: no `--workspace` command resolves them,
so `cargo build`, `cargo clippy`, `cargo test --workspace` and
`cargo deny --locked check` all pass over a lock that has gone stale. The root
`Cargo.lock` is not in scope here for exactly that reason — every one of those
commands already resolves it.

For a fuzz crate the drift is not theoretical. `cargo fuzz run` has no `--locked`
(cargo-fuzz 0.13.2 exposes no such flag), so a stale lock is silently re-resolved
and rewritten mid-run: #398 had the GRIB2 fuzz lock pinning rust-j2k 0.2.0 for a
whole release cycle while the workspace pinned `=0.3.0`.

`.github/workflows/fuzz.yml` gates the three fuzz jobs on this check, so drift is
loud rather than absorbed. But CI is late, and it is also narrow — that workflow
only runs when a format crate changes, and `crates/fieldglass-verify`'s lock has
no workflow watching it at all. A fuzz lock goes stale the moment its *format
crate's* manifest changes, because the fuzz crate path-depends on it and the lock
records that crate's dependency edges: #640 edited all three format-crate
manifests, staled all three locks at once, and the first anyone heard of it was
three red fuzz jobs on the pull request.

So this runs in the hook too, on the commit that causes it. It costs tens of
milliseconds: `cargo metadata --locked` resolves without building, and for the
common drift — a dependency dropped, or re-pinned to a version already in the
index cache — it refuses without touching the network. Adding a dependency cargo
has never seen does cost one index update before the same refusal.

Discovery is a pruned directory walk, not the `crates/*/fuzz/Cargo.toml` glob the
workflow used to run inline, which failed open twice: it never looked at
`fieldglass-verify`, and a fuzz directory whose manifest went missing matched
nothing and was silently skipped. It walks for **both** halves of a nested
workspace, because either one alone leaves the mirror image of that gap open:

- a `Cargo.lock` with no manifest beside it is a finding, not something to skip;
- a nested `[workspace]` manifest with **no** lockfile beside it is also a
  finding. `cargo fuzz init` writes a fuzz crate whose lock is easy to leave
  uncommitted, and a check keyed on lockfiles alone would call that tree clean
  while `cargo fuzz run` re-resolved freely — #398 all over again.

A failure is classified rather than assumed: cargo exits non-zero for a manifest
it cannot parse and for a registry it cannot reach as well as for a lock that
disagrees, and telling someone with a correct lock and no network to run
`cargo update` sends them somewhere that will also fail.
"""

from __future__ import annotations

import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Pruned rather than filtered afterwards: build directories hold vendored crate
# sources with lockfiles of their own, and `.git` is large enough to be worth not
# walking. Deliberately the same set as `tools/check_unused_dependencies.py`,
# which walks this tree for the neighbouring reason — two prune lists over one
# tree drift, and a divergence between them is a difference nobody chose.
PRUNED = {"target", "node_modules", ".git", ".venv", "out", "dist"}

# How to read a non-zero `cargo metadata --locked`. Only the first of these is
# the drift this checker is named for.
STALE = "stale"
UNRESOLVABLE = "unresolvable"
NO_CARGO = "no-cargo"

# cargo's own words when the lock is the thing that disagrees. Every other
# non-zero exit — a manifest that will not parse, an index it cannot reach — is
# a different problem with different advice.
_LOCK_REFUSAL = "--locked was passed"


def run_cargo_metadata(manifest: Path) -> tuple[str | None, str]:
    """Resolve `manifest` against the lockfile beside it, without building.

    Returns `(kind, detail)`, where `kind` is `None` on success and otherwise
    one of `STALE`, `UNRESOLVABLE` or `NO_CARGO`. A cargo that cannot be run at
    all is a failure and not a skip: a machine without cargo must not be told
    its lockfiles are fine.
    """
    try:
        completed = subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--manifest-path",
                str(manifest),
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            check=False,
        )
    except OSError as exc:  # cargo missing, not executable, ...
        return NO_CARGO, f"could not run cargo: {exc}"
    if completed.returncode == 0:
        return None, ""
    detail = (completed.stderr or completed.stdout).strip()
    return (STALE if _LOCK_REFUSAL in detail else UNRESOLVABLE), detail


def declares_a_workspace(manifest: Path) -> bool:
    """Whether `manifest` is a workspace root of its own.

    A manifest that will not parse is reported as one: it is then walked into,
    `cargo metadata` is run on it, and the parse error reaches the user as
    cargo's own message rather than being silently dropped here.
    """
    try:
        return "workspace" in tomllib.loads(manifest.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return True


def nested_workspaces(repo: Path) -> list[Path]:
    """Every directory under `repo`, bar `repo` itself, that is a nested workspace.

    A directory qualifies on either half — a `Cargo.lock`, or a `Cargo.toml`
    declaring `[workspace]` — so that a missing one of the pair is a finding
    rather than an omission. Sorted, for a stable report.
    """
    found: set[Path] = set()
    stack = [repo]
    while stack:
        directory = stack.pop()
        try:
            entries = list(directory.iterdir())
        except OSError:
            continue
        for entry in entries:
            if entry.is_dir():
                if entry.name not in PRUNED:
                    stack.append(entry)
            elif entry.parent == repo:
                continue
            elif entry.name == "Cargo.lock":
                found.add(entry.parent)
            elif entry.name == "Cargo.toml" and declares_a_workspace(entry):
                found.add(entry.parent)
    return sorted(found)


def check(repo: Path = REPO, metadata=run_cargo_metadata) -> list[str]:
    """Every problem found, as printable lines. Empty means the tree is clean."""
    directories = nested_workspaces(repo)
    if not directories:
        # Zero workspaces checked is the fail-open shape this checker exists to
        # avoid, so it is reported as a failure rather than a clean run.
        return [f"no nested workspace found under {repo} — has the layout moved?"]

    problems: list[str] = []
    for directory in directories:
        # Kept going after a failure, rather than the workflow's old `set -e`:
        # with three locks staled by one manifest edit, naming all three saves
        # two more round trips.
        manifest = directory / "Cargo.toml"
        if not manifest.is_file():
            problems.append(f"{directory}: Cargo.lock with no Cargo.toml beside it")
            continue
        if not (directory / "Cargo.lock").is_file():
            problems.append(
                f"{manifest}: nested workspace with no Cargo.lock beside it — "
                f"nothing resolves this crate with `--locked`, so commit one "
                f"(`cargo update -w` in {directory})"
            )
            continue
        kind, detail = metadata(manifest)
        if kind == STALE:
            problems.append(
                f"{manifest}: lockfile disagrees with the manifest — refresh it with "
                f"`cargo update -w` in {directory} (or `cargo check` there), and commit "
                f"the Cargo.lock alongside the manifest change\n    {detail}"
            )
        elif kind == NO_CARGO:
            problems.append(f"{manifest}: could not check this lockfile\n    {detail}")
        elif kind is not None:
            problems.append(
                f"{manifest}: cargo could not resolve this crate — this is not a stale "
                f"lockfile, so `cargo update` is not the fix\n    {detail}"
            )
    return problems


def main() -> int:
    problems = check()
    if problems:
        print("Nested workspaces whose lockfiles are not in order:\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
