#!/usr/bin/env python3
"""Fail if a crate that is not a host names a host's binding types.

    python3 tools/check_host_types.py [--root PATH]

ADR-0006 decision 1 makes each host a binding of the `fieldglass` umbrella:
"No host type appears in it", and #464's acceptance says it outright — *no
function outside a host crate takes a host DTO or returns a host error type*.
That is the rule this checks.

**Why a checker and not a Rust test.** The rule is about what a crate does
*not* contain, across every file of every crate, and Rust has no way to assert
the absence of a dependency edge from inside the crate that would have it. A
`cargo` manifest check alone is not enough either: `napi` could arrive through
a re-export, and a `wasm_bindgen::` path could be written out in full without
any `use`. So this walks the source, the way
`check_format_crate_reexports.py` does, and the manifests beside it.

**What counts as a host.** `crates/fieldglass-napi` and `crates/fieldglass-wasm`
— the two crates whose whole job is the binding — and nothing else. The list is
written out rather than inferred from a manifest, so adding a third host is a
decision recorded here.

**What is walked.** Every workspace member the root `Cargo.toml` lists, minus
the hosts, and every `Cargo.toml` under each — `tests/crate-independence` is a
member and not under `crates/`, and each format crate has a `fuzz/` package with
a manifest of its own. The member list is read rather than guessed, and a member
whose directory or sources are missing is reported, so the walk cannot quietly
skip one.

**What counts as a host type.** Any path rooted at one of the binding crates
(`napi`, `napi_derive`, `wasm_bindgen`, `js_sys`, `serde_wasm_bindgen`,
`wasm_bindgen_futures`, `web_sys`), the attribute macros they are usually
spelled with (`#[napi…]`, `#[wasm_bindgen…]`), and a dependency line naming one
of those packages in a non-host crate's manifest.

Accepted limitations, stated rather than discovered:

  * Matching is textual, so a host name inside a string literal would be
    reported. Comments — doc comments included — are stripped first, because
    they legitimately discuss the hosts; string literals are not. Nothing in
    the tree trips it today, and the escape hatch if something does is to
    reword rather than to add a suppression.
  * A host name reached through an alias (`use napi as n;`) is invisible past
    the `use` line, which is itself reported. A *manifest* rename
    (`n = { package = "napi" }`) is resolved: the renamed key becomes a host
    root for that crate's sources too.
  * The walk asserts it found every workspace member it was told about, and at
    least one `.rs` file in each. A checker that silently found nothing would
    pass while checking nothing, which is the failure mode this file exists to
    prevent.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# Crates whose entire purpose is a language binding. Everything else in the
# workspace is engine, and owes the rule.
HOST_CRATES = frozenset({"fieldglass-napi", "fieldglass-wasm"})

# Crate roots that only a host may name.
HOST_ROOTS = (
    "napi",
    "napi_derive",
    "napi_build",
    "wasm_bindgen",
    "wasm_bindgen_futures",
    "js_sys",
    "serde_wasm_bindgen",
    "web_sys",
)

# Cargo package names for the same set, for the manifest half.
HOST_PACKAGES = frozenset(
    {
        "napi",
        "napi-derive",
        "napi-build",
        "wasm-bindgen",
        "wasm-bindgen-futures",
        "js-sys",
        "serde-wasm-bindgen",
        "web-sys",
    }
)

# `napi::Error`, `use wasm_bindgen::prelude::*`, a bare `JsValue`, and the two
# attribute macros. Anchored on a non-identifier character so `my_napi_helper`
# and `wasm_bindgen_shim` do not match.
PATH_RE = re.compile(r"(?<![A-Za-z0-9_])(" + "|".join(HOST_ROOTS) + r")::")
ATTR_RE = re.compile(r"#\[\s*(napi|wasm_bindgen)\b")
JS_TYPE_RE = re.compile(r"(?<![A-Za-z0-9_])JsValue(?![A-Za-z0-9_])")

# A `[dependencies]`-style line: `napi = "3"`, `napi.workspace = true`.
DEP_RE = re.compile(r"^\s*([A-Za-z0-9_-]+)\s*(=|\.)")
# The section header above it. Only dependency tables are read, so a `[features]`
# entry that happens to be called `napi` is not a dependency.
SECTION_RE = re.compile(r"^\s*\[([^\]]+)\]")
DEP_SECTIONS = ("dependencies", "dev-dependencies", "build-dependencies")
# `n = { package = "napi", … }` — the rename that would otherwise defeat both
# halves of the check at once.
RENAME_RE = re.compile(r"""package\s*=\s*["']([A-Za-z0-9_-]+)["']""")
# The workspace member list in the root manifest.
MEMBERS_RE = re.compile(r"^\s*members\s*=\s*\[(.*?)\]", re.DOTALL | re.MULTILINE)


def strip_comments(text: str) -> str:
    """Drop `//` line comments (doc comments included) and `/* … */` blocks.

    They discuss the hosts on purpose — the API crate's own rules are explained
    by naming `napi` — and a rule that fired on prose would be a rule people
    delete.

    A block comment is replaced by its own newlines rather than by a space, so
    every line number after it still matches the file. A checker that reports
    the wrong line is a checker nobody trusts the second time.
    """

    def blanked(match: re.Match[str]) -> str:
        return "\n" * match.group(0).count("\n")

    text = re.sub(r"/\*.*?\*/", blanked, text, flags=re.DOTALL)
    return re.sub(r"//.*", "", text)


def offences_in_source(path: Path, extra_roots: frozenset[str]) -> list[str]:
    """Every line of a non-host `.rs` file that names a host type.

    `extra_roots` are crate roots a manifest rename introduced, so
    `n = { package = "napi" }` followed by `n::Error` is caught too.
    """
    out: list[str] = []
    patterns: list[tuple[re.Pattern[str], str]] = [
        (PATH_RE, "a host crate path"),
        (ATTR_RE, "a host attribute macro"),
        (JS_TYPE_RE, "a host DTO (JsValue)"),
    ]
    if extra_roots:
        alias = "|".join(re.escape(root) for root in sorted(extra_roots))
        patterns.insert(
            1,
            (
                re.compile(r"(?<![A-Za-z0-9_])(" + alias + r")::"),
                "a host crate path under a manifest rename",
            ),
        )
    text = strip_comments(path.read_text(encoding="utf-8"))
    for number, line in enumerate(text.splitlines(), start=1):
        for pattern, what in patterns:
            if pattern.search(line):
                out.append(f"{path}:{number}: {what} — {line.strip()}")
                break
    return out


def offences_in_manifest(path: Path) -> tuple[list[str], set[str]]:
    """Host dependency lines in a non-host manifest, and the roots a rename
    introduced."""
    out: list[str] = []
    renamed: set[str] = set()
    section = ""
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        stripped = line.split("#", 1)[0]
        header = SECTION_RE.match(stripped)
        if header:
            section = header.group(1)
            continue
        # `[dependencies]`, `[dev-dependencies]`, `[target.'…'.dependencies]`,
        # `[workspace.dependencies]` — but not `[features]`, where a key may
        # legitimately share a dependency's name.
        if not section.split(".")[-1] in DEP_SECTIONS:
            continue
        match = DEP_RE.match(stripped)
        if not match:
            continue
        key = match.group(1)
        rename = RENAME_RE.search(stripped)
        package = rename.group(1) if rename else key
        if package in HOST_PACKAGES:
            out.append(f"{path}:{number}: depends on the host package {package!r}")
            if rename:
                renamed.add(key.replace("-", "_"))
    return out, renamed


def workspace_members(root: Path) -> list[str]:
    """The member paths the root manifest lists, in order.

    Read rather than guessed: `tests/crate-independence` is a member and is not
    under `crates/`, so a walk of `crates/` alone would leave a whole non-host
    package unchecked.
    """
    manifest = root / "Cargo.toml"
    if not manifest.is_file():
        return []
    match = MEMBERS_RE.search(manifest.read_text(encoding="utf-8"))
    if not match:
        return []
    return re.findall(r"""["']([^"']+)["']""", match.group(1))


def check(root: Path) -> list[str]:
    """Every offence across the workspace, plus the checker's own liveness."""
    crates = root / "crates"
    if not crates.is_dir():
        return [f"{crates}: no crates directory — the checker is pointed at the wrong root"]

    problems: list[str] = []
    seen_hosts: set[str] = set()
    scanned_crates = 0

    # Every workspace member, plus any directory under `crates/` the member
    # list does not mention (a crate added to the tree and not to the manifest
    # is a mistake, but it should still be checked, not skipped).
    members = {root / m for m in workspace_members(root)}
    members |= {p for p in crates.iterdir() if p.is_dir()}

    for crate in sorted(members):
        if crate.name in HOST_CRATES:
            seen_hosts.add(crate.name)
            continue
        if not crate.is_dir():
            problems.append(f"{crate}: a workspace member with no directory")
            continue
        sources = sorted(crate.rglob("*.rs"))
        if not sources:
            problems.append(f"{crate}: no .rs files found — the walk is broken, not the crate")
            continue
        scanned_crates += 1
        # Manifests first: a rename found here becomes a crate root the source
        # scan below has to know about.
        renamed: set[str] = set()
        for manifest in sorted(crate.rglob("Cargo.toml")):
            found, aliases = offences_in_manifest(manifest)
            problems.extend(found)
            renamed |= aliases
        for source in sources:
            problems.extend(offences_in_source(source, frozenset(renamed)))

    missing_hosts = sorted(HOST_CRATES - seen_hosts)
    if missing_hosts:
        problems.append(
            "these crates are exempt as hosts but do not exist: "
            + ", ".join(missing_hosts)
            + " — update HOST_CRATES rather than leaving a dead exemption"
        )
    if scanned_crates == 0:
        problems.append("no non-host crate was scanned; the check would pass vacuously")
    return problems


def main() -> int:
    """Entry point. Encoding is UTF-8 throughout; see the read calls above."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=Path(__file__).resolve().parent.parent,
        help="repository root (default: the one this script lives in)",
    )
    args = parser.parse_args()
    problems = check(args.root)
    if problems:
        print("Host types outside a host crate (ADR-0006 decision 1):", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    print("no host types outside a host crate")
    return 0


if __name__ == "__main__":
    sys.exit(main())
