#!/usr/bin/env python3
"""Self-test for `check_crate_license_files.py`.

    python3 tools/test_check_crate_license_files.py

Asserts on the two parsing functions directly. A checker that read the wrong
field of `cargo metadata` would see no publishable crate, or count an
unpublished one, and either way stop meaning anything while still passing.
"""

from __future__ import annotations

import sys

from check_crate_license_files import missing_from_listing, publishable_crates

failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


def package(name: str, publish: list[str] | None) -> dict:
    return {
        "id": f"path+file:///ws/crates/{name}#0.1.0",
        "name": name,
        "publish": publish,
        "manifest_path": f"/ws/crates/{name}/Cargo.toml",
    }


def main() -> int:
    metadata = {
        "packages": [
            package("published", None),
            package("unpublished", []),
            package("registry-only", ["crates-io"]),
            package("not-a-member", None),
        ],
        "workspace_members": [
            "path+file:///ws/crates/published#0.1.0",
            "path+file:///ws/crates/unpublished#0.1.0",
            "path+file:///ws/crates/registry-only#0.1.0",
        ],
    }
    names = [name for name, _ in publishable_crates(metadata)]
    check("a crate with no `publish` key is published", "published" in names, repr(names))
    check("`publish = false` is skipped", "unpublished" not in names, repr(names))
    check("a registry list is published", "registry-only" in names, repr(names))
    check("a non-member package is skipped", "not-a-member" not in names, repr(names))

    full = ".cargo_vcs_info.json\nCargo.toml\nLICENSE-APACHE\nLICENSE-MIT\nREADME.md\nsrc/lib.rs\n"
    check("a listing with both files misses nothing", missing_from_listing(full) == [])

    # The 0.6.0 crates (#1545): a listing with neither must report both.
    bare = ".cargo_vcs_info.json\nCargo.toml\nREADME.md\nsrc/lib.rs\n"
    missing = missing_from_listing(bare)
    check("a listing with neither reports both", missing == ["LICENSE-MIT", "LICENSE-APACHE"], repr(missing))

    # A license file below the crate root is not the one a reader looks for.
    nested = "Cargo.toml\nLICENSE-APACHE\ndocs/LICENSE-MIT\n"
    missing = missing_from_listing(nested)
    check("a nested copy does not count", missing == ["LICENSE-MIT"], repr(missing))

    print("\nall checks passed" if failures == 0 else f"\n{failures} check(s) failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
