#!/usr/bin/env python3
"""Unit tests for tools/check_architecture_diagrams.py.

The checker gates commits, so its regex / brace-counting helpers are worth
pinning down. Pure functions only — no filesystem. Run:

    python3 tools/test_check_architecture_diagrams.py
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_architecture_diagrams",
    Path(__file__).resolve().parent / "check_architecture_diagrams.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)


class StripNoise(unittest.TestCase):
    def test_blanks_line_comment_and_literals(self):
        # Braces hiding in a comment and a string must not be counted.
        line = 'let s = "a{b}c"; // closes mod here }'
        cleaned = chk._strip_noise(line)
        self.assertNotIn("{", cleaned)
        self.assertNotIn("}", cleaned)

    def test_keeps_real_braces(self):
        self.assertIn("{", chk._strip_noise("fn f() {"))


class ProductionLines(unittest.TestCase):
    def test_strips_cfg_test_mod(self):
        text = (
            "pub fn keep() {}\n"
            "#[cfg(test)]\n"
            "mod tests {\n"
            "    struct WideMock;\n"
            "    impl PlanarGridProjector for WideMock {}\n"
            "}\n"
            "pub fn also_keep() {}\n"
        )
        out = "\n".join(chk.production_lines(text))
        self.assertIn("keep", out)
        self.assertIn("also_keep", out)
        self.assertNotIn("WideMock", out)

    def test_strips_cfg_all_test_item_impl(self):
        # Item-level (no surrounding mod) and cfg(all(test, …)) form.
        text = (
            '#[cfg(all(test, feature = "x"))]\n'
            "impl Grib1Packing for Mock {}\n"
            "impl Grib1Packing for RealPacking {}\n"
        )
        out = "\n".join(chk.production_lines(text))
        self.assertNotIn("Mock", out)
        self.assertIn("RealPacking", out)

    def test_brace_in_test_string_does_not_swallow_production(self):
        text = (
            "#[cfg(test)]\n"
            "mod tests {\n"
            '    const S: &str = "}";\n'
            "}\n"
            "impl FormatReader for RealReader {}\n"
        )
        pairs = chk.impl_pairs_from_text(text, {"FormatReader"})
        self.assertEqual(pairs, {("FormatReader", "RealReader")})


class ImplMatching(unittest.TestCase):
    def test_single_line(self):
        text = "impl FormatReader for Grib2Reader {}\n"
        self.assertEqual(
            chk.impl_pairs_from_text(text, {"FormatReader"}),
            {("FormatReader", "Grib2Reader")},
        )

    def test_multiline_header_joined(self):
        text = "impl<T>\n    FormatReader\n    for Grib2Reader<T>\n{\n}\n"
        self.assertEqual(
            chk.impl_pairs_from_text(text, {"FormatReader"}),
            {("FormatReader", "Grib2Reader")},
        )

    def test_ignores_non_first_party_trait(self):
        text = "impl Debug for Foo {}\n"
        self.assertEqual(chk.impl_pairs_from_text(text, {"FormatReader"}), set())

    def test_inherent_impl_is_not_a_realization(self):
        text = "impl Grib2Reader {\n    pub fn new() {}\n}\n"
        self.assertEqual(chk.impl_pairs_from_text(text, {"FormatReader"}), set())


class MermaidExtraction(unittest.TestCase):
    def test_prose_edge_outside_fence_ignored(self):
        text = (
            "Docs mention `Foo <|.. Bar` in prose.\n\n"
            "```mermaid\nclassDiagram\n    Foo <|.. Real\n```\n"
        )
        edges = chk.edges_from_mermaid(text, chk.REALIZATION_RE)
        self.assertEqual(edges, {("Foo", "Real")})

    def test_flow_edges(self):
        text = "```mermaid\nflowchart TD\n    napi --> core\n```\n"
        self.assertEqual(
            chk.edges_from_mermaid(text, chk.FLOW_EDGE_RE), {("napi", "core")}
        )


class CompositionNodes(unittest.TestCase):
    def test_collects_endpoints_skips_cardinality(self):
        text = (
            "```mermaid\nclassDiagram\n"
            "    class Grib2Message\n"
            '    Grib2Message *-- "many" IndicatorSection\n'
            "    GridTemplate --> LatLonTemplate\n"
            "```\n"
        )
        nodes = chk.composition_type_nodes(text)
        self.assertIn("Grib2Message", nodes)
        self.assertIn("IndicatorSection", nodes)
        self.assertIn("LatLonTemplate", nodes)
        self.assertNotIn("many", nodes)
        self.assertNotIn("class", nodes)

    def test_captures_reverse_dependency_arrow(self):
        # `<..` endpoints must be collected too (F2).
        text = "```mermaid\nclassDiagram\n    GridTemplate <.. GridDefinitionSection\n```\n"
        nodes = chk.composition_type_nodes(text)
        self.assertEqual(nodes, {"GridTemplate", "GridDefinitionSection"})


class FenceParsing(unittest.TestCase):
    def test_broken_fence_yields_no_blocks(self):
        # A mis-spelled fence must not silently parse as empty-and-fine (F3).
        self.assertEqual(chk.mermaid_blocks("```mermaidx\nclassDiagram\n```\n"), [])

    def test_good_fence_yields_block(self):
        self.assertEqual(len(chk.mermaid_blocks("```mermaid\nA <|.. B\n```\n")), 1)


class CrateDeps(unittest.TestCase):
    def test_only_dependencies_section(self):
        toml = (
            "[dependencies]\n"
            'fieldglass-core = { path = "../fieldglass-core" }\n'
            "[dev-dependencies]\n"
            'fieldglass-grib1 = { path = "../fieldglass-grib1" }\n'
        )
        self.assertEqual(chk.crate_deps_from_toml(toml), {"core"})


if __name__ == "__main__":
    unittest.main(verbosity=2)
