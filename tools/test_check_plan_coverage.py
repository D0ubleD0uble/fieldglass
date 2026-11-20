#!/usr/bin/env python3
"""Self-test for `check_plan_coverage.py`.

    python3 tools/test_check_plan_coverage.py

Asserts on the sets the parsing functions return, not only on an exit status:
a reader that found no issue in the CHANGELOG would pass every plan, and one
that counted the plan's setup comments would pass a plan with no check.

The release-cycle cases (#1898) run the script itself, in a copy of the
repository layout with its own CHANGELOG.md, RELEASE-TEST-PLAN.md and
Cargo.toml, so they go through the same file reads and exit status the hook
does.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from check_plan_coverage import issue_refs, missing, plan_refs

failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


CHANGELOG = """\
# Changelog

## [Unreleased]

### Added
- **A feature.** It does a thing. Closes #1750.
- **Another.** Hovering is faster (#1796); see also #1801, #1802 and #1803/#1804.

### Fixed
- **Labels.** A mean read the same as its spread, as `#1 · VGRD · 2 · +0h`,
  and a field of Code Table 4.7, such as `#3 · VGRD ·
  2 · +0h`, now differs. Part of #1305. Closes #1760.

## [0.6.1] — 2026-11-05

- **Old.** Closes #1500.
"""

PLAN = """\
# Manual test plan — everything that changed since v0.6.1

**Baseline: v0.6.1 (2026-11-05).** Rewritten each cycle.

Preamble that mentions #1750 in passing.

## How to add to this plan

An example: name a section for its issue, like #1796.

## 0. Setup (once)

```sh
# Build the files #1801 needs.
python3 build.py
```

### 0.6.1 beside it

The setup again mentions #1802.

## 1. Regression floor

Open the file. What failure looks like: it does not open (#1750).

## 2. A thing (#1803, #1804)

Check one. `#1305` in a code span is not a reference.

## No manual check

- **Hover** (#1796). Covered by a test.

## Recording the outcome

Note #1760 here.
"""


TOOLS = Path(__file__).resolve().parent


def run(changelog: str, plan: str, version: str) -> tuple[int, str]:
    """`check_plan_coverage.py`'s exit status and output over these three files."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "tools").mkdir()
        for script in ("check_plan_coverage.py", "release_notes.py"):
            shutil.copy(TOOLS / script, root / "tools" / script)
        (root / "CHANGELOG.md").write_text(changelog, encoding="utf-8")
        (root / "RELEASE-TEST-PLAN.md").write_text(plan, encoding="utf-8")
        (root / "Cargo.toml").write_text(
            f'[workspace]\nmembers = []\n\n[workspace.package]\nversion    = "{version}"\n',
            encoding="utf-8",
        )
        done = subprocess.run(
            [sys.executable, str(root / "tools" / "check_plan_coverage.py")],
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=False,
        )
        return done.returncode, done.stdout + done.stderr


def promote(changelog: str, version: str) -> str:
    """`changelog` after a release prep: `[Unreleased]` renamed to `version`, a new empty one above."""
    return changelog.replace("## [Unreleased]\n", f"## [Unreleased]\n\n## [{version}] — 2026-11-21\n", 1)


def release_cycle() -> None:
    covered = PLAN.replace("## No manual check\n", "## No manual check\n\n- Rest (#1305, #1760, #1801, #1802).\n")

    # Master between releases: the version is the plan's baseline.
    status, out = run(CHANGELOG, covered, "0.6.1")
    check("between releases, a covered [Unreleased] passes", status == 0, out)
    check(
        "between releases, the output names [Unreleased] and says the version is released",
        "checking [Unreleased] of CHANGELOG.md (0.6.1 is the plan's baseline" in out,
        out,
    )
    status, out = run(CHANGELOG, PLAN, "0.6.1")
    check("between releases, an uncovered [Unreleased] still fails", status == 1 and "  #1305" in out, out)

    # The prep PR: [Unreleased] renamed to [0.6.2], the version bumped, the
    # plan still on its v0.6.1 baseline, and a reference added while
    # reviewing the promoted entries.
    prep = promote(CHANGELOG, "0.6.2").replace("now differs.", "now differs. See also #1899.", 1)
    check("the prep fixture cites #1899 under [0.6.2]", "#1899" in prep.split("## [0.6.1]")[0], prep)
    status, out = run(prep, covered, "0.6.2")
    check("a prep PR with an uncovered issue fails", status == 1, out)
    check("a prep PR's failure names the issue", "  #1899" in out, out)
    check("a prep PR's failure names only that issue", out.count("\n  #") == 1, out)
    check(
        "a prep PR's gap in [0.6.2] says the baseline moves only after the tag",
        "Only once v0.6.2 is tagged" in out and "Moving it before the tag stops [0.6.2] being checked." in out,
        out,
    )
    check(
        "a prep PR's output names [0.6.2] and why",
        "checking [Unreleased] and [0.6.2] of CHANGELOG.md (0.6.2 is newer than the plan's baseline v0.6.1"
        in out,
        out,
    )
    status, out = run(promote(CHANGELOG, "0.6.2"), covered, "0.6.2")
    check("a covered prep PR passes", status == 0, out)
    check(
        "a covered prep PR counts the promoted issues",
        "all 8 issues cited under [Unreleased] and [0.6.2]" in out,
        out,
    )

    # After the prep merges, a change can land under the new [Unreleased]
    # before the tag. Both sections are still checked.
    later = promote(CHANGELOG, "0.6.2").replace(
        "## [Unreleased]\n", "## [Unreleased]\n\n- **Next.** Closes #1950.\n", 1
    )
    status, out = run(later, covered, "0.6.2")
    check("before the tag, a new [Unreleased] issue is checked", status == 1 and "  #1950" in out, out)
    # The gap is only under [Unreleased], so no reset can explain it, and
    # moving the baseline would switch the [0.6.2] check off.
    check("a gap only under [Unreleased] does not suggest moving the baseline", "Baseline" not in out, out)
    status, out = run(later, covered.replace("- Rest (#1305,", "- Rest (#1950, #1305,"), "0.6.2")
    check("before the tag, [Unreleased] does not replace [0.6.2]", status == 0 and "[0.6.2]" in out, out)
    status, out = run(later.replace("Closes #1750.", "Closes #1751."), covered.replace("- Rest (", "- Rest (#1950, "), "0.6.2")
    check("before the tag, a [0.6.2] gap still fails", status == 1 and "  #1751" in out, out)

    # The post-release reset: the plan rewritten against v0.6.2 with none of
    # the shipped checks, [Unreleased] empty, the version still 0.6.2.
    reset = (
        "# Manual test plan — everything that changed since v0.6.2\n\n"
        "**Baseline: v0.6.2 (2026-11-22).** Rewritten each cycle.\n\n"
        "## 1. Regression floor\n\nOpen the file.\n\n## No manual check\n\n"
    )
    status, out = run(promote(CHANGELOG, "0.6.2"), reset, "0.6.2")
    check("the post-release reset passes", status == 0, out)
    check(
        "the post-release reset checks [Unreleased] alone and says 0.6.2 is released",
        "checking [Unreleased] of CHANGELOG.md (0.6.2 is the plan's baseline, so it is released)" in out
        and "no issue is cited under [Unreleased]; nothing to cover" in out,
        out,
    )
    # A reset that forgets the baseline fails closed, and says what to move.
    stale = reset.replace("v0.6.2 (2026-11-22)", "v0.6.1 (2026-11-05)")
    status, out = run(promote(CHANGELOG, "0.6.2"), stale, "0.6.2")
    check(
        "a reset that leaves the old baseline fails and says to move it",
        status == 1 and "Only once v0.6.2 is tagged: if the plan was then reset, move its '**Baseline:' line" in out,
        out,
    )

    # Fail closed where "not released yet" cannot be read.
    status, out = run(CHANGELOG, covered.replace("**Baseline: v0.6.1", "**Baseline: the last tag"), "0.6.1")
    check("a plan with no baseline line fails", status == 1 and "no '**Baseline: vX.Y.Z' line" in out, out)
    status, out = run(CHANGELOG, covered.replace("**Baseline: v0.6.1 (2026-11-05).** Rewritten each cycle.\n\n", ""), "0.6.1")
    check("a plan with the line removed fails", status == 1 and "no '**Baseline: vX.Y.Z' line" in out, out)
    moved = covered.replace("**Baseline: v0.6.1 (2026-11-05).** Rewritten each cycle.\n\n", "").replace(
        "## 1. Regression floor\n", "## 1. Regression floor\n\n**Baseline: v0.6.1 (2026-11-05).**\n"
    )
    status, out = run(CHANGELOG, moved, "0.6.1")
    check("a baseline line inside a section does not count", status == 1 and "no '**Baseline:" in out, out)
    status, out = run(CHANGELOG, covered, "0.6.0")
    check("a baseline newer than the version fails", status == 1 and "is newer than the workspace version" in out, out)
    status, out = run(CHANGELOG, covered, "0.6.2")
    check(
        "an unreleased version with no CHANGELOG section fails",
        status == 1 and "no '## [0.6.2]' section" in out,
        out,
    )
    status, out = run(CHANGELOG, covered, "0.6.1-rc.1")
    check("a version not of the form X.Y.Z fails", status == 1 and "not of the form X.Y.Z" in out, out)
    # Versions compare as numbers, not text: 0.6.10 is newer than 0.6.9.
    status, out = run(promote(CHANGELOG, "0.6.10"), covered.replace("v0.6.1 (", "v0.6.9 ("), "0.6.10")
    check("0.6.10 is newer than a v0.6.9 baseline", status == 0 and "[0.6.10]" in out, out)


def main() -> int:
    # The CHANGELOG side: every form of reference the real file uses.
    refs = issue_refs(CHANGELOG_UNRELEASED)
    check("reads `Closes #N`", 1750 in refs, repr(sorted(refs)))
    check("reads `(#N)`", 1796 in refs, repr(sorted(refs)))
    check("reads a list `#a, #b and #c`", {1801, 1802, 1803} <= refs, repr(sorted(refs)))
    check("reads both sides of `#a/#b`", {1803, 1804} <= refs, repr(sorted(refs)))
    check("reads `Part of #N`", 1305 in refs, repr(sorted(refs)))
    check("reads the last reference of an entry", 1760 in refs, repr(sorted(refs)))
    # Message labels are quoted in code spans, and a code span may wrap a
    # line. Neither `#1` nor `#3` is an issue.
    check("a code span's `#1` is not an issue", 1 not in refs, repr(sorted(refs)))
    check("a code span wrapping a line is skipped", 3 not in refs, repr(sorted(refs)))
    check(
        "exactly the issues cited",
        refs == {1750, 1796, 1801, 1802, 1803, 1804, 1305, 1760},
        repr(sorted(refs)),
    )

    check(
        "a range counts its two ends, as written",
        issue_refs("not registered yet (#424-#426)") == {424, 426},
        repr(sorted(issue_refs("not registered yet (#424-#426)"))),
    )
    check("an HTML entity is not an issue", issue_refs("a &#8212; b") == set())
    check("a URL fragment is not an issue", issue_refs("see page#12 there") == set())
    check("`#12a` is not an issue", issue_refs("tag #12a") == set())
    check("a heading's `#` is not a number", issue_refs("## 3. Title\n") == set())
    check(
        "a reference inside a fenced block is skipped",
        issue_refs("text\n\n```sh\n# see #42\n```\n\nafter #43\n") == {43},
        repr(sorted(issue_refs("text\n\n```sh\n# see #42\n```\n\nafter #43\n"))),
    )
    # A blank line inside the block, as section 0's long setup block has, so
    # the fence is not just one code span to the paragraph reader.
    spaced = "```sh\n# for #42\n\n# for #46\n```\n\nafter #43\n"
    check(
        "a fenced block with a blank line in it is skipped",
        issue_refs(spaced) == {43},
        repr(sorted(issue_refs(spaced))),
    )
    check(
        "an HTML comment is skipped",
        issue_refs("before <!-- #44 --> after #45") == {45},
        repr(sorted(issue_refs("before <!-- #44 --> after #45"))),
    )
    check(
        "a reference at the start of a wrapped line counts",
        issue_refs("covered (#1849,\n#1844). The rest") == {1849, 1844},
        repr(sorted(issue_refs("covered (#1849,\n#1844). The rest"))),
    )

    # The plan side: only the numbered check sections from 1 up and the
    # "No manual check" list count.
    prefs = plan_refs(PLAN)
    check("a check line counts", 1750 in prefs, repr(sorted(prefs)))
    check("a section heading counts", {1803, 1804} <= prefs, repr(sorted(prefs)))
    check("the No manual check list counts", 1796 in prefs, repr(sorted(prefs)))
    check("setup's fenced comment does not count", 1801 not in prefs, repr(sorted(prefs)))
    check("setup's prose does not count", 1802 not in prefs, repr(sorted(prefs)))
    check("Recording the outcome does not count", 1760 not in prefs, repr(sorted(prefs)))
    check("a code span in a check does not count", 1305 not in prefs, repr(sorted(prefs)))
    check("exactly the counted references", prefs == {1750, 1796, 1803, 1804}, repr(sorted(prefs)))

    # The whole check: the issues of [Unreleased] the plan does not cover, and
    # nothing from a released section.
    gaps = missing(CHANGELOG, PLAN)
    check("names every uncovered issue, in order", gaps == [1305, 1760, 1801, 1802], repr(gaps))
    check("a released section is not checked", 1500 not in gaps, repr(gaps))

    covered = PLAN.replace("## No manual check\n", "## No manual check\n\n- Rest (#1305, #1760, #1801, #1802).\n")
    check("a covered plan passes", missing(CHANGELOG, covered) == [], repr(missing(CHANGELOG, covered)))

    renamed = covered.replace("## No manual check", "## Not checked")
    check(
        "a renamed No manual check heading stops counting",
        missing(CHANGELOG, renamed) == [1305, 1760, 1796, 1801, 1802],
        repr(missing(CHANGELOG, renamed)),
    )
    check(
        "a section 0 is not a check section even with a longer number",
        plan_refs("## 0. Setup\n\nSee #7.\n\n## 10. Ten\n\nSee #8.\n") == {8},
        repr(sorted(plan_refs("## 0. Setup\n\nSee #7.\n\n## 10. Ten\n\nSee #8.\n"))),
    )

    fenced_heading = "## 0. Setup\n\n```sh\n## 1. not a heading\n# for #7\n```\n\nSee #6.\n\n## 2. Two\n\nSee #8.\n"
    check(
        "a `## ` line inside a fence is not a heading",
        plan_refs(fenced_heading) == {8},
        repr(sorted(plan_refs(fenced_heading))),
    )

    # Right after a release prep the CHANGELOG has no [Unreleased] yet, or an
    # empty one: there is nothing to cover.
    check("no [Unreleased] section has nothing to cover", missing("## [0.6.2]\n- Closes #9.\n", PLAN) == [])
    check("an empty [Unreleased] has nothing to cover", missing("## [Unreleased]\n\n## [0.6.2]\n- #9\n", PLAN) == [])

    release_cycle()

    print()
    if failures:
        print(f"{failures} check(s) failed")
        return 1
    print("all checks passed")
    return 0


CHANGELOG_UNRELEASED = CHANGELOG.split("## [Unreleased]", 1)[1].split("## [0.6.1]", 1)[0]

if __name__ == "__main__":
    sys.exit(main())
