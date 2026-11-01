#!/usr/bin/env python3
"""Fail when a published crate would ship without its license text.

    python3 tools/check_crate_license_files.py

Every library crate declares `license = "MIT OR Apache-2.0"`, and both licenses
ask for their text to travel with the software. `cargo package` packs a crate
from its own directory, so the workspace root's `LICENSE-MIT` and
`LICENSE-APACHE` are not in it unless the crate directory has them too. The
0.6.0 crates went out without them (#1545): a notice generator such as
cargo-about then finds no license text in the crate and falls back to the SPDX
template, with `<year>` and `<copyright holders>` left as placeholders.

Each crate directory holds the two files as symlinks to the root copies, which
`cargo package` follows and packs as ordinary files. This checker asks cargo
itself, through `cargo package --list`, rather than looking for the files on
disk, so an `include` or `exclude` in a manifest that drops them is caught too.
It also compares each one with the root copy, because a checkout without
symlink support (Windows, by default) turns a symlink into a one-line text file
that cargo would pack as the license.

The crates come from `cargo metadata`: every workspace member without
`publish = false`. A crate added later is covered without editing this file.
`--list` builds nothing, so the whole check takes well under a second, and it
works at a version that is not on crates.io yet, as during release prep.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The files each published crate must carry, with the root copy they must match.
REQUIRED = ("LICENSE-MIT", "LICENSE-APACHE")


def publishable_crates(metadata: dict) -> list[tuple[str, Path]]:
    """Name and directory of every workspace member cargo would publish.

    `publish` is `null` when the manifest says nothing (publishable anywhere)
    and a list of registries otherwise; `publish = false` is the empty list.
    """
    members = set(metadata["workspace_members"])
    crates = []
    for package in metadata["packages"]:
        if package["id"] not in members:
            continue
        if package["publish"] == []:
            continue
        crates.append((package["name"], Path(package["manifest_path"]).parent))
    return sorted(crates)


def missing_from_listing(listing: str) -> list[str]:
    """The required files absent from `cargo package --list` output.

    Only a top-level path counts: `docs/LICENSE-MIT` would be packed, but it is
    not where a reader or a notice generator looks.
    """
    packed = {line.strip() for line in listing.splitlines()}
    return [name for name in REQUIRED if name not in packed]


def main() -> int:
    metadata = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--no-deps"],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            encoding="utf-8",
        ).stdout
    )
    crates = publishable_crates(metadata)
    if not crates:
        print("cargo metadata listed no publishable crate; nothing was checked", file=sys.stderr)
        return 1

    failed = False
    for name, crate_dir in crates:
        # `--allow-dirty` because the hook runs on uncommitted work.
        result = subprocess.run(
            ["cargo", "package", "--list", "--allow-dirty", "-p", name],
            cwd=REPO_ROOT,
            capture_output=True,
            encoding="utf-8",
        )
        if result.returncode != 0:
            print(f"{name}: `cargo package --list` failed:\n{result.stderr}", file=sys.stderr)
            failed = True
            continue

        for file in missing_from_listing(result.stdout):
            print(
                f"{name}: the published crate would not contain {file}. Add it to "
                f"{crate_dir.relative_to(REPO_ROOT)}/ as a symlink to ../../{file}, and "
                f"check that no `include` or `exclude` in its Cargo.toml drops it",
                file=sys.stderr,
            )
            failed = True

        for file in REQUIRED:
            packed = crate_dir / file
            if not packed.is_file():
                continue  # already reported as missing above
            if packed.read_bytes() != (REPO_ROOT / file).read_bytes():
                print(
                    f"{name}: {packed.relative_to(REPO_ROOT)} differs from the root {file}. "
                    f"It should be a symlink to ../../{file}; a checkout without symlink "
                    f"support writes a text file holding the link path instead",
                    file=sys.stderr,
                )
                failed = True

    if failed:
        return 1
    print(f"license files OK: {len(crates)} publishable crates each pack {' and '.join(REQUIRED)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
