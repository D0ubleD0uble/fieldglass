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

Discovery is a pruned directory walk for `Cargo.lock` rather than the glob the
workflow used to run inline, because `crates/*/fuzz/Cargo.toml` fails open on the
cases worth catching: it never looked at `fieldglass-verify`, and a fuzz
directory whose manifest went missing matches nothing and is silently skipped,
leaving the loop to report success having checked one crate fewer than it thinks.
Here, a lockfile with no manifest beside it — and a run that finds no nested
lockfiles at all — is a failure.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# Pruned rather than filtered afterwards: build directories hold vendored crate
# sources with lockfiles of their own, and `.git` is large enough to be worth not
# walking. `target` is pruned at any depth because each nested workspace builds
# into its own.
PRUNED = {"target", "node_modules", ".git"}


def run_cargo_metadata(manifest: Path) -> tuple[bool, str]:
    """Resolve `manifest` against the lockfile beside it, without building.

    Returns `(ok, detail)`. A cargo that cannot be run at all is a failure and
    not a skip: a machine without cargo must not be told its lockfiles are fine.
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
        return False, f"could not run cargo: {exc}"
    if completed.returncode == 0:
        return True, ""
    return False, (completed.stderr or completed.stdout).strip()


def nested_lockfiles(repo: Path) -> list[Path]:
    """Every committed `Cargo.lock` under `repo` except the root workspace's.

    Sorted, and whether or not a manifest sits beside it — a lockfile whose
    manifest was renamed away is a finding, not something to skip.
    """
    found: list[Path] = []
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
            elif entry.name == "Cargo.lock" and entry.parent != repo:
                found.append(entry)
    return sorted(found)


def check(repo: Path = REPO, metadata=run_cargo_metadata) -> list[str]:
    """Every problem found, as printable lines. Empty means the tree is clean."""
    lockfiles = nested_lockfiles(repo)
    if not lockfiles:
        # Zero lockfiles checked is the fail-open shape this checker exists to
        # avoid, so it is reported as a failure rather than a clean run.
        return [f"no nested Cargo.lock found under {repo} — has the layout moved?"]

    problems: list[str] = []
    for lockfile in lockfiles:
        # Kept going after a failure, rather than the workflow's old `set -e`:
        # with three locks staled by one manifest edit, naming all three saves
        # two more round trips.
        manifest = lockfile.parent / "Cargo.toml"
        if not manifest.is_file():
            problems.append(f"{lockfile}: no Cargo.toml beside it")
            continue
        ok, detail = metadata(manifest)
        if not ok:
            problems.append(f"{manifest}: lockfile disagrees with the manifest\n    {detail}")
    return problems


def main() -> int:
    problems = check()
    if problems:
        print("Nested lockfiles out of sync with their manifests:\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "\nRefresh each one with `cargo update` in that crate's directory, "
            "and commit the Cargo.lock alongside the manifest change.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
