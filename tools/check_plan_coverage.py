#!/usr/bin/env python3
"""Fail when an issue the CHANGELOG cites has no place in the release test plan.

    python3 tools/check_plan_coverage.py

Every issue `#N` cited under `## [Unreleased]` in CHANGELOG.md must appear in
RELEASE-TEST-PLAN.md, in a check section's heading or text, or in its
"No manual check" list with the reason it has no check (#1873). Nothing else
made a new change show up in the plan, and the 0.6.2 cycle needed three
plan-coverage PRs to catch up (#1854, #1859, #1864).

**What counts as an issue reference.** `#` followed by digits, where the `#`
does not follow a letter, digit, `_` or `&`, and no letter, digit or `_`
follows the digits. Text inside a code span or a fenced code block is not read,
and neither is an HTML comment. That rule covers every form the CHANGELOG
uses: `Closes #N.`, `Part of #N`, `(#N)`, `(#a, #b)`, `#a-#b`, and a
reference that starts a wrapped line. It leaves out what is not an issue:

- a message label quoted from the panel, such as `#1 · VGRD · 2 · +0h`. The
  CHANGELOG quotes UI text in code spans, and a code span may wrap a line, so
  spans are matched across line breaks within a paragraph;
- an HTML entity (`&#8212;`), a URL fragment (`page#12`) and a token such as
  `#12a`;
- a Markdown heading's `#`, which a space follows, not a digit.

A table or template number is written without `#` in both files ("Code Table
4.11", "template 3.30"), so it never matches. A range `#a-#b` counts only its
two ends, as written. If quoted UI text with a `#N` were ever written outside a
code span it would be read as an issue and this check would ask for a plan
line: it fails closed, never open.

**Which parts of the plan count.** The numbered sections from `## 1.` up, and
`## No manual check`. The title, "How to add to this plan", section 0 (setup,
whose comments name the issues each built file is for) and "Recording the
outcome" do not: a mention there is not a check, and counting it would let a
change pass with only its fixture built. A section that is renamed out of that
shape stops counting, so a renamed "No manual check" fails every issue it held
rather than passing silently.

Only that direction is checked. The plan may name issues the CHANGELOG does
not, as its "No manual check" list does for CI and docs work.

Right after a release prep there is no `[Unreleased]` section, or an empty one,
and so nothing to cover; the script says so rather than printing a bare OK.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

from release_notes import UNRELEASED, section

REPO_ROOT = Path(__file__).resolve().parent.parent
CHANGELOG = REPO_ROOT / "CHANGELOG.md"
PLAN = REPO_ROOT / "RELEASE-TEST-PLAN.md"

ISSUE_REF = re.compile(r"(?<![\w&])#(\d+)(?!\w)")
# A code span: a run of backticks, then the shortest text up to the same run.
# Applied per paragraph, so it may cross a line break but not a blank line.
CODE_SPAN = re.compile(r"(`+).*?\1", re.DOTALL)
HTML_COMMENT = re.compile(r"<!--.*?-->", re.DOTALL)
FENCE = re.compile(r"^\s*(```|~~~)")
CHECK_SECTION = re.compile(r"^## (\d+)\.\s")
NO_MANUAL_CHECK = "## No manual check"


def _paragraphs(text: str) -> list[str]:
    """`text` split at blank lines, with fenced code blocks left out."""
    paragraphs: list[str] = []
    current: list[str] = []
    fence: str | None = None
    for line in text.splitlines():
        opened = FENCE.match(line)
        if fence is not None:
            if opened and opened.group(1) == fence:
                fence = None
            continue
        if opened:
            fence = opened.group(1)
            paragraphs.append("\n".join(current))
            current = []
            continue
        if not line.strip():
            paragraphs.append("\n".join(current))
            current = []
            continue
        current.append(line)
    paragraphs.append("\n".join(current))
    return paragraphs


def issue_refs(text: str) -> set[int]:
    """Every issue number `text` cites, by the rule in the module docstring."""
    text = HTML_COMMENT.sub("", text)
    refs: set[int] = set()
    for paragraph in _paragraphs(text):
        prose = CODE_SPAN.sub("", paragraph)
        refs.update(int(n) for n in ISSUE_REF.findall(prose))
    return refs


def plan_refs(plan: str) -> set[int]:
    """The issues the plan's check sections and "No manual check" list name."""
    counted: list[str] = []
    keep = False
    fence: str | None = None
    for line in plan.splitlines():
        # A `## ` line inside a fenced block is a shell comment, not a heading.
        opened = FENCE.match(line)
        if fence is not None:
            if opened and opened.group(1) == fence:
                fence = None
        elif opened:
            fence = opened.group(1)
        elif line.startswith("## "):
            numbered = CHECK_SECTION.match(line)
            keep = (numbered is not None and int(numbered.group(1)) >= 1) or (
                line.rstrip() == NO_MANUAL_CHECK
            )
        if keep:
            counted.append(line)
    return issue_refs("\n".join(counted))


def missing(changelog: str, plan: str) -> list[int]:
    """The issues `[Unreleased]` cites that the plan does not, in order."""
    unreleased = section(changelog, UNRELEASED) or ""
    return sorted(issue_refs(unreleased) - plan_refs(plan))


def main() -> int:
    changelog = CHANGELOG.read_text(encoding="utf-8")
    plan = PLAN.read_text(encoding="utf-8")
    unreleased = section(changelog, UNRELEASED)
    cited = issue_refs(unreleased or "")
    if not cited:
        where = "no [Unreleased] section" if unreleased is None else "no issue under [Unreleased]"
        print(f"plan coverage: CHANGELOG.md has {where}; nothing to cover")
        return 0
    gaps = missing(changelog, plan)
    if not gaps:
        print(f"plan coverage: all {len(cited)} issues cited under [Unreleased] are in RELEASE-TEST-PLAN.md")
        return 0
    print(
        f"plan coverage: RELEASE-TEST-PLAN.md is missing {len(gaps)} of the {len(cited)} issues "
        "cited under [Unreleased] in CHANGELOG.md:"
    )
    for n in gaps:
        print(f"  #{n}")
    print(
        "Add a check for each, in a numbered section from 1 up (see 'How to add to this plan'), "
        "or a line in '## No manual check' saying why it has none and which test covers it. "
        "A mention in section 0's setup does not count."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
