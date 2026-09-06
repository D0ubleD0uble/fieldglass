#!/usr/bin/env python3
"""Fail when the documented parsing surface is not the one the format crates use.

    python3 tools/check_parsing_surface.py

`fieldglass-core` serves two audiences behind one API. The format crates
(`fieldglass-grib1`, `-grib2`, `-netcdf`) take it with `default-features = false`
and consume only its *parsing* surface; `fieldglass-napi` and the umbrella take
the viewer and analysis surfaces on top. The crate documentation names that
parsing surface, module by module, and the `fieldglass-core` README says the same
thing again for a crates.io reader.

**Neither statement was checked, and both had drifted.** The pre-commit hook
`cargo-clippy-format-crates-parsing-only` (#551) proves the three libraries use
nothing *gated*. That is a weaker property than the doc claims: an ungated module
missing from the list can be used freely and every gate stays green. Which is
exactly what happened — `lead_time` arrived in `fieldglass-core` when both GRIB
editions' forecast strings moved there (#545), both libraries call it, and
neither copy of the list mentions it. In the other direction `detect` sat on both
lists while no library named it at all (#558 found `detect_format` has no caller
anywhere in the workspace).

So the invariant here is **equality**, not containment, over three sets:

    modules the three libraries name
      == modules the crate doc lists
      == modules the README lists

Containment would let the list go stale in the second direction, which is the
half `detect` demonstrates: a subset check calls that tree clean, and the
sentence's whole value is telling a would-be standalone consumer how small the
surface really is.

**Libraries, not whole crates, and that is measured rather than chosen.**
Scanning the format crates' test targets as well pulls in `contour`, which is
behind the `analysis` feature: `fieldglass-grib1` asks for it in
`[dev-dependencies]` so one test can contour what it decodes. Including tests
would therefore put a gated module into "the parsing surface", contradicting the
sentence's own justification — that none of these modules is behind a feature,
which is what makes a `default-features = false` dependency work. The libraries
are also what a consumer of the format crate actually compiles.

A `#[cfg(test)] mod` inside `src/` is a test target too, compiled with the same
dev-dependency features, so it is excluded on the same grounds — skipping
`tests/` and counting an inline test module would apply the rule to half the
test code. The file form, `#[cfg(test)] mod tests;`, is reported rather than
followed, so the exclusion cannot be widened by moving a test to its own file.

**Anything this cannot read is reported, never dropped.** Every one of these
would make the measured surface silently too small, and a surface measured too
small reads as "the documentation lists too much": a name that is neither a
module nor a crate-root re-export; a glob import; an `as`-alias of the crate,
after which no use is spelled `fieldglass_core::` at all; a crate-root re-export
whose left segment is a *dependency* rather than one of core's own modules; and
one name re-exported from two modules, which cannot be attributed to either.

Both copies are checked because fixing one would leave the other to drift, and
two copies of one list with a gate on neither is how this got wrong the first
time. Each is delimited by `parsing-surface` HTML comments — invisible in
rustdoc and on crates.io — so the checker reads a region rather than guessing at
a paragraph, and a region that has gone missing is a failure rather than an empty
list that trivially matches nothing.

The Rust lexer is imported from `check_unused_dependencies.py` rather than
written again: naming a module in a comment is not using it, and that scanner has
already absorbed the escaped-backslash and `c"…"`-prefix bugs a fresh one
rediscovers.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from check_unused_dependencies import strip_comments_and_literals  # noqa: E402

REPO = Path(__file__).resolve().parent.parent
CORE = REPO / "crates" / "fieldglass-core"

# The crates whose libraries define the surface. Pinned rather than globbed: a
# fourth format crate should widen this list past a reviewer, not silently.
FORMAT_CRATES = ("fieldglass-grib1", "fieldglass-grib2", "fieldglass-netcdf")

# Sanity floors for the parses below. Each is far under the real count and only
# fires when a parse has stopped working — the failure mode where every set comes
# back empty and equality holds vacuously.
MIN_MODULES = 10
MIN_REEXPORTS = 20
MIN_SURFACE = 5

_MODULE = re.compile(r"^pub mod ([a-z_0-9]+);", re.M)
_MOD_LINE = re.compile(r"^pub mod ([a-z_0-9]+);")
_ATTR_LINE = re.compile(r"^\s*#!?\[")
_FEATURE_NAME = re.compile(r'feature\s*=\s*"([a-z_0-9-]+)"')
# A re-export names its module, then any number of further path segments, then
# either one name or a brace group. The middle segments matter because
# `pub use projection::grid::GridGeometry;` is a legal refactor of core, and a
# pattern that could not read it would report the *use site* as unattributable
# and point the reader at this checker rather than at their change.
_REEXPORT = re.compile(
    r"^pub use ([a-z_0-9]+)::(?:[a-z_0-9]+::)*(\{.*?\}|[A-Za-z_][A-Za-z_0-9]*)\s*;",
    re.M | re.S,
)
_LEAF = re.compile(r"[A-Za-z_][A-Za-z_0-9]*")
_USE = re.compile(r"fieldglass_core\s*::\s*")
# `use fieldglass_core as fgc;` renames the crate, and every later `fgc::…` is
# invisible to `_USE`. Reported rather than followed: following an alias means
# resolving Rust name binding, and the repo has no such alias.
_ALIAS = re.compile(r"\buse\s+fieldglass_core\s+as\s+([A-Za-z_][A-Za-z_0-9]*)\s*;")
# `#[cfg(test)] mod tests { … }` inside `src/` compiles with the crate's
# dev-dependency features on — which for `fieldglass-grib1` means `analysis`.
# Counting it would admit a gated module to "the parsing surface", the very
# thing scanning `tests/` was excluded to avoid.
_CFG_TEST_BLOCK = re.compile(r"#\[cfg\(test\)\]\s*(?:#\[[^\n]*\]\s*)*mod\s+[a-z_0-9]+\s*\{")
_CFG_TEST_FILE = re.compile(r"#\[cfg\(test\)\]\s*(?:#\[[^\n]*\]\s*)*mod\s+([a-z_0-9]+)\s*;")
_REGION = re.compile(
    r"<!--\s*parsing-surface\b.*?-->(.*?)<!--\s*/parsing-surface\s*-->", re.S
)
_TICKED = re.compile(r"\[?`([a-z_0-9]+)`\]?")


def core_modules(lib_rs: str) -> dict[str, str | None]:
    """Every `pub mod` in core's lib.rs, mapped to the feature gating it or None.

    The module list comes from the stripped source, so a `pub mod` inside a
    comment is not one. The *gate* is read from the raw source, because the
    feature name lives in a string literal and the lexer blanks literals —
    reading `#[cfg(feature = "render")]` from the stripped text finds
    `#[cfg(feature = )]` and reports every gated module as ungated.

    Gates are found by walking back over the attribute lines above each
    declaration rather than by matching one exact `#[cfg]` spelling. Any
    attribute mentioning `feature` counts, so `#[cfg(any(feature = "render",
    feature = "analysis"))]` and a `#[cfg]` with a `#[doc(hidden)]` between it
    and the `pub mod` are both gates. Reading one of those as ungated would make
    this checker tell someone to document a feature-gated module as part of a
    surface whose whole definition is that nothing in it is gated. A `feature`
    attribute this cannot name is still a gate, reported by its own text.
    """
    declared = set(_MODULE.findall(strip_comments_and_literals(lib_rs)))
    gated: dict[str, str] = {}
    lines = lib_rs.splitlines()
    for index, line in enumerate(lines):
        match = _MOD_LINE.match(line)
        if match is None or match.group(1) not in declared:
            continue
        attributes = []
        back = index - 1
        while back >= 0 and _ATTR_LINE.match(lines[back]):
            attributes.append(lines[back])
            back -= 1
        for attribute in attributes:
            if "feature" not in attribute:
                continue
            names = _FEATURE_NAME.findall(attribute)
            gated[match.group(1)] = ", ".join(names) if names else attribute.strip()
            break
    return {module: gated.get(module) for module in sorted(declared)}


def root_reexports(lib_rs: str) -> tuple[dict[str, str], list[str]]:
    """Names re-exported at core's crate root, mapped to the module they come from.

    A format crate writes `fieldglass_core::GlobalGrid`, not
    `fieldglass_core::global_grid::GlobalGrid`, so without this the use is
    unattributable — and an unattributable use is reported as a failure, never
    dropped.

    The second return value is the collisions: one name re-exported from two
    modules would otherwise resolve to whichever `pub use` line came last, and
    could move a module in or out of the measured surface with no diagnostic.
    Core has none today, and this is what keeps that true.
    """
    found: dict[str, str] = {}
    collisions: list[str] = []
    for module, names in _REEXPORT.findall(strip_comments_and_literals(lib_rs)):
        leaves = names[1:-1].split(",") if names.startswith("{") else [names]
        for leaf in leaves:
            match = _LEAF.match(leaf.strip())
            if match is None:
                continue
            name = match.group(0)
            if name in found and found[name] != module:
                collisions.append(f"`{name}` is re-exported from both `{found[name]}` and `{module}`")
            found[name] = module
    return found, collisions


def library_source(path: Path) -> tuple[str, list[str]]:
    """The comment-free source of one library file, minus its `#[cfg(test)]` modules.

    Returns the text and any `#[cfg(test)] mod x;` declarations found, which name
    a *file* this scan does not follow and so are reported rather than ignored.
    """
    text = strip_comments_and_literals(path.read_text(encoding="utf-8"))
    deferred = _CFG_TEST_FILE.findall(text)
    out, cursor = [], 0
    while True:
        block = _CFG_TEST_BLOCK.search(text, cursor)
        if block is None:
            out.append(text[cursor:])
            break
        out.append(text[cursor : block.start()])
        depth, index = 1, block.end()
        while index < len(text) and depth:
            if text[index] == "{":
                depth += 1
            elif text[index] == "}":
                depth -= 1
            index += 1
        cursor = index
    return "".join(out), deferred


def documented_surface(text: str) -> list[str] | None:
    """The core module names inside the `parsing-surface` region, or None if absent.

    Only names that are core modules count, so ordinary backticked prose in the
    region — a type, a feature, a crate — is ignored, while a *gated* module named
    there is picked up and reported.
    """
    region = _REGION.search(text)
    if region is None:
        return None
    return _TICKED.findall(region.group(1))


def _use_leaves(text: str, pos: int) -> list[str]:
    """First path segments of the use-tree at `pos`, expanding one brace group."""
    n = len(text)
    while pos < n and text[pos].isspace():
        pos += 1
    if pos >= n:
        return []
    if text[pos] != "{":
        match = _LEAF.match(text, pos)
        return [match.group(0)] if match else []
    depth, current, parts = 0, [], []
    for index in range(pos, n):
        char = text[index]
        if char == "{":
            depth += 1
            if depth > 1:
                current.append(char)
        elif char == "}":
            depth -= 1
            if depth == 0:
                parts.append("".join(current))
                break
            current.append(char)
        elif char == "," and depth == 1:
            parts.append("".join(current))
            current = []
        else:
            current.append(char)
    leaves = []
    for part in parts:
        match = _LEAF.match(part.strip())
        if match:
            leaves.append(match.group(0))
    return leaves


def modules_used(
    library: Path, modules: dict[str, str | None], reexports: dict[str, str]
) -> tuple[set[str], list[str]]:
    """Core modules named by the `.rs` files under `library`, plus what it could not read.

    Anything that would make the measured surface silently too small is returned
    to be reported rather than swallowed: a name that is neither a module nor a
    crate-root re-export (the re-export map is incomplete), a glob import (which
    hides every name it brings in), an `as`-alias of the crate (after which no
    use is spelled `fieldglass_core::`), a re-export that comes from a
    *dependency* rather than one of core's own modules, and a `#[cfg(test)]`
    module living in its own file (which this scan does not follow).
    """
    used: set[str] = set()
    opaque: list[str] = []
    for path in sorted(library.rglob("*.rs")):
        text, deferred = library_source(path)
        where = f"{library}/{path.relative_to(library)}"
        opaque.extend(
            f"{where}: `#[cfg(test)] mod {name};` is a separate file, which this "
            f"scan does not follow — inline the test module or move it to `tests/`"
            for name in deferred
        )
        opaque.extend(
            f"{where}: `use fieldglass_core as {alias};` renames the crate, so no "
            f"later use of it can be attributed — spell the crate out"
            for alias in _ALIAS.findall(text)
        )
        for match in _USE.finditer(text):
            leaves = _use_leaves(text, match.end())
            if not leaves:
                # `use fieldglass_core::*;` brings in every root re-export under
                # names this scan cannot see.
                opaque.append(
                    f"{where}: `fieldglass_core::*` hides which modules are used — "
                    f"name them"
                )
                continue
            for leaf in leaves:
                if leaf in modules:
                    used.add(leaf)
                elif leaf in reexports:
                    origin = reexports[leaf]
                    if origin in modules:
                        used.add(origin)
                    else:
                        # `pub use some_dep::Thing;` at core's root. Not one of
                        # core's modules, so it is not part of any surface this
                        # checker describes — and looking it up in `modules`
                        # would raise rather than report.
                        opaque.append(
                            f"{where}: `fieldglass_core::{leaf}` is re-exported from "
                            f"`{origin}`, which is not a module of core"
                        )
                elif leaf not in ("self", "crate", "super"):
                    # Reported relative to the library, not to REPO: `check` takes
                    # the tree to scan as an argument, so a path outside this
                    # checkout is an ordinary call and must not raise here.
                    opaque.append(
                        f"{where}: `fieldglass_core::{leaf}` resolves to no core module "
                        f"— the re-export map in check_parsing_surface.py is incomplete"
                    )
    return used, opaque


def check(core: Path = CORE, crates: Path = REPO / "crates") -> list[str]:
    """Every problem found, as printable lines. Empty means the tree is clean."""
    problems: list[str] = []
    lib_rs = (core / "src" / "lib.rs").read_text(encoding="utf-8")
    readme = (core / "README.md").read_text(encoding="utf-8")

    modules = core_modules(lib_rs)
    reexports, collisions = root_reexports(lib_rs)
    problems.extend(
        f"{core}/src/lib.rs: {collision} — one name from two modules cannot be "
        f"attributed to either"
        for collision in collisions
    )
    if len(modules) < MIN_MODULES:
        problems.append(
            f"only {len(modules)} `pub mod` found in {core}/src/lib.rs "
            f"(expected at least {MIN_MODULES}) — the parse has stopped working"
        )
    if len(reexports) < MIN_REEXPORTS:
        problems.append(
            f"only {len(reexports)} crate-root re-exports found in {core}/src/lib.rs "
            f"(expected at least {MIN_REEXPORTS}) — the parse has stopped working"
        )
    if problems:
        # Every set below is derived from these two, so an empty or ambiguous
        # parse would make the comparisons meaningless rather than merely wrong.
        return problems

    used: set[str] = set()
    for name in FORMAT_CRATES:
        library = crates / name / "src"
        if not library.is_dir():
            problems.append(f"{library}: no library to scan — has a format crate moved?")
            continue
        crate_used, opaque = modules_used(library, modules, reexports)
        problems.extend(opaque)
        if not crate_used:
            # A crate that names nothing would shrink the measured surface, and
            # all three of these depend on core.
            problems.append(f"{library}: names no `fieldglass_core::` path at all")
        used |= crate_used

    gated_used = sorted(m for m in used if modules[m] is not None)
    problems.extend(
        f"a format crate library uses `fieldglass_core::{module}`, which is behind "
        f"the `{modules[module]}` feature — it cannot be part of the parsing surface"
        for module in gated_used
    )

    for label, path, text in (
        ("the crate documentation", core / "src" / "lib.rs", lib_rs),
        ("the README", core / "README.md", readme),
    ):
        names = documented_surface(text)
        if names is None:
            problems.append(
                f"{path}: no `<!-- parsing-surface -->` region — {label} states the "
                f"surface, so losing the markers would leave the claim unchecked"
            )
            continue
        listed = {n for n in names if n in modules}
        if len(listed) < MIN_SURFACE:
            problems.append(
                f"{path}: the parsing-surface region names only {len(listed)} core "
                f"modules (expected at least {MIN_SURFACE})"
            )
            continue
        for module in sorted(used - listed):
            problems.append(
                f"{path}: a format crate library uses `fieldglass_core::{module}`, "
                f"which {label} does not list"
            )
        for module in sorted(listed - used):
            problems.append(
                f"{path}: {label} lists `fieldglass_core::{module}`, which no format "
                f"crate library names"
            )
    return problems


def main() -> int:
    problems = check()
    if problems:
        print("The documented parsing surface is not the one in use:\n", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        print(
            "\nBoth `crates/fieldglass-core/src/lib.rs` and "
            "`crates/fieldglass-core/README.md` state this list, and it is the set of "
            "core modules the three format crate *libraries* name. Edit both regions "
            "to match the code, or stop using the module.",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
