#!/usr/bin/env python3
"""Write the JP2-wrapped JPEG 2000 fuzz seed of the `fieldglass-grib2` target (#1801).

    python3 tools/build_grib2_jp2_fuzz_seed.py

GRIB2 template 5.40 puts a raw JPEG 2000 codestream in §7, which opens with the
SOC marker `FF 4F`. A JP2 file wraps a codestream in boxes and opens with the
12-byte signature box instead. rust-j2k 0.4.0 decodes both, so a JP2 file in §7
used to skip the reader's SIZ pre-check (#848), which reads only a raw
codestream: this 337-byte file states an 8192 x 8192 image on a 1 x 1 grid and
was decoded in full (67 M samples, 266 MB, about a second in release) before
the sample count refused it.

The codestream is the one #1801's test round used: OpenJPEG 2.5.0 output (its
COM marker says so) with SIZ set to one 8192 x 8192 tile of one 1-bit unsigned
component, five decomposition levels, and a single 131-byte tile-part. It is
written out below as hex. This script adds the smallest JP2 wrapper rust-j2k
accepts (ISO/IEC 15444-1 Annex I): the signature box, `ftyp` with brand
`jp2 `, a `jp2h` holding `ihdr` and an enumerated grayscale `colr`, and the
`jp2c` box. The section layout is that of the 8192 x 8192 seed
(`jpeg2000_codestream_8192x8192_on_1x1.grib2`) with §7 replaced.

The seed is written to the fuzz corpus; `tests/fuzz_seeds/` holds a byte-for-byte
copy so the published crate's tests can read it (#926):

    cp crates/fieldglass-grib2/fuzz/corpus/decode/jpeg2000_jp2_8192x8192_on_1x1.grib2 \
       crates/fieldglass-grib2/tests/fuzz_seeds/
"""

import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parent.parent
CORPUS = ROOT / "crates" / "fieldglass-grib2" / "fuzz" / "corpus" / "decode"
LAYOUT = CORPUS / "jpeg2000_codestream_8192x8192_on_1x1.grib2"
OUT = CORPUS / "jpeg2000_jp2_8192x8192_on_1x1.grib2"

SIDE = 8192

CODESTREAM = bytes.fromhex(
    "ff4fff5100290000000020000000200000000000000000000000200000002000"
    "00000000000000000001000101ff52000c00000001000504040001ff5c001340"
    "08101018101018101018101018101018ff640025000143726561746564206279"
    "204f70656e4a5045472076657273696f6e20322e352e30ff90000a0000000000"
    "830001ff93f7366f366cd9b366f366f366cd9b3660115054afff7f115054afff"
    "7f115054afff7f115054afff7f115054afff7f115054afff7f115054afff7f11"
    "5054afff7f115054afff7f115054afff7f115054afff7f115054afff7f115054"
    "afff7f115054afff7f115054afff7f115054afff7f8080808080ffd9"
)


def box(kind, payload):
    """One JP2 box: `LBox` (4 bytes, the whole box), `TBox`, then the contents."""
    return struct.pack(">I", 8 + len(payload)) + kind + payload


def jp2():
    # ihdr: HEIGHT, WIDTH, NC, BPC (depth - 1, unsigned), C = 7 (JPEG 2000),
    # UnkC, IPR. colr: METH = 1 (enumerated), PREC, APPROX, EnumCS = 17 (grayscale).
    ihdr = struct.pack(">IIHBBBB", SIDE, SIDE, 1, 0, 7, 0, 0)
    colr = struct.pack(">BBBI", 1, 0, 0, 17)
    return (
        box(b"jP  ", b"\r\n\x87\n")
        + box(b"ftyp", b"jp2 " + struct.pack(">I", 0) + b"jp2 ")
        + box(b"jp2h", box(b"ihdr", ihdr) + box(b"colr", colr))
        + box(b"jp2c", CODESTREAM)
    )


def main():
    layout = LAYOUT.read_bytes()
    sections = {}
    pos = 16
    while layout[pos : pos + 4] != b"7777":
        (length,) = struct.unpack(">I", layout[pos : pos + 4])
        sections[layout[pos + 4]] = layout[pos : pos + length]
        pos += length
    data = jp2()
    assert len(data) == 337, len(data)
    sections[7] = struct.pack(">IB", 5 + len(data), 7) + data
    body = b"".join(sections[n] for n in range(1, 8)) + b"7777"
    OUT.write_bytes(layout[:8] + struct.pack(">Q", 16 + len(body)) + body)


if __name__ == "__main__":
    main()
