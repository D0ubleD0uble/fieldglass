#!/usr/bin/env python3
"""Build a GRIB2 matrix-of-values (DRT 5.1, matrixBitmapsPresent=1) fixture.

Stock eccodes cannot represent the true per-point matrix — it divides by zero
and *crashes* on `matrixBitmapsPresent=1` — so this cannot go through the
eccodes encoder. Instead it takes a valid `matrixBitmapsPresent=0` skeleton from
the eccodes wheel (valid §0–§4) and byte-edits §3/§5/§6/§7 to hand-assemble the
true-matrix form, independently of the Rust decoder.

The encoded field deliberately matches the GRIB1 `hand_matrix_of_values.grib1`
fixture: a 16×31 grid (496 points), NR=1/NC=2 (datum 2), all-present primary and
secondary bitmaps, R=0/E=0/D=0, 8-bit packing, coded byte k = k % 256. So the
decoded matrix value at flat index k is k % 256 — the same hand-computable
oracle the GRIB1 decoder is validated against, giving a cross-edition check.

Regenerate:  python3 tools/build_grib2_matrix_reshape_fixture.py
Needs:       eccodes + numpy in this Python (for the skeleton only).
"""

from __future__ import annotations

import pathlib
import struct

import numpy as np

try:
    import eccodes as ec
except ImportError:  # pragma: no cover
    raise SystemExit("eccodes Python module not found (pip install eccodes numpy).")

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)
SAMPLE = pathlib.Path.home() / (
    "Code/research/wx_sci/wxcode-rs/eccodes/samples/GRIB2.tmpl"
)

NI, NJ = 16, 31          # 496 points, matching hand_matrix_of_values.grib1
NR, NC = 1, 2            # datum = 2
DATUM = NR * NC
POINTS = NI * NJ
CODED = POINTS * DATUM   # all cells present → 992 coded values


def skeleton() -> bytearray:
    """A valid matrixBitmapsPresent=0 grid_simple_matrix GRIB2 message."""
    with open(SAMPLE, "rb") as f:
        h = ec.codes_grib_new_from_file(f)
    ec.codes_set(h, "packingType", "grid_simple_matrix")
    ec.codes_set(h, "bitsPerValue", 8)
    ec.codes_set_values(h, np.zeros(ec.codes_get(h, "numberOfValues"), dtype=float))
    msg = bytearray(ec.codes_get_message(h))
    ec.codes_release(h)
    return msg


def sections(msg: bytes):
    """Yield (number, start, length) for each GRIB2 section."""
    off = 16  # after the 16-byte §0 Indicator Section
    end = len(msg) - 4  # before "7777"
    while off < end:
        seclen = struct.unpack(">I", msg[off:off + 4])[0]
        secnum = msg[off + 4]
        yield secnum, off, seclen
        off += seclen


def main() -> None:
    if not SAMPLE.exists():
        raise SystemExit(f"eccodes sample not found: {SAMPLE}")
    msg = skeleton()
    secs = {n: (s, l) for n, s, l in sections(msg)}

    # --- §3 GDS: force Ni=16, Nj=31 (template 3.0 payload octets 31–38 =
    #     section octets 45–52; payload starts at section octet 15). ---
    s3, _ = secs[3]
    p = s3 + 14  # template payload start
    struct.pack_into(">I", msg, p + 16, NI)
    struct.pack_into(">I", msg, p + 20, NJ)
    # numberOfDataPoints (section octets 7–10) must equal Ni·Nj.
    struct.pack_into(">I", msg, s3 + 6, POINTS)

    # --- §5 DRS template 5.1: set the matrix header. Payload starts at section
    #     octet 12 (= s5 + 11). Layout: R f32[0:4], E i16[4:6], D i16[6:8],
    #     bits[8], matrixBitmapsPresent[9], numberOfCodedValues u32[10:14],
    #     NR u16[14:16], NC u16[16:18]. ---
    s5, l5 = secs[5]
    q = s5 + 11
    struct.pack_into(">f", msg, q + 0, 0.0)      # R = 0
    struct.pack_into(">h", msg, q + 4, 0)        # E = 0
    struct.pack_into(">h", msg, q + 6, 0)        # D = 0
    msg[q + 8] = 8                               # bitsPerValue
    msg[q + 9] = 1                               # matrixBitmapsPresent = 1
    struct.pack_into(">I", msg, q + 10, CODED)   # numberOfCodedValues
    struct.pack_into(">H", msg, q + 14, NR)
    struct.pack_into(">H", msg, q + 16, NC)
    # numberOfValues (section octets 6–9) — keep it as the point count.
    struct.pack_into(">I", msg, s5 + 5, POINTS)

    # --- §7 DS: [CODED all-set secondary bits, byte-aligned][CODED 8-bit values
    #     k%256]. Rebuild the section body and length. ---
    sec_bytes = (CODED + 7) // 8
    body = bytearray(b"\xff" * sec_bytes)          # all secondary bits set
    body += bytes((k % 256) for k in range(CODED))  # coded values
    s7, l7 = secs[7]
    new_s7 = bytearray(struct.pack(">I", 5 + len(body)))  # length
    new_s7.append(7)                                      # section number
    new_s7 += body
    # Splice: everything before §7, the new §7, then the "7777" trailer.
    out = bytearray(msg[:s7]) + new_s7 + b"7777"
    # --- §0 total length (octets 9–16). ---
    struct.pack_into(">Q", out, 8, len(out))

    path = FIXTURES / "matrix_reshape_16x31.grib2"
    path.write_bytes(out)
    print(f"wrote {path.name}: {len(out)} bytes, "
          f"{NI}x{NJ} grid, NR={NR} NC={NC}, {CODED} coded values (k%256)")


if __name__ == "__main__":
    main()
