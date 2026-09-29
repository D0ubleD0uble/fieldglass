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

The `sz` target reads an 8-byte szip header instead (see
`crates/fieldglass-aec/fuzz/fuzz_targets/sz.rs`):

    byte 0      options mask
    byte 1      bits per pixel, 0 meaning 64
    byte 2      pixels per block / 2 - 1
    bytes 3-4   pixels per scanline - 1, little endian
    bytes 5-6   output length in bytes, little endian
    byte 7      unused

Every szip case in the corpus is small, so each one is a seed. One more is
built here: the stream of `a_bad_code_after_the_last_pixel_is_never_read` in
`crates/fieldglass-aec/tests/sz_corpus.rs`, so the target starts on the one
difference from libsz it allows (a bad code after the last pixel).

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


# 8 bits, 2 per block, 5 per scanline: uncompressed blocks 1 2, 3 4, 5 pad,
# 6 7, 8 9, then a zero block of two blocks with one left in the interval.
BAD_CODE_AFTER_LAST_PIXEL = bytes.fromhex("e0205c0c13828070607e101208")


def sz_header(case):
    return struct.pack(
        "<BBBHHB",
        case["options_mask"] & 0xFF,
        case["bits_per_pixel"] % 64,
        case["pixels_per_block"] // 2 - 1,
        case["pixels_per_scanline"] - 1,
        case["dest_len"],
        0,
    )


def write_seeds(directory, seeds):
    directory.mkdir(parents=True, exist_ok=True)
    for stale in directory.glob("*"):
        stale.unlink()
    for name, body in seeds:
        (directory / name).write_bytes(body)


def main():
    manifest = json.loads((FIXTURES / "manifest.json").read_text(encoding="utf-8"))
    cases = manifest["aec_cases"]
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
    seeds = [(c["name"], header(c) + (FIXTURES / c["stream"]).read_bytes()) for c in chosen.values()]
    for target in TARGETS:
        write_seeds(OUT / target, seeds)
    sz = [(c["name"], sz_header(c) + (FIXTURES / c["stream"]).read_bytes()) for c in manifest["sz_cases"]]
    sz.append((
        "sz_bad_code_after_last_pixel",
        sz_header({"options_mask": 0, "bits_per_pixel": 8, "pixels_per_block": 2,
                   "pixels_per_scanline": 5, "dest_len": 6})
        + BAD_CODE_AFTER_LAST_PIXEL,
    ))
    write_seeds(OUT / "sz", sz)
    print(f"{len(chosen)} seeds per AEC target, {len(sz)} for sz")


if __name__ == "__main__":
    main()
