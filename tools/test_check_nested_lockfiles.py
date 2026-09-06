#!/usr/bin/env python3
"""Unit tests for tools/check_nested_lockfiles.py.

    python3 tools/test_check_nested_lockfiles.py

The checker's whole job is to notice an absence, which is the shape of gate that
rots into a green no-op without anyone seeing it. So every way it could report
success while having checked nothing has a test named for it, and the last class
runs the real thing against this repository.

`cargo` is injected rather than invoked in the temporary trees: a synthetic crate
would have to resolve its dependencies from the network to say anything, and what
is under test here is the checker's own bookkeeping, not cargo's. The one place a
real `cargo` runs is `TheRepoItselfPasses`, against the real tree.
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
    """A nested workspace directory, optionally missing one of its two files."""
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
    """A stand-in for `cargo metadata` that records what it was asked to resolve.

    What it records is the path relative to the temporary root, not the directory
    name: two fuzz crates are both called `fuzz`, so a bare name cannot tell
    "resolved grib1" from "resolved grib2" and an assertion on it would pass
    while the checker looked at the wrong crate.
    """

    def __init__(self, root: Path, stale: frozenset[str] = frozenset(), kind: str = chk.STALE):
        self.root = root
        self.stale = stale
        self.kind = kind
        self.seen: list[str] = []

    def __call__(self, manifest: Path) -> tuple[str | None, str]:
        where = str(manifest.parent.relative_to(self.root))
        self.seen.append(where)
        if where in self.stale:
            detail = (
                "cannot update the lock file because --locked was passed"
                if self.kind == chk.STALE
                else "failed to load manifest"
            )
            return self.kind, detail
        return None, ""


class Catches(unittest.TestCase):
    def test_a_stale_lockfile_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-grib1/fuzz")
            problems = chk.check(root, Recorder(root, frozenset({"crates/fieldglass-grib1/fuzz"})))
        self.assertEqual(len(problems), 1)
        self.assertIn("lockfile disagrees with the manifest", problems[0])
        self.assertIn("cargo update -w", problems[0])

    def test_no_nested_workspace_at_all_is_a_failure_not_a_clean_run(self):
        # The fail-open shape: check nothing, report success. A tree holding
        # only the root workspace has to say so rather than pass.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            problems = chk.check(root, Recorder(root))
        self.assertEqual(len(problems), 1)
        self.assertIn("no nested workspace found", problems[0])

    def test_a_missing_repository_directory_is_a_failure_not_a_clean_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            problems = chk.check(root / "nowhere", Recorder(root))
        self.assertEqual(len(problems), 1)
        self.assertIn("no nested workspace found", problems[0])

    def test_a_lockfile_whose_manifest_is_gone_is_not_skipped(self):
        # The glob the workflow used to run — `crates/*/fuzz/Cargo.toml` —
        # matches nothing here and resolves the other crate happily.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-grib1/fuzz")
            crate(root, "crates/fieldglass-grib2/fuzz", manifest=False)
            recorder = Recorder(root)
            problems = chk.check(root, recorder)
        self.assertEqual(len(problems), 1)
        self.assertIn("Cargo.lock with no Cargo.toml beside it", problems[0])
        self.assertEqual(recorder.seen, ["crates/fieldglass-grib1/fuzz"])

    def test_a_nested_workspace_with_no_lockfile_is_not_invisible(self):
        # The mirror of the case above, and the one a lockfile-keyed walk misses
        # entirely: `cargo fuzz init` writes a crate whose lock is easy to leave
        # uncommitted, and nothing else in the repo would resolve it.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-grib1/fuzz")
            crate(root, "crates/fieldglass-netcdf/fuzz", lock=False)
            recorder = Recorder(root)
            problems = chk.check(root, recorder)
        self.assertEqual(len(problems), 1)
        self.assertIn("no Cargo.lock beside it", problems[0])
        self.assertEqual(recorder.seen, ["crates/fieldglass-grib1/fuzz"])

    def test_every_workspace_is_checked_even_after_one_fails(self):
        # `set -e` in the old workflow loop stopped at the first; one manifest
        # edit stales all three fuzz locks, so naming all three is the point.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            for name in ("grib1", "grib2", "netcdf"):
                crate(root, f"crates/{name}")
            recorder = Recorder(root, frozenset({"crates/grib1", "crates/netcdf"}))
            problems = chk.check(root, recorder)
        self.assertEqual(len(problems), 2)
        self.assertEqual(recorder.seen, ["crates/grib1", "crates/grib2", "crates/netcdf"])

    def test_a_cargo_that_cannot_be_run_is_a_failure_not_a_skip(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-verify")
            problems = chk.check(root, lambda m: (chk.NO_CARGO, "could not run cargo: nope"))
        self.assertEqual(len(problems), 1)
        self.assertIn("could not check this lockfile", problems[0])
        # Not the stale-lock advice: `cargo update` cannot help a missing cargo.
        self.assertNotIn("cargo update", problems[0])

    def test_run_cargo_metadata_reports_a_missing_binary_rather_than_raising(self):
        original = chk.subprocess.run

        def explode(*args, **kwargs):
            raise OSError("No such file or directory: 'cargo'")

        chk.subprocess.run = explode
        try:
            kind, detail = chk.run_cargo_metadata(Path("Cargo.toml"))
        finally:
            chk.subprocess.run = original
        self.assertEqual(kind, chk.NO_CARGO)
        self.assertIn("could not run cargo", detail)

    def test_a_cargo_failure_that_is_not_a_stale_lock_is_not_called_one(self):
        # An unparseable manifest, or an index cargo cannot reach offline, exits
        # non-zero too. Reporting those as drift sends someone with a correct
        # lock to `cargo update`, which fails for the same underlying reason.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-verify")
            recorder = Recorder(root, frozenset({"crates/fieldglass-verify"}), chk.UNRESOLVABLE)
            problems = chk.check(root, recorder)
        self.assertEqual(len(problems), 1)
        self.assertIn("is not a stale lockfile", problems[0])
        self.assertNotIn("disagrees with the manifest", problems[0])


class LetsThrough(unittest.TestCase):
    def test_a_tree_whose_locks_all_resolve(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            for name in ("grib1", "grib2", "netcdf"):
                crate(root, f"crates/{name}/fuzz")
            self.assertEqual(chk.check(root, Recorder(root)), [])

    def test_the_root_workspace_is_not_one_of_them(self):
        # Every `--workspace` command already resolves it, and `cargo deny
        # --locked check` gates it; re-resolving the whole graph here would be
        # the expensive half of the check for none of the coverage.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-verify")
            recorder = Recorder(root)
            self.assertEqual(chk.check(root, recorder), [])
            self.assertEqual(recorder.seen, ["crates/fieldglass-verify"])

    def test_an_ordinary_workspace_member_is_not_one_of_them(self):
        # A member crate has neither a `[workspace]` table nor a lock of its own,
        # and is resolved by every `--workspace` command there is.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            member = root / "crates" / "fieldglass-core"
            member.mkdir(parents=True)
            (member / "Cargo.toml").write_text('[package]\nname = "x"\n', encoding="utf-8")
            crate(root, "crates/fieldglass-verify")
            found = [str(d.relative_to(root)) for d in chk.nested_workspaces(root)]
            self.assertEqual(found, ["crates/fieldglass-verify"])

    def test_build_and_dependency_directories_are_not_walked(self):
        # Vendored sources under `target/` and `node_modules/` carry lockfiles
        # of their own; resolving those is neither possible nor wanted.
        with tempfile.TemporaryDirectory() as tmp:
            root = repo(Path(tmp))
            crate(root, "crates/fieldglass-verify")
            for pruned in ("target", "node_modules", ".venv", "out", "dist"):
                crate(root, f"{pruned}/package/somedep-1.0.0")
            crate(root, "crates/fieldglass-verify/target/package/otherdep-2.0.0")
            found = [str(d.relative_to(root)) for d in chk.nested_workspaces(root)]
            self.assertEqual(found, ["crates/fieldglass-verify"])


class TheRepoItselfPasses(unittest.TestCase):
    """The real check, against the real tree, with the real cargo."""

    def test_the_six_nested_workspaces_are_the_ones_found(self):
        # Pinned by path so that a seventh nested workspace — or a lost one —
        # has to come past a reviewer here rather than quietly changing what is
        # gated. These are exactly the crates declaring a bare `[workspace]`.
        found = [str(d.relative_to(chk.REPO)) for d in chk.nested_workspaces(chk.REPO)]
        self.assertEqual(
            found,
            [
                "crates/fieldglass-fetchplan/fuzz",
                "crates/fieldglass-grib1/fuzz",
                "crates/fieldglass-grib2/fuzz",
                "crates/fieldglass-netcdf/fuzz",
                "crates/fieldglass-verify",
                "crates/fieldglass-zarr/fuzz",
            ],
        )

    def test_no_lockfile_is_stale(self):
        self.assertEqual(chk.check(), [])


if __name__ == "__main__":
    unittest.main()
