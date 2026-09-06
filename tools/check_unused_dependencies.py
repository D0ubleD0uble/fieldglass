#!/usr/bin/env python3
"""Fail if a package declares a dependency none of its own Rust names.

    python3 tools/check_unused_dependencies.py

Cargo says nothing about a dependency that is declared and never used. It
resolves it, downloads it, compiles it, links it, and reports success — so the
line survives every gate in this repo: `cargo build`, `cargo clippy -D
warnings`, `cargo test --workspace`, `cargo deny check` and the six-target
cross-compile all pass with it there. That is how `serde` sat in
`fieldglass-grib1`, `fieldglass-grib2` and `fieldglass-netcdf`, and `thiserror`
in `fieldglass-grib1`, until #538: four lines, in three crates published to
crates.io, naming crates no line of their source had ever mentioned.

The cost is not only build time. A published manifest is a statement about what
a crate needs, and it is the statement a downstream reader audits, licence-scans
and vendors against. `cargo deny check` reads the same graph, so an unused
dependency also widens the advisory and licence surface the project holds itself
to for nothing.

**What it checks.** For every package in the tree — workspace member or not,
which is deliberate: the `fuzz/` crates and `fieldglass-verify` are their own
workspaces and no `--workspace` command sees them — each key of
`[dependencies]`, `[dev-dependencies]`, `[build-dependencies]` and their
`[target.'cfg(…)'.…]` forms must appear as an identifier somewhere in that
package's own `.rs` files. The key is what the check looks for, not the
`package = "…"` it may rename, because the key is the name the code spells.

**What it deliberately does not check.** Whether a dependency is in the *right*
table. A normal dependency used only from `tests/` is a real downstream cost —
it is compiled by every consumer and used by none of them — but
`tests/crate-independence` is exactly that shape on purpose (its whole subject
is the manifest, and the assertions live in `tests/standalone.rs`), so the rule
would need an exception on the one package it would fire on. Not worth a gate
that starts with an allow-list as long as its findings.

**Comments and string literals do not count as use.** This repo comments
heavily and names crates in prose — the manifests explain every dependency, and
so do the modules that use them — so a check that grepped the raw text would
call `serde` used in `fieldglass-grib2` on the strength of a doc comment in a
test. The sources are stripped of comments and literals first; `SKIPPED` below
is where a dependency that genuinely has no identifier to find is written down
with its reason.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Directories that are never a package of ours.
PRUNED = {"target", "node_modules", ".git", ".venv", "out", "dist"}

# The dependency tables cargo compiles. `build-dependencies` is included
# because `build.rs` is source like any other, and `fieldglass-napi`'s names
# `napi_build` there and nowhere else.
DEP_TABLES = ("dependencies", "dev-dependencies", "build-dependencies")

# Fewer packages than this and the walk has gone wrong — a rename, a prune
# entry that swallowed a tree — and the check would pass by finding nothing to
# check. The count is 12 today (eight under `crates/`, three `fuzz/`, and
# `tests/crate-independence`); the floor is the number below which the answer
# is certainly the walk's fault and not the tree's.
MIN_PACKAGES = 10

# (package name, dependency key) -> why it is declared without being named.
# Empty: every dependency in the tree is spelled in the source that uses it.
# An entry here is a claim a reviewer has to read, which is the point.
SKIPPED: dict[tuple[str, str], str] = {}

# The start of a Rust string literal, with the `b` / `r` / `br` prefixes and the
# raw-string hashes. Matched only where an identifier character cannot precede
# it, so the `r` of `for` is not read as a raw string.
_STRING_START = re.compile(r'(b?r)(#*)"|b?"')


def strip_comments_and_literals(src: str) -> str:
    """Blank out Rust comments, string literals and char literals.

    Naming a crate is not using it. Every dependency in this workspace is
    explained in a comment somewhere, usually in the module that uses it but
    sometimes in one that does not, so raw text is the wrong thing to search.
    Literals go too: a `docs.rs` URL in an error message is not a use either.
    Everything removed is replaced by a space so identifiers cannot be joined
    across the gap.
    """
    out: list[str] = []
    i, n = 0, len(src)
    while i < n:
        two = src[i : i + 2]
        if two == "//":
            end = src.find("\n", i)
            i = n if end == -1 else end
            out.append(" ")
            continue
        if two == "/*":
            # Rust block comments nest, so a `/*` inside one is not noise.
            depth, j = 1, i + 2
            while j < n and depth:
                if src[j : j + 2] == "/*":
                    depth, j = depth + 1, j + 2
                elif src[j : j + 2] == "*/":
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            i = j
            out.append(" ")
            continue
        if src[i] == "'":
            # A char literal, or a lifetime. `'a` is a lifetime; `'a'` is a
            # char; `'\n'` is a char whose body contains an escape.
            if src[i : i + 2] == "'\\":
                j = i + 2
                while j < n and src[j] != "'":
                    j += 2 if src[j] == "\\" else 1
                i = j + 1
                out.append(" ")
                continue
            if i + 2 < n and src[i + 2] == "'":
                i += 3
                out.append(" ")
                continue
            out.append("'")
            i += 1
            continue
        if src[i] in '"br':
            preceded_by_ident = i > 0 and (src[i - 1].isalnum() or src[i - 1] == "_")
            m = None if preceded_by_ident else _STRING_START.match(src, i)
            if m:
                if m.group(1):  # raw: no escapes, closed by `"` plus its hashes
                    close = '"' + "#" * len(m.group(2))
                    end = src.find(close, m.end())
                    i = n if end == -1 else end + len(close)
                else:
                    j = m.end()
                    while j < n and src[j] != '"':
                        j += 2 if src[j] == "\\" else 1
                    i = j + 1
                out.append(" ")
                continue
        out.append(src[i])
        i += 1
    return "".join(out)


def package_dirs(root: Path) -> list[Path]:
    """Every directory holding a `Cargo.toml` with a `[package]` table."""
    found: list[Path] = []
    stack = [root]
    while stack:
        here = stack.pop()
        manifest = here / "Cargo.toml"
        if manifest.is_file():
            try:
                if "package" in tomllib.loads(manifest.read_text(encoding="utf-8")):
                    found.append(here)
            except tomllib.TOMLDecodeError:
                # `check-toml` (pre-commit) owns malformed manifests; this
                # check has nothing to say about one and should not be the
                # hook that reports it.
                pass
        for child in here.iterdir():
            if child.is_dir() and child.name not in PRUNED and not child.is_symlink():
                stack.append(child)
    return sorted(found)


def declared_dependencies(manifest: dict) -> dict[str, str]:
    """Dependency key -> the table it was declared in, across every form."""
    declared: dict[str, str] = {}
    tables = [(manifest, "")]
    for cfg, spec in manifest.get("target", {}).items():
        tables.append((spec, f"target.'{cfg}'."))
    for table, prefix in tables:
        for name in DEP_TABLES:
            for key in table.get(name, {}):
                declared.setdefault(key, f"{prefix}{name}")
    return declared


def source_identifiers(pkg: Path) -> tuple[set[str], int]:
    """Identifiers spelled in a package's own `.rs` files, and how many there were.

    A nested package (`crates/fieldglass-grib1/fuzz`) is walked as itself, so
    its sources are excluded here: a dependency of the fuzz crate must be named
    by the fuzz crate.
    """
    idents: set[str] = set()
    files = 0
    stack = [pkg]
    while stack:
        here = stack.pop()
        for child in sorted(here.iterdir()):
            if child.is_symlink():
                continue
            if child.is_dir():
                if child.name in PRUNED or (child / "Cargo.toml").is_file():
                    continue
                stack.append(child)
            elif child.suffix == ".rs":
                files += 1
                text = strip_comments_and_literals(child.read_text(encoding="utf-8"))
                idents.update(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", text))
    return idents, files


def main() -> int:
    problems: list[str] = []
    packages = package_dirs(ROOT)
    if len(packages) < MIN_PACKAGES:
        problems.append(
            f"found {len(packages)} package(s) under {ROOT}, expected at least "
            f"{MIN_PACKAGES} — the walk is wrong, so this check proved nothing"
        )

    seen: set[tuple[str, str]] = set()
    for pkg in packages:
        rel = pkg.relative_to(ROOT).as_posix() or "."
        manifest = tomllib.loads((pkg / "Cargo.toml").read_text(encoding="utf-8"))
        name = manifest["package"].get("name", rel)
        declared = declared_dependencies(manifest)
        if not declared:
            continue

        idents, files = source_identifiers(pkg)
        if files == 0:
            problems.append(
                f"{rel}: declares {len(declared)} dependencies and has no `.rs` "
                f"file — every one of them would look unused, so this is the "
                f"walk being wrong rather than the manifest"
            )
            continue

        for key, table in sorted(declared.items()):
            if (name, key) in SKIPPED:
                seen.add((name, key))
                continue
            if key.replace("-", "_") not in idents:
                problems.append(
                    f"{rel}/Cargo.toml: [{table}] `{key}` is not named anywhere in "
                    f"{rel}'s own `.rs` files — cargo compiles it regardless and "
                    f"says nothing, so drop the line, or record it in SKIPPED in "
                    f"tools/check_unused_dependencies.py with the reason"
                )

    for entry in SKIPPED:
        if entry not in seen:
            problems.append(
                f"SKIPPED names {entry[0]} / {entry[1]}, which is not a declared "
                f"dependency any more — delete the entry with the line it excused"
            )

    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
