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
exception list, as the semver job has none.

**Which schema types are input.** The schema does not say; the rule that
decides it is `SENT` in `crates/fieldglass/tests/api_rules.rs`, the option
types a host sends, which the `api_rules` test reads to pick each type's
generator (serialize contract for what a host receives, deserialize contract
for what it sends). This script reads that list from the baseline's copy and
from the working tree's, and takes both. A type is input when a `SENT` root
reaches it through `$ref`, and output when a root type outside `SENT` does
(the roots are the schema's `x-fieldglass-feature-roots`). A type both reach
gets both sets of rules; one neither reaches gets both too. Reading the
baseline's list as well means a PR cannot move a type out of `SENT` to loosen
its check.

What fails, on the schema:

* a definition removed, or a root type moved out of the build it was in
  (`x-fieldglass-feature-roots`, which decides the npm entry that declares it);
* a property removed, from any type;
* a property newly required on an input type, and a property no longer
  required on an output type (a key a host could rely on being present);
* a narrowed type: a value the baseline schema accepted that the new one
  does not. That covers a removed enum value or `oneOf` variant, a dropped
  `null`, `number` to `integer`, a smaller integer format or bound, a tighter
  array length, a `$ref` to another definition, and `additionalProperties`
  turned off.

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


def directions(schema: dict, sent: Iterable[str], where: str) -> tuple[set[str], set[str]]:
    """(input names, output names) in one schema. A name in neither is in both."""
    defs = definitions(schema, where)
    sent = set(sent)
    missing = sorted(sent - set(defs))
    if missing:
        raise CheckError(f"{where}: SENT names {missing}, which are not definitions")
    roots = {r for names in feature_roots(schema, where).values() for r in names}
    inputs = reach(defs, sent, where)
    outputs = reach(defs, roots - sent, where)
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
    problems: list[str] = []
    for ob in branches(old, where):
        problems.extend(covered(ob, new_branches, direction, where))
    return problems


def describe(branch: dict) -> str:
    if "value" in branch:
        return f"the value {branch['value']}"
    if "$ref" in branch:
        return f"`{branch['$ref']}`"
    if "any" in branch:
        return "any value"
    return f"`{branch.get('type')}`"


def covered(ob: dict, new_branches: list[dict], direction: Direction, where: str) -> list[str]:
    """Problems with the new alternative that best covers one old one."""
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
    if len(candidates) > 1 and best:
        # A removed variant of a union: no new alternative matches, and the
        # nearest one's differences say which.
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


def check_schema(old: dict, new: dict, old_sent: list[str], new_sent: list[str]) -> list[str]:
    """Every way the new schema breaks the old one's npm contract."""
    old_defs = definitions(old, "baseline schema")
    new_defs = definitions(new, "schema")
    old_in, old_out = directions(old, old_sent, "baseline schema")
    new_in, new_out = directions(new, new_sent, "schema")
    inputs, outputs = old_in | new_in, old_out | new_out
    problems = []
    for name, old_def in sorted(old_defs.items()):
        if name not in new_defs:
            problems.append(f"{name}: definition removed")
            continue
        direction = Direction(name in inputs, name in outputs)
        problems += compare(old_def, new_defs[name], direction, name)
    old_roots = feature_roots(old, "baseline schema")
    new_roots = feature_roots(new, "schema")
    for feature, names in sorted(old_roots.items()):
        kept = set(new_roots.get(feature, [])) | set(new_roots.get(ALWAYS, []))
        for name in names:
            if name not in kept:
                problems.append(f"{name}: no longer a root of the `{feature}` build ({FEATURE_ROOTS})")
    return problems


def direction_table(schema: dict, sent: list[str], baseline_sent: list[str]) -> str:
    inputs, outputs = directions(schema, set(sent) | set(baseline_sent), "schema")
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


def run(root: Path, baseline: str) -> tuple[list[str], str]:
    """(problems, summary of how the types were classified)."""
    old_schema = load_json(git_show(root, baseline, SCHEMA), f"{baseline}:{SCHEMA}")
    new_schema = load_json(read(root, SCHEMA), SCHEMA)
    old_sent = parse_sent(git_show(root, baseline, API_RULES), f"{baseline}:{API_RULES}")
    new_sent = parse_sent(read(root, API_RULES), API_RULES)
    old_pkg = load_json(git_show(root, baseline, PACKAGE), f"{baseline}:{PACKAGE}")
    new_pkg = load_json(read(root, PACKAGE), PACKAGE)
    problems = [f"{SCHEMA}: {p}" for p in check_schema(old_schema, new_schema, old_sent, new_sent)]
    problems += [f"{PACKAGE}: {p}" for p in check_contributes(old_pkg, new_pkg)]
    return problems, direction_table(new_schema, new_sent, old_sent)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--baseline", required=True, help="the tag to compare with (semver_ci.py baseline)")
    parser.add_argument("--root", type=Path, default=REPO, help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    try:
        problems, table = run(args.root, args.baseline)
    except CheckError as err:
        print(f"::error::{err}", file=sys.stderr)
        return 1
    print(f"Compared with {args.baseline}.\n{table}")
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
