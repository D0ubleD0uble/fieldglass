#!/usr/bin/env python3
"""Unit tests for tools/gen_api_declarations.py.

    python3 tools/test_gen_api_declarations.py

Mostly *when it fails*: a drift gate that has never been seen to fail is not
known to be connected. The cases are a hand edit to either generated file, a
schema change nobody regenerated from, and a schema keyword the generator has no
rule for. The rest pin the two rules the declarations exist to state — a
returned field with nothing to report is `T | null` under its own key, and an
option a host may leave out is `?` — and that the output does not depend on the
order the schema happens to list things in, which is what makes a conflict in a
generated file a re-run rather than a merge.

`TheSplit` covers the browser files' split by feature (#1158): which file a
definition lands in, and each schema the split refuses rather than guesses at.
"""

from __future__ import annotations

import importlib.util
import json
import re
import shutil
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "gen_api_declarations",
    Path(__file__).resolve().parent / "gen_api_declarations.py",
)
gen = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gen)


def schema(defs: dict) -> dict:
    return {"$defs": defs}


RETURNED = {
    "type": "object",
    "description": "A returned DTO.",
    "properties": {
        "name": {"type": "string", "description": "Always there."},
        "edition": {"type": ["integer", "null"], "format": "int32"},
        "grid": {"anyOf": [{"$ref": "#/$defs/Grid"}, {"type": "null"}]},
        "bounds": {
            "type": ["array", "null"],
            "items": {"type": "number"},
            "minItems": 4,
            "maxItems": 4,
        },
    },
    # The serialize contract: every field required, including the Options.
    "required": ["name", "edition", "grid", "bounds"],
}
GRID = {
    "type": "object",
    "properties": {"kind": {"type": "string"}},
    "required": ["kind"],
}
SENT = {
    "type": "object",
    "properties": {
        "projection": {"type": "string"},
        "width": {"type": ["integer", "null"], "default": None},
    },
    # The deserialize contract: a defaulted field is not required.
    "required": ["projection"],
}


class Rendering(unittest.TestCase):
    def render(self, defs: dict) -> str:
        return gen.render(schema(defs), "// header")

    def test_a_returned_none_is_a_present_null(self):
        out = self.render({"Info": RETURNED, "Grid": GRID})
        self.assertIn("  edition: number | null;", out)
        self.assertIn("  grid: Grid | null;", out)
        self.assertNotIn("edition?", out)

    def test_an_option_a_host_may_leave_out_is_optional(self):
        out = self.render({"Opts": SENT})
        self.assertIn("  projection: string;", out)
        self.assertIn("  width?: number | null;", out)

    def added(self, prop: dict, required: bool) -> dict:
        """RETURNED with a `code` property marked as added after it shipped."""
        info = json.loads(json.dumps(RETURNED))
        info["properties"]["code"] = {gen.ADDED_IN: "0.6.1", **prop}
        if required:
            info["required"].append("code")
        return {"Info": info, "Grid": GRID}

    def test_a_key_added_after_its_type_shipped_is_optional_and_nullable(self):
        # #1552: a host hands back an object it kept from before the key.
        out = self.render(self.added({"type": ["integer", "null"]}, required=False))
        self.assertIn("  code?: number | null;", out)
        # Every other returned key keeps the #574 shape.
        self.assertIn("  edition: number | null;", out)
        self.assertNotIn("edition?", out)
        out = self.render(self.added({"anyOf": [{"$ref": "#/$defs/Grid"}, {"type": "null"}]}, required=False))
        self.assertIn("  code?: Grid | null;", out)

    def test_an_added_key_still_required_is_refused(self):
        with self.assertRaises(gen.SchemaError):
            self.render(self.added({"type": ["integer", "null"]}, required=True))

    def test_an_added_key_that_cannot_be_null_is_refused(self):
        with self.assertRaises(gen.SchemaError):
            self.render(self.added({"type": "integer"}, required=False))

    def test_an_added_key_naming_no_release_is_refused(self):
        defs = self.added({"type": ["integer", "null"]}, required=False)
        defs["Info"]["properties"]["code"][gen.ADDED_IN] = "soon"
        with self.assertRaises(gen.SchemaError):
            self.render(defs)

    def test_a_fixed_length_array_is_a_tuple(self):
        out = self.render({"Info": RETURNED, "Grid": GRID})
        self.assertIn("  bounds: [number, number, number, number] | null;", out)

    def test_an_array_a_caller_may_pass_typed_says_so(self):
        # #1073: slice indices a page holds as a `Uint32Array`.
        typed = {
            "type": "object",
            "properties": {
                "at": {"type": "array", "items": {"type": "integer"}, gen.TYPED_ARRAY: "Uint32Array"}
            },
            "required": ["at"],
        }
        self.assertIn("  at: number[] | Uint32Array;", self.render({"Req": typed}))
        typed["properties"]["at"][gen.TYPED_ARRAY] = "Uint32"
        with self.assertRaises(gen.SchemaError):
            self.render({"Req": typed})

    def test_required_fields_keep_their_declaration_order(self):
        out = self.render({"Info": RETURNED, "Grid": GRID})
        order = [out.index(f"  {k}:") for k in ("name", "edition", "grid", "bounds")]
        self.assertEqual(order, sorted(order), out)

    def test_an_enum_of_consts_is_a_union_with_its_docs(self):
        out = self.render(
            {
                "Kind": {
                    "oneOf": [
                        {"const": "a", "type": "string", "description": "First."},
                        {"const": "b", "type": "string"},
                    ]
                }
            }
        )
        self.assertIn('export type Kind =\n  /**\n   * First.\n   */\n  | "a"\n  | "b";', out)

    def test_error_is_renamed_so_it_cannot_shadow_the_global(self):
        out = self.render(
            {
                "Error": {"oneOf": [{"type": "object", "properties": {"code": {"const": "x"}}}]},
                "Uses": {
                    "type": "object",
                    "properties": {"e": {"$ref": "#/$defs/Error"}},
                    "required": ["e"],
                },
            }
        )
        self.assertIn("export type ApiError =", out)
        self.assertIn("  e: ApiError;", out)
        self.assertNotIn("export type Error", out)

    def test_a_stated_loose_object_gets_an_index_signature(self):
        loose = dict(GRID, additionalProperties=True)
        out = self.render({"Grid": loose})
        self.assertIn("  [key: string]: unknown;", out)

    def test_rust_links_become_plain_text(self):
        doc = (
            "See [`kind`](Self::kind), [`Session::message`](crate::Session::message), "
            "[`Placement`], `crate::api::Georef` and [the spec](https://example.org). */"
        )
        out = self.render({"Grid": dict(GRID, description=doc)})
        self.assertIn("See `kind`, `Session::message`, `Placement`, `Georef` and", out)
        self.assertIn("[the spec](https://example.org)", out)
        self.assertIn("*\\/", out)

    def test_the_output_does_not_depend_on_schema_order(self):
        forward = {"Info": RETURNED, "Grid": GRID, "Opts": SENT}
        backward = {
            k: dict(reversed(list(v.items()))) for k, v in reversed(list(forward.items()))
        }
        self.assertEqual(self.render(forward), self.render(backward))


class Refusals(unittest.TestCase):
    def test_an_unknown_keyword_is_refused_not_widened(self):
        bad = dict(GRID, properties={"kind": {"type": "string", "pattern": "^a"}})
        with self.assertRaises(gen.SchemaError) as e:
            gen.render(schema({"Grid": bad}), "")
        self.assertIn("pattern", str(e.exception))
        self.assertIn("Grid.kind", str(e.exception))

    def test_a_combination_it_would_half_render_is_refused(self):
        # Each of these renders from one keyword and would drop the other.
        for name, node in [
            ("$ref with siblings", {"$ref": "#/$defs/Grid", "properties": {}}),
            ("oneOf with properties", {"oneOf": [{"type": "string"}], "properties": {}}),
            ("anyOf with a type", {"anyOf": [{"type": "string"}], "type": "string"}),
            ("enum on a number", {"type": "number", "enum": [1, 2]}),
            ("const with items", {"const": "a", "items": {"type": "string"}}),
            ("properties on a string", {"type": "string", "properties": {}}),
            ("items with no array type", {"type": "object", "properties": {}, "items": {}}),
        ]:
            with self.subTest(name), self.assertRaises(gen.SchemaError):
                wrapped = dict(GRID, properties={"kind": {"type": "string"}, "x": node})
                gen.render(schema({"Grid": wrapped}), "")

    def test_the_combinations_it_renders_in_full_are_accepted(self):
        for node in [
            {"type": ["array", "null"], "items": {"type": "number"}, "minItems": 2, "maxItems": 2},
            {"type": "string", "const": "a"},
            {"type": "string", "enum": ["a", "b"]},
            {"anyOf": [{"$ref": "#/$defs/Grid"}, {"type": "null"}], "description": "doc"},
        ]:
            with self.subTest(node):
                wrapped = dict(GRID, properties={"kind": {"type": "string"}, "x": node})
                gen.render(schema({"Grid": wrapped}), "")

    def test_a_dangling_ref_is_refused(self):
        bad = dict(GRID, properties={"kind": {"$ref": "#/$defs/Missing"}})
        with self.assertRaises(gen.SchemaError):
            gen.render(schema({"Grid": bad}), "")

    def test_an_object_with_no_shape_is_refused(self):
        with self.assertRaises(gen.SchemaError):
            gen.render(schema({"Blob": {"type": "object"}}), "")

    def test_an_empty_schema_is_refused(self):
        with self.assertRaises(gen.SchemaError):
            gen.render({"$defs": {}}, "")


def grouped(defs: dict, always: list, fetchplan: list, zarr: list) -> dict:
    return {"$defs": defs, gen.FEATURE_ROOTS: {"always": always, "fetchplan": fetchplan, "zarr": zarr}}


OBJ = {"type": "object", "properties": {}, "required": []}


def refers(*names: str) -> dict:
    return {
        "type": "object",
        "properties": {n.lower(): {"$ref": f"#/$defs/{n}"} for n in names},
        "required": [n.lower() for n in names],
    }


class TheSplit(unittest.TestCase):
    """Which browser file declares what (#1158)."""

    def test_a_nested_definition_follows_the_roots_that_reach_it(self):
        parts = gen.split(
            grouped(
                {
                    "Info": refers("Shared"),
                    "Shared": OBJ,
                    "Item": refers("Dialect", "Shared"),
                    "Dialect": OBJ,
                    "Plan": OBJ,
                },
                ["Info"],
                ["Item"],
                ["Plan"],
            )
        )
        self.assertEqual(parts["always"], {"Info", "Shared"})
        self.assertEqual(parts["fetchplan"], {"Item", "Dialect"})
        self.assertEqual(parts["zarr"], {"Plan"})

    def test_an_every_build_type_reaching_a_feature_root_is_refused(self):
        with self.assertRaisesRegex(gen.SchemaError, r"\['Item'\].*'fetchplan'"):
            gen.split(grouped({"Info": refers("Item"), "Item": OBJ, "Plan": OBJ}, ["Info"], ["Item"], ["Plan"]))

    def test_a_definition_only_two_features_share_is_refused(self):
        with self.assertRaisesRegex(gen.SchemaError, "declare them twice"):
            gen.split(
                grouped(
                    {"Info": OBJ, "Item": refers("Both"), "Plan": refers("Both"), "Both": OBJ},
                    ["Info"],
                    ["Item"],
                    ["Plan"],
                )
            )

    def test_a_definition_no_root_reaches_is_refused(self):
        with self.assertRaisesRegex(gen.SchemaError, r"\['Stray'\] are reached by no root"):
            gen.split(grouped({"Info": OBJ, "Item": OBJ, "Plan": OBJ, "Stray": OBJ}, ["Info"], ["Item"], ["Plan"]))

    def test_a_root_listed_twice_is_refused(self):
        with self.assertRaisesRegex(gen.SchemaError, "more than one feature"):
            gen.split(grouped({"Info": OBJ, "Plan": OBJ}, ["Info"], ["Info"], ["Plan"]))

    def test_a_feature_with_no_file_is_refused(self):
        doc = grouped({"Info": OBJ, "Item": OBJ, "Plan": OBJ}, ["Info"], ["Item"], ["Plan"])
        doc[gen.FEATURE_ROOTS]["grib3"] = []
        with self.assertRaisesRegex(gen.SchemaError, "WASM_FEATURE_OUT"):
            gen.split(doc)

    def test_a_schema_without_the_list_is_refused(self):
        with self.assertRaisesRegex(gen.SchemaError, gen.FEATURE_ROOTS):
            gen.split({"$defs": {"Info": OBJ}})


class TheDriftGate(unittest.TestCase):
    """`check` against copies of the real schema and outputs."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.schema = root / "api.schema.json"
        shutil.copyfile(gen.SCHEMA, self.schema)
        self.real = (gen.EXTENSION_OUT, gen.WASM_OUT, gen.WASM_FEATURE_OUT)
        gen.EXTENSION_OUT = root / "api.generated.ts"
        gen.WASM_OUT = root / "api.generated.d.ts"
        gen.WASM_FEATURE_OUT = {f: root / p.name for f, p in self.real[2].items()}
        shutil.copyfile(self.real[0], gen.EXTENSION_OUT)
        shutil.copyfile(self.real[1], gen.WASM_OUT)
        for feature, path in gen.WASM_FEATURE_OUT.items():
            shutil.copyfile(self.real[2][feature], path)

    def tearDown(self):
        gen.EXTENSION_OUT, gen.WASM_OUT, gen.WASM_FEATURE_OUT = self.real
        self.tmp.cleanup()

    def test_the_copies_pass(self):
        self.assertEqual(gen.check(self.schema), [])

    def test_a_hand_edit_to_the_extension_file_is_caught(self):
        text = gen.EXTENSION_OUT.read_text(encoding="utf-8")
        edited = text.replace("forecastHours: number | null;", "forecastHours?: number;")
        self.assertNotEqual(text, edited, "the edit must land for the test to mean anything")
        gen.EXTENSION_OUT.write_text(edited, encoding="utf-8")
        problems = gen.check(self.schema)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("api.generated.ts", problems[0])
        self.assertIn("-  forecastHours?: number;", problems[0])

    def test_a_hand_edit_to_the_wasm_file_is_caught(self):
        with gen.WASM_OUT.open("a", encoding="utf-8") as f:
            f.write("export interface Extra {}\n")
        problems = gen.check(self.schema)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("api.generated.d.ts", problems[0])

    def test_a_deleted_output_is_caught(self):
        gen.WASM_OUT.unlink()
        self.assertEqual(len(gen.check(self.schema)), 1)

    def test_a_hand_edit_to_a_feature_file_is_caught(self):
        for feature, path in gen.WASM_FEATURE_OUT.items():
            with self.subTest(feature=feature):
                original = path.read_text(encoding="utf-8")
                path.write_text(original + "export interface Extra {}\n", encoding="utf-8")
                problems = gen.check(self.schema)
                self.assertEqual(len(problems), 1, problems)
                self.assertIn(path.name, problems[0])
                path.unlink()
                self.assertEqual(len(gen.check(self.schema)), 1, "a deleted feature file passed")
                path.write_text(original, encoding="utf-8")
        self.assertEqual(gen.check(self.schema), [])

    def test_a_planner_type_moved_to_every_build_is_caught(self):
        # The registration the Rust test holds to the module; here, what the
        # split does with a schema that says otherwise.
        original = self.schema.read_text(encoding="utf-8")

        def moved(name: str) -> list[str]:
            doc = json.loads(original)
            roots = doc[gen.FEATURE_ROOTS]
            roots["fetchplan"].remove(name)
            roots["always"].append(name)
            self.schema.write_text(json.dumps(doc), encoding="utf-8")
            return gen.check(self.schema)

        # A type that refers to nothing else of the planner's: both files drift.
        problems = moved("Candidate")
        self.assertEqual(len(problems), 2, problems)
        default = next(p for p in problems if f"{gen.WASM_OUT.name} is not" in p)
        self.assertIn("+export interface Candidate {", default)
        # One that does: the split refuses it outright.
        problems = moved("Query")
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("only a build with 'fetchplan' binds", problems[0])

    def test_a_schema_nobody_regenerated_from_is_caught(self):
        doc = json.loads(self.schema.read_text(encoding="utf-8"))
        doc["$defs"]["MessageInfo"]["properties"]["added"] = {"type": ["string", "null"]}
        doc["$defs"]["MessageInfo"]["required"].append("added")
        self.schema.write_text(json.dumps(doc), encoding="utf-8")
        problems = gen.check(self.schema)
        self.assertEqual(len(problems), 2, problems)
        self.assertTrue(all("added: string | null;" in p for p in problems), problems)

    def test_a_missing_schema_is_an_error_not_a_pass(self):
        self.schema.unlink()
        problems = gen.check(self.schema)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("missing", problems[0])


class TheRepoItself(unittest.TestCase):
    def test_the_checked_in_declarations_are_what_the_schema_generates(self):
        self.assertEqual(gen.check(), [])

    def test_the_default_browser_file_declares_no_planner_or_zarr_type(self):
        # The acceptance item of #1158, read off the files the builds embed:
        # every planner and Zarr name is declared in its feature's file, and
        # none in the file the default package embeds.
        declared = re.compile(r"^export (?:interface|type) (\w+)", re.M)
        default = set(declared.findall(gen.WASM_OUT.read_text(encoding="utf-8")))
        planner = set(declared.findall(gen.WASM_FEATURE_OUT["fetchplan"].read_text(encoding="utf-8")))
        zarr = set(declared.findall(gen.WASM_FEATURE_OUT["zarr"].read_text(encoding="utf-8")))
        self.assertLessEqual({"PlanItem", "PlanRange", "Query", "SourceSpec", "Dialect", "Expectation"}, planner)
        self.assertEqual({"ChunkRef", "IndexLocation", "InnerChunkRef", "ShardIndexRef", "WindowPlan", "WindowRequest"}, zarr)
        self.assertEqual(default & (planner | zarr), set())
        self.assertIn("MessageInfo", default)
        # The extension's addon has every type, so it declares all of them.
        extension = set(declared.findall(gen.EXTENSION_OUT.read_text(encoding="utf-8")))
        self.assertEqual(extension, default | planner | zarr)

    def test_every_returned_optional_in_message_info_is_a_present_null(self):
        # The decision this file exists for, read off the real output.
        out = gen.EXTENSION_OUT.read_text(encoding="utf-8")
        body = out[out.index("export interface MessageInfo {") :]
        body = body[: body.index("\n}\n")]
        fields = [line.strip() for line in body.splitlines() if line.startswith("  ") and ":" in line and not line.strip().startswith("*")]
        self.assertTrue(any("| null;" in f for f in fields), fields)
        # The one optional key is the one added after MessageInfo shipped
        # (#1552), and it is still `| null`, which is what a returned None is.
        optional = [f for f in fields if "?:" in f]
        self.assertEqual(optional, ["originatingCentreCode?: number | null;"], fields)


if __name__ == "__main__":
    unittest.main(verbosity=2)
