#!/usr/bin/env python3
"""Self-test for `check_binaryen_pin.py`.

    python3 tools/test_check_binaryen_pin.py

A drift guard that cannot fail is worse than none: it reports "OK" forever and
the thing it was supposed to watch drifts anyway. These assert on the parsing
function's output directly, not only on an exit status, so a regex that silently
matched nothing would be caught rather than read as agreement.
"""

from __future__ import annotations

import sys

from check_binaryen_pin import (
    pins_from_text,
    release_toolchain_problems,
    toolchain_inputs,
    toolchain_problems,
    toolchain_values,
    workflow_env_keys,
)

WORKFLOW = """\
name: CI
jobs:
  wasm:
    env:
      BINARYEN_VERSION: version_132
      BINARYEN_SHA256: 195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572
    steps:
      - run: echo hi
"""

failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


def main() -> int:
    pins = pins_from_text(WORKFLOW)
    check(
        "reads the version",
        pins.get("BINARYEN_VERSION") == "version_132",
        repr(pins.get("BINARYEN_VERSION")),
    )
    check(
        "reads the checksum",
        pins.get("BINARYEN_SHA256", "").startswith("195ddc94"),
        repr(pins.get("BINARYEN_SHA256")),
    )

    # The failure that matters: a workflow that sets neither must come back
    # empty rather than matching something else and reading as agreement.
    check("a workflow with no pin yields nothing", pins_from_text("jobs: {}") == {})

    # A commented-out pin is not a pin. `#  BINARYEN_VERSION: x` would match a
    # careless regex and make a removed pin look present.
    commented = pins_from_text("      # BINARYEN_VERSION: version_999\n")
    check("a commented pin is not read", commented == {}, repr(commented))

    # Two different values in one file is its own error, not a silent first-wins.
    try:
        pins_from_text(WORKFLOW + "      BINARYEN_VERSION: version_999\n")
        check("two values in one file raise", False, "it returned instead")
    except ValueError:
        check("two values in one file raise", True)

    # The same value twice is fine — a workflow may install binaryen in two jobs.
    repeated = pins_from_text(WORKFLOW + "      BINARYEN_VERSION: version_132\n")
    check("the same value twice is fine", repeated["BINARYEN_VERSION"] == "version_132")

    # The Rust the size gate measures must be the one the release ships (#1811).
    def wf(line: str) -> str:
        return f"jobs:\n  wasm:\n    env:\n{line}    steps: []\n"

    ok = "      RUSTUP_TOOLCHAIN: 1.99.1\n"
    check(
        "equal toolchain pins pass",
        toolchain_problems({"ci.yml": wf(ok), "release.yml": wf(ok)}) == [],
    )
    differ = toolchain_problems(
        {"ci.yml": wf(ok), "release.yml": wf("      RUSTUP_TOOLCHAIN: 1.99.2\n")}
    )
    check("differing toolchain pins fail", any("disagrees" in p for p in differ), repr(differ))
    missing = toolchain_problems({"ci.yml": wf(ok), "release.yml": wf("")})
    check("a missing toolchain pin fails", any("not set" in p for p in missing), repr(missing))
    both_missing = toolchain_problems({"ci.yml": wf(""), "release.yml": wf("")})
    check("both missing fails rather than reading as equal", len(both_missing) == 2, repr(both_missing))
    st = "      RUSTUP_TOOLCHAIN: stable\n"
    stable = toolchain_problems({"ci.yml": wf(st), "release.yml": wf(st)})
    check("`stable` fails even when equal", len(stable) == 2, repr(stable))
    check(
        "quotes and trailing comments are tolerated",
        toolchain_values(
            "      RUSTUP_TOOLCHAIN: \"1.99.1\"  # pin\n      RUSTUP_TOOLCHAIN: '1.99.1'\n"
        )
        == ["1.99.1", "1.99.1"],
    )
    check(
        "a commented toolchain pin is not read",
        toolchain_values("      # RUSTUP_TOOLCHAIN: 1.99.1\n") == [],
    )
    two = toolchain_problems(
        {"ci.yml": wf(ok + ok.replace("1.99.1", "1.98.0")), "release.yml": wf(ok)}
    )
    check("two values in one file fail", any("more than one" in p for p in two), repr(two))

    # Every Rust job in release.yml builds with the pin (#1877).
    setup = (
        "      - uses: dtolnay/rust-toolchain@683b588c # master\n"
        "        with:\n"
        "          toolchain: ${{ env.RUSTUP_TOOLCHAIN }}\n"
    )

    def release(env: str, *setups: str) -> str:
        jobs = "".join(f"  job{i}:\n    steps:\n{s}" for i, s in enumerate(setups))
        return f"name: Release\non: push\n{env}jobs:\n{jobs}"

    top = "env:\n  # The pin.\n  RUSTUP_TOOLCHAIN: 1.99.1\n"
    check(
        "a workflow-wide pin read by every setup passes",
        release_toolchain_problems(release(top, setup, setup)) == [],
        repr(release_toolchain_problems(release(top, setup, setup))),
    )
    check("the workflow-level env is read", workflow_env_keys(release(top, setup)) == ["RUSTUP_TOOLCHAIN"])
    back_to_stable = release_toolchain_problems(
        release(top, setup, setup.replace("${{ env.RUSTUP_TOOLCHAIN }}", "stable"))
    )
    check(
        "one setup back on `stable` fails",
        len(back_to_stable) == 1 and "'stable'" in back_to_stable[0],
        repr(back_to_stable),
    )
    literal = release_toolchain_problems(release(top, setup.replace("${{ env.RUSTUP_TOOLCHAIN }}", "1.99.1")))
    check("a literal version in a setup fails: it can drift from the pin", len(literal) == 1, repr(literal))
    job_only = (
        "name: Release\non: push\njobs:\n  wasm:\n    env:\n      RUSTUP_TOOLCHAIN: 1.99.1\n"
        f"    steps:\n{setup}"
    )
    check("a job-level env is not the workflow-level one", workflow_env_keys(job_only) == [])
    no_top = release_toolchain_problems(job_only)
    check("a pin set on one job only fails", any("workflow-level" in p for p in no_top), repr(no_top))
    commented_top = release_toolchain_problems(release("env:\n  # RUSTUP_TOOLCHAIN: 1.99.1\n  OTHER: x\n", setup))
    check("a commented workflow-wide pin is not read", any("workflow-level" in p for p in commented_top))
    no_setup = release_toolchain_problems(release(top))
    check("no Rust setup at all fails rather than passing vacuously", no_setup != [], repr(no_setup))
    check(
        "quoted toolchain inputs are unquoted",
        toolchain_inputs("          toolchain: 'stable'  # day's\n          toolchain: \"1.99.1\"\n")
        == ["stable", "1.99.1"],
    )
    check(
        "the action's name is not an input",
        toolchain_inputs("      - uses: dtolnay/rust-toolchain@abc # master\n") == [],
    )

    print("\nall checks passed" if failures == 0 else f"\n{failures} check(s) failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
