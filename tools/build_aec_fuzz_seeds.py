#!/usr/bin/env python3
"""Write the seed corpus of the `fieldglass-aec` fuzz targets.

Each seed is a committed conformance stream (`crates/fieldglass-aec/tests/
fixtures/`) behind the 7-byte parameter header that
`crates/fieldglass-aec/fuzz/fuzz_targets/common.rs` reads:

    byte 0      bits per sample - 1
    byte 1      block size / 2 - 1
    bytes 2-3   reference sample interval - 1, little endian
    byte 4      flags
    bytes 5-6   sample count, little endian

The subset is small on purpose: one stream per (bits, flags) pair, smallest
first, plus every stream that is not a plain option case (truncated, trailing
fill, the libaec-rejected second-extension case), and a few real fields.
Seeds only need to put the fuzzer near each code path; it finds the rest.

    python3 tools/build_aec_fuzz_seeds.py
"""

import json
import pathlib
import struct

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "crates/fieldglass-aec/tests/fixtures"
OUT = ROOT / "crates/fieldglass-aec/fuzz/corpus"
TARGETS = ("decode", "differential")
MAX_OPTION = 1500
MAX_OTHER = 5000
MAX_FIELDS = 4


def header(case):
    return struct.pack(
        "<BBHBH",
        case["bits_per_sample"] - 1,
        case["block_size"] // 2 - 1,
        case["rsi"] - 1,
        case["flags"] & 0x3F,
        case["samples"],
    )


def main():
    cases = json.loads((FIXTURES / "manifest.json").read_text(encoding="utf-8"))["aec_cases"]
    sized = [(len((FIXTURES / c["stream"]).read_bytes()), c["name"], c) for c in cases]
    sized.sort(key=lambda t: t[:2])
    chosen = {}
    fields = 0
    for size, name, case in sized:
        limit = MAX_OPTION if case["kind"] == "option" else MAX_OTHER
        if size > limit or case["samples"] > 0xFFFF or not 1 <= case["bits_per_sample"] <= 32:
            continue
        if case["kind"] == "option":
            chosen.setdefault((case["bits_per_sample"], case["flags"]), case)
        elif case["kind"] == "field":
            if fields < MAX_FIELDS:
                fields += 1
                chosen[name] = case
        else:
            chosen[name] = case
    for target in TARGETS:
        directory = OUT / target
        directory.mkdir(parents=True, exist_ok=True)
        for stale in directory.glob("*"):
            stale.unlink()
        for case in chosen.values():
            body = (FIXTURES / case["stream"]).read_bytes()
            (directory / case["name"]).write_bytes(header(case) + body)
    print(f"{len(chosen)} seeds per target")


if __name__ == "__main__":
    main()
