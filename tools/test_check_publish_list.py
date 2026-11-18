#!/usr/bin/env python3
"""Self-test for `check_publish_list.py`.

    python3 tools/test_check_publish_list.py

Runs the checker's functions on hand-built `cargo metadata` and on edited
copies of the real `release.yml` and `RELEASING.md`: a dropped crate, an extra
one, a swapped pair, a duplicate and a new publishable member must each fail
with the crate named, and the files as committed must pass.
"""

from __future__ import annotations

import sys
from pathlib import Path

from check_publish_list import (
    ListError,
    problems,
    publishable_crates,
    releasing_md_list,
    release_yml_list,
)

REPO = Path(__file__).resolve().parent.parent

failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


def dep(name: str, kind: str | None = None, req: str = "=0.1.0") -> dict:
    return {"name": name, "kind": kind, "req": req}


def package(name: str, deps: list[dict], publish: list[str] | None = None) -> dict:
    return {
        "id": f"path+file:///ws/crates/{name}#0.1.0",
        "name": name,
        "publish": publish,
        "dependencies": deps,
    }


def metadata(*packages: dict, members: list[str] | None = None) -> dict:
    names = members if members is not None else [p["name"] for p in packages]
    return {
        "packages": list(packages),
        "workspace_members": [f"path+file:///ws/crates/{n}#0.1.0" for n in names],
    }


def mentions(found: list[str], *words: str) -> bool:
    return any(all(w in line for w in words) for line in found)


def test_publishable_crates() -> None:
    print("publishable_crates")
    meta = metadata(
        package("base", [dep("serde"), dep("base", "dev", "*")]),
        package("mid", [dep("base"), dep("helper", "build")]),
        package("helper", []),
        package("top", [dep("mid"), dep("base", "dev"), dep("tests-only", "dev", "*")]),
        package("tests-only", []),
        package("binding", [dep("top")], publish=[]),
        package("registry-only", [], publish=["crates-io"]),
        package("outsider", []),
        members=["base", "mid", "helper", "top", "tests-only", "binding", "registry-only"],
    )
    crates = publishable_crates(meta)
    check("`publish = false` is out", "binding" not in crates, repr(sorted(crates)))
    check("a registry list is publishable", "registry-only" in crates, repr(sorted(crates)))
    check("a non-member is out", "outsider" not in crates, repr(sorted(crates)))
    check("a non-workspace dependency is no edge", crates["base"] == set(), repr(crates["base"]))
    check("a normal and a build dependency are edges", crates["mid"] == {"base", "helper"}, repr(crates["mid"]))
    check(
        "a versioned dev-dependency is an edge, a path-only one is not",
        crates["top"] == {"mid", "base"},
        repr(crates["top"]),
    )


def test_problems() -> None:
    print("problems")
    crates = {"a": set(), "b": {"a"}, "c": {"a", "b"}}
    check("the dependency order passes", problems(["a", "b", "c"], crates, "here") == [])

    found = problems(["a", "c"], crates, "here")
    check("a dropped crate is named", mentions(found, "here", "b", "missing"), repr(found))

    found = problems(["a", "b", "c", "d"], crates, "here")
    check("an extra crate is named", mentions(found, "here", "d", "not a publishable"), repr(found))

    found = problems(["b", "a", "c"], crates, "here")
    check("a swapped pair names both crates", mentions(found, "here", "b", "before", "a"), repr(found))

    found = problems(["a", "b", "b", "c"], crates, "here")
    check("a duplicate is named", mentions(found, "here", "b", "twice"), repr(found))

    grown = {**crates, "d": {"c"}}
    found = problems(["a", "b", "c"], grown, "here")
    check("a new publishable member is missing", mentions(found, "here", "d", "missing"), repr(found))


def test_release_yml() -> None:
    print("release_yml_list")
    text = (REPO / ".github/workflows/release.yml").read_text(encoding="utf-8")
    listed = release_yml_list(text)
    check("the committed loop parses to eight crates", len(listed) == 8, repr(listed))
    check("the committed loop starts with aec", listed[:2] == ["fieldglass-aec", "fieldglass-core"], repr(listed))
    check("the umbrella is last", listed[-1:] == ["fieldglass"], repr(listed))

    dropped = text.replace(" fieldglass-grib1 ", " ", 1)
    check("a dropped crate parses short", "fieldglass-grib1" not in release_yml_list(dropped))

    # A `for crate in` loop elsewhere in the file is not the publish loop.
    decoy = text.replace("  publish-crates:", "  other:\n    run: for crate in decoy; do :; done\n  publish-crates:", 1)
    check("only the publish-crates job is read", "decoy" not in release_yml_list(decoy))

    try:
        release_yml_list(text.replace("for crate in", "for pkg in"))
        check("a missing loop is an error", False, "no ListError")
    except ListError:
        check("a missing loop is an error", True)
    try:
        release_yml_list(text.replace("  publish-crates:", "  publish-all:"))
        check("a missing job is an error", False, "no ListError")
    except ListError:
        check("a missing job is an error", True)


def test_releasing_md() -> None:
    print("releasing_md_list")
    text = (REPO / "RELEASING.md").read_text(encoding="utf-8")
    listed = releasing_md_list(text)
    check("the committed sentence parses to eight crates", len(listed) == 8, repr(listed))
    check("a short `-x` name expands", "fieldglass-grib1" in listed, repr(listed))
    check("the umbrella is last", listed[-1:] == ["fieldglass"], repr(listed))

    swapped = text.replace("`fieldglass-aec`, `fieldglass-core`", "`fieldglass-core`, `fieldglass-aec`", 1)
    check("a swapped pair parses swapped", releasing_md_list(swapped)[:2] == ["fieldglass-core", "fieldglass-aec"])

    try:
        releasing_md_list(text.replace("dependency order:", "this order:"))
        check("a missing sentence is an error", False, "no ListError")
    except ListError:
        check("a missing sentence is an error", True)


def test_committed_tree() -> None:
    """The real files against a crate graph matching the real workspace."""
    print("committed tree")
    core = {"fieldglass-core"}
    crates = {
        "fieldglass-aec": set(),
        "fieldglass-core": set(),
        "fieldglass-grib1": core,
        "fieldglass-grib2": core | {"fieldglass-aec", "fieldglass-grib1"},
        "fieldglass-netcdf": core | {"fieldglass-aec"},
        "fieldglass-zarr": core,
        "fieldglass-fetchplan": core | {"fieldglass-zarr"},
        "fieldglass": core | {"fieldglass-grib1", "fieldglass-grib2", "fieldglass-netcdf", "fieldglass-zarr", "fieldglass-fetchplan"},
    }
    yml = release_yml_list((REPO / ".github/workflows/release.yml").read_text(encoding="utf-8"))
    md = releasing_md_list((REPO / "RELEASING.md").read_text(encoding="utf-8"))
    check("release.yml passes", problems(yml, crates, "release.yml") == [], repr(problems(yml, crates, "release.yml")))
    check("RELEASING.md passes", problems(md, crates, "RELEASING.md") == [], repr(problems(md, crates, "RELEASING.md")))


def main() -> int:
    test_publishable_crates()
    test_problems()
    test_release_yml()
    test_releasing_md()
    test_committed_tree()
    if failures:
        print(f"{failures} check(s) failed", file=sys.stderr)
        return 1
    print("all checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
