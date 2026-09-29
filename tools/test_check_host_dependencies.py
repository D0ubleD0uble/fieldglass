#!/usr/bin/env python3
"""Unit tests for tools/check_host_dependencies.py.

    python3 tools/test_check_host_dependencies.py

The checker's value is that it fails, so what is tested is *when*: an unlisted
edge to any workspace crate, and a listed edge that has gone. A test that only
asserted the repo passes today would pass just as well against a checker that
returned no problems ever.
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


def package(name, *deps, kind=None):
    """A `cargo metadata --no-deps` package entry naming `deps`."""
    return {
        "name": name,
        "dependencies": [{"name": d, "kind": kind} for d in deps],
    }


# A small workspace: the umbrella, a core, a decoder, and one host.
def workspace(*host_deps, kind=None):
    return [
        package("fieldglass", "fieldglass-core", "fieldglass-grib2", "serde"),
        package("fieldglass-core", "serde"),
        package("fieldglass-grib2", "fieldglass-core"),
        package("h", *host_deps, kind=kind),
    ]


class Ratchet(unittest.TestCase):
    """Both directions, over a stubbed `cargo metadata`."""

    def run_with(self, packages, allowed):
        real_allowed, real_hosts = chk.ALLOWED_DIRECT, chk.HOSTS
        try:
            chk.ALLOWED_DIRECT = allowed
            chk.HOSTS = {"h"}
            return chk.check(packages)
        finally:
            chk.ALLOWED_DIRECT = real_allowed
            chk.HOSTS = real_hosts

    def test_a_host_on_the_umbrella_alone_passes(self):
        # Third-party crates are not the rule's business.
        self.assertEqual(self.run_with(workspace("fieldglass", "napi", "serde"), {}), [])

    def test_a_host_naming_core_is_refused_and_names_the_umbrella(self):
        # The edge #574 removed: not a decoder, and still a second route past
        # the umbrella. The pinned violation this checker exists to catch.
        problems = self.run_with(workspace("fieldglass", "fieldglass-core"), {})
        self.assertEqual(len(problems), 1)
        self.assertIn("fieldglass-core", problems[0])
        self.assertIn("re-export", problems[0])

    def test_a_host_naming_a_decoder_is_refused(self):
        problems = self.run_with(workspace("fieldglass", "fieldglass-grib2"), {})
        self.assertEqual(len(problems), 1)
        self.assertIn("fieldglass-grib2", problems[0])

    def test_a_dev_dependency_counts_alike(self):
        problems = self.run_with(workspace("fieldglass-core", kind="dev"), {})
        self.assertEqual(len(problems), 1)
        self.assertIn("fieldglass-core", problems[0])

    def test_a_crate_new_to_the_workspace_is_covered_without_a_list(self):
        # The set is the workspace's, not a table here, so a crate added later
        # is refused the day it lands.
        packages = workspace("fieldglass", "fieldglass-brand-new") + [
            package("fieldglass-brand-new")
        ]
        problems = self.run_with(packages, {})
        self.assertEqual(len(problems), 1)
        self.assertIn("fieldglass-brand-new", problems[0])

    def test_a_listed_edge_is_allowed(self):
        self.assertEqual(
            self.run_with(
                workspace("fieldglass", "fieldglass-core"),
                {"h": {"fieldglass-core": "why"}},
            ),
            [],
        )

    def test_a_listed_edge_that_has_gone_is_refused(self):
        # The half that keeps the list from outliving the dependency it excuses.
        problems = self.run_with(workspace("fieldglass"), {"h": {"fieldglass-core": "why"}})
        self.assertEqual(len(problems), 1)
        self.assertIn("stale", problems[0])

    def test_a_host_that_vanished_is_refused_rather_than_skipped(self):
        real_hosts = chk.HOSTS
        try:
            chk.HOSTS = {"h", "gone"}
            problems = chk.check(workspace("fieldglass"))
        finally:
            chk.HOSTS = real_hosts
        self.assertEqual(len(problems), 1)
        self.assertIn("gone", problems[0])

    def test_an_umbrella_that_vanished_is_refused(self):
        packages = [p for p in workspace("fieldglass") if p["name"] != "fieldglass"]
        problems = self.run_with(packages, {})
        self.assertEqual(len(problems), 1)
        self.assertIn("umbrella", problems[0])


class TheRepoItselfPasses(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.packages = chk.cargo_metadata_packages()
        cls.found = chk.direct_workspace_dependencies(cls.packages)

    def test_the_real_manifests_pass(self):
        self.assertEqual(chk.check(self.packages), [])

    def test_the_remaining_edges_are_the_ones_named(self):
        # Pinned so that each edge a transition removes — and each one it
        # somehow adds — comes past a reviewer here as well as in the manifest.
        # Empty since #574: the transition is finished, so an entry reappearing
        # here is an exception being argued for, and should read as one.
        self.assertEqual(
            {host: sorted(crates) for host, crates in chk.ALLOWED_DIRECT.items()},
            {},
        )

    def test_every_listed_host_is_a_host(self):
        for host in chk.ALLOWED_DIRECT:
            self.assertIn(host, chk.HOSTS)

    def test_the_node_host_names_the_umbrella_alone(self):
        # The one #662, #726 and #574 moved: it named three format crates and
        # `fieldglass-core` directly, alongside the umbrella, and now reaches
        # all of them through it.
        self.assertEqual(self.found["fieldglass-napi"], set())

    def test_the_browser_host_names_the_umbrella_alone(self):
        # The one that was already right, and the reason #662 could be stated as
        # a divergence rather than as a preference.
        self.assertEqual(self.found["fieldglass-wasm"], set())

    def test_the_real_workspace_holds_the_crates_the_rule_is_about(self):
        # A fail-open guard: if `cargo metadata` stopped reporting the workspace,
        # every host would name "no workspace crate" and pass.
        names = {p["name"] for p in self.packages}
        for crate in ("fieldglass", "fieldglass-core", "fieldglass-grib2"):
            self.assertIn(crate, names)


if __name__ == "__main__":
    unittest.main(verbosity=2)
