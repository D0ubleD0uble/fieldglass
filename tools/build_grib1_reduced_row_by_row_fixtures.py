#!/usr/bin/env python3
"""Build the GRIB1 reduced-grid ``grid_second_order_row_by_row`` fixtures (#611).

``row_by_row`` is the one GRIB1 second-order layout whose groups come from the
grid rather than from the section: one group per row, each holding that row's
points. On a reduced grid the rows differ in width, so group ``j`` holds
``pl[j]`` points — which is what eccodes' ``DataG1SecondOrderRowByRowPacking``
does when the message has a ``pl`` key.

eccodes 2.34.1 cannot *encode* this packing (its ``pack_double`` switches the
message to ``grid_second_order``), so the section is assembled here and eccodes'
*decode* is the oracle. The IS, PDS and GDS are the committed
``reduced_gg_n32_smooth.grib1``'s (N32, 64 rows of 20 to 128 points, 6114
total, ``jPointsAreConsecutive = 0``); only the BDS is new:

- ``reduced_gg_row_by_row.grib1`` — the field quantised to 1/16 K, one group
  per row: the row's minimum as its first-order value and the smallest width
  that holds the row's residuals. Every eighth row (3, 11, 19, …) is flattened
  to its minimum, so those groups are zero-width runs and the residual reader
  has to skip nothing for them.
- ``reduced_gg_row_by_row_boust.grib1`` — the same octets with the
  ``boustrophedonicOrdering`` bit (octet 14, 0x04) set.

**eccodes ignores that bit for this packing**, and so does the reader.
``grib1/data.grid_second_order_row_by_row.def`` is the one second-order data
definition with no ``data_apply_boustrophedonic`` wrapper, so both files decode
to the same values in eccodes — which is what the second oracle records, taken
from eccodes' own decode of the flagged file rather than assumed.

``jPointsAreConsecutive`` stays 0 on purpose. eccodes' unpack takes its row
count from ``Ni`` when that flag is set, and a reduced grid's ``Ni`` is the
missing value, so a flagged reduced message is not one eccodes can be an oracle
for.

Usage:  python3 tools/build_grib1_reduced_row_by_row_fixtures.py
Needs:  eccodes 2.34.1 on PATH (`grib_get`, `grib_get_data`, `grib_dump`).
"""

from __future__ import annotations

import math
from pathlib import Path

from build_grib1_reduced_second_order_fixtures import row_widths, sample_indices, write_oracle
from eccodes_oracle import decoded_values, grib_get

FIXTURES = (
    Path(__file__).resolve().parent.parent
    / "crates"
    / "fieldglass-grib1"
    / "tests"
    / "fixtures"
)
SOURCE = FIXTURES / "reduced_gg_n32_smooth.grib1"
NUM_VALUES = 6114
NUM_ROWS = 64
#: Binary scale factor: values are stored in steps of 2^-4 = 1/16.
BINARY_SCALE = -4
#: Rows flattened to a constant, so their groups are zero-width.
FLAT_ROW_STEP = 8
FLAT_ROW_OFFSET = 3
BOUSTROPHEDONIC_BIT = 0x04


def ibm_float(value: float) -> bytes:
    """``value`` as a 4-octet IBM System/360 float, which GRIB1 uses for R.

    Only exact encodings are accepted: the reference value is chosen as a whole
    number, and a lossy one would move every decoded point.
    """
    if value == 0:
        return bytes(4)
    sign = 0x80 if value < 0 else 0
    magnitude = abs(value)
    exponent = 0
    while magnitude >= 1:
        magnitude /= 16
        exponent += 1
    while magnitude < 1 / 16:
        magnitude *= 16
        exponent -= 1
    mantissa = magnitude * 2**24
    assert mantissa == int(mantissa), f"{value} has no exact IBM encoding"
    return bytes([sign | (exponent + 64)]) + int(mantissa).to_bytes(3, "big")


def sign_magnitude_16(value: int) -> bytes:
    """A GRIB1 signed 16-bit field: high bit is the sign."""
    return ((0x8000 if value < 0 else 0) | abs(value)).to_bytes(2, "big")


class BitWriter:
    def __init__(self) -> None:
        self.bits: list[int] = []

    def write(self, value: int, width: int) -> None:
        assert 0 <= value < (1 << width) or width == 0 and value == 0, (value, width)
        self.bits += [(value >> (width - 1 - i)) & 1 for i in range(width)]

    def pad_to_octet(self) -> None:
        self.bits += [0] * (-len(self.bits) % 8)

    def to_bytes(self) -> bytes:
        assert len(self.bits) % 8 == 0
        return bytes(
            int("".join(map(str, self.bits[i : i + 8])), 2) for i in range(0, len(self.bits), 8)
        )


def split_message(message: bytes) -> tuple[bytes, bytes]:
    """The PDS and GDS octets of a one-message GRIB1 file with no BMS."""
    pds_len = int.from_bytes(message[8:11], "big")
    pds = message[8 : 8 + pds_len]
    assert pds[7] & 0x80 and not pds[7] & 0x40, "expected a GDS and no BMS"
    gds_start = 8 + pds_len
    gds_len = int.from_bytes(message[gds_start : gds_start + 3], "big")
    return pds, message[gds_start : gds_start + gds_len]


def row_by_row_bds(stored: list[int], widths: list[int], reference: float) -> bytes:
    """A ``grid_second_order_row_by_row`` BDS for integers ``stored``.

    Layout per ``grib1/data.grid_second_order_row_by_row.def`` (0-indexed
    octets): the 11-octet simple header, N1, the extended flag, N2, the group
    and point counts, ``extraValues``, one width octet per group, then the
    first-order values and the per-row residuals, each block octet-aligned.
    """
    firsts: list[int] = []
    group_widths: list[int] = []
    start = 0
    for width in widths:
        row = stored[start : start + width]
        firsts.append(min(row))
        group_widths.append((max(row) - min(row)).bit_length())
        start += width

    width_of_firsts = max(max(firsts).bit_length(), 1)
    first_order = BitWriter()
    for first in firsts:
        first_order.write(first, width_of_firsts)
    first_order.pad_to_octet()

    residuals = BitWriter()
    start = 0
    for width, first, group_width in zip(widths, firsts, group_widths):
        for x in stored[start : start + width]:
            residuals.write(x - first, group_width)
        start += width
    unused_bits = -len(residuals.bits) % 8
    residuals.pad_to_octet()

    descriptors_end = 21 + len(widths)
    n1 = descriptors_end + 1
    n2 = n1 + len(first_order.bits) // 8
    body = (
        ibm_float(reference)
        + bytes([width_of_firsts])
        + n1.to_bytes(2, "big")
        + bytes([0x10])  # secondOrderOfDifferentWidth; no secondary bitmap, not extended
        + n2.to_bytes(2, "big")
        + len(widths).to_bytes(2, "big")  # codedNumberOfFirstOrderPackedValues
        + NUM_VALUES.to_bytes(2, "big")  # numberOfSecondOrderPackedValues
        + bytes([0])  # extraValues
        + bytes(group_widths)
        + first_order.to_bytes()
        + residuals.to_bytes()
    )
    length = 3 + 1 + 2 + len(body)
    # Complex packing (0x40) with the additional-flags bit (0x10), as the
    # regular-grid hand_second_order_* fixtures set it; the low nibble is the
    # count of unused bits at the end of the section.
    flag = 0x40 | 0x10 | unused_bits
    return length.to_bytes(3, "big") + bytes([flag]) + sign_magnitude_16(BINARY_SCALE) + body


def assemble(pds: bytes, gds: bytes, bds: bytes) -> bytes:
    total = 8 + len(pds) + len(gds) + len(bds) + 4
    return b"GRIB" + total.to_bytes(3, "big") + bytes([1]) + pds + gds + bds + b"7777"


def main() -> None:
    message = SOURCE.read_bytes()
    pds, gds = split_message(message)
    assert int(grib_get(SOURCE, ["decimalScaleFactor"])[0]) == 0
    assert int(grib_get(SOURCE, ["jPointsAreConsecutive"])[0]) == 0

    widths = row_widths(SOURCE)
    assert len(widths) == NUM_ROWS and sum(widths) == NUM_VALUES, widths[:4]

    source_values = decoded_values(SOURCE)
    assert len(source_values) == NUM_VALUES and None not in source_values
    reference = float(math.floor(min(source_values)))
    step = 2.0**BINARY_SCALE
    stored = [round((v - reference) / step) for v in source_values]
    start = 0
    for row, width in enumerate(widths):
        if row % FLAT_ROW_STEP == FLAT_ROW_OFFSET:
            floor = min(stored[start : start + width])
            # A row that was already constant would be zero-width anyway.
            assert max(stored[start : start + width]) > floor, row
            stored[start : start + width] = [floor] * width
        start += width

    plain = FIXTURES / "reduced_gg_row_by_row.grib1"
    plain_bytes = assemble(pds, gds, row_by_row_bds(stored, widths, reference))
    plain.write_bytes(plain_bytes)

    extended_flag = 8 + len(pds) + len(gds) + 13
    assert plain_bytes[extended_flag] == 0x10
    boust = FIXTURES / "reduced_gg_row_by_row_boust.grib1"
    boust_bytes = bytearray(plain_bytes)
    boust_bytes[extended_flag] |= BOUSTROPHEDONIC_BIT
    boust.write_bytes(bytes(boust_bytes))

    assert grib_get(plain, ["packingType"])[0] == "grid_second_order_row_by_row"
    assert int(grib_get(boust, ["boustrophedonicOrdering"])[0]) == 1

    # The expected field, computed here, must be what eccodes decodes — or the
    # section is not the layout eccodes reads and it is no oracle at all.
    expected = [reference + x * step for x in stored]
    indices = sample_indices(widths)
    for path in (plain, boust):
        values = decoded_values(path)
        assert len(values) == NUM_VALUES, (path.name, len(values))
        worst = max(abs(a - b) for a, b in zip(values, expected))
        assert worst < 1e-6, f"{path.name}: eccodes disagrees with the packed field by {worst}"
        write_oracle(
            path.with_name(path.stem + "_expected.json"),
            values,
            indices,
            f"eccodes 2.34.1 grib_get_data of {path.name}, hand-assembled "
            "grid_second_order_row_by_row on the reduced N32 Gaussian grid of "
            "reduced_gg_n32_smooth.grib1 by "
            "tools/build_grib1_reduced_row_by_row_fixtures.py. Provenance in NOTICE.md.",
        )
        print(f"wrote {path.name} ({path.stat().st_size} bytes)")
    print(f"oracles carry {len(indices)} sampled points over {NUM_ROWS} rows")


if __name__ == "__main__":
    main()
