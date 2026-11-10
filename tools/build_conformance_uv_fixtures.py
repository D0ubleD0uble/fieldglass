#!/usr/bin/env python3
"""Build the multi-message wind fixtures the conformance suite pairs (#1205, #1140).

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

`uv_mismatch.grib2` and `uv_mismatch.grib1` are for `vector_mismatch`
(#1140), which asks whether the u and v are at one level and one time. Each
is a u and several v, the same committed message with its parameter, level
and time octets rewritten:

    0  u at 10 m
    1  v at 10 m, the same time
    2  v at 850 hPa
    3  v at 10 m, 6 hours later
    4  v at 850 hPa, 6 hours later
    5  (GRIB2) v at 10 m stated as 100 with scale factor 1;
       (GRIB1) v on the ground (level type 1), value octets 0
    6  (GRIB2) v at the same valid time from a run 6 hours later;
       (GRIB1) v on the ground, value octets 5, which mean nothing there

and, in `uv_mismatch.grib2` only, pairs for how a level is compared:

    7  u at 0.3 m, stated as 3 with scale factor 1
    8  v at 0.3 m, stated as 30 with scale factor 2
    9  v on the ground (type 1, a surface that names a place)
    10 v with its first surface coded missing (255)
    11 v at a height above ground whose scale factor and value are missing
    12 u in the layer from 10 m down to the ground, ground value 5
    13 v in the same layer, ground value 9

`uv_members.grib2` is for the ensemble member: the committed template 4.1
member, as u and v, with its perturbation number (§4 octet 36) rewritten:

    0  u, member 1
    1  v, member 1
    2  v, member 7

Usage:  python3 tools/build_conformance_uv_fixtures.py
Needs:  nothing beyond the standard library.
"""

from __future__ import annotations

import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
GFS = ROOT / "crates/fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2"
ECMWF = ROOT / "crates/fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1"
MEMBER = ROOT / "crates/fieldglass-grib2/tests/fixtures/ensemble_member_4_1.grib2"

# A missing surface: the type, scale factor and value octets all ones.
MISSING = (255, None, None)
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


def write_surface(msg: bytearray, at: int, surface: tuple[int, int | None, int | None]) -> None:
    """Write a fixed surface (type, scale factor, scaled value; `None` for
    coded missing, all ones) at `at`."""
    kind, scale, value = surface
    msg[at] = kind
    msg[at + 1] = 0xFF if scale is None else scale
    msg[at + 2 : at + 6] = (0xFFFFFFFF if value is None else value).to_bytes(4, "big")


def grib2_at(
    source: bytes,
    number: int,
    surface: tuple[int, int | None, int | None],
    forecast_hours: int,
    reference_hour: int | None = None,
    second: tuple[int, int | None, int | None] = MISSING,
) -> bytes:
    """`source` as discipline-0 category-2 parameter `number`, at `surface`
    (type, scale factor, scaled value: §4 octets 23-28), the layer's bottom
    `second` (octets 29-34, missing for a single level) and `forecast_hours`
    (§4 octets 19-22, in the source's unit, which is hours). `reference_hour`
    rewrites the hour of the reference time (§1 octet 17)."""
    msg = bytearray(grib2_as(source, 2, number))
    s4 = grib2_section(msg, 4)
    assert int.from_bytes(msg[s4 + 7 : s4 + 9], "big") == 0, "template 4.0"
    assert msg[s4 + 17] == 1, "forecast time in hours"
    msg[s4 + 18 : s4 + 22] = forecast_hours.to_bytes(4, "big")
    write_surface(msg, s4 + 22, surface)
    write_surface(msg, s4 + 28, second)
    if reference_hour is not None:
        s1 = grib2_section(msg, 1)
        msg[s1 + 16] = reference_hour
    return bytes(msg)


def grib1_at(source: bytes, parameter: int, level: tuple[int, int], p1: int) -> bytes:
    """`source` as `parameter`, at `level` (level type and 16-bit value: PDS
    octets 10-12) and `p1` (PDS octet 19, in the source's unit, which is
    hours)."""
    msg = bytearray(grib1_as(source, parameter))
    assert msg[8 + 17] == 1 and msg[8 + 20] == 0, "hours, time range 0"
    kind, value = level
    msg[8 + 9] = kind
    msg[8 + 10 : 8 + 12] = value.to_bytes(2, "big")
    msg[8 + 18] = p1
    return bytes(msg)


def grib2_member(source: bytes, number: int, member: int) -> bytes:
    """The template 4.1 `source` as discipline-0 category-2 parameter
    `number`, member `member` (§4 octet 36)."""
    msg = bytearray(grib2_as(source, 2, number))
    s4 = grib2_section(msg, 4)
    assert int.from_bytes(msg[s4 + 7 : s4 + 9], "big") == 1, "template 4.1"
    msg[s4 + 35] = member
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

    # Code Table 4.5: 103 is a height above ground in metres, 100 an isobaric
    # surface in pascals. The source's forecast time is +204h and its
    # reference time 2012-03-02 06:00, so message 6 (12:00, +198h) is valid
    # when message 0 is.
    assert gfs[16 + 16] == 6
    ten_m, p850 = (103, 0, 10), (100, 0, 85000)
    (OUT / "uv_mismatch.grib2").write_bytes(
        grib2_at(gfs, 2, ten_m, 204)
        + grib2_at(gfs, 3, ten_m, 204)
        + grib2_at(gfs, 3, p850, 204)
        + grib2_at(gfs, 3, ten_m, 210)
        + grib2_at(gfs, 3, p850, 210)
        + grib2_at(gfs, 3, (103, 1, 100), 204)
        + grib2_at(gfs, 3, ten_m, 198, reference_hour=12)
        + grib2_at(gfs, 2, (103, 1, 3), 204)
        + grib2_at(gfs, 3, (103, 2, 30), 204)
        + grib2_at(gfs, 3, (1, 0, 0), 204)
        + grib2_at(gfs, 3, MISSING, 204)
        + grib2_at(gfs, 3, (103, None, None), 204)
        + grib2_at(gfs, 2, ten_m, 204, second=(1, 0, 5))
        + grib2_at(gfs, 3, ten_m, 204, second=(1, 0, 9))
    )
    # The source is ensemble type 3, member 7 of 20.
    member = MEMBER.read_bytes()
    (OUT / "uv_members.grib2").write_bytes(
        grib2_member(member, 2, 1) + grib2_member(member, 3, 1) + grib2_member(member, 3, 7)
    )
    # ON388 Table 3: 105 is a height above ground in metres, 100 an isobaric
    # level in hPa. The source is a +0h forecast.
    ten_m1, hpa850 = (105, 10), (100, 850)
    (OUT / "uv_mismatch.grib1").write_bytes(
        grib1_at(ecmwf, 165, ten_m1, 0)
        + grib1_at(ecmwf, 166, ten_m1, 0)
        + grib1_at(ecmwf, 166, hpa850, 0)
        + grib1_at(ecmwf, 166, ten_m1, 6)
        + grib1_at(ecmwf, 166, hpa850, 6)
        + grib1_at(ecmwf, 166, (1, 0), 0)
        + grib1_at(ecmwf, 166, (1, 5), 0)
    )


if __name__ == "__main__":
    main()
