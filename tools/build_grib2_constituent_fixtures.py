#!/usr/bin/env python3
"""Build GRIB2 fixtures in the chemistry and aerosol product templates (#1130).

`fieldglass-grib2` reads templates 4.40-4.48, which put a constituent type
(and for the aerosol ones a size range, and for 4.48 a wavelength range)
between the parameter and the rest of 4.0's block, so their fixed surfaces sit
at other octets than 4.0's. No committed fixture used any of them except one
real 4.48 (`crates/fieldglass/tests/fixtures/gefs_chem_0p25_pmtc_sfc.grib2`).
These are one file of each, every one with values of its own so a field read
from another template's octets shows:

- `constituent_4_40.grib2` to `constituent_4_46.grib2` and
  `constituent_4_48.grib2`, as the eccodes PyPI wheel writes them.
- Two layouts eccodes does not write as WMO's tables have them:
  - `constituent_4_47.grib2`, template 4.47 as WMO lays it out: the type of
    generating process at octet 12, then the aerosol type at 13-14 and the
    size range at 15-25. eccodes (2.34.1, 2.45.0 and the 2.48 wheel) lays
    4.47 out as 4.46, with the aerosol type at 12-13 and the generating
    process at 25, so the wheel's message has its octets 12-25 rotated here to
    the spec's order. wgrib2 reads 4.47 as the spec does
    (`code_table_4_233_location`).
  - `constituent_4_44_four_octet_time.grib2`, template 4.44 with a four-octet
    forecast time. WMO's 4.44 has two (octets 32-33, 45 octets in all);
    eccodes once wrote four (GRIB-29) and still reads a 4.44 of any other
    length that way (`template.4.forecast_time_44.def`), as
    `fieldglass-grib2` does. Built from the wheel's 4.44 by widening the
    forecast time, with a value two octets cannot hold.

One message per file, because `tools/regenerate-eccodes-snapshots.py` reads
the interval keys through `grib_get`, which it skips for a file where any
message lacks them. All start from the committed `regular_latlon_surface.grib2`
(16 x 31, 2 m temperature) with only §4 changed, so each stays small and the
decode path is the one that fixture already pins.

**Encode with the wheel, verify with the pin.** The eccodes PyPI wheel writes
the templates; the pinned eccodes 2.34.1 CLI on PATH reads every message back
here, and `tools/regenerate-eccodes-snapshots.py` records its answers as the
oracle. For 4.47 the pin is the oracle for the parameter, the fixed surfaces,
the forecast time, the ensemble block and the interval, which it reads from the
spec's octets; it is not for the aerosol type, the size range and the
generating process, which it reads from the wrong ones. The builder asserts
what the pin reads for those too, so the divergence is recorded rather than
assumed.

Usage:  python3 tools/build_grib2_constituent_fixtures.py
Needs:  the `eccodes` PyPI wheel (encoding) and eccodes 2.34.1 on PATH (oracle).
Then:   python3 tools/regenerate-eccodes-snapshots.py --edition 2
"""

from __future__ import annotations

import pathlib
import subprocess
import sys

import eccodes as ec

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)
SOURCE = FIXTURES / "regular_latlon_surface.grib2"

# Keys every message sets, then each template's own. A layer whose top is
# written with a negative scale factor (500 m as 5 x 10^2) exercises the
# sign-magnitude octet at the new offset.
INTERVAL = {"typeOfStatisticalProcessing": 1, "stepRange": "0-6"}
ENSEMBLE = {"typeOfEnsembleForecast": 3, "numberOfForecastsInEnsemble": 30}


def sizes(kind: int, first: tuple[int, int], second: tuple[int, int] | None) -> dict:
    keys = {
        "typeOfSizeInterval": kind,
        "scaleFactorOfFirstSize": first[0],
        "scaledValueOfFirstSize": first[1],
    }
    if second is not None:
        keys["scaleFactorOfSecondSize"] = second[0]
        keys["scaledValueOfSecondSize"] = second[1]
    return keys


# (template, keys). The read-back below checks each key against the pin.
MESSAGES: list[tuple[int, dict]] = [
    (
        40,
        {
            "parameterCategory": 20,
            "parameterNumber": 0,
            "constituentType": 5,  # nitrogen dioxide
            "typeOfFirstFixedSurface": 105,
            "scaleFactorOfFirstFixedSurface": 0,
            "scaledValueOfFirstFixedSurface": 1,
            "forecastTime": 3,
        },
    ),
    (
        41,
        {
            "parameterCategory": 20,
            "parameterNumber": 2,
            "constituentType": 0,  # ozone
            "typeOfFirstFixedSurface": 100,
            "scaleFactorOfFirstFixedSurface": 0,
            "scaledValueOfFirstFixedSurface": 50000,
            "perturbationNumber": 4,
            "forecastTime": 6,
            **ENSEMBLE,
        },
    ),
    (
        42,
        {
            "parameterCategory": 20,
            "parameterNumber": 1,
            "constituentType": 4,  # carbon monoxide
            "typeOfFirstFixedSurface": 1,
            **INTERVAL,
        },
    ),
    (
        43,
        {
            "parameterCategory": 20,
            "parameterNumber": 1,
            "constituentType": 8,  # sulphur dioxide
            "typeOfFirstFixedSurface": 103,
            "scaleFactorOfFirstFixedSurface": -2,
            "scaledValueOfFirstFixedSurface": 5,
            "typeOfSecondFixedSurface": 103,
            "scaleFactorOfSecondFixedSurface": 0,
            "scaledValueOfSecondFixedSurface": 10,
            "perturbationNumber": 9,
            **ENSEMBLE,
            **INTERVAL,
        },
    ),
    (
        44,
        {
            "parameterCategory": 13,
            "parameterNumber": 192,
            "constituentType": 62001,  # dust dry
            **sizes(0, (6, 10), None),
            "typeOfFirstFixedSurface": 1,
            "forecastTime": 12,
        },
    ),
    (
        45,
        {
            "parameterCategory": 20,
            "parameterNumber": 0,
            "constituentType": 62008,  # sea salt dry
            **sizes(2, (7, 2), (6, 2)),
            "typeOfFirstFixedSurface": 105,
            "scaleFactorOfFirstFixedSurface": 0,
            "scaledValueOfFirstFixedSurface": 2,
            "perturbationNumber": 11,
            "forecastTime": 9,
            **ENSEMBLE,
        },
    ),
    (
        46,
        {
            "parameterCategory": 20,
            "parameterNumber": 1,
            "constituentType": 62006,  # sulphate dry
            **sizes(0, (7, 25), None),
            "typeOfFirstFixedSurface": 10,
            **INTERVAL,
        },
    ),
    (
        48,
        {
            "parameterCategory": 20,
            "parameterNumber": 102,
            "constituentType": 62000,  # total aerosol
            **sizes(0, (5, 2), None),
            "typeOfWavelengthInterval": 7,
            "scaleFactorOfFirstWavelength": 9,
            "scaledValueOfFirstWavelength": 545,
            "scaleFactorOfSecondWavelength": 9,
            "scaledValueOfSecondWavelength": 565,
            "typeOfFirstFixedSurface": 10,
            "forecastTime": 24,
        },
    ),
]

SPEC_4_47 = {
    "parameterCategory": 20,
    "parameterNumber": 1,
    "typeOfGeneratingProcess": 4,  # ensemble forecast
    "constituentType": 62009,  # black carbon dry
    **sizes(2, (8, 1), (6, 1)),
    "typeOfFirstFixedSurface": 103,
    "scaleFactorOfFirstFixedSurface": 0,
    "scaledValueOfFirstFixedSurface": 2,
    "perturbationNumber": 6,
    **ENSEMBLE,
    **INTERVAL,
}

# A 4.44 forecast time two octets cannot hold: 70,000 minutes.
LEGACY_4_44 = {
    "parameterCategory": 13,
    "parameterNumber": 193,
    "constituentType": 62008,
    **sizes(0, (7, 25), None),
    "typeOfFirstFixedSurface": 1,
    "indicatorOfUnitOfTimeRange": 0,
    "forecastTime": 1000,
}
LEGACY_FORECAST_MINUTES = 70_000

# What the pin must read back from each built file, per message. For 4.47 the
# aerosol keys are what it reads from 4.46's octets on a message laid out as
# the spec says: the generating process (4) is the type's high octet, so
# `constituentType` is 4 x 256 + 0xF2 (62009 >> 8) = 1266, and so on down the
# range. Asserting them pins the divergence the NOTICE records.
SPEC_4_47_PIN = {
    "productDefinitionTemplateNumber": "47",
    "parameterCategory": "20",
    "parameterNumber": "1",
    "typeOfFirstFixedSurface": "103",
    "scaledValueOfFirstFixedSurface": "2",
    "perturbationNumber": "6",
    "numberOfForecastsInEnsemble": "30",
    "typeOfStatisticalProcessing": "1",
    "lengthOfTimeRange": "6",
    "constituentType": "1266",
    "typeOfGeneratingProcess": "1",
}
LEGACY_4_44_PIN = {
    "productDefinitionTemplateNumber": "44",
    "section4Length": "47",
    "parameterCategory": "13",
    "parameterNumber": "193",
    "constituentType": "62008",
    "scaledValueOfFirstSize": "25",
    "indicatorOfUnitOfTimeRange": "0",
    "forecastTime": str(LEGACY_FORECAST_MINUTES),
    "typeOfFirstFixedSurface": "1",
}


def oracle(path: pathlib.Path, key: str) -> list[str]:
    """Ask the *pinned* eccodes CLI what it reads back, one line per message."""
    out = subprocess.run(
        # `:i` asks for the code rather than a name such as `sfc`; `-f` prints
        # `not_found` for a message without the key rather than stopping.
        ["grib_get", "-f", "-p", key if key == "stepRange" else f"{key}:i", str(path)],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout
    return [line.strip() for line in out.splitlines() if line.strip()]


def encode(source: int, template: int, keys: dict) -> bytes:
    """One message from the wheel: `source` with §4 set to `template` and `keys`."""
    h = ec.codes_clone(source)
    try:
        ec.codes_set(h, "productDefinitionTemplateNumber", template)
        for key, value in keys.items():
            ec.codes_set(h, key, value)
        return ec.codes_get_message(h)
    finally:
        ec.codes_release(h)


def section_4(message: bytes) -> tuple[int, int]:
    """Byte offset and length of §4 in a single-message GRIB2 buffer."""
    at = 16  # §0
    while at < len(message) - 4:
        length = int.from_bytes(message[at : at + 4], "big")
        if message[at + 4] == 4:
            return at, length
        at += length
    raise SystemExit("no section 4")


def with_section_4(message: bytes, at: int, old_len: int, new: bytes) -> bytes:
    """`message` with the §4 at `at` replaced by `new`, and §0's length fixed."""
    out = bytearray(message[:at] + new + message[at + old_len :])
    out[8:16] = len(out).to_bytes(8, "big")
    return bytes(out)


def spec_4_47(source: int) -> bytes:
    """The wheel's 4.47 with octets 12-25 rotated into WMO's order."""
    message = encode(source, 47, SPEC_4_47)
    at, length = section_4(message)
    sec = bytearray(message[at : at + length])
    # Octet n is index n - 1. The wheel writes aerosol type and size range at
    # 12-24 and the generating process at 25; WMO has the process at 12.
    eccodes_order = bytes(sec[11:25])
    if eccodes_order[13] != SPEC_4_47["typeOfGeneratingProcess"]:
        raise SystemExit("the wheel's 4.47 no longer puts the process at octet 25")
    sec[11:25] = eccodes_order[13:] + eccodes_order[:13]
    return with_section_4(message, at, length, bytes(sec))


def legacy_4_44(source: int) -> bytes:
    """The wheel's 4.44 with its forecast time widened to four octets."""
    message = encode(source, 44, LEGACY_4_44)
    at, length = section_4(message)
    sec = message[at : at + length]
    if length != 45:
        raise SystemExit(f"the wheel's 4.44 is {length} octets, not WMO's 45")
    # Octets 32-33 are the forecast time; four sign-magnitude octets replace
    # them, and the section grows by two.
    wide = sec[:31] + LEGACY_FORECAST_MINUTES.to_bytes(4, "big") + sec[33:]
    wide = (len(wide)).to_bytes(4, "big") + wide[4:]
    return with_section_4(message, at, length, wide)


def check(path: pathlib.Path, expected: list[dict[str, str]]) -> bool:
    ok = True
    for key in sorted({k for e in expected for k in e}):
        got = oracle(path, key)
        for i, want in enumerate(expected):
            if key in want and (i >= len(got) or got[i] != want[key]):
                seen = got[i] if i < len(got) else None
                print(f"{path.name}[{i}]: {key} reads back {seen!r}, expected {want[key]!r}",
                      file=sys.stderr)
                ok = False
    return ok


def as_text(keys: dict) -> dict[str, str]:
    # `stepRange` is a derived key the pin prints as written; the rest are ints.
    return {k: str(v) for k, v in keys.items()}


def main() -> int:
    # (file, bytes, what the pin must read back)
    builds: list[tuple[str, bytes, dict[str, str]]] = []
    with SOURCE.open("rb") as f:
        source = ec.codes_grib_new_from_file(f)
    try:
        for template, keys in MESSAGES:
            builds.append(
                (
                    f"constituent_4_{template}.grib2",
                    encode(source, template, keys),
                    {"productDefinitionTemplateNumber": str(template), **as_text(keys)},
                )
            )
        builds.append(("constituent_4_47.grib2", spec_4_47(source), SPEC_4_47_PIN))
        builds.append(
            ("constituent_4_44_four_octet_time.grib2", legacy_4_44(source), LEGACY_4_44_PIN)
        )
    finally:
        ec.codes_release(source)

    ok = True
    for name, message, expected in builds:
        out = FIXTURES / name
        out.write_bytes(message)
        ok &= check(out, [expected])
        print(f"wrote {out.relative_to(FIXTURES.parents[3])} ({out.stat().st_size} bytes)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
