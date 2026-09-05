#!/usr/bin/env python3
"""Build the GRIB1 reduced-grid second-order fixtures (#605).

Second-order packing is ECMWF's and ECMWF's operational grids are reduced
Gaussian, so the two go together in the wild — but no committed fixture paired
them, which is why the reader could skip the boustrophedonic undo on a reduced
grid and nothing noticed. This builds the pair, repacked from the committed
``reduced_gg_n32_smooth.grib1`` (N32, 64 rows of 20 to 128 points, 6114 total):

- ``reduced_gg_second_order.grib1`` — ``grid_second_order``,
  ``boustrophedonicOrdering = 0``. Stored forwards; eccodes 2.34.1 decodes it,
  so it carries an ``.eccodes.ref.json`` snapshot like any other fixture.
- ``reduced_gg_second_order_boust.grib1`` — the same field with
  ``boustrophedonicOrdering = 1``. ``grib_set`` re-packs when that flag changes,
  so eccodes' own (correct) encoder writes every odd row backwards.

**eccodes' decode is not the oracle for the second one.**
``DataApplyBoustrophedonic::unpack`` has an off-by-one in the branch it takes
when the message has a ``pl`` key: the reversal walks down from
``start + pl[j]`` with a post-decrement where the uniform branch uses
``start + numberOfColumns - 1``, so every odd row lands one slot right and the
last one writes past the end of the buffer. On a 64-row grid that last row is
odd, and eccodes 2.34.1 **segfaults** — ``grib_dump``, ``grib_get_data`` and
``grib_get -p numberOfValues`` all crash, which is why this fixture is on the
``undecodable`` list in ``tools/regenerate-eccodes-snapshots.py``. Its
``pack_double`` is correct (pre-decrement), so the expected decode is taken from
the forward-stored sibling: both files hold the same field.

Usage:  python3 tools/build_grib1_reduced_second_order_fixtures.py
Needs:  eccodes 2.34.1 on PATH (`grib_set`, `grib_get`, `grib_get_data`).
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

from eccodes_oracle import decoded_values, grib_get, grib_set

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


def row_widths(grib_path: Path) -> list[int]:
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


def sample_indices(widths: list[int]) -> list[int]:
    """Four points from every row: its first two, its middle and its last.

    A reversal that steps by the wrong width moves a row's ends furthest, and
    sampling *every* row is what separates "the undo ran" from "the undo ran
    with the right per-row widths" — a uniform-width reversal over a ragged
    grid would leave the early rows right and drift from there.
    """
    picks: list[int] = []
    start = 0
    for width in widths:
        picks += [start, start + 1, start + width // 2, start + width - 1]
        start += width
    return sorted({i for i in picks if 0 <= i < NUM_VALUES})


def write_oracle(oracle_path: Path, values: list[float | None], indices: list[int], note: str):
    present = [v for v in values if v is not None]
    oracle_path.write_text(
        json.dumps(
            {
                "count": len(values),
                "missing_count": sum(1 for v in values if v is None),
                "min": min(present),
                "max": max(present),
                "mean": sum(present) / len(present),
                "samples": {str(i): values[i] for i in indices},
                "tolerance_absolute": 0.001,
                "source": note,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


def main() -> None:
    plain = FIXTURES / "reduced_gg_second_order.grib1"
    grib_set(SOURCE, plain, ["packingType=grid_second_order"])
    assert grib_get(plain, ["packingType"])[0] == "grid_second_order"
    assert int(grib_get(plain, ["boustrophedonicOrdering"])[0]) == 0

    boust = FIXTURES / "reduced_gg_second_order_boust.grib1"
    # No ``-r``. Setting ``boustrophedonicOrdering`` already re-encodes the
    # values (it changes the ``packingType`` concept), and it re-encodes them
    # from the *forward* message, which eccodes decodes correctly. Adding ``-r``
    # asks for a second re-pack, this time of the boustrophedonic message it
    # just wrote — that goes through the broken unpack branch and segfaults.
    grib_set(plain, boust, ["boustrophedonicOrdering=1"], repack=False)
    assert int(grib_get(boust, ["boustrophedonicOrdering"])[0]) == 1
    # The two must differ in their *stored* octets, or eccodes only flipped a
    # flag and the fixture would pass with the undo missing.
    assert plain.read_bytes() != boust.read_bytes(), "eccodes did not re-pack"

    widths = row_widths(plain)
    assert len(widths) == NUM_ROWS and sum(widths) == NUM_VALUES, widths[:4]
    indices = sample_indices(widths)

    values = decoded_values(plain)
    assert len(values) == NUM_VALUES, len(values)

    write_oracle(
        FIXTURES / "reduced_gg_second_order_expected.json",
        values,
        indices,
        "eccodes 2.34.1 grib_get_data. Oracle for grid_second_order on a "
        "reduced N32 Gaussian grid (boustrophedonicOrdering = 0), repacked from "
        "reduced_gg_n32_smooth.grib1 by "
        "tools/build_grib1_reduced_second_order_fixtures.py "
        "(grib_set -r -s packingType=grid_second_order). Provenance in NOTICE.md.",
    )
    write_oracle(
        FIXTURES / "reduced_gg_second_order_boust_expected.json",
        values,
        indices,
        "eccodes 2.34.1 grib_get_data of the *sibling* "
        "reduced_gg_second_order.grib1, which holds the same field stored "
        "forwards. eccodes cannot decode this message at all — its "
        "DataApplyBoustrophedonic pl branch writes past the end of the value "
        "buffer on a 64-row grid and 2.34.1 segfaults (see NOTICE.md) — so its "
        "correct encoder is the oracle instead: grib_set re-packs when "
        "boustrophedonicOrdering changes, storing every odd row backwards. "
        "Built by tools/build_grib1_reduced_second_order_fixtures.py. "
        "Provenance in NOTICE.md.",
    )
    for path in (plain, boust):
        print(f"wrote {path.name} ({path.stat().st_size} bytes)")
    print(f"oracles carry {len(indices)} sampled points over {NUM_ROWS} rows")


if __name__ == "__main__":
    main()
