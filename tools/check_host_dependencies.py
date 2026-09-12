#!/usr/bin/env python3
"""Fail when a host crate names a format crate in its manifest.

    python3 tools/check_host_dependencies.py

A host is a binding over `fieldglass` ([ADR-0006] decision 1): the napi addon and
the browser bundle. The umbrella is what decides the surface a host may use, so a
host that *also* depends on a decoder directly has two routes to the same reader
and the two hosts drift apart — which is exactly what #662 found. The browser
bundle took `fieldglass` and could open a NetCDF file only because the umbrella
reached the reader; the addon reached it twice over, and built its NetCDF path
against the reader rather than against `Session`.

**This is a ratchet, not a wall.** The transition is not finished — `fieldglass-napi`
still names both GRIB crates — so the remaining edges are listed in
`ALLOWED_DIRECT` with the reason each survives. The check fails in *both*
directions:

* a host gaining an edge that is not listed, which is the rule; and
* a listed edge that is no longer there, which means the transition moved and the
  list should record it.

The second half is what keeps the list honest. Without it an entry outlives the
dependency it excuses, and the gate quietly stops measuring anything.

[ADR-0006]: docs/decisions/0006-one-umbrella-crate-and-host-bindings-over-it.md
"""

from __future__ import annotations

import json
import subprocess
import sys

# The binding crates. A host is not a library: it exists to present `fieldglass`
# to one runtime, so it is the one place this rule is about.
HOSTS = {"fieldglass-napi", "fieldglass-wasm"}

# The decoders. A host should reach these only through `fieldglass`.
FORMAT_CRATES = {
    "fieldglass-grib1",
    "fieldglass-grib2",
    "fieldglass-netcdf",
    "fieldglass-zarr",
}

# Host -> the format crates it still names, and why. Shrinks as the transition
# proceeds; an entry removed from here must be removed from the manifest in the
# same commit, and the reverse.
ALLOWED_DIRECT: dict[str, dict[str, str]] = {
    "fieldglass-napi": {
        "fieldglass-grib1": (
            "the GRIB1 handle still holds a `Grib1Reader` and reads the parameter "
            "and centre tables directly; #662 moved the NetCDF half and this is "
            "the rest of it"
        ),
        "fieldglass-grib2": (
            "as GRIB1: the handle holds a `Grib2Reader` and reads the WMO tables, "
            "the product-definition section and the centre/discipline lookups"
        ),
    },
}


def direct_format_dependencies() -> dict[str, set[str]]:
    """Each host's direct dependencies that are format crates.

    From `cargo metadata --no-deps`, so it reads the manifests rather than the
    resolved graph: an edge that arrives transitively through `fieldglass` is
    exactly what this rule *wants*, and would be indistinguishable in the graph.
    """
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    ).stdout
    packages = json.loads(out)["packages"]
    found: dict[str, set[str]] = {}
    for package in packages:
        if package["name"] not in HOSTS:
            continue
        found[package["name"]] = {
            dep["name"] for dep in package["dependencies"] if dep["name"] in FORMAT_CRATES
        }
    return found


def check() -> list[str]:
    problems: list[str] = []
    found = direct_format_dependencies()

    missing_hosts = HOSTS - found.keys()
    if missing_hosts:
        # A host renamed away silently stops being checked, which is the same
        # fail-open as an empty list.
        problems.append(
            f"these host crates were not found by `cargo metadata`: "
            f"{', '.join(sorted(missing_hosts))} — was one renamed?"
        )

    for host in sorted(found):
        allowed = ALLOWED_DIRECT.get(host, {})
        for crate in sorted(found[host] - allowed.keys()):
            problems.append(
                f"{host} depends on {crate} directly. A host reaches a decoder "
                f"through `fieldglass` (ADR-0006 decision 1) — add what it needs "
                f"to the umbrella's surface, as `fieldglass::netcdf` does (#662). "
                f"If the edge really has to stay, add it to ALLOWED_DIRECT in "
                f"tools/check_host_dependencies.py with the reason."
            )
        for crate in sorted(allowed.keys() - found[host]):
            problems.append(
                f"{host} no longer depends on {crate}, so its ALLOWED_DIRECT entry "
                f"in tools/check_host_dependencies.py is stale — delete it, and the "
                f"transition is that much further along."
            )
    return problems


def main() -> int:
    problems = check()
    if problems:
        print("A host crate's dependencies are not what ADR-0006 decision 1 asks:")
        for problem in problems:
            print(f"  - {problem}")
        return 1
    remaining = sum(len(v) for v in ALLOWED_DIRECT.values())
    print(
        f"host dependencies OK — every host reaches its decoders through "
        f"`fieldglass`, with {remaining} edge(s) still to move."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
