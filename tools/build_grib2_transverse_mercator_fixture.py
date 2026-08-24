#!/usr/bin/env python3
"""Build the GRIB2 §3.12 (transverse Mercator) test fixture.

    python3 tools/build_grib2_transverse_mercator_fixture.py

Why hand-built rather than a trimmed real file: the template's real-world user
is the Met Office UKV, whose GRIB is not redistributable, and eccodes ships no
§3.12 sample. It *can* encode one, though — `gridDefinitionTemplateNumber = 12`
on the stock `regular_ll_sfc_grib2` sample takes every §3.12 key — so this
writes a small grid carrying the projection parameters a real UKV message
carries, taken from the header quoted in SciTools/iris-grib#140.

What it cannot do is give us the geolocation oracle. eccodes has **no**
transverse-Mercator geoiterator at any version — `codes_grib_iterator_new` on a
§3.12 message answers "Function not yet implemented" at the pinned 2.34.1 and
still at 2.48 — so `grib_get_data` cannot report latitudes and longitudes for
this template the way it does for Lambert or space view. The projection maths is
checked against PROJ instead, in `fieldglass-core`'s own tests; eccodes remains
the oracle for everything else about the message, through the usual
`.eccodes.ref.json` snapshot.

The encoder is the eccodes **2.48 wheel** rather than the pinned 2.34.1 CLI
because only the Python API can set an IEEE-float key (`scaleFactorAtReferencePoint`)
and the four signed corner coordinates in one pass. Nothing about the bytes is
version-specific: the pinned CLI reads every key back, which is what the
snapshot pins.
"""
from __future__ import annotations

import struct
from pathlib import Path

import eccodes
import numpy as np

OUT = Path("crates/fieldglass-grib2/tests/fixtures/transverse_mercator_ukv.grib2")

# British National Grid, which is what UKV is published on: OSGB36's true origin
# at 49°N 2°W, its scale factor, and its false easting and northing.
LAT_REF, LON_REF = 49.0, -2.0
SCALE_FACTOR = 0.9996012717
FALSE_EASTING_M, FALSE_NORTHING_M = 400_000.0, -100_000.0

# Airy 1830, declared the way UKV declares it: shape 3, axes in km with a scale
# factor of 3. That is a millimetre-resolution grid on a metre-resolution field,
# so the axes round to 6 377 563 m and 6 356 257 m — the fixture keeps the
# rounding rather than hiding it, because it is what a reader will meet.
SHAPE_OF_EARTH = 3
MAJOR_AXIS_SCALED, MINOR_AXIS_SCALED = 6_377_563, 6_356_257
AXIS_SCALE_FACTOR = 3

# The real UKV grid is 548 x 704 at 2 km. This is the same *extent* at 48 km, so
# the fixture spans the UK — and so a render of it is worth looking at — while
# staying under a kilobyte.
NI, NJ = 24, 30
D_METRES = 48_000.0
X1_METRES, Y1_METRES = -238_000.0, 1_222_000.0
# `scanningMode = 0`: +i, −j. UKV scans the same way (its `Y2` is below `Y1`).
SCANNING_MODE = 0

# Centimetres are §3.12's unit for every linear field.
CM = 100


def main() -> int:
    handle = eccodes.codes_grib_new_from_samples("regular_ll_sfc_grib2")
    eccodes.codes_set(handle, "gridDefinitionTemplateNumber", 12)
    settings = [
        ("shapeOfTheEarth", SHAPE_OF_EARTH),
        ("scaleFactorOfEarthMajorAxis", AXIS_SCALE_FACTOR),
        ("scaledValueOfEarthMajorAxis", MAJOR_AXIS_SCALED),
        ("scaleFactorOfEarthMinorAxis", AXIS_SCALE_FACTOR),
        ("scaledValueOfEarthMinorAxis", MINOR_AXIS_SCALED),
        ("Ni", NI),
        ("Nj", NJ),
        ("latitudeOfReferencePoint", round(LAT_REF * 1e6)),
        ("longitudeOfReferencePoint", round(LON_REF * 1e6)),
        ("scaleFactorAtReferencePoint", SCALE_FACTOR),
        ("XR", round(FALSE_EASTING_M * CM)),
        ("YR", round(FALSE_NORTHING_M * CM)),
        ("scanningMode", SCANNING_MODE),
        ("Di", round(D_METRES * CM)),
        ("Dj", round(D_METRES * CM)),
        ("X1", round(X1_METRES * CM)),
        ("Y1", round(Y1_METRES * CM)),
        ("X2", round((X1_METRES + (NI - 1) * D_METRES) * CM)),
        ("Y2", round((Y1_METRES - (NJ - 1) * D_METRES) * CM)),
    ]
    for key, value in settings:
        eccodes.codes_set(handle, key, value)

    # A ramp rather than a constant: a constant field survives a transposed or
    # flipped raster unchanged, so it would let a scan-order bug pass. Values
    # are 2-m temperatures in a plausible range.
    eccodes.codes_set(handle, "bitsPerValue", 16)
    ramp = 273.15 + np.arange(NI * NJ, dtype=float) * (20.0 / (NI * NJ))
    eccodes.codes_set_values(handle, ramp)

    message = eccodes.codes_get_message(handle)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_bytes(message)
    stored = struct.unpack(">f", struct.pack(">f", SCALE_FACTOR))[0]
    print(f"wrote {OUT} ({len(message)} bytes)")
    print(f"  {NI}x{NJ} at {D_METRES:.0f} m, scale factor stored as f32 {stored!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
