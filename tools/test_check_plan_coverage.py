#!/usr/bin/env python3
"""Self-test for `check_plan_coverage.py`.

    python3 tools/test_check_plan_coverage.py

Asserts on the sets the parsing functions return, not only on an exit status:
a reader that found no issue in the CHANGELOG would pass every plan, and one
that counted the plan's setup comments would pass a plan with no check.
"""

from __future__ import annotations

import sys

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

    print()
    if failures:
        print(f"{failures} check(s) failed")
        return 1
    print("all checks passed")
    return 0


CHANGELOG_UNRELEASED = CHANGELOG.split("## [Unreleased]", 1)[1].split("## [0.6.1]", 1)[0]

if __name__ == "__main__":
    sys.exit(main())
