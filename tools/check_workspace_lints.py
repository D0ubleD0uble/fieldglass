#!/usr/bin/env python3
"""Fail if a workspace member opts out of `[workspace.lints]` without saying so.

    python3 tools/check_workspace_lints.py

The lint standard lives in the root manifest's `[workspace.lints]` table, and a
member only picks it up by writing

    [lints]
    workspace = true

There is no diagnostic for omitting that. Cargo does not warn, `cargo clippy`
does not warn, and the member simply compiles under the default lint levels — a
gate that fails open. A new crate added to the workspace is exactly when it
happens, and exactly when nobody is looking at the other six manifests. So the
inheritance line is checked here rather than assumed.

The second half is the `missing_docs` burn-down. That lint is `warn` in the
workspace table, which the pre-commit hook turns into an error with
`-D warnings`, so the crates that still carry undocumented public items opt out
with a crate-root `#![allow(missing_docs)]`. Those opt-outs are debt, and debt
that nothing counts grows: a crate that finishes its burn-down and forgets to
delete the attribute is indistinguishable from one that never started, and a
crate that quietly adds the attribute to silence a new warning looks like it was
always there.

`DEBT` is therefore a ratchet, checked in both directions. A crate holding the
attribute must be listed, and a listed crate must still hold it — so finishing a
crate is a two-line diff (delete the attribute, delete the entry) and starting a
new opt-out cannot happen without editing this file, where a reviewer sees it.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Lint groups the root manifest must define. Named rather than counted so that
# dropping `[workspace.lints.clippy]` — the level the pre-commit hook and CI
# have always enforced — is a failure and not a silent relaxation.
REQUIRED_GROUPS = ("rust", "clippy")

# Crates that still carry `#![allow(missing_docs)]`, with the public-item count
# measured when the opt-out was written. The count is prose, not an assertion:
# what is checked is that the attribute is present, so that finishing a crate
# forces this entry to go with it.
DEBT = {
    "crates/fieldglass-core": 158,
    "crates/fieldglass-grib1": 110,
    "crates/fieldglass-grib2": 194,
    "crates/fieldglass-netcdf": 118,
}

# `#![allow(missing_docs)]`, tolerating extra lints in the same attribute and
# whitespace anywhere a Rust attribute allows it.
ALLOW_MISSING_DOCS = re.compile(r"#!\s*\[\s*allow\s*\([^)]*\bmissing_docs\b[^)]*\)\s*\]")


def inherits_workspace_lints(manifest: dict) -> bool:
    """Whether a member manifest opts into the workspace lint table."""
    return manifest.get("lints", {}).get("workspace") is True


def main() -> int:
    problems: list[str] = []

    root = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    lints = root.get("workspace", {}).get("lints", {})
    for group in REQUIRED_GROUPS:
        if group not in lints:
            problems.append(
                f"Cargo.toml: [workspace.lints.{group}] is missing — the standard "
                f"has to live in the manifest for a plain `cargo build`, "
                f"rust-analyzer and a crates.io reader to see it"
            )

    members = root.get("workspace", {}).get("members", [])
    if not members:
        problems.append("Cargo.toml: [workspace] members is empty or missing")

    for member in members:
        manifest_path = ROOT / member / "Cargo.toml"
        if not manifest_path.is_file():
            problems.append(f"{member}/Cargo.toml: workspace member has no manifest")
            continue
        manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
        if not inherits_workspace_lints(manifest):
            problems.append(
                f"{member}/Cargo.toml: missing `[lints]` / `workspace = true`, so "
                f"this crate compiles under the default lint levels while every "
                f"other member is held to [workspace.lints]"
            )

        lib = ROOT / member / "src" / "lib.rs"
        has_allow = bool(lib.is_file() and ALLOW_MISSING_DOCS.search(lib.read_text(encoding="utf-8")))
        listed = member in DEBT
        if has_allow and not listed:
            problems.append(
                f"{member}/src/lib.rs: `#![allow(missing_docs)]` is not recorded in "
                f"DEBT in tools/check_workspace_lints.py — add it there, with the "
                f"item count, so the opt-out is reviewed rather than absorbed"
            )
        if listed and not has_allow:
            problems.append(
                f"{member}: DEBT lists this crate but `src/lib.rs` no longer has "
                f"`#![allow(missing_docs)]` — the burn-down is done, so delete the "
                f"DEBT entry too"
            )

    for member in DEBT:
        if member not in members:
            problems.append(
                f"{member}: DEBT names a crate that is not a workspace member"
            )

    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
