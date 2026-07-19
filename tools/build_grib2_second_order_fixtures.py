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

FIXTURES = (
    Path(__file__).resolve().parent.parent
    / "crates"
    / "fieldglass-grib2"
    / "tests"
    / "fixtures"
)
SOURCE = FIXTURES / "regular_latlon_surface.grib2"
NUM_VALUES = 16 * 31  # 496


def grib_set(src: Path, dst: Path, sets: list[str]) -> None:
    subprocess.run(
        ["grib_set", "-r", *sum((["-s", s] for s in sets), []), str(src), str(dst)],
        capture_output=True,
        text=True,
        check=True,
    )


def grib_set_no_repack(src: Path, dst: Path, sets: list[str]) -> None:
    # `-r` re-packs the data section; for a pure metadata flip (setting the
    # boustrophedonic flag on an already-second-order message) we must NOT
    # re-pack, so eccodes reorders the stored data on decode instead.
    subprocess.run(
        ["grib_set", *sum((["-s", s] for s in sets), []), str(src), str(dst)],
        capture_output=True,
        text=True,
        check=True,
    )


def grib_get(path: Path, keys: list[str]) -> list[str]:
    out = subprocess.run(
        ["grib_get", "-p", ",".join(keys), str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    return out.stdout.split()


def decoded_values(path: Path) -> list[float | None]:
    out = subprocess.run(
        ["grib_get_data", "-m", "9999", str(path)],
        capture_output=True,
        text=True,
        check=True,
    )
    vals: list[float | None] = []
    for line in out.stdout.strip().splitlines()[1:]:
        v = line.split()[2]
        vals.append(None if v == "9999" else float(v))
    return vals


def write_oracle(
    grib_path: Path, oracle_path: Path, template: int, sample_indices: list[int], note: str
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
    vals = decoded_values(grib_path)
    assert len(vals) == NUM_VALUES, (len(vals), NUM_VALUES)
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
    oracle_path.write_text(json.dumps(oracle, indent=2) + "\n")
    print(
        f"wrote {grib_path.name} ({grib_path.stat().st_size} bytes) + oracle "
        f"[{packing}, NG={ng}, SPD={spd}, boust={boust}]"
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
    grib_set_no_repack(so2, sob, ["secondOrderFlags=128"])
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


if __name__ == "__main__":
    main()
