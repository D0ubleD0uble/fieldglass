#!/usr/bin/env python3
"""Unit tests for tools/check_host_dependencies.py.

    python3 tools/test_check_host_dependencies.py

The checker's value is that it fails, so what is tested is *when*: an unlisted
edge, and a listed edge that has gone. A test that only asserted the repo passes
today would pass just as well against a checker that returned no problems ever.
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "check_host_dependencies",
    Path(__file__).resolve().parent / "check_host_dependencies.py",
)
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)


class Ratchet(unittest.TestCase):
    """Both directions, over a stubbed `cargo metadata`."""

    def run_with(self, found, allowed):
        real_found, real_allowed = chk.direct_format_dependencies, chk.ALLOWED_DIRECT
        real_hosts = chk.HOSTS
        try:
            chk.direct_format_dependencies = lambda: found
            chk.ALLOWED_DIRECT = allowed
            chk.HOSTS = set(found)
            return chk.check()
        finally:
            chk.direct_format_dependencies = real_found
            chk.ALLOWED_DIRECT = real_allowed
            chk.HOSTS = real_hosts

    def test_a_host_with_no_format_dependency_passes(self):
        self.assertEqual(self.run_with({"h": set()}, {}), [])

    def test_an_unlisted_edge_is_refused_and_names_the_umbrella(self):
        problems = self.run_with({"h": {"fieldglass-netcdf"}}, {})
        self.assertEqual(len(problems), 1)
        self.assertIn("fieldglass-netcdf", problems[0])
        self.assertIn("fieldglass::netcdf", problems[0])

    def test_a_listed_edge_is_allowed(self):
        self.assertEqual(
            self.run_with({"h": {"fieldglass-grib1"}}, {"h": {"fieldglass-grib1": "why"}}),
            [],
        )

    def test_a_listed_edge_that_has_gone_is_refused(self):
        # The half that keeps the list from outliving the dependency it excuses.
        problems = self.run_with({"h": set()}, {"h": {"fieldglass-grib1": "why"}})
        self.assertEqual(len(problems), 1)
        self.assertIn("stale", problems[0])

    def test_a_host_that_vanished_is_refused_rather_than_skipped(self):
        real_hosts = chk.HOSTS
        real_found = chk.direct_format_dependencies
        try:
            chk.HOSTS = {"h", "gone"}
            chk.direct_format_dependencies = lambda: {"h": set()}
            problems = chk.check()
        finally:
            chk.HOSTS = real_hosts
            chk.direct_format_dependencies = real_found
        self.assertEqual(len(problems), 1)
        self.assertIn("gone", problems[0])


class TheRepoItselfPasses(unittest.TestCase):
    def test_the_real_manifests_pass(self):
        self.assertEqual(chk.check(), [])

    def test_the_remaining_edges_are_the_ones_named(self):
        # Pinned so that each edge the transition removes — and each one it
        # somehow adds — comes past a reviewer here as well as in the manifest.
        # Empty since #726: the transition is finished, so an entry reappearing
        # here is an exception being argued for, and should read as one.
        self.assertEqual(
            {host: sorted(crates) for host, crates in chk.ALLOWED_DIRECT.items()},
            {},
        )

    def test_every_listed_host_is_a_host(self):
        for host in chk.ALLOWED_DIRECT:
            self.assertIn(host, chk.HOSTS)

    def test_the_node_host_names_no_decoder(self):
        # The one #662 and #726 moved: it named three format crates directly,
        # alongside the umbrella, and now reaches all of them through it.
        self.assertEqual(chk.direct_format_dependencies()["fieldglass-napi"], set())

    def test_the_browser_host_names_no_decoder(self):
        # The one that was already right, and the reason #662 could be stated as
        # a divergence rather than as a preference.
        self.assertEqual(chk.direct_format_dependencies()["fieldglass-wasm"], set())


if __name__ == "__main__":
    unittest.main(verbosity=2)
