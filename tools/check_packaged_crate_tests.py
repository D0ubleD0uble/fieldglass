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

A crate that lists itself as a dev-dependency to switch on a feature for its own
tests (`fieldglass-core`'s `testing`, the umbrella's `schema`) loses that line in
the package, so those features are off in a plain `cargo test` and the tests
behind `required-features` are skipped. Each such crate therefore gets a second
`cargo test` pass with exactly those features, read from `cargo metadata`.

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


class StepError(Exception):
    """A step failed in a way worth one readable message, not a traceback."""


def capture(cmd: list[str], cwd: Path) -> str:
    """Run a command and return stdout; a failure becomes a `StepError`."""
    try:
        done = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, encoding="utf-8", check=False)
    except OSError as exc:
        raise StepError(f"could not run {cmd[0]}: {exc}") from exc
    if done.returncode != 0:
        detail = (done.stderr or done.stdout).strip()
        raise StepError(f"`{' '.join(cmd[:3])}` exited {done.returncode}:\n{detail}")
    return done.stdout


def self_dev_features(crate: dict) -> list[str]:
    """Features a crate turns on for its own tests through a self dev-dependency."""
    feats: set[str] = set()
    for dep in crate["dependencies"]:
        if dep["kind"] == "dev" and dep["name"] == crate["name"]:
            feats.update(dep["features"])
    return sorted(feats)


def publishable_crates() -> list[dict]:
    """Workspace members that `cargo publish` would upload (`publish` is not `[]`)."""
    meta = json.loads(capture(["cargo", "metadata", "--no-deps", "--format-version", "1"], REPO))
    members = set(meta["workspace_members"])
    return sorted(
        (p for p in meta["packages"] if p["id"] in members and p["publish"] != []),
        key=lambda p: p["name"],
    )


def unpack(archive: Path, dest: Path) -> Path:
    """Extract a `.crate` (gzip tar rooted at `<name>-<version>/`) under `dest`."""
    with tarfile.open(archive, "r:gz") as tar:
        # Our own `cargo package` output, extracted with the `data` filter,
        # which refuses absolute paths, `..` and links that leave `dest`.
        for member in tar:
            tar.extract(member, dest, filter="data")
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
    out = capture(["cargo", "metadata", "--format-version", "1", "--config", str(config)], directory)
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

    try:
        crates = publishable_crates()
    except StepError as exc:
        print(f"error: could not list the publishable crates: {exc}", file=sys.stderr)
        return 1
    names = [c["name"] for c in crates]
    print(f"publishable crates: {', '.join(names)}", flush=True)

    start = time.monotonic()
    scratch = Path(tempfile.mkdtemp(prefix="fieldglass-packaged-")).resolve()
    if scratch.is_relative_to(REPO):
        # cargo would find this checkout's workspace above the unpacked crates.
        shutil.rmtree(scratch, ignore_errors=True)
        print("error: TMPDIR is inside the checkout; point it elsewhere", file=sys.stderr)
        return 2
    try:
        # One target directory for all eight: the siblings are the same source,
        # so a shared one compiles each dependency once.
        env = {**os.environ, "CARGO_TARGET_DIR": str(scratch / "target")}

        package_args = ["package", "--no-verify", "--target-dir", str(scratch / "package-target")]
        if args.allow_dirty:
            package_args.append("--allow-dirty")
        for name in names:
            package_args += ["-p", name]
        # One call for all of them: a crate pinning a sibling with `=` can only be
        # packaged alongside it, so a failure here stops the run (nothing to test).
        if subprocess.run(["cargo", *package_args], cwd=REPO, check=False).returncode != 0:
            print("error: `cargo package` failed; nothing to test (see above)", file=sys.stderr)
            return 1
        packaged_at = time.monotonic()

        failures: list[str] = []
        unpacked: dict[str, Path] = {}
        for crate in crates:
            archive = scratch / "package-target" / "package" / f'{crate["name"]}-{crate["version"]}.crate'
            try:
                unpacked[crate["name"]] = unpack(archive, scratch / "src")
            except (OSError, tarfile.TarError) as exc:
                print(f"error: could not unpack {archive.name}: {exc}", file=sys.stderr)
                failures.append(f'{crate["name"]} (unpack)')

        config = scratch / "patch.toml"
        config.write_text(patch_config(unpacked), encoding="utf-8")

        for crate in crates:
            name = crate["name"]
            if name not in unpacked:
                continue
            directory = unpacked[name]
            print(f"\n=== {name}: cargo test from {directory}", flush=True)
            try:
                registry = sibling_sources_are_paths(directory, config, set(names), name)
            except StepError as exc:
                print(f"error: {name}: could not resolve the unpacked crate: {exc}", file=sys.stderr)
                failures.append(f"{name} (resolve)")
                continue
            if registry:
                print(f"error: {name} resolved a sibling from a registry: {registry}", file=sys.stderr)
                failures.append(f"{name} (sibling not patched)")
                continue
            passes: list[tuple[str, list[str]]] = [("default features", [])]
            feats = self_dev_features(crate)
            if feats:
                joined = ",".join(feats)
                passes.append((f"self dev-dependency features {joined}", ["--features", joined]))
            for label, extra in passes:
                print(f"--- {name}: {label}", flush=True)
                done = subprocess.run(
                    ["cargo", "test", "--no-fail-fast", "--config", str(config), *extra, *args.test_args],
                    cwd=directory,
                    env=env,
                    check=False,
                )
                if done.returncode != 0:
                    failures.append(f"{name} ({label})" if extra else name)
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
