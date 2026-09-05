#!/usr/bin/env python3
"""Self-test for `check_host_types.py`.

    python3 tools/test_check_host_types.py

A checker nobody has watched fail is a checker nobody knows is connected. Each
case below builds a miniature workspace in a temporary directory and asserts
what the checker says about it — including the two ways it could fail *open*:
an empty tree, and a tree where the host exemption has swallowed everything.

The real repository is checked too, as the last case, so a rule that stopped
matching its own codebase is caught here rather than at review time.
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from check_host_types import check  # noqa: E402


def workspace(files: dict[str, str]) -> Path:
    """A temporary tree with the given files, plus both host crates so the
    exemption is never the thing under test by accident."""
    root = Path(tempfile.mkdtemp())
    defaults = {
        "crates/fieldglass-napi/src/lib.rs": "use napi::bindgen_prelude::Buffer;\n",
        "crates/fieldglass-wasm/src/lib.rs": "use wasm_bindgen::prelude::*;\n",
    }
    for name, text in {**defaults, **files}.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    return root


FAILURES: list[str] = []


def expect(label: str, problems: list[str], *, wanted: int, contains: str = "") -> None:
    """Assert a case's problem count, and optionally a phrase in one of them."""
    if len(problems) != wanted:
        FAILURES.append(f"{label}: expected {wanted} problem(s), got {len(problems)}: {problems}")
        return
    if contains and not any(contains in p for p in problems):
        FAILURES.append(f"{label}: no problem mentioned {contains!r}: {problems}")


def main() -> int:
    """Run every case and report."""
    # The control: an engine crate that names no host type passes. Without it,
    # a checker that reported everything would satisfy every case below.
    expect(
        "a clean engine crate",
        check(
            workspace(
                {
                    "crates/fieldglass-core/src/lib.rs": (
                        "pub fn open(bytes: &[u8]) -> Result<(), String> { let _ = bytes; Ok(()) }\n"
                    ),
                    "crates/fieldglass-core/Cargo.toml": '[dependencies]\nserde = "1"\n',
                }
            )
        ),
        wanted=0,
    )

    expect(
        "a napi path in an engine crate",
        check(
            workspace(
                {
                    "crates/fieldglass/src/lib.rs": (
                        "pub fn open() -> napi::Result<()> { Ok(()) }\n"
                    ),
                }
            )
        ),
        wanted=1,
        contains="a host crate path",
    )

    expect(
        "a napi attribute in an engine crate",
        check(workspace({"crates/fieldglass/src/lib.rs": "#[napi(object)]\npub struct A;\n"})),
        wanted=1,
        contains="a host attribute macro",
    )

    expect(
        "a JsValue in an engine crate",
        check(
            workspace(
                {"crates/fieldglass/src/lib.rs": "pub fn f() -> Result<JsValue, ()> { Err(()) }\n"}
            )
        ),
        wanted=1,
        contains="JsValue",
    )

    expect(
        "a host package in an engine manifest",
        check(
            workspace(
                {
                    "crates/fieldglass/src/lib.rs": "pub fn f() {}\n",
                    "crates/fieldglass/Cargo.toml": '[dependencies]\nwasm-bindgen = "0.2"\n',
                }
            )
        ),
        wanted=1,
        contains="host package",
    )

    # Prose about the hosts is not a violation: the API crate's own docs name
    # `napi` to explain why its types are shaped the way they are.
    expect(
        "a doc comment naming napi",
        check(
            workspace(
                {
                    "crates/fieldglass/src/lib.rs": (
                        "/// A DTO crosses as napi::Error would not.\n"
                        "// napi::Result is deliberately absent here.\n"
                        "pub fn f() {}\n"
                    )
                }
            )
        ),
        wanted=0,
    )

    # A name that merely contains a host root is not a host type.
    expect(
        "an identifier containing a host root",
        check(
            workspace(
                {
                    "crates/fieldglass/src/lib.rs": (
                        "pub fn to_napi_shape() {}\npub struct WasmBindgenLike;\n"
                    )
                }
            )
        ),
        wanted=0,
    )

    # A member outside `crates/` is walked. `tests/crate-independence` is one
    # in the real workspace, and a walk of `crates/` alone left it unchecked.
    expect(
        "a host path in a member outside crates/",
        check(
            workspace(
                {
                    "Cargo.toml": '[workspace]\nmembers = ["crates/fieldglass", "tests/probe"]\n',
                    "crates/fieldglass/src/lib.rs": "pub fn f() {}\n",
                    "tests/probe/src/main.rs": "fn main() -> napi::Result<()> { Ok(()) }\n",
                }
            )
        ),
        wanted=1,
        contains="a host crate path",
    )

    # A nested package's manifest is read too: each format crate has a `fuzz/`
    # one, and only the top-level manifest used to be checked.
    expect(
        "a host package in a nested manifest",
        check(
            workspace(
                {
                    "crates/fieldglass/src/lib.rs": "pub fn f() {}\n",
                    "crates/fieldglass/fuzz/Cargo.toml": '[dependencies]\nnapi = "3"\n',
                }
            )
        ),
        wanted=1,
        contains="host package",
    )

    # A manifest rename defeats both halves at once unless it is resolved.
    expect(
        "a renamed host dependency, and its use",
        check(
            workspace(
                {
                    "crates/fieldglass/Cargo.toml": (
                        '[dependencies]\nn = { package = "napi", version = "3" }\n'
                    ),
                    "crates/fieldglass/src/lib.rs": "pub fn f() -> n::Result<()> { Ok(()) }\n",
                }
            )
        ),
        wanted=2,
        contains="manifest rename",
    )

    # A `[features]` key that shares a dependency's name is not a dependency.
    expect(
        "a feature named after a host package",
        check(
            workspace(
                {
                    "crates/fieldglass/src/lib.rs": "pub fn f() {}\n",
                    "crates/fieldglass/Cargo.toml": '[features]\nnapi = []\n',
                }
            )
        ),
        wanted=0,
    )

    # A block comment must not shift the reported line number.
    problems = check(
        workspace(
            {
                "crates/fieldglass/src/lib.rs": (
                    "/* a\n   multi-line\n   comment */\npub fn f() -> napi::Result<()> { Ok(()) }\n"
                )
            }
        )
    )
    if len(problems) != 1 or ":4:" not in problems[0]:
        FAILURES.append(f"a block comment shifted the reported line: {problems}")

    # The two fail-open shapes. An empty tree must not pass.
    empty = Path(tempfile.mkdtemp())
    (empty / "crates").mkdir()
    expect(
        "an empty crates directory",
        check(empty),
        wanted=2,
        contains="pass vacuously",
    )

    expect(
        "a crate directory with no sources",
        check(workspace({"crates/fieldglass/Cargo.toml": "[package]\n"})),
        wanted=2,
        contains="the walk is broken",
    )

    # And the repository itself.
    repo = Path(__file__).resolve().parent.parent
    problems = check(repo)
    if problems:
        FAILURES.append("the repository itself: " + "; ".join(problems))

    if FAILURES:
        print("check_host_types self-test failures:", file=sys.stderr)
        for failure in FAILURES:
            print(f"  {failure}", file=sys.stderr)
        return 1
    print("check_host_types self-test: all cases pass")
    return 0


if __name__ == "__main__":
    sys.exit(main())
