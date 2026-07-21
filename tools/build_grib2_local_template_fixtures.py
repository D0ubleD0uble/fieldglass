#!/usr/bin/env python3
"""Build the GRIB2 pre-standard local-template fixtures (part of #307).

Two local-use (49152+) data-representation templates carry real historical
NCEP data with a §5/§7 layout identical to a registered image packing:

- **5.40010** — pre-standard PNG, byte-identical to template 5.41. eccodes has
  no `template.5.40010.def`, so it *cannot* decode this at all (a genuine
  exceed-eccodes case); Fieldglass decodes it through the PNG codec.
- **5.40000** — pre-standard JPEG 2000, byte-identical to template 5.40 (its
  eccodes def is literally `include template.5.40.def`). eccodes decodes it as
  `grid_jpeg`.

Each fixture is the matching committed image fixture with only its §5
data-representation-template number relabelled (octets 10–11), so the §7
codestream is untouched:

- `png_local_40010.grib2`     ← `png_eta_lambert.grib2` (5.41 → 5.40010)
- `jpeg2000_local_40000.grib2` ← `jpeg2000_regular_latlon.grib2` (5.40 → 5.40000)

Value oracles (`*_expected.json`):

- 40000: eccodes `grib_get_data` on the relabelled file (eccodes decodes it).
- 40010: eccodes cannot decode the relabelled file, so the oracle is eccodes'
  decode of the *original* 5.41 fixture — the §7 is identical, so the values
  Fieldglass must produce are exactly those. This is the "our own 5.41 decode"
  oracle the issue calls for, anchored to eccodes' 5.41 output.

Usage:
    python3 tools/build_grib2_local_template_fixtures.py
"""

from __future__ import annotations

import json
import struct
from pathlib import Path

from eccodes_oracle import decoded_values, grib_get

FIXTURES = (
    Path(__file__).resolve().parent.parent
    / "crates"
    / "fieldglass-grib2"
    / "tests"
    / "fixtures"
)


def relabel_drt(src: Path, dst: Path, number: int) -> int:
    """Copy `src` to `dst` with the §5 template number set to `number`.
    Returns the original template number."""
    buf = bytearray(src.read_bytes())
    assert buf[:4] == b"GRIB" and buf[7] == 2, f"{src.name}: not GRIB2"
    pos = 16
    old = None
    while buf[pos : pos + 4] != b"7777":
        sec_len = struct.unpack(">I", buf[pos : pos + 4])[0]
        if buf[pos + 4] == 5:
            old = struct.unpack(">H", buf[pos + 9 : pos + 11])[0]
            struct.pack_into(">H", buf, pos + 9, number)
            break
        pos += sec_len
    assert old is not None, f"{src.name}: no §5 found"
    dst.write_bytes(buf)
    return old


def write_oracle(
    oracle_source: Path, oracle_path: Path, template_number: int, packing: str,
    samples: list[int], note: str,
) -> None:
    """`oracle_source` is the file eccodes can actually decode (the original
    5.41 fixture for 40010, or the relabelled file for 40000)."""
    bits, ref, e, d = grib_get(
        oracle_source,
        ["bitsPerValue", "referenceValue", "binaryScaleFactor", "decimalScaleFactor"],
    )
    vals = decoded_values(oracle_source)
    present = [v for v in vals if v is not None]
    oracle = {
        "count": len(vals),
        "missing_count": sum(1 for v in vals if v is None),
        "min": min(present),
        "max": max(present),
        "mean": sum(present) / len(present),
        "samples": {str(i): vals[i] for i in samples},
        "tolerance_absolute": 0.001,
        "section5": {
            "dataRepresentationTemplateNumber": template_number,
            "packingType": packing,
            "bitsPerValue": int(bits),
            "referenceValue": float(ref),
            "binaryScaleFactor": int(e),
            "decimalScaleFactor": int(d),
        },
        "source": note,
    }
    oracle_path.write_text(json.dumps(oracle, indent=2) + "\n")


def main() -> None:
    # 5.40000 — relabel 5.40; eccodes decodes the result directly.
    src40 = FIXTURES / "jpeg2000_regular_latlon.grib2"
    f40000 = FIXTURES / "jpeg2000_local_40000.grib2"
    old = relabel_drt(src40, f40000, 40000)
    assert old == 40, old
    write_oracle(
        f40000, FIXTURES / "jpeg2000_local_40000_expected.json", 40000, "grid_jpeg",
        [0, 1, 100, 250, 495],
        "eccodes 2.34.1 grib_get_data + grib_get on the relabelled file (eccodes "
        "decodes 5.40000 as grid_jpeg). jpeg2000_regular_latlon.grib2 with its §5 "
        "template number changed 40 -> 40000 by "
        "tools/build_grib2_local_template_fixtures.py; §7 codestream unchanged. "
        "Provenance in NOTICE.md.",
    )
    print(f"wrote {f40000.name} ({f40000.stat().st_size} bytes) + oracle")

    # 5.40010 — relabel 5.41; eccodes CANNOT decode it, so the oracle is
    # eccodes' decode of the original 5.41 fixture (identical §7).
    src41 = FIXTURES / "png_eta_lambert.grib2"
    f40010 = FIXTURES / "png_local_40010.grib2"
    old = relabel_drt(src41, f40010, 40010)
    assert old == 41, old
    write_oracle(
        src41, FIXTURES / "png_local_40010_expected.json", 40010, "grid_png",
        [0, 1, 3000, 6000, 6044],
        "eccodes 2.34.1 grib_get_data + grib_get on the ORIGINAL "
        "png_eta_lambert.grib2 (5.41): eccodes has no 5.40010 definition and "
        "cannot decode the relabelled file, but its §7 is byte-identical, so the "
        "5.41 decode is the value oracle. Fixture built by "
        "tools/build_grib2_local_template_fixtures.py (§5 template number "
        "41 -> 40010). Provenance in NOTICE.md.",
    )
    print(f"wrote {f40010.name} ({f40010.stat().st_size} bytes) + oracle")


if __name__ == "__main__":
    main()
