#!/usr/bin/env python3
"""Unit tests for tools/check_verified_kernels.py.

    python3 tools/test_check_verified_kernels.py

The checker notices an absence — an include that went away, a path filter that
never covered a file — which is the kind of gate that rots into a green no-op
without anyone seeing it. So each way it could pass while having checked nothing
has a test named for it, and the last class runs it against this repository.
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_verified_kernels",
    Path(__file__).resolve().parent / "check_verified_kernels.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)

KERNEL = "crates/fieldglass-core/src/scaling.rs"
PROOF = "#[cfg_attr(verus_keep_ghost, verus_spec(r => ensures true))]\nfn f() {}\n"
INCLUDE = '#[path = "../../fieldglass-core/src/scaling.rs"]\npub mod scaling;\n'


def workflow(push: list[str], pull_request: list[str]) -> str:
    def block(globs: list[str]) -> str:
        return "".join(f"      - '{g}'\n" for g in globs)

    return (
        "name: Verify\n"
        "# a comment at the top level\n"
        "on:\n"
        "  push:\n"
        "    branches: [master]\n"
        "    paths:\n"
        f"{block(push)}"
        "  pull_request:\n"
        "    paths:\n"
        f"{block(pull_request)}"
        "\n"
        "jobs:\n"
        "  verus:\n"
        "    paths:\n"
        "      - 'not/a/trigger/**'\n"
    )


def tree(
    root: Path,
    *,
    kernel: str | None = PROOF,
    include: str | None = INCLUDE,
    push: list[str] | None = None,
    pull_request: list[str] | None = None,
) -> Path:
    """A repository with one kernel file, the verification crate, and verify.yml."""
    covered = ["crates/fieldglass-verify/**", KERNEL]
    (root / "crates/fieldglass-core/src").mkdir(parents=True)
    (root / "crates/fieldglass-core/src/lib.rs").write_text("pub mod scaling;\n", encoding="utf-8")
    if kernel is not None:
        (root / KERNEL).write_text(kernel, encoding="utf-8")
    (root / "crates/fieldglass-verify/src").mkdir(parents=True)
    # The verification crate's own sources are Verus throughout; they are not
    # kernels, whatever they contain.
    lib = "verus! { fn g() {} }\n// verus_spec\n" + (include or "")
    (root / "crates/fieldglass-verify/src/lib.rs").write_text(lib, encoding="utf-8")
    (root / ".github/workflows").mkdir(parents=True)
    (root / ".github/workflows/verify.yml").write_text(
        workflow(covered if push is None else push, covered if pull_request is None else pull_request),
        encoding="utf-8",
    )
    return root


class Catches(unittest.TestCase):
    def test_a_kernel_the_verification_crate_does_not_include(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = chk.check(tree(Path(tmp), include=None))
            self.assertEqual(len(problems), 1)
            self.assertIn(KERNEL, problems[0])
            self.assertIn("never checked", problems[0])

    def test_an_include_that_names_no_file(self):
        # A kernel renamed without updating the include: the old path dangles
        # and the new file is uncovered. Both are reported.
        with tempfile.TemporaryDirectory() as tmp:
            root = tree(Path(tmp))
            (root / KERNEL).rename(root / "crates/fieldglass-core/src/scale.rs")
            problems = chk.check(
                root_with_globs(root, ["crates/fieldglass-verify/**", "crates/**"])
            )
            self.assertTrue(any("names no file" in p for p in problems), problems)
            self.assertTrue(any("scale.rs" in p and "never checked" in p for p in problems))

    def test_a_kernel_missing_from_the_push_trigger(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = chk.check(tree(Path(tmp), push=["crates/fieldglass-verify/**"]))
            self.assertEqual(len(problems), 1)
            self.assertIn("`push`", problems[0])

    def test_a_kernel_missing_from_the_pull_request_trigger(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = chk.check(tree(Path(tmp), pull_request=["crates/fieldglass-verify/**"]))
            self.assertEqual(len(problems), 1)
            self.assertIn("`pull_request`", problems[0])

    def test_a_trigger_with_no_paths_at_all(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = chk.check(tree(Path(tmp), push=[]))
            self.assertEqual(problems, [f"{chk.WORKFLOW}: no `paths:` list under `push:`"])

    def test_no_kernel_anywhere_is_a_failure_not_a_clean_run(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = chk.check(tree(Path(tmp), kernel="fn f() {}\n"))
            self.assertEqual(len(problems), 1)
            self.assertIn("has the layout moved", problems[0])

    def test_a_missing_workflow_is_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = tree(Path(tmp))
            (root / ".github/workflows/verify.yml").unlink()
            problems = chk.check(root)
            self.assertTrue(any("cannot read it" in p for p in problems), problems)

    def test_a_single_star_does_not_cross_directories(self):
        self.assertFalse(chk.glob_matches("crates/*.rs", KERNEL))
        self.assertTrue(chk.glob_matches("crates/**", KERNEL))
        self.assertTrue(chk.glob_matches("crates/fieldglass-core/src/*.rs", KERNEL))


def root_with_globs(root: Path, globs: list[str]) -> Path:
    (root / ".github/workflows/verify.yml").write_text(workflow(globs, globs), encoding="utf-8")
    return root


class LetsThrough(unittest.TestCase):
    def test_an_included_and_covered_kernel(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(chk.check(tree(Path(tmp))), [])

    def test_a_directory_glob_covers_the_kernel(self):
        with tempfile.TemporaryDirectory() as tmp:
            globs = ["crates/fieldglass-verify/**", "crates/fieldglass-core/**"]
            self.assertEqual(chk.check(tree(Path(tmp), push=globs, pull_request=globs)), [])

    def test_build_directories_are_not_walked(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = tree(Path(tmp))
            vendored = root / "crates/fieldglass-core/target/package/vstd/src"
            vendored.mkdir(parents=True)
            (vendored / "lib.rs").write_text(PROOF, encoding="utf-8")
            self.assertEqual(chk.check(root), [])


class TheRepoItselfPasses(unittest.TestCase):
    """The real check, against the real tree."""

    def test_the_kernel_files_are_the_ones_found(self):
        # Pinned by path, so a new kernel file — or a lost one — comes past a
        # reviewer here rather than quietly changing what is verified.
        found = [p.as_posix() for p in chk.kernel_files(chk.REPO)]
        self.assertEqual(found, [KERNEL])

    def test_every_kernel_is_included_and_covered(self):
        self.assertEqual(chk.check(), [])


if __name__ == "__main__":
    unittest.main()
