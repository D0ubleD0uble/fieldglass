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
"""

from __future__ import annotations

import importlib.util
import json
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

    def test_a_fixed_length_array_is_a_tuple(self):
        out = self.render({"Info": RETURNED, "Grid": GRID})
        self.assertIn("  bounds: [number, number, number, number] | null;", out)

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


class TheDriftGate(unittest.TestCase):
    """`check` against copies of the real schema and outputs."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.schema = root / "api.schema.json"
        shutil.copyfile(gen.SCHEMA, self.schema)
        self.real = (gen.EXTENSION_OUT, gen.WASM_OUT)
        gen.EXTENSION_OUT = root / "api.generated.ts"
        gen.WASM_OUT = root / "api.generated.d.ts"
        shutil.copyfile(self.real[0], gen.EXTENSION_OUT)
        shutil.copyfile(self.real[1], gen.WASM_OUT)

    def tearDown(self):
        gen.EXTENSION_OUT, gen.WASM_OUT = self.real
        self.tmp.cleanup()

    def test_the_copies_pass(self):
        self.assertEqual(gen.check(self.schema), [])

    def test_a_hand_edit_to_the_extension_file_is_caught(self):
        text = gen.EXTENSION_OUT.read_text(encoding="utf-8")
        edited = text.replace("edition: number | null;", "edition?: number;")
        self.assertNotEqual(text, edited, "the edit must land for the test to mean anything")
        gen.EXTENSION_OUT.write_text(edited, encoding="utf-8")
        problems = gen.check(self.schema)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("api.generated.ts", problems[0])
        self.assertIn("-  edition?: number;", problems[0])

    def test_a_hand_edit_to_the_wasm_file_is_caught(self):
        with gen.WASM_OUT.open("a", encoding="utf-8") as f:
            f.write("export interface Extra {}\n")
        problems = gen.check(self.schema)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("api.generated.d.ts", problems[0])

    def test_a_deleted_output_is_caught(self):
        gen.WASM_OUT.unlink()
        self.assertEqual(len(gen.check(self.schema)), 1)

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

    def test_every_returned_optional_in_message_info_is_a_present_null(self):
        # The decision this file exists for, read off the real output.
        out = gen.EXTENSION_OUT.read_text(encoding="utf-8")
        body = out[out.index("export interface MessageInfo {") :]
        body = body[: body.index("\n}\n")]
        fields = [line.strip() for line in body.splitlines() if line.startswith("  ") and ":" in line and not line.strip().startswith("*")]
        self.assertTrue(any("| null;" in f for f in fields), fields)
        self.assertFalse(any("?:" in f for f in fields), fields)


if __name__ == "__main__":
    unittest.main(verbosity=2)
