#!/usr/bin/env python3
"""Build the GRIB2 run-length packing (DRS template 5.200) test fixtures (#301).

eccodes 2.34.1 (the pinned oracle) decodes ``grid_run_length`` but cannot be
coaxed into *encoding* an arbitrary field as run-length from the CLI (the
encoder only accepts values that already fall exactly on a preset level table).
So we hand-build the two fixtures instead and use eccodes' *decode* as the
value oracle — the pattern ``tests/fixtures/NOTICE.md`` records for packings
eccodes cannot re-encode.

Both fixtures reuse the §0–§4 (indicator, identification, grid, product) of the
committed ``regular_latlon_surface.grib2`` (a 16×31 = 496-point regular lat/lon
surface field), replacing §5/§6/§7/§8 with a template-5.200 section, a
no-bitmap §6, and a run-length §7. §6 carries no bitmap: run-length encodes
missing points as level 0, so the missing seam is exercised without one.

Outputs (under ``crates/fieldglass-grib2/tests/fixtures``):

- ``runlength_regular_latlon.grib2`` — 8 bits/value, decimalScaleFactor = 1.
  A run longer than ``range`` (300 > 250) exercises the multi-digit base-range
  run length; a level-0 run exercises missing.
- ``runlength_4bit_regular_latlon.grib2`` — 4 bits/value, decimalScaleFactor
  raw byte 129 (sign-magnitude → −1). Exercises sub-byte code packing,
  base-10 multi-digit runs, single-point runs, and a negative decimal scale.

Each ``.grib2`` gets a sibling ``*_expected.json`` value oracle produced from
eccodes ``grib_get_data`` / ``grib_get``. The ``.eccodes.ref.json`` metadata
snapshots are produced separately by ``tools/regenerate-eccodes-snapshots.py``.

Usage:
    python3 tools/build_grib2_runlength_fixtures.py
"""

from __future__ import annotations

import json
import struct
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


def split_sections(buf: bytes) -> tuple[bytes, list[tuple[int, bytes]]]:
    assert buf[:4] == b"GRIB" and buf[7] == 2, "source is not GRIB2"
    is16 = buf[:16]
    pos = 16
    secs: list[tuple[int, bytes]] = []
    while buf[pos : pos + 4] != b"7777":
        sec_len = struct.unpack(">I", buf[pos : pos + 4])[0]
        secs.append((buf[pos + 4], buf[pos : pos + sec_len]))
        pos += sec_len
    return is16, secs


def pack_codes(codes: list[int], bits: int) -> bytes:
    """MSB-first bit-pack `codes`, each `bits` wide. Asserts byte alignment:
    eccodes writes `floor(pos/8)` bytes, so a stream that is not a whole number
    of bytes would drop or invent a code."""
    out_bits: list[int] = []
    for c in codes:
        for i in range(bits - 1, -1, -1):
            out_bits.append((c >> i) & 1)
    assert len(out_bits) % 8 == 0, (
        f"{len(codes)} codes × {bits} bits = {len(out_bits)} bits is not "
        "byte-aligned; pick a code count with (count·bits) % 8 == 0"
    )
    out = bytearray()
    for i in range(0, len(out_bits), 8):
        byte = 0
        for j in range(8):
            byte = (byte << 1) | out_bits[i + j]
        out.append(byte)
    return bytes(out)


def encode_runs(runs: list[tuple[int, int]], bits: int, max_level: int) -> bytes:
    """Encode (level, count) runs into a §7 run-length payload. A run is the
    level code followed by base-`range` digits of (count-1), least significant
    first, each offset by max_level+1."""
    rng = (1 << bits) - 1 - max_level
    assert rng > 0, "range must be positive"
    codes: list[int] = []
    for level, count in runs:
        assert 0 <= level <= max_level and count >= 1
        codes.append(level)
        rem = count - 1
        while rem > 0:
            codes.append(rem % rng + max_level + 1)
            rem //= rng
    return pack_codes(codes, bits)


def build_section5(bits, max_level, level_values, decimal_scale_raw) -> bytes:
    body = bytearray()
    body += struct.pack(">I", 0)  # length placeholder
    body += bytes([5])  # section number
    body += struct.pack(">I", NUM_VALUES)  # numberOfValues
    body += struct.pack(">H", 200)  # dataRepresentationTemplateNumber
    body += bytes([bits])  # bitsPerValue
    body += struct.pack(">H", max_level)  # maxLevelValue
    body += struct.pack(">H", len(level_values))  # numberOfLevelValues
    body += bytes([decimal_scale_raw & 0xFF])  # decimalScaleFactor
    for lv in level_values:
        body += struct.pack(">H", lv)
    struct.pack_into(">I", body, 0, len(body))
    return bytes(body)


def build_message(runs, bits, max_level, level_values, decimal_scale_raw) -> bytes:
    is16, secs = split_sections(SOURCE.read_bytes())
    keep = [body for num, body in secs if num in (1, 2, 3, 4)]
    total = sum(count for _, count in runs)
    assert total == NUM_VALUES, (total, NUM_VALUES)

    s5 = build_section5(bits, max_level, level_values, decimal_scale_raw)
    s6 = struct.pack(">I", 6) + bytes([6, 255])  # no bitmap
    payload = encode_runs(runs, bits, max_level)
    s7 = struct.pack(">I", 5 + len(payload)) + bytes([7]) + payload

    body = b"".join(keep) + s5 + s6 + s7
    is_hdr = bytearray(is16)
    struct.pack_into(">Q", is_hdr, 8, 16 + len(body) + 4)  # totalLength
    return bytes(is_hdr) + body + b"7777"


def signed_decimal_scale(raw: int) -> int:
    return -(raw - 128) if raw > 127 else raw


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
    grib_path: Path, oracle_path: Path, sample_indices: list[int], note: str
) -> None:
    keys = [
        "bitsPerValue",
        "maxLevelValue",
        "numberOfLevelValues",
        "decimalScaleFactor",
    ]
    bits, max_level, n_levels, dscale = (int(x) for x in grib_get(grib_path, keys))
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
        "tolerance_absolute": 0.001,
        "section5": {
            "dataRepresentationTemplateNumber": 200,
            "packingType": "grid_run_length",
            "bitsPerValue": bits,
            "maxLevelValue": max_level,
            "numberOfLevelValues": n_levels,
            "decimalScaleFactor": dscale,
            "decimalScaleFactorSigned": signed_decimal_scale(dscale),
        },
        "source": note,
    }
    oracle_path.write_text(json.dumps(oracle, indent=2) + "\n")


def build(name, runs, bits, max_level, level_values, decimal_scale_raw, samples, note):
    grib_path = FIXTURES / f"{name}.grib2"
    grib_path.write_bytes(
        build_message(runs, bits, max_level, level_values, decimal_scale_raw)
    )
    # Cross-check: eccodes must decode exactly what we intended to encode.
    scale = 10.0 ** (-signed_decimal_scale(decimal_scale_raw))
    levels = [None] + [lv * scale for lv in level_values]
    expected: list[float | None] = []
    for level, count in runs:
        expected += [levels[level]] * count
    got = decoded_values(grib_path)
    for i, (a, b) in enumerate(zip(expected, got)):
        if a is None:
            assert b is None, f"{name} idx {i}: expected missing, eccodes {b}"
        else:
            assert b is not None and abs(a - b) < 1e-9, f"{name} idx {i}: {a} vs {b}"
    write_oracle(grib_path, FIXTURES / f"{name}_expected.json", samples, note)
    print(f"wrote {grib_path.name} ({grib_path.stat().st_size} bytes) + oracle")


def main() -> None:
    build(
        "runlength_regular_latlon",
        runs=[(1, 50), (0, 46), (3, 100), (5, 300)],
        bits=8,
        max_level=5,
        level_values=[10, 20, 30, 40, 50],
        decimal_scale_raw=1,
        samples=[0, 49, 50, 95, 96, 195, 196, 495],
        note=(
            "eccodes 2.34.1 grib_get_data + grib_get. Oracle for DRS template "
            "5.200 (grid_run_length) at bitsPerValue=8. Hand-built: §0-§4 from "
            "regular_latlon_surface.grib2, §5/§6/§7 synthesised by "
            "tools/build_grib2_runlength_fixtures.py (eccodes cannot CLI-encode "
            "run-length). Runs exercise a >range multi-digit run and a level-0 "
            "(missing) run. Provenance in NOTICE.md."
        ),
    )
    build(
        "runlength_4bit_regular_latlon",
        runs=[(1, 200), (0, 100), (3, 150), (5, 1), (2, 44), (4, 1)],
        bits=4,
        max_level=5,
        level_values=[1, 2, 3, 4, 5],
        decimal_scale_raw=129,  # sign-magnitude -> -1
        samples=[0, 199, 200, 299, 300, 449, 450, 451, 494, 495],
        note=(
            "eccodes 2.34.1 grib_get_data + grib_get. Oracle for DRS template "
            "5.200 (grid_run_length) at bitsPerValue=4 with a negative decimal "
            "scale (raw byte 129 -> sign-magnitude -1). Hand-built by "
            "tools/build_grib2_runlength_fixtures.py. Exercises sub-byte code "
            "packing, base-10 multi-digit runs, single-point runs, and missing "
            "(level 0). Provenance in NOTICE.md."
        ),
    )


if __name__ == "__main__":
    main()
