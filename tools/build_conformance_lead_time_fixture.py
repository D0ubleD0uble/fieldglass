#!/usr/bin/env python3
"""Build the GRIB1 fixture whose two hours numbers differ (#1694).

`MessageInfo` reports a message's lead twice: `forecastHours`, P1 in hours as
0.6.0 reported it whatever the time-range indicator says, and `offsetHours`,
the hours from the reference time to where the product starts. Every other
message the conformance suite records has the two equal, so a host that sent
one number under both keys would pass. The suite names a fixture file rather
than building bytes in memory, so this one is on disk.

Each message is the committed `j_consecutive_latlon.grib1` (ECMWF, a +0h
forecast in hours) with its PDS P1 and time-range indicator rewritten:

    0  time range 1, an analysis, P1 6: forecastHours 6, offsetHours 0
    1  time range 200, which ECMWF's Code Table 5 does not name, P1 12:
       forecastHours 12, offsetHours null

Only those two octets change; the grid and the packed values are the source
message's.

Usage:  python3 tools/build_conformance_lead_time_fixture.py
Needs:  nothing beyond the standard library.
"""

from __future__ import annotations

import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
SOURCE = ROOT / "crates/fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1"
OUT = ROOT / "crates/fieldglass/tests/fixtures/lead_hours.grib1"

# Section 0 is 8 octets, so PDS octet n sits at 8 + n - 1.
P1 = 8 + 19 - 1
TIME_RANGE = 8 + 21 - 1


def restamped(source: bytes, p1: int, time_range: int) -> bytes:
    """`source` with PDS octet 19 (P1) and octet 21 (time range) rewritten."""
    msg = bytearray(source)
    msg[P1] = p1
    msg[TIME_RANGE] = time_range
    return bytes(msg)


def main() -> None:
    source = SOURCE.read_bytes()
    # ECMWF (PDS octet 5), hours (octet 18), P1 0, P2 0, time range 0.
    assert (source[8 + 4], source[8 + 17], source[P1], source[8 + 19], source[TIME_RANGE]) == (98, 1, 0, 0, 0)
    OUT.write_bytes(restamped(source, 6, 1) + restamped(source, 12, 200))


if __name__ == "__main__":
    main()
