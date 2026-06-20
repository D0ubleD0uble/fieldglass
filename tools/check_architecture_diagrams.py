#!/usr/bin/env python3
"""Drift guard for the architecture diagrams under ``docs/architecture/``.

The diagrams (see ``docs/architecture/README.md``) document the workspace at
three altitudes. This script keeps the two mechanically-checkable ones honest:

  * ``01-crates.md`` — the crate dependency flowchart must match the actual
    ``[dependencies]`` path-deps between the workspace crates.
  * ``02-trait-seams.md`` — every ``impl <FirstPartyTrait> for <Type>`` in
    ``crates/*/src`` (a trait declared ``pub trait`` here, excluding
    ``#[cfg(test)]`` mocks) must appear as a ``Trait <|.. Type`` realization
    edge, and every such edge must have a matching impl.
  * ``03-composition.md`` — *partial* guard: every type named as a node in the
    composition diagrams must still exist as a ``pub struct``/``pub enum``/
    ``pub trait`` in the source. This catches renames and deletions; it does
    **not** verify completeness of the section ownership (that stays manual).

Failures are loud and point at the file to edit. Pure text-processing logic
lives in standalone functions (``*_from_text``, ``production_lines``,
``mermaid_blocks``, …) so it can be unit-tested without the filesystem; see
``tools/test_check_architecture_diagrams.py``.

Known, accepted limitations (documented rather than fixed because the cost
outweighs the risk in this codebase):
  * Matching is by *base name* — two distinct types sharing a base name would
    collapse. A collision detector warns when that situation arises.
  * Macro-generated trait impls are invisible to a regex; a NOTE is emitted if
    a ``macro_rules!`` body references a first-party trait in an ``impl``.

Run:
    python3 tools/check_architecture_diagrams.py

Exit status: 0 when the diagrams match the source, 1 on drift. Standard library
only. Wired into pre-commit (commit stage), so it also runs in CI via
``pre-commit run --all-files``.
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

# Mermaid keywords that could appear where a type node is expected. The current
# regexes structurally can't capture these (keywords and quoted cardinality fall
# outside the capture groups), so this is belt-and-suspenders against a future
# regex change — not load-bearing today.
NON_TYPE_TOKENS = {"class", "many", "note", "direction"}

# Built-in / primitive nodes allowed to appear in the composition diagram
# without a `pub struct/enum/trait` declaration backing them.
COMPOSITION_TYPE_ALLOWLIST: set[str] = set()

# `pub trait Name` / `pub struct Name` / `pub enum Name`.
TRAIT_DECL_RE = re.compile(r"\bpub\s+trait\s+([A-Za-z_][A-Za-z0-9_]*)")
PUB_TYPE_RE = re.compile(r"\bpub\s+(?:struct|enum)\s+([A-Za-z_][A-Za-z0-9_]*)")

# `impl<…> path::Trait<…> for path::Type<…>` — capture the bare trait and type
# names, ignoring lifetimes/generics and any module path prefix. Run against a
# *logical* header (continuation lines joined), so multi-line impls match too.
IMPL_RE = re.compile(
    r"^\s*impl\b.*?\b"
    r"(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Za-z_][A-Za-z0-9_]*)"  # trait base name
    r"\s*(?:<[^>]*>)?\s+for\s+"
    r"(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Za-z_][A-Za-z0-9_]*)"  # type base name
)

# A `#[cfg(...)]` attribute whose predicate mentions `test` (covers
# `#[cfg(test)]` and `#[cfg(all(test, feature = "x"))]`).
CFG_TEST_RE = re.compile(r"#\[\s*cfg\s*\([^)]*\btest\b")

_LINE_COMMENT_RE = re.compile(r"//.*$")
_STRING_OR_CHAR_RE = re.compile(r'"(?:\\.|[^"\\])*"' r"|'(?:\\.|[^'\\])'")


def _strip_noise(line: str) -> str:
    """Blank out line comments and string/char literals so brace counting and
    keyword matching don't trip over ``"{"``, ``'}'`` or ``// mod x {``."""
    line = _LINE_COMMENT_RE.sub("", line)
    return _STRING_OR_CHAR_RE.sub("''", line)


def _item_brace_span(lines: list[str], start: int) -> int:
    """Index of the line that closes the item beginning at ``lines[start]``.

    Handles both brace-delimited items (``mod``/``impl``/``struct {{ … }}``) and
    ``;``-terminated ones (``struct X;``, ``use …;``). Returns the index of the
    last line belonging to the item.
    """
    depth = 0
    seen_brace = False
    for k in range(start, len(lines)):
        clean = _strip_noise(lines[k])
        depth += clean.count("{") - clean.count("}")
        if "{" in clean:
            seen_brace = True
        if seen_brace and depth <= 0:
            return k
        if not seen_brace and ";" in clean:
            return k
    return len(lines) - 1


def production_lines(text: str) -> list[str]:
    """Source lines with every ``#[cfg(<…test…>)]``-attributed item removed.

    Test mocks (``struct WideMock; impl PlanarGridProjector for WideMock``) are
    not production seams and must not be required in the diagrams. Generalised
    beyond ``mod`` to any test-gated item (``impl``/``struct``/``fn``/``use``).
    """
    # Assumes the `#[cfg(test)]` attribute is on its own line (rustfmt always
    # splits it from the item). An inline `#[cfg(test)] mod x {` would skip the
    # wrong span — not handled because rustfmt makes it unreachable here.
    lines = text.splitlines()
    out: list[str] = []
    i, n = 0, len(lines)
    while i < n:
        if CFG_TEST_RE.search(lines[i]):
            # Skip following attribute lines to reach the item it gates.
            j = i + 1
            while j < n and (lines[j].strip() == "" or lines[j].lstrip().startswith("#[")):
                j += 1
            if j < n:
                i = _item_brace_span(lines, j) + 1
                continue
        out.append(lines[i])
        i += 1
    return out


def _logical_impl_headers(lines: list[str]) -> list[str]:
    """Yield impl headers as single logical lines (continuations joined).

    rustfmt can wrap a long ``impl`` across lines (generics, ``for`` on the next
    line, ``where`` clauses). Join from ``impl`` until ``{``/``;``/``where`` so
    :data:`IMPL_RE` sees the whole header.
    """
    headers: list[str] = []
    i, n = 0, len(lines)
    while i < n:
        clean = _strip_noise(lines[i])
        if re.match(r"\s*impl\b", clean):
            parts = [clean]
            k = i
            while k < n and not re.search(r"[{;]|\bwhere\b", _strip_noise(lines[k])):
                k += 1
                if k < n:
                    parts.append(_strip_noise(lines[k]).strip())
            headers.append(" ".join(parts))
            i = k + 1
            continue
        i += 1
    return headers


def traits_from_text(text: str) -> set[str]:
    names: set[str] = set()
    for line in production_lines(text):
        m = TRAIT_DECL_RE.search(line)
        if m:
            names.add(m.group(1))
    return names


def pub_types_from_text(text: str) -> set[str]:
    names: set[str] = set()
    for line in production_lines(text):
        for rx in (PUB_TYPE_RE, TRAIT_DECL_RE):
            m = rx.search(line)
            if m:
                names.add(m.group(1))
    return names


def impl_pairs_from_text(text: str, traits: set[str]) -> set[tuple[str, str]]:
    pairs: set[tuple[str, str]] = set()
    for header in _logical_impl_headers(production_lines(text)):
        m = IMPL_RE.match(header)
        if m and m.group(1) in traits:
            pairs.add((m.group(1), m.group(2)))
    return pairs


def mermaid_blocks(text: str) -> list[str]:
    """Return the bodies of all ```` ```mermaid ```` fenced blocks."""
    return re.findall(r"```mermaid\s*\n(.*?)```", text, re.DOTALL)


def edges_from_mermaid(text: str, pattern: re.Pattern[str]) -> set[tuple[str, str]]:
    pairs: set[tuple[str, str]] = set()
    for block in mermaid_blocks(text):
        for m in pattern.finditer(block):
            pairs.add((m.group(1), m.group(2)))
    return pairs


REALIZATION_RE = re.compile(r"([A-Za-z_][A-Za-z0-9_]*)\s*<\|\.\.\s*([A-Za-z_][A-Za-z0-9_]*)")
FLOW_EDGE_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*-->\s*([A-Za-z_][A-Za-z0-9_]*)\b")


def composition_type_nodes(text: str) -> set[str]:
    """Type identifiers used as nodes in the composition diagrams.

    Endpoints of relationship edges (`*--`, `o--`, `..>`, `-->`) and `class X`
    declarations. Cardinality labels and Mermaid keywords are filtered out.
    """
    nodes: set[str] = set()
    rel_re = re.compile(
        r"([A-Za-z_][A-Za-z0-9_]*)\s*(?:\*--|o--|\.\.>|<\.\.|-->|--\|>|<\|\.\.)"
        r"(?:\s*\"[^\"]*\")?\s*([A-Za-z_][A-Za-z0-9_]*)"
    )
    class_re = re.compile(r"\bclass\s+([A-Za-z_][A-Za-z0-9_]*)")
    for block in mermaid_blocks(text):
        for m in rel_re.finditer(block):
            nodes.update({m.group(1), m.group(2)})
        for m in class_re.finditer(block):
            nodes.add(m.group(1))
    return {n for n in nodes if n not in NON_TYPE_TOKENS}


def crate_deps_from_toml(text: str) -> set[str]:
    """First-party crate base names depended on under ``[dependencies]``.

    Section-aware: ignores ``[dev-dependencies]`` / ``[build-dependencies]``.
    """
    deps: set[str] = set()
    in_deps = False
    for raw in text.splitlines():
        line = raw.strip()
        if line.startswith("[") and line.endswith("]"):
            in_deps = line == "[dependencies]"
            continue
        if in_deps:
            m = re.match(r"fieldglass-(core|grib1|grib2|netcdf)\s*=", line)
            if m:
                deps.add(m.group(1))
    return deps


# ── filesystem walking (thin wrappers over the pure functions above) ──────────


def rust_sources() -> list[Path]:
    return sorted(CRATES_DIR.glob("**/src/**/*.rs"))


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def first_party_traits() -> set[str]:
    out: set[str] = set()
    for p in rust_sources():
        out |= traits_from_text(_read(p))
    return out


def all_pub_types() -> set[str]:
    out: set[str] = set()
    for p in rust_sources():
        out |= pub_types_from_text(_read(p))
    return out


def source_realizations(traits: set[str]) -> set[tuple[str, str]]:
    out: set[tuple[str, str]] = set()
    for p in rust_sources():
        out |= impl_pairs_from_text(_read(p), traits)
    return out


def diagram_realizations() -> set[tuple[str, str]]:
    out: set[tuple[str, str]] = set()
    for p in sorted(DIAGRAMS_DIR.glob("*.md")):
        out |= edges_from_mermaid(_read(p), REALIZATION_RE)
    return out


def diagram_crate_edges() -> set[tuple[str, str]]:
    text = _read(DIAGRAMS_DIR / "01-crates.md")
    return edges_from_mermaid(text, FLOW_EDGE_RE)


def actual_crate_edges() -> set[tuple[str, str]]:
    edges: set[tuple[str, str]] = set()
    for toml in CRATES_DIR.glob("fieldglass-*/Cargo.toml"):
        consumer = toml.parent.name.removeprefix("fieldglass-")
        for dep in crate_deps_from_toml(_read(toml)):
            edges.add((consumer, dep))
    return edges


def macro_impl_notes(traits: set[str]) -> list[str]:
    notes: list[str] = []
    for p in rust_sources():
        text = _read(p)
        if "macro_rules!" in text and re.search(r"\bimpl\b", text):
            for t in traits:
                if re.search(rf"macro_rules!.*\bimpl\b[^;{{]*\b{t}\b", text, re.DOTALL):
                    notes.append(f"{p.relative_to(REPO_ROOT)} defines a macro that impls {t}")
    return notes


def type_collisions() -> dict[str, list[str]]:
    seen: dict[str, set[str]] = {}
    for p in rust_sources():
        for name in pub_types_from_text(_read(p)):
            seen.setdefault(name, set()).add(str(p.relative_to(REPO_ROOT)))
    return {n: sorted(files) for n, files in seen.items() if len(files) > 1}


# ── checks ────────────────────────────────────────────────────────────────────


def check_trait_seams(traits: set[str], documented: set[str]) -> list[str]:
    source = {(t, ty) for (t, ty) in source_realizations(traits) if t in documented}
    drawn = {(t, ty) for (t, ty) in diagram_realizations() if t in documented}
    errors = []
    for trait, ty in sorted(source - drawn):
        errors.append(
            f"02-trait-seams.md: impl {trait} for {ty} has no `{trait} <|.. {ty}` edge"
        )
    for trait, ty in sorted(drawn - source):
        errors.append(
            f"02-trait-seams.md: edge `{trait} <|.. {ty}` has no matching impl (stale)"
        )
    return errors


def check_crate_graph() -> list[str]:
    actual = actual_crate_edges()
    drawn = diagram_crate_edges()
    errors = []
    for a, b in sorted(actual - drawn):
        errors.append(f"01-crates.md: dependency {a} --> {b} is missing from the flowchart")
    for a, b in sorted(drawn - actual):
        errors.append(f"01-crates.md: flowchart edge {a} --> {b} is not a real dependency")
    return errors


def check_diagrams_present() -> list[str]:
    """Fail loudly if a diagram file has no parseable ```mermaid``` block.

    Without this, broken fences would make the trait/composition scans pass
    vacuously (empty drawn-sets) for whichever check has no source counterpart.
    """
    errors = []
    for name in ("01-crates.md", "02-trait-seams.md", "03-composition.md"):
        path = DIAGRAMS_DIR / name
        if not path.is_file():
            errors.append(f"{name}: missing")
        elif not mermaid_blocks(_read(path)):
            errors.append(f"{name}: no ```mermaid``` block parsed (broken fence?)")
    return errors


def check_composition_types(known: set[str]) -> list[str]:
    text = _read(DIAGRAMS_DIR / "03-composition.md")
    nodes = composition_type_nodes(text)
    unknown = nodes - known - COMPOSITION_TYPE_ALLOWLIST
    return [
        f"03-composition.md: node `{n}` is not a known pub struct/enum/trait "
        f"(renamed or deleted?)"
        for n in sorted(unknown)
    ]


def main() -> int:
    if not DIAGRAMS_DIR.is_dir():
        print(f"error: {DIAGRAMS_DIR} not found", file=sys.stderr)
        return 1

    traits = first_party_traits()
    documented = traits - UNDIAGRAMMED_TRAITS
    known_types = all_pub_types()

    errors = check_diagrams_present()
    if not errors:
        errors = (
            check_crate_graph()
            + check_trait_seams(traits, documented)
            + check_composition_types(known_types)
        )

    # Non-fatal advisories.
    for note in macro_impl_notes(traits):
        print(f"NOTE: {note} — macro impls are invisible to this check; "
              f"verify 02-trait-seams.md by hand.", file=sys.stderr)
    for name, files in sorted(type_collisions().items()):
        print(f"WARNING: type name `{name}` is declared in multiple files "
              f"({', '.join(files)}); base-name matching may conflate them.",
              file=sys.stderr)

    if not errors:
        n_real = len({(t, ty) for (t, ty) in source_realizations(traits) if t in documented})
        print(
            f"architecture diagrams OK — crate graph, {n_real} trait "
            f"realization(s) across {len(documented)} trait(s), and composition "
            f"nodes all match the source."
        )
        return 0

    print("Architecture diagrams are out of sync with the source:\n")
    for e in errors:
        print(f"  - {e}")
    print(
        "\nUpdate docs/architecture/ (see its README). For a deliberately "
        "undocumented first-party trait, add it to UNDIAGRAMMED_TRAITS in this "
        "script.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
