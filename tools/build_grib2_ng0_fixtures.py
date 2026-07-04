#!/usr/bin/env python3
"""Rebuild the GRIB2 NG == 0 constant-field fixtures (#222).

Byte-patches the committed complex-packing fixtures into their
``numberOfGroupsOfDataValues == 0`` constant-field form (eccodes
ECC-2095): §5 octets 32-35 zeroed, §7 truncated to its bare 5-octet
header (no group blocks — for 5.3 not even the spatial-differencing
extra descriptors), and the §7 length and §0 ``totalLength`` recomputed
to match.

Sources and outputs (all under ``crates/fieldglass-grib2/tests/fixtures``):

- ``complex_regular_latlon.grib2``      -> ``complex_ng0_regular_latlon.grib2``
- ``complex_spd2_regular_latlon.grib2`` -> ``complex_spd2_ng0_regular_latlon.grib2``

The sibling ``*_expected.json`` value oracles are NOT regenerated here:
they require eccodes >= 2.42 (ECC-2095 shipped in 2.42.0; the pinned
2.34.1 predates it and mis-decodes NG == 0). See
``crates/fieldglass-grib2/tests/fixtures/NOTICE.md`` for the version
caveat and provenance.

Usage:
    python3 tools/build_grib2_ng0_fixtures.py
"""

from __future__ import annotations

import struct
from pathlib import Path

FIXTURES = (
    Path(__file__).resolve().parent.parent
    / "crates"
    / "fieldglass-grib2"
    / "tests"
    / "fixtures"
)

PATCHES = {
    "complex_regular_latlon.grib2": "complex_ng0_regular_latlon.grib2",
    "complex_spd2_regular_latlon.grib2": "complex_spd2_ng0_regular_latlon.grib2",
}


def patch(src: Path, dst: Path) -> None:
    data = src.read_bytes()
    assert data[:4] == b"GRIB" and data[7] == 2, f"{src.name}: not GRIB2"
    total_len = struct.unpack(">Q", data[8:16])[0]
    assert total_len == len(data), (src.name, total_len, len(data))

    out = bytearray(data[:16])  # section 0; totalLength fixed below
    pos = 16
    while data[pos : pos + 4] != b"7777":
        sec_len = struct.unpack(">I", data[pos : pos + 4])[0]
        sec_num = data[pos + 4]
        body = bytearray(data[pos : pos + sec_len])
        if sec_num == 5:
            # Octets 32-35 (1-based): numberOfGroupsOfDataValues -> 0.
            body[31:35] = b"\x00\x00\x00\x00"
        elif sec_num == 7:
            # Bare header: NG == 0 leaves no group blocks in section 7.
            body = bytearray(struct.pack(">IB", 5, 7))
        out += body
        pos += sec_len
    out += b"7777"
    out[8:16] = struct.pack(">Q", len(out))
    dst.write_bytes(out)
    print(f"wrote {dst.name} ({len(data)} -> {len(out)} bytes)")


def main() -> int:
    for src_name, dst_name in PATCHES.items():
        patch(FIXTURES / src_name, FIXTURES / dst_name)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
