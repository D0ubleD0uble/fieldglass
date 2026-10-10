#!/usr/bin/env python3
"""Run each published crate's tests from its unpacked `.crate`.

    python3 tools/check_packaged_crate_tests.py [--allow-dirty] [--keep] [-- <cargo test args>]

A crate on crates.io is not the directory in this repository. The `.crate`
carries only the files under the crate's own directory: not a sibling crate's
fixtures, not a `fuzz/` seed (a separate package), and no path-only
dev-dependency, which `cargo package` strips (#926). A test that reaches for any
of them passes in the workspace and fails for the first person who runs
`cargo test` on the published crate. `cargo publish --workspace --dry-run` builds
each library but runs no tests, so nothing else notices.

This packages every publishable workspace member, unpacks each outside the
checkout, and runs `cargo test` in the unpacked copy. Every sibling is patched
to its own unpacked copy, so the run cannot fall back to the same version on
crates.io; before each test run the resolved graph is checked to prove it.

The publishable set is read from `cargo metadata` (a member with
`publish = false` is out), so a ninth crate joins this check without an edit
here. `release.yml`'s publish loop is its own list; `release.yml` and this
script agree because both are the set of crates that publish.

`fieldglass` (the umbrella) is packaged without its integration tests, which
need the whole repository and are left out by `exclude`; what is tested here is
what a user of the crate gets.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent


def cargo(*args: str, cwd: Path = REPO, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    """Run cargo, streaming its output; raises on a non-zero exit."""
    return subprocess.run(
        ["cargo", *args], cwd=cwd, env=env, check=True, text=True, encoding="utf-8"
    )


def publishable_crates() -> list[dict]:
    """Workspace members that `cargo publish` would upload (`publish` is not `[]`)."""
    out = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=REPO,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout
    meta = json.loads(out)
    members = set(meta["workspace_members"])
    return sorted(
        (p for p in meta["packages"] if p["id"] in members and p["publish"] != []),
        key=lambda p: p["name"],
    )


def unpack(archive: Path, dest: Path) -> Path:
    """Extract a `.crate` (gzip tar rooted at `<name>-<version>/`) under `dest`."""
    with tarfile.open(archive, "r:gz") as tar:
        tar.extractall(dest, filter="data")
    return dest / archive.name.removesuffix(".crate")


def patch_config(unpacked: dict[str, Path]) -> str:
    """A cargo config pointing every published crate at its unpacked copy."""
    lines = ["[patch.crates-io]"]
    for name, path in sorted(unpacked.items()):
        lines.append(f'{name} = {{ path = "{path.as_posix()}" }}')
    return "\n".join(lines) + "\n"


def sibling_sources_are_paths(directory: Path, config: Path, names: set[str], own: str) -> list[str]:
    """Names of published siblings in `directory`'s graph that did not resolve to a path.

    A registry source here would mean the test run is exercising crates.io's copy
    of a sibling, not the one just packaged.
    """
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--config", str(config)],
        cwd=directory,
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout
    bad = []
    for pkg in json.loads(out)["packages"]:
        if pkg["name"] in names and pkg["name"] != own and pkg["source"] is not None:
            bad.append(f'{pkg["name"]} from {pkg["source"]}')
    return bad


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--allow-dirty", action="store_true", help="package an uncommitted tree (local runs)")
    parser.add_argument("--keep", action="store_true", help="keep the unpacked crates and print where")
    parser.add_argument("test_args", nargs="*", help="extra arguments for `cargo test` (after --)")
    args = parser.parse_args()

    crates = publishable_crates()
    names = [c["name"] for c in crates]
    print(f"publishable crates: {', '.join(names)}", flush=True)

    start = time.monotonic()
    scratch = Path(tempfile.mkdtemp(prefix="fieldglass-packaged-"))
    try:
        # One target directory for all eight: the siblings are the same source,
        # so a shared one compiles each dependency once.
        env = {**os.environ, "CARGO_TARGET_DIR": str(scratch / "target")}

        package_args = ["package", "--no-verify", "--target-dir", str(scratch / "package-target")]
        if args.allow_dirty:
            package_args.append("--allow-dirty")
        for name in names:
            package_args += ["-p", name]
        cargo(*package_args)
        packaged_at = time.monotonic()

        unpacked: dict[str, Path] = {}
        for crate in crates:
            archive = scratch / "package-target" / "package" / f'{crate["name"]}-{crate["version"]}.crate'
            unpacked[crate["name"]] = unpack(archive, scratch / "src")

        config = scratch / "patch.toml"
        config.write_text(patch_config(unpacked), encoding="utf-8")

        failures: list[str] = []
        for crate in crates:
            name, directory = crate["name"], unpacked[crate["name"]]
            print(f"\n=== {name}: cargo test from {directory}", flush=True)
            registry = sibling_sources_are_paths(directory, config, set(names), name)
            if registry:
                print(f"error: {name} resolved a sibling from a registry: {registry}", file=sys.stderr)
                failures.append(f"{name} (sibling not patched)")
                continue
            done = subprocess.run(
                ["cargo", "test", "--config", str(config), *args.test_args],
                cwd=directory,
                env=env,
                check=False,
            )
            if done.returncode != 0:
                failures.append(name)
        end = time.monotonic()

        print(
            f"\npackaging {packaged_at - start:.0f}s, unpack + tests {end - packaged_at:.0f}s",
            flush=True,
        )
        if args.keep:
            print(f"kept: {scratch}")
        if failures:
            print(f"FAILED from the unpacked .crate: {', '.join(failures)}", file=sys.stderr)
            print(
                "A test reads a file outside its own crate, or relies on a dev-dependency "
                "`cargo package` strips. Copy the fixture into the crate (see RELEASING.md, #926).",
                file=sys.stderr,
            )
            return 1
        print(f"ok: all {len(names)} published crates pass from their unpacked .crate")
        return 0
    finally:
        if not args.keep:
            shutil.rmtree(scratch, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
