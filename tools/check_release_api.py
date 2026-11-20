#!/usr/bin/env python3
"""Fail when the npm API or the extension's contributions break against a tag (#1871).

    python3 -I tools/check_release_api.py --baseline v0.6.1

`cargo semver-checks` holds the published crates to the last release. Two
public surfaces it cannot see are held here, each read from the working tree
and from `git show <baseline>:<file>`:

* **The host API schema**, `crates/fieldglass/schema/api.schema.json`. The
  `@fieldglass/wasm` declarations are generated from it
  (`tools/gen_api_declarations.py`), and `fieldglass-wasm` is `publish = false`,
  so semver-checks skips it. The conventions' npm rule: output types may gain
  fields, input types only optional ones; nothing is removed, renamed or
  narrowed.
* **The extension's `contributes`** in `extension/package.json`. A patch
  renames or removes no setting key or command id.

The baseline is the tag the *Public API* job measures against, printed by
`python3 -I tools/semver_ci.py baseline`, and CI passes it in. There is no
exception list, as the semver job has none. The job's other input is
mirrored too: when `tools/semver-release-type` lets the next release break
(`minor` while the workspace major is 0, `major` from 1.0), each break is
printed as a warning and the run passes; otherwise a break fails it.

**Which schema types are input.** The schema does not say. Two things in the
source do, and the script reads both from the baseline and from the working
tree and takes the union of each:

* `SENT` in `crates/fieldglass/tests/api_rules.rs`: the types read with the
  deserialize contract, the option objects a host sends.
* The browser host's `unchecked_param_type`, `unchecked_optional_param_type`
  and `unchecked_return_type` attributes in `crates/fieldglass-wasm/src`:
  what `@fieldglass/wasm` takes and gives back. A parameter type is input even when it is not in `SENT`
  (`WindowRequest`), and one that is returned as well (`MessageInfo`,
  `PlanItem`, `PlanRange`, `Stats`) is passed back, so a key it gains must be
  optional there, as #1552 marks it with `x-fieldglass-added-in`.

A type is input when a `SENT` root or a parameter type reaches it through
`$ref`, and output when a root outside `SENT` (the schema's
`x-fieldglass-feature-roots`) or a return type does. A type both reach gets
both sets of rules, and so does one neither reaches. A name in an attribute
that is not a definition, a TypeScript builtin or one of the host's own
`HAND_WRITTEN` interfaces is an error. Taking the baseline's lists as well
means a PR cannot drop a type from one to loosen its check.

What fails, on the schema:

* a definition removed, or a root type moved out of the build it was in
  (`x-fieldglass-feature-roots`, which decides the npm entry that declares it);
* a property removed, from any type;
* a property newly required on an input type, and a property no longer
  required on an output type (a key a host could rely on being present);
* a value on an output type that may now be `null` where it could not be
  before (#1894, owner decision on #1871): a `type` list or an `anyOf` or
  `oneOf` branch that accepts `null`, a `null` enum value, or a branch that
  accepts any value. Each definition is compared on its own, so a `null`
  added inside a `$ref`'d definition is reported there, under the
  directions that reach it. A new enum value or variant on an output type
  passes: callers handle a value they don't know;
* a narrowed type: a value the baseline schema accepted that the new one
  does not. That covers a removed enum value or `oneOf` variant, a dropped
  `null`, `number` to `integer`, a smaller integer format or bound, a tighter
  array length, a `$ref` to another definition, and `additionalProperties`
  turned off. A keyword it has no rule for (`exclusiveMinimum`, `multipleOf`)
  is an error rather than unchecked.

On `contributes`, what fails is a removed configuration key, a removed enum
value or type of a setting, a removed command id, custom editor `viewType` or
language id. A command id this extension owns (`fieldglass.`) that a menu or
keybinding names counts as a public command id too. A `contributes` key this
script has no rule for fails on either side, so a new kind of contribution
gets a rule before it ships. Additions pass.

Tested by `tools/test_check_release_api.py`.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterable
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCHEMA = "crates/fieldglass/schema/api.schema.json"
API_RULES = "crates/fieldglass/tests/api_rules.rs"
PACKAGE = "extension/package.json"

FEATURE_ROOTS = "x-fieldglass-feature-roots"
ALWAYS = "always"
TYPED_ARRAY = "x-fieldglass-typed-array"
ADDED_IN = "x-fieldglass-added-in"

# Keywords that say nothing about which values a schema accepts.
IGNORED = {"description", "title", "default", "examples", ADDED_IN, "$schema"}

SENT_LIST = re.compile(r"const SENT: &\[&str\] = &\[(?P<body>.*?)\];", re.DOTALL)

WASM_SRC = "crates/fieldglass-wasm/src"
# A wasm-bindgen attribute that gives a parameter or a return its TypeScript
# type, with the Rust string literal's body. wasm-bindgen 0.2.129's attribute
# list (wasm-bindgen-macro-support's parser.rs) has five that name or write
# TypeScript types: these three; `typescript_custom_section`, raw text the
# crate uses for the generated declarations and the HAND_WRITTEN interfaces;
# and `typescript_type`, which this crate does not use and `wasm_types`
# refuses, as it has no rule here.
WASM_TYPE_ATTR = re.compile(r'unchecked_(param|optional_param|return)_type\s*=\s*"((?:[^"\\]|\\.)*)"')
WASM_UNREAD_ATTR = re.compile(r"\btypescript_type\s*=")

# Names a host's TypeScript types use that are not schema definitions.
TS_BUILTINS = {
    "any", "unknown", "void", "null", "undefined", "never", "object",
    "string", "number", "boolean", "bigint", "true", "false",
    "Array", "Map", "Record", "Promise", "Partial", "Readonly",
    "Uint8Array", "Uint16Array", "Uint32Array", "Int8Array", "Int16Array", "Int32Array",
    "Float32Array", "Float64Array",
}
# The browser host's own TypeScript interfaces, written in a
# `typescript_custom_section` rather than generated from the schema, so they
# have no definition to classify. Each must still be declared there (checked
# on the working tree), so an entry cannot outlive its interface.
HAND_WRITTEN = {
    # `render`'s return: the schema's `Raster` with `rgba` as a Uint8Array
    # (lib.rs, #1156). Its `range` is the schema's `ColorRange`, which other
    # return types reach, so the output rules still cover it.
    "RenderedRaster": "lib.rs",
    # One stored object's fetched byte ranges, an argument of the Zarr
    # window call (zarr.rs). Plain numbers and bytes, no schema type inside.
    "ObjectRanges": "zarr.rs",
}

# The setting the Public API job reads for the next release's bump.
RELEASE_TYPE = "tools/semver-release-type"
WORKSPACE_MANIFEST = "Cargo.toml"

# What each integer and number `format` admits: (lowest, highest, precision).
# Precision ranks an integer below `float` below `double`, so a field that
# went from `double` to `float` reads as narrowed. A format not listed here
# is an error rather than a guess.
FORMATS: dict[str, tuple[float, float, int]] = {
    "int8": (-(2**7), 2**7 - 1, 0),
    "int16": (-(2**15), 2**15 - 1, 0),
    "int32": (-(2**31), 2**31 - 1, 0),
    "int64": (-(2**63), 2**63 - 1, 0),
    "int": (-(2**63), 2**63 - 1, 0),
    "uint8": (0, 2**8 - 1, 0),
    "uint16": (0, 2**16 - 1, 0),
    "uint32": (0, 2**32 - 1, 0),
    "uint64": (0, 2**64 - 1, 0),
    "uint": (0, 2**64 - 1, 0),
    "float": (-3.4028234663852886e38, 3.4028234663852886e38, 1),
    "double": (float("-inf"), float("inf"), 2),
}

# The keywords a number or integer schema may carry here. Any other
# (`exclusiveMinimum`, `multipleOf`) is an error rather than unchecked.
NUMBER_KEYWORDS = {"type", "format", "minimum", "maximum"}

# The `contributes` keys this script has a rule for.
CONTRIBUTES_RULES = {"languages", "customEditors", "commands", "configuration", "menus", "keybindings"}
# The prefix of the command ids this extension owns. A menu or keybinding may
# also name a built-in command, which is not this extension's to keep.
OWN_COMMAND_PREFIX = "fieldglass."


class CheckError(Exception):
    """A file this script cannot read the way it expects; the message says why."""


# ---------------------------------------------------------------- schema


def parse_sent(text: str, where: str) -> list[str]:
    """The names in `api_rules.rs`'s `SENT` list."""
    m = SENT_LIST.search(text)
    if not m:
        raise CheckError(f"{where}: no `const SENT: &[&str] = &[...]` list found")
    names = re.findall(r'"([A-Za-z0-9_]+)"', m.group("body"))
    if not names:
        raise CheckError(f"{where}: the SENT list is empty")
    return names


def definitions(schema: dict, where: str) -> dict[str, dict]:
    defs = schema.get("$defs")
    if not isinstance(defs, dict) or not defs:
        raise CheckError(f"{where}: no `$defs` table")
    return defs


def ref_name(ref: str, where: str) -> str:
    prefix = "#/$defs/"
    if not ref.startswith(prefix):
        raise CheckError(f"{where}: `$ref` {ref!r} does not point into `$defs`")
    return ref[len(prefix):]


def refs_in(value: object, where: str) -> set[str]:
    out: set[str] = set()
    if isinstance(value, dict):
        if isinstance(value.get("$ref"), str):
            out.add(ref_name(value["$ref"], where))
        for v in value.values():
            out |= refs_in(v, where)
    elif isinstance(value, list):
        for v in value:
            out |= refs_in(v, where)
    return out


def reach(defs: dict[str, dict], roots: Iterable[str], where: str) -> set[str]:
    seen: set[str] = set()
    stack = [r for r in roots if r in defs]
    while stack:
        name = stack.pop()
        if name in seen:
            continue
        seen.add(name)
        stack.extend(refs_in(defs[name], f"{where} {name}") - seen)
    return seen


def feature_roots(schema: dict, where: str) -> dict[str, list[str]]:
    roots = schema.get(FEATURE_ROOTS)
    if not isinstance(roots, dict) or not all(isinstance(v, list) for v in roots.values()):
        raise CheckError(f"{where}: no `{FEATURE_ROOTS}` map of lists")
    return roots


def rust_string(body: str) -> str:
    """The text of a Rust `"..."` literal's body: `\\"` and `\\\\` unescaped."""
    return re.sub(r"\\(.)", r"\1", body)


def ts_type_names(expr: str) -> set[str]:
    """The type names a TypeScript type expression uses.

    String literal types (`"f32"`) and object literal keys (`values:`) are
    not type names, and the builtins in `TS_BUILTINS` are left out.
    """
    expr = re.sub(r'"(?:[^"\\]|\\.)*"', " ", expr)
    expr = re.sub(r"\b[A-Za-z_$][A-Za-z0-9_$]*\s*\??\s*:", " ", expr)
    return set(re.findall(r"\b[A-Za-z_$][A-Za-z0-9_$]*\b", expr)) - TS_BUILTINS


def wasm_types(sources: Iterable[str]) -> tuple[set[str], set[str]]:
    """(parameter type names, return type names) the browser host declares.

    Read from every `unchecked_param_type`, `unchecked_optional_param_type`
    and `unchecked_return_type` in the wasm crate's sources: what
    `@fieldglass/wasm` takes and gives back. An optional parameter is a
    parameter. A `typescript_type` attribute is an error: it names a type this
    script has no rule for.
    """
    params: set[str] = set()
    returns: set[str] = set()
    for text in sources:
        if WASM_UNREAD_ATTR.search(text):
            raise CheckError(
                "the wasm crate uses `typescript_type`, which tools/check_release_api.py has no rule for"
            )
        for kind, body in WASM_TYPE_ATTR.findall(text):
            names = ts_type_names(rust_string(body))
            (returns if kind == "return" else params).update(names)
    return params, returns


class HostRoots:
    """Where a schema type crosses a host boundary, read off the source.

    `sent`: the `SENT` list of `api_rules.rs`, the types read with the
    deserialize contract. `params` and `returns`: the type names the browser
    host's `unchecked_param_type` and `unchecked_return_type` attributes use.
    A name in `params` is input whether or not it is in `sent`: `WindowRequest`
    is not, and `MessageInfo` and `PlanItem` are returned and passed back.
    """

    def __init__(self, sent: Iterable[str], params: Iterable[str] = (), returns: Iterable[str] = ()) -> None:
        self.sent = set(sent)
        self.params = set(params)
        self.returns = set(returns)

    def __or__(self, other: HostRoots) -> HostRoots:
        return HostRoots(self.sent | other.sent, self.params | other.params, self.returns | other.returns)


def directions(schema: dict, roots: HostRoots, where: str) -> tuple[set[str], set[str]]:
    """(input names, output names) in one schema. A name in neither is in both.

    Input is what `SENT` or a browser parameter type reaches; output what a
    feature root outside `SENT` or a browser return type reaches.
    """
    defs = definitions(schema, where)
    missing = sorted(roots.sent - set(defs))
    if missing:
        raise CheckError(f"{where}: SENT names {missing}, which are not definitions")
    unknown = sorted((roots.params | roots.returns) - set(defs) - set(HAND_WRITTEN))
    if unknown:
        raise CheckError(
            f"{where}: the wasm crate's unchecked_param_type/unchecked_return_type name {unknown}, "
            "which are not schema definitions, TypeScript builtins or HAND_WRITTEN types"
        )
    features = {r for names in feature_roots(schema, where).values() for r in names}
    inputs = reach(defs, roots.sent | roots.params, where)
    outputs = reach(defs, (features - roots.sent) | roots.returns, where)
    unreached = set(defs) - inputs - outputs
    return inputs | unreached, outputs | unreached


class Direction:
    """Which rules apply to a type: what a host sends, receives, or both."""

    def __init__(self, input: bool, output: bool) -> None:
        self.input = input
        self.output = output


def branches(schema: object, where: str) -> list[dict]:
    """The schema as a list of alternatives, each one type, `$ref` or value.

    `anyOf` and `oneOf` are flattened, a list-valued `type` is split per type,
    and an `enum` or `const` becomes one branch per value, so a value the old
    schema admitted can be looked for among the new one's alternatives.
    """
    if schema is True or schema == {}:
        return [{"any": True}]
    if schema is False:
        return []
    if not isinstance(schema, dict):
        raise CheckError(f"{where}: a schema that is not an object: {schema!r}")
    rest = {k: v for k, v in schema.items() if k not in IGNORED}
    if "nullable" in rest:
        # OpenAPI 3.0's form. schemars' draft 2020-12 settings write `null`
        # as a `type` list entry or an `anyOf` branch, which `admits_null`
        # reads; `nullable` would be read as neither.
        raise CheckError(f"{where}: `nullable` has no rule here; the schema writes `null` as a type")
    for key in ("anyOf", "oneOf"):
        if key in rest:
            members = rest.pop(key)
            if not isinstance(members, list):
                raise CheckError(f"{where}: `{key}` is not a list")
            out: list[dict] = []
            for i, member in enumerate(members):
                merged = dict(rest)
                if isinstance(member, dict):
                    merged.update(member)
                else:
                    merged = member
                out.extend(branches(merged, f"{where}|{i}"))
            return out
    if "$ref" in rest:
        return [{"$ref": ref_name(rest["$ref"], where)}]
    if "const" in rest or "enum" in rest:
        values = [rest.pop("const")] if "const" in rest else rest.pop("enum")
        if not isinstance(values, list):
            raise CheckError(f"{where}: `enum` is not a list")
        return [{"value": json.dumps(v, sort_keys=True), **rest} for v in values]
    kind = rest.get("type")
    if isinstance(kind, list):
        return [b for t in kind for b in branches({**rest, "type": t}, where)]
    if kind is None:
        if rest:
            raise CheckError(f"{where}: a schema with no type this script reads: {sorted(rest)}")
        return [{"any": True}]
    return [rest]


def value_type(encoded: str) -> str:
    value = json.loads(encoded)
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "boolean"
    if isinstance(value, int):
        return "integer"
    if isinstance(value, float):
        return "number"
    if isinstance(value, str):
        return "string"
    if isinstance(value, list):
        return "array"
    return "object"


def numeric_range(branch: dict, where: str) -> tuple[float, float, int]:
    unknown = sorted(set(branch) - NUMBER_KEYWORDS)
    if unknown:
        raise CheckError(f"{where}: number keywords this script has no rule for: {unknown}")
    fmt = branch.get("format")
    if fmt is None:
        lo, hi, precision = float("-inf"), float("inf"), 2
    elif fmt in FORMATS:
        lo, hi, precision = FORMATS[fmt]
    else:
        raise CheckError(f"{where}: number format {fmt!r} has no range in FORMATS")
    if branch.get("type") == "integer":
        precision = 0
    if isinstance(branch.get("minimum"), (int, float)):
        lo = max(lo, branch["minimum"])
    if isinstance(branch.get("maximum"), (int, float)):
        hi = min(hi, branch["maximum"])
    return lo, hi, precision


def admits_value(branch: dict, encoded: str) -> bool:
    """Whether a new-side branch accepts one value the old side named."""
    if "any" in branch:
        return True
    if "value" in branch:
        return branch["value"] == encoded
    if "$ref" in branch:
        return False
    kind = value_type(encoded)
    expected = branch.get("type")
    if expected == kind or (expected == "number" and kind == "integer"):
        if kind in ("integer", "number"):
            lo, hi, precision = numeric_range(branch, "value")
            value = json.loads(encoded)
            return lo <= value <= hi and (precision > 0 or float(value).is_integer())
        return True
    return False


def compare(old: object, new: object, direction: Direction, where: str) -> list[str]:
    """Every way `new` accepts less than `old`, with where it was found."""
    new_branches = branches(new, where)
    old_branches = branches(old, where)
    problems: list[str] = []
    if direction.output and admits_null(new_branches) and not admits_null(old_branches):
        problems.append(f"{where}: newly accepts `null` on an output type")
    for ob in old_branches:
        # One of several old alternatives of its type: a union's variant.
        union = sum(1 for b in old_branches if "value" not in b and b.get("type", 0) == ob.get("type", 1)) > 1
        problems.extend(covered(ob, new_branches, direction, where, union))
    return problems


def admits_null(alternatives: list[dict]) -> bool:
    """Whether one of a schema's alternatives accepts `null`.

    A `$ref` does not count: the definition it names is compared on its own,
    under the directions that reach it, so a `null` added there is reported
    there.
    """
    return any("any" in b or b.get("type") == "null" or b.get("value") == "null" for b in alternatives)


def describe(branch: dict) -> str:
    if "value" in branch:
        return f"the value {branch['value']}"
    if "$ref" in branch:
        return f"`{branch['$ref']}`"
    if "any" in branch:
        return "any value"
    return f"`{branch.get('type')}`"


def covered(ob: dict, new_branches: list[dict], direction: Direction, where: str, union: bool = False) -> list[str]:
    """Problems with the new alternative that best covers one old one.

    `union` says the old branch was one variant of several of its type, so
    when no new alternative covers it the report says a variant went.
    """
    if any("any" in nb for nb in new_branches):
        return []
    if "any" in ob:
        return [f"{where}: narrowed, no longer accepts any value"]
    if "value" in ob:
        if any(admits_value(nb, ob["value"]) for nb in new_branches):
            return []
        return [f"{where}: narrowed, no longer accepts {describe(ob)} (a removed enum value or variant)"]
    if "$ref" in ob:
        if any(nb.get("$ref") == ob["$ref"] for nb in new_branches):
            return []
        return [f"{where}: narrowed, no longer refers to {describe(ob)}"]
    candidates = [nb for nb in new_branches if same_kind(ob, nb)]
    if not candidates:
        return [f"{where}: narrowed, no longer accepts {describe(ob)}"]
    best: list[str] | None = None
    for nb in candidates:
        found = compare_typed(ob, nb, direction, where)
        if not found:
            return []
        if best is None or len(found) < len(best):
            best = found
    if union and best:
        # A removed or changed variant of a union: no new alternative matches,
        # and the nearest one's differences say which.
        return [f"{where}: narrowed, no variant accepts a former `{ob['type']}` one; the closest differs: " + "; ".join(best)]
    return best or []


def same_kind(ob: dict, nb: dict) -> bool:
    if "type" not in nb or "value" in nb:
        return False
    return nb["type"] == ob["type"] or (ob["type"] == "integer" and nb["type"] == "number")


def compare_typed(ob: dict, nb: dict, direction: Direction, where: str) -> list[str]:
    kind = ob["type"]
    if kind in ("integer", "number"):
        olo, ohi, oprec = numeric_range(ob, where)
        nlo, nhi, nprec = numeric_range(nb, where)
        problems = []
        if nlo > olo or nhi < ohi:
            problems.append(f"{where}: narrowed, range [{olo}, {ohi}] is now [{nlo}, {nhi}]")
        if nprec < oprec:
            problems.append(
                f"{where}: narrowed, `{ob.get('format', ob['type'])}` is now `{nb.get('format', nb['type'])}`"
            )
        return problems
    if kind == "array":
        return compare_array(ob, nb, direction, where)
    if kind == "object":
        return compare_object(ob, nb, direction, where)
    if kind in ("string", "boolean", "null"):
        leftover = sorted(k for k in set(ob) | set(nb) if k != "type" and ob.get(k) != nb.get(k))
        if leftover:
            return [f"{where}: `{kind}` keywords changed: {leftover}"]
        return []
    raise CheckError(f"{where}: type {kind!r} has no rule here")


def compare_array(ob: dict, nb: dict, direction: Direction, where: str) -> list[str]:
    problems = []
    if "items" in ob or "items" in nb:
        problems += compare(ob.get("items", True), nb.get("items", True), direction, f"{where}[]")
    if nb.get("minItems", 0) > ob.get("minItems", 0):
        problems.append(f"{where}: narrowed, minItems {ob.get('minItems', 0)} is now {nb['minItems']}")
    omax, nmax = ob.get("maxItems"), nb.get("maxItems")
    if nmax is not None and (omax is None or nmax < omax):
        problems.append(f"{where}: narrowed, maxItems {omax} is now {nmax}")
    if TYPED_ARRAY in ob and ob[TYPED_ARRAY] != nb.get(TYPED_ARRAY):
        problems.append(f"{where}: narrowed, no longer accepts a {ob[TYPED_ARRAY]}")
    unknown = sorted(set(ob) - {"type", "items", "minItems", "maxItems", TYPED_ARRAY})
    if unknown:
        raise CheckError(f"{where}: array keywords this script has no rule for: {unknown}")
    return problems


def compare_object(ob: dict, nb: dict, direction: Direction, where: str) -> list[str]:
    unknown = sorted((set(ob) | set(nb)) - {"type", "properties", "required", "additionalProperties"})
    if unknown:
        raise CheckError(f"{where}: object keywords this script has no rule for: {unknown}")
    problems = []
    old_props = ob.get("properties", {})
    new_props = nb.get("properties", {})
    old_req = set(ob.get("required", []))
    new_req = set(nb.get("required", []))
    for key, sub in old_props.items():
        if key not in new_props:
            problems.append(f"{where}.{key}: property removed")
            continue
        problems += compare(sub, new_props[key], direction, f"{where}.{key}")
    if direction.input:
        for key in sorted(new_req - old_req):
            problems.append(f"{where}.{key}: newly required on an input type")
    if direction.output:
        for key in sorted((old_req - new_req) & set(new_props)):
            problems.append(f"{where}.{key}: no longer required on an output type")
    old_extra = ob.get("additionalProperties", True)
    new_extra = nb.get("additionalProperties", True)
    if old_extra is not False:
        if new_extra is False:
            problems.append(f"{where}: narrowed, `additionalProperties` is now false")
        else:
            problems += compare(old_extra, new_extra, direction, f"{where}{{*}}")
    return problems


def check_schema(old: dict, new: dict, old_roots: HostRoots, new_roots: HostRoots) -> list[str]:
    """Every way the new schema breaks the old one's npm contract.

    Each side's roots classify that side's schema, and a type takes every
    direction either side gives it, so the working tree cannot drop a rule
    the baseline applied.
    """
    old_defs = definitions(old, "baseline schema")
    new_defs = definitions(new, "schema")
    old_in, old_out = directions(old, old_roots, "baseline schema")
    new_in, new_out = directions(new, new_roots, "schema")
    inputs, outputs = old_in | new_in, old_out | new_out
    problems = []
    for name, old_def in sorted(old_defs.items()):
        if name not in new_defs:
            problems.append(f"{name}: definition removed")
            continue
        direction = Direction(name in inputs, name in outputs)
        problems += compare(old_def, new_defs[name], direction, name)
    old_features = feature_roots(old, "baseline schema")
    new_features = feature_roots(new, "schema")
    for feature, names in sorted(old_features.items()):
        kept = set(new_features.get(feature, [])) | set(new_features.get(ALWAYS, []))
        for name in names:
            if name not in kept:
                problems.append(f"{name}: no longer a root of the `{feature}` build ({FEATURE_ROOTS})")
    return problems


def direction_table(old: dict, new: dict, old_roots: HostRoots, new_roots: HostRoots) -> str:
    old_in, old_out = directions(old, old_roots, "baseline schema")
    new_in, new_out = directions(new, new_roots, "schema")
    names = set(definitions(new, "schema"))
    inputs, outputs = (old_in | new_in) & names, (old_out | new_out) & names
    both = inputs & outputs
    return (
        f"input types ({len(inputs - both)}): {', '.join(sorted(inputs - both))}\n"
        f"types with both rules ({len(both)}): {', '.join(sorted(both))}\n"
        f"output types: {len(outputs - both)}"
    )


# ------------------------------------------------------------ contributes


def as_list(value: object, where: str) -> list:
    if value is None:
        return []
    if isinstance(value, dict):
        return [value]
    if isinstance(value, list):
        return value
    raise CheckError(f"{where}: expected a list or an object, got {type(value).__name__}")


def ids(entries: Iterable[object], key: str, where: str) -> set[str]:
    out = set()
    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get(key), str):
            raise CheckError(f"{where}: an entry without a string `{key}`: {entry!r}")
        out.add(entry[key])
    return out


def settings(contributes: dict, where: str) -> dict[str, dict]:
    out: dict[str, dict] = {}
    for section in as_list(contributes.get("configuration"), f"{where} configuration"):
        props = section.get("properties", {}) if isinstance(section, dict) else None
        if not isinstance(props, dict):
            raise CheckError(f"{where}: a configuration section without a `properties` object")
        out.update(props)
    return out


def public_commands(contributes: dict, where: str) -> set[str]:
    out = ids(as_list(contributes.get("commands"), f"{where} commands"), "command", f"{where} commands")
    menus = contributes.get("menus", {})
    if not isinstance(menus, dict):
        raise CheckError(f"{where}: `menus` is not an object")
    referenced = set()
    for menu, entries in menus.items():
        for entry in as_list(entries, f"{where} menus.{menu}"):
            if isinstance(entry, dict) and isinstance(entry.get("command"), str):
                referenced.add(entry["command"])
    for entry in as_list(contributes.get("keybindings"), f"{where} keybindings"):
        if isinstance(entry, dict) and isinstance(entry.get("command"), str):
            referenced.add(entry["command"].lstrip("-"))
    return out | {c for c in referenced if c.startswith(OWN_COMMAND_PREFIX)}


def setting_types(spec: dict) -> set[str]:
    kind = spec.get("type")
    if kind is None:
        return set()
    return set(kind) if isinstance(kind, list) else {kind}


def check_contributes(old_pkg: dict, new_pkg: dict) -> list[str]:
    """Every public id in the old `contributes` that the new one dropped."""
    old = old_pkg.get("contributes", {})
    new = new_pkg.get("contributes", {})
    if not isinstance(old, dict) or not isinstance(new, dict):
        raise CheckError("`contributes` is not an object")
    problems = []
    for side, table in (("baseline", old), ("working tree", new)):
        unknown = sorted(set(table) - CONTRIBUTES_RULES)
        if unknown:
            problems.append(
                f"{side} `contributes` has {unknown}, which this check has no rule for; "
                "add one to tools/check_release_api.py"
            )
    pairs = (
        ("languages", "id", "language id"),
        ("customEditors", "viewType", "custom editor viewType"),
    )
    for key, field, label in pairs:
        before = ids(as_list(old.get(key), key), field, f"baseline {key}")
        after = ids(as_list(new.get(key), key), field, key)
        problems += [f"{label} {name!r} removed" for name in sorted(before - after)]
    before = public_commands(old, "baseline")
    after = public_commands(new, "working tree")
    problems += [f"command id {name!r} removed" for name in sorted(before - after)]
    old_settings = settings(old, "baseline")
    new_settings = settings(new, "working tree")
    for key, spec in sorted(old_settings.items()):
        if key not in new_settings:
            problems.append(f"setting {key!r} removed")
            continue
        now = new_settings[key]
        old_types, new_types = setting_types(spec), setting_types(now)
        # A setting with no `type` takes any value, so only a typed one narrows.
        if new_types:
            lost = (old_types or {"any"}) - new_types - ({"integer"} if "number" in new_types else set())
            if lost:
                problems.append(f"setting {key!r}: type {sorted(old_types) or 'any'} is now {sorted(new_types)}")
        if "enum" in now:
            allowed = {json.dumps(v, sort_keys=True) for v in now["enum"]}
            if "enum" not in spec:
                problems.append(f"setting {key!r}: narrowed to an enum")
            else:
                for value in spec["enum"]:
                    if json.dumps(value, sort_keys=True) not in allowed:
                        problems.append(f"setting {key!r}: enum value {value!r} removed")
    return problems


# ------------------------------------------------------------------- main


def git_show(root: Path, rev: str, path: str) -> str:
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "show", f"{rev}:{path}"],
            check=True,
            capture_output=True,
        ).stdout
    except subprocess.CalledProcessError as err:
        detail = err.stderr.decode("utf-8", "replace").strip()
        raise CheckError(f"git show {rev}:{path} failed: {detail}") from err
    return out.decode("utf-8")


def read(root: Path, path: str) -> str:
    try:
        return (root / path).read_text(encoding="utf-8")
    except OSError as err:
        raise CheckError(f"cannot read {path}: {err}") from err


def load_json(text: str, where: str) -> dict:
    try:
        value = json.loads(text)
    except json.JSONDecodeError as err:
        raise CheckError(f"{where} is not JSON: {err}") from err
    if not isinstance(value, dict):
        raise CheckError(f"{where} is not a JSON object")
    return value


def git_ls(root: Path, rev: str, path: str) -> list[str]:
    try:
        out = subprocess.run(
            ["git", "-C", str(root), "ls-tree", "-r", "--name-only", rev, "--", path],
            check=True,
            capture_output=True,
        ).stdout
    except subprocess.CalledProcessError as err:
        detail = err.stderr.decode("utf-8", "replace").strip()
        raise CheckError(f"git ls-tree {rev} {path} failed: {detail}") from err
    return [line for line in out.decode("utf-8").splitlines() if line.endswith(".rs")]


def check_hand_written(sources: dict[str, str]) -> None:
    """Each HAND_WRITTEN name is still an interface the wasm crate declares."""
    text = "\n".join(sources.values())
    for name in sorted(HAND_WRITTEN):
        if not re.search(rf"export (interface|type) {re.escape(name)}\b", text):
            raise CheckError(
                f"HAND_WRITTEN names {name}, which {WASM_SRC} no longer declares; drop it from the list"
            )


def run(root: Path, baseline: str) -> tuple[list[str], str]:
    """(problems, summary of how the types were classified)."""
    old_schema = load_json(git_show(root, baseline, SCHEMA), f"{baseline}:{SCHEMA}")
    new_schema = load_json(read(root, SCHEMA), SCHEMA)
    old_sources = {p: git_show(root, baseline, p) for p in git_ls(root, baseline, WASM_SRC)}
    new_sources = {
        str(p.relative_to(root)): p.read_text(encoding="utf-8") for p in sorted((root / WASM_SRC).rglob("*.rs"))
    }
    check_hand_written(new_sources)
    old_roots = HostRoots(
        parse_sent(git_show(root, baseline, API_RULES), f"{baseline}:{API_RULES}"),
        *wasm_types(old_sources.values()),
    )
    new_roots = HostRoots(parse_sent(read(root, API_RULES), API_RULES), *wasm_types(new_sources.values()))
    old_pkg = load_json(git_show(root, baseline, PACKAGE), f"{baseline}:{PACKAGE}")
    new_pkg = load_json(read(root, PACKAGE), PACKAGE)
    problems = [f"{SCHEMA}: {p}" for p in check_schema(old_schema, new_schema, old_roots, new_roots)]
    problems += [f"{PACKAGE}: {p}" for p in check_contributes(old_pkg, new_pkg)]
    return problems, direction_table(old_schema, new_schema, old_roots, new_roots)


def read_release_type(text: str) -> str:
    """The setting, read as semver.yml reads it: `#` lines dropped, spaces removed."""
    return "".join("".join(line.split()) for line in text.splitlines() if not line.startswith("#"))


def workspace_major(root: Path) -> int:
    """The major of `fieldglass-core`'s version, the one semver.yml reads."""
    try:
        core = tomllib.loads(read(root, "crates/fieldglass-core/Cargo.toml"))
        version = core["package"]["version"]
        if isinstance(version, dict) and version.get("workspace") is True:
            version = tomllib.loads(read(root, WORKSPACE_MANIFEST))["workspace"]["package"]["version"]
        return int(str(version).split(".")[0])
    except (KeyError, ValueError, tomllib.TOMLDecodeError) as err:
        raise CheckError(f"could not read the fieldglass-core version: {err!r}") from err


def breaks_allowed(setting: str, major: int) -> bool:
    """Whether the next release may break, mirroring semver.yml's mapping.

    While the major is 0, `patch` is checked as cargo's `minor` (no break)
    and `minor` as `major` (a break is allowed). From 1.0, only `major` allows
    one. Any other value is an error, as it is there.
    """
    if major == 0:
        allowed = {"patch": False, "minor": True}
    else:
        allowed = {"patch": False, "minor": False, "major": True}
    if setting not in allowed:
        raise CheckError(
            f"{RELEASE_TYPE} must be one of {sorted(allowed)} while the workspace major is {major}, got {setting!r}"
        )
    return allowed[setting]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--baseline", required=True, help="the tag to compare with (semver_ci.py baseline)")
    parser.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    try:
        setting = read_release_type(read(args.root, RELEASE_TYPE))
        allowed = breaks_allowed(setting, workspace_major(args.root))
        problems, table = run(args.root, args.baseline)
    except CheckError as err:
        print(f"::error::{err}", file=sys.stderr)
        return 1
    print(f"Compared with {args.baseline}; next release {setting} ({RELEASE_TYPE}).\n{table}")
    if problems and allowed:
        # The cycle that may break: report each one so review sees it, as
        # the Public API job's mapped `--release-type` lets its own through.
        for p in problems:
            print(f"::warning::{p}", file=sys.stderr)
        print(
            f"{len(problems)} break(s) against {args.baseline}, allowed because {RELEASE_TYPE} says "
            f"{setting}. Each needs a CHANGELOG entry.",
            file=sys.stderr,
        )
        return 0
    if problems:
        for p in problems:
            print(f"::error::{p}", file=sys.stderr)
        print(
            f"{len(problems)} break(s) against {args.baseline}. A patch may only add to these "
            "surfaces (conventions, ADR-0018); a break goes to the next minor.",
            file=sys.stderr,
        )
        return 1
    print("No break in the npm API schema or the extension's contributions.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
