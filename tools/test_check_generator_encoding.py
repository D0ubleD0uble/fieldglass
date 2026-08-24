#!/usr/bin/env python3
"""Unit tests for tools/check_generator_encoding.py.

The checker gates commits, and a checker that silently passes everything is
worse than none — so what it must *catch* is pinned alongside what it must let
through. Pure AST, no filesystem. Run:

    python3 tools/test_check_generator_encoding.py
"""

from __future__ import annotations

import ast
import importlib.util
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_generator_encoding",
    Path(__file__).resolve().parent / "check_generator_encoding.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)


def flagged(source: str) -> list[str]:
    """Run the checker's rule over a source string, returning the call names."""
    tree = ast.parse(source)
    names = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func = node.func
        name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", None)
        if name not in chk.TEXT_IO:
            continue
        if chk.qualifier(node) in chk.NOT_TEXT_IO:
            continue
        mode = chk.mode_of(node)
        if mode is not None and "b" in mode:
            continue
        if any(keyword.arg == "encoding" for keyword in node.keywords):
            continue
        names.append(name)
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call) or not chk.asks_for_text(node):
            continue
        if any(keyword.arg == "encoding" for keyword in node.keywords):
            continue
        names.append("subprocess(text=True)")
    return names


class Catches(unittest.TestCase):
    def test_write_text_without_encoding(self):
        self.assertEqual(flagged('p.write_text("x")'), ["write_text"])

    def test_read_text_without_encoding(self):
        self.assertEqual(flagged("p.read_text()"), ["read_text"])

    def test_text_mode_open(self):
        self.assertEqual(flagged('open("f")'), ["open"])
        self.assertEqual(flagged('open("f", "r")'), ["open"])
        self.assertEqual(flagged('open("f", mode="w")'), ["open"])

    def test_reconfigure_without_encoding(self):
        # The fix for a stream you did not open is to name its encoding; a bare
        # `reconfigure()` is not that fix.
        self.assertEqual(flagged("sys.stdout.reconfigure(line_buffering=True)"), ["reconfigure"])


class CatchesSubprocess(unittest.TestCase):
    """`text=True` decodes the child's output with the locale encoding too.

    The generators drive `grib_get` and read parameter names straight out of
    its stdout; under an ASCII locale that is a `UnicodeDecodeError` on the
    first umlaut, and under a mismatched one it is a silent substitution. This
    half of the rule was missed on the first pass and cost seven call sites.
    """

    def test_text_true(self):
        self.assertEqual(
            flagged('subprocess.run(cmd, text=True)'), ["subprocess(text=True)"]
        )

    def test_universal_newlines(self):
        self.assertEqual(
            flagged('subprocess.run(cmd, universal_newlines=True)'),
            ["subprocess(text=True)"],
        )

    def test_check_output_and_popen(self):
        self.assertEqual(
            flagged('subprocess.check_output(cmd, text=True)'), ["subprocess(text=True)"]
        )
        self.assertEqual(
            flagged('subprocess.Popen(cmd, text=True)'), ["subprocess(text=True)"]
        )


class LetsThrough(unittest.TestCase):
    def test_explicit_encoding(self):
        self.assertEqual(flagged('p.write_text("x", encoding="utf-8")'), [])
        self.assertEqual(flagged('open("f", encoding="utf-8")'), [])
        self.assertEqual(flagged('sys.stdout.reconfigure(encoding="utf-8")'), [])

    def test_binary_mode_has_no_encoding_to_get_wrong(self):
        # `open(path, mode)` puts the mode at index 1; `Path.open(mode)` at
        # index 0. Reading only one of those shapes is how a binary write gets
        # reported as a missing encoding.
        self.assertEqual(flagged('open("f", "rb")'), [])
        self.assertEqual(flagged('open("f", "wb")'), [])
        self.assertEqual(flagged('p.open("wb")'), [])
        self.assertEqual(flagged('p.open(mode="rb")'), [])

    def test_same_named_calls_that_are_not_text_io(self):
        self.assertEqual(flagged('tarfile.open(fileobj=b, mode="r:gz")'), [])
        self.assertEqual(flagged('gzip.open("f")'), [])

    def test_subprocess_with_encoding_or_in_bytes(self):
        self.assertEqual(flagged('subprocess.run(cmd, text=True, encoding="utf-8")'), [])
        # No text flag at all: the output stays bytes, so there is nothing to
        # decode and nothing to get wrong.
        self.assertEqual(flagged("subprocess.run(cmd, capture_output=True)"), [])
        self.assertEqual(flagged("subprocess.run(cmd, text=False)"), [])

    def test_unrelated_calls(self):
        self.assertEqual(flagged('json.dumps({"a": 1})'), [])
        self.assertEqual(flagged('print("hello")'), [])


class TheRepoItselfPasses(unittest.TestCase):
    """The checker's own verdict on `tools/`, so the hook cannot rot silently."""

    def test_no_offenders(self):
        self.assertEqual(chk.main(), 0)


if __name__ == "__main__":
    unittest.main()
