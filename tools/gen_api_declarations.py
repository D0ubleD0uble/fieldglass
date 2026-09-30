#!/usr/bin/env python3
"""Generate the hosts' TypeScript declarations from the API's JSON Schema (#574).

    python3 tools/gen_api_declarations.py           # rewrite the declarations
    python3 tools/gen_api_declarations.py --check   # fail if they have drifted

One schema, two consumers (ADR-0006 decision 2). The schema is
`crates/fieldglass/schema/api.schema.json`, which `cargo test -p fieldglass
--test api_rules` holds to the Rust wire types; this script turns it into:

* `extension/src/api.generated.ts`, which `extension/src/native.ts` imports the
  DTO types the addon returns from;
* `crates/fieldglass-wasm/src/api.generated.d.ts`, which the browser host embeds
  in wasm-bindgen's own `.d.ts` so `@fieldglass/wasm` publishes typed values
  rather than `any`.

The two files carry the same declarations under a different header, because both
hosts put the same bytes on the wire: every key of a returned object present,
with `null` for a Rust `None` (#574, decided 2026-09-29). An optional field is
therefore `field: T | null` and never `field?: T` on anything a host returns. On
an option object a host *sends*, a key it leaves out takes its default, so there
the key is `field?: T` — which is what the schema's `required` list says for each
direction, and all this script reads.

**Regenerating is a re-run, not a merge.** Output is a pure function of the
schema: definitions in name order, properties in the order the schema requires
them and then by name. Two branches that each changed the schema resolve by
re-recording the schema and re-running this, never by editing either output.

**It fails loudly on a schema it does not understand.** A keyword it has no
rule for, or a combination of keywords it would render only half of (a `$ref`
with siblings, an `enum` on a number), is an error naming it and where it was
found, not a silent `unknown`: a type that quietly widened to `unknown` is the drift this exists to
stop.
"""

from __future__ import annotations

import argparse
import difflib
import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCHEMA = REPO / "crates" / "fieldglass" / "schema" / "api.schema.json"
EXTENSION_OUT = REPO / "extension" / "src" / "api.generated.ts"
WASM_OUT = REPO / "crates" / "fieldglass-wasm" / "src" / "api.generated.d.ts"

REGENERATE = (
    "FIELDGLASS_UPDATE_SCHEMA=1 cargo test -p fieldglass --test api_rules, "
    "then python3 tools/gen_api_declarations.py"
)

# Keywords that say nothing TypeScript can state. Anything outside these and the
# ones `ts_type` handles is refused.
IGNORED = {"description", "format", "minimum", "maximum", "default", "title"}
HANDLED = {
    "$ref",
    "const",
    "enum",
    "oneOf",
    "anyOf",
    "type",
    "items",
    "minItems",
    "maxItems",
    "properties",
    "required",
    "additionalProperties",
}

# The longest fixed-length array spelled as a tuple. `[f64; 4]` boxes and corner
# pairs are tuples; anything longer reads better as `number[]`.
MAX_TUPLE = 8

# Definitions declared under another name in TypeScript, with the reason. The
# schema keeps the Rust name; only the declaration moves.
RENAMES: dict[str, str] = {
    # A module-level `Error` would shadow the global one for every file that
    # imports it, and in the wasm package's `.d.ts` it would share a module with
    # the declarations wasm-bindgen writes. The serialised error is also not what
    # either host throws (both throw a JS `Error`; the browser host sets its
    # `code`), so the distinct name is the more accurate one.
    "Error": "ApiError",
}

IDENT = re.compile(r"^[A-Za-z_$][A-Za-z0-9_$]*$")


class SchemaError(Exception):
    """The schema holds something this generator has no rule for."""


# ---------------------------------------------------------------------------
# Doc comments
# ---------------------------------------------------------------------------


def doc_text(description: str) -> str:
    """A Rust doc comment, rewritten for a TypeScript reader.

    Intra-doc links name Rust items a JavaScript caller cannot follow (#582
    removed the same noise from napi's generated `index.d.ts`), so a link keeps
    its text and loses its target; a web link is kept whole.
    """
    text = description.strip()
    # [`text`](Self::field) and [text](crate::path) -> the text alone.
    text = re.sub(
        r"\[([^\]]+)\]\((?![a-z]+://)[^)]*\)",
        lambda m: m.group(1),
        text,
    )
    # [`Name`] -> `Name`.
    text = re.sub(r"\[(`[^`\]]+`)\]", r"\1", text)
    # `crate::Session::count` -> `Session::count`: the path is where the item
    # lives in Rust, which says nothing to a JavaScript reader.
    text = re.sub(r"`(?:crate::(?:api::)?|Self::|fieldglass_core::(?:\w+::)*)([^`]+)`", r"`\1`", text)
    for rust, ts in RENAMES.items():
        text = text.replace(f"`{rust}`", f"`{ts}`")
    # A comment terminator inside the text would end the JSDoc block early.
    return text.replace("*/", "*\\/")


def doc_block(description: str | None, indent: str) -> list[str]:
    if not description:
        return []
    lines = doc_text(description).splitlines()
    out = [f"{indent}/**"]
    for line in lines:
        out.append(f"{indent} *{' ' + line if line.strip() else ''}".rstrip())
    out.append(f"{indent} */")
    return out


# ---------------------------------------------------------------------------
# Types
# ---------------------------------------------------------------------------


def check_keywords(schema: dict, where: str) -> None:
    unknown = set(schema) - IGNORED - HANDLED
    if unknown:
        raise SchemaError(
            f"{where}: no rule for keyword(s) {sorted(unknown)}; teach "
            "tools/gen_api_declarations.py what they mean in TypeScript"
        )
    check_combination(schema, where)


# Keywords that only mean something beside a `type` naming that kind.
OBJECT_ONLY = {"properties", "required", "additionalProperties"}
ARRAY_ONLY = {"items", "minItems", "maxItems"}


def check_combination(schema: dict, where: str) -> None:
    """Refuse a node whose keywords `ts_type` would not all honour.

    `ts_type` reads the first of `$ref`, `const`, `oneOf`/`anyOf` and `type`
    it finds and renders that. A node that also carried, say, `properties`
    beside a `$ref` would have those properties silently dropped, which is the
    quiet widening this generator exists to refuse. So every combination
    outside the ones it renders in full is an error: `type` with the keywords
    of that kind, `const` or `enum` with a string `type`, and a `$ref`, a
    `oneOf` or an `anyOf` alone.
    """
    structural = set(schema) - IGNORED
    for alone in ("$ref", "oneOf", "anyOf"):
        if alone in schema and structural != {alone}:
            raise SchemaError(
                f"{where}: {alone} beside {sorted(structural - {alone})} — only the "
                f"{alone} would be rendered; no rule for the combination"
            )
    kinds = schema.get("type")
    kinds = set(kinds) if isinstance(kinds, list) else {kinds} if kinds else set()
    if "const" in schema and structural - {"const", "type"}:
        raise SchemaError(
            f"{where}: const beside {sorted(structural - {'const', 'type'})}; "
            "no rule for the combination"
        )
    if "enum" in schema and kinds - {"string"}:
        raise SchemaError(f"{where}: enum on type {sorted(kinds)}; only a string enum is rendered")
    for keywords, kind in ((OBJECT_ONLY, "object"), (ARRAY_ONLY, "array")):
        present = structural & keywords
        if present and kind not in kinds:
            raise SchemaError(
                f"{where}: {sorted(present)} without type {kind!r} (type is "
                f"{sorted(kinds) or 'absent'}); no rule for the combination"
            )


def literal(value) -> str:
    if isinstance(value, bool) or value is None or isinstance(value, (int, float, str)):
        return json.dumps(value, ensure_ascii=False)
    raise SchemaError(f"a const or enum value TypeScript cannot spell: {value!r}")


def union(members: list[str]) -> str:
    seen: list[str] = []
    for m in members:
        if m not in seen:
            seen.append(m)
    return " | ".join(seen)


def ordered_properties(schema: dict) -> list[str]:
    """Required properties in the order the schema requires them, then the rest
    by name — declaration order for a returned type, whose every field is
    required, and a stable order for everything else."""
    props = schema.get("properties", {})
    required = [p for p in schema.get("required", []) if p in props]
    missing = [p for p in schema.get("required", []) if p not in props]
    if missing:
        raise SchemaError(f"required names no property: {missing}")
    return required + sorted(p for p in props if p not in required)


def object_body(schema: dict, defs: dict, indent: str, where: str) -> list[str]:
    """The member lines of an object type, one property per line."""
    lines: list[str] = []
    required = set(schema.get("required", []))
    for name in ordered_properties(schema):
        prop = schema["properties"][name]
        lines.extend(doc_block(prop.get("description"), indent))
        key = name if IDENT.match(name) else json.dumps(name)
        optional = "" if name in required else "?"
        lines.append(f"{indent}{key}{optional}: {ts_type(prop, defs, indent, f'{where}.{name}')};")
    extra = schema.get("additionalProperties")
    if extra is True:
        lines.append(f"{indent}[key: string]: unknown;")
    elif extra not in (None, False):
        raise SchemaError(f"{where}: additionalProperties must be true or false, not {extra!r}")
    return lines


def ts_single(schema: dict, kind: str, defs: dict, indent: str, where: str) -> str:
    if kind == "null":
        return "null"
    if kind == "boolean":
        return "boolean"
    if kind in ("integer", "number"):
        return "number"
    if kind == "string":
        if "enum" in schema:
            return union([literal(v) for v in schema["enum"]])
        return "string"
    if kind == "array":
        if "items" not in schema:
            raise SchemaError(f"{where}: an array with no items")
        item = ts_type(schema["items"], defs, indent, f"{where}[]")
        lo, hi = schema.get("minItems"), schema.get("maxItems")
        if lo is not None and lo == hi and 0 < lo <= MAX_TUPLE:
            return "[" + ", ".join([item] * lo) + "]"
        return f"({item})[]" if " " in item else f"{item}[]"
    if kind == "object":
        if "properties" not in schema:
            if schema.get("additionalProperties") is True:
                return "{ [key: string]: unknown }"
            raise SchemaError(f"{where}: an object with no properties and no stated looseness")
        inner = indent + "  "
        body = object_body(schema, defs, inner, where)
        return "{\n" + "\n".join(body) + f"\n{indent}}}"
    raise SchemaError(f"{where}: no rule for type {kind!r}")


def ts_type(schema: dict, defs: dict, indent: str, where: str) -> str:
    if not isinstance(schema, dict):
        raise SchemaError(f"{where}: a schema that is not an object: {schema!r}")
    check_keywords(schema, where)
    if "$ref" in schema:
        ref = schema["$ref"]
        name = ref.removeprefix("#/$defs/")
        if name == ref or name not in defs:
            raise SchemaError(f"{where}: a $ref that names no definition: {ref}")
        return RENAMES.get(name, name)
    if "const" in schema:
        return literal(schema["const"])
    for key in ("oneOf", "anyOf"):
        if key in schema:
            return union([ts_type(m, defs, indent, f"{where}|{i}") for i, m in enumerate(schema[key])])
    kind = schema.get("type")
    if kind is None:
        raise SchemaError(f"{where}: no type, $ref, const, oneOf or anyOf")
    if isinstance(kind, list):
        return union([ts_single(schema, k, defs, indent, where) for k in kind])
    return ts_single(schema, kind, defs, indent, where)


def declaration(rust_name: str, schema: dict, defs: dict) -> list[str]:
    name = RENAMES.get(rust_name, rust_name)
    check_keywords(schema, name)
    lines = doc_block(schema.get("description"), "")
    if schema.get("type") == "object" and "properties" in schema:
        lines.append(f"export interface {name} {{")
        lines.extend(object_body(schema, defs, "  ", name))
        lines.append("}")
        return lines
    members = schema.get("oneOf") or schema.get("anyOf")
    if members:
        # One member per line, each with its own doc: an enum's variants carry
        # the meaning of the value, and a one-line union would drop it.
        lines.append(f"export type {name} =")
        for i, member in enumerate(members):
            lines.extend(doc_block(member.get("description"), "  "))
            lines.append(f"  | {ts_type(member, defs, '  ', f'{name}|{i}')}")
        lines[-1] += ";"
        return lines
    lines.append(f"export type {name} = {ts_type(schema, defs, '', name)};")
    return lines


def render(schema: dict, header: str) -> str:
    """Every definition in the schema, as TypeScript, under `header`."""
    defs = schema.get("$defs")
    if not isinstance(defs, dict) or not defs:
        raise SchemaError("the schema has no $defs, so there is nothing to declare")
    out = [header.rstrip(), ""]
    for name in sorted(defs):
        out.extend(declaration(name, defs[name], defs))
        out.append("")
    return "\n".join(out).rstrip() + "\n"


# ---------------------------------------------------------------------------
# The two outputs
# ---------------------------------------------------------------------------

COMMON = f"""\
// Generated by tools/gen_api_declarations.py from
// crates/fieldglass/schema/api.schema.json. Do not edit by hand; regenerate with
//   {REGENERATE}
//
// A returned object carries every key, and a field with nothing to report is
// `null`, never missing (#574). An option object a host sends may leave out any
// key marked `?`."""

EXTENSION_HEADER = f"""\
{COMMON}
//
// The fieldglass API's wire types, as the native addon returns them. Imported by
// native.ts."""

WASM_HEADER = f"""\
{COMMON}
//
// Embedded in the package's .d.ts by src/lib.rs (typescript_custom_section)."""


def outputs(schema: dict) -> dict[Path, str]:
    return {
        EXTENSION_OUT: render(schema, EXTENSION_HEADER),
        WASM_OUT: render(schema, WASM_HEADER),
    }


def shown(path: Path) -> str:
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def check(schema_path: Path | None = None) -> list[str]:
    """Every output that differs from what the schema generates, with a diff."""
    schema_path = schema_path or SCHEMA
    if not schema_path.is_file():
        return [f"{shown(schema_path)} is missing; record it with {REGENERATE}"]
    try:
        wanted = outputs(json.loads(schema_path.read_text(encoding="utf-8")))
    except SchemaError as e:
        return [f"{shown(schema_path)}: {e}"]
    problems = []
    for path, text in wanted.items():
        have = path.read_text(encoding="utf-8") if path.is_file() else ""
        if have != text:
            diff = "".join(
                list(
                    difflib.unified_diff(
                        have.splitlines(keepends=True),
                        text.splitlines(keepends=True),
                        fromfile=f"{shown(path)} (checked in)",
                        tofile=f"{shown(path)} (generated)",
                        n=1,
                    )
                )[:40]
            )
            problems.append(f"{shown(path)} is not what the schema generates:\n{diff}")
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="compare the checked-in declarations with the schema instead of writing them",
    )
    args = parser.parse_args(argv)

    if args.check:
        problems = check()
        if problems:
            print("The generated API declarations have drifted from the schema (#574):")
            for p in problems:
                print(f"  - {p}")
            print(f"Regenerate: {REGENERATE}")
            return 1
        print("The API declarations match the schema.")
        return 0

    try:
        wanted = outputs(json.loads(SCHEMA.read_text(encoding="utf-8")))
    except SchemaError as e:
        print(f"{shown(SCHEMA)}: {e}", file=sys.stderr)
        return 1
    for path, text in wanted.items():
        path.write_text(text, encoding="utf-8")
        print(f"wrote {shown(path)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
