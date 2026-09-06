#!/usr/bin/env python3
"""Unit tests for tools/check_nested_lockfiles.py.

    python3 tools/test_check_nested_lockfiles.py

The checker's whole job is to notice an absence, which is the shape of gate that
rots into a green no-op without anyone seeing it. So every way it could report
success while having checked nothing has a test named for it, and the last class
runs the real thing against this repository.

`cargo` is injected rather than invoked in the temporary trees: a synthetic crate
would have to resolve its dependencies from the network to say anything, and what
is under test here is the checker's own bookkeeping, not cargo's.
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_nested_lockfiles",
    Path(__file__).resolve().parent / "check_nested_lockfiles.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)


def crate(root: Path, relative: str, *, manifest: bool = True, lock: bool = True) -> Path:
    """A nested crate directory, optionally missing one of its two files."""
    directory = root / relative
    directory.mkdir(parents=True, exist_ok=True)
    if manifest:
        (directory / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    if lock:
        (directory / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    return directory


def repo(root: Path) -> Path:
    """A tree with the root workspace's own manifest and lockfile in place."""
    (root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    (root / "Cargo.lock").write_text("version = 4\n", encoding="utf-8")
    return root


class Recorder:
    """A stand-in for `cargo metadata` that records what it was asked to resolve."""

    def __init__(self, stale: frozenset[str] = frozenset()):
        self.stale = stale
        self.seen: list[str] = []

    def __call__(self, manifest: Path) -> tuple[bool, str]:
        name = manifest.parent.name
        self.seen.append(name)
        if name in self.stale:
            return False, "cannot update the lock file because --locked was passed"
        return True, ""


class Catches(unittest.TestCase):
    def test_a_stale_lockfile_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-grib1/fuzz")
            problems = chk.check(root, Recorder(frozenset({"fuzz"})))
        self.assertEqual(len(problems), 1)
        self.assertIn("disagrees with the manifest", problems[0])

    def test_no_nested_lockfiles_at_all_is_a_failure_not_a_clean_run(self):
        # The fail-open shape: check nothing, report success. A tree holding
        # only the root workspace has to say so rather than pass.
        with tempfile.TemporaryDirectory() as tmp:
            problems = chk.check(repo(Path(tmp)), Recorder())
        self.assertEqual(len(problems), 1)
        self.assertIn("no nested Cargo.lock found", problems[0])

    def test_a_missing_repository_directory_is_a_failure_not_a_clean_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = chk.check(Path(tmp) / "nowhere", Recorder())
        self.assertEqual(len(problems), 1)
        self.assertIn("no nested Cargo.lock found", problems[0])

    def test_a_lockfile_whose_manifest_is_gone_is_not_skipped(self):
        # The glob the workflow used to run — `crates/*/fuzz/Cargo.toml` —
        # matches nothing here and resolves the other crate happily.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-grib1/fuzz")
            crate(root, "crates/fieldglass-grib2/fuzz", manifest=False)
            recorder = Recorder()
            problems = chk.check(root, recorder)
        self.assertEqual(len(problems), 1)
        self.assertIn("no Cargo.toml beside it", problems[0])
        self.assertEqual(recorder.seen, ["fuzz"])

    def test_every_lockfile_is_checked_even_after_one_fails(self):
        # `set -e` in the old workflow loop stopped at the first; one manifest
        # edit stales all three fuzz locks, so naming all three is the point.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            for name in ("grib1", "grib2", "netcdf"):
                crate(root, f"crates/{name}")
            recorder = Recorder(frozenset({"grib1", "netcdf"}))
            problems = chk.check(root, recorder)
        self.assertEqual(len(problems), 2)
        self.assertEqual(recorder.seen, ["grib1", "grib2", "netcdf"])

    def test_a_cargo_that_cannot_be_run_is_a_failure_not_a_skip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-verify")
            problems = chk.check(root, lambda m: chk.run_cargo_metadata(m.parent / "not-a-manifest"))
        self.assertEqual(len(problems), 1)

    def test_run_cargo_metadata_reports_a_missing_binary_rather_than_raising(self):
        original = chk.subprocess.run

        def explode(*args, **kwargs):
            raise OSError("No such file or directory: 'cargo'")

        chk.subprocess.run = explode
        try:
            ok, detail = chk.run_cargo_metadata(Path("Cargo.toml"))
        finally:
            chk.subprocess.run = original
        self.assertFalse(ok)
        self.assertIn("could not run cargo", detail)


class LetsThrough(unittest.TestCase):
    def test_a_tree_whose_locks_all_resolve(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            for name in ("grib1", "grib2", "netcdf"):
                crate(root, f"crates/{name}/fuzz")
            self.assertEqual(chk.check(root, Recorder()), [])

    def test_the_root_workspace_lockfile_is_not_one_of_them(self):
        # Every `--workspace` command already resolves it, and `cargo deny
        # --locked check` gates it; re-resolving the whole graph here would be
        # the expensive half of the check for none of the coverage.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-verify")
            recorder = Recorder()
            self.assertEqual(chk.check(root, recorder), [])
            self.assertEqual(recorder.seen, ["fieldglass-verify"])

    def test_build_and_dependency_directories_are_not_walked(self):
        # Vendored sources under `target/` and `node_modules/` carry lockfiles
        # of their own; resolving those is neither possible nor wanted.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-verify")
            crate(root, "target/package/somedep-1.0.0")
            crate(root, "crates/fieldglass-verify/target/package/otherdep-2.0.0")
            crate(root, "node_modules/some-package")
            found = [p.parent.name for p in chk.nested_lockfiles(root)]
            self.assertEqual(found, ["fieldglass-verify"])


class TheRepoItselfPasses(unittest.TestCase):
    """The real check, against the real tree, with the real cargo."""

    def test_the_four_nested_lockfiles_are_the_ones_found(self):
        # Pinned by path so that a fifth nested workspace — or a lost one — has
        # to come past a reviewer here rather than quietly changing what is
        # gated. These are exactly the crates declaring a bare `[workspace]`.
        found = [str(p.parent.relative_to(chk.REPO)) for p in chk.nested_lockfiles(chk.REPO)]
        self.assertEqual(
            found,
            [
                "crates/fieldglass-grib1/fuzz",
                "crates/fieldglass-grib2/fuzz",
                "crates/fieldglass-netcdf/fuzz",
                "crates/fieldglass-verify",
            ],
        )

    def test_no_lockfile_is_stale(self):
        self.assertEqual(chk.check(), [])


if __name__ == "__main__":
    unittest.main()
