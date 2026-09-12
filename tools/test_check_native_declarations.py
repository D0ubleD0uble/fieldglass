#!/usr/bin/env python3
"""Unit tests for tools/check_native_declarations.py.

    python3 tools/test_check_native_declarations.py

What is tested is *when it fails*, over synthetic pairs of files — a test that
only ran the real repo would pass just as well against a checker that returned no
problems ever. The four directions are: a missing method, a mistyped optional, a
`| null` on something napi returns as `undefined`, and a stale allowlist entry.
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

    def run_on(self, generated: str, handwritten: str, known: set[str] | None = None):
        real = (chk.GENERATED, chk.HANDWRITTEN, chk.KNOWN_NULLABLE, chk.IGNORED_GENERATED)
        with tempfile.TemporaryDirectory() as tmp:
            g, h = Path(tmp) / "index.d.ts", Path(tmp) / "native.ts"
            g.write_text(generated, encoding="utf-8")
            h.write_text(handwritten, encoding="utf-8")
            try:
                chk.GENERATED, chk.HANDWRITTEN = g, h
                chk.KNOWN_NULLABLE = known if known is not None else set()
                chk.IGNORED_GENERATED = {}
                return chk.check()
            finally:
                chk.GENERATED, chk.HANDWRITTEN, chk.KNOWN_NULLABLE, chk.IGNORED_GENERATED = real

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
        # A hand-written object type is a *view*: `MessageMeta` has ~65 fields and
        # the extension reads 16. Completeness is checked for methods, not fields.
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

    def test_a_real_type_divergence_is_refused(self):
        hand = self.HAND.replace("  a: string;", "  a: number;")
        problems = self.run_on(self.GEN, hand)
        self.assertTrue(any("declares `number`" in p for p in problems), problems)

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

    def test_the_known_divergences_are_the_ones_named(self):
        # Pinned so the debt shrinks visibly. #574 deletes `MessageMeta`, which
        # empties this set.
        self.assertEqual(len(chk.KNOWN_NULLABLE), 23)
        self.assertTrue(
            all(k.startswith("MessageMeta.") for k in chk.KNOWN_NULLABLE),
            "every known divergence should be on the type #574 removes",
        )

    def test_every_ignored_name_has_a_reason(self):
        for name, reason in chk.IGNORED_GENERATED.items():
            self.assertTrue(reason.strip(), f"{name} is ignored with no reason")


if __name__ == "__main__":
    unittest.main(verbosity=2)
