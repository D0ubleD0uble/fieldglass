#!/usr/bin/env python3
"""Unit tests for tools/check_changelog_required.py.

    python3 tools/test_check_changelog_required.py

What is tested is *when it fails*. Each shipped pattern is matched by a real
path and each exclusion refuses one, the `[Unreleased]` comparison is shown to
ignore whitespace and released sections, and the label and the Dependabot
exemption are each shown to be the only thing that lets a shipped change
through. The patterns are checked against this repository's tracked files, so
a renamed directory that stops matching anything fails here. The last class
runs the script end to end over a scratch git repository.
"""

from __future__ import annotations

import importlib.util
import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location(
    "check_changelog_required", TOOLS / "check_changelog_required.py"
)
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)

REPO = TOOLS.parent

BASE = """\
# Changelog

## [Unreleased]

### Fixed

- **A fix.** Something was wrong and is now right.
  It wraps onto a second line. Closes #10.

## [0.6.1] - 2026-11-06

### Fixed

- **An old fix.** Closes #9.

[Unreleased]: https://example.invalid/compare/v0.6.1...HEAD
"""


class Shipped(unittest.TestCase):
    """Every pattern, and every exclusion, by a path it must decide."""

    def test_each_shipped_pattern_matches(self):
        for path in (
            "Cargo.toml",
            "crates/fieldglass-core/Cargo.toml",
            "crates/fieldglass-napi/build.rs",
            "crates/fieldglass-core/src/lib.rs",
            "crates/fieldglass-grib2/src/templates/deep/nested.rs",
            "crates/fieldglass/schema/api.schema.json",
            "crates/fieldglass/conformance/suite.json",
            "crates/fieldglass-napi/package.json",
            "crates/fieldglass-wasm/npm/package.json",
            "extension/package.json",
            "extension/src/extension.ts",
            "extension/src/render/panel.ts",
            "extension/media/coastline-110m.json",
            "extension/icon.png",
        ):
            with self.subTest(path=path):
                self.assertTrue(chk.shipped(path), path)

    def test_each_exclusion_and_unlisted_path_is_not_shipped(self):
        for path in (
            # Extension tests, by the owner's rule.
            "extension/src/test/suite/render.test.ts",
            "extension/src/test/runTest.ts",
            # Crates that never ship: publish = false and not in the .vsix.
            "crates/fieldglass-perf/src/corpus.rs",
            "crates/fieldglass-perf/Cargo.toml",
            "crates/fieldglass-verify/Cargo.toml",
            # Parts of a shipped crate outside its package's code.
            "crates/fieldglass-core/tests/grid.rs",
            "crates/fieldglass-core/README.md",
            "crates/fieldglass-grib2/examples/dump.rs",
            "crates/fieldglass-grib2/fuzz/Cargo.toml",
            "crates/fieldglass-grib2/fuzz/fuzz_targets/decode.rs",
            "crates/fieldglass-wasm/npm/README.md",
            # A file named like a manifest but not one of a shipped package.
            "package.json",
            "tests/crate-independence/Cargo.toml",
            "tools/node/package.json",
            # Lockfiles, tooling, CI and docs.
            "Cargo.lock",
            "extension/package-lock.json",
            "extension/README.md",
            "extension/scripts/copy-package-files.cjs",
            ".github/workflows/ci.yml",
            "tools/check_changelog_required.py",
            "CHANGELOG.md",
            "docs/architecture/01-crates.md",
            # A near miss on each side of a `*`: no crate directory at all.
            "crates/src/lib.rs",
            "crates/Cargo.toml",
            "extension/srcx/a.ts",
        ):
            with self.subTest(path=path):
                self.assertFalse(chk.shipped(path), path)

    def test_every_pattern_matches_a_tracked_file(self):
        # A directory renamed out from under a pattern would quietly stop the
        # check; each pattern must still name something in this repository.
        tracked = subprocess.run(
            ["git", "-C", str(REPO), "ls-files"], check=True, capture_output=True, text=True, encoding="utf-8"
        ).stdout.splitlines()
        for pattern in chk.SHIPPED:
            with self.subTest(pattern=pattern):
                regex = chk.glob_regex(pattern)
                self.assertTrue(any(regex.fullmatch(p) for p in tracked), pattern)

    def test_excluded_crates_are_unpublished(self):
        # The crates left out are the ones that never ship. If one starts to
        # publish, it must come off the list.
        for crate in chk.UNSHIPPED_CRATES:
            with self.subTest(crate=crate):
                manifest = (REPO / "crates" / crate / "Cargo.toml").read_text(encoding="utf-8")
                self.assertRegex(manifest, r"(?m)^publish\s*=\s*false\s*$")


class Unreleased(unittest.TestCase):
    """Only a change to the words of `[Unreleased]` counts."""

    def test_an_added_entry_counts(self):
        head = BASE.replace("Closes #10.\n", "Closes #10.\n- **Another.** Closes #11.\n", 1)
        self.assertTrue(chk.unreleased_changed(BASE, head))

    def test_a_reworded_entry_counts(self):
        head = BASE.replace("is now right", "is right now", 1)
        self.assertTrue(chk.unreleased_changed(BASE, head))

    def test_whitespace_alone_does_not_count(self):
        # Re-wrapping, trailing spaces, blank lines and indentation.
        head = BASE.replace(
            "Something was wrong and is now right.\n  It wraps",
            "Something was wrong and is now right. It   wraps",
            1,
        ).replace("### Fixed\n\n- **A fix.**", "### Fixed\n\n\n-   **A fix.**  ", 1)
        self.assertNotEqual(BASE, head)
        self.assertFalse(chk.unreleased_changed(BASE, head))

    def test_a_released_section_does_not_count(self):
        head = BASE.replace("**An old fix.**", "**An old fix, reworded.**", 1)
        self.assertNotEqual(BASE, head)
        self.assertFalse(chk.unreleased_changed(BASE, head))

    def test_no_section_on_either_side_is_unchanged(self):
        old = BASE.replace("## [Unreleased]", "## [0.6.2] - 2026-11-20", 1)
        self.assertFalse(chk.unreleased_changed(old, old))

    def test_a_release_prep_emptying_the_section_counts(self):
        head = BASE.replace(
            "## [Unreleased]\n", "## [Unreleased]\n\n## [0.6.2] - 2026-11-20\n", 1
        )
        self.assertTrue(chk.unreleased_changed(BASE, head))

    def test_a_missing_changelog_reads_as_empty(self):
        self.assertTrue(chk.unreleased_changed("", BASE))
        self.assertFalse(chk.unreleased_changed("", ""))


class Verdict(unittest.TestCase):
    """The rule: shipped files need an entry, the label, or Dependabot."""

    ENTRY = BASE.replace("Closes #10.\n", "Closes #10.\n- **New.** Closes #12.\n", 1)
    CODE = ["crates/fieldglass-core/src/lib.rs", "tools/x.py"]

    def test_shipped_change_without_entry_fails_and_names_the_files(self):
        problems = chk.verdict(self.CODE, BASE, BASE, labels=[], author="someone")
        self.assertEqual(len(problems), 1)
        self.assertIn("crates/fieldglass-core/src/lib.rs", problems[0])
        self.assertNotIn("tools/x.py", problems[0])
        self.assertIn(chk.LABEL, problems[0])

    def test_shipped_change_with_entry_passes(self):
        self.assertEqual(chk.verdict(self.CODE, BASE, self.ENTRY, labels=[], author="someone"), [])

    def test_whitespace_only_entry_still_fails(self):
        head = BASE.replace("right.\n  It", "right. It", 1)
        self.assertNotEqual(chk.verdict(self.CODE, BASE, head, labels=[], author="someone"), [])

    def test_the_label_passes(self):
        self.assertEqual(chk.verdict(self.CODE, BASE, BASE, labels=["no-changelog"], author="someone"), [])

    def test_only_the_exact_label_passes(self):
        for label in ("no changelog", "no-changelog-needed", "skip-changelog", "semver-change"):
            with self.subTest(label=label):
                self.assertNotEqual(chk.verdict(self.CODE, BASE, BASE, labels=[label], author="someone"), [])

    def test_dependabot_is_exempt(self):
        self.assertEqual(
            chk.verdict(["Cargo.toml", "extension/package.json"], BASE, BASE, labels=[], author="dependabot[bot]"),
            [],
        )

    def test_only_the_dependabot_author_is_exempt(self):
        # A person's PR is not exempt for looking like Dependabot's.
        for author in ("dependabot", "dependabot-preview[bot]", "github-actions[bot]", "Dependabot[bot]x"):
            with self.subTest(author=author):
                self.assertNotEqual(chk.verdict(["Cargo.toml"], BASE, BASE, labels=[], author=author), [])

    def test_no_shipped_file_passes_without_entry(self):
        files = [".github/workflows/ci.yml", "tools/x.py", "extension/src/test/a.test.ts", "CHANGELOG.md"]
        self.assertEqual(chk.verdict(files, BASE, BASE, labels=[], author="someone"), [])

    def test_no_files_passes(self):
        self.assertEqual(chk.verdict([], BASE, BASE, labels=[], author="someone"), [])


def git(root: Path, *args: str) -> str:
    env = {
        **os.environ,
        "GIT_AUTHOR_NAME": "test",
        "GIT_AUTHOR_EMAIL": "test@example.invalid",
        "GIT_COMMITTER_NAME": "test",
        "GIT_COMMITTER_EMAIL": "test@example.invalid",
        "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_CONFIG_NOSYSTEM": "1",
    }
    # A hook runs with GIT_DIR and GIT_INDEX_FILE set to the outer repository;
    # left in place they would point these commands at it.
    for key in ("GIT_DIR", "GIT_INDEX_FILE", "GIT_WORK_TREE"):
        env.pop(key, None)
    return subprocess.run(
        ["git", "-C", str(root), *args], check=True, capture_output=True, env=env, text=True, encoding="utf-8"
    ).stdout.strip()


class EndToEnd(unittest.TestCase):
    """`main` over a scratch repository, reading files and CHANGELOG from git."""

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        git(self.root, "init", "-q", "-b", "master")
        self.write("CHANGELOG.md", BASE)
        self.write("crates/fieldglass-core/src/lib.rs", "pub fn a() {}\n")
        self.write("tools/x.py", "x = 1\n")
        self.commit("base")
        self.base = git(self.root, "rev-parse", "HEAD")
        git(self.root, "switch", "-q", "-c", "pr")

    def tearDown(self):
        self.tmp.cleanup()

    def write(self, rel: str, text: str) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")

    def commit(self, message: str) -> str:
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "-m", message)
        return git(self.root, "rev-parse", "HEAD")

    def main(self, head: str, *extra: str) -> int:
        return chk.main(["--root", str(self.root), "--base", self.base, "--head", head, *extra])

    def test_code_without_entry_fails_then_label_passes(self):
        head = self.commit_code()
        self.assertEqual(self.main(head, "--author", "someone"), 1)
        self.assertEqual(self.main(head, "--author", "someone", "--labels", json.dumps(["no-changelog"])), 0)
        self.assertEqual(self.main(head, "--author", "dependabot[bot]"), 0)

    def test_code_with_entry_passes(self):
        self.write("crates/fieldglass-core/src/lib.rs", "pub fn b() {}\n")
        self.write("CHANGELOG.md", BASE.replace("Closes #10.\n", "Closes #10.\n- **B.** Closes #13.\n", 1))
        self.assertEqual(self.main(self.commit("b"), "--author", "someone"), 0)

    def test_tooling_only_passes(self):
        self.write("tools/x.py", "x = 2\n")
        self.assertEqual(self.main(self.commit("tooling"), "--author", "someone"), 0)

    def test_a_shipped_file_moved_out_counts(self):
        # A rename reads as a deletion of the shipped path, which still ships
        # a change, not as an addition outside it.
        git(self.root, "mv", "crates/fieldglass-core/src/lib.rs", "tools/lib.rs")
        self.assertEqual(self.main(self.commit("move"), "--author", "someone"), 1)

    def test_master_moving_on_is_not_the_pr_changing_the_changelog(self):
        # The base branch gains a CHANGELOG entry after the PR branched. The
        # PR's head lacks it, so comparing the base tip with the head would
        # see a change; the merge base sees none.
        head = self.commit_code()
        git(self.root, "switch", "-q", "master")
        self.write("CHANGELOG.md", BASE.replace("Closes #10.\n", "Closes #10.\n- **Master.** Closes #14.\n", 1))
        self.base = self.commit("master moves on")
        self.assertEqual(self.main(head, "--author", "someone"), 1)

    def test_a_bad_labels_argument_is_an_error(self):
        head = self.commit_code()
        self.assertEqual(self.main(head, "--author", "someone", "--labels", "no-changelog"), 2)

    def commit_code(self) -> str:
        self.write("crates/fieldglass-core/src/lib.rs", "pub fn c() {}\n")
        return self.commit("code")


if __name__ == "__main__":
    unittest.main()
