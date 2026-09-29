#!/usr/bin/env python3
"""Fail if a verified kernel file has stopped being verified.

    python3 tools/check_verified_kernels.py

A *kernel file* is production source that carries Verus proofs: specs in
`cfg_attr(verus_keep_ghost, verus_spec(...))` attributes, which a normal build
never expands. The shipped crate compiles the file as ordinary Rust, and
`crates/fieldglass-verify` includes the very same file with
`#[path = "..."] mod ...;` and proves it (#199, docs/verification.md). One copy,
so there is no drift between a proof and the code it is about.

What that layout cannot see is the include going away. Nothing fails if the
`#[path]` line is deleted, or points somewhere else after a file is renamed: the
shipped crate still builds, `scripts/verify.sh` still verifies whatever is left,
and the specs in the orphaned file sit there looking like proofs. Nor does it
see the CI trigger: `.github/workflows/verify.yml` runs only on the paths it
lists, so a kernel file missing from them is edited without Verus ever running,
and the job keeps reporting green while guarding nothing.

So this checks three things, and each failure names the file:

- every `.rs` file under `crates/` that contains `verus_spec` (outside the
  verification crate itself) is `#[path]`-included by a file under
  `crates/fieldglass-verify/src/`;
- every such `#[path]` names a file that exists;
- every kernel file matches a `paths:` entry of **both** the `push` and the
  `pull_request` trigger of `verify.yml`.

Finding no kernel file at all is a failure rather than a clean run: that is the
shape this checker exists to catch, reached by the layout moving under it.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
VERIFY_CRATE = Path("crates/fieldglass-verify")
WORKFLOW = Path(".github/workflows/verify.yml")

# The same prune set as the other tree-walking checkers.
PRUNED = {"target", "node_modules", ".git", ".venv", "out", "dist"}

MARKER = "verus_spec"
_PATH_ATTR = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]')


def kernel_files(repo: Path) -> list[Path]:
    """Every `.rs` file under `crates/` that carries a proof, repo-relative.

    The verification crate's own sources are not kernels: they are written in
    Verus syntax and verified where they stand.
    """
    found: list[Path] = []
    stack = [repo / "crates"]
    while stack:
        directory = stack.pop()
        try:
            entries = list(directory.iterdir())
        except OSError:
            continue
        for entry in entries:
            if entry.is_dir():
                if entry.name not in PRUNED and entry != repo / VERIFY_CRATE:
                    stack.append(entry)
            elif entry.suffix == ".rs" and MARKER in entry.read_text(encoding="utf-8"):
                found.append(entry.relative_to(repo))
    return sorted(found)


def included_files(repo: Path) -> tuple[set[Path], list[str]]:
    """What the verification crate includes by `#[path]`, and any dangling one.

    A path resolves against the directory of the file that declares it, which
    is how rustc resolves it for a module declared in `lib.rs` or `mod.rs`.
    """
    included: set[Path] = set()
    problems: list[str] = []
    for source in sorted((repo / VERIFY_CRATE / "src").rglob("*.rs")):
        for target in _PATH_ATTR.findall(source.read_text(encoding="utf-8")):
            resolved = (source.parent / target).resolve()
            try:
                relative = resolved.relative_to(repo.resolve())
            except ValueError:
                problems.append(f"{source.relative_to(repo)}: #[path] {target!r} leaves the repo")
                continue
            if not resolved.is_file():
                problems.append(
                    f"{source.relative_to(repo)}: #[path] {target!r} names no file "
                    f"({relative}) — was a kernel renamed or moved?"
                )
                continue
            included.add(relative)
    return included, problems


def trigger_paths(workflow_text: str) -> dict[str, list[str]]:
    """The `paths:` list of each `on:` trigger, keyed by trigger name.

    A small line-based reader rather than a YAML parser, because the workflow's
    shape is fixed and this keeps the checker free of dependencies: a trigger
    key at two-space indent, `paths:` beneath it, and `- 'glob'` items.
    """
    triggers: dict[str, list[str]] = {}
    current: str | None = None
    in_paths = False
    in_on = False
    for raw in workflow_text.splitlines():
        line = raw.split("#", 1)[0].rstrip()
        if not line:
            continue
        indent = len(line) - len(line.lstrip())
        text = line.strip()
        if indent == 0:
            in_on = text == "on:"
            current, in_paths = None, False
            continue
        if not in_on:
            continue
        if indent == 2 and text.endswith(":"):
            current, in_paths = text[:-1], False
            triggers.setdefault(current, [])
        elif current and text == "paths:":
            in_paths = True
        elif current and in_paths and text.startswith("- "):
            triggers[current].append(text[2:].strip().strip("'\""))
        elif in_paths and not text.startswith("- "):
            in_paths = False
    return triggers


def glob_matches(pattern: str, path: str) -> bool:
    """GitHub's `paths` glob: `**` crosses directories, `*` does not."""
    regex = ""
    i = 0
    while i < len(pattern):
        if pattern.startswith("**", i):
            regex += ".*"
            i += 2
        elif pattern[i] == "*":
            regex += "[^/]*"
            i += 1
        else:
            regex += re.escape(pattern[i])
            i += 1
    return re.fullmatch(regex, path) is not None


def check(repo: Path = REPO) -> list[str]:
    """Every problem found, as printable lines. Empty means all is in order."""
    kernels = kernel_files(repo)
    if not kernels:
        return [f"no file under {repo / 'crates'} carries `{MARKER}` — has the layout moved?"]

    included, problems = included_files(repo)
    for kernel in kernels:
        if kernel not in included:
            problems.append(
                f"{kernel}: carries `{MARKER}` but {VERIFY_CRATE}/src does not include it "
                f'with `#[path = "..."]`, so its proofs are never checked'
            )

    workflow = repo / WORKFLOW
    try:
        triggers = trigger_paths(workflow.read_text(encoding="utf-8"))
    except OSError as exc:
        return [*problems, f"{WORKFLOW}: cannot read it: {exc}"]
    for trigger in ("push", "pull_request"):
        globs = triggers.get(trigger)
        if not globs:
            problems.append(f"{WORKFLOW}: no `paths:` list under `{trigger}:`")
            continue
        for kernel in kernels:
            if not any(glob_matches(g, kernel.as_posix()) for g in globs):
                problems.append(
                    f"{WORKFLOW}: `{trigger}` paths do not cover {kernel}, so an edit "
                    f"to it never runs Verus"
                )
    return problems


def main() -> int:
    problems = check()
    if problems:
        print("Verified kernel files that are no longer verified:\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
