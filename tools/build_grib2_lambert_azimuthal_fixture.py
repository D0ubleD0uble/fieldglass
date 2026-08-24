#!/usr/bin/env python3
"""Build the GRIB2 §3.140 (Lambert azimuthal equal-area) test fixture.

    python3 tools/build_grib2_lambert_azimuthal_fixture.py

Hand-built because eccodes ships no §3.140 sample and neither the CEMS/EFAS
archive nor the OSI SAF products are redistributable here. eccodes encodes the
template happily — `gridDefinitionTemplateNumber = 140` on the stock
`regular_ll_sfc_grib2` sample takes every key — and, unlike §3.12, it also
*geolocates* it: `lambert_azimuthal_equal_area` is a real geoiterator, so
`grib_get_data` is the oracle for this one after all.

The parameters are ETRS89-LAEA's (EPSG:3035, the European statistical grid the
EFAS archive is published on): the tangent point at 52degN 10degE on GRS80. The
grid is 20 x 16 at 200 km rather than the real 5 km, which keeps the fixture
under a kilobyte while still spanning Europe.

The encoder is the eccodes **2.48** wheel rather than the pinned 2.34.1 CLI
because only the Python API sets the template's keys in one pass. Nothing about
the bytes is version-specific, and the pinned CLI reads every key back — that is
what `transverse_mercator_ukv.grib2.eccodes.ref.json`'s sibling snapshot pins.

One caveat worth knowing before you change the tangent point: eccodes cannot
geolocate a **south-polar** §3.140 at all. Its polar-aspect guard tests
`cosb1 == 0.0`, which holds at +90deg but not at -90deg, and the projected plane
inflates by eight orders of magnitude; `codes_get_array(h, "latitudes")` answers
`Invalid value: arcsin argument=7.60531e+06` and refuses. That case is checked
against PROJ in `fieldglass-core` instead.
"""
from __future__ import annotations

from pathlib import Path

import eccodes
import numpy as np

OUT = Path("crates/fieldglass-grib2/tests/fixtures/lambert_azimuthal_efas.grib2")

# ETRS89-LAEA (EPSG:3035): tangent at 52degN 10degE on GRS80.
STANDARD_PARALLEL, CENTRAL_LONGITUDE = 52.0, 10.0
# GRS80 comes from a fixed shape code, so the axes are exact — no scaled-value
# rounding of the kind the §3.12 fixture carries.
SHAPE_OF_EARTH = 4

# Same extent as the EFAS domain, two orders of magnitude coarser.
NX, NY = 20, 16
D_METRES = 200_000.0
LAT_FIRST, LON_FIRST = 35.0, -10.0
# `scanningMode = 64`: +i, +j. Chosen to differ from the §3.12 fixture's mode 0
# so the two together cover both j directions through the real parser.
SCANNING_MODE = 64

MICRODEGREES = 1e6
MILLIMETRES = 1000


def main() -> int:
    handle = eccodes.codes_grib_new_from_samples("regular_ll_sfc_grib2")
    eccodes.codes_set(handle, "gridDefinitionTemplateNumber", 140)
    for key, value in [
        ("shapeOfTheEarth", SHAPE_OF_EARTH),
        ("numberOfPointsAlongXAxis", NX),
        ("numberOfPointsAlongYAxis", NY),
        ("latitudeOfFirstGridPoint", round(LAT_FIRST * MICRODEGREES)),
        ("longitudeOfFirstGridPoint", round(LON_FIRST * MICRODEGREES)),
        ("standardParallelInMicrodegrees", round(STANDARD_PARALLEL * MICRODEGREES)),
        ("centralLongitudeInMicrodegrees", round(CENTRAL_LONGITUDE * MICRODEGREES)),
        ("xDirectionGridLengthInMillimetres", round(D_METRES * MILLIMETRES)),
        ("yDirectionGridLengthInMillimetres", round(D_METRES * MILLIMETRES)),
        ("scanningMode", SCANNING_MODE),
    ]:
        eccodes.codes_set(handle, key, value)

    # A ramp, for the same reason as the §3.12 fixture: a constant field would
    # survive a transposed or flipped raster and let a scan-order bug pass.
    eccodes.codes_set(handle, "bitsPerValue", 16)
    eccodes.codes_set_values(handle, 250.0 + np.arange(NX * NY, dtype=float) * 0.1)

    message = eccodes.codes_get_message(handle)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(message)
    print(f"wrote {OUT} ({len(message)} bytes)")

    # Print the corner geolocations eccodes reports, which is what the Rust
    # tests pin. Read through the array keys rather than `grib_get_data`, whose
    # three decimal places are only 111 m of resolution.
    check = eccodes.codes_new_from_message(message)
    lats = eccodes.codes_get_array(check, "latitudes")
    lons = eccodes.codes_get_array(check, "longitudes")
    for i, j in [(0, 0), (NX - 1, 0), (0, NY - 1), (NX - 1, NY - 1)]:
        k = j * NX + i
        lon = lons[k] - 360.0 if lons[k] > 180.0 else lons[k]
        print(f"  ({i:2d}, {j:2d}) -> {lats[k]:.9f}, {lon:.9f}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
