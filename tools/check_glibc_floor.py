#!/usr/bin/env python3
"""Fail when a Linux native module needs a newer glibc than the supported floor.

    python3 tools/check_glibc_floor.py --floor 2.28 extension/bin/fieldglass.linux-x64-gnu.node

The release workflow runs this after each Linux build of the extension's
native module (#1866). The 0.6.1 module was built on Ubuntu 24.04 and needed
glibc 2.35, so it failed to load on RHEL 8 and 9, Ubuntu 20.04 and Debian 11,
and nothing noticed until a review read its symbol table. The floor is VS
Code's own, glibc 2.28.

It reads the module's version references (`objdump -p`), which is the list the
dynamic loader checks when it opens the file: one name there that the system's
glibc does not define and the load fails, whatever the symbols. That list also
holds names that are not release numbers, such as `GLIBC_PRIVATE` or the
`GLIBC_ABI_DT_RELR` marker that glibc 2.36 introduced. No glibc at the floor
satisfies those, so they fail too. Weak references (flag 0x02) count like any
other: the floor says what the module may ask for, not what it can survive
without.

`objdump -T` then names the symbols bound to each version over the floor, so
the error says what to look for (0.6.1: `hypot` at 2.35, `dlopen` at 2.34).
Host binutils reads an ELF file of any architecture, so the arm64 module is
checked on the x64 runner that built it.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

# One version reference line: hash, flags, index, name.
REFERENCE = re.compile(r"^\s+0x[0-9a-f]+\s+0x[0-9a-f]+\s+\d+\s+(\S+)\s*$")
# A numbered glibc release, `GLIBC_2.3.4`.
RELEASE = re.compile(r"GLIBC_(\d+(?:\.\d+)+)")


def version_key(name: str) -> tuple[int, ...] | None:
    """`GLIBC_2.3.4` as `(2, 3, 4)`, or `None` when the name is not a release."""
    m = RELEASE.fullmatch(name)
    return tuple(int(part) for part in m.group(1).split(".")) if m else None


def glibc_requirements(objdump_p: str) -> set[str]:
    """Every `GLIBC_*` name under `Version References:` in an `objdump -p` listing."""
    found: set[str] = set()
    inside = False
    for line in objdump_p.splitlines():
        if line.startswith("Version References:"):
            inside = True
            continue
        if inside:
            if line and not line[0].isspace():
                break
            m = REFERENCE.match(line)
            if m and m.group(1).startswith("GLIBC_"):
                found.add(m.group(1))
    return found


def problems(required: set[str], floor: str) -> list[str]:
    """Why these requirements do not load on glibc `floor`, oldest first, or `[]`."""
    if not required:
        return ["no GLIBC_ version references found; not a glibc module, or objdump changed"]
    limit = tuple(int(part) for part in floor.split("."))
    above, unknown = [], []
    for name in required:
        key = version_key(name)
        if key is None:
            unknown.append(name)
        elif key > limit:
            above.append((key, name))
    out = [f"{name} is newer than the glibc {floor} floor" for _, name in sorted(above)]
    out += [f"{name} is not a glibc release, so glibc {floor} cannot satisfy it" for name in sorted(unknown)]
    return out


def symbols_by_version(objdump_t: str) -> dict[str, list[str]]:
    """Undefined dynamic symbols in an `objdump -T` listing, by `GLIBC_*` version."""
    out: dict[str, list[str]] = {}
    for line in objdump_t.splitlines():
        fields = line.split()
        if "*UND*" not in fields or len(fields) < 2:
            continue
        version = fields[-2].strip("()")
        if version.startswith("GLIBC_"):
            out.setdefault(version, []).append(fields[-1])
    return out


def objdump(flag: str, path: Path) -> str:
    return subprocess.run(
        ["objdump", flag, str(path)], check=True, capture_output=True, encoding="utf-8"
    ).stdout


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--floor", required=True, help="oldest glibc to load on, e.g. 2.28")
    parser.add_argument("modules", nargs="+", type=Path)
    args = parser.parse_args(argv)
    if not re.fullmatch(r"\d+(\.\d+)+", args.floor):
        print(f"--floor {args.floor!r} is not a version such as 2.28", file=sys.stderr)
        return 2

    failed = False
    for module in args.modules:
        try:
            required = glibc_requirements(objdump("-p", module))
            symbols = symbols_by_version(objdump("-T", module))
        except (OSError, subprocess.CalledProcessError) as e:
            print(f"::error::{module}: objdump failed: {e}", file=sys.stderr)
            failed = True
            continue
        found = problems(required, args.floor)
        if found:
            failed = True
            for problem in found:
                version = problem.split()[0]
                names = ", ".join(sorted(set(symbols.get(version, [])))) or "no symbol"
                print(f"::error::{module}: {problem} ({names})", file=sys.stderr)
            continue
        newest = max(required, key=lambda n: version_key(n) or ())
        print(f"{module}: needs glibc {newest.removeprefix('GLIBC_')}, floor {args.floor}: OK")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
