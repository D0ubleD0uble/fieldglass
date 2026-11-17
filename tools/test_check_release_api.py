#!/usr/bin/env python3
"""Unit tests for tools/check_release_api.py.

    python3 tools/test_check_release_api.py

What is tested is *when it fails*. Each kind of break is made alone in a copy
of a small synthetic schema or `package.json`, beside additive changes that
must pass, so a rule that stopped firing is a failing test here. The last
class runs the script end to end over a scratch git repository with a tag.
"""

from __future__ import annotations

import copy
import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "check_release_api", Path(__file__).resolve().parent / "check_release_api.py"
)
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)

SENT = ["Opts"]


def base_schema() -> dict:
    """An input type, an output type, and an enum both of them reach."""
    return {
        "$defs": {
            "Opts": {
                "type": "object",
                "properties": {
                    "index": {"type": "integer", "format": "uint32", "minimum": 0},
                    "scale": {"type": ["number", "null"], "format": "double"},
                    "mode": {"$ref": "#/$defs/Mode"},
                    "box": {
                        "type": "array",
                        "items": {"type": "number", "format": "double"},
                        "minItems": 4,
                        "maxItems": 4,
                    },
                    "ids": {
                        "type": "array",
                        "items": {"type": "integer", "format": "uint32", "minimum": 0},
                        "x-fieldglass-typed-array": "Uint32Array",
                    },
                },
                "required": ["index"],
            },
            "Out": {
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "mode": {"$ref": "#/$defs/Mode"},
                    "count": {"type": "integer", "format": "uint8", "minimum": 0, "maximum": 255},
                    "extra": {"type": "object", "additionalProperties": True},
                },
                "required": ["name", "mode", "count", "extra"],
                "additionalProperties": False,
            },
            "Mode": {
                "oneOf": [
                    {"type": "string", "const": "fast", "description": "Fast."},
                    {"type": "string", "const": "slow"},
                ]
            },
            "Shape": {
                "oneOf": [
                    {
                        "type": "object",
                        "properties": {"kind": {"type": "string", "const": "circle"}, "r": {"type": "number"}},
                        "required": ["kind", "r"],
                    },
                    {
                        "type": "object",
                        "properties": {"kind": {"type": "string", "const": "square"}, "side": {"type": "number"}},
                        "required": ["kind", "side"],
                    },
                ]
            },
        },
        chk.FEATURE_ROOTS: {"always": ["Opts", "Out", "Mode", "Shape"], "zarr": []},
    }


def schema_problems(edit, sent_new=SENT) -> list[str]:
    old = base_schema()
    new = copy.deepcopy(old)
    edit(new["$defs"], new)
    return chk.check_schema(old, new, SENT, sent_new)


class SchemaBreaks(unittest.TestCase):
    def assertFlags(self, problems: list[str], *needles: str) -> None:
        self.assertTrue(problems, "expected a break, got none")
        for needle in needles:
            self.assertTrue(any(needle in p for p in problems), (needle, problems))

    def test_an_unchanged_schema_passes(self):
        self.assertEqual(schema_problems(lambda d, s: None), [])

    def test_a_removed_definition_fails(self):
        self.assertFlags(schema_problems(lambda d, s: d.pop("Shape")), "Shape: definition removed")

    def test_a_removed_input_property_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["properties"].pop("scale")), "Opts.scale: property removed"
        )

    def test_a_removed_output_property_fails(self):
        def edit(d, s):
            d["Out"]["properties"].pop("count")
            d["Out"]["required"].remove("count")

        self.assertFlags(schema_problems(edit), "Out.count: property removed")

    def test_a_newly_required_input_field_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["required"].append("scale")),
            "Opts.scale: newly required on an input type",
        )

    def test_a_new_required_input_field_fails(self):
        def edit(d, s):
            d["Opts"]["properties"]["fresh"] = {"type": "boolean"}
            d["Opts"]["required"].append("fresh")

        self.assertFlags(schema_problems(edit), "Opts.fresh: newly required on an input type")

    def test_an_output_field_no_longer_required_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Out"]["required"].remove("count")),
            "Out.count: no longer required on an output type",
        )

    def test_a_type_dropped_from_sent_keeps_its_input_rule(self):
        # The baseline's SENT still names Opts, so the working tree's list
        # cannot take it out of the input rules.
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["required"].append("scale"), sent_new=["Out"]),
            "newly required on an input type",
        )

    def test_a_removed_enum_value_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Mode"]["oneOf"].pop()), 'Mode: narrowed, no longer accepts the value "slow"'
        )

    def test_a_removed_enum_keyword_value_fails(self):
        def make(d, s):
            d["Mode"] = {"type": "string", "enum": ["fast", "slow"]}

        old = base_schema()
        make(old["$defs"], old)
        new = copy.deepcopy(old)
        new["$defs"]["Mode"]["enum"] = ["fast"]
        self.assertTrue(any('"slow"' in p for p in chk.check_schema(old, new, SENT, SENT)))

    def test_a_removed_variant_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Shape"]["oneOf"].pop()),
            "Shape: narrowed, no variant accepts a former `object` one",
            'no longer accepts the value "square"',
        )

    def test_a_dropped_null_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["properties"]["scale"].update(type="number")),
            "Opts.scale: narrowed, no longer accepts `null`",
        )

    def test_number_to_integer_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["properties"]["scale"].update(type=["integer", "null"])),
            "Opts.scale: narrowed",
        )

    def test_a_smaller_integer_format_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["properties"]["index"].update(format="uint8")),
            "Opts.index: narrowed, range",
        )

    def test_a_tighter_bound_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Out"]["properties"]["count"].update(maximum=100)), "Out.count: narrowed"
        )

    def test_double_to_float_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["properties"]["box"]["items"].update(format="float")),
            "Opts.box[]: narrowed",
        )

    def test_a_shorter_tuple_fails(self):
        def edit(d, s):
            d["Opts"]["properties"]["box"].update(minItems=3, maxItems=3)

        self.assertFlags(schema_problems(edit), "Opts.box: narrowed, maxItems 4 is now 3")

    def test_a_dropped_typed_array_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Opts"]["properties"]["ids"].pop("x-fieldglass-typed-array")),
            "no longer accepts a Uint32Array",
        )

    def test_a_ref_to_another_definition_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Out"]["properties"]["mode"].update({"$ref": "#/$defs/Shape"})),
            "Out.mode: narrowed, no longer refers to `Mode`",
        )

    def test_additional_properties_turned_off_fails(self):
        self.assertFlags(
            schema_problems(lambda d, s: d["Out"]["properties"]["extra"].update(additionalProperties=False)),
            "Out.extra: narrowed, `additionalProperties` is now false",
        )

    def test_a_root_moved_behind_a_feature_fails(self):
        def edit(d, s):
            s[chk.FEATURE_ROOTS]["always"].remove("Shape")
            s[chk.FEATURE_ROOTS]["zarr"].append("Shape")

        self.assertFlags(schema_problems(edit), "Shape: no longer a root of the `always` build")


class SchemaAdditions(unittest.TestCase):
    def test_additive_changes_pass(self):
        def edit(d, s):
            # An optional input field, a required output field, a new
            # definition and root, a new enum value, a new variant, and
            # widenings: uint8 to uint32, integer to number, a new `null`.
            d["Opts"]["properties"]["fresh"] = {"type": ["boolean", "null"]}
            d["Out"]["properties"]["added"] = {"type": "string"}
            d["Out"]["required"].append("added")
            d["New"] = {"type": "object", "properties": {}, "required": []}
            s[chk.FEATURE_ROOTS]["always"].append("New")
            d["Mode"]["oneOf"].append({"type": "string", "const": "medium"})
            d["Shape"]["oneOf"].append({"type": "object", "properties": {}, "required": []})
            d["Out"]["properties"]["count"] = {"type": "integer", "format": "uint32", "minimum": 0}
            d["Opts"]["properties"]["index"] = {"type": ["number", "null"], "format": "double"}
            d["Opts"]["properties"]["box"].pop("maxItems")

        self.assertEqual(schema_problems(edit), [])

    def test_a_root_moved_to_every_build_passes(self):
        old = base_schema()
        old[chk.FEATURE_ROOTS]["always"].remove("Shape")
        old[chk.FEATURE_ROOTS]["zarr"].append("Shape")
        self.assertEqual(chk.check_schema(old, base_schema(), SENT, SENT), [])

    def test_an_output_field_marked_added_in_may_be_optional(self):
        def edit(d, s):
            d["Out"]["properties"]["later"] = {"type": ["string", "null"], chk.ADDED_IN: "0.6.1"}

        self.assertEqual(schema_problems(edit), [])

    def test_descriptions_are_not_api(self):
        self.assertEqual(schema_problems(lambda d, s: d["Out"].update(description="Reworded.")), [])


class Directions(unittest.TestCase):
    def test_a_type_reached_from_both_sides_gets_both_rules(self):
        inputs, outputs = chk.directions(base_schema(), SENT, "schema")
        self.assertIn("Mode", inputs & outputs)
        self.assertNotIn("Opts", outputs)
        self.assertNotIn("Out", inputs)

    def test_a_sent_name_that_is_not_a_definition_is_an_error(self):
        with self.assertRaises(chk.CheckError):
            chk.directions(base_schema(), ["Missing"], "schema")

    def test_sent_is_read_from_the_rust_list(self):
        text = 'x\nconst SENT: &[&str] = &[\n    "DecodeOptions",\n    // note\n    "Query",\n];\n'
        self.assertEqual(chk.parse_sent(text, "api_rules.rs"), ["DecodeOptions", "Query"])

    def test_a_missing_sent_list_is_an_error(self):
        with self.assertRaises(chk.CheckError):
            chk.parse_sent("const OTHER: &[&str] = &[];", "api_rules.rs")

    def test_an_unknown_number_format_is_an_error(self):
        def edit(d, s):
            d["Out"]["properties"]["count"]["format"] = "uint128"

        with self.assertRaises(chk.CheckError):
            schema_problems(edit)

    def test_an_unknown_array_keyword_is_an_error(self):
        def edit(d, s):
            d["Opts"]["properties"]["box"]["uniqueItems"] = True

        old = base_schema()
        edit(old["$defs"], old)
        with self.assertRaises(chk.CheckError):
            chk.check_schema(old, copy.deepcopy(old), SENT, SENT)


def base_package() -> dict:
    return {
        "name": "fieldglass",
        "contributes": {
            "languages": [{"id": "fieldglass-grib1", "extensions": [".grb"]}],
            "customEditors": [{"viewType": "fieldglass.viewer", "selector": []}],
            "commands": [{"command": "fieldglass.openZarrStore", "title": "Open"}],
            "menus": {"explorer/context": [{"command": "fieldglass.fromMenu"}]},
            "keybindings": [
                {"command": "fieldglass.fromKey", "key": "ctrl+k"},
                {"command": "workbench.action.files.save", "key": "ctrl+s"},
            ],
            "configuration": {
                "title": "Fieldglass",
                "properties": {
                    "fieldglass.colormap": {"type": "string", "enum": ["viridis", "gray"]},
                    "fieldglass.limit": {"type": "integer"},
                },
            },
        },
    }


def package_problems(edit) -> list[str]:
    old = base_package()
    new = copy.deepcopy(old)
    edit(new["contributes"])
    return chk.check_contributes(old, new)


class ContributesBreaks(unittest.TestCase):
    def assertFlags(self, problems: list[str], needle: str) -> None:
        self.assertTrue(any(needle in p for p in problems), (needle, problems))

    def test_an_unchanged_package_passes(self):
        self.assertEqual(package_problems(lambda c: None), [])

    def test_a_removed_setting_fails(self):
        self.assertFlags(
            package_problems(lambda c: c["configuration"]["properties"].pop("fieldglass.limit")),
            "setting 'fieldglass.limit' removed",
        )

    def test_a_removed_setting_in_a_list_of_sections_fails(self):
        def edit(c):
            c["configuration"] = [{"title": "A", "properties": {"fieldglass.colormap": {"type": "string"}}}]

        self.assertFlags(package_problems(edit), "setting 'fieldglass.limit' removed")

    def test_a_removed_setting_enum_value_fails(self):
        self.assertFlags(
            package_problems(lambda c: c["configuration"]["properties"]["fieldglass.colormap"].update(enum=["viridis"])),
            "enum value 'gray' removed",
        )

    def test_a_narrowed_setting_type_fails(self):
        self.assertFlags(
            package_problems(lambda c: c["configuration"]["properties"]["fieldglass.colormap"].update(type="number")),
            "setting 'fieldglass.colormap': type",
        )

    def test_a_removed_command_fails(self):
        self.assertFlags(package_problems(lambda c: c["commands"].clear()), "command id 'fieldglass.openZarrStore'")

    def test_a_removed_menu_command_fails(self):
        self.assertFlags(package_problems(lambda c: c["menus"].clear()), "command id 'fieldglass.fromMenu'")

    def test_a_removed_keybinding_command_fails(self):
        self.assertFlags(package_problems(lambda c: c["keybindings"].pop(0)), "command id 'fieldglass.fromKey'")

    def test_a_removed_custom_editor_fails(self):
        self.assertFlags(package_problems(lambda c: c["customEditors"].clear()), "viewType 'fieldglass.viewer'")

    def test_a_removed_language_fails(self):
        self.assertFlags(package_problems(lambda c: c["languages"].clear()), "language id 'fieldglass-grib1'")

    def test_a_contribution_with_no_rule_fails(self):
        self.assertFlags(package_problems(lambda c: c.update(views={})), "no rule for")


class ContributesAdditions(unittest.TestCase):
    def test_additions_pass(self):
        def edit(c):
            c["languages"].append({"id": "fieldglass-zarr"})
            c["customEditors"].append({"viewType": "fieldglass.viewer.any"})
            c["commands"].append({"command": "fieldglass.new", "title": "New"})
            c["configuration"]["properties"]["fieldglass.new"] = {"type": "boolean"}
            c["configuration"]["properties"]["fieldglass.colormap"]["enum"].append("magma")
            c["configuration"]["properties"]["fieldglass.limit"]["type"] = ["number", "null"]

        self.assertEqual(package_problems(edit), [])

    def test_a_menu_command_moved_to_commands_passes(self):
        def edit(c):
            c["menus"].clear()
            c["commands"].append({"command": "fieldglass.fromMenu", "title": "Moved"})

        self.assertEqual(package_problems(edit), [])

    def test_a_builtin_command_in_a_keybinding_is_not_ours_to_keep(self):
        self.assertEqual(package_problems(lambda c: c["keybindings"].pop(1)), [])

    def test_no_configuration_on_either_side_passes(self):
        old = base_package()
        old["contributes"].pop("configuration")
        self.assertEqual(chk.check_contributes(old, copy.deepcopy(old)), [])


def git(root: Path, *args: str) -> None:
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "test",
        "GIT_AUTHOR_EMAIL": "test@example.invalid",
        "GIT_COMMITTER_NAME": "test",
        "GIT_COMMITTER_EMAIL": "test@example.invalid",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
    }
    # A hook runs with GIT_DIR and GIT_INDEX_FILE set to the outer repository;
    # left in place they would point these commands at it.
    for key in ("GIT_DIR", "GIT_INDEX_FILE", "GIT_WORK_TREE"):
        env.pop(key, None)
    subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, env=env)


class EndToEnd(unittest.TestCase):
    """`main` over a scratch repository: the tag read through `git show`."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.write(base_schema(), base_package(), SENT)
        git(self.root, "init", "-q")
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "base")
        git(self.root, "tag", "v1.0.0")

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, schema: dict, package: dict, sent: list[str]) -> None:
        for rel, text in (
            (chk.SCHEMA, json.dumps(schema, indent=2)),
            (chk.PACKAGE, json.dumps(package, indent=2)),
            (chk.API_RULES, "const SENT: &[&str] = &[" + "".join(f'"{n}",' for n in sent) + "];\n"),
        ):
            path = self.root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")

    def test_the_tagged_tree_passes(self):
        self.assertEqual(chk.main(["--baseline", "v1.0.0", "--root", str(self.root)]), 0)

    def test_a_break_in_the_working_tree_fails(self):
        schema = base_schema()
        schema["$defs"]["Opts"]["required"].append("scale")
        package = base_package()
        package["contributes"]["commands"].clear()
        self.write(schema, package, SENT)
        problems, _ = chk.run(self.root, "v1.0.0")
        self.assertEqual(len(problems), 2, problems)
        self.assertEqual(chk.main(["--baseline", "v1.0.0", "--root", str(self.root)]), 1)

    def test_a_missing_tag_fails(self):
        self.assertEqual(chk.main(["--baseline", "v9.9.9", "--root", str(self.root)]), 1)


if __name__ == "__main__":
    unittest.main()
