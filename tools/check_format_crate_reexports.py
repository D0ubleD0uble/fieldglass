#!/usr/bin/env python3
"""Fail if a format crate names a `fieldglass-core` type in its own public API
without re-exporting it.

    python3 tools/check_format_crate_reexports.py

`docs/architecture/01-crates.md` records the rule: a format crate re-exports
exactly the `fieldglass_core` names that appear in its *own* public signatures.
The reason is a consumer's manifest. `fieldglass-grib1` returns
`fieldglass_core::FieldglassError` from every fallible call, so writing a
`match` over it means naming the type — and if the crate does not re-export it,
the only way to name it is to add `fieldglass-core` to your own `[dependencies]`.
A line written for one type is written without `default-features = false`, which
unifies `render` and `fs` back on for everything in the graph, the browser
build included.

**What each gate covers.** `tests/crate-independence` is a package that depends
on the three format crates and deliberately not on `core`, so a re-export that
is *removed* stops compiling. Nothing caught one that is never *added*: a new
`pub fn` returning a core type compiles fine, says nothing, and the cost lands
on a consumer. #537's one-off scan found three such misses in grib2 that manual
review had already passed over. This checker is that scan, run every commit.

**How it decides.** Per file, the `fieldglass_core` names in scope come from
that file's own `use` statements, plus any `fieldglass_core::…::Name` path
written out in place of an import. A name is *required* when it appears in a
public signature: the header of a `pub fn` / `pub struct` / `pub enum` /
`pub trait` / `pub type` / `pub const` / `pub static`, a `pub` field of a public
struct, an enum variant payload, a method signature inside a `pub trait`, an
`impl … for …` header, or any item inside one of those impls. The last two
carry most of the weight. A trait impl is reachable from anywhere both its
types are, which is how `GridGeometry` enters both GRIB crates' API (via
`From<&GridDescription>`) without any `pub fn` naming it, and its items are the
trait's signatures rather than private helpers — `type Error = FieldglassError;`
on a `TryFrom` impl is a name a consumer has to write and appears nowhere else.
Items in an *inherent* impl are only public when they say `pub`, and they are
reachable when the type the impl hangs them on is, which is not the same
question as whether the method's own name is re-exported.

A `pub` item in a private module counts only when a public module re-exports it
— by name, by its declared name behind a rename, or through a `pub use mod::*;`
that publishes the module whole.

Accepted limitations, in the shape `check_architecture_diagrams.py` documents
its own:

  * Matching is by *base name*. A local type sharing a name with a core one
    would be reported; there is none today.
  * A signature spread through a macro body is invisible to a regex. Say so
    here rather than reaching for a real parser.
  * Module visibility is traced through `mod` / `pub mod` declarations in
    files. `#[path]` is not resolved — and because skipping a file would make
    this gate fail open, every `.rs` under a crate's `src/` that the walk does
    not reach is reported as an error rather than ignored. A private inline
    `mod x { … }` is not resolved either, but its items are still scanned under
    the file's own visibility, so that one over-reports rather than under-.
    Every inline module in the three crates today is `#[cfg(test)]`, which is
    dropped before any of this.
  * `use fieldglass_core::*;`, `use fieldglass_core as …;` and its in-tree
    spelling `use fieldglass_core::{self as fc, …};` are rejected outright for
    the same reason: the checker cannot see through any of them, and each would
    silently blind it.

Line numbers in every message are line numbers in the file. `#[cfg(test)]` items
are blanked in place rather than removed for exactly that reason — one of these
crates carries 1,496 lines of test module in a single file.

`ALLOWED_UNEXPORTED` is the line the rule draws, per crate and per name, and is
checked in both directions — an entry naming something that is no longer in a
public signature is a failure too, so the allowlist cannot outlive its reason.

**The converse is deliberately not checked.** A re-export that no signature
needs is not reported, because two real ones exist for reasons a signature
cannot express: `fieldglass_core::expand_reduced_to_regular` lives in core
because both GRIB editions' reduced grids need it (#503) and stays on grib1's
path because callers of that crate have it, and `cct_tables::lookup_sub_centre`
is re-exported beside each edition's own centre table so a consumer rendering a
message header does not have to know which crate each half came from. Enforcing
"nothing extra" would mean a second allowlist holding those two and would catch
nothing that `tests/crate-independence` does not already pin.

Pure text-processing logic lives in standalone functions so it can be
unit-tested without the filesystem; see
`tools/test_check_format_crate_reexports.py`.

Exit status: 0 when every crate re-exports what its signatures name, 1
otherwise. Standard library only. Wired into pre-commit (commit stage), so it
also runs in CI via `pre-commit run --all-files`.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CRATES_DIR = REPO_ROOT / "crates"

# The three crates the rule is about: they depend on `fieldglass-core` and are
# meant to be usable as a downstream crate's only Fieldglass dependency.
FORMAT_CRATES = ("fieldglass-grib1", "fieldglass-grib2", "fieldglass-netcdf")

# Core names allowed in a public signature without a re-export, per crate, with
# the reason. Checked in both directions: a name listed here that no longer
# appears in a public signature is a stale entry and fails.
#
# `GridGeometry`'s payload structs (`GaussianParams`, `LatLonParams`, …) are the
# case this list exists for, and they are *not* here: they are core's own API,
# reached by destructuring the enum, and no format-crate signature names one. If
# one ever does, that is a genuine finding, not something to excuse — the three
# parameter structs grib2 hands back by value (`LambertAzimuthalParams`,
# `TransverseMercatorParams`, `GeostationaryParams`) are re-exported for exactly
# that reason.
#
# Empty, and the default answer to a new finding is a re-export rather than an
# entry here. Its only occupants so far were the four names grib2's `FormatReader`
# / `DataMessage` stub impls dragged into its public API, and #540 deleted the
# stubs instead of excusing them. Add `"Name": "why",` only when the name is one
# the crate does not hand back on its own account.
ALLOWED_UNEXPORTED: dict[str, dict[str, str]] = {}

# A `#[cfg(...)]` attribute whose predicate mentions `test` (covers
# `#[cfg(test)]` and `#[cfg(all(test, feature = "x"))]`).
CFG_TEST_RE = re.compile(r"#\[\s*cfg\s*\([^)]*\btest\b")

# `mod x;` / `pub mod x;` / `pub(crate) mod x;` — group 1 present means some
# `pub`, group 2 present means it is restricted and therefore not public API.
MOD_DECL_RE = re.compile(r"^\s*(pub\s*(?:\(\s*([^)]*?)\s*\))?\s+)?mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")

# A fully-public item declaration. `pub(crate)` / `pub(super)` do not match:
# `pub` must be followed by whitespace, not `(`.
PUB_ITEM_RE = re.compile(
    r"^\s*pub\s+(?:async\s+|unsafe\s+|const\s+|extern\s+\"[^\"]*\"\s+)*"
    r"(fn|struct|enum|trait|type|const|static|union)\s+([A-Za-z_][A-Za-z0-9_]*)"
)

# A `pub` struct field: `pub name: Type,`.
PUB_FIELD_RE = re.compile(r"^\s*pub\s+([A-Za-z_][A-Za-z0-9_]*)\s*:")

# A method / associated item inside a `pub trait` body. Trait items are as
# public as the trait.
TRAIT_ITEM_RE = re.compile(r"^\s*(?:async\s+|unsafe\s+|const\s+|extern\s+\"[^\"]*\"\s+)*(fn|type|const)\s+")

IMPL_HEADER_RE = re.compile(r"^\s*(?:unsafe\s+)?impl\b")

USE_START_RE = re.compile(r"^\s*(?:pub\s*(?:\([^)]*\))?\s+)?use\s+fieldglass_core\s*::")
PUB_USE_RE = re.compile(r"^\s*pub\s+use\s+")

IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

# `fieldglass_core::projection::LambertParams` written out in a signature
# instead of imported. Rarer than a `use`, and just as much a name a consumer
# has to be able to write.
INLINE_PATH_RE = re.compile(r"\bfieldglass_core\s*::\s*(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*([A-Za-z_][A-Za-z0-9_]*)")

# `use fieldglass_core as core;` renames the crate, after which every path
# through it is invisible to the scan above. Rejected for the same reason a
# glob is: a check that cannot see the names must say so, not pass.
CRATE_RENAME_RE = re.compile(r"^\s*(?:pub\s*(?:\([^)]*\))?\s+)?use\s+fieldglass_core\s+as\s+")

# The same rename spelled from inside the tree: `use fieldglass_core::{self as
# fc, …};`. `self` binds the crate, so `fc::GridGeometry` is the renamed form
# and is just as invisible.
SELF_RENAME_RE = re.compile(r"\bself\s+as\s+[A-Za-z_]")

_IMPL_KEYWORD_RE = re.compile(r"^\s*(?:unsafe\s+)?impl\b\s*(?:<[^>]*>)?")

_LINE_COMMENT_RE = re.compile(r"//.*$")
_STRING_OR_CHAR_RE = re.compile(r'"(?:\\.|[^"\\])*"' r"|'(?:\\.|[^'\\])'")


def strip_noise(line: str) -> str:
    """Blank out line comments and string/char literals.

    Without it a doc link (``/// see [`GridGeometry`]``) or a literal ``"{"``
    would be read as source.
    """
    return _STRING_OR_CHAR_RE.sub("''", _LINE_COMMENT_RE.sub("", line))


def _item_span(lines: list[str], start: int) -> int:
    """Index of the last line of the item beginning at ``lines[start]``.

    Handles a brace-delimited item (``impl`` / ``struct { … }``) and a
    ``;``-terminated one (``struct X;``, ``use …;``) alike. A run-away scan here
    would swallow the rest of the file, so callers that mean "the block this
    header opened" must first check that the header really opened one (see
    :func:`_logical_header`).
    """
    depth = 0
    seen_brace = False
    for k in range(start, len(lines)):
        clean = strip_noise(lines[k])
        depth += clean.count("{") - clean.count("}")
        if "{" in clean:
            seen_brace = True
        if seen_brace and depth <= 0:
            return k
        if not seen_brace and ";" in clean:
            return k
    return len(lines) - 1


def masked_lines(text: str) -> list[str]:
    """Source lines with every ``#[cfg(<…test…>)]`` item blanked *in place*.

    A test mock's `impl` is not public API, so it must not be scanned. Blanked
    rather than deleted because every message this checker prints carries a line
    number: `crates/fieldglass-grib2/src/ds.rs` alone is 1,496 lines of test
    module, so a list with those lines removed would send a reader to a
    completely different part of the file.

    Assumes the attribute sits on its own line, which rustfmt guarantees.
    """
    lines = text.splitlines()
    out = list(lines)
    i, n = 0, len(lines)
    while i < n:
        if CFG_TEST_RE.search(lines[i]):
            # Skip any further attribute lines to reach the item they gate.
            j = i + 1
            while j < n and (lines[j].strip() == "" or lines[j].lstrip().startswith("#[")):
                j += 1
            if j < n:
                end = _item_span(out, j)
                for k in range(i, end + 1):
                    out[k] = ""
                i = end + 1
                continue
        i += 1
    return out


def _logical_header(lines: list[str], start: int, stop: str = "{;=,") -> tuple[str, int, str]:
    """Join an item header from ``start`` up to what ends it.

    rustfmt wraps a long signature across lines, so the whole header has to be
    one string before any name in it can be read. Returns the joined text, the
    index of its last line, and the character that ended it: ``{`` for an item
    with a body, ``;`` for a declaration, ``=`` for an initialiser, ``,`` for a
    struct field or enum variant, and ``""`` if the file ran out. ``stop``
    narrows that set — a `type` alias names its target after the ``=``, so ``=``
    must not end its header.

    Angle brackets are counted as nesting so a generic parameter list is not
    mistaken for the end of a field — with ``->`` blanked first, since its
    ``>`` closes nothing.
    """
    parts: list[str] = []
    depth = 0
    k = start
    while k < len(lines):
        clean = strip_noise(lines[k])
        parts.append(clean.strip())
        for ch in clean.replace("->", "  "):
            if ch in "([<":
                depth += 1
            elif ch in ")]>":
                depth -= 1
            elif depth <= 0 and ch in stop:
                return " ".join(parts), k, ch
        k += 1
    return " ".join(parts), len(lines) - 1, ""


def _split_top_level(body: str) -> list[str]:
    """Split a `use` tree body on commas outside any nested braces."""
    out: list[str] = []
    depth = 0
    current: list[str] = []
    for ch in body:
        if ch == "{":
            depth += 1
        elif ch == "}":
            depth -= 1
        if ch == "," and depth == 0:
            out.append("".join(current))
            current = []
        else:
            current.append(ch)
    out.append("".join(current))
    return [part.strip() for part in out if part.strip()]


def flatten_use_tree(body: str) -> list[tuple[str, str]]:
    """`(binding, core name)` leaves of a `use fieldglass_core::<body>` tree.

    Handles nesting (`{FieldglassError, bits::{BitReader, …}}`), module paths
    (`bits::ibm_float_to_f64`) and renames (`FieldglassError as CoreError`).
    `self` leaves bind a module, not a name, and are dropped.
    """
    body = body.strip()
    if not body:
        return []
    if body.startswith("{") and body.endswith("}"):
        leaves: list[tuple[str, str]] = []
        for part in _split_top_level(body[1:-1]):
            leaves.extend(flatten_use_tree(part))
        return leaves
    if "{" in body:
        # `path::{…}` — the path prefix names modules, so only the tree matters.
        return flatten_use_tree(body[body.index("{") :])
    match = re.fullmatch(
        r"(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*([A-Za-z_][A-Za-z0-9_]*)"
        r"(?:\s+as\s+([A-Za-z_][A-Za-z0-9_]*))?",
        body,
    )
    if not match:
        return []
    name, alias = match.group(1), match.group(2)
    if name == "self":
        return []
    return [(alias or name, name)]


def core_use_statements(lines: list[str]) -> list[tuple[int, bool, str]]:
    """`(line number, is_pub_use, tree body)` for each `use fieldglass_core::…`.

    Statements are joined across lines first: rustfmt wraps a long import list,
    and half of one parses as an empty tree.
    """
    out: list[tuple[int, bool, str]] = []
    i = 0
    while i < len(lines):
        if USE_START_RE.match(strip_noise(lines[i])):
            is_pub = bool(PUB_USE_RE.match(strip_noise(lines[i])))
            buffer = strip_noise(lines[i]).strip()
            k = i
            while ";" not in buffer and k + 1 < len(lines):
                k += 1
                buffer += " " + strip_noise(lines[k]).strip()
            body = buffer.split("fieldglass_core", 1)[1].split(";", 1)[0]
            out.append((i + 1, is_pub, body.lstrip().removeprefix("::")))
            i = k
        i += 1
    return out


def core_names_from_text(text: str) -> tuple[dict[str, str], set[str], list[int]]:
    """What one file says about `fieldglass_core`.

    Returns the names it brings into scope (binding → core name), the ones it
    re-exports with `pub use`, and the line numbers of any import that hides
    what it brings in — a glob, or a rename of the crate itself. The caller must
    treat those as failures: past either one the scan sees nothing.
    """
    in_scope: dict[str, str] = {}
    re_exported: set[str] = set()
    opaque: list[int] = []
    lines = masked_lines(text)
    for line_no, is_pub, body in core_use_statements(lines):
        if "*" in body or SELF_RENAME_RE.search(body):
            opaque.append(line_no)
            continue
        for binding, name in flatten_use_tree(body):
            in_scope[binding] = name
            if is_pub:
                re_exported.add(name)
    for line_no, line in enumerate(lines, start=1):
        if CRATE_RENAME_RE.match(strip_noise(line)):
            opaque.append(line_no)
    return in_scope, re_exported, sorted(opaque)


def local_reexported_names(text: str) -> tuple[set[str], set[str]]:
    """What a file publishes from elsewhere in its own crate.

    A `pub` item in a private module is public API only when something reaches
    out and re-exports it, which is how `fieldglass-grib2` publishes
    `tables_local::LocalTableCentre` from a private module. Returns the item
    names — both the declared name and any alias, since a rename leaves the
    declaration under its own name — and separately the modules re-exported
    whole with a glob, which makes everything public in them.
    """
    names: set[str] = set()
    glob_modules: set[str] = set()
    lines = masked_lines(text)
    i = 0
    while i < len(lines):
        clean = strip_noise(lines[i])
        if PUB_USE_RE.match(clean) and not USE_START_RE.match(clean):
            buffer = clean.strip()
            k = i
            while ";" not in buffer and k + 1 < len(lines):
                k += 1
                buffer += " " + strip_noise(lines[k]).strip()
            body = buffer.split("use", 1)[1].split(";", 1)[0]
            for binding, name in flatten_use_tree(body):
                names.update({binding, name})
            glob_modules.update(re.findall(r"([A-Za-z_][A-Za-z0-9_]*)\s*::\s*\*", body))
            i = k
        i += 1
    return names, glob_modules


def impl_self_type(header: str) -> str | None:
    """The base name of the type an `impl` header hangs items on.

    `impl<'a> From<&GridDescription> for GridGeometry` gives `GridGeometry`;
    `impl LambertAzimuthalTemplate` gives the template. Used to ask whether the
    methods inside are reachable, which turns on the *type's* name and not on
    each method's.
    """
    body = header.split("{", 1)[0].split(" where ", 1)[0]
    tail = _IMPL_KEYWORD_RE.sub("", body.rsplit(" for ", 1)[-1])
    match = re.search(r"(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*([A-Za-z_][A-Za-z0-9_]*)", tail)
    return match.group(1) if match else None


def public_signatures(text: str, *, module_is_public: bool, reexported: set[str]) -> list[tuple[int, str]]:
    """`(line number, joined signature)` for everything a consumer must name.

    `module_is_public` says whether the file's module is reachable through
    `pub mod`; when it is not, an item counts only if it — or, for a method, the
    type its `impl` block hangs it on — is `reexported` by name from a public
    module. `impl … for …` headers count either way: a trait impl is visible
    wherever both of its types are, regardless of where it is written, and every
    item inside one is as reachable as the impl, which is how
    `type Error = FieldglassError;` becomes a name a consumer has to write.
    """
    lines = masked_lines(text)
    signatures: list[tuple[int, str]] = []
    # `(last line of the block, self type)` for each `impl` we are inside.
    impl_stack: list[tuple[int, str | None]] = []
    i = 0
    while i < len(lines):
        while impl_stack and impl_stack[-1][0] < i:
            impl_stack.pop()
        clean = strip_noise(lines[i])

        if IMPL_HEADER_RE.match(clean):
            header, end, terminator = _logical_header(lines, i, stop="{;")
            signatures.append((i + 1, header))
            block_end = _item_span(lines, end) if terminator == "{" else end
            if " for " in f" {header} ":
                # A trait impl: its items are the trait's signatures, not
                # private helpers, so all of them count.
                for k in range(end + 1, block_end):
                    if TRAIT_ITEM_RE.match(strip_noise(lines[k])):
                        signatures.append((k + 1, _logical_header(lines, k, stop="{;")[0]))
                i = block_end + 1
                continue
            # An inherent impl: descend rather than skip, because its `pub fn`s
            # are where three of grib2's §3 templates hand a core parameter
            # struct back.
            impl_stack.append((block_end, impl_self_type(header)))
            i = end + 1
            continue

        item = PUB_ITEM_RE.match(clean)
        if not item:
            i += 1
            continue

        kind, name = item.group(1), item.group(2)
        # An alias puts the type it names on the far side of the `=`, so that is
        # not where its header ends. No item header ends at a top-level comma
        # either — that only happens in a `where` clause rustfmt has wrapped.
        header, end, terminator = _logical_header(lines, i, stop="{;" if kind == "type" else "{;=")
        body_end = _item_span(lines, end) if terminator == "{" else end
        owner = impl_stack[-1][1] if impl_stack else None
        if module_is_public or name in reexported or (owner is not None and owner in reexported):
            # The item's own name is dropped: a `pub fn` may legitimately share
            # a name with something imported from core without naming the type.
            signatures.append((i + 1, re.sub(rf"\b{re.escape(name)}\b", "", header, count=1)))
            if kind == "struct":
                for k in range(end + 1, body_end):
                    if PUB_FIELD_RE.match(strip_noise(lines[k])):
                        signatures.append((k + 1, _logical_header(lines, k)[0]))
            elif kind == "enum":
                # Every variant of a public enum is public, payloads included.
                for k in range(end + 1, body_end):
                    signatures.append((k + 1, strip_noise(lines[k])))
            elif kind == "trait":
                for k in range(end + 1, body_end):
                    if TRAIT_ITEM_RE.match(strip_noise(lines[k])):
                        signatures.append((k + 1, _logical_header(lines, k, stop="{;")[0]))
        i = body_end + 1
    return signatures


def module_files(src_dir: Path) -> tuple[list[tuple[Path, bool]], set[str]]:
    """Every source file reachable from `lib.rs`, with whether it is public API.

    Also returns the item names `lib.rs` and the other public modules re-export
    from elsewhere in the crate, which is what makes a `pub` item in a private
    module reachable.
    """
    reached: list[tuple[Path, bool]] = []
    reexported: set[str] = set()
    seen: set[Path] = set()
    queue: list[tuple[Path, bool]] = [(src_dir / "lib.rs", True)]
    while queue:
        path, is_public = queue.pop()
        if path in seen or not path.is_file():
            continue
        seen.add(path)
        reached.append((path, is_public))
        text = path.read_text(encoding="utf-8")
        names, glob_modules = local_reexported_names(text)
        if is_public:
            reexported |= names
        child_dir = path.parent if path.name in ("lib.rs", "mod.rs") else path.parent / path.stem
        for line in masked_lines(text):
            decl = MOD_DECL_RE.match(strip_noise(line))
            if not decl:
                continue
            declared_public = bool(decl.group(1)) and not decl.group(2)
            # `pub use tables::*;` publishes a private module's items wholesale,
            # so the module is as public as a `pub mod` would have made it.
            child_public = is_public and (declared_public or decl.group(3) in glob_modules)
            for candidate in (child_dir / f"{decl.group(3)}.rs", child_dir / decl.group(3) / "mod.rs"):
                if candidate.is_file():
                    queue.append((candidate, child_public))
                    break
    return reached, reexported


def check_crate(crate: str, src_dir: Path) -> list[str]:
    """Every problem this crate has, as messages naming the file and the line."""
    problems: list[str] = []
    files, reexported_items = module_files(src_dir)

    # Both sides are built from `src_dir`, so they compare without resolving —
    # and resolving only one of them would break the message's `relative_to`
    # under a symlinked root.
    unreached = sorted(set(src_dir.rglob("*.rs")) - {path for path, _ in files})
    for path in unreached:
        problems.append(
            f"{path.relative_to(REPO_ROOT)}: not reachable from lib.rs through "
            f"`mod` declarations, so this checker never read it — resolve the "
            f"declaration (`#[path]`? an inline `mod`?) or the gate fails open here"
        )

    core_reexports: set[str] = set()
    for path, is_public in files:
        _, re_exported, _ = core_names_from_text(path.read_text(encoding="utf-8"))
        if is_public:
            core_reexports |= re_exported

    allowed = ALLOWED_UNEXPORTED.get(crate, {})
    required: dict[str, tuple[Path, int, str]] = {}
    for path, is_public in files:
        text = path.read_text(encoding="utf-8")
        in_scope, _, opaque = core_names_from_text(text)
        for line_no in opaque:
            problems.append(
                f"{path.relative_to(REPO_ROOT)}:{line_no}: a glob or renamed import of "
                f"fieldglass_core hides which core names this file uses, and this check "
                f"cannot see through it — import the names explicitly"
            )
        for line_no, signature in public_signatures(
            text, module_is_public=is_public, reexported=reexported_items
        ):
            named = {in_scope[ident] for ident in IDENT_RE.findall(signature) if ident in in_scope}
            named |= set(INLINE_PATH_RE.findall(signature))
            for name in sorted(named):
                if name not in required:
                    required[name] = (path, line_no, signature.strip())

    for name, (path, line_no, signature) in sorted(required.items()):
        if name in core_reexports or name in allowed:
            continue
        problems.append(
            f"{path.relative_to(REPO_ROOT)}:{line_no}: `{name}` is a fieldglass-core "
            f"name in a public signature of {crate}, which does not re-export it — "
            f"add `pub use fieldglass_core::{name};` to src/lib.rs so a consumer "
            f"needs no fieldglass-core of its own (docs/architecture/01-crates.md)\n"
            f"    {signature[:120]}"
        )

    for name, reason in sorted(allowed.items()):
        if name not in required:
            problems.append(
                f"{crate}: ALLOWED_UNEXPORTED lists `{name}` ({reason}) but no public "
                f"signature names it any more — delete the entry"
            )
        elif name in core_reexports:
            problems.append(
                f"{crate}: ALLOWED_UNEXPORTED lists `{name}` ({reason}) but the crate "
                f"re-exports it — delete the entry"
            )

    return problems


def main() -> int:
    problems: list[str] = []
    for crate in FORMAT_CRATES:
        src_dir = CRATES_DIR / crate / "src"
        if not src_dir.is_dir():
            problems.append(f"{crate}: no src/ directory — did the crate move?")
            continue
        problems.extend(check_crate(crate, src_dir))

    for crate in ALLOWED_UNEXPORTED:
        if crate not in FORMAT_CRATES:
            problems.append(f"{crate}: ALLOWED_UNEXPORTED names a crate this check does not scan")

    if problems:
        print("A format crate's public API names a fieldglass-core type it does not re-export (#583):", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
