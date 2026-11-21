#!/usr/bin/env python3
"""Fail when a macOS native module needs a newer macOS than the supported floor.

    python3 tools/check_macos_floor.py extension/bin/fieldglass.darwin-arm64.node

The release workflow runs this after each macOS build of the extension's
native module (#1904). A Mach-O file records the oldest macOS it loads on as
the `minos` of its `LC_BUILD_VERSION` load command, or the version of
`LC_VERSION_MIN_MACOSX` from older toolchains, and macOS refuses to load a
file whose `minos` is newer than itself. The 0.6.2 dry-run modules read 10.12
(x86_64) and 11.0 (arm64), which are rustc's defaults for those targets.
The final link writes that value, so it moves when a Rust bump changes the
default or when the build sets a deployment target
(`MACOSX_DEPLOYMENT_TARGET`, which rustc reads). Either could raise it
without any other gate noticing, since the smoke test loads the modules only
on recent macOS. An object file built for a newer macOS does not raise it:
the linker only warns. A dependency that calls an API newer than the floor
is out of this check's scope, since the version commands do not record it.

The checker reads every slice of a thin or universal file and every version
command in each slice. It fails on one above its architecture's floor, and it
fails closed: a slice with no version command, a version command for another
platform, an architecture with no floor, or a file it cannot parse. It uses
only the standard library, so it runs on the macOS runner's own Python, and it
reads the file itself rather than calling `otool`, so its self-test runs on
Linux too.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path
from typing import NamedTuple

CPU_TYPE_X86_64 = 0x01000007
CPU_TYPE_ARM64 = 0x0100000C
ARCH_NAMES = {CPU_TYPE_X86_64: "x86_64", CPU_TYPE_ARM64: "arm64"}


def version(major: int, minor: int, patch: int = 0) -> int:
    """A macOS version in Mach-O's packed `xxxx.yy.zz` form."""
    return (major << 16) | (minor << 8) | patch


# The oldest macOS each module must load on, by CPU type. VS Code 1.85, the
# oldest the extension supports (`engines.vscode` ^1.85.0 in
# extension/package.json), runs on macOS 10.15 and later. Apple Silicon first
# shipped with macOS 11.0, so no arm64 Mac runs anything older. Raise these
# only with the `engines.vscode` floor.
FLOORS = {
    CPU_TYPE_X86_64: version(10, 15),
    CPU_TYPE_ARM64: version(11, 0),
}

PLATFORM_MACOS = 1
LC_VERSION_MIN_MACOSX = 0x24
LC_BUILD_VERSION = 0x32
# The smallest size of each version command, so one too short for its fields
# is refused rather than read into the next command.
VERSION_SIZES = {LC_BUILD_VERSION: 24, LC_VERSION_MIN_MACOSX: 16}

FAT_MAGIC = 0xCAFEBABE
FAT_MAGIC_64 = 0xCAFEBABF
# Thin header magics as read little-endian: native order, then byte-swapped.
THIN_MAGICS = {
    0xFEEDFACE: ("<", 28),
    0xFEEDFACF: ("<", 32),
    0xCEFAEDFE: (">", 28),
    0xCFFAEDFE: (">", 32),
}


class Slice(NamedTuple):
    """One architecture's image: its CPU type and each `(platform, minos)` it records."""

    cpu: int
    versions: list[tuple[int, int]]


def version_text(packed: int) -> str:
    """`0x000A0C00` as `10.12`, and `0x000A0F01` as `10.15.1`."""
    text = f"{packed >> 16}.{(packed >> 8) & 0xFF}"
    return text + f".{packed & 0xFF}" if packed & 0xFF else text


def arch_name(cpu: int) -> str:
    return ARCH_NAMES.get(cpu, f"cputype {cpu:#x}")


def unpack(fmt: str, data: bytes, offset: int, what: str) -> tuple[int, ...]:
    if offset < 0 or offset + struct.calcsize(fmt) > len(data):
        raise ValueError(f"the file ends inside {what}")
    return struct.unpack_from(fmt, data, offset)


def read_thin(data: bytes) -> Slice:
    """The CPU type and version commands of one thin Mach-O image."""
    if len(data) < 4:
        raise ValueError("too short for a Mach-O header")
    (magic,) = struct.unpack_from("<I", data)
    if magic not in THIN_MAGICS:
        raise ValueError(f"not a Mach-O file (magic {magic:#010x})")
    order, header_size = THIN_MAGICS[magic]
    _, cpu, _, _, ncmds, sizeofcmds = unpack(order + "6I", data, 0, "the Mach-O header")
    end = header_size + sizeofcmds
    if end > len(data):
        raise ValueError("the load commands run past the end of the file")
    versions = []
    offset = header_size
    for index in range(ncmds):
        cmd, size = unpack(order + "2I", data, offset, f"load command {index}")
        if size < 8 or offset + size > end:
            raise ValueError(f"load command {index} has a bad size ({size})")
        if cmd in VERSION_SIZES and size < VERSION_SIZES[cmd]:
            raise ValueError(
                f"load command {index} is a version command of {size} bytes, "
                f"shorter than its {VERSION_SIZES[cmd]}"
            )
        if cmd == LC_BUILD_VERSION:
            platform, minos = unpack(order + "2I", data, offset + 8, "LC_BUILD_VERSION")
            versions.append((platform, minos))
        elif cmd == LC_VERSION_MIN_MACOSX:
            (minos,) = unpack(order + "I", data, offset + 8, "LC_VERSION_MIN_MACOSX")
            versions.append((PLATFORM_MACOS, minos))
        offset += size
    return Slice(cpu, versions)


def read_slices(data: bytes) -> list[Slice]:
    """Every slice of a thin or universal Mach-O file. `ValueError` if it is neither."""
    if len(data) >= 8:
        magic, count = struct.unpack_from(">2I", data)
        if magic in (FAT_MAGIC, FAT_MAGIC_64):
            if count == 0:
                raise ValueError("a universal file with no slices")
            wide = magic == FAT_MAGIC_64
            entry, fmt = (32, ">2I2Q") if wide else (20, ">4I")
            slices = []
            for index in range(count):
                cpu, _, offset, size = unpack(fmt, data, 8 + index * entry, f"fat entry {index}")
                if offset + size > len(data):
                    raise ValueError(f"slice {index} ({arch_name(cpu)}) runs past the end of the file")
                found = read_thin(data[offset : offset + size])
                if found.cpu != cpu:
                    raise ValueError(
                        f"slice {index} is listed as {arch_name(cpu)} but its header says {arch_name(found.cpu)}"
                    )
                slices.append(found)
            return slices
    return [read_thin(data)]


def problems(slices: list[Slice], floors: dict[int, int] = FLOORS) -> list[str]:
    """Why these slices do not all load on their architecture's floor, or `[]`."""
    out = []
    for piece in slices:
        arch = arch_name(piece.cpu)
        floor = floors.get(piece.cpu)
        if floor is None:
            out.append(f"{arch} has no macOS floor in check_macos_floor.py")
            continue
        if not piece.versions:
            out.append(f"{arch} has no LC_BUILD_VERSION or LC_VERSION_MIN_MACOSX command")
        for platform, minos in piece.versions:
            if platform != PLATFORM_MACOS:
                out.append(f"{arch} has a build version for platform {platform}, not macOS ({PLATFORM_MACOS})")
            elif minos > floor:
                out.append(f"{arch} needs macOS {version_text(minos)}, newer than the {version_text(floor)} floor")
    return out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("modules", nargs="+", type=Path)
    args = parser.parse_args(argv)

    failed = False
    for module in args.modules:
        try:
            slices = read_slices(module.read_bytes())
        except (OSError, ValueError) as e:
            print(f"::error::{module}: {e}", file=sys.stderr)
            failed = True
            continue
        found = problems(slices)
        if found:
            failed = True
            for problem in found:
                print(f"::error::{module}: {problem}", file=sys.stderr)
            continue
        for piece in slices:
            newest = max(minos for _, minos in piece.versions)
            print(
                f"{module}: {arch_name(piece.cpu)} needs macOS {version_text(newest)}, "
                f"floor {version_text(FLOORS[piece.cpu])}: OK"
            )
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
