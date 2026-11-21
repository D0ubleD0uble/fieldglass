#!/usr/bin/env python3
"""Self-test for `check_macos_floor.py`.

    python3 tools/test_check_macos_floor.py

Every Mach-O file here is built by hand below: a header and a few load
commands, which is all the checker looks at. Building
them keeps the test independent of any Apple toolchain, so it runs on the
Linux pre-push hook and on the macOS runner alike. The release workflow then
runs the checker on the real modules (#1904).
"""

from __future__ import annotations

import contextlib
import io
import struct
import sys
import tempfile
from pathlib import Path

from check_macos_floor import FLOORS, Slice, main, problems, read_slices, version_text

MH_MAGIC_64 = 0xFEEDFACF
MH_MAGIC = 0xFEEDFACE
MH_DYLIB = 6
FAT_MAGIC = 0xCAFEBABE
FAT_MAGIC_64 = 0xCAFEBABF
CPU_X86_64 = 0x01000007
CPU_ARM64 = 0x0100000C
LC_SEGMENT = 0x1
LC_SEGMENT_64 = 0x19
LC_VERSION_MIN_MACOSX = 0x24
LC_VERSION_MIN_IPHONEOS = 0x25
LC_BUILD_VERSION = 0x32
PLATFORM_MACOS = 1
PLATFORM_IOS = 2


def encode(version: str) -> int:
    """`10.12` as Mach-O's packed `xxxx.yy.zz`."""
    parts = [int(p) for p in version.split(".")] + [0, 0]
    return (parts[0] << 16) | (parts[1] << 8) | parts[2]


def build_version(minos: str, platform: int = PLATFORM_MACOS) -> bytes:
    return struct.pack("<6I", LC_BUILD_VERSION, 24, platform, encode(minos), encode("15.0"), 0)


def version_min(minos: str) -> bytes:
    return struct.pack("<4I", LC_VERSION_MIN_MACOSX, 16, encode(minos), encode("10.15"))


def segment() -> bytes:
    """An `LC_SEGMENT_64` with no sections, so the reader has to skip a command."""
    return struct.pack("<2I16s4Q4I", LC_SEGMENT_64, 72, b"__TEXT", 0, 0, 0, 0, 5, 5, 0, 0)


def thin(cpu: int, *commands: bytes, bits: int = 64, big_endian: bool = False) -> bytes:
    order = ">" if big_endian else "<"
    body = b"".join(commands)
    if bits == 64:
        head = struct.pack(order + "8I", MH_MAGIC_64, cpu, 0, MH_DYLIB, len(commands), len(body), 0, 0)
    else:
        head = struct.pack(order + "7I", MH_MAGIC, cpu, 0, MH_DYLIB, len(commands), len(body), 0)
    if big_endian:
        # Load commands are in the file's byte order too.
        body = b"".join(
            struct.pack(">%dI" % (len(c) // 4), *struct.unpack("<%dI" % (len(c) // 4), c))
            for c in commands
        )
    return head + body


def universal(*slices: tuple[int, bytes], wide: bool = False) -> bytes:
    """A fat file holding each `(cputype, thin file)` at a 4 KiB boundary."""
    offset = 4096
    table, data = [], b""
    for cpu, body in slices:
        if wide:
            table.append(struct.pack(">2I2Q2I", cpu, 0, offset + len(data), len(body), 12, 0))
        else:
            table.append(struct.pack(">5I", cpu, 0, offset + len(data), len(body), 12))
        data += body + b"\0" * (-len(body) % 4096)
    head = struct.pack(">2I", FAT_MAGIC_64 if wide else FAT_MAGIC, len(slices)) + b"".join(table)
    return head + b"\0" * (offset - len(head)) + data


failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


def found(data: bytes) -> list[str]:
    """What the checker says is wrong with `data`: a refused read, or each problem."""
    try:
        return problems(read_slices(data))
    except ValueError as e:
        return [str(e)]


def main_test() -> int:
    check(
        "the floors are 10.15 for x86_64 and 11.0 for arm64",
        {cpu: version_text(v) for cpu, v in FLOORS.items()} == {CPU_X86_64: "10.15", CPU_ARM64: "11.0"},
        repr(FLOORS),
    )

    # The four cases the issue names.
    x64_dry_run = thin(CPU_X86_64, segment(), build_version("10.12"))
    check(
        "an x86_64 module below the floor is read as 10.12 and passes",
        read_slices(x64_dry_run) == [Slice(CPU_X86_64, [(PLATFORM_MACOS, encode("10.12"))])]
        and found(x64_dry_run) == [],
        repr(read_slices(x64_dry_run)),
    )
    above = found(thin(CPU_X86_64, segment(), build_version("12.0")))
    check(
        "an x86_64 module above the floor fails, naming both versions",
        len(above) == 1 and "12.0" in above[0] and "10.15" in above[0],
        repr(above),
    )
    missing = found(thin(CPU_X86_64, segment()))
    check("a module with no version command fails", len(missing) == 1, repr(missing))
    mixed = found(
        universal(
            (CPU_X86_64, thin(CPU_X86_64, build_version("10.12"))),
            (CPU_ARM64, thin(CPU_ARM64, build_version("13.0"))),
        )
    )
    check(
        "a universal binary with one slice above the floor fails on that slice",
        len(mixed) == 1 and "arm64" in mixed[0] and "13.0" in mixed[0],
        repr(mixed),
    )

    # The boundaries, which a `>` turned `>=` or `<` would move.
    check("an x86_64 module at the floor passes", found(thin(CPU_X86_64, build_version("10.15"))) == [])
    check("an arm64 module at the floor passes", found(thin(CPU_ARM64, build_version("11.0"))) == [])
    check("10.15.1 is above the 10.15 floor", len(found(thin(CPU_X86_64, build_version("10.15.1")))) == 1)
    check("an arm64 module at 11.1 fails", len(found(thin(CPU_ARM64, build_version("11.1")))) == 1)
    # The floor is per architecture: 10.15 passes on x86_64 and fails on arm64.
    check("an arm64 module at 10.15 passes", found(thin(CPU_ARM64, build_version("10.15"))) == [])
    check("an x86_64 module at 11.0 fails", len(found(thin(CPU_X86_64, build_version("11.0")))) == 1)

    # Every version command counts, not only the first.
    two = found(thin(CPU_X86_64, build_version("10.12"), version_min("10.16")))
    check("a second version command above the floor fails", len(two) == 1 and "10.16" in two[0], repr(two))
    check(
        "LC_VERSION_MIN_MACOSX is read like LC_BUILD_VERSION",
        found(thin(CPU_X86_64, version_min("10.9"))) == []
        and len(found(thin(CPU_X86_64, version_min("10.16")))) == 1,
    )

    # Files that are not a macOS module for a known architecture.
    ios = found(thin(CPU_ARM64, build_version("11.0", platform=PLATFORM_IOS)))
    check("a build version for another platform fails", len(ios) == 1 and "platform 2" in ios[0], repr(ios))
    # The iOS form of LC_VERSION_MIN_MACOSX, which is not read as a macOS one.
    ios_min = struct.pack("<4I", LC_VERSION_MIN_IPHONEOS, 16, encode("9.0"), encode("14.0"))
    check("a module with only an iOS version command fails", len(found(thin(CPU_ARM64, ios_min))) == 1)
    lying = found(universal((CPU_ARM64, thin(CPU_X86_64, build_version("10.12")))))
    check(
        "a fat entry whose CPU type disagrees with its slice's header fails",
        len(lying) == 1 and "header says x86_64" in lying[0],
        repr(lying),
    )
    check("an unknown architecture fails", len(found(thin(0x12, build_version("10.12")))) == 1)
    # Read both ways, so a header misread as having no version command cannot pass.
    for name, layout in (("a 32-bit header", {"bits": 32}), ("a big-endian header", {"big_endian": True})):
        check(
            f"{name} is read",
            found(thin(CPU_X86_64, build_version("10.12"), **layout)) == []
            and len(found(thin(CPU_X86_64, build_version("12.0"), **layout))) == 1,
        )
    check(
        "a 64-bit fat table is read",
        found(universal((CPU_X86_64, thin(CPU_X86_64, build_version("10.12"))), wide=True)) == []
        and len(found(universal((CPU_X86_64, thin(CPU_X86_64, build_version("12.0"))), wide=True))) == 1,
    )
    check(
        "a universal binary with every slice below the floor passes",
        found(
            universal(
                (CPU_X86_64, thin(CPU_X86_64, build_version("10.12"))),
                (CPU_ARM64, thin(CPU_ARM64, build_version("11.0"))),
            )
        )
        == [],
    )
    check(
        "a two-slice 64-bit fat table is read slice by slice",
        found(
            universal(
                (CPU_X86_64, thin(CPU_X86_64, build_version("10.12"))),
                (CPU_ARM64, thin(CPU_ARM64, build_version("11.0"))),
                wide=True,
            )
        )
        == []
        and [p.split()[0] for p in found(
            universal(
                (CPU_X86_64, thin(CPU_X86_64, build_version("10.12"))),
                (CPU_ARM64, thin(CPU_ARM64, build_version("13.0"))),
                wide=True,
            )
        )]
        == ["arm64"],
    )
    # A version command too short for its payload. Read anyway, this one takes
    # the next command's header for its platform and minos: LC_SEGMENT (1, so
    # macOS) of size 56, which is macOS 0.0.56 and would pass.
    short_build = struct.pack("<2I", LC_BUILD_VERSION, 8)
    next_segment = struct.pack("<2I", LC_SEGMENT, 56) + b"\0" * 48
    short_min = struct.pack("<2I", LC_VERSION_MIN_MACOSX, 8)
    # A command of size 0 after a good version command. Skipped as if it were
    # empty, it would let the file through.
    empty = struct.pack("<2I", LC_SEGMENT_64, 0)
    for name, data in (
        ("an LC_BUILD_VERSION shorter than 24 bytes", thin(CPU_X86_64, short_build, next_segment)),
        ("an LC_VERSION_MIN_MACOSX shorter than 16 bytes", thin(CPU_X86_64, short_min, next_segment)),
        ("a load command of size 0", thin(CPU_X86_64, build_version("10.12"), empty)),
        ("an ELF file", b"\x7fELF" + b"\0" * 60),
        ("an empty file", b""),
        ("a truncated header", thin(CPU_X86_64, build_version("10.12"))[:20]),
        ("a load command cut short", thin(CPU_X86_64, build_version("10.12"))[:-4]),
        ("a fat file with no slices", struct.pack(">2I", FAT_MAGIC, 0)),
        ("a fat slice past the end", universal((CPU_X86_64, thin(CPU_X86_64, build_version("10.12"))))[:4100]),
    ):
        check(f"{name} fails", len(found(data)) >= 1, repr(found(data)))

    # End to end through `main`, on files.
    with tempfile.TemporaryDirectory() as tmp:
        good = Path(tmp, "good.node")
        good.write_bytes(x64_dry_run)
        bad = Path(tmp, "bad.node")
        bad.write_bytes(thin(CPU_ARM64, build_version("14.0")))
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = main([str(good)])
        check(
            "main passes a good module and prints what it read",
            rc == 0 and "x86_64 needs macOS 10.12, floor 10.15: OK" in out.getvalue(),
            out.getvalue(),
        )
        err = io.StringIO()
        with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(err):
            rc = main([str(good), str(bad)])
        check(
            "main fails when any module is above the floor",
            rc == 1 and "::error::" in err.getvalue() and "14.0" in err.getvalue(),
            err.getvalue(),
        )
        with contextlib.redirect_stderr(io.StringIO()):
            rc = main([str(Path(tmp, "missing.node"))])
        check("a missing file fails", rc == 1)

    if failures:
        print(f"{failures} check(s) failed")
        return 1
    print("check_macos_floor self-test passed")
    return 0


if __name__ == "__main__":
    sys.exit(main_test())
