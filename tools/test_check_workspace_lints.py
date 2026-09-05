#!/usr/bin/env python3
"""Unit tests for tools/check_workspace_lints.py.

The checker exists because cargo is silent about a member that forgets
`lints.workspace = true`. A checker that is silent about the same thing would be
worse than none, so each failure it is supposed to raise is pinned here against
a synthetic workspace built in a temp directory. Run:

    python3 tools/test_check_workspace_lints.py
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_workspace_lints",
    Path(__file__).resolve().parent / "check_workspace_lints.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)

ROOT_MANIFEST = """\
[workspace]
members = ["crates/a"]

[workspace.lints.rust]
missing_docs = "warn"

[workspace.lints.clippy]
all = "deny"

[workspace.lints.rustdoc]
all = "deny"
"""

MEMBER_INHERITING = """\
[package]
name = "a"

[lints]
workspace = true
"""


class Fixture:
    """A synthetic workspace on disk, mounted under the checker's ROOT."""

    def __init__(self, root_manifest=ROOT_MANIFEST, member=MEMBER_INHERITING, lib=""):
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        (self.root / "Cargo.toml").write_text(root_manifest, encoding="utf-8")
        crate = self.root / "crates" / "a"
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(member, encoding="utf-8")
        (crate / "src" / "lib.rs").write_text(lib, encoding="utf-8")

    def run(self, debt=None) -> int:
        saved_root, saved_debt = chk.ROOT, chk.DEBT
        chk.ROOT, chk.DEBT = self.root, {} if debt is None else debt
        try:
            return chk.main()
        finally:
            chk.ROOT, chk.DEBT = saved_root, saved_debt

    def close(self) -> None:
        self._tmp.cleanup()


class Inheritance(unittest.TestCase):
    def test_member_inheriting_passes(self):
        fx = Fixture()
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_member_with_no_lints_table_fails(self):
        fx = Fixture(member="[package]\nname = 'a'\n")
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_member_declaring_its_own_lints_fails(self):
        # Cargo rejects mixing `workspace = true` with lint entries, so a crate
        # spelling out its own table is opting out of the shared standard
        # entirely — the case this checker is here for.
        fx = Fixture(member="[package]\nname = 'a'\n\n[lints.rust]\nmissing_docs = 'allow'\n")
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_workspace_false_is_not_inheriting(self):
        fx = Fixture(member="[package]\nname = 'a'\n\n[lints]\nworkspace = false\n")
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_missing_member_manifest_fails(self):
        fx = Fixture()
        self.addCleanup(fx.close)
        (fx.root / "crates" / "a" / "Cargo.toml").unlink()
        self.assertEqual(fx.run(), 1)


class RootTable(unittest.TestCase):
    def test_missing_clippy_group_fails(self):
        fx = Fixture(
            root_manifest='[workspace]\nmembers = ["crates/a"]\n\n'
            '[workspace.lints.rust]\nmissing_docs = "warn"\n\n'
            '[workspace.lints.rustdoc]\nall = "deny"\n'
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_missing_rustdoc_group_fails(self):
        # The table #582 added. Rustdoc's lints are warn-by-default, so dropping
        # it is silent everywhere else: `cargo doc` keeps exiting 0 and prints
        # the broken links it used to print.
        fx = Fixture(
            root_manifest='[workspace]\nmembers = ["crates/a"]\n\n'
            '[workspace.lints.rust]\nmissing_docs = "warn"\n\n'
            '[workspace.lints.clippy]\nall = "deny"\n'
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_no_lints_table_at_all_fails(self):
        fx = Fixture(root_manifest='[workspace]\nmembers = ["crates/a"]\n')
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)


class DebtRatchet(unittest.TestCase):
    ALLOW = "#![allow(missing_docs)]\n"

    def test_recorded_opt_out_passes(self):
        fx = Fixture(lib=self.ALLOW)
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(debt={"crates/a": 1}), 0)

    def test_unrecorded_opt_out_fails(self):
        fx = Fixture(lib=self.ALLOW)
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(debt={}), 1)

    def test_stale_debt_entry_fails(self):
        # The burn-down finished and the attribute went, but the entry stayed.
        fx = Fixture(lib="//! Documented.\n")
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(debt={"crates/a": 1}), 1)

    def test_debt_naming_a_non_member_fails(self):
        fx = Fixture()
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(debt={"crates/gone": 1}), 1)

    def test_allow_is_recognised_however_it_is_spelled(self):
        for spelling in (
            "#![allow(missing_docs)]\n",
            "#![allow(missing_docs, unused)]\n",
            "#![allow(unused, missing_docs)]\n",
            "#! [ allow ( missing_docs ) ]\n",
        ):
            with self.subTest(spelling=spelling):
                fx = Fixture(lib=spelling)
                self.addCleanup(fx.close)
                self.assertEqual(fx.run(debt={}), 1)

    def test_a_similar_name_is_not_the_lint(self):
        fx = Fixture(lib="#![allow(missing_docs_in_private_items)]\n")
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(debt={}), 0)


class TheRepoItselfPasses(unittest.TestCase):
    """The checker's verdict on the real workspace, so the hook cannot rot."""

    def test_no_offenders(self):
        self.assertEqual(chk.main(), 0)


if __name__ == "__main__":
    unittest.main()
