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

from check_binaryen_pin import pins_from_text

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

    print("\nall checks passed" if failures == 0 else f"\n{failures} check(s) failed")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
