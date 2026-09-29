#!/usr/bin/env python3
"""Fail when a host crate names any workspace crate other than the umbrella.

    python3 tools/check_host_dependencies.py

A host is a binding over `fieldglass` ([ADR-0006] decision 1): the napi addon and
the browser bundle. The umbrella is what decides the surface a host may use, so a
host that *also* depends on a crate below it has two routes to the same code and
the two hosts drift apart — which is exactly what #662 found. The browser bundle
took `fieldglass` and could open a NetCDF file only because the umbrella reached
the reader; the addon reached it twice over, and built its NetCDF path against
the reader rather than against `Session`.

**Every workspace crate counts, not only the decoders.** Until #574 this check
listed the four format crates, and `fieldglass-core` sat outside the list: the
addon named it directly for its geometry types, format detection and unit
typesetting, 24 uses the browser bundle reached through the umbrella instead.
#574 read "napi depends on `fieldglass` only" strictly, so the rule is now the
manifest's whole workspace set minus the umbrella — a crate added to the
workspace later is covered the day it lands, rather than when someone remembers
to add it to a list here. What a host needs from below the umbrella, the
umbrella re-exports as part of its host surface.

Dev-dependencies and build-dependencies count alike: a test that reaches past
the umbrella proves something about a route the shipped host does not take.

**This is a ratchet, not a wall.** An exception is argued for in
`ALLOWED_DIRECT`, with its reason, and the check fails in *both* directions:

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

# The one workspace crate a host may name.
UMBRELLA = "fieldglass"

# Host -> the workspace crates it still names besides the umbrella, and why.
# Empty since #574: `fieldglass-napi` dropped `fieldglass-core`, the last edge
# (#726 had removed the format crates).
#
# Kept as a structure rather than deleted, because an entry is how an exception
# would have to be argued for — added here with its reason, in the same commit
# as the manifest edge, where a reviewer sees both. The check still fails for an
# unlisted edge, so an empty table is the strictest setting, not a disabled one.
ALLOWED_DIRECT: dict[str, dict[str, str]] = {}


def cargo_metadata_packages() -> list[dict]:
    """The workspace members, from `cargo metadata --no-deps`.

    `--no-deps` so this reads the manifests rather than the resolved graph: an
    edge that arrives transitively through `fieldglass` is exactly what this
    rule *wants*, and would be indistinguishable in the graph. It also makes
    `packages` the workspace set, which is what the rule is stated over.
    """
    out = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    ).stdout
    return json.loads(out)["packages"]


def direct_workspace_dependencies(packages: list[dict]) -> dict[str, set[str]]:
    """Each host's direct dependencies that are workspace crates, bar the umbrella.

    Every dependency kind — normal, dev and build — because the manifest lists
    them all under `dependencies` with a `kind` beside each.
    """
    workspace = {package["name"] for package in packages}
    found: dict[str, set[str]] = {}
    for package in packages:
        if package["name"] not in HOSTS:
            continue
        found[package["name"]] = {
            dep["name"]
            for dep in package["dependencies"]
            if dep["name"] in workspace and dep["name"] != UMBRELLA
        }
    return found


def check(packages: list[dict] | None = None) -> list[str]:
    problems: list[str] = []
    if packages is None:
        packages = cargo_metadata_packages()
    found = direct_workspace_dependencies(packages)

    missing_hosts = HOSTS - found.keys()
    if missing_hosts:
        # A host renamed away silently stops being checked, which is the same
        # fail-open as an empty list.
        problems.append(
            f"these host crates were not found by `cargo metadata`: "
            f"{', '.join(sorted(missing_hosts))} — was one renamed?"
        )
    if UMBRELLA not in {package["name"] for package in packages}:
        # Likewise the umbrella: renamed, every host edge to it would start
        # counting as a violation — or, worse, a new name would be exempt.
        problems.append(
            f"the umbrella crate `{UMBRELLA}` was not found by `cargo metadata` — "
            f"was it renamed?"
        )

    for host in sorted(found):
        allowed = ALLOWED_DIRECT.get(host, {})
        for crate in sorted(found[host] - allowed.keys()):
            problems.append(
                f"{host} depends on {crate} directly. A host names no workspace "
                f"crate but `{UMBRELLA}` (ADR-0006 decision 1, #574) — have the "
                f"umbrella re-export what the host needs as part of its host "
                f"surface, as `fieldglass::netcdf` does (#662) and the geometry "
                f"re-exports do (#574). If the edge really has to stay, add it to "
                f"ALLOWED_DIRECT in tools/check_host_dependencies.py with the reason."
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
        f"host dependencies OK — every host names `{UMBRELLA}` and no other "
        f"workspace crate, with {remaining} edge(s) still to move."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
