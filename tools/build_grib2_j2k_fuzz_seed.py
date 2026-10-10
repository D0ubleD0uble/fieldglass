#!/usr/bin/env python3
"""Write the worst-case JPEG 2000 fuzz seed of the `fieldglass-grib2` target (#838).

    python3 tools/build_grib2_j2k_fuzz_seed.py

The seed is a §5.40 message on a 128 x 128 grid, the largest image the fuzz
target decodes (`FUZZ_MAX_J2K_SAMPLES` = 2^14), whose codestream is as costly to
decode as its size allows:

* one tile, one component, 8 bits, five decomposition levels, 64 x 64
  code-blocks, one layer, reversible 5-3;
* QCD with seven guard bits and exponent 24 for every subband, so each block has
  30 bit-planes (`rust_j2k`'s `MAX_BIT_PLANES`) and the coding-pass loop runs its
  full 3 * 30 - 2 = 88 passes over every sample;
* every code-block is included in layer 0 and declares the largest pass count,
  164, in two codeword segments (109 and 55 passes) of one byte each;
* every coded byte is 0xFF. Random bytes decode in about a third of the time
  (8 us against 25 us per sample on the fuzz build): the MQ decoder takes the
  cheap path on most symbols there.

The packet headers are the only part that needs writing: tag-tree bits that
include each block with no zero bit-planes, a pass count, an Lblock increment of
zero and two length fields. The section layout is that of the 8192 x 8192 seed
(`jpeg2000_codestream_8192x8192_on_1x1.grib2`), with the grid and both point
counts changed.
"""

import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = ROOT / "crates" / "fieldglass-grib2" / "fuzz" / "corpus" / "decode"
LAYOUT = CORPUS / "jpeg2000_codestream_8192x8192_on_1x1.grib2"
OUT = CORPUS / "jpeg2000_codestream_128x128_max_passes.grib2"

SIDE = 128
LEVELS = 5
XCB = 4  # code-block width and height exponent, minus two: 64 x 64
GUARD_BITS = 7
EXPONENT = 24
CODED_BYTE = 0xFF


class BitWriter:
    """Packet-header bits, MSB first, with the Annex B.10.1 stuffing after 0xFF."""

    def __init__(self):
        self.out = bytearray()
        self.cur = 0
        self.n = 0
        self.cap = 8

    def bit(self, value):
        self.cur = (self.cur << 1) | value
        self.n += 1
        if self.n == self.cap:
            self.out.append(self.cur)
            self.cap = 7 if self.cur == 0xFF else 8
            self.cur = 0
            self.n = 0

    def bits(self, value, count):
        for i in range(count - 1, -1, -1):
            self.bit((value >> i) & 1)

    def finish(self):
        while self.n:
            self.bit(0)
        if self.out and self.out[-1] == 0xFF:
            self.out.append(0)
        return bytes(self.out)


def blocks_across(size):
    return -(-size // (1 << (XCB + 2)))


def band_sizes(resolution):
    """The subband side lengths of one resolution (a square image)."""
    size = SIDE
    if resolution == 0:
        for _ in range(LEVELS):
            size = -(-size // 2)
        return [size]
    for _ in range(LEVELS - resolution + 1):
        size = -(-size // 2)
    return [size, size, size]


def packets():
    out = bytearray()
    for resolution in range(LEVELS + 1):
        header = BitWriter()
        header.bit(1)  # non-empty packet
        body = bytearray()
        for size in band_sizes(resolution):
            across = blocks_across(size)
            depth = 1
            width = across
            while width > 1:
                width = (width + 1) // 2
                depth += 1
            known = (set(), set())  # inclusion tree, zero-bit-plane tree
            for y in range(across):
                for x in range(across):
                    for tree in known:
                        for level in range(depth - 1, -1, -1):
                            node = (level, x >> level, y >> level)
                            if node not in tree:
                                tree.add(node)
                                header.bit(1)  # value 0 at this node
                    header.bits((1 << 16) - 1, 16)  # 164 coding passes
                    header.bit(0)  # Lblock stays 3
                    header.bits(1, 9)  # 109-pass segment: 1 byte (3 + 6 bits)
                    header.bits(1, 8)  # 55-pass segment: 1 byte (3 + 5 bits)
                    body += bytes([CODED_BYTE, CODED_BYTE])
        out += header.finish() + body
    return bytes(out)


def codestream():
    siz = b"\xff\x51" + struct.pack(
        ">HHIIIIIIIIHBBB", 41, 0, SIDE, SIDE, 0, 0, SIDE, SIDE, 0, 0, 1, 7, 1, 1
    )
    cod = b"\xff\x52" + struct.pack(">HBBHBBBBBB", 12, 0, 0, 1, 0, LEVELS, XCB, XCB, 0, 1)
    subbands = 3 * LEVELS + 1
    qcd = (
        b"\xff\x5c"
        + struct.pack(">HB", 3 + subbands, GUARD_BITS << 5)
        + bytes([EXPONENT << 3]) * subbands
    )
    tile = b"\xff\x93" + packets()
    sot = b"\xff\x90" + struct.pack(">HHIBB", 10, 0, 12 + len(tile), 0, 1)
    return b"\xff\x4f" + siz + cod + qcd + sot + tile + b"\xff\xd9"


def main():
    layout = LAYOUT.read_bytes()
    sections = {}
    pos = 16
    while pos < len(layout) - 4:
        length = int.from_bytes(layout[pos : pos + 4], "big")
        sections[layout[pos + 4]] = bytearray(layout[pos : pos + length])
        pos += length
    count = struct.pack(">I", SIDE * SIDE)
    sections[3][6:10] = count  # number of data points
    sections[3][30:34] = struct.pack(">I", SIDE)  # Ni
    sections[3][34:38] = struct.pack(">I", SIDE)  # Nj
    sections[5][5:9] = count
    cs = codestream()
    sections[7] = bytearray(struct.pack(">IB", 5 + len(cs), 7)) + cs
    body = b"".join(bytes(sections[n]) for n in range(1, 8)) + b"7777"
    OUT.write_bytes(layout[:8] + struct.pack(">Q", 16 + len(body)) + body)
    print(f"{OUT.relative_to(ROOT)}: {OUT.stat().st_size} bytes, codestream {len(cs)}")


if __name__ == "__main__":
    main()
