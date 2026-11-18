#!/usr/bin/env python3
"""Self-test for `check_glibc_floor.py`.

    python3 tools/test_check_glibc_floor.py

The two `objdump -p` listings below are copied from real modules: the
`fieldglass.linux-x64-gnu.node` shipped in the 0.6.1 `.vsix`, which needs
glibc 2.35 and fails to load on glibc 2.28 (#1866), and the same module built
with `napi build --use-napi-cross`, which loads there. The last case runs the
checker end to end on a real ELF file, so a change in how `objdump` prints its
listing fails here rather than letting every module through.
"""

from __future__ import annotations

import contextlib
import io
import os
import re
import shutil
import sys
from pathlib import Path

from check_glibc_floor import glibc_requirements, main, objdump, problems, version_key

SHIPPED_0_6_1 = """\
Dynamic Section:
  VERNEED              0x0000000000000af0
  VERNEEDNUM           0x0000000000000004

Version References:
  required from libgcc_s.so.1:
    0x0b792650 0x00 03 GCC_3.0
    0x0b792653 0x00 12 GCC_3.3
    0x09276060 0x00 18 GCC_4.2.0
  required from libm.so.6:
    0x09691a75 0x00 07 GLIBC_2.2.5
    0x06969189 0x00 09 GLIBC_2.29
    0x069691b5 0x00 08 GLIBC_2.35
  required from libc.so.6:
    0x09691a75 0x00 02 GLIBC_2.2.5
    0x0d696913 0x00 13 GLIBC_2.3
    0x09691974 0x00 17 GLIBC_2.3.4
    0x06969194 0x00 04 GLIBC_2.14
    0x06969198 0x02 16 GLIBC_2.18
    0x06969185 0x02 15 GLIBC_2.25
    0x06969188 0x02 14 GLIBC_2.28
    0x069691b0 0x02 11 GLIBC_2.30
    0x069691b3 0x00 10 GLIBC_2.33
    0x069691b4 0x00 06 GLIBC_2.34
  required from ld-linux-x86-64.so.2:
    0x0d696913 0x00 05 GLIBC_2.3

"""

NAPI_CROSS = """\
Version References:
  required from libdl.so.2:
    0x09691a75 0x00 08 GLIBC_2.2.5
  required from ld-linux-x86-64.so.2:
    0x0d696913 0x00 07 GLIBC_2.3
  required from libpthread.so.0:
    0x09691a75 0x00 05 GLIBC_2.2.5
  required from libc.so.6:
    0x06969194 0x00 12 GLIBC_2.14
    0x09691974 0x00 11 GLIBC_2.3.4
    0x0d696913 0x00 06 GLIBC_2.3
    0x09691a75 0x00 04 GLIBC_2.2.5
  required from libgcc_s.so.1:
    0x09276060 0x00 10 GCC_4.2.0
    0x0b792653 0x00 09 GCC_3.3
    0x0b792650 0x00 03 GCC_3.0
  required from libm.so.6:
    0x09691a75 0x00 02 GLIBC_2.2.5
"""

failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


def main_test() -> int:
    shipped = glibc_requirements(SHIPPED_0_6_1)
    check(
        "reads every GLIBC_ version the 0.6.1 module needs, and no GCC_ one",
        shipped
        == {
            "GLIBC_2.2.5", "GLIBC_2.3", "GLIBC_2.3.4", "GLIBC_2.14", "GLIBC_2.18",
            "GLIBC_2.25", "GLIBC_2.28", "GLIBC_2.29", "GLIBC_2.30", "GLIBC_2.33",
            "GLIBC_2.34", "GLIBC_2.35",
        },
        repr(sorted(shipped)),
    )
    found = problems(shipped, "2.28")
    check(
        "the 0.6.1 module fails the 2.28 floor on each newer version",
        [p.split()[0] for p in found]
        == ["GLIBC_2.29", "GLIBC_2.30", "GLIBC_2.33", "GLIBC_2.34", "GLIBC_2.35"],
        repr(found),
    )
    check("the 0.6.1 module passes a 2.35 floor", problems(shipped, "2.35") == [])

    rebuilt = glibc_requirements(NAPI_CROSS)
    check(
        "reads the napi-cross module's versions across libdl, libpthread, libc and libm",
        rebuilt == {"GLIBC_2.2.5", "GLIBC_2.3", "GLIBC_2.3.4", "GLIBC_2.14"},
        repr(sorted(rebuilt)),
    )
    check("the napi-cross module passes the 2.28 floor", problems(rebuilt, "2.28") == [])

    # Compared as numbers: as strings, 2.3.4 sorts after 2.14 and 2.9 after 2.28.
    check("2.3.4 is older than 2.14", version_key("GLIBC_2.3.4") < version_key("GLIBC_2.14"))
    check("2.9 is older than 2.28", problems({"GLIBC_2.9"}, "2.28") == [])
    check("the floor itself passes", problems({"GLIBC_2.28"}, "2.28") == [])

    # Versions no glibc 2.28 defines. GLIBC_ABI_DT_RELR first appeared in 2.36.
    for name in ("GLIBC_PRIVATE", "GLIBC_ABI_DT_RELR"):
        check(f"{name} fails the floor", len(problems({name}, "2.28")) == 1)

    # A listing with no glibc versions at all is a wrong file or a changed
    # objdump format. Either way the check must not pass it.
    check("no Version References fails", len(problems(glibc_requirements(""), "2.28")) == 1)
    check(
        "a section in another objdump layout is not read as empty-and-fine",
        glibc_requirements("Version References:\n  libc.so.6 GLIBC_2.35\n") == set(),
    )

    # End to end on a real ELF file: this interpreter. Which glibc versions it
    # needs depends on the host (the oldest is 2.2.5 on x64 and 2.17 on arm64),
    # so this asserts only that some release is read, that none is 99 or
    # newer, and that every one is newer than 2.0, which predates them all.
    if shutil.which("objdump") is None:
        check("objdump is installed", False, "the end-to-end case needs binutils")
    else:
        real = os.path.realpath(sys.executable)
        found = glibc_requirements(objdump("-p", Path(real)))
        check(
            "reads GLIBC_ releases from a real ELF file",
            any(version_key(name) for name in found),
            repr(sorted(found)),
        )
        check("a real ELF file passes a 99.0 floor", main(["--floor", "99.0", real]) == 0)
        err = io.StringIO()
        with contextlib.redirect_stderr(err):
            rc = main(["--floor", "2.0", real])
        check(
            "a real ELF file fails a 2.0 floor, naming its versions",
            rc == 1 and re.search(r"GLIBC_[\d.]+ is newer than the glibc 2\.0 floor", err.getvalue()),
            err.getvalue()[:200],
        )
        with contextlib.redirect_stderr(io.StringIO()):
            rc = main(["--floor", "2.28", real + ".missing"])
        check("a missing file fails", rc == 1)

    if failures:
        print(f"{failures} check(s) failed")
        return 1
    print("check_glibc_floor self-test passed")
    return 0


if __name__ == "__main__":
    sys.exit(main_test())
