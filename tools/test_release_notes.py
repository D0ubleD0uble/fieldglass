#!/usr/bin/env python3
"""Self-test for `release_notes.py`.

    python3 tools/test_release_notes.py

The check stands between a release and an irreversible Marketplace publish
(#858), so each case below is one a release has hit or would hit: an oversized
section, a missing one, an empty one, and a dry run on `master` between
releases, where the workspace version is the last release's.
"""

from __future__ import annotations

import contextlib
import io
import sys
import tempfile
from pathlib import Path

from release_notes import (
    GITHUB_RELEASE_BODY_LIMIT,
    for_dry_run,
    for_tag,
    main,
    section,
    workspace_version,
)

failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


def changelog(unreleased: str, released: str = "### Added\n- old entry\n") -> str:
    return (
        "# Changelog\n\nIntro.\n\n"
        f"## [Unreleased]\n{unreleased}\n"
        f"## [0.5.0] — 2026-09-04\n{released}\n"
        "## [0.4.0] — 2026-08-23\n- older\n\n"
        "[Unreleased]: https://example.invalid/compare/v0.5.0...HEAD\n"
    )


CARGO_TOML = '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "0.5.0"\n'

# Over the limit in characters but not by much, and multi-byte, so a byte
# count would trip earlier than a character count and the test can tell them
# apart.
OVERSIZED = "- " + "é" * GITHUB_RELEASE_BODY_LIMIT + "\n"
# Under the limit in characters, over it in UTF-8 bytes.
UNDER_IN_CHARS = "- " + "é" * (GITHUB_RELEASE_BODY_LIMIT // 2 + 10) + "\n"


def quiet(fn, *args):
    with contextlib.redirect_stdout(io.StringIO()):
        return fn(*args)


def test_section() -> None:
    text = changelog("### Added\n- new entry\n")
    check(
        "extracts a section up to the next heading",
        section(text, "0.5.0") == "### Added\n- old entry\n\n",
        repr(section(text, "0.5.0")),
    )
    check("an absent heading is None", section(text, "9.9.9") is None)
    check(
        "an empty section is not None",
        section(changelog(""), "Unreleased") == "\n",
        repr(section(changelog(""), "Unreleased")),
    )
    check(
        "a version is not matched as a prefix of a longer one",
        section("## [0.5.0-rc1]\n- x\n", "0.5.0") is None,
    )
    check("reads the workspace version", workspace_version(CARGO_TOML) == "0.5.0")


def test_tag() -> None:
    text = changelog("- pending\n")
    body, errors = quiet(for_tag, text, "0.5.0", "v0.5.0")
    check("a tag takes its own section", body == "### Added\n- old entry\n\n", repr(body))
    check("a good tag has no errors", errors == [], repr(errors))

    _, errors = quiet(for_tag, changelog("- x\n", OVERSIZED), "0.5.0", "v0.5.0")
    check(
        "an oversized tag section fails",
        any("characters" in e for e in errors),
        repr(errors),
    )

    body, errors = quiet(for_tag, changelog("- x\n", UNDER_IN_CHARS), "0.5.0", "v0.5.0")
    check(
        "the limit counts characters, not bytes",
        errors == [] and len(body.encode()) > GITHUB_RELEASE_BODY_LIMIT,
        repr(errors),
    )

    _, errors = quiet(for_tag, text, "0.6.0", "v0.6.0")
    check("a tag with no section fails", any("no '## [0.6.0]'" in e for e in errors), repr(errors))

    _, errors = quiet(for_tag, text, "0.6.0", "v0.5.0")
    check(
        "a tag that is not the workspace version fails",
        any("does not match" in e for e in errors),
        repr(errors),
    )

    _, errors = quiet(for_tag, changelog("- x\n", "\n  \n"), "0.5.0", "v0.5.0")
    check("a whitespace-only tag section fails", any("empty" in e for e in errors), repr(errors))


def test_dry_run() -> None:
    # Between releases: the workspace version is the last release's, whose
    # section exists, and [Unreleased] is where the next release is growing.
    # A dry run must look at [Unreleased] too, or it passes on old notes.
    _, errors = quiet(for_dry_run, changelog(OVERSIZED), "0.5.0")
    check(
        "between releases, an oversized [Unreleased] fails",
        any("[Unreleased]" in e for e in errors),
        repr(errors),
    )

    body, errors = quiet(for_dry_run, changelog("- pending\n"), "0.5.0")
    check("between releases, both sections fit", errors == [], repr(errors))
    check("the versioned section is the one written", body == "### Added\n- old entry\n\n", repr(body))

    # After prep: [Unreleased] is empty and the new version has its section.
    body, errors = quiet(for_dry_run, changelog("", OVERSIZED), "0.5.0")
    check(
        "after prep, an oversized versioned section fails",
        any("[0.5.0]" in e for e in errors),
        repr(errors),
    )

    # A prep that bumped the version but forgot to promote the CHANGELOG.
    body, errors = quiet(for_dry_run, changelog("- pending\n"), "0.6.0")
    check("no versioned section falls back to [Unreleased]", errors == [], repr(errors))
    check("and writes [Unreleased]", body == "- pending\n\n", repr(body))

    _, errors = quiet(for_dry_run, changelog("\n"), "0.6.0")
    check(
        "neither section present fails",
        any("neither" in e for e in errors),
        repr(errors),
    )


def test_main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        (root / "Cargo.toml").write_text(CARGO_TOML, encoding="utf-8")
        (root / "CHANGELOG.md").write_text(changelog("- pending\n"), encoding="utf-8")
        out = root / "notes.md"
        args = [
            "--changelog",
            str(root / "CHANGELOG.md"),
            "--cargo-toml",
            str(root / "Cargo.toml"),
            "--out",
            str(out),
        ]
        sink = io.StringIO()
        with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
            status = main(["--tag", "v0.5.0", *args])
        check("main exits 0 on a good tag", status == 0, sink.getvalue())
        check("main writes the notes", out.read_text(encoding="utf-8").startswith("### Added"))

        out.unlink()
        (root / "CHANGELOG.md").write_text(changelog(OVERSIZED), encoding="utf-8")
        sink = io.StringIO()
        with contextlib.redirect_stdout(sink), contextlib.redirect_stderr(sink):
            status = main(["--dry-run", *args])
        check("main exits 1 on an oversized dry run", status == 1, sink.getvalue())
        check("main annotates the failure", "::error::" in sink.getvalue(), sink.getvalue())
        check("main writes nothing on failure", not out.exists())


def run() -> int:
    test_section()
    test_tag()
    test_dry_run()
    test_main()
    if failures:
        print(f"{failures} check(s) failed")
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(run())
