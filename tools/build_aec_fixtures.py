#!/usr/bin/env python3
"""Build the `fieldglass-aec` conformance corpus from a pinned libaec (#758).

    python3 tools/build_aec_fixtures.py                      # rewrite the corpus
    python3 tools/build_aec_fixtures.py --out DIR            # write it elsewhere
    python3 tools/build_aec_fixtures.py --dump-expected DIR  # plus libaec's output
    python3 tools/build_aec_fixtures.py --full DIR           # the large matrix

`fieldglass-aec` is checked against libaec 1.1.7 wherever libaec is correct
(ADR-0012 decisions 3, 4 and 8). This script is the only place that oracle is run. It downloads the
libaec v1.1.7 tag tarball, checks its SHA-256, builds it twice with cmake in a
temporary directory, and drives the built libraries through `ctypes`:

* the **default** build, for everything except `PAD_RSI`;
* a build with `-DCMAKE_C_FLAGS=-DENABLE_RSI_PADDING`, because the default
  encoder never writes RSI padding (`encode.c:480` is behind that macro and
  nothing in libaec's CMake defines it). Only its encoder is used: the macro
  does not touch the decoder.

A third build, with the decoder's second-extension table widened, is used only
to confirm why the stock decoder refuses a stream (`is_se_table_rejection`). It
never supplies an expected output.

Every stream comes from **libaec's own encoder**, and libaec's **decoder** is
always the value oracle. There are no hand-built streams. The inputs of the
option-forced cases are ported from libaec's `tests/check_code_options.c`
(BSD-2-Clause, see the crate's `tests/fixtures/NOTICE.md`), which constructs a
field per coding option and asserts the option id the encoder emits; this
script makes the same assertion, over the same block sizes and RSIs, and fails
if any of them does not hold.

What it writes, under `crates/fieldglass-aec/tests/fixtures` unless `--out`:

* `streams/*.rz`, one encoded stream per case;
* `manifest.json`: a header naming the libaec commit and tarball digest, the
  parameter grid (each row is `aec_decode_init`'s verdict on one parameter
  set), the AEC cases (parameters, sample count, libaec's status, bytes out and
  the SHA-256 of those bytes) and the szip cases (the same, from libsz's
  `SZ_BufftoBuffDecompress`).

Running it twice gives a byte-identical tree. Every field comes from a seeded
generator, and `streams/` is emptied first so a dropped case leaves no file
behind. Never edit `manifest.json` by hand: `tools/aec_oracle.py`, run in CI,
regenerates it and diffs (#763). Every case must round-trip (libaec's output equals the field the
encoder was given) unless its kind says otherwise, so a case libaec cannot
decode never enters the corpus by accident.

`--dump-expected crates/fieldglass-aec/tests/expected` writes libaec's decoded
bytes where git ignores them, for reading a failing case by eye. `--full DIR`
writes the large matrix (4,819 cases, not committed) in the same format, for
the CI oracle job.

Needs cmake 3.26 or newer, a C compiler and network access (or `--tarball`),
only to regenerate. The Rust tests need none of them.
"""
from __future__ import annotations

import argparse
import ctypes
import hashlib
import io
import json
import random
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
FIXTURES = REPO / "crates" / "fieldglass-aec" / "tests" / "fixtures"

LIBAEC_VERSION = "1.1.7"
# `v1.1.7` is a lightweight tag on this commit.
LIBAEC_COMMIT = "0c4c01463d2c64a112a61271d317b74efb660608"
# Composed from literals only, so a static analyser can fold it to one fixed
# https:// URL. Fetched with single-argument `urlopen` for the same reason.
SOURCE_URL = "https://github.com/MathisRosenhauer/libaec/archive/refs/tags/v1.1.7.tar.gz"
SOURCE_SHA256 = "26661a569a7def45a2e97fbbd09e0dc5bbb2f8ab1b41250c19e795559eec6fb2"

CMAKE_COMMON = [
    "-DCMAKE_BUILD_TYPE=Release",
    "-DBUILD_TESTING=OFF",
    "-DBUILD_STATIC_LIBS=OFF",
]
RSI_PADDING_FLAGS = "-DCMAKE_C_FLAGS=-DENABLE_RSI_PADDING"

# libaec.h flag bits. GRIB2's `ccsdsFlags` uses the same values.
SIGNED = 1
THREE_BYTE = 2
MSB = 4
PREPROCESS = 8
RESTRICTED = 16
PAD_RSI = 32
NOT_ENFORCE = 64

# szlib.h option-mask bits. libsz honours only MSB and NN when decoding.
SZ_EC = 4
SZ_LSB = 8
SZ_MSB = 16
SZ_NN = 32

AEC_OK = 0
AEC_DATA_ERROR = -3

# check_code_options.c's buffer: 3 KiB of samples per case.
CHECK_BUF_BYTES = 1024 * 3
STANDARD_BLOCKS = (8, 16, 32, 64)


# --------------------------------------------------------------------------
# libaec, built from the pinned tarball
# --------------------------------------------------------------------------


def fetch_source(tarball: Path | None) -> bytes:
    """The tag tarball, from `tarball` if given, else downloaded. Digest-checked."""
    if tarball is not None:
        data = tarball.read_bytes()
    else:
        print(f"downloading {SOURCE_URL}", file=sys.stderr)
        with urllib.request.urlopen(SOURCE_URL) as resp:
            data = resp.read()
    digest = hashlib.sha256(data).hexdigest()
    if digest != SOURCE_SHA256:
        raise SystemExit(
            f"libaec tarball SHA-256 is {digest}, expected {SOURCE_SHA256}; "
            "refusing to build an oracle from unpinned source"
        )
    return data


# The confirmation build: libaec's decoder with its second-extension table
# widened from pair sums 0-12 to 0-128, which covers any sum a block of up to
# 256 samples at up to 32 bits can reach within the uncompressed length the
# encoder bounds it by. It is never an oracle for output. It only proves that a
# stream the stock decoder refuses fails for that one reason: the wide decoder
# must then return exactly the field the encoder was given.
SE_TABLE_PATCHES = (
    ("src/decode.h", "#define SE_TABLE_SIZE 90\n", "#define SE_TABLE_SIZE 8384\n"),
    ("src/decode.c", "    for (int i = 0; i < 13; i++) {\n", "    for (int i = 0; i < 129; i++) {\n"),
)


def widen_se_table(src: Path, dest: Path) -> None:
    """Copy the source tree to `dest` with SE_TABLE_PATCHES applied, each once."""
    shutil.copytree(src, dest)
    for rel, old, new in SE_TABLE_PATCHES:
        path = dest / rel
        text = path.read_text(encoding="utf-8")
        if text.count(old) != 1:
            raise SystemExit(f"{rel}: expected exactly one {old.strip()!r} to widen the SE table")
        path.write_text(text.replace(old, new), encoding="utf-8")


def build_libaec(work: Path, source: bytes) -> dict[str, Path]:
    """Build every flavour; return the directory holding each one's libraries."""
    # Member by member rather than `extractall`: regular files only, and none
    # that would land outside `work`. The digest check already pins the
    # tarball; this keeps a bad one from writing anywhere even so.
    with tarfile.open(fileobj=io.BytesIO(source), mode="r:gz") as tar:
        for member in tar.getmembers():
            if not member.isfile():
                continue
            rel = Path(member.name)
            if rel.is_absolute() or ".." in rel.parts:
                raise SystemExit(f"libaec tarball member escapes the build directory: {member.name}")
            handle = tar.extractfile(member)
            if handle is None:
                continue
            dest = work / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_bytes(handle.read())
    src = work / f"libaec-{LIBAEC_VERSION}"
    wide_src = work / "libaec-wide-se-table"
    widen_se_table(src, wide_src)
    out: dict[str, Path] = {}
    for name, tree, extra in (
        ("default", src, []),
        ("rsi_padding", src, [RSI_PADDING_FLAGS]),
        ("wide_se_table", wide_src, []),
    ):
        build = work / f"build-{name}"
        subprocess.run(
            ["cmake", "-S", str(tree), "-B", str(build), *CMAKE_COMMON, *extra],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        subprocess.run(
            ["cmake", "--build", str(build), "--parallel", "4"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
        out[name] = build / "src"
    return out


class AecStream(ctypes.Structure):
    """`struct aec_stream` (`libaec.h.in:77-107`)."""

    _fields_ = [
        ("next_in", ctypes.c_void_p),
        ("avail_in", ctypes.c_size_t),
        ("total_in", ctypes.c_size_t),
        ("next_out", ctypes.c_void_p),
        ("avail_out", ctypes.c_size_t),
        ("total_out", ctypes.c_size_t),
        ("bits_per_sample", ctypes.c_uint),
        ("block_size", ctypes.c_uint),
        ("rsi", ctypes.c_uint),
        ("flags", ctypes.c_uint),
        ("state", ctypes.c_void_p),
    ]


class SzCom(ctypes.Structure):
    """`SZ_com_t` (`szlib.h:67-73`). Not HDF5's `cd_values` order."""

    _fields_ = [
        ("options_mask", ctypes.c_int),
        ("bits_per_pixel", ctypes.c_int),
        ("pixels_per_block", ctypes.c_int),
        ("pixels_per_scanline", ctypes.c_int),
    ]


def _stream(inp, in_len, out, out_len, bps, block, rsi, flags) -> AecStream:
    return AecStream(
        ctypes.cast(inp, ctypes.c_void_p),
        in_len,
        0,
        ctypes.cast(out, ctypes.c_void_p),
        out_len,
        0,
        bps,
        block,
        rsi,
        flags,
        None,
    )


class Libaec:
    """One built `libaec.so`, loaded by path."""

    def __init__(self, path: Path):
        self.lib = ctypes.CDLL(str(path))
        for fn in ("aec_buffer_encode", "aec_buffer_decode", "aec_decode_init", "aec_decode_end"):
            getattr(self.lib, fn).argtypes = [ctypes.POINTER(AecStream)]
            getattr(self.lib, fn).restype = ctypes.c_int
        self.lib.aec_decode.argtypes = [ctypes.POINTER(AecStream), ctypes.c_int]
        self.lib.aec_decode.restype = ctypes.c_int

    def encode(self, data: bytes, bps: int, block: int, rsi: int, flags: int) -> bytes:
        inp = ctypes.create_string_buffer(data, len(data))
        cap = 2 * len(data) + 4096
        out = ctypes.create_string_buffer(cap)
        strm = _stream(inp, len(data), out, cap, bps, block, rsi, flags)
        status = self.lib.aec_buffer_encode(ctypes.byref(strm))
        if status != AEC_OK:
            raise RuntimeError(f"aec_buffer_encode returned {status} for {bps}/{block}/{rsi}/{flags}")
        return out.raw[: strm.total_out]

    def decode(self, stream: bytes, bps: int, block: int, rsi: int, flags: int, out_len: int):
        """libaec's verdict and output for exactly `out_len` bytes of room."""
        inp = ctypes.create_string_buffer(stream, max(len(stream), 1))
        out = ctypes.create_string_buffer(max(out_len, 1))
        strm = _stream(inp, len(stream), out, out_len, bps, block, rsi, flags)
        status = self.lib.aec_buffer_decode(ctypes.byref(strm))
        return status, out.raw[: strm.total_out]

    def decode_stepwise(self, stream: bytes, bps: int, block: int, rsi: int, flags: int, n: int):
        """Decode one sample per call; return the status and every byte produced.

        On an error `aec_buffer_decode` neither flushes nor corrects
        `total_out`. One sample of room per call makes every sample libaec
        decodes before an error visible.
        """
        width = bytes_per_sample(bps, flags)
        inp = ctypes.create_string_buffer(stream, max(len(stream), 1))
        out = ctypes.create_string_buffer(max(n * width, 1))
        base = ctypes.cast(out, ctypes.c_void_p).value
        strm = _stream(inp, len(stream), out, 0, bps, block, rsi, flags)
        if self.lib.aec_decode_init(ctypes.byref(strm)) != AEC_OK:
            raise RuntimeError("aec_decode_init failed")
        produced = 0
        status = AEC_OK
        try:
            while produced < n * width:
                strm.next_out = base + produced
                strm.avail_out = width
                status = self.lib.aec_decode(ctypes.byref(strm), 0)
                if status != AEC_OK or strm.avail_out == width:
                    break
                produced += width
        finally:
            self.lib.aec_decode_end(ctypes.byref(strm))
        return status, out.raw[:produced]

    def decode_init(self, bps: int, block: int, rsi: int, flags: int) -> int:
        """`aec_decode_init`'s return code for one parameter set."""
        strm = _stream(None, 0, None, 0, bps, block, rsi, flags)
        status = self.lib.aec_decode_init(ctypes.byref(strm))
        if status == AEC_OK:
            self.lib.aec_decode_end(ctypes.byref(strm))
        return status


class Libsz:
    """One built `libsz.so`, loaded by path."""

    def __init__(self, path: Path):
        self.lib = ctypes.CDLL(str(path))
        sig = [
            ctypes.c_void_p,
            ctypes.POINTER(ctypes.c_size_t),
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.POINTER(SzCom),
        ]
        for fn in ("SZ_BufftoBuffCompress", "SZ_BufftoBuffDecompress"):
            getattr(self.lib, fn).argtypes = sig
            getattr(self.lib, fn).restype = ctypes.c_int

    def compress(self, data: bytes, mask: int, bpp: int, ppb: int, pps: int) -> bytes:
        inp = ctypes.create_string_buffer(data, len(data))
        cap = 2 * len(data) + 4096
        out = ctypes.create_string_buffer(cap)
        out_len = ctypes.c_size_t(cap)
        param = SzCom(mask, bpp, ppb, pps)
        status = self.lib.SZ_BufftoBuffCompress(out, ctypes.byref(out_len), inp, len(data), ctypes.byref(param))
        if status != AEC_OK:
            raise RuntimeError(f"SZ_BufftoBuffCompress returned {status} for {mask}/{bpp}/{ppb}/{pps}")
        return out.raw[: out_len.value]

    def decompress(self, stream: bytes, mask: int, bpp: int, ppb: int, pps: int, dest_len: int):
        inp = ctypes.create_string_buffer(stream, max(len(stream), 1))
        out = ctypes.create_string_buffer(max(dest_len, 1))
        out_len = ctypes.c_size_t(dest_len)
        param = SzCom(mask, bpp, ppb, pps)
        status = self.lib.SZ_BufftoBuffDecompress(out, ctypes.byref(out_len), inp, len(stream), ctypes.byref(param))
        return status, out.raw[: out_len.value]


# --------------------------------------------------------------------------
# Sample layout
# --------------------------------------------------------------------------


def bytes_per_sample(bps: int, flags: int) -> int:
    """libaec's sample width (`decode.c:710-736`)."""
    if bps > 16:
        return 3 if bps <= 24 and flags & THREE_BYTE else 4
    return 2 if bps > 8 else 1


def pack(values: list[int], bps: int, flags: int) -> bytes:
    """Samples as libaec's encoder reads them.

    Signed samples go in as n-bit two's-complement patterns, not sign-extended
    to the byte width: the encoder sign-extends from bit n-1 itself, and a
    sample with bits set above n is mis-encoded.
    """
    width = bytes_per_sample(bps, flags)
    order = "big" if flags & MSB else "little"
    mask = (1 << bps) - 1
    return b"".join((v & mask).to_bytes(width, order) for v in values)


def decoded_layout(values: list[int], bps: int, flags: int) -> bytes:
    """What libaec's decoder writes for `values`.

    Sign extension happens in the postprocessor (`decode.c:55-127`), so SIGNED
    samples come out sign-extended only with PREPROCESS. Without it the decoder
    copies the n-bit pattern straight out.
    """
    width = bytes_per_sample(bps, flags)
    order = "big" if flags & MSB else "little"
    mask = (1 << bps) - 1
    top = 1 << (bps - 1)
    extend = flags & SIGNED and flags & PREPROCESS
    out = []
    for v in values:
        v &= mask
        if extend and v & top:
            v -= 1 << bps
        out.append((v & ((1 << (8 * width)) - 1)).to_bytes(width, order))
    return b"".join(out)


def value_range(bps: int, flags: int) -> tuple[int, int]:
    if flags & SIGNED:
        return -(1 << (bps - 1)), (1 << (bps - 1)) - 1
    return 0, (1 << bps) - 1


def id_len(bps: int, flags: int) -> int:
    """Option-id width (`decode.c:710-754`)."""
    if bps > 16:
        return 5
    if bps > 8:
        return 4
    if flags & RESTRICTED and bps <= 4:
        return 1 if bps <= 2 else 2
    return 3


# --------------------------------------------------------------------------
# Option-forced inputs, ported from libaec tests/check_code_options.c
# --------------------------------------------------------------------------

# check_byte_orderings(): the five flag sets the file walks, in its order.
CHECK_ORDERINGS = (
    ("nopp_lsb_u", 0),
    ("pp_lsb_u", PREPROCESS),
    ("pp_lsb_s", PREPROCESS | SIGNED),
    ("pp_msb_u", PREPROCESS | MSB),
    ("pp_msb_s", PREPROCESS | MSB | SIGNED),
)
# check_bps() walks bits per sample 8, 16, 24 and 32, setting 3BYTE at 24.
CHECK_BPS = (8, 16, 24, 32)
# The restricted option set exists only up to 4 bits, which the file never
# reaches. The same patterns force its ids too.
RESTRICTED_ORDERINGS = (
    ("nopp_restricted", RESTRICTED),
    ("pp_restricted", PREPROCESS | RESTRICTED),
)


@dataclass(frozen=True)
class OptionInput:
    option: str  # "zero", "se", "uncompressed", "fs", "split_k<k>"
    expected_id: int
    id_bits: int  # how many leading bits of the stream the id occupies
    pattern: tuple[int, ...]  # repeated to fill the buffer


def option_inputs(bps: int, flags: int) -> list[OptionInput]:
    """check_zero / check_se / check_uncompressed / check_fs / check_splitting.

    The file writes each pattern at the sample's byte width, which is the same
    as n bits for the widths it uses. At other widths `pack` masks to n bits.
    `check_zero` memsets the buffer to 0x55 with preprocessing (a constant
    field) and to 0 without; the port keeps the constant, masked to n bits.
    """
    xmin, xmax = value_range(bps, flags)
    ilen = id_len(bps, flags)
    pp = bool(flags & PREPROCESS)
    kmax = (1 << ilen) - 3
    out = [
        OptionInput("zero", 0, ilen + 1, ((0x55555555 & ((1 << bps) - 1)) if pp else 0,)),
        OptionInput(
            "se",
            1,
            ilen + 1,
            (xmax - 1,) * 4 + (xmax,) * 4 if pp else (0, 0, 0, 0, 1, 0, 0, 2),
        ),
        OptionInput("uncompressed", (1 << ilen) - 1, ilen, (xmax, xmin)),
    ]
    # With a 1-bit id (the restricted set at 1 or 2 bits) id 1 is the
    # uncompressed option and there is no split, FS included.
    if kmax >= 0:
        out.append(OptionInput("fs", 1, ilen, (xmin + 2, xmin, xmin, xmin) if pp else (0, 0, 0, 4)))
    # `for (int k = 1; k < bps - 2; k++)`, bounded by the id space, which the
    # file's widths never exceed but the restricted set does.
    for k in range(1, min(bps - 3, kmax) + 1):
        if pp:
            pat = (xmin + (1 << (k - 1)) - 1, xmin, xmin + (1 << (k + 1)) - 1, xmin)
        else:
            pat = (0, (1 << k) - 1, 0, (1 << (k + 2)) - 1)
        out.append(OptionInput(f"split_k{k}", k + 1, ilen, pat))
    return out


def fill(pattern: tuple[int, ...], n: int) -> list[int]:
    reps = n // len(pattern) + 1
    return list(pattern * reps)[:n]


def emitted_id(stream: bytes, bits: int) -> int:
    return stream[0] >> (8 - bits)


# Options the ported patterns cannot force at some widths. At 1 and 2 bits the
# FS pattern's values do not fit (no preprocessing: 4 needs 3 bits) or FS is
# never cheaper than an uncompressed or low-entropy block, so the encoder
# chooses one of those. Recorded, not hand-built: the FS path is the split
# path with k = 0, which every other width exercises.
UNFORCEABLE = {("fs", 1), ("fs", 2)}


def option_field(inp: OptionInput, bps: int, flags: int, n: int) -> tuple[bytes, bytes]:
    """The encoder's input for `inp`, and what libaec's decoder must give back."""
    values = fill(inp.pattern, n)
    return pack(values, bps, flags), decoded_layout(values, bps, flags)


def check_option(
    aec: Libaec,
    inp: OptionInput,
    bps: int,
    flags: int,
    block: int,
    rsi: int,
    n: int,
    prepared: tuple[bytes, bytes] | None = None,
) -> tuple[bytes, bytes]:
    """Encode, assert the id the way check_block_sizes() does, and round-trip.

    `prepared` is `option_field(inp, bps, flags, n)`, passed in by a caller that
    checks the same field at many block sizes and RSIs, so it is built once.
    """
    data, expected = prepared if prepared is not None else option_field(inp, bps, flags, n)
    stream = aec.encode(data, bps, block, rsi, flags)
    got = emitted_id(stream, inp.id_bits)
    if got != inp.expected_id:
        raise SystemExit(
            f"check_code_options: {inp.option} at {bps} bits, flags {flags}, block {block}, "
            f"rsi {rsi} emitted id {got:#x}, expected {inp.expected_id:#x}"
        )
    status, out = aec.decode(stream, bps, block, rsi, flags, len(expected))
    if status != AEC_OK or out != expected:
        raise SystemExit(f"check_code_options: {inp.option} at {bps}/{block}/{rsi}/{flags} does not round-trip")
    return stream, expected


def check_code_options(aec: Libaec, widths: range | tuple[int, ...], exhaustive_rsi: bool) -> int:
    """Every id assertion in check_code_options.c, over `widths`.

    With `exhaustive_rsi` it walks every RSI from 1 to the buffer's maximum, as
    the file does; otherwise RSI 1, 2 and the maximum. Returns the number of
    assertions made.
    """
    count = 0
    orderings = [(name, fl) for name, fl in CHECK_ORDERINGS]
    for bps in widths:
        flag_sets = list(orderings)
        if bps <= 4:
            flag_sets += list(RESTRICTED_ORDERINGS)
        for _, base in flag_sets:
            flags = base | (THREE_BYTE if 17 <= bps <= 24 else 0)
            width = bytes_per_sample(bps, flags)
            n = CHECK_BUF_BYTES // width
            for inp in option_inputs(bps, flags):
                if (inp.option, bps) in UNFORCEABLE:
                    continue
                prepared = option_field(inp, bps, flags, n)
                for block in STANDARD_BLOCKS:
                    max_rsi = min(4096, CHECK_BUF_BYTES // (block * width))
                    rsis = range(1, max_rsi + 1) if exhaustive_rsi else sorted({1, 2, max_rsi})
                    for rsi in rsis:
                        check_option(aec, inp, bps, flags, block, rsi, n, prepared)
                        count += 1
    return count


# --------------------------------------------------------------------------
# Cases
# --------------------------------------------------------------------------


@dataclass
class AecCase:
    name: str
    kind: str
    bits_per_sample: int
    block_size: int
    rsi: int
    flags: int
    samples: int
    stream: bytes
    encoder: str = "default"
    note: str = ""
    # What the encoder was given, in libaec's decoded layout. Every complete
    # decode must reproduce it: libaec round-trips its own streams.
    source: bytes | None = None
    # Filled by the oracle.
    status: int = 0
    output: bytes = b""


@dataclass
class SzCase:
    name: str
    options_mask: int
    bits_per_pixel: int
    pixels_per_block: int
    pixels_per_scanline: int
    dest_len: int
    stream: bytes
    note: str = ""
    source: bytes = b""
    status: int = 0
    output: bytes = b""


@dataclass
class Corpus:
    grid: list[dict] = field(default_factory=list)
    aec: list[AecCase] = field(default_factory=list)
    sz: list[SzCase] = field(default_factory=list)


def flag_tag(flags: int) -> str:
    parts = []
    for bit, tag in (
        (PREPROCESS, "pp"),
        (SIGNED, "s"),
        (MSB, "msb"),
        (THREE_BYTE, "3b"),
        (RESTRICTED, "r"),
        (PAD_RSI, "pad"),
    ):
        if flags & bit:
            parts.append(tag)
    return "-".join(parts) or "raw"


def field_values(kind: str, n: int, bps: int, flags: int, seed: int) -> list[int]:
    """A deterministic field in range for `bps` / `flags`."""
    rng = random.Random(seed)
    lo, hi = value_range(bps, flags)
    span = hi - lo
    if kind == "constant":
        return [lo + span // 3] * n
    if kind == "zeros":
        return [0] * n
    if kind == "noise":
        return [lo + rng.getrandbits(bps) % (span + 1) for _ in range(n)]
    if kind == "smooth":
        # A random walk with small steps: what a physical field looks like to
        # the preprocessor.
        step = max(1, span >> 6)
        v = lo + span // 2
        out = []
        for _ in range(n):
            v += rng.randint(-step, step)
            v = min(hi, max(lo, v))
            out.append(v)
        return out
    if kind == "runs":
        # Long constant stretches between noisy blocks: zero blocks, runs that
        # cross 64-block segments and the ROS code.
        out = []
        v = lo
        while len(out) < n:
            run = rng.choice((1, 3, 7, 40, 300, 700, 2000))
            if rng.random() < 0.5:
                v = lo + rng.getrandbits(bps) % (span + 1)
            out.extend([v] * run)
            for _ in range(rng.randint(0, 20)):
                out.append(lo + rng.getrandbits(bps) % (span + 1))
        return out[:n]
    raise ValueError(kind)


def encode_case(aec: Libaec, name, kind, values, bps, block, rsi, flags, note="", encoder="default", libs=None):
    lib = libs[encoder] if libs else aec
    enc_flags = flags | (0 if block in STANDARD_BLOCKS else NOT_ENFORCE)
    stream = lib.encode(pack(values, bps, flags), bps, block, rsi, enc_flags)
    source = decoded_layout(values, bps, flags)
    return AecCase(name, kind, bps, block, rsi, flags, len(values), stream, encoder, note, source)


def option_cases(aec: Libaec, widths, orderings_for) -> list[AecCase]:
    """One committed stream per (width, ordering, option).

    The id assertion is re-made on the committed stream. The block size and RSI
    rotate so every standard block size and several RSIs appear, and the sample
    count leaves a partial RSI and a partial last block.
    """
    cases = []
    idx = 0
    for bps in widths:
        for oname, base in orderings_for(bps):
            flags = base | (THREE_BYTE if 17 <= bps <= 24 else 0)
            for inp in option_inputs(bps, flags):
                if (inp.option, bps) in UNFORCEABLE:
                    continue
                block = STANDARD_BLOCKS[idx % 4]
                rsi = (1, 2, 3, 5)[(idx // 4) % 4]
                n = min(CHECK_BUF_BYTES // bytes_per_sample(bps, flags), 2 * block * rsi + block // 2 + 1)
                stream, expected = check_option(aec, inp, bps, flags, block, rsi, n)
                name = f"opt_b{bps:02d}_{oname}_{inp.option}"
                cases.append(
                    AecCase(
                        name,
                        "option",
                        bps,
                        block,
                        rsi,
                        flags,
                        n,
                        stream,
                        note=f"check_code_options {inp.option}, id {inp.expected_id:#x}",
                        source=expected,
                    )
                )
                idx += 1
    return cases


def committed_orderings(bps: int):
    out = list(CHECK_ORDERINGS)
    if bps <= 4:
        out += list(RESTRICTED_ORDERINGS)
    return out


def field_cases(libs: dict[str, Libaec]) -> list[AecCase]:
    """Coverage beyond the option-forced set: every axis at least once."""
    aec = libs["default"]
    cases: list[AecCase] = []

    def add(name, kind, bps, block, rsi, flags, n, field_kind, seed, note="", encoder="default"):
        vals = field_values(field_kind, n, bps, flags, seed)
        cases.append(encode_case(aec, name, kind, vals, bps, block, rsi, flags, note, encoder, libs))

    # Every width, 1 to 32, over rotating flags, byte orders and fields.
    kinds = ("smooth", "noise", "runs")
    for bps in range(1, 33):
        base = [PREPROCESS, PREPROCESS | SIGNED | MSB, MSB, PREPROCESS | MSB, SIGNED][bps % 5]
        flags = base | (THREE_BYTE if 17 <= bps <= 24 and bps % 2 else 0)
        kind = kinds[bps % 3]
        add(f"width_b{bps:02d}_{flag_tag(flags)}_{kind}", "field", bps, 16, 8, flags, 700, kind, 1000 + bps)

    # Block sizes the standard does not allow and libaec's decoder does, which
    # HDF5 writes. Encoded with NOT_ENFORCE, which decoding ignores.
    for block in (2, 4, 6, 10, 18, 34, 128, 256):
        for flags in (0, PREPROCESS):
            add(
                f"block_{block:03d}_b12_{flag_tag(flags)}",
                "field",
                12,
                block,
                3,
                flags,
                block * 7 + 5,
                "smooth",
                2000 + block,
            )
    # Block 2 with preprocessing leaves one encoded sample after the reference.
    add("block_002_b08_pp_runs", "field", 8, 2, 64, PREPROCESS, 1000, "runs", 2100)

    # Reference sample intervals, including both limits.
    for rsi in (1, 2, 3, 128, 4096):
        n = min(8 * rsi + 13, 40_000) if rsi < 4096 else 8 * 4096 + 77
        add(f"rsi_{rsi:04d}_b08_pp", "field", 8, 8, rsi, PREPROCESS, n, "smooth", 3000 + rsi)

    # The restricted option set at 1 to 4 bits, and ignored above 8.
    for bps in (1, 2, 3, 4):
        for flags in (RESTRICTED, PREPROCESS | RESTRICTED):
            add(f"restricted_b{bps}_{flag_tag(flags)}", "field", bps, 16, 4, flags, 300, "noise", 4000 + bps)
    add(
        "restricted_ignored_b12_pp-r",
        "field",
        12,
        16,
        4,
        PREPROCESS | RESTRICTED,
        300,
        "smooth",
        4012,
        note="RESTRICTED is ignored above 8 bits",
    )

    # 3BYTE at 17 to 24 bits in both byte orders, and ignored outside it.
    for bps, flags in (
        (17, PREPROCESS | THREE_BYTE),
        (24, PREPROCESS | THREE_BYTE | MSB),
        (24, PREPROCESS | THREE_BYTE | SIGNED),
        (20, THREE_BYTE | MSB | SIGNED),
        (12, PREPROCESS | THREE_BYTE),
        (28, PREPROCESS | THREE_BYTE | MSB),
    ):
        add(f"threebyte_b{bps}_{flag_tag(flags)}", "field", bps, 32, 4, flags, 500, "smooth", 5000 + bps)

    # SIGNED without preprocessing: sign extension alone.
    for bps in (5, 12, 32):
        add(f"signed_nopp_b{bps:02d}", "field", bps, 16, 4, SIGNED, 200, "noise", 5100 + bps)

    # PAD_RSI, from the padded build: with and without preprocessing, zero
    # blocks at an RSI end, and an RSI of one block.
    for bps, block, rsi, flags, kind in (
        (8, 16, 4, PAD_RSI, "noise"),
        (8, 16, 4, PAD_RSI | PREPROCESS, "smooth"),
        (12, 8, 1, PAD_RSI | PREPROCESS | MSB, "runs"),
        (32, 16, 16, PAD_RSI | PREPROCESS | MSB, "smooth"),
        (3, 64, 2, PAD_RSI, "runs"),
    ):
        add(
            f"padrsi_b{bps:02d}_j{block}_r{rsi}_{flag_tag(flags)}_{kind}",
            "field",
            bps,
            block,
            rsi,
            flags,
            block * rsi * 5 + 3,
            kind,
            6000 + bps + block + rsi,
            encoder="rsi_padding",
        )

    # Sample counts: one sample, a partial first block, and a field that is a
    # constant (all zero blocks, ROS to the RSI end).
    add("count_1_b16_pp", "field", 16, 16, 4, PREPROCESS, 1, "noise", 7001)
    add("count_1_b16_raw", "field", 16, 16, 4, 0, 1, "noise", 7002)
    add("count_15_b16_pp", "field", 16, 16, 4, PREPROCESS, 15, "smooth", 7003)
    add("count_block_plus_one_b10", "field", 10, 32, 3, PREPROCESS, 33, "smooth", 7004)
    add("constant_b16_pp_long", "field", 16, 32, 128, PREPROCESS, 32 * 128 * 3 + 5, "constant", 7005)
    add("zeros_b16_raw_long", "field", 16, 64, 100, 0, 64 * 100 + 64 * 7, "zeros", 7006)
    add("runs_b24_pp_msb", "field", 24, 32, 64, PREPROCESS | MSB, 20_000, "runs", 7007)
    add("runs_b07_raw", "field", 7, 8, 200, 0, 12_000, "runs", 7008)

    # Truncated streams: libaec returns AEC_OK with short output (ADR-0012
    # decision 4 makes this an error for fieldglass-aec).
    base = encode_case(aec, "", "", field_values("smooth", 4096, 16, PREPROCESS, 8001), 16, 32, 16, PREPROCESS | MSB)
    for name, cut in (("truncated_half_b16", len(base.stream) // 2), ("truncated_one_byte_b16", len(base.stream) - 1)):
        cases.append(
            AecCase(
                name,
                "truncated",
                16,
                32,
                16,
                PREPROCESS | MSB,
                4096,
                base.stream[:cut],
                note=f"stream cut to {cut} of {len(base.stream)} bytes",
                source=base.source,
            )
        )
    # Trailing garbage after the last block is ignored.
    cases.append(
        AecCase(
            "trailing_garbage_b16",
            "trailing",
            16,
            32,
            16,
            PREPROCESS | MSB,
            4096,
            base.stream + b"\xff" * 100,
            note="100 bytes of 0xff appended",
            source=base.source,
        )
    )

    cases.append(trailing_zero_block_case(aec))
    cases.append(se_pair_sum_case(libs))
    return cases


def trailing_zero_block_case(aec: Libaec) -> AecCase:
    """AEC_DATA_ERROR after every requested sample is out (ADR-0012 decision 4).

    A stream whose samples end exactly on an RSI boundary, followed by bytes
    that parse as a zero block longer than the next RSI. The last RSI is
    flushed before the next id is read, so the output is complete and right;
    then `m_zero_block` checks the run against the RSI before it checks for
    room (`decode.c:529-541`) and returns M_ERROR. The first appended byte that
    does this is taken, so the search is deterministic.
    """
    bps, block, rsi = 8, 16, 4
    n = block * rsi * 3
    base = encode_case(aec, "", "", field_values("noise", n, bps, 0, 8101), bps, block, rsi, 0)
    for extra in [bytes([b]) for b in range(256)] + [bytes([a, b]) for a in range(256) for b in range(256)]:
        stream = base.stream + extra
        status, out = aec.decode(stream, bps, block, rsi, 0, n)
        if status != AEC_OK and out == base.source:
            return AecCase(
                "trailing_zero_block_overrun_b08",
                "trailing_fill",
                bps,
                block,
                rsi,
                0,
                n,
                stream,
                note=f"libaec returns {status} after every sample; appended {extra.hex()} parses as a zero block past the RSI",
                source=base.source,
            )
    raise SystemExit("no appended bytes reproduce the trailing zero-block AEC_DATA_ERROR")


def is_se_table_rejection(libs: dict[str, Libaec], case: AecCase) -> bool:
    """Whether libaec refuses `case` only because of its second-extension table.

    The stock decoder must return AEC_DATA_ERROR (-3), and the wide-table build
    must decode the same stream to exactly the field the encoder was given.
    Anything else is a failure nobody has explained, and must stop the build.
    """
    args = (case.stream, case.bits_per_sample, case.block_size, case.rsi, case.flags)
    status, _ = libs["default"].decode(*args, len(case.source))
    if status != AEC_DATA_ERROR:
        return False
    wide_status, wide_out = libs["wide_se_table"].decode(*args, len(case.source))
    return wide_status == AEC_OK and wide_out == case.source


def se_pair_sum_case(libs: dict[str, Libaec]) -> AecCase:
    """A stream libaec's own encoder writes and its decoder rejects.

    The encoder bounds a second-extension block only by its total length
    (`assess_se_option`, `encode.c:396-416`), so at 3 bits and block 256 it can
    emit a pair whose sum is 13 or 14. The decoder's table stops at a sum of 12
    (`SE_TABLE_SIZE`, 90), so it returns AEC_DATA_ERROR mid-stream
    (`decode.c:566, 599`). CCSDS 121.0-B-3 puts no bound on the sum.

    These are the parameters of the planning spike's example for ADR-0012
    decision 4 (3 bits, block 256, RSI 3, 2,050 samples). That example was
    really this rejection, misread because `total_out` reports the full room
    after an error: `aec_decode` adds `avail_out` to it on entry
    (`decode.c:824`) and returns on M_ERROR before subtracting it or flushing
    (`decode.c:830-831`). The trailing-fill behaviour decision 4 describes is
    real, and `trailing_zero_block_overrun_b08` pins it.

    The samples libaec really produced are counted by decoding one sample per
    call. The manifest records that prefix and the digest of the field the
    encoder was given. The case is only accepted if `is_se_table_rejection`
    confirms the cause.
    """
    aec = libs["default"]
    bps, block, rsi, n, flags = 3, 256, 3, 2050, PREPROCESS
    for seed in range(10_000):
        vals = field_values("noise", n, bps, flags, 9000 + seed)
        stream = aec.encode(pack(vals, bps, flags), bps, block, rsi, flags | NOT_ENFORCE)
        status, _ = aec.decode(stream, bps, block, rsi, flags, n)
        if status == AEC_OK:
            continue
        case = AecCase(
            "se_pair_sum_over_12_b03_j256_r3_pp",
            "libaec_rejects",
            bps,
            block,
            rsi,
            flags,
            n,
            stream,
            note=(
                "libaec's encoder writes a second-extension pair sum above 12 and its decoder "
                f"returns {status} (seed {9000 + seed}); a decoder with a wider table reads it exactly"
            ),
            source=decoded_layout(vals, bps, flags),
        )
        if not is_se_table_rejection(libs, case):
            raise SystemExit(f"seed {9000 + seed}: libaec returns {status}, and not because of its SE table")
        return case
    raise SystemExit("no stream reproduces the second-extension rejection")


def sz_cases(sz: Libsz) -> list[SzCase]:
    """libsz cases for `fieldglass_aec::sz` (#761)."""
    cases: list[SzCase] = []
    masks = (SZ_NN | SZ_MSB, SZ_NN | SZ_LSB, SZ_EC | SZ_MSB, SZ_EC | SZ_LSB)
    idx = 0

    def data_for(bpp: int, pixels: int, mask: int, seed: int) -> bytes:
        rng = random.Random(seed)
        if bpp in (32, 64):
            # A smooth field of floats, stored in the mask's byte order.
            fmt = (">" if mask & SZ_MSB else "<") + ("f" if bpp == 32 else "d")
            v = 280.0
            out = bytearray()
            for _ in range(pixels):
                v += rng.uniform(-0.5, 0.5)
                out += struct.pack(fmt, v)
            return bytes(out)
        width = 1 if bpp <= 8 else 2 if bpp <= 16 else 4
        order = "big" if mask & SZ_MSB else "little"
        hi = (1 << bpp) - 1
        v = hi // 2
        step = max(1, hi >> 7)
        out = bytearray()
        for _ in range(pixels):
            v = min(hi, max(0, v + rng.randint(-step, step)))
            out += v.to_bytes(width, order)
        return bytes(out)

    def add(name, mask, bpp, ppb, pps, pixels, seed, note=""):
        data = data_for(bpp, pixels, mask, seed)
        stream = sz.compress(data, mask, bpp, ppb, pps)
        cases.append(SzCase(name, mask, bpp, ppb, pps, len(data), stream, note, data))

    for bpp in (8, 12, 16, 24, 32, 64):
        for ppb in (2, 8, 10, 16, 18, 32):
            for exact in (True, False):
                mask = masks[idx % 4]
                idx += 1
                pps = ppb * 4 if exact else ppb * 4 + ppb // 2 + 1
                if pps % ppb == 0 and not exact:
                    pps += 1
                pixels = pps * 3 + (0 if exact else pps // 3)
                tag = "exact" if exact else "padded"
                add(f"sz_b{bpp:02d}_ppb{ppb:02d}_{tag}", mask, bpp, ppb, pps, pixels, 10_000 + bpp * 100 + ppb * 2 + exact)

    # One block per scanline (RSI 1), with pps below and equal to ppb.
    add("sz_b16_rsi1_pps_lt_ppb", SZ_NN | SZ_LSB, 16, 32, 20, 20 * 9, 11_001, "rsi = ceil(20 / 32) = 1")
    add("sz_b16_rsi1_pps_eq_ppb", SZ_NN | SZ_MSB, 16, 32, 32, 32 * 9, 11_002, "rsi = 1")
    add("sz_b08_pps1", SZ_NN | SZ_LSB, 8, 32, 1, 50, 11_003, "one pixel per scanline, 31 pad samples each")
    # A bpp-32 chunk whose byte-plane edge falls mid-scanline: 150 pixels give
    # 150-byte planes, and pps = 64 puts the first plane edge inside the third
    # scanline of 8-bit samples.
    add(
        "sz_b32_plane_edge_mid_scanline",
        SZ_NN | SZ_LSB,
        32,
        16,
        64,
        150,
        11_004,
        "150-byte planes over 64-sample scanlines",
    )
    add("sz_b64_plane_edge_mid_scanline", SZ_NN | SZ_MSB, 64, 10, 45, 77, 11_005, "77-byte planes over 45-sample scanlines")
    # Byte planes whose last scanline is partial by more than a block, with
    # pps a multiple of ppb (no per-line padding). libsz's add_padding still
    # fills that last scanline to a whole one, so the stream ends there, not
    # after the last block (#421 review). libhdf5 writes the first shape for
    # any long 1-D chunk: it caps pps at 128 blocks.
    for name, mask, bpp, ppb, pps, pixels, seed in (
        ("sz_b32_short_last_line_pps4096", SZ_EC | SZ_NN | SZ_MSB, 32, 32, 4096, 5000, 11_007),
        ("sz_b32_short_last_line_ppb16", SZ_EC | SZ_NN | SZ_MSB, 32, 16, 64, 40, 11_008),
        ("sz_b32_short_last_line_pps48", SZ_EC | SZ_NN | SZ_MSB, 32, 16, 48, 20, 11_009),
        ("sz_b32_short_last_line_ppb32", SZ_EC | SZ_NN | SZ_MSB, 32, 32, 32, 20, 11_010),
        ("sz_b32_short_last_line_ec", SZ_EC | SZ_MSB, 32, 8, 16, 3, 11_011),
        ("sz_b64_short_last_line_ppb32", SZ_EC | SZ_NN | SZ_MSB, 64, 32, 64, 11, 11_012),
    ):
        add(name, mask, bpp, ppb, pps, pixels, seed,
            f"{pixels} pixels over {pps}-pixel scanlines: the last is short by more than a block")
    # The options libsz ignores when decoding.
    add("sz_b16_ignored_options", SZ_NN | SZ_LSB | 1 | 2 | 128, 16, 16, 64, 64 * 5, 11_006, "K13, CHIP and RAW set")
    return cases


def params_grid(aec: Libaec) -> list[dict]:
    """`aec_decode_init`'s verdict over each axis, with the others valid.

    Rows vary one parameter at a time from a valid base (8 bits, block 16,
    RSI 128, no flags), plus every width against the restricted and 3BYTE
    flags, where acceptance depends on the pair. Values stay inside the types
    `Params::new` takes (u8, u16, u16, u8).
    """
    rows: list[tuple[int, int, int, int]] = []
    for bps in list(range(0, 41)) + [64, 128, 255]:
        rows.append((bps, 16, 128, 0))
    for block in list(range(0, 261)) + [512, 1024, 4096, 65534, 65535]:
        rows.append((8, block, 128, 0))
    for rsi in [0, 1, 2, 3, 64, 128, 1000, 4095, 4096, 4097, 4098, 8192, 65535]:
        rows.append((8, 16, rsi, 0))
    for bps in range(1, 33):
        for flags in (
            RESTRICTED,
            RESTRICTED | PREPROCESS,
            RESTRICTED | SIGNED | MSB | THREE_BYTE | PREPROCESS | PAD_RSI,
            NOT_ENFORCE,
            0xFF,
        ):
            rows.append((bps, 16, 128, flags))
    # Every flag byte at one width inside the restricted range and one outside.
    for bps in (4, 6):
        for flags in range(256):
            rows.append((bps, 16, 128, flags))
    seen = set()
    out = []
    for row in rows:
        if row in seen:
            continue
        seen.add(row)
        bps, block, rsi, flags = row
        out.append(
            {
                "bits_per_sample": bps,
                "block_size": block,
                "rsi": rsi,
                "flags": flags,
                "status": aec.decode_init(bps, block, rsi, flags),
            }
        )
    return out


# --------------------------------------------------------------------------
# Oracle and output
# --------------------------------------------------------------------------


def run_oracle(corpus: Corpus, aec: Libaec, sz: Libsz) -> None:
    """Decode every case with libaec / libsz and check it says what we expect.

    Every complete decode must also equal the field the encoder was given, so a
    case only enters the corpus if libaec round-trips it.
    """
    for case in corpus.aec:
        out_len = case.samples * bytes_per_sample(case.bits_per_sample, case.flags)
        args = (case.stream, case.bits_per_sample, case.block_size, case.rsi, case.flags)
        case.status, case.output = aec.decode(*args, out_len)
        full = len(case.output) == out_len
        if case.kind == "truncated":
            ok = case.status == AEC_OK and not full and case.source.startswith(case.output)
        elif case.kind == "trailing_fill":
            ok = case.status != AEC_OK and case.output == case.source
        elif case.kind == "libaec_rejects":
            status, prefix = aec.decode_stepwise(*args, case.samples)
            ok = case.status != AEC_OK and status == case.status and len(prefix) < out_len
            ok = ok and case.source.startswith(prefix)
            case.output = prefix
        else:
            ok = case.status == AEC_OK and full and case.output == case.source
        if not ok:
            raise SystemExit(
                f"self-check: libaec gives status {case.status}, {len(case.output)}/{out_len} bytes for "
                f"{case.name} ({case.kind})"
            )
    for case in corpus.sz:
        case.status, case.output = sz.decompress(
            case.stream,
            case.options_mask,
            case.bits_per_pixel,
            case.pixels_per_block,
            case.pixels_per_scanline,
            case.dest_len,
        )
        if case.status != AEC_OK or case.output != case.source:
            raise SystemExit(f"self-check: libsz gives {case.status}, {len(case.output)} bytes for {case.name}")


def check_names(corpus: Corpus) -> None:
    names = [c.name for c in corpus.aec] + [c.name for c in corpus.sz]
    dupes = {n for n in names if names.count(n) > 1}
    if dupes:
        raise SystemExit(f"duplicate case names: {sorted(dupes)}")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def aec_row(c: AecCase) -> dict:
    return {
        "name": c.name,
        "kind": c.kind,
        "stream": f"streams/{c.name}.rz",
        "stream_sha256": sha256(c.stream),
        "encoder": c.encoder,
        "bits_per_sample": c.bits_per_sample,
        "block_size": c.block_size,
        "rsi": c.rsi,
        "flags": c.flags,
        "samples": c.samples,
        "libaec_status": c.status,
        "total_out": len(c.output),
        "output_sha256": sha256(c.output),
        **({"source_sha256": sha256(c.source)} if c.kind == "libaec_rejects" else {}),
        "note": c.note,
    }


def sz_row(c: SzCase) -> dict:
    return {
        "name": c.name,
        "stream": f"streams/{c.name}.rz",
        "stream_sha256": sha256(c.stream),
        "options_mask": c.options_mask,
        "bits_per_pixel": c.bits_per_pixel,
        "pixels_per_block": c.pixels_per_block,
        "pixels_per_scanline": c.pixels_per_scanline,
        "dest_len": c.dest_len,
        "libsz_status": c.status,
        "total_out": len(c.output),
        "output_sha256": sha256(c.output),
        "note": c.note,
    }


def render_manifest(corpus: Corpus, check_count: int) -> str:
    """One row per line, so a regeneration diff reads case by case."""
    header = {
        "generator": "tools/build_aec_fixtures.py",
        "libaec_version": LIBAEC_VERSION,
        "libaec_commit": LIBAEC_COMMIT,
        "libaec_tarball_url": SOURCE_URL,
        "libaec_tarball_sha256": SOURCE_SHA256,
        "cmake_flags": {
            "default": " ".join(CMAKE_COMMON),
            "rsi_padding": " ".join([*CMAKE_COMMON, RSI_PADDING_FLAGS]),
        },
        "check_code_options_assertions": check_count,
        "counts": {
            "params_grid": len(corpus.grid),
            "aec_cases": len(corpus.aec),
            "sz_cases": len(corpus.sz),
        },
    }

    def block(key: str, rows: list[dict]) -> str:
        body = ",\n".join("    " + json.dumps(r, ensure_ascii=True) for r in rows)
        return f'  "{key}": [\n{body}\n  ]'

    parts = ['  "header": ' + json.dumps(header, indent=2, ensure_ascii=True).replace("\n", "\n  ")]
    parts.append(block("params_grid", corpus.grid))
    parts.append(block("aec_cases", [aec_row(c) for c in corpus.aec]))
    parts.append(block("sz_cases", [sz_row(c) for c in corpus.sz]))
    return "{\n" + ",\n".join(parts) + "\n}\n"


def write_corpus(out: Path, corpus: Corpus, check_count: int) -> None:
    streams = out / "streams"
    if streams.exists():
        shutil.rmtree(streams)
    streams.mkdir(parents=True)
    for c in corpus.aec:
        (streams / f"{c.name}.rz").write_bytes(c.stream)
    for c in corpus.sz:
        (streams / f"{c.name}.rz").write_bytes(c.stream)
    (out / "manifest.json").write_text(render_manifest(corpus, check_count), encoding="utf-8", newline="\n")


def dump_expected(out: Path, corpus: Corpus) -> None:
    """libaec's decoded bytes, for reading a failing case by eye."""
    out.mkdir(parents=True, exist_ok=True)
    for c in corpus.aec:
        (out / f"{c.name}.bin").write_bytes(c.output)
    for c in corpus.sz:
        (out / f"{c.name}.bin").write_bytes(c.output)


def full_corpus(libs: dict[str, Libaec]) -> list[AecCase]:
    """The large matrix (#763): too big to commit, generated in CI.

    Every width 1 to 32, over a spread of block sizes, RSIs and flag sets, three
    field kinds each, plus every option-forced input at every width.
    """
    aec = libs["default"]
    cases = option_cases(aec, range(1, 33), committed_orderings)
    flag_sets = (
        0,
        PREPROCESS,
        PREPROCESS | SIGNED,
        PREPROCESS | MSB,
        PREPROCESS | SIGNED | MSB,
        SIGNED | MSB,
        PREPROCESS | THREE_BYTE,
    )
    idx = 0
    for bps in range(1, 33):
        for block in (2, 8, 10, 16, 18, 32, 64, 256):
            for flags in flag_sets:
                if flags & THREE_BYTE and not 17 <= bps <= 24:
                    continue
                rsi = (1, 2, 3, 128, 4096)[idx % 5]
                kind = ("smooth", "noise", "runs")[idx % 3]
                n = min(block * rsi * 2 + 7, 20_000)
                vals = field_values(kind, n, bps, flags, 20_000 + idx)
                name = f"full_b{bps:02d}_j{block:03d}_r{rsi:04d}_{flag_tag(flags)}_{kind}"
                cases.append(label_rejects(libs, encode_case(aec, name, "field", vals, bps, block, rsi, flags)))
                idx += 1
    for bps in range(1, 33):
        for block in (8, 16, 32, 64):
            for flags in (PAD_RSI, PAD_RSI | PREPROCESS, PAD_RSI | PREPROCESS | MSB | SIGNED):
                rsi = (1, 3, 64)[idx % 3]
                n = block * rsi * 3 + 5
                vals = field_values("runs", n, bps, flags, 30_000 + idx)
                name = f"full_pad_b{bps:02d}_j{block:03d}_r{rsi:04d}_{flag_tag(flags)}"
                case = encode_case(aec, name, "field", vals, bps, block, rsi, flags, encoder="rsi_padding", libs=libs)
                cases.append(label_rejects(libs, case))
                idx += 1
    return cases


def label_rejects(libs: dict[str, Libaec], case: AecCase) -> AecCase:
    """Mark a matrix case libaec's decoder refuses for the known reason.

    At large blocks and few bits libaec's encoder can write a second-extension
    pair its decoder rejects (see `se_pair_sum_case`). A case that fails for
    that reason, confirmed by `is_se_table_rejection`, is kept and labelled.
    Any other failure stops the build: it is unexplained, and relabelling it
    would hide it.
    """
    args = (case.stream, case.bits_per_sample, case.block_size, case.rsi, case.flags)
    status, _ = libs["default"].decode(*args, len(case.source))
    if status == AEC_OK:
        return case
    if not is_se_table_rejection(libs, case):
        raise SystemExit(f"{case.name}: libaec returns {status}, and not because of its SE table")
    case.kind = "libaec_rejects"
    print(f"{case.name}: libaec's SE table refuses it; labelled libaec_rejects", file=sys.stderr)
    return case


def generate(libs: dict[str, Libaec], sz: Libsz, full: bool) -> tuple[Corpus, int]:
    aec = libs["default"]
    # The file's own assertions, exhaustively, then every other width at three
    # RSIs per block size.
    check_count = check_code_options(aec, CHECK_BPS, exhaustive_rsi=True)
    check_count += check_code_options(aec, [b for b in range(1, 33) if b not in CHECK_BPS], exhaustive_rsi=False)
    corpus = Corpus()
    if full:
        corpus.aec = full_corpus(libs)
    else:
        corpus.grid = params_grid(aec)
        corpus.aec = option_cases(aec, CHECK_BPS, committed_orderings) + option_cases(
            aec, (1, 2, 3, 4), lambda _b: RESTRICTED_ORDERINGS
        )
        corpus.aec += field_cases(libs)
        corpus.sz = sz_cases(sz)
    check_names(corpus)
    run_oracle(corpus, aec, sz)
    check_padding_build(libs)
    return corpus, check_count


def check_padding_build(libs: dict[str, Libaec]) -> None:
    """Prove the two builds really differ, so PAD_RSI streams are padded.

    If a future libaec (or a cmake cache) dropped the macro, the padded build
    would silently write the same streams as the default one.
    """
    vals = field_values("noise", 16 * 4 * 3, 8, 0, 42)
    data = pack(vals, 8, 0)
    plain = libs["default"].encode(data, 8, 16, 4, PAD_RSI)
    padded = libs["rsi_padding"].encode(data, 8, 16, 4, PAD_RSI)
    if plain == padded:
        raise SystemExit("the ENABLE_RSI_PADDING build writes the same stream as the default one")


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", type=Path, default=FIXTURES, help="corpus directory (default: the crate's fixtures)")
    ap.add_argument("--tarball", type=Path, help="use this copy of the libaec tarball instead of downloading")
    ap.add_argument("--dump-expected", type=Path, metavar="DIR", help="also write libaec's decoded bytes here")
    ap.add_argument("--full", type=Path, metavar="DIR", help="write the large matrix here instead of the corpus")
    args = ap.parse_args(argv)

    source = fetch_source(args.tarball)
    with tempfile.TemporaryDirectory(prefix="libaec-") as tmp:
        dirs = build_libaec(Path(tmp), source)
        libs = {name: Libaec(d / "libaec.so") for name, d in dirs.items()}
        # libsz links `libaec.so.0` by soname, which is already loaded from
        # the default build, so only the default libsz is used.
        sz = Libsz(dirs["default"] / "libsz.so")
        corpus, check_count = generate(libs, sz, full=args.full is not None)

    out = args.full if args.full is not None else args.out
    write_corpus(out, corpus, check_count)
    if args.dump_expected is not None:
        dump_expected(args.dump_expected, corpus)
    size = sum(p.stat().st_size for p in out.rglob("*") if p.is_file())
    print(
        f"{len(corpus.grid)} grid rows, {len(corpus.aec)} aec cases "
        f"({sum(c.kind == 'libaec_rejects' for c in corpus.aec)} refused by libaec's SE table), "
        f"{len(corpus.sz)} sz cases, {check_count} check_code_options assertions; {size} bytes under {out}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
