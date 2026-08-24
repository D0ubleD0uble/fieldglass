#!/usr/bin/env python3
"""Snapshot eccodes' decode of an *already committed* real-producer fixture.

The §5 fixture builders under ``tools/`` each *construct* a GRIB message and
then record eccodes' decode of it. A fixture promoted straight from the
operational corpus (``samples/``) has nothing to construct — the bytes are the
producer's — but it still needs the same ``<stem>_expected.json`` value oracle
the decode tests read, so this script generates one for a file that already
exists.

The oracle is eccodes' answer, not ours: values come from ``grib_get_data`` and
the §5 parameters from ``grib_get``, both from the pinned CLI (see
``tools/eccodes_oracle.py`` and ``tests/fixtures/NOTICE.md``). Run it once when
promoting a fixture; the JSON is committed so the test suite needs no eccodes.

Usage:
    python3 tools/build_real_fixture_oracle.py <fixture.grib2> --note "..." \
        [--samples 0,1,100,1000,-1]

Requires: ``grib_get`` / ``grib_get_data`` on PATH (Debian/Ubuntu:
``apt install libeccodes-tools``).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from eccodes_oracle import PINNED_VERSION, decoded_values, grib_get  # noqa: E402

#: §5 keys worth snapshotting per data-representation template. Each entry is
#: the set the matching decode test asserts against; a template absent here
#: falls back to the parameters every packing carries.
COMMON_KEYS = [
    "referenceValue",
    "binaryScaleFactor",
    "decimalScaleFactor",
    "bitsPerValue",
    "typeOfOriginalFieldValues",
]
TEMPLATE_KEYS: dict[int, list[str]] = {
    40: COMMON_KEYS + ["typeOfCompressionUsed"],
    41: COMMON_KEYS,
    0: COMMON_KEYS,
}


def build(fixture: Path, sample_indices: list[int], note: str) -> Path:
    template, packing = grib_get(
        fixture, ["dataRepresentationTemplateNumber", "packingType"]
    )
    template = int(template)
    keys = TEMPLATE_KEYS.get(template, COMMON_KEYS)
    raw = grib_get(fixture, keys)
    if len(raw) != len(keys):
        raise SystemExit(f"grib_get returned {len(raw)} values for {len(keys)} keys")

    def typed(key: str, value: str) -> float | int:
        # eccodes prints integers for the integer keys; keep them integral so
        # the committed JSON reads the way the other oracles do.
        return float(value) if key == "referenceValue" else int(value)

    section5: dict[str, object] = {
        "dataRepresentationTemplateNumber": template,
        "packingType": packing,
    }
    section5.update({k: typed(k, v) for k, v in zip(keys, raw)})

    values = decoded_values(fixture)
    present = [v for v in values if v is not None]
    if not present:
        raise SystemExit(f"{fixture}: every point is missing — nothing to pin")

    # A negative index anchors on the last point, which is the one a scan-order
    # or trailing-group bug moves first.
    resolved = [i if i >= 0 else len(values) + i for i in sample_indices]
    oracle = {
        "count": len(values),
        "missing_count": sum(1 for v in values if v is None),
        "min": min(present),
        "max": max(present),
        "mean": sum(present) / len(present),
        "samples": {str(i): values[i] for i in resolved},
        "tolerance_absolute": 0.001,
        "section5": section5,
        "source": (
            f"eccodes {PINNED_VERSION} grib_get_data + grib_get. {note} "
            "Provenance in NOTICE.md."
        ),
    }
    out = fixture.with_name(f"{fixture.stem}_expected.json")
    out.write_text(json.dumps(oracle, indent=2) + "\n", encoding="utf-8")
    return out


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("fixture", type=Path)
    ap.add_argument("--note", required=True, help="one-line description for `source`")
    ap.add_argument(
        "--samples",
        default="0,1,100,1000,-1",
        help="comma-separated point indices to anchor (negative counts from the end)",
    )
    args = ap.parse_args()
    indices = [int(x) for x in args.samples.split(",")]
    out = build(args.fixture, indices, args.note)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
