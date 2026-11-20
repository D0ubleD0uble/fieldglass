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
ROOTS = chk.HostRoots(SENT)


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


def schema_problems(edit, sent_new=SENT, old_roots=None, new_roots=None) -> list[str]:
    old = base_schema()
    new = copy.deepcopy(old)
    edit(new["$defs"], new)
    return chk.check_schema(old, new, old_roots or ROOTS, new_roots or chk.HostRoots(sent_new))


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
        self.assertTrue(any('"slow"' in p for p in chk.check_schema(old, new, ROOTS, ROOTS)))

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

    def test_double_to_float_fails_within_the_same_bounds(self):
        # Same range on both sides, so only the precision rule can see it.
        old = base_schema()
        old["$defs"]["Out"]["properties"]["ratio"] = {"type": "number", "format": "double", "minimum": 0, "maximum": 1}
        new = copy.deepcopy(old)
        new["$defs"]["Out"]["properties"]["ratio"]["format"] = "float"
        self.assertEqual(
            chk.check_schema(old, new, ROOTS, ROOTS), ["Out.ratio: narrowed, `double` is now `float`"]
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

    def test_a_number_keyword_with_no_rule_is_an_error(self):
        for keyword, value in (("exclusiveMinimum", 0), ("multipleOf", 2), ("exclusiveMaximum", 9)):
            with self.subTest(keyword=keyword):
                with self.assertRaises(chk.CheckError):
                    schema_problems(lambda d, s: d["Out"]["properties"]["count"].update({keyword: value}))

    def test_a_fractional_value_against_an_integer_format_fails(self):
        old = base_schema()
        old["$defs"]["Out"]["properties"]["count"] = {"enum": [1.5]}
        new = base_schema()
        new["$defs"]["Out"]["properties"]["count"] = {"type": "number", "format": "uint32"}
        problems = chk.check_schema(old, new, ROOTS, ROOTS)
        self.assertTrue(any("no longer accepts the value 1.5" in p for p in problems), problems)


def window_schema() -> dict:
    """`base_schema` plus a root outside SENT that the browser host takes as
    an argument (as `WindowRequest` is), and one it returns and takes back
    (as `MessageInfo` and `PlanItem` are)."""
    s = base_schema()
    s["$defs"]["Window"] = {
        "type": "object",
        "properties": {"path": {"type": "string"}, "start": {"type": ["integer", "null"], "format": "uint64"}},
        "required": ["path"],
    }
    s["$defs"]["Info"] = {
        "type": "object",
        "properties": {"index": {"type": "integer", "format": "uint32", "minimum": 0}},
        "required": ["index"],
    }
    # `Info` is no feature root here, so only the browser host's parameter
    # and return lists give it a direction.
    s[chk.FEATURE_ROOTS]["always"] += ["Window"]
    return s


WASM = chk.HostRoots(SENT, params={"Window", "Info"}, returns={"Info", "Out"})


def window_problems(edit, old_roots=WASM, new_roots=WASM) -> list[str]:
    old = window_schema()
    new = copy.deepcopy(old)
    edit(new["$defs"])
    return chk.check_schema(old, new, old_roots, new_roots)


class HostBoundary(unittest.TestCase):
    def test_a_new_required_field_on_a_browser_argument_fails(self):
        def edit(d):
            d["Window"]["properties"]["stride"] = {"type": "integer", "format": "uint32", "minimum": 0}
            d["Window"]["required"].append("stride")

        problems = window_problems(edit)
        self.assertTrue(any("Window.stride: newly required on an input type" in p for p in problems), problems)
        # Without the wasm attribute the type is output only, and the same
        # change passes: the parameter list is what makes it input.
        self.assertEqual(window_problems(edit, ROOTS, ROOTS), [])

    def test_a_browser_argument_dropped_from_the_working_tree_keeps_its_input_rule(self):
        def edit(d):
            d["Window"]["required"].append("start")

        problems = window_problems(edit, WASM, ROOTS)
        self.assertTrue(any("Window.start: newly required" in p for p in problems), problems)

    def test_a_new_required_field_on_a_passed_back_type_fails(self):
        def edit(d):
            d["Info"]["properties"]["lead"] = {"type": ["integer", "null"], "format": "uint32"}
            d["Info"]["required"].append("lead")

        problems = window_problems(edit)
        self.assertTrue(any("Info.lead: newly required on an input type" in p for p in problems), problems)

    def test_a_field_added_in_a_release_on_a_passed_back_type_passes(self):
        # The #1552 rule: optional in the schema, marked with the release.
        def edit(d):
            d["Info"]["properties"]["lead"] = {"type": ["integer", "null"], "format": "uint32", chk.ADDED_IN: "0.6.2"}

        self.assertEqual(window_problems(edit), [])

    def test_a_passed_back_type_keeps_its_output_rule(self):
        problems = window_problems(lambda d: d["Info"]["required"].remove("index"))
        self.assertTrue(any("Info.index: no longer required on an output type" in p for p in problems), problems)

    def test_a_return_type_is_output(self):
        inputs, outputs = chk.directions(window_schema(), chk.HostRoots(SENT, returns={"Info"}), "schema")
        self.assertIn("Info", outputs)
        self.assertNotIn("Info", inputs)

    def test_a_type_the_host_names_that_is_not_a_definition_is_an_error(self):
        with self.assertRaises(chk.CheckError):
            chk.directions(window_schema(), chk.HostRoots(SENT, params={"Nowhere"}), "schema")

    def test_a_hand_written_interface_is_allowed(self):
        inputs, _ = chk.directions(window_schema(), chk.HostRoots(SENT, params={"ObjectRanges"}), "schema")
        self.assertNotIn("ObjectRanges", inputs)

    def test_a_hand_written_entry_must_still_be_declared(self):
        sources = {"lib.rs": "export interface RenderedRaster {}\nexport interface ObjectRanges {}\n"}
        chk.check_hand_written(sources)
        with self.assertRaises(chk.CheckError):
            chk.check_hand_written({"lib.rs": "export interface RenderedRaster {}\n"})


class TypeScriptNames(unittest.TestCase):
    def test_names_are_read_off_a_type_expression(self):
        cases = {
            "Stats | null": {"Stats"},
            "PaletteOptions | null | undefined": {"PaletteOptions"},
            "Candidate[]": {"Candidate"},
            '"f32" | "f64"': set(),
            "Map<string, Uint8Array | ObjectRanges | null> | Record<string, Uint8Array | ObjectRanges | null>": {
                "ObjectRanges"
            },
            '{ lut: Uint8Array; t0: number; scale: "linear" | "log10"; range: ColorRange }': {"ColorRange"},
            "{ values: Float32Array; bounds: [number, number, number, number] }": set(),
            "{ maybe?: Georef }": {"Georef"},
        }
        for expr, names in cases.items():
            with self.subTest(expr=expr):
                self.assertEqual(chk.ts_type_names(expr), names)

    def test_attributes_are_read_from_rust_source(self):
        source = (
            '#[wasm_bindgen(unchecked_param_type = "WindowRequest")] request: JsValue,\n'
            '#[wasm_bindgen(unchecked_return_type = "{ scale: \\"linear\\"; range: ColorRange }")]\n'
            '#[wasm_bindgen(unchecked_return_type = "\\"f32\\" | \\"f64\\"")]\n'
            '#[wasm_bindgen(unchecked_param_type = "Stats | null")] stats: JsValue,\n'
        )
        self.assertEqual(chk.wasm_types([source]), ({"WindowRequest", "Stats"}, {"ColorRange"}))

    def test_an_optional_parameter_is_a_parameter(self):
        source = '#[wasm_bindgen(unchecked_optional_param_type = "MessageInfo | null")] info: JsValue,\n'
        self.assertEqual(chk.wasm_types([source]), ({"MessageInfo"}, set()))
        inputs, _ = chk.directions(
            window_schema(), chk.HostRoots(SENT, *chk.wasm_types(['unchecked_optional_param_type = "Info | null"'])),
            "schema",
        )
        self.assertIn("Info", inputs)

    def test_typescript_type_has_no_rule_and_is_an_error(self):
        with self.assertRaises(chk.CheckError):
            chk.wasm_types(['#[wasm_bindgen(typescript_type = "Thing")] pub type Thing;\n'])

    def test_the_real_wasm_crate_names_only_known_types(self):
        sources = [p.read_text(encoding="utf-8") for p in (chk.REPO / chk.WASM_SRC).rglob("*.rs")]
        params, returns = chk.wasm_types(sources)
        self.assertIn("WindowRequest", params)
        self.assertIn("MessageInfo", params & returns)
        schema = json.loads((chk.REPO / chk.SCHEMA).read_text(encoding="utf-8"))
        sent = chk.parse_sent((chk.REPO / chk.API_RULES).read_text(encoding="utf-8"), "api_rules.rs")
        inputs, outputs = chk.directions(schema, chk.HostRoots(sent, params, returns), "schema")
        self.assertIn("WindowRequest", inputs)
        self.assertIn("PlanItem", inputs & outputs)


class ReleaseType(unittest.TestCase):
    def test_the_mapping_mirrors_the_public_api_job(self):
        self.assertFalse(chk.breaks_allowed("patch", 0))
        self.assertTrue(chk.breaks_allowed("minor", 0))
        self.assertFalse(chk.breaks_allowed("patch", 1))
        self.assertFalse(chk.breaks_allowed("minor", 1))
        self.assertTrue(chk.breaks_allowed("major", 1))
        for setting, major in (("major", 0), ("", 0), ("Minor", 1)):
            with self.subTest(setting=setting, major=major):
                with self.assertRaises(chk.CheckError):
                    chk.breaks_allowed(setting, major)

    def test_the_setting_is_read_like_semver_yml_reads_it(self):
        self.assertEqual(chk.read_release_type("# a comment\n minor \n"), "minor")


def nested_schema() -> dict:
    """`base_schema` with a definition only `Out` reaches and one only `Opts`
    reaches, each through a `$ref` property, and an enum only `Out` reaches."""
    s = base_schema()
    s["$defs"]["Inner"] = {"type": "object", "properties": {"label": {"type": "string"}}, "required": ["label"]}
    s["$defs"]["Sent"] = {"type": "object", "properties": {"label": {"type": "string"}}, "required": ["label"]}
    s["$defs"]["Out"]["properties"]["inner"] = {"$ref": "#/$defs/Inner"}
    s["$defs"]["Out"]["required"].append("inner")
    s["$defs"]["Opts"]["properties"]["sent"] = {"$ref": "#/$defs/Sent"}
    s["$defs"]["Kind"] = {"type": "string", "enum": ["a", "b"]}
    s["$defs"]["Out"]["properties"]["kind"] = {"$ref": "#/$defs/Kind"}
    s["$defs"]["Out"]["required"].append("kind")
    return s


def nested_problems(edit) -> list[str]:
    old = nested_schema()
    new = copy.deepcopy(old)
    edit(new["$defs"])
    return chk.check_schema(old, new, ROOTS, ROOTS)


class OutputMayBeEmpty(unittest.TestCase):
    """Owner decision on #1871 (2026-11-19), #1894: a returned value that may
    now be `null` breaks a caller; a new enum value or variant does not.

    A required output field that becomes optional is the older rule, tested
    in `SchemaBreaks` and `HostBoundary`."""

    NULL = "newly accepts `null` on an output type"

    def assertNull(self, problems: list[str], where: str) -> None:
        self.assertIn(f"{where}: {self.NULL}", problems)

    def test_a_nullable_output_field_fails(self):
        problems = schema_problems(lambda d, s: d["Out"]["properties"]["name"].update(type=["string", "null"]))
        self.assertEqual(problems, [f"Out.name: {self.NULL}"])

    def test_a_ref_output_field_wrapped_with_null_fails(self):
        def edit(d, s):
            d["Out"]["properties"]["mode"] = {"anyOf": [{"$ref": "#/$defs/Mode"}, {"type": "null"}]}

        self.assertEqual(schema_problems(edit), [f"Out.mode: {self.NULL}"])

    def test_a_wider_branch_that_includes_null_fails(self):
        for wider in ({"type": ["object", "null"]}, {}, {"type": "null", "description": "None."}):
            with self.subTest(wider=wider):

                def edit(d, s, wider=wider):
                    d["Out"]["properties"]["mode"] = {"anyOf": [{"$ref": "#/$defs/Mode"}, wider]}

                self.assertNull(schema_problems(edit), "Out.mode")

    def test_a_null_enum_value_on_an_output_type_fails(self):
        self.assertEqual(nested_problems(lambda d: d["Kind"]["enum"].append(None)), [f"Kind: {self.NULL}"])
        self.assertNull(schema_problems(lambda d, s: d["Mode"]["oneOf"].append({"const": None})), "Mode")

    def test_a_nullable_array_item_on_an_output_type_fails(self):
        old = base_schema()
        old["$defs"]["Out"]["properties"]["list"] = {"type": "array", "items": {"type": "string"}}
        new = copy.deepcopy(old)
        new["$defs"]["Out"]["properties"]["list"]["items"]["type"] = ["string", "null"]
        self.assertEqual(chk.check_schema(old, new, ROOTS, ROOTS), [f"Out.list[]: {self.NULL}"])

    def test_null_inside_a_nested_definition_only_an_output_reaches_fails(self):
        self.assertEqual(
            nested_problems(lambda d: d["Inner"]["properties"]["label"].update(type=["string", "null"])),
            [f"Inner.label: {self.NULL}"],
        )

        # The definition itself, as an `Option` field's `anyOf` would make it.
        def wrap(d):
            d["Inner"] = {"anyOf": [d["Inner"], {"type": "null"}]}

        self.assertEqual(nested_problems(wrap), [f"Inner: {self.NULL}"])

    def test_null_on_an_input_type_passes(self):
        self.assertEqual(
            nested_problems(lambda d: d["Sent"]["properties"]["label"].update(type=["string", "null"])), []
        )
        self.assertEqual(
            schema_problems(lambda d, s: d["Opts"]["properties"]["index"].update(type=["integer", "null"])), []
        )

    def test_a_field_that_was_already_nullable_passes(self):
        old = base_schema()
        old["$defs"]["Out"]["properties"]["name"]["type"] = ["string", "null"]
        new = copy.deepcopy(old)
        new["$defs"]["Out"]["properties"]["name"] = {"anyOf": [{"type": ["string", "null"]}, {"type": "integer"}]}
        self.assertEqual(chk.check_schema(old, new, ROOTS, ROOTS), [])

    def test_a_passed_back_type_gets_the_rule(self):
        problems = window_problems(lambda d: d["Info"]["properties"]["index"].update(type=["integer", "null"]))
        self.assertEqual(problems, [f"Info.index: {self.NULL}"])

    def test_new_enum_values_and_variants_on_an_output_type_pass(self):
        def edit(d):
            d["Kind"]["enum"].append("c")
            d["Shape"]["oneOf"].append(
                {"type": "object", "properties": {"kind": {"type": "string", "const": "dot"}}, "required": ["kind"]}
            )

        self.assertEqual(nested_problems(edit), [])
        inputs, outputs = chk.directions(nested_schema(), ROOTS, "schema")
        self.assertIn("Kind", outputs - inputs)
        self.assertIn("Shape", outputs - inputs)

    def test_a_new_optional_nullable_field_marked_added_in_passes(self):
        def edit(d):
            d["Inner"]["properties"]["later"] = {"type": ["string", "null"], chk.ADDED_IN: "0.6.2"}

        self.assertEqual(nested_problems(edit), [])

    def test_openapi_nullable_has_no_rule_and_is_an_error(self):
        # schemars' draft 2020-12 settings write `type` lists and `anyOf`
        # branches; OpenAPI 3.0's `nullable` keyword is neither.
        with self.assertRaises(chk.CheckError):
            schema_problems(lambda d, s: d["Out"]["properties"]["name"].update(nullable=True))


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
        self.assertEqual(chk.check_schema(old, base_schema(), ROOTS, ROOTS), [])

    def test_an_output_field_marked_added_in_may_be_optional(self):
        def edit(d, s):
            d["Out"]["properties"]["later"] = {"type": ["string", "null"], chk.ADDED_IN: "0.6.1"}

        self.assertEqual(schema_problems(edit), [])

    def test_descriptions_are_not_api(self):
        self.assertEqual(schema_problems(lambda d, s: d["Out"].update(description="Reworded.")), [])


class Directions(unittest.TestCase):
    def test_a_type_reached_from_both_sides_gets_both_rules(self):
        inputs, outputs = chk.directions(base_schema(), ROOTS, "schema")
        self.assertIn("Mode", inputs & outputs)
        self.assertNotIn("Opts", outputs)
        self.assertNotIn("Out", inputs)

    def test_a_sent_name_that_is_not_a_definition_is_an_error(self):
        with self.assertRaises(chk.CheckError):
            chk.directions(base_schema(), chk.HostRoots(["Missing"]), "schema")

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
            chk.check_schema(old, copy.deepcopy(old), ROOTS, ROOTS)


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

    def test_a_removed_unbinding_keybinding_command_fails(self):
        def make(c):
            c["keybindings"].append({"command": "-fieldglass.unbound", "key": "ctrl+u"})

        old = base_package()
        make(old["contributes"])
        new = copy.deepcopy(old)
        new["contributes"]["keybindings"].pop()
        problems = chk.check_contributes(old, new)
        self.assertTrue(any("command id 'fieldglass.unbound' removed" in p for p in problems), problems)

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

    def write(self, schema: dict, package: dict, sent: list[str], release: str = "patch", version: str = "0.6.1",
              wasm: str = 'unchecked_param_type = "Opts"',
              interfaces: str = "export interface RenderedRaster {} export interface ObjectRanges {}") -> None:
        for rel, text in (
            (chk.RELEASE_TYPE, f"# the next release\n{release}\n"),
            (chk.WORKSPACE_MANIFEST, f'[workspace.package]\nversion = "{version}"\n'),
            ("crates/fieldglass-core/Cargo.toml", "[package]\nversion.workspace = true\n"),
            (f"{chk.WASM_SRC}/lib.rs", f"// {wasm}\nconst A: &str = \"{interfaces}\";\n"),
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

    def broken(self, **kwargs) -> None:
        schema = base_schema()
        schema["$defs"]["Opts"]["required"].append("scale")
        self.write(schema, base_package(), SENT, **kwargs)

    def main(self) -> int:
        return chk.main(["--baseline", "v1.0.0", "--root", str(self.root)])

    def test_a_break_in_a_minor_cycle_is_a_warning(self):
        self.broken(release="minor")
        self.assertEqual(self.main(), 0)

    def test_a_break_in_a_patch_cycle_fails(self):
        self.broken(release="patch")
        self.assertEqual(self.main(), 1)

    def test_major_is_an_error_on_0x_and_allows_breaks_from_1_0(self):
        self.broken(release="major")
        self.assertEqual(self.main(), 1)
        self.broken(release="major", version="1.2.0")
        self.assertEqual(self.main(), 0)
        self.broken(release="minor", version="1.2.0")
        self.assertEqual(self.main(), 1)

    def test_a_nullable_output_field_is_a_warning_in_a_minor_cycle(self):
        schema = base_schema()
        schema["$defs"]["Out"]["properties"]["name"]["type"] = ["string", "null"]
        self.write(schema, base_package(), SENT, release="minor")
        problems, _ = chk.run(self.root, "v1.0.0")
        self.assertEqual(problems, [f"{chk.SCHEMA}: Out.name: newly accepts `null` on an output type"])
        self.assertEqual(self.main(), 0)
        self.write(schema, base_package(), SENT, release="patch")
        self.assertEqual(self.main(), 1)

    def test_a_host_type_outside_the_schema_fails(self):
        self.write(base_schema(), base_package(), SENT, wasm='unchecked_param_type = "Nowhere"')
        self.assertEqual(self.main(), 1)

    def test_a_missing_tag_fails(self):
        self.assertEqual(chk.main(["--baseline", "v9.9.9", "--root", str(self.root)]), 1)

    def test_a_browser_argument_at_the_tag_keeps_its_input_rule(self):
        # The tag takes `Window` as an argument; the working tree drops the
        # attribute and adds a required key. Only the tag's sources say the
        # type is input (it is a feature root, so output on its own).
        self.write(window_schema(), base_package(), SENT, wasm='unchecked_param_type = "Window"')
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", "window")
        git(self.root, "tag", "v1.1.0")
        schema = window_schema()
        schema["$defs"]["Window"]["properties"]["stride"] = {"type": "integer", "format": "uint32", "minimum": 0}
        schema["$defs"]["Window"]["required"].append("stride")
        self.write(schema, base_package(), SENT, wasm="no attribute")
        problems, _ = chk.run(self.root, "v1.1.0")
        self.assertEqual(problems, [f"{chk.SCHEMA}: Window.stride: newly required on an input type"])
        self.assertEqual(chk.main(["--baseline", "v1.1.0", "--root", str(self.root)]), 1)

    def test_a_hand_written_entry_the_crate_dropped_fails_the_run(self):
        self.write(base_schema(), base_package(), SENT, interfaces="export interface RenderedRaster {}")
        with self.assertRaises(chk.CheckError):
            chk.run(self.root, "v1.0.0")
        self.assertEqual(self.main(), 1)


if __name__ == "__main__":
    unittest.main()
