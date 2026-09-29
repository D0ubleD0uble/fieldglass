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

So this checks these things, and each failure names the file:

- every `.rs` file under `crates/` that contains `verus_spec` (outside the
  verification crate itself) is `#[path]`-included by a file under
  `crates/fieldglass-verify/src/`;
- an include counts only when it is live. Comments are stripped first, so a
  `// #[path = ...]` or a `/* ... */` around the include is not one. The
  `#[path]` must sit on a `mod NAME;` item at the top level of its file, and
  that item may carry no `cfg` or `cfg_attr`: `#[cfg(any())]` switches an
  include off as surely as deleting it, and `verify.sh` then goes green over
  whatever proofs remain;
- every such `#[path]` names a file that exists;
- every kernel file, and every file a kernel's build in the verification crate
  depends on (`SUPPORT`), matches a `paths:` entry of **both** the `push` and
  the `pull_request` trigger of `verify.yml`.

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

# Files outside the kernels that the verification crate's build of them reads,
# so an edit to one can break verification without touching a kernel. A kernel
# names `crate::FieldglassError` (error.rs) and `crate::bits::BitReader`
# (bits.rs, whose methods the trusted spec names by signature), and the
# verification crate builds `fieldglass-core` as a dependency with
# `default-features = false`, which its lib.rs and Cargo.toml decide.
SUPPORT = (
    Path("crates/fieldglass-core/src/bits.rs"),
    Path("crates/fieldglass-core/src/error.rs"),
    Path("crates/fieldglass-core/src/lib.rs"),
    Path("crates/fieldglass-core/Cargo.toml"),
)

_PATH_ATTR = re.compile(r'#\[\s*path\s*=\s*"([^"]+)"\s*\]')
# One outer attribute. The inner `[^\[\]]` alternation allows one level of
# nested brackets, which is all an attribute on a `mod` item here needs.
_ATTR = r"#\[(?:[^\[\]]|\[[^\[\]]*\])*\]"
# Attributes, then an out-of-line module item: `mod NAME;`, `pub` optional.
_MOD_ITEM = re.compile(
    rf"((?:{_ATTR}\s*)+)(?:pub(?:\s*\([^)]*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;"
)
_CFG_ATTR = re.compile(r"#\[\s*cfg(?:_attr)?\s*\(")
_STRING = re.compile(r'"(?:\\.|[^"\\])*"')


def strip_comments(src: str) -> str:
    """Blank out `//` and (nested) `/* */` comments, keeping string literals.

    Unlike `check_unused_dependencies.strip_comments_and_literals`, string
    literals survive, because the `#[path]` target is one. Each removed
    character becomes a space (newlines kept), so offsets are unchanged.
    """
    out = list(src)
    i, n = 0, len(src)
    while i < n:
        if src[i] == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            i = j + 1
        elif src.startswith("//", i):
            end = src.find("\n", i)
            end = n if end == -1 else end
            out[i:end] = " " * (end - i)
            i = end
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif src.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            out[i:j] = ["\n" if c == "\n" else " " for c in src[i:j]]
            i = j
        else:
            i += 1
    return "".join(out)


def brace_depth(text: str, at: int) -> int:
    """How many `{` are open at offset `at`, ignoring braces inside strings."""
    code = _STRING.sub(lambda m: " " * len(m.group(0)), text[:at])
    return code.count("{") - code.count("}")


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
            elif entry.suffix == ".rs" and MARKER in strip_comments(
                entry.read_text(encoding="utf-8")
            ):
                found.append(entry.relative_to(repo))
    return sorted(found)


def included_files(repo: Path) -> tuple[set[Path], list[str]]:
    """What the verification crate includes by `#[path]`, and any bad include.

    Only a live include counts (see the module docs): comments are stripped,
    and the `#[path]` must be on a top-level `mod NAME;` with no `cfg`. A
    path resolves against the directory of the file that declares it, which
    is how rustc resolves it for a module declared in `lib.rs` or `mod.rs`.
    """
    included: set[Path] = set()
    problems: list[str] = []
    for source in sorted((repo / VERIFY_CRATE / "src").rglob("*.rs")):
        where = source.relative_to(repo)
        text = strip_comments(source.read_text(encoding="utf-8"))
        live: list[str] = []
        claimed: set[int] = set()
        for item in _MOD_ITEM.finditer(text):
            attrs = item.group(1)
            paths = list(_PATH_ATTR.finditer(attrs))
            if not paths:
                continue
            for path in paths:
                claimed.add(item.start(1) + path.start())
            name = item.group(2)
            if _CFG_ATTR.search(attrs):
                problems.append(
                    f"{where}: the #[path] include `mod {name}` carries a cfg, so it "
                    f"can be switched off while verify.sh stays green; includes of "
                    f"kernel files must be unconditional"
                )
            elif brace_depth(text, item.start()) != 0:
                problems.append(
                    f"{where}: the #[path] include `mod {name}` is nested inside a "
                    f"block; put it at the top level of the file"
                )
            else:
                live.extend(p.group(1) for p in paths)
        for stray in _PATH_ATTR.finditer(text):
            if stray.start() not in claimed:
                problems.append(
                    f"{where}: #[path] {stray.group(1)!r} is not on a `mod NAME;` "
                    f"item, so it includes nothing"
                )
        for target in live:
            resolved = (source.parent / target).resolve()
            try:
                relative = resolved.relative_to(repo.resolve())
            except ValueError:
                problems.append(f"{where}: #[path] {target!r} leaves the repo")
                continue
            if not resolved.is_file():
                problems.append(
                    f"{where}: #[path] {target!r} names no file "
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
        for watched in (*kernels, *SUPPORT):
            if not any(glob_matches(g, watched.as_posix()) for g in globs):
                problems.append(
                    f"{WORKFLOW}: `{trigger}` paths do not cover {watched}, so an edit "
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
