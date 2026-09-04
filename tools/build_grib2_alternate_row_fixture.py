#!/usr/bin/env python3
"""Build the GRIB2 alternate-row-scanning (boustrophedon) test fixture.

    python3 tools/build_grib2_alternate_row_fixture.py

Why this fixture exists: §3 Flag Table 3.4 bit 4 ("adjacent rows scan in the
opposite direction") is the one scanning-mode flag a decoder cannot fold into
the projection — the field itself has to be un-scrambled. The real-world case is
the National Blend of Models, whose Lambert 2 m field carries
``scanningMode = 80`` (bit 6 j-positive + bit 4 alternate rows), but NBM grids
are ~2345 x 1597 and not something to commit. This writes the same scanning mode
on the same projection at 6 x 5.

Why Lambert rather than the cheaper regular lat/lon: the **oracle**. eccodes'
``values`` key is the field in storage order — it does *not* undo alternate-row
scanning, which is the separate, opt-in ``swapScanningAlternativeRows`` key. The
geoiterator does, in ``transform_iterator_data``, so ``grib_get_data`` prints
the regularised order. But only for the projections that route through it: the
lat/lon iterator states the assumption ``alternativeRowScanning == 0`` in its own
source and ignores the flag, so a ``regular_ll`` fixture would report an order
that is not an oracle for anything. ``LambertConformal`` calls
``transform_iterator_data``; this was checked by building both and comparing.

``scanningMode = 80`` also makes that transform *exactly* the row flip and
nothing else: ``jScansPositively`` is set and ``iScansNegatively`` is clear, so
the j-flip and i-flip branches of ``pointer_to_data`` are identities and what is
left is ``i = nx - 1 - i when the storage row is odd``.

The values are ``1..Ni*Nj`` in storage order, so the expected file reads as the
permutation itself rather than as physical data — a wrong parity or a
row-expansion off-by-one is visible by eye in the diff.

The encoder is the eccodes **2.48 wheel** (the pinned 2.34.1 CLI cannot set the
§3.30 keys and the values array in one pass); the oracle is the **pinned 2.34.1
CLI**, which is what the rest of the corpus is pinned against. Nothing about the
bytes is version-specific.

Outputs:
  crates/fieldglass-grib2/tests/fixtures/alternate_row_lambert.grib2
  crates/fieldglass-grib2/tests/fixtures/alternate_row_lambert_expected.json

Run ``tools/regenerate-eccodes-snapshots.py`` afterwards for the metadata
snapshot. Requires: the ``eccodes`` PyPI wheel, and ``grib_get_data`` on PATH.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

import eccodes
import numpy as np

FIXTURES = Path("crates/fieldglass-grib2/tests/fixtures")
OUT = FIXTURES / "alternate_row_lambert.grib2"
ORACLE = FIXTURES / "alternate_row_lambert_expected.json"

# Small enough to read in a diff, large enough that the flip is unambiguous:
# an even row count would leave the last row unflipped, so five rows exercise
# both parities at both ends.
NI, NJ = 6, 5

# CONUS-ish Lambert, the projection NBM is published on, coarsened to 200 km so
# the whole grid is a few hundred bytes.
LAT1, LON1 = 30.0, 260.0
LAD, LOV = 38.5, 262.5
LATIN1 = LATIN2 = 38.5
D_METRES = 200_000.0

# §3 Flag Table 3.4: bit 6 (0x40) j scans positively, bit 4 (0x10) adjacent rows
# scan in opposite directions. This is NBM's own scanning mode.
SCANNING_MODE = 0x40 | 0x10


def main() -> int:
    handle = eccodes.codes_grib_new_from_samples("regular_ll_sfc_grib2")
    eccodes.codes_set(handle, "gridDefinitionTemplateNumber", 30)
    settings = [
        ("shapeOfTheEarth", 6),
        ("Nx", NI),
        ("Ny", NJ),
        ("latitudeOfFirstGridPointInDegrees", LAT1),
        ("longitudeOfFirstGridPointInDegrees", LON1),
        ("LaDInDegrees", LAD),
        ("LoVInDegrees", LOV),
        ("DxInMetres", D_METRES),
        ("DyInMetres", D_METRES),
        ("Latin1InDegrees", LATIN1),
        ("Latin2InDegrees", LATIN2),
        ("latitudeOfSouthernPoleInDegrees", 0.0),
        ("longitudeOfSouthernPoleInDegrees", 0.0),
        ("projectionCentreFlag", 0),
        ("scanningMode", SCANNING_MODE),
        ("bitsPerValue", 8),
    ]
    for key, value in settings:
        eccodes.codes_set(handle, key, value)

    # `codes_set_values` writes the array verbatim — it applies no scanning-mode
    # transform of its own (that is `swapScanningAlternativeRows`, which this
    # never packs) — so these land as the stored field, in scan order.
    stored = np.arange(1.0, NI * NJ + 1.0)
    eccodes.codes_set_values(handle, stored)

    message = eccodes.codes_get_message(handle)
    FIXTURES.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(message)

    # The oracle: eccodes' own geoiterator order, read back through the pinned
    # CLI. Column 3 of `grib_get_data` is the value; the row order it prints is
    # the raster order a caller expects.
    dump = subprocess.run(
        ["grib_get_data", str(OUT)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    )
    lines = dump.stdout.splitlines()[1:]  # drop the "Latitude Longitude Value" header
    regularised = [float(line.split()[2]) for line in lines if line.strip()]
    if len(regularised) != NI * NJ:
        raise SystemExit(f"grib_get_data returned {len(regularised)} points, want {NI * NJ}")
    if regularised == list(stored):
        raise SystemExit("grib_get_data returned the storage order — this is not an oracle")

    version = subprocess.run(
        ["grib_get_data", "-V"], capture_output=True, text=True, encoding="utf-8"
    ).stdout.strip()
    ORACLE.write_text(
        json.dumps(
            {
                "_comment": (
                    "Values of alternate_row_lambert.grib2 in raster (row-major, "
                    "west-to-east) order, which is what decode_message_values must "
                    "return. Oracle: grib_get_data from the pinned eccodes CLI, whose "
                    "geoiterator applies the alternate-row flip; the stored field is "
                    "1..30 in scan order. Regenerate with "
                    "tools/build_grib2_alternate_row_fixture.py."
                ),
                "eccodesOracle": version,
                "eccodesEncoder": f"ecCodes Version {eccodes.codes_get_api_version()}",
                "ni": NI,
                "nj": NJ,
                "scanningMode": SCANNING_MODE,
                "stored": list(stored),
                "regularised": regularised,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )

    print(f"wrote {OUT} ({len(message)} bytes), {NI}x{NJ} scanningMode {SCANNING_MODE}")
    print(f"wrote {ORACLE}")
    for j in range(NJ):
        print(f"  row {j}: {[int(v) for v in regularised[j * NI:(j + 1) * NI]]}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
