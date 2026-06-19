#!/usr/bin/env python3
"""Drift guard for the architecture diagrams under ``docs/architecture/``.

The diagrams (see ``docs/architecture/README.md``) document the trait seams of
the workspace — the traits defined here and the structs that implement them.
That documentation rots silently: add an ``impl Grib1Packing for FooPacking``
and forget the diagram, and the picture is quietly wrong.

This script makes that failure loud. It:

  1. Collects every *first-party* trait — one declared ``pub trait NAME`` in
     ``crates/*/src`` — so std / derive / foreign-trait impls are ignored.
  2. Collects every ``impl <Trait> for <Type>`` in ``crates/*/src`` whose trait
     is first-party.
  3. Requires each such realization to appear as a UML realization edge
     (``Trait <|.. Type``) somewhere under ``docs/architecture/``.

A realization that no diagram mentions fails the check. The reverse — a diagram
edge with no matching ``impl`` — also fails, catching stale edges left behind
after a refactor.

If a first-party trait is deliberately undocumented (not a "seam"), add its
name to ``UNDIAGRAMMED_TRAITS`` below with a comment saying why; that is the one
sanctioned escape hatch.

Run:
    python3 tools/check_architecture_diagrams.py

Exit status: 0 when the diagrams match the source, 1 on drift. No third-party
dependencies; standard library only. Wired into pre-commit (commit stage), so
it also runs in CI via ``pre-commit run --all-files``.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = REPO_ROOT / "crates"
DIAGRAMS_DIR = REPO_ROOT / "docs" / "architecture"

# First-party traits intentionally left out of the diagrams. Keep empty unless
# you have a reason; add `"TraitName",  # why` rather than deleting the trait
# from the source-of-truth scan.
UNDIAGRAMMED_TRAITS: set[str] = set()

# `pub trait Name` — optional generics/`: Supertrait` bound follow the name.
TRAIT_DECL_RE = re.compile(r"\bpub\s+trait\s+([A-Za-z_][A-Za-z0-9_]*)")

# `impl<…> path::Trait<…> for path::Type<…>` — capture the bare trait and type
# names, ignoring lifetimes/generics and any module path prefix.
IMPL_RE = re.compile(
    r"^\s*impl\s*(?:<[^>]*>)?\s+"
    r"(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Za-z_][A-Za-z0-9_]*)"  # trait base name
    r"\s*(?:<[^>]*>)?\s+for\s+"
    r"(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Za-z_][A-Za-z0-9_]*)"  # type base name
)


def rust_sources() -> list[Path]:
    return sorted(CRATES_DIR.glob("**/src/**/*.rs"))


def production_lines(text: str) -> list[str]:
    """Source lines with `#[cfg(test)] mod …` blocks removed.

    Test mocks (`struct WideMock; impl PlanarGridProjector for WideMock`) are
    not production seams and must not be required in the diagrams. Brace
    counting is naive — good enough for these files, in the spirit of the
    other heuristic tools in this directory.
    """
    lines = text.splitlines()
    out: list[str] = []
    i, n = 0, len(lines)
    while i < n:
        if lines[i].strip().startswith("#[cfg(test)]"):
            j = i + 1
            while j < n and lines[j].strip() == "":
                j += 1
            if j < n and lines[j].lstrip().startswith("mod "):
                depth, started, k = 0, False, j
                while k < n:
                    depth += lines[k].count("{") - lines[k].count("}")
                    started = started or "{" in lines[k]
                    if started and depth <= 0:
                        break
                    k += 1
                i = k + 1
                continue
        out.append(lines[i])
        i += 1
    return out


def first_party_traits() -> set[str]:
    traits: set[str] = set()
    for path in rust_sources():
        for line in production_lines(path.read_text(encoding="utf-8")):
            m = TRAIT_DECL_RE.search(line)
            if m:
                traits.add(m.group(1))
    return traits


def source_realizations(traits: set[str]) -> set[tuple[str, str]]:
    """(trait, type) pairs for every impl of a first-party trait."""
    pairs: set[tuple[str, str]] = set()
    for path in rust_sources():
        for line in production_lines(path.read_text(encoding="utf-8")):
            m = IMPL_RE.match(line)
            if m and m.group(1) in traits:
                pairs.add((m.group(1), m.group(2)))
    return pairs


def diagram_realizations() -> set[tuple[str, str]]:
    """(trait, type) pairs drawn as `Trait <|.. Type` across the diagrams."""
    edge_re = re.compile(
        r"([A-Za-z_][A-Za-z0-9_]*)\s*<\|\.\.\s*([A-Za-z_][A-Za-z0-9_]*)"
    )
    pairs: set[tuple[str, str]] = set()
    for path in sorted(DIAGRAMS_DIR.glob("*.md")):
        for m in edge_re.finditer(path.read_text(encoding="utf-8")):
            pairs.add((m.group(1), m.group(2)))
    return pairs


def main() -> int:
    if not DIAGRAMS_DIR.is_dir():
        print(f"error: {DIAGRAMS_DIR} not found", file=sys.stderr)
        return 1

    traits = first_party_traits()
    documented = traits - UNDIAGRAMMED_TRAITS

    source = {(t, ty) for (t, ty) in source_realizations(traits) if t in documented}
    drawn = diagram_realizations()
    # Only judge edges for traits we expect to document.
    drawn_documented = {(t, ty) for (t, ty) in drawn if t in documented}

    missing = source - drawn_documented  # impl exists, no diagram edge
    stale = drawn_documented - source  # diagram edge, no impl

    if not missing and not stale:
        print(
            f"architecture diagrams OK — {len(source)} trait realization(s) "
            f"across {len(documented)} documented trait(s) match the source."
        )
        return 0

    if missing:
        print("Trait realizations in the source but MISSING from the diagrams:")
        for trait, ty in sorted(missing):
            print(f"  impl {trait} for {ty}  →  add `{trait} <|.. {ty}` to a diagram")
    if stale:
        print("Realization edges in the diagrams with NO matching impl (stale):")
        for trait, ty in sorted(stale):
            print(f"  {trait} <|.. {ty}  →  remove it, or restore the impl")
    print(
        "\nFix docs/architecture/ (or, for a deliberately-undocumented trait, "
        "add it to UNDIAGRAMMED_TRAITS in this script).",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
