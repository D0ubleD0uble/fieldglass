#!/usr/bin/env python3
"""Build a GRIB2 matrix-of-values (DRT 5.1, `grid_simple_matrix`) decode fixture.

Template 5.1 is experimental and eccodes only handles the
`matrixBitmapsPresent = 0` case — one simple-packed value per grid point, with
`NR`/`NC` and the coordinate arrays as descriptive metadata (the true per-point
matrix, `matrixBitmapsPresent = 1`, makes eccodes' accessor assert out and has
no oracle). This fixture pins the flat, oracle-verifiable case: the §7 values
decode exactly like template 5.0.

Encoded with the eccodes Python wheel (the CLI cannot set a values array and the
matrix template is experimental); the value oracle is the pinned CLI 2.34.1
`grib_get_data`. Provenance in tests/fixtures/NOTICE.md.

Regenerate:  python3 tools/build_grib2_matrix_fixtures.py
Needs:       eccodes + numpy in this Python; grib_get_data on PATH (2.34.1).
"""

from __future__ import annotations

import pathlib
import subprocess
import sys

import numpy as np

try:
    import eccodes as ec
except ImportError:  # pragma: no cover
    sys.exit("eccodes Python module not found (pip install eccodes numpy).")

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)
SAMPLE = pathlib.Path.home() / (
    "Code/research/wx_sci/wxcode-rs/eccodes/samples/GRIB2.tmpl"
)


def main() -> None:
    if not SAMPLE.exists():
        sys.exit(f"eccodes sample not found: {SAMPLE}")
    with open(SAMPLE, "rb") as f:
        h = ec.codes_grib_new_from_file(f)
    ndp = ec.codes_get(h, "numberOfDataPoints")
    ec.codes_set(h, "packingType", "grid_simple_matrix")
    # NR/NC are descriptive metadata for the flat (matrixBitmapsPresent=0) case;
    # set them to a genuine 2x3 to exercise the parser.
    ec.codes_set(h, "NR", 2)
    ec.codes_set(h, "NC", 3)
    ec.codes_set(h, "bitsPerValue", 12)
    vals = (np.arange(ndp, dtype=float) % 23) * 0.5 - 3.0
    ec.codes_set_values(h, vals)
    grib_path = FIXTURES / "matrix_simple_regular_latlon.grib2"
    with open(grib_path, "wb") as f:
        ec.codes_write(h, f)
    meta = {
        k: ec.codes_get(h, k)
        for k in ("dataRepresentationTemplateNumber", "NR", "NC",
                  "matrixBitmapsPresent", "numberOfCodedValues",
                  "numberOfValues", "bitsPerValue")
    }
    ec.codes_release(h)

    # Value oracle from the pinned CLI (grib_get_data prints lat/lon/value rows;
    # take the trailing value column).
    out = subprocess.run(
        ["grib_get_data", str(grib_path)],
        capture_output=True, text=True, check=True,
    ).stdout.splitlines()[1:]
    values = [line.split()[-1] for line in out if line.strip()]
    (FIXTURES / "matrix_simple_regular_latlon.eccodes.ref.txt").write_text(
        "\n".join(values) + "\n"
    )
    print(f"wrote {grib_path.name}: {meta}")
    print(f"oracle values: {len(values)} (numberOfDataPoints={ndp})")
    assert len(values) == ndp, f"oracle {len(values)} != numberOfDataPoints {ndp}"
    print("done")


if __name__ == "__main__":
    main()
