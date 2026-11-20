#!/usr/bin/env python3
"""Fail when an issue the CHANGELOG cites has no place in the release test plan.

    python3 tools/check_plan_coverage.py

Every issue `#N` cited under `## [Unreleased]` in CHANGELOG.md, and under the
workspace version's section while that version is not released (see "Which
CHANGELOG sections are checked" below), must appear in RELEASE-TEST-PLAN.md, in a check section's heading or text, or in its
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

**Which CHANGELOG sections are checked.** `[Unreleased]` always. The release
prep PR renames it to the version it cuts and leaves it empty, so from that
commit until the tag the issues to cover sit under `## [<workspace version>]`
instead (#1898). That section is checked too while the version is not
released yet.

"Not released yet" is read from the plan's own `**Baseline: vX.Y.Z` line: the
plan describes what changed since its baseline, and the post-release reset
rewrites it against the version that just shipped. So a workspace version
newer than the baseline has not shipped, and one equal to it has. A tag would
say the same, but CI's pre-commit job checks out without tags. The CHANGELOG
alone cannot tell: it reads the same just before the tag as just after it.
The check fails closed when the answer is unclear: when the plan has no
`**Baseline: vX.Y.Z` line before its first section, when the workspace
version is not of the form X.Y.Z, when the baseline is newer than the
workspace version, or when the version is not released and the CHANGELOG has
no section for it. A plan reset that forgets to move its baseline fails too,
on the shipped version's issues, and the message says to move it, once the
version is tagged. Moving the line before the tag is the one way to make the
check pass open: the version section then stops being checked.

The output names the sections checked and why. With no issue to cover, as
right after the reset, it says so rather than printing a bare OK.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

from release_notes import UNRELEASED, section, workspace_version

REPO_ROOT = Path(__file__).resolve().parent.parent
CHANGELOG = REPO_ROOT / "CHANGELOG.md"
PLAN = REPO_ROOT / "RELEASE-TEST-PLAN.md"
CARGO_TOML = REPO_ROOT / "Cargo.toml"

ISSUE_REF = re.compile(r"(?<![\w&])#(\d+)(?!\w)")
# A code span: a run of backticks, then the shortest text up to the same run.
# Applied per paragraph, so it may cross a line break but not a blank line.
CODE_SPAN = re.compile(r"(`+).*?\1", re.DOTALL)
HTML_COMMENT = re.compile(r"<!--.*?-->", re.DOTALL)
FENCE = re.compile(r"^\s*(```|~~~)")
CHECK_SECTION = re.compile(r"^## (\d+)\.\s")
NO_MANUAL_CHECK = "## No manual check"
BASELINE = re.compile(r"^\*\*Baseline: v(\d+\.\d+\.\d+)\b")
VERSION = re.compile(r"(\d+)\.(\d+)\.(\d+)")


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


def plan_baseline(plan: str) -> str | None:
    """The version on the plan's `**Baseline: vX.Y.Z` line, before its first section."""
    for line in plan.splitlines():
        if line.startswith("## "):
            break
        found = BASELINE.match(line)
        if found:
            return found.group(1)
    return None


def _version(text: str) -> tuple[int, ...] | None:
    return tuple(int(part) for part in text.split(".")) if VERSION.fullmatch(text) else None


def sections_to_check(changelog: str, plan: str, version: str) -> tuple[list[str], str]:
    """The CHANGELOG sections to check, and why those.

    `version` is the workspace version. See "Which CHANGELOG sections are
    checked" in the module docstring. When the answer is unclear the list is
    empty and the reason says why.
    """
    baseline = plan_baseline(plan)
    if baseline is None:
        return [], (
            "RELEASE-TEST-PLAN.md has no '**Baseline: vX.Y.Z' line before its first section, "
            f"so it cannot tell whether {version} is released"
        )
    ours, theirs = _version(version), _version(baseline)
    if ours is None or theirs is None:
        return [], f"the workspace version {version!r} is not of the form X.Y.Z"
    if theirs > ours:
        return [], f"the plan's baseline v{baseline} is newer than the workspace version {version}"
    if theirs == ours:
        return [UNRELEASED], f"{version} is the plan's baseline, so it is released"
    if section(changelog, version) is None:
        return [], (
            f"the workspace version {version} is newer than the plan's baseline v{baseline}, "
            f"so it is not released yet, but CHANGELOG.md has no '## [{version}]' section to check"
        )
    return [UNRELEASED, version], (
        f"{version} is newer than the plan's baseline v{baseline}, so it is not released yet"
    )


def missing(changelog: str, plan: str, names: list[str] | None = None) -> list[int]:
    """The issues the sections `names` cite that the plan does not, in order.

    `names` defaults to `[Unreleased]` alone.
    """
    return sorted(_cited(changelog, names or [UNRELEASED]) - plan_refs(plan))


def _cited(changelog: str, names: list[str]) -> set[int]:
    cited: set[int] = set()
    for name in names:
        cited |= issue_refs(section(changelog, name) or "")
    return cited


def check(changelog: str, plan: str, version: str) -> tuple[int, list[str]]:
    """The exit status and the lines to print, given the three files' text.

    `version` is the workspace version from `Cargo.toml`.
    """
    names, why = sections_to_check(changelog, plan, version)
    if not names:
        return 1, [f"plan coverage: {why}."]
    label = " and ".join(f"[{name}]" for name in names)
    lines = [f"plan coverage: checking {label} of CHANGELOG.md ({why})"]
    cited = _cited(changelog, names)
    if not cited:
        if names == [UNRELEASED] and section(changelog, UNRELEASED) is None:
            lines.append("plan coverage: there is no [Unreleased] section; nothing to cover")
        else:
            lines.append(f"plan coverage: no issue is cited under {label}; nothing to cover")
        return 0, lines
    gaps = missing(changelog, plan, names)
    if not gaps:
        lines.append(f"plan coverage: all {len(cited)} issues cited under {label} are in RELEASE-TEST-PLAN.md")
        return 0, lines
    lines.append(
        f"plan coverage: RELEASE-TEST-PLAN.md is missing {len(gaps)} of the {len(cited)} issues "
        f"cited under {label}:"
    )
    lines += [f"  #{n}" for n in gaps]
    lines.append(
        "Add a check for each, in a numbered section from 1 up (see 'How to add to this plan'), "
        "or a line in '## No manual check' saying why it has none and which test covers it. "
        "A mention in section 0's setup does not count."
    )
    # A reset that forgot the baseline fails on the shipped section's issues.
    # A gap only under [Unreleased] cannot come from that, and at prep moving
    # the line would switch the version section's check off, so say nothing.
    if version in names and set(gaps) & _cited(changelog, [version]):
        lines.append(
            f"Only once v{version} is tagged: if the plan was then reset, move its '**Baseline:' "
            f"line to v{version}. Moving it before the tag stops [{version}] being checked."
        )
    return 1, lines


def main() -> int:
    status, lines = check(
        CHANGELOG.read_text(encoding="utf-8"),
        PLAN.read_text(encoding="utf-8"),
        workspace_version(CARGO_TOML.read_text(encoding="utf-8")),
    )
    print("\n".join(lines))
    return status


if __name__ == "__main__":
    sys.exit(main())
