#!/usr/bin/env python3
"""Build the GRIB2 log-preprocessing packing (DRS template 5.61) fixtures (#305).

Template 5.61 (simple packing with logarithmic pre-processing) is decoded as
ordinary simple unpacking followed by `Y = exp(X) - B`, where `B` is the §5
`preProcessingParameter`. eccodes 2.34.1 (the pin) both encodes and decodes
`grid_simple_log_preprocessing`, so these fixtures are eccodes re-encodings of
the committed `regular_latlon_surface.grib2` field and eccodes' decode is the
value oracle (the same round-trip pattern used for the CCSDS fixtures).

Two fixtures cover both branches of the inverse transform:

- ``log_regular_latlon.grib2`` — the all-positive temperature field, so the
  encoder sets ``preProcessingParameter = 0`` and decode is ``Y = exp(X)``.
- ``log_negative_regular_latlon.grib2`` — the same field shifted by −300 K so
  it holds non-positive values, which drives the encoder to a non-zero
  ``preProcessingParameter`` and exercises the ``Y = exp(X) − B`` branch.

Each ``.grib2`` gets a sibling ``*_expected.json`` value oracle from eccodes
``grib_get_data`` / ``grib_get``. The ``.eccodes.ref.json`` metadata snapshots
are produced by ``tools/regenerate-eccodes-snapshots.py``.

Usage:
    python3 tools/build_grib2_log_preprocessing_fixtures.py
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


def run(cmd: list[str]) -> str:
    return subprocess.run(cmd, capture_output=True, text=True, check=True).stdout


def grib_get(path: Path, keys: list[str]) -> list[str]:
    return run(["grib_get", "-p", ",".join(keys), str(path)]).split()


def decoded_values(path: Path) -> list[float | None]:
    out = run(["grib_get_data", "-m", "9999", str(path)])
    vals: list[float | None] = []
    for line in out.strip().splitlines()[1:]:
        v = line.split()[2]
        vals.append(None if v == "9999" else float(v))
    return vals


def write_oracle(grib_path: Path, oracle_path: Path, samples: list[int], note: str) -> None:
    ref, e, d, bits, ppp = grib_get(
        grib_path,
        [
            "referenceValue",
            "binaryScaleFactor",
            "decimalScaleFactor",
            "bitsPerValue",
            "preProcessingParameter",
        ],
    )
    vals = decoded_values(grib_path)
    present = [v for v in vals if v is not None]
    oracle = {
        "count": len(vals),
        "missing_count": sum(1 for v in vals if v is None),
        "min": min(present),
        "max": max(present),
        "mean": sum(present) / len(present),
        "samples": {str(i): vals[i] for i in samples},
        # Log decode reconstructs values through exp(), so a slightly looser
        # tolerance than the linear packings absorbs the round-trip error.
        "tolerance_absolute": 0.01,
        "section5": {
            "dataRepresentationTemplateNumber": 61,
            "packingType": "grid_simple_log_preprocessing",
            "referenceValue": float(ref),
            "binaryScaleFactor": int(e),
            "decimalScaleFactor": int(d),
            "bitsPerValue": int(bits),
            "preProcessingParameter": float(ppp),
            "typeOfPreProcessing": 1,
        },
        "source": note,
    }
    oracle_path.write_text(json.dumps(oracle, indent=2) + "\n")


def build(name: str, offset: float | None, samples: list[int], note: str) -> None:
    grib_path = FIXTURES / f"{name}.grib2"
    tmp = FIXTURES / f".{name}.tmp.grib2"
    src = SOURCE
    if offset is not None:
        # Shift the field so it holds non-positive values, forcing the encoder
        # to a non-zero preProcessingParameter.
        run(["grib_set", "-s", f"offsetValuesBy={offset}", str(SOURCE), str(tmp)])
        src = tmp
    # `-r` repacks the values through the log pre-processing encoder.
    run(["grib_set", "-r", "-s", "packingType=grid_simple_log_preprocessing", str(src), str(grib_path)])
    if tmp.exists():
        tmp.unlink()
    write_oracle(grib_path, FIXTURES / f"{name}_expected.json", samples, note)
    ppp = grib_get(grib_path, ["preProcessingParameter"])[0]
    print(f"wrote {grib_path.name} ({grib_path.stat().st_size} bytes, ppp={ppp}) + oracle")


def main() -> None:
    build(
        "log_regular_latlon",
        offset=None,
        samples=[0, 1, 100, 250, 495],
        note=(
            "eccodes 2.34.1 grib_get_data + grib_get. Oracle for DRS template "
            "5.61 (grid_simple_log_preprocessing). eccodes re-encode of "
            "regular_latlon_surface.grib2 via `grib_set -r -s "
            "packingType=grid_simple_log_preprocessing`. All-positive field so "
            "preProcessingParameter = 0 (decode Y = exp(X)). Provenance in "
            "NOTICE.md."
        ),
    )
    build(
        "log_negative_regular_latlon",
        offset=-300.0,
        samples=[0, 1, 100, 250, 495],
        note=(
            "eccodes 2.34.1 grib_get_data + grib_get. Oracle for DRS template "
            "5.61 (grid_simple_log_preprocessing) with a non-zero "
            "preProcessingParameter. regular_latlon_surface.grib2 shifted by "
            "-300 K (`grib_set -s offsetValuesBy=-300`) so it holds "
            "non-positive values, then re-encoded via `grib_set -r -s "
            "packingType=grid_simple_log_preprocessing`. Exercises the decode "
            "Y = exp(X) - B branch. Provenance in NOTICE.md."
        ),
    )


if __name__ == "__main__":
    main()
