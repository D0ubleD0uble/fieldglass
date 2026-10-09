#!/usr/bin/env python3
"""Extract a release's notes from CHANGELOG.md and check they can be published.

    python3 tools/release_notes.py --tag vX.Y.Z --out release-notes.md
    python3 tools/release_notes.py --dry-run

`release.yml` uses a CHANGELOG section as the GitHub Release body. GitHub
rejects a body over 125,000 characters, and by the time the Release step runs
the extension is already on the Marketplace, which cannot be undone (#858). So
the workflow runs this first, in its own job that every publishing job waits
for, and on a `workflow_dispatch` dry run too.

**On a tag** (`--tag vX.Y.Z`) the section is `## [X.Y.Z]`. It must exist and
have content, and the tag must match the workspace version in `Cargo.toml`,
since every channel publishes that version.

**On a dry run** (`--dry-run`) there is no tag, so the check covers each
section that could become the next release's notes:

- `## [<workspace version>]`, if that heading exists. After the prep PR it is
  the section the tag will publish. Between releases it is the last release's
  section, already published, so checking it again costs nothing.
- `## [Unreleased]`, if it has content. Between releases it is what the next
  prep promotes, so a dry run on `master` finds an oversized section before
  prep does.

At least one of the two must be present. The file written to `--out` is the
first that applies, in that order.

A section runs from its heading to the next line starting `## [`, the same
rule as the `awk` this replaced. Length is counted in characters (Python
`len()`), which is what GitHub's limit counts, not in bytes.
"""

from __future__ import annotations

import argparse
import sys
import tomllib
from pathlib import Path

GITHUB_RELEASE_BODY_LIMIT = 125_000
UNRELEASED = "Unreleased"


def section(changelog: str, name: str) -> str | None:
    """The lines under `## [name]`, up to the next `## [` heading.

    `None` when the heading is absent, so a missing section and an empty one
    stay distinguishable. The result ends with a newline per line, as `awk`'s
    `print` wrote it.
    """
    heading = f"## [{name}]"
    lines: list[str] | None = None
    for line in changelog.splitlines():
        if lines is None:
            if line.startswith(heading):
                lines = []
            continue
        if line.startswith("## ["):
            break
        lines.append(line)
    if lines is None:
        return None
    return "".join(f"{line}\n" for line in lines)


def problems(name: str, body: str | None) -> list[str]:
    """Why `body` cannot be the release body for `## [name]`; empty if it can."""
    if body is None:
        return [f"CHANGELOG.md has no '## [{name}]' section"]
    if not body.strip():
        return [f"the '## [{name}]' section of CHANGELOG.md is empty"]
    if len(body) > GITHUB_RELEASE_BODY_LIMIT:
        return [
            f"the '## [{name}]' section of CHANGELOG.md is {len(body):,} characters; "
            f"a GitHub Release body is limited to {GITHUB_RELEASE_BODY_LIMIT:,}"
        ]
    return []


def workspace_version(cargo_toml: str) -> str:
    return tomllib.loads(cargo_toml)["workspace"]["package"]["version"]


def for_tag(changelog: str, version: str, tag: str) -> tuple[str | None, list[str]]:
    """The notes for tag `tag`, and every reason they cannot be published."""
    if not tag.startswith("v"):
        return None, [f"tag {tag!r} is not of the form vX.Y.Z"]
    wanted = tag[1:]
    errors = []
    if wanted != version:
        errors.append(f"tag {tag} does not match the workspace version {version}")
    body = section(changelog, wanted)
    errors += problems(wanted, body)
    print(f"[{wanted}]: {len(body or ''):,} characters")
    return (body if not errors else None), errors


def for_dry_run(changelog: str, version: str) -> tuple[str | None, list[str]]:
    """The notes a dry run checks: see the module docstring."""
    candidates = []
    versioned = section(changelog, version)
    if versioned is not None:
        candidates.append((version, versioned))
    unreleased = section(changelog, UNRELEASED)
    if unreleased is not None and unreleased.strip():
        candidates.append((UNRELEASED, unreleased))
    if not candidates:
        return None, [
            f"CHANGELOG.md has neither a '## [{version}]' section nor a "
            f"non-empty '## [{UNRELEASED}]' section"
        ]
    errors = []
    for name, body in candidates:
        print(f"[{name}]: {len(body):,} characters")
        errors += problems(name, body)
    return (candidates[0][1] if not errors else None), errors


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--tag", help="the release tag, vX.Y.Z")
    mode.add_argument(
        "--dry-run",
        action="store_true",
        help="no tag: check the workspace version's section and [Unreleased]",
    )
    parser.add_argument("--changelog", default="CHANGELOG.md", type=Path)
    parser.add_argument("--cargo-toml", default="Cargo.toml", type=Path)
    parser.add_argument("--out", type=Path, help="where to write the notes")
    args = parser.parse_args(argv)

    changelog = args.changelog.read_text(encoding="utf-8")
    version = workspace_version(args.cargo_toml.read_text(encoding="utf-8"))
    if args.tag is not None:
        body, errors = for_tag(changelog, version, args.tag)
    else:
        body, errors = for_dry_run(changelog, version)

    for error in errors:
        # The `::error::` prefix makes GitHub Actions show it as an annotation.
        print(f"::error::{error}", file=sys.stderr)
    if errors:
        return 1
    if args.out is not None and body is not None:
        args.out.write_text(body, encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
