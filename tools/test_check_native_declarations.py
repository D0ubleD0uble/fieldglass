#!/usr/bin/env python3
"""Unit tests for tools/check_native_declarations.py.

    python3 tools/test_check_native_declarations.py

What is tested is *when it fails*, over synthetic pairs of files — a test that
only ran the real repo would pass just as well against a checker that returned no
problems ever. The four directions are: a missing method, a mistyped optional, a
`| null` on something napi returns as `undefined`, and a stale allowlist entry.
A fifth covers the API types the addon returns through serde (#574): a name a
generated signature uses must be one the generated declarations export. A sixth
covers the wire contract itself (#574): an object the addon returns may not
leave a key out, and one marked `use_nullable` must be declared `T | null`.
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "check_native_declarations",
    Path(__file__).resolve().parent / "check_native_declarations.py",
)
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)


class Synthetic(unittest.TestCase):
    """Run `check` against a written pair of files."""

    def run_on(
        self,
        generated: str,
        handwritten: str,
        known: set[str] | None = None,
        api: str = "",
        inputs: dict[str, str] | None = None,
    ):
        real = (
            chk.GENERATED,
            chk.HANDWRITTEN,
            chk.API_GENERATED,
            chk.KNOWN_NULLABLE,
            chk.IGNORED_GENERATED,
            chk.INPUT_OBJECTS,
        )
        with tempfile.TemporaryDirectory() as tmp:
            g, h = Path(tmp) / "index.d.ts", Path(tmp) / "native.ts"
            a = Path(tmp) / "api.generated.ts"
            g.write_text(generated, encoding="utf-8")
            h.write_text(handwritten, encoding="utf-8")
            a.write_text(api, encoding="utf-8")
            try:
                chk.GENERATED, chk.HANDWRITTEN, chk.API_GENERATED = g, h, a
                chk.KNOWN_NULLABLE = known if known is not None else set()
                chk.IGNORED_GENERATED = {}
                # `O` below has an optional field, which only an object a caller
                # *sends* may have; the tests about returned objects pass `{}`.
                chk.INPUT_OBJECTS = inputs if inputs is not None else {"O": "an input"}
                return chk.check()
            finally:
                (
                    chk.GENERATED,
                    chk.HANDWRITTEN,
                    chk.API_GENERATED,
                    chk.KNOWN_NULLABLE,
                    chk.IGNORED_GENERATED,
                    chk.INPUT_OBJECTS,
                ) = real

    GEN = """
export declare class H {
  static open(p: string): H
  read(): number
}
export interface O {
  a: string
  b?: number
}
"""
    HAND = """
export interface H {
  read(): number;
}
export interface HCtor {
  open(p: string): H;
}
export interface O {
  a: string;
  b?: number;
}
"""

    def test_a_matching_pair_passes(self):
        self.assertEqual(self.run_on(self.GEN, self.HAND), [])

    def test_a_missing_method_is_refused(self):
        hand = self.HAND.replace("  read(): number;\n", "")
        problems = self.run_on(self.GEN, hand)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("H.read", problems[0])

    def test_a_missing_static_is_refused(self):
        hand = self.HAND.replace("  open(p: string): H;\n", "")
        problems = self.run_on(self.GEN, hand)
        self.assertTrue(any("H.open" in p for p in problems), problems)

    def test_an_optional_declared_required_is_refused(self):
        hand = self.HAND.replace("  b?: number;", "  b: number;")
        problems = self.run_on(self.GEN, hand)
        self.assertTrue(any("optional" in p and "required" in p for p in problems), problems)

    def test_a_null_union_on_an_optional_is_refused(self):
        # The #288 shape, and the whole reason this checker exists.
        hand = self.HAND.replace("  b?: number;", "  b: number | null;")
        problems = self.run_on(self.GEN, hand)
        self.assertTrue(any("#288" in p for p in problems), problems)

    def test_a_known_divergence_is_allowed(self):
        hand = self.HAND.replace("  b?: number;", "  b: number | null;")
        self.assertEqual(self.run_on(self.GEN, hand, known={"O.b"}), [])

    def test_a_stale_known_entry_is_refused(self):
        # The ratchet's second direction: a fixed field must leave the list.
        problems = self.run_on(self.GEN, self.HAND, known={"O.b"})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("stale", problems[0])

    def test_a_field_missing_from_an_object_view_is_allowed(self):
        # A hand-written object type is a *view*: a field nothing reads need not
        # be declared. Completeness is checked for methods, not fields.
        hand = self.HAND.replace("  a: string;\n", "")
        self.assertEqual(self.run_on(self.GEN, hand), [])

    def test_an_inherited_method_counts_as_declared(self):
        hand = """
export interface Base {
  read(): number;
}
export interface H extends Base {
}
export interface HCtor {
  open(p: string): H;
}
export interface O {
  a: string;
  b?: number;
}
"""
        self.assertEqual(self.run_on(self.GEN, hand), [])

    def test_a_narrowed_string_union_is_allowed(self):
        gen = self.GEN.replace("  a: string\n", "  a: string\n")
        hand = self.HAND.replace('  a: string;', '  a: "x" | "y";')
        self.assertEqual(self.run_on(gen, hand), [])

    def test_a_narrowing_keeps_the_null_napi_sends(self):
        # A nullable `string` narrowed to a literal union must stay nullable;
        # dropping the `| null` is the #288 shape, and so is adding one.
        gen = self.NULLABLE_GEN.replace("  a: string\n", "  a: string | null\n")
        kept = self.NULLABLE_HAND.replace('  a: string;', '  a: "x" | "y" | null;')
        self.assertEqual(self.run_on(gen, kept, inputs={}), [])
        dropped = self.NULLABLE_HAND.replace('  a: string;', '  a: "x" | "y";')
        problems = self.run_on(gen, dropped, inputs={})
        self.assertTrue(any("O.a" in p and "string | null" in p for p in problems), problems)
        added = self.NULLABLE_HAND.replace('  a: string;', '  a: "x" | "y" | null;')
        problems = self.run_on(self.NULLABLE_GEN, added, inputs={})
        self.assertTrue(any("O.a" in p for p in problems), problems)

    def test_a_real_type_divergence_is_refused(self):
        hand = self.HAND.replace("  a: string;", "  a: number;")
        problems = self.run_on(self.GEN, hand)
        self.assertTrue(any("declares `number`" in p for p in problems), problems)

    # A method returning an API type through serde (#574): napi writes the name
    # from `ts_return_type` and declares nothing for it.
    SERDE_GEN = GEN.replace("  read(): number\n", "  read(): number\n  info(i: number): MessageInfo\n")
    SERDE_HAND = HAND.replace("  read(): number;\n", "  read(): number;\n  info(i: number): MessageInfo;\n")

    def test_a_serde_return_type_the_api_declares_passes(self):
        api = "export interface MessageInfo {\n  edition: number | null;\n}\n"
        self.assertEqual(self.run_on(self.SERDE_GEN, self.SERDE_HAND, api=api), [])

    def test_a_serde_return_type_nothing_declares_is_refused(self):
        problems = self.run_on(self.SERDE_GEN, self.SERDE_HAND, api="export interface Other {}\n")
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("`MessageInfo`", problems[0])

    def test_a_capitalised_string_literal_is_not_a_type_name(self):
        gen = self.GEN.replace("  read(): number\n", '  read(): "Linear" | \'Log\'\n')
        hand = self.HAND.replace("  read(): number;\n", '  read(): "Linear" | "Log";\n')
        self.assertEqual(self.run_on(gen, hand), [])

    def test_a_name_only_in_a_comment_does_not_count_as_declared(self):
        api = "// export interface MessageInfo {}\n"
        problems = self.run_on(self.SERDE_GEN, self.SERDE_HAND, api=api)
        self.assertTrue(any("`MessageInfo`" in p for p in problems), problems)

    # The wire contract (#574): what the addon returns carries every key.
    NULLABLE_GEN = GEN.replace("  b?: number\n", "  b: number | null\n  c: Array<number> | null\n")
    NULLABLE_HAND = HAND.replace("  b?: number;", "  b: number | null;\n  c: number[] | null;")

    def test_a_returned_object_with_an_optional_field_is_refused(self):
        problems = self.run_on(self.GEN, self.HAND, inputs={})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("O.b", problems[0])
        self.assertIn("leaves the key out", problems[0])

    def test_a_nullable_returned_object_declared_nullable_passes(self):
        self.assertEqual(self.run_on(self.NULLABLE_GEN, self.NULLABLE_HAND, inputs={}), [])

    def test_a_nullable_field_declared_optional_is_refused(self):
        # The addon writes `null` under the key; `field?: T` would have a caller
        # guard with `=== undefined` and miss it.
        hand = self.NULLABLE_HAND.replace("  b: number | null;", "  b?: number;")
        problems = self.run_on(self.NULLABLE_GEN, hand, inputs={})
        self.assertTrue(any("O.b" in p and "optional" in p for p in problems), problems)

    def test_a_nullable_field_declared_without_null_is_refused(self):
        hand = self.NULLABLE_HAND.replace("  b: number | null;", "  b: number;")
        problems = self.run_on(self.NULLABLE_GEN, hand, inputs={})
        self.assertTrue(any("declares `number`" in p for p in problems), problems)

    def test_a_stale_input_entry_is_refused(self):
        problems = self.run_on(self.GEN, self.HAND, inputs={"O": "an input", "Gone": "x"})
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("Gone", problems[0])
        self.assertIn("stale", problems[0])

    def test_an_unparsable_generated_file_is_refused_not_passed(self):
        problems = self.run_on("// nothing at all\n", self.HAND)
        self.assertEqual(len(problems), 1, problems)
        self.assertIn("stopped measuring", problems[0])


class TheRepoItselfPasses(unittest.TestCase):
    def test_the_real_files_agree(self):
        if not chk.GENERATED.is_file():
            self.skipTest(
                "the addon is not built in this checkout; CI runs this after "
                "`napi build`, and the checker itself errors rather than skipping"
            )
        self.assertEqual(chk.check(), [])

    def test_there_are_no_known_divergences(self):
        # #574 deleted `MessageMeta`, whose 23 fields were the whole list. An
        # entry added back is a declaration that disagrees with the runtime.
        self.assertEqual(chk.KNOWN_NULLABLE, set())

    def test_the_only_input_object_is_the_render_options(self):
        # Pinned: an object added here is exempt from the every-key contract,
        # which a reviewer should see happen.
        self.assertEqual(set(chk.INPUT_OBJECTS), {"RenderOptions"})
        for name, reason in chk.INPUT_OBJECTS.items():
            self.assertTrue(reason.strip(), f"{name} is an input with no reason")

    def test_every_ignored_name_has_a_reason(self):
        for name, reason in chk.IGNORED_GENERATED.items():
            self.assertTrue(reason.strip(), f"{name} is ignored with no reason")


if __name__ == "__main__":
    unittest.main(verbosity=2)
