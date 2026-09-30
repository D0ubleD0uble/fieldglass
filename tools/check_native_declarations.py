#!/usr/bin/env python3
"""Fail when `extension/src/native.ts` drifts from what napi-rs generates.

    python3 tools/check_native_declarations.py

`extension/src/native.ts` declares the native module's shapes **by hand**, and
napi-rs generates the same shapes into `extension/bin/index.d.ts` on every
`napi build`. Two statements of one interface, and nothing compared them — so a
method added on the Rust side and forgotten in TypeScript is invisible until a
call site needs it, and a field whose optionality was mistyped is invisible until
it is `undefined` at runtime.

Both have happened. #659 added four methods to `ZarrHandle` and declared none of
them; the omission surfaced only because a throwaway assignability check was
written for another reason.

**Four properties, and only four.** This is a drift gate, not a TypeScript parser:

1. **No generated *method* is missing.** Every method on a generated class has a
   declaration, because a method the extension cannot call is a capability that
   silently is not there — #659 added four to `ZarrHandle` and declared none.

   **Object *fields* are deliberately partial and are not checked for
   completeness.** A hand-written object type is a view, and declaring a field
   nothing reads is declaring a shape nothing uses. The reverse is not checked
   either — `native.ts` also declares the loader, the webview payloads and
   `SlicePanelHandle`, none of which napi generates.

2. **Every field that *is* declared agrees, optionality and `| null`
   included** — on a `#[napi(object)]`, which is what napi generates an
   interface for. A plain `#[napi(object)]` maps a Rust `None` to JavaScript
   `undefined` and generates `field?: T`; one marked `use_nullable = true`
   writes `null` and generates `field: T | null`. A hand-written declaration
   that says the other one type-checks and then fails *open*: a
   `field: T | null` over an `undefined` passes every `!== null` guard, which is
   how a grid-less GRIB1 spectral message crashed the editor with
   `undefined.toFixed()` (#288, fixed in #289). That is the one bug this file
   exists to make impossible.

   The API's own types are the other kind (#574). The addon returns them
   through serde (`#[napi(ts_return_type = "MessageInfo")]` over a
   `serde_json::Value`), which keeps every key and writes `null` for `None`, so
   their declarations are `field: T | null` and are *generated* from the Rust
   schema into `extension/src/api.generated.ts` rather than written here.
   napi emits no interface for them, so property 2 never sees them.

3. **Every type a generated signature names is declared somewhere.** A
   `ts_return_type` puts a name into `index.d.ts` that napi does not declare
   itself; it must be one `api.generated.ts` exports, or the method's return
   type is a name nothing defines — which `skipLibCheck` would let through.

4. **No object the addon returns leaves a key out.** The wire contract is that
   every key of a returned object is present and a field with nothing to report
   is `null` (#574, decided 2026-09-29), on both hosts. A generated interface
   with an optional (`?`) field is a `#[napi(object)]` without
   `use_nullable = true`, so it breaks that contract whatever `native.ts` says.
   The objects a caller *sends* are the exception — leaving a key out of an
   option object means "the default" — and they are named in `INPUT_OBJECTS`,
   each with its reason.

Types beyond optionality are compared only after normalising the spellings the
two generators legitimately differ on (`Array<T>` against `T[]`); anything else
is reported, because a silent type divergence is the third way these two files
can disagree.

**It cannot pass vacuously.** A missing `index.d.ts` is an error naming the build
command, not a skip: a gate that quietly passes when its input is absent is worse
than no gate. That is also why this runs in CI's extension job, after the addon is
built, rather than at the commit stage where no addon exists.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GENERATED = REPO / "extension" / "bin" / "index.d.ts"
HANDWRITTEN = REPO / "extension" / "src" / "native.ts"
# The API's wire types, generated from the Rust schema (#574).
API_GENERATED = REPO / "extension" / "src" / "api.generated.ts"

# Names a generated signature may use without anything in the repo declaring
# them: the language's and Node's own.
GLOBAL_TYPES = {
    "Array",
    "Buffer",
    "Float32Array",
    "Float64Array",
    "Int32Array",
    "Promise",
    "Record",
    "Uint8Array",
    "Uint32Array",
}

# Spellings the two sides legitimately differ on. Left is what napi generates.
TYPE_ALIASES: dict[str, str] = {
    # napi-rs writes the generic form; the hand-written file uses the shorthand.
    "Array<": "[]",
}

# Generated names `native.ts` deliberately does not declare, with the reason.
# An entry here is a hole in property 1, so each needs one.
IGNORED_GENERATED: dict[str, str] = {
    "DecodedVariable": (
        "returned by `decode_variable`, which the extension does not call — it "
        "renders slices through `renderSlice` and never asks for raw values. "
        "Declaring it would be declaring a shape nothing reads."
    ),
}

# Fields the hand-written file declares differently from what napi generates,
# knowingly. **This is a ratchet, not an exemption**: the check fails when a field
# diverges without being listed, *and* when a listed field stops diverging, so an
# entry cannot outlive the debt it records.
#
# Empty since #574. It held the 23 `MessageMeta` fields typed `T | null` where
# napi generated `T?`; `MessageMeta` is gone, the extension reads the API's
# `MessageInfo` instead, and every object the addon returns now writes `null`.
# Adding an entry means a declaration that disagrees with the runtime again —
# the #288 shape — so it wants a reason as good as that one had.
KNOWN_NULLABLE: set[str] = set()

# Generated objects a caller *sends* rather than receives, which property 4
# exempts: an option object's absent key means "use the default", so `field?: T`
# is the right declaration for it. Each needs its reason, and an entry napi no
# longer generates is reported as stale.
INPUT_OBJECTS: dict[str, str] = {
    "RenderOptions": (
        "the picker state the panel sends to every render, overlay, contour and "
        "probe call; an omitted field is the renderer's default"
    ),
}


def strip_comments(text: str) -> str:
    """Remove `/** … */` and `// …`, so a type named in prose is not a member."""
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return re.sub(r"//[^\n]*", "", text)


EXTENDS_RE = re.compile(
    r"export interface (?P<name>[A-Za-z_][A-Za-z0-9_]*)\s+extends\s+(?P<bases>[^{]+)\{"
)


def extends(text: str) -> dict[str, list[str]]:
    """Interface name -> the interfaces it extends.

    Followed when collecting hand-written members, because an inherited method
    *is* declared — `ZarrHandle extends SlicePanelHandle` is how #659 avoided
    writing eight signatures twice, and a checker that ignored it would demand
    the duplication it exists to prevent.
    """
    out: dict[str, list[str]] = {}
    for m in EXTENDS_RE.finditer(text):
        out[m.group("name")] = [b.strip() for b in m.group("bases").split(",") if b.strip()]
    return out


def blocks(text: str, pattern: str) -> dict[str, str]:
    """Body of each `pattern`-introduced brace block, by name.

    Brace-counted rather than regex-terminated: a member whose type contains a
    brace (an inline object) would otherwise end the block early and silently
    shorten what is compared.
    """
    out: dict[str, str] = {}
    for m in re.finditer(pattern, text):
        name = m.group("name")
        i = text.index("{", m.end() - 1) if "{" not in m.group(0) else m.start() + m.group(0).index("{")
        depth = 0
        for j in range(i, len(text)):
            if text[j] == "{":
                depth += 1
            elif text[j] == "}":
                depth -= 1
                if depth == 0:
                    out[name] = text[i + 1 : j]
                    break
    return out


MEMBER_RE = re.compile(
    r"^\s*(?P<static>static\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*"
    r"(?P<optional>\?)?\s*(?P<call>\()?",
    re.MULTILINE,
)


def members(body: str) -> dict[str, bool]:
    """Member name -> whether it is optional.

    A method is never "optional" in the sense that matters here; only a field's
    `?` is compared, because that is what encodes `Option`.
    """
    out: dict[str, bool] = {}
    for m in MEMBER_RE.finditer(body):
        name = m.group("name")
        if name in {"static", "readonly", "declare"}:
            continue
        # A member is a field or a method; both are named at the start of a line.
        out[name] = bool(m.group("optional"))
    return out


def field_types(body: str) -> dict[str, str]:
    """Field name -> its normalised type, for the non-method members."""
    out: dict[str, str] = {}
    for line in body.splitlines():
        m = re.match(
            r"\s*(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\??\s*:\s*(?P<type>.+?)\s*;?\s*$", line
        )
        if not m:
            continue
        ty = m.group("type").strip().rstrip(",;")
        # `Array<T>` and `T[]` are the same type, spelled two ways — including
        # inside a union, which is where a nullable array puts it.
        while True:
            spelled = re.sub(r"Array<(?P<inner>[^<>]+)>", r"\g<inner>[]", ty)
            if spelled == ty:
                break
            ty = spelled
        out[m.group("name")] = ty
    return out


def inherited_chain(name: str, graph: dict[str, list[str]]) -> list[str]:
    """`name` and every interface it extends, transitively."""
    seen: list[str] = []
    stack = [name]
    while stack:
        at = stack.pop()
        if at in seen:
            continue
        seen.append(at)
        stack.extend(graph.get(at, []))
    return seen


def shown(path: Path) -> str:
    """A path for a message: repo-relative when it is in the repo, else as given.

    The self-test points these at a temporary directory, and `relative_to` raises
    there — so a checker that only formatted repo paths could not be tested.
    """
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def check() -> list[str]:
    problems: list[str] = []
    if not GENERATED.is_file():
        return [
            f"{shown(GENERATED)} is missing, so nothing was compared. "
            "Build the addon first: `cd crates/fieldglass-napi && npx napi build "
            "--platform --release --target x86_64-unknown-linux-gnu --output-dir "
            "../../extension/bin`. This check does not skip — a gate that passes "
            "when its input is absent is worse than no gate."
        ]

    gen = strip_comments(GENERATED.read_text(encoding="utf-8"))
    hand = strip_comments(HANDWRITTEN.read_text(encoding="utf-8"))

    gen_classes = blocks(gen, r"export declare class (?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\{")
    gen_ifaces = blocks(gen, r"export interface (?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*\{")
    hand_ifaces = blocks(
        hand, r"export interface (?P<name>[A-Za-z_][A-Za-z0-9_]*)(?:\s+extends\s+[^{]+)?\s*\{"
    )
    hand_extends = extends(hand)

    if not gen_classes or not gen_ifaces:
        problems.append(
            f"parsed no classes or no interfaces out of {shown(GENERATED)} — "
            "the generated shape changed and this checker stopped measuring anything"
        )
        return problems

    # --- 1. A generated class's methods are declared somewhere by hand --------
    #
    # A class arrives split in two: instance methods on `X`, and the static
    # constructors on `XCtor`, because TypeScript cannot state both on one
    # interface the way a class does.
    for name, body in gen_classes.items():
        if name in IGNORED_GENERATED:
            continue
        declared: dict[str, bool] = {}
        for candidate in (name, f"{name}Ctor"):
            for iface in inherited_chain(candidate, hand_extends):
                if iface in hand_ifaces:
                    declared.update(members(hand_ifaces[iface]))
        if not declared:
            problems.append(
                f"napi generates class {name}, and native.ts declares neither "
                f"`{name}` nor `{name}Ctor`"
            )
            continue
        for member in members(body):
            if member not in declared:
                problems.append(
                    f"{name}.{member} is generated by napi and not declared in "
                    f"native.ts — add it to `{name}` (or `{name}Ctor` if it is static)"
                )

    # --- 2. A generated object type's fields match, optionality included ------
    for name in sorted(set(INPUT_OBJECTS) - set(gen_ifaces)):
        problems.append(
            f"INPUT_OBJECTS names {name}, which napi no longer generates — the entry "
            "in tools/check_native_declarations.py is stale"
        )
    for name, body in gen_ifaces.items():
        if name in IGNORED_GENERATED:
            continue
        # --- 4. An object the addon returns leaves no key out -----------------
        if name not in INPUT_OBJECTS:
            for field, optional in members(body).items():
                if optional:
                    problems.append(
                        f"{name}.{field} is optional in index.d.ts, so the addon leaves "
                        "the key out when it has nothing to say; every object it returns "
                        "writes `null` instead (#574). Mark the `#[napi(object)]` "
                        "`use_nullable = true`, or name it in INPUT_OBJECTS if callers "
                        "send it rather than receive it"
                    )
        hand_body = hand_ifaces.get(name)
        if hand_body is None:
            problems.append(f"napi generates interface {name}, and native.ts does not declare it")
            continue
        gen_members, hand_members = members(body), members(hand_body)
        gen_types, hand_types = field_types(body), field_types(hand_body)
        for field, optional in gen_members.items():
            qualified = f"{name}.{field}"
            if field not in hand_members:
                # Not drift: a hand-written object type is a *view* of the
                # generated one, and a narrower view is the point. See property 1.
                continue
            hand_ty = hand_types.get(field, "")
            known = qualified in KNOWN_NULLABLE
            diverges = optional != hand_members[field] or (optional and "null" in hand_ty)
            if known:
                # The ratchet's second direction: a listed field that has been
                # fixed must leave the list, or the list outlives the debt it
                # records and stops measuring anything.
                if not diverges:
                    problems.append(
                        f"{qualified} no longer diverges, so its KNOWN_NULLABLE entry in "
                        "tools/check_native_declarations.py is stale — delete it"
                    )
                continue
            if optional != hand_members[field]:
                problems.append(
                    f"{qualified}: napi generates it "
                    f"{'optional' if optional else 'required'} and native.ts declares it "
                    f"{'optional' if hand_members[field] else 'required'}"
                )
            if optional and "null" in hand_ty:
                problems.append(
                    f"{qualified} is typed `{hand_ty}` — this object leaves the key "
                    "out for a Rust `None`, so the field is `undefined`, never `null`, "
                    "and a `!== null` guard on it fails open (#288). Use `field?: T`."
                )
            gen_ty = gen_types.get(field, "")
            # A hand-written literal union where napi says `string` is a
            # *narrowing*: the Rust side returns one of a closed set and the
            # declaration says which, which is strictly more information. Not
            # drift, and not something to undo. The narrowing has to keep the
            # `| null` exactly as napi states it, though: a generated
            # `string | null` against a hand-written `"a" | "b"` would drop the
            # null the addon sends, and a `=== "a"`-style guard written against
            # that declaration never sees it — the #288 shape again.
            narrowing = (
                gen_ty.replace(" | null", "") == "string"
                and '"' in hand_ty
                and ("null" in gen_ty) == ("null" in hand_ty)
            )
            if gen_ty and hand_ty and not narrowing and gen_ty != hand_ty:
                problems.append(
                    f"{name}.{field}: napi generates `{gen_ty}` and native.ts declares "
                    f"`{hand_ty}`"
                )

    # --- 3. A name a generated signature uses is declared somewhere -----------
    declared_by_napi = set(DECLARED_RE.findall(gen))
    exported_by_api = (
        set(DECLARED_RE.findall(strip_comments(API_GENERATED.read_text(encoding="utf-8"))))
        if API_GENERATED.is_file()
        else set()
    )
    # String literals first: a capitalised word inside `"…"` (a literal union
    # napi copies from a `ts_type`) names no type.
    code = re.sub(r""""(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*'""", '""', gen)
    for used in sorted(set(TYPE_NAME_RE.findall(code)) - declared_by_napi - GLOBAL_TYPES):
        if used not in exported_by_api:
            problems.append(
                f"index.d.ts names `{used}`, which neither it nor {shown(API_GENERATED)} "
                "declares — a `ts_return_type` naming an API type that is not in the "
                "schema, or a generated file that is stale (python3 "
                "tools/gen_api_declarations.py)"
            )
    return problems


# `export interface X`, `export declare class X`, `export type X = …`, …
DECLARED_RE = re.compile(
    r"export\s+(?:declare\s+)?(?:const\s+)?(?:class|interface|type|enum|function|const)\s+"
    r"([A-Za-z_][A-Za-z0-9_]*)"
)
# A capitalised identifier, which in a declaration file is a type name.
TYPE_NAME_RE = re.compile(r"\b[A-Z][A-Za-z0-9_]*\b")


def main() -> int:
    problems = check()
    if problems:
        print("native.ts has drifted from what napi-rs generates (#574):")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    print("native.ts declarations match the generated ones.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
