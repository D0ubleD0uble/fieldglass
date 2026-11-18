#!/usr/bin/env python3
"""Fail when the crates.io publish list drifts from the workspace.

    python3 tools/check_publish_list.py

The `publish-crates` job in `.github/workflows/release.yml` publishes the
library crates from a list typed into its `for crate in ...` loop, and
RELEASING.md names the same crates in the same order under *crates.io*. Nothing
else derives that list. A new publishable crate missing from it would be
packaged and semver-checked on every PR but never published, and the
umbrella's `=` pin on it would then fail at tag time, after the Marketplace job
had already published in parallel (#1875).

This compares both lists with the publishable workspace members from
`cargo metadata --no-deps --locked` (every member whose `publish` is not
`false`). Each list must name every one of them exactly once, and nothing else,
in an order where every crate comes after the workspace crates it depends on:
`cargo publish` resolves a crate's dependencies from the registry, so a crate
published before one of its dependencies fails.

Normal and build dependencies are ordering edges. A dev-dependency is one only
when it carries a version: `cargo package` strips a path-only dev-dependency,
but keeps a versioned one, which the registry must then resolve
(`fieldglass-grib2`'s tests pin `fieldglass-grib1`). A crate's dev-dependency on
itself, used to switch on a feature for its own tests, is not an edge.

`tools/check_packaged_crate_tests.py` and `tools/check_crate_license_files.py`
read the same publishable set from `cargo metadata`, but neither reads the
publish list, and both run `cargo package`; this check only parses text and
takes about a second.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
RELEASE_YML = Path(".github/workflows/release.yml")
RELEASING_MD = Path("RELEASING.md")


class ListError(Exception):
    """The list could not be found where the checker expects it."""


def publishable_crates(metadata: dict) -> dict[str, set[str]]:
    """Each publishable workspace member, with the publishable members it must follow.

    `publish` is `null` when the manifest says nothing and a list of registries
    otherwise; `publish = false` is the empty list.
    """
    members = set(metadata["workspace_members"])
    packages = [p for p in metadata["packages"] if p["id"] in members and p["publish"] != []]
    names = {p["name"] for p in packages}
    crates: dict[str, set[str]] = {}
    for package in packages:
        edges = set()
        for dep in package["dependencies"]:
            if dep["name"] not in names or dep["name"] == package["name"]:
                continue
            if dep["kind"] == "dev" and dep["req"] == "*":
                continue
            edges.add(dep["name"])
        crates[package["name"]] = edges
    return crates


def release_yml_list(text: str) -> list[str]:
    """The crates in the `for crate in ...; do` loop of the `publish-crates` job."""
    job = re.search(r"^  publish-crates:\n(.*?)(?=^  \S|\Z)", text, re.M | re.S)
    if job is None:
        raise ListError("no `publish-crates` job")
    loop = re.search(r"\bfor crate in (.*?); do\b", job.group(1), re.S)
    if loop is None:
        raise ListError("no `for crate in ...; do` loop in the `publish-crates` job")
    return loop.group(1).replace("\\\n", " ").split()


def releasing_md_list(text: str) -> list[str]:
    """The crates named after "dependency order:" under `### crates.io`.

    A short name such as `-grib1` stands for `fieldglass-grib1`.
    """
    section = re.search(r"^### crates\.io\n(.*?)(?=^#{2,3} |\Z)", text, re.M | re.S)
    if section is None:
        raise ListError("no `### crates.io` section")
    sentence = re.search(r"dependency order:(.*?)\.(?:\s|$)", section.group(1), re.S)
    if sentence is None:
        raise ListError('no "dependency order:" sentence under `### crates.io`')
    names = re.findall(r"`([^`]+)`", sentence.group(1))
    return [f"fieldglass{n}" if n.startswith("-") else n for n in names]


def problems(listed: list[str], crates: dict[str, set[str]], where: str) -> list[str]:
    """Every way `listed` differs from a dependency-ordered list of `crates`."""
    found = []
    for name in sorted(set(crates) - set(listed)):
        found.append(f"{where}: {name} is missing (it is a publishable workspace member)")
    for name in sorted(set(listed) - set(crates)):
        found.append(f"{where}: {name} is not a publishable workspace member")
    position: dict[str, int] = {}
    for index, name in enumerate(listed):
        if name in position:
            found.append(f"{where}: {name} is listed twice")
        else:
            position[name] = index
    for name, index in position.items():
        for dep in sorted(crates.get(name, ())):
            if position.get(dep, -1) > index:
                found.append(f"{where}: {name} is listed before {dep}, which it depends on")
    return found


def main() -> int:
    done = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--locked", "--format-version", "1"],
        cwd=REPO,
        capture_output=True,
        encoding="utf-8",
        check=False,
    )
    if done.returncode != 0:
        print(f"error: cargo metadata failed:\n{done.stderr.strip()}", file=sys.stderr)
        return 1
    crates = publishable_crates(json.loads(done.stdout))
    if not crates:
        print("error: cargo metadata listed no publishable crate; nothing was checked", file=sys.stderr)
        return 1

    found = []
    for path, parse in ((RELEASE_YML, release_yml_list), (RELEASING_MD, releasing_md_list)):
        try:
            listed = parse((REPO / path).read_text(encoding="utf-8"))
        except (OSError, ListError) as exc:
            found.append(f"{path}: {exc}")
            continue
        found.extend(problems(listed, crates, str(path)))

    if found:
        for line in found:
            print(f"error: {line}", file=sys.stderr)
        print(
            "The crates.io publish list must name every publishable workspace member once, "
            "each after the workspace crates it depends on. Fix the list (or the manifest's "
            "`publish` key) so the two agree.",
            file=sys.stderr,
        )
        return 1
    print(f"ok: both publish lists match the {len(crates)} publishable crates in dependency order")
    return 0


if __name__ == "__main__":
    sys.exit(main())
