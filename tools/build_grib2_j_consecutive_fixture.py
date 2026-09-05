#!/usr/bin/env python3
"""Build a GRIB2 lat/lon fixture that stores its points column by column.

§3 Flag Table 3.4 bit 3 (`jPointsAreConsecutive`) says the message stores
meridians rather than parallels: adjacent stored points step in `j`, not in `i`.
No fixture in the repo's GRIB2 corpus set it — checked, every one reads back a
scanning mode with bit 3 clear — so `decode_message_raster` could hand back a
transposed field and every test still passed (#602). This builds the message
that tells the two apart. It is the GRIB2 twin of
`tools/build_grib1_j_consecutive_fixture.py`, deliberately the same grid so the
two editions' tests read alike.

**Why it must not be square.** `Ni != Nj` is the whole point: on a square grid a
transposed raster is still `Ni·Nj` long and a decoder that forgets the flag
produces a plausible-looking picture. At 8x5 the wrong reading is 5 rows of 8
where the right one is... also 5 rows of 8, but with the values permuted — so
the *values* have to distinguish it too, which is what the ramp below does.

**Why a ramp and not a smooth field.** This fixture is read by an assertion:
`10*j + i` gives every one of the 40 points a distinct value that names its own
position, so a test can say `raster[j*ni + i] == 10*j + i` and a single misplaced
point fails it. A smooth field would let a transpose hide in the interpolation.

**Encode with the wheel, verify with the pin.** Setting the values array needs
`codes_set_values`, which the packaged 2.34.1 CLI has no equivalent for; eccodes
2.34.1 on PATH is then the oracle for what was written, as `NOTICE.md` requires.
The pin's `regular_ll` geoiterator does honour the flag — it walks the message
column by column — so `grib_get_data` is a valid oracle here, and the builder
asserts exactly that rather than taking it on trust.

It also writes `j_consecutive_boust_expected.json`, the oracle for the *other*
half of #602: the run length the second-order boustrophedonic undo reverses.
That case is a one-bit edit to an existing 16 x 31 fixture, so the file is not
committed a second time — but the pinned eccodes' answer for it is, or the claim
that eccodes reverses meridians of `Nj` would live only in a comment and CI
would never see it.

Usage:  python3 tools/build_grib2_j_consecutive_fixture.py
Needs:  the `eccodes` PyPI wheel (encoding) and eccodes 2.34.1 on PATH (oracle).
Then:   python3 tools/regenerate-eccodes-snapshots.py --edition 2
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

import eccodes as ec
import numpy as np

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)

# The stock regular lat/lon GRIB2 sample, shrunk to the grid below. Its §4 is
# inherited and does not describe this synthetic field; only the decode path is
# under test.
SAMPLE = pathlib.Path("/usr/share/eccodes/samples/regular_ll_sfc_grib2.tmpl")
OUT = FIXTURES / "j_consecutive_latlon.grib2"

# The committed second-order boustrophedonic fixture, and the oracle derived
# from it by setting §3 bit 3 in memory. 16 x 31, so its two candidate run
# lengths are different numbers and the answer is unambiguous.
BOUST = FIXTURES / "second_order_boust_regular_latlon.grib2"
BOUST_OUT = FIXTURES / "j_consecutive_boust_expected.json"
BOUST_NI, BOUST_NJ = 16, 31

# §3 octet 72 is the scanning-mode octet of template 3.0, which both messages
# use. Bit 3 of Flag Table 3.4.
SCANNING_MODE_OCTET = 72
SCAN_J_CONSECUTIVE = 0x20

NI, NJ = 8, 5

# North-to-south, west-to-east, 5-degree steps: the ordinary orientation, so the
# only thing unusual about this message is which axis runs fastest.
LAT_FIRST, LON_FIRST = 60.0, 0.0
STEP = 5.0

# 16 bits over a span of 44 resolves to under 1e-3, which is what the test's
# tolerance is set from. Simple packing cannot store the integers exactly.
BITS_PER_VALUE = 16

# What the pin must read back. Asserted rather than printed: a builder that
# quietly dropped the scanning-mode bit would leave the transpose untested while
# looking like it had covered it.
EXPECTED = {
    "gridType": "regular_ll",
    "gridDefinitionTemplateNumber": "0",
    "Ni": str(NI),
    "Nj": str(NJ),
    "jPointsAreConsecutive": "1",
    "iScansNegatively": "0",
    "jScansPositively": "0",
    "alternativeRowScanning": "0",
    "packingType": "grid_simple",
    "numberOfDataPoints": str(NI * NJ),
}


def field(i: int, j: int) -> float:
    """Value at column `i`, row `j` — a position, spelled as a number."""
    return float(10 * j + i)


def oracle(path: pathlib.Path, key: str) -> str:
    """Ask the *pinned* eccodes CLI what it reads back."""
    return subprocess.run(
        ["grib_get", "-p", key, str(path)],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def geoiterator(path: pathlib.Path) -> list[tuple[float, float, float]]:
    """`(lat, lon, value)` per stored point, in stored order, from the pin."""
    out = subprocess.run(
        ["grib_get_data", "-L", "%.4f %.4f", str(path)],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.splitlines()[1:]
    rows = []
    for line in out:
        lat, lon, value = line.split()
        rows.append((float(lat), float(lon), float(value)))
    return rows


def section3_offset(message: bytes) -> int:
    """Byte offset of §3 within a single-message GRIB2 file."""
    off = 16  # past §0
    while True:
        if message[off : off + 4] == b"7777":
            raise SystemExit("message has no §3")
        length = int.from_bytes(message[off : off + 4], "big")
        if message[off + 4] == 3:
            return off
        off += length


def write_boustrophedonic_oracle() -> None:
    """Record what the pin decodes when §3 bit 3 is set on the second-order
    boustrophedonic fixture.

    The reversal run is a *stored* run, so under j-consecutive scanning it is a
    meridian of `Nj` points rather than a parallel of `Ni`. eccodes says so, and
    this writes down what it said: the value sequence here is the one
    `decode_message_values` must return for that patched message. The file this
    is derived from stays committed exactly once — the patch is a single bit,
    and the Rust test applies it in memory the same way.
    """
    original = bytearray(BOUST.read_bytes())
    at = section3_offset(original) + SCANNING_MODE_OCTET - 1
    if original[at] != 0:
        raise SystemExit(f"{BOUST.name}: expected scanning mode 0, found {original[at]}")
    original[at] |= SCAN_J_CONSECUTIVE

    patched = BOUST.with_name("j_consecutive_boust.patched.grib2")
    patched.write_bytes(bytes(original))
    try:
        for key, want in (("jPointsAreConsecutive", "1"), ("Ni", str(BOUST_NI))):
            got = oracle(patched, key)
            if got != want:
                raise SystemExit(f"patched fixture reads {key}={got!r}, expected {want!r}")
        rows = geoiterator(patched)
    finally:
        patched.unlink()

    if len(rows) != BOUST_NI * BOUST_NJ:
        raise SystemExit(f"geoiterator yielded {len(rows)} of {BOUST_NI * BOUST_NJ} points")
    values = [value for _, _, value in rows]

    BOUST_OUT.write_text(
        json.dumps(
            {
                "ni": BOUST_NI,
                "nj": BOUST_NJ,
                "scanningMode": SCAN_J_CONSECUTIVE,
                "tolerance_absolute": 1e-3,
                "values": values,
                "source": (
                    "eccodes 2.34.1 grib_get_data on second_order_boust_regular_latlon.grib2 "
                    "with §3 octet 72 bit 0x20 set in memory (a pure metadata flip). Oracle "
                    "for the second-order boustrophedonic run length under j-consecutive "
                    "scanning: the pin reverses runs of Nj=31, not Ni=16. Bit 4 is clear, so "
                    "the geoiterator applies no row flip and this sequence is the stored "
                    "order decode_message_values must return. Built by "
                    "tools/build_grib2_j_consecutive_fixture.py; provenance in NOTICE.md."
                ),
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    print(
        f"{BOUST_OUT.name}: {len(values)} values from the pin "
        f"({BOUST_NI}x{BOUST_NJ}, bit 3 set in memory)",
        file=sys.stderr,
    )


def main() -> int:
    if not SAMPLE.is_file():
        raise SystemExit(f"{SAMPLE} is missing; install eccodes' sample files")

    with SAMPLE.open("rb") as source:
        handle = ec.codes_grib_new_from_file(source)
    try:
        for key, value in (
            ("Ni", NI),
            ("Nj", NJ),
            ("latitudeOfFirstGridPointInDegrees", LAT_FIRST),
            ("longitudeOfFirstGridPointInDegrees", LON_FIRST),
            ("latitudeOfLastGridPointInDegrees", LAT_FIRST - (NJ - 1) * STEP),
            ("longitudeOfLastGridPointInDegrees", LON_FIRST + (NI - 1) * STEP),
            ("iDirectionIncrementInDegrees", STEP),
            ("jDirectionIncrementInDegrees", STEP),
            ("jPointsAreConsecutive", 1),
            ("bitsPerValue", BITS_PER_VALUE),
        ):
            ec.codes_set(handle, key, value)

        # Stored order under the flag: `i` outer, `j` inner. Writing the ramp in
        # this order is what makes the file's bytes column-major rather than a
        # row-major file with a lying flag.
        values = np.array(
            [field(i, j) for i in range(NI) for j in range(NJ)], dtype=np.float64
        )
        ec.codes_set_values(handle, values)
        OUT.write_bytes(ec.codes_get_message(handle))
    finally:
        ec.codes_release(handle)

    for key, want in EXPECTED.items():
        got = oracle(OUT, key)
        if got != want:
            raise SystemExit(f"{OUT.name}: eccodes reads {key}={got!r}, expected {want!r}")

    # The pin's geoiterator is the oracle the Rust test leans on, so check here
    # that it really does walk this message column by column: the first `Nj`
    # stored points must share a longitude and step in latitude. If a future
    # eccodes stopped honouring the flag, this is where that shows up.
    rows = geoiterator(OUT)
    if len(rows) != NI * NJ:
        raise SystemExit(f"{OUT.name}: geoiterator yielded {len(rows)} of {NI * NJ} points")
    column = rows[:NJ]
    if len({lon for _, lon, _ in column}) != 1:
        raise SystemExit(
            f"{OUT.name}: the first {NJ} stored points are not one meridian — "
            "eccodes is not honouring jPointsAreConsecutive"
        )
    for k, (lat, _, value) in enumerate(rows):
        i, j = divmod(k, NJ)
        want_lat = LAT_FIRST - j * STEP
        if abs(lat - want_lat) > 1e-6 or abs(value - field(i, j)) > 1e-3:
            raise SystemExit(
                f"{OUT.name}: stored point {k} is ({lat}, {value}), "
                f"expected ({want_lat}, {field(i, j)})"
            )

    print(
        f"{OUT.name}: {NI}x{NJ} column-major, {len(rows)} points, "
        f"{OUT.stat().st_size} bytes (verified against the pin)",
        file=sys.stderr,
    )

    write_boustrophedonic_oracle()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
