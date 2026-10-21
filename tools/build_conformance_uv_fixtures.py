#!/usr/bin/env python3
"""Build the two multi-message wind fixtures the conformance suite pairs (#1205).

`vector_pairing` asks whether two messages of one file are the u and v
components of a vector, and every committed GRIB fixture holds one message and
none of them is a wind component. So each fixture here is a committed message
repeated with its parameter octets rewritten, concatenated into one file, as
`crates/fieldglass/tests/vector_pairing.rs` builds its pairs in memory. The
suite needs them on disk because its cases name a fixture file.

Message order, shared by both files so the cases read alike:

    0  u component
    1  v component
    2  wind speed
    3  the message as committed (a pressure for GRIB2, 2t for GRIB1)

Only the parameter octets change: the grid and the packed values are the
source message's, which is all `vector_pairing` never reads.

Usage:  python3 tools/build_conformance_uv_fixtures.py
Needs:  nothing beyond the standard library.
"""

from __future__ import annotations

import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
GFS = ROOT / "crates/fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2"
ECMWF = ROOT / "crates/fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1"
OUT = ROOT / "crates/fieldglass/tests/fixtures"


def grib2_section(msg: bytes, number: int) -> int:
    """The offset of a GRIB2 message's section `number`."""
    at = 16
    while msg[at : at + 4] != b"7777":
        if msg[at + 4] == number:
            return at
        at += int.from_bytes(msg[at : at + 4], "big")
    raise SystemExit(f"no section {number}")


def grib2_as(source: bytes, category: int, number: int) -> bytes:
    """`source` as discipline-0 parameter `category`-`number` (§4 octets 10, 11)."""
    assert source[6] == 0, "discipline 0"
    msg = bytearray(source)
    s4 = grib2_section(msg, 4)
    msg[s4 + 9] = category
    msg[s4 + 10] = number
    return bytes(msg)


def grib1_as(source: bytes, parameter: int) -> bytes:
    """`source` with its PDS parameter (octet 9 of the PDS) rewritten."""
    msg = bytearray(source)
    msg[8 + 8] = parameter
    return bytes(msg)


def main() -> None:
    gfs = GFS.read_bytes()
    (OUT / "uv_pairs.grib2").write_bytes(
        grib2_as(gfs, 2, 2) + grib2_as(gfs, 2, 3) + grib2_as(gfs, 2, 1) + gfs
    )
    # ECMWF table 128: 165 is 10u, 166 is 10v, 207 is 10si (wind speed); the
    # message as committed is 167 (2t), checked below.
    ecmwf = ECMWF.read_bytes()
    assert ecmwf[8 + 8] == 167
    (OUT / "uv_pairs.grib1").write_bytes(
        grib1_as(ecmwf, 165) + grib1_as(ecmwf, 166) + grib1_as(ecmwf, 207) + ecmwf
    )


if __name__ == "__main__":
    main()
