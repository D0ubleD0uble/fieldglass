#!/usr/bin/env python3
"""Build the GRIB2 second-order packing (DRS templates 5.50001 / 5.50002) test
fixtures (#307).

Unlike run-length (5.200), eccodes 2.34.1 (the pinned oracle) *can* encode
second-order packing from the CLI, so these fixtures are produced by repacking
the committed ``regular_latlon_surface.grib2`` (a 16x31 = 496-point regular
lat/lon surface field) rather than hand-built:

- ``second_order_regular_latlon.grib2`` — ``grid_second_order`` (template
  5.50002, boustrophedonicOrdering = 0). The common operational case.
- ``second_order_no_boust_regular_latlon.grib2`` —
  ``grid_second_order_no_boustrophedonic`` (template 5.50001). Same field, no
  ``secondOrderFlags`` octet; decodes identically.
- ``second_order_boust_regular_latlon.grib2`` — the 5.50002 fixture with
  ``secondOrderFlags`` set to 0x80 (boustrophedonicOrdering = 1). eccodes then
  reverses the odd rows on decode, so this exercises the alternating-row path.
  The value oracle is eccodes' own decode of that reordered field.
- ``second_order_reduced_gaussian.grib2`` /
  ``second_order_boust_reduced_gaussian.grib2`` — the same packing on a *reduced*
  N32 Gaussian grid, forwards and boustrophedonic (#605). Built from
  ``reduced_gaussian_pressure_level.grib2``; see ``build_reduced_pair`` for why
  eccodes' decode of the boustrophedonic one is not its oracle.

Each ``.grib2`` gets a sibling ``*_expected.json`` value oracle produced from
eccodes ``grib_get_data`` / ``grib_get``. The ``.eccodes.ref.json`` metadata
snapshots are produced separately by ``tools/regenerate-eccodes-snapshots.py``.

Usage:
    python3 tools/build_grib2_second_order_fixtures.py
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from eccodes_oracle import decoded_values, grib_get, grib_set

FIXTURES = (
    Path(__file__).resolve().parent.parent
    / "crates"
    / "fieldglass-grib2"
    / "tests"
    / "fixtures"
)
SOURCE = FIXTURES / "regular_latlon_surface.grib2"
NUM_VALUES = 16 * 31  # 496
#: The reduced Gaussian source for the #605 pair — an N32 reduced grid whose 64
#: rows run 20 to 128 points wide, so a boustrophedonic reversal has to step by
#: ``PL[j]`` rather than by any single column count.
REDUCED_SOURCE = FIXTURES / "reduced_gaussian_pressure_level.grib2"
REDUCED_NUM_VALUES = 6114


def write_oracle(
    grib_path: Path,
    oracle_path: Path,
    template: int,
    sample_indices: list[int],
    note: str,
    *,
    num_values: int = NUM_VALUES,
    values_from: Path | None = None,
) -> None:
    keys = [
        "packingType",
        "bitsPerValue",
        "numberOfGroups",
        "widthOfFirstOrderValues",
        "widthOfWidths",
        "widthOfLengths",
        "orderOfSPD",
        "widthOfSPD",
    ]
    got = grib_get(grib_path, keys)
    packing = got[0]
    bits, ng, wfo, ww, wl, spd, wspd = (int(x) for x in got[1:])
    boust = int(grib_get(grib_path, ["boustrophedonicOrdering"])[0]) if template == 50002 else 0
    # ``values_from`` names a *different* message to take the expected decode
    # from. The reduced boustrophedonic fixture needs it: eccodes' own decode of
    # that message is wrong (see ``build_reduced_pair``), while its
    # non-boustrophedonic sibling — the same field, stored forwards — decodes
    # correctly and is what the fixture must come back as.
    vals = decoded_values(values_from or grib_path)
    assert len(vals) == num_values, (len(vals), num_values)
    present = [v for v in vals if v is not None]
    oracle = {
        "count": len(vals),
        "missing_count": sum(1 for v in vals if v is None),
        "min": min(present),
        "max": max(present),
        "mean": sum(present) / len(present),
        "samples": {str(i): vals[i] for i in sample_indices},
        # Full eccodes decode, in scan order, for value-for-value validation
        # (missing points are null). This is the primary oracle; the samples and
        # summary stats above are redundant cross-checks.
        "values": vals,
        "tolerance_absolute": 0.001,
        "section5": {
            "dataRepresentationTemplateNumber": template,
            "packingType": packing,
            "bitsPerValue": bits,
            "numberOfGroups": ng,
            "widthOfFirstOrderValues": wfo,
            "widthOfWidths": ww,
            "widthOfLengths": wl,
            "orderOfSPD": spd,
            "widthOfSPD": wspd,
            "boustrophedonicOrdering": boust,
        },
        "source": note,
    }
    oracle_path.write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    print(
        f"wrote {grib_path.name} ({grib_path.stat().st_size} bytes) + oracle "
        f"[{packing}, NG={ng}, SPD={spd}, boust={boust}]"
    )


def reduced_row_widths(grib_path: Path) -> list[int]:
    """``PL`` — the points in each row of a reduced grid.

    ``grib_get -p pl`` refuses an array key, so this reads the ``pl`` block out
    of ``grib_dump -O`` instead.
    """
    dump = subprocess.run(
        ["grib_dump", "-O", str(grib_path)],
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    ).stdout
    start = dump.index("{", dump.index("pl = ("))
    body = dump[start + 1 : dump.index("}", start)]
    return [int(float(x)) for x in body.replace("\n", "").split(",") if x.strip()]


def build_reduced_pair() -> None:
    """The reduced-grid second-order pair for #605.

    Second-order packing is ECMWF's and ECMWF's operational grids are reduced
    Gaussian, so the two go together in the wild — but no fixture paired them,
    which is why both readers could skip the boustrophedonic undo on a reduced
    grid and nothing noticed.

    **eccodes' decode is not the oracle here.** Its
    ``DataApplyBoustrophedonic::unpack`` has an off-by-one in the branch it
    takes when the message has a ``pl`` key: the reversal walks down from
    ``start + pl[j]`` with a post-decrement where the uniform branch uses
    ``start + numberOfColumns - 1``, so every odd row lands one slot to the
    right and the last one writes past the end of the buffer. On this GRIB2
    message that shifts 3028 of 6114 points (2.34.1 and 2.48.0 alike); the
    GRIB1 equivalent segfaults. Its *encoder* is correct — ``pack_double`` uses
    a pre-decrement — so the fixture is built by asking eccodes to **write** the
    boustrophedonic form and taking the expected decode from the forward-stored
    sibling. Both files hold the same field.
    """
    plain = FIXTURES / "second_order_reduced_gaussian.grib2"
    grib_set(REDUCED_SOURCE, plain, ["packingType=grid_second_order"])
    assert int(grib_get(plain, ["boustrophedonicOrdering"])[0]) == 0

    boust = FIXTURES / "second_order_boust_reduced_gaussian.grib2"
    grib_set(plain, boust, ["boustrophedonicOrdering=1"])
    assert int(grib_get(boust, ["boustrophedonicOrdering"])[0]) == 1

    # The two must differ in their *stored* octets, or eccodes only flipped a
    # flag and the fixture would pass with the undo missing.
    assert plain.read_bytes() != boust.read_bytes(), "eccodes did not re-pack"

    # Sample the points a row-stepping bug moves: the first, second and last of
    # the first few rows, both parities, plus the ends of the field.
    widths = reduced_row_widths(plain)
    assert len(widths) == 64 and sum(widths) == REDUCED_NUM_VALUES, widths[:4]
    samples, start = [], 0
    for width in widths[:6]:
        samples += [start, start + 1, start + width - 1]
        start += width
    samples += [0, REDUCED_NUM_VALUES - 1]

    write_oracle(
        boust,
        FIXTURES / "second_order_boust_reduced_gaussian_expected.json",
        50002,
        sorted(set(samples)),
        "eccodes 2.34.1 grib_get_data of the *sibling* "
        "second_order_reduced_gaussian.grib2, which holds the same field stored "
        "forwards. eccodes' own decode of this message is not a valid oracle: "
        "its DataApplyBoustrophedonic pl branch is off by one and shifts every "
        "odd row (see NOTICE.md). Built from reduced_gaussian_pressure_level.grib2 "
        "by tools/build_grib2_second_order_fixtures.py. Provenance in NOTICE.md.",
        num_values=REDUCED_NUM_VALUES,
        values_from=plain,
    )


def main() -> None:
    samples = [0, 1, 15, 16, 17, 31, 32, 247, 248, 480, 494, 495]

    # 5.50002 — grid_second_order (the common case, boustrophedonicOrdering=0).
    so2 = FIXTURES / "second_order_regular_latlon.grib2"
    grib_set(SOURCE, so2, ["packingType=grid_second_order"])
    write_oracle(
        so2,
        FIXTURES / "second_order_regular_latlon_expected.json",
        50002,
        samples,
        "eccodes 2.34.1 grib_get_data + grib_get. Oracle for DRS template "
        "5.50002 (grid_second_order, boustrophedonicOrdering=0). Repacked from "
        "regular_latlon_surface.grib2 by tools/build_grib2_second_order_fixtures.py "
        "(grib_set -r -s packingType=grid_second_order). Provenance in NOTICE.md.",
    )

    # 5.50001 — grid_second_order_no_boustrophedonic (no secondOrderFlags octet).
    so1 = FIXTURES / "second_order_no_boust_regular_latlon.grib2"
    grib_set(SOURCE, so1, ["packingType=grid_second_order_no_boustrophedonic"])
    write_oracle(
        so1,
        FIXTURES / "second_order_no_boust_regular_latlon_expected.json",
        50001,
        samples,
        "eccodes 2.34.1 grib_get_data + grib_get. Oracle for DRS template "
        "5.50001 (grid_second_order_no_boustrophedonic). Repacked from "
        "regular_latlon_surface.grib2 by tools/build_grib2_second_order_fixtures.py "
        "(grib_set -r -s packingType=grid_second_order_no_boustrophedonic). "
        "Provenance in NOTICE.md.",
    )

    # 5.50002 with boustrophedonicOrdering=1 — flip the flag WITHOUT re-packing
    # so eccodes reverses the odd rows on decode. The oracle is that reordered
    # decode, exercising the alternating-row path.
    sob = FIXTURES / "second_order_boust_regular_latlon.grib2"
    grib_set(so2, sob, ["secondOrderFlags=128"], repack=False)
    assert int(grib_get(sob, ["boustrophedonicOrdering"])[0]) == 1
    write_oracle(
        sob,
        FIXTURES / "second_order_boust_regular_latlon_expected.json",
        50002,
        samples,
        "eccodes 2.34.1 grib_get_data + grib_get. Oracle for DRS template "
        "5.50002 with boustrophedonicOrdering=1. Derived from "
        "second_order_regular_latlon.grib2 by setting secondOrderFlags=128 "
        "(grib_set WITHOUT -r, a pure metadata flip) so eccodes reverses the "
        "odd rows on decode. Built by tools/build_grib2_second_order_fixtures.py. "
        "Provenance in NOTICE.md.",
    )

    build_reduced_pair()


if __name__ == "__main__":
    main()
