# Test fixture provenance

Every file in this directory except this one was written by
`tools/build_aec_fixtures.py` in the repository root. None of the streams or
expected outputs were produced by this repository's code: libaec's encoder wrote
every stream, and libaec's decoder (or libsz, for the szip cases) produced
every expected output. A decode that matches `manifest.json` is matching libaec,
not a second copy of our own arithmetic.

Never edit `manifest.json` or a stream by hand. Regenerate:

```sh
python3 tools/build_aec_fixtures.py
```

It needs cmake 3.26 or newer, a C compiler and network access (or
`--tarball PATH` with a local copy of the tarball below). Running it twice gives
a byte-identical tree. The `AEC oracle` CI job (`.github/workflows/aec-oracle.yml`,
#763) regenerates the corpus and fails on any diff; `python3 tools/aec_oracle.py`
runs the same job locally.

## The oracle

| | |
| --- | --- |
| Library | libaec 1.1.7 |
| Tag | `v1.1.7`, commit `0c4c01463d2c64a112a61271d317b74efb660608` |
| Tarball | <https://github.com/MathisRosenhauer/libaec/archive/refs/tags/v1.1.7.tar.gz> |
| Tarball SHA-256 | `26661a569a7def45a2e97fbbd09e0dc5bbb2f8ab1b41250c19e795559eec6fb2` |
| Default build | `cmake -DCMAKE_BUILD_TYPE=Release -DBUILD_TESTING=OFF -DBUILD_STATIC_LIBS=OFF` |
| Padded build | the same plus `-DCMAKE_C_FLAGS=-DENABLE_RSI_PADDING` |

The generator refuses a tarball with any other digest. It builds both flavours
in a temporary directory and loads `libaec.so` and `libsz.so` through `ctypes`.

The padded build exists because libaec's default encoder never writes RSI
padding: the code is behind `#ifdef ENABLE_RSI_PADDING` (`encode.c:480`), and
nothing in libaec's CMake defines it. Only the padded build's encoder is used,
for the five `padrsi_*` streams. The macro does not touch the decoder, so every
expected output comes from the default build. The generator checks the two
builds really write different `PAD_RSI` streams.

## What is here

- `streams/*.rz`: one stream per case, exactly the bytes libaec was given to
  decode.
- `manifest.json`:
  - `header`: the libaec version, commit, tarball URL and digest, both cmake
    flag sets, the case counts, and how many `check_code_options` id assertions
    the generator made.
  - `params_grid` (981 rows): parameter sets handed to `aec_decode_init`, with
    its status (0 accepts, -1 is `AEC_CONF_ERROR`). The grid varies one
    parameter at a time from a valid base (8 bits, block 16, RSI 128, no
    flags), so each rejection is for the one parameter that differs. It adds
    every width against the restricted and 3BYTE flags, and every flag byte at
    4 and 6 bits, where acceptance depends on the pair.
  - `aec_cases` (540 rows): parameters, sample count, libaec's status, the
    bytes it wrote and their SHA-256, and the SHA-256 of the stream file.
  - `sz_cases` (84 rows): `SZ_com_t` parameters (in libsz's order: mask, bits
    per pixel, pixels per block, pixels per scanline, which is not HDF5's
    `cd_values` order), the requested length, libsz's status, the bytes it
    wrote and their SHA-256.

The tests pin the three counts, so a missing, emptied or partial manifest
fails, and they check that every referenced stream exists with its recorded
digest and that every stream is referenced.

To see libaec's decoded bytes when a case fails, write them to a directory git
ignores:

```sh
python3 tools/build_aec_fixtures.py --out "$(mktemp -d)" --dump-expected crates/fieldglass-aec/tests/expected
```

## The cases

**Option-forced (`opt_*`, 450).** Inputs ported from libaec's
`tests/check_code_options.c`: a constructed field per coding option (zero block,
second extension, uncompressed, FS, split with every k from 1 to its maximum)
at 8, 16, 24 and 32 bits, in the file's five orderings (no preprocessing; and
preprocessed LSB unsigned, LSB signed, MSB unsigned, MSB signed), with 3BYTE at
24 bits as the file sets it. The same patterns also force the restricted option
set at 1 to 4 bits, which the file never reaches (30 more). The generator
asserts the option id each stream starts with, the way `check_block_sizes()`
does. Before it writes anything it makes every one of the file's assertions over
its full block-size and RSI loops, and the same assertions at every other width
from 1 to 32 at three RSIs per block size: 151,680 in all.

One option cannot be forced at every width: **FS at 1 and 2 bits.** The FS
pattern's values do not fit in 1 bit, and at 2 bits an FS block is never cheaper
than an uncompressed or low-entropy one, so the encoder picks one of those.
Those cases are left out rather than hand-built: FS is the split path with
k = 0, which every other width exercises. There are **no hand-built streams**
in this corpus.

**Coverage (`width_*`, `block_*`, `rsi_*`, `restricted_*`, `threebyte_*`,
`signed_nopp_*`, `padrsi_*`, `count_*`, `zeros_*`, `constant_*`, `runs_*`, 85).**
Seeded fields (smooth, noise, long runs, constant) chosen so every axis appears
at least once: every width from 1 to 32; block sizes 2, 4, 6, 10, 18, 34, 128
and 256 (encoded with `AEC_NOT_ENFORCE`, which the decoder ignores); RSIs of 1,
2, 3, 128 and 4096; the restricted set at 1 to 4 bits and ignored at 12; 3BYTE
at 17 to 24 bits in both byte orders and ignored at 12 and 28; SIGNED without
preprocessing; `PAD_RSI` with and without preprocessing; one sample, a partial
first block, a partial last block, and zero-block runs across 64-block segments
and RSI ends.

**Stream edges (5).**

- `truncated_half_b16`, `truncated_one_byte_b16`: the same stream cut short.
  libaec returns `AEC_OK` with short output; `fieldglass-aec` must return an
  error (ADR-0012 decision 4).
- `trailing_garbage_b16`: 100 bytes of `0xff` after the stream. libaec ignores
  them.
- `trailing_zero_block_overrun_b08`: a stream that ends exactly on an RSI
  boundary, followed by one byte (`0x01`) that parses as a zero block longer
  than the next RSI. libaec flushes every requested sample, then returns
  `AEC_DATA_ERROR`, because `m_zero_block` checks the run against the RSI
  before it checks for room (`decode.c:529-541`). This is the case ADR-0012
  decision 4 describes: `fieldglass-aec` stops at the requested count and
  returns the same bytes with `Ok`.
- `se_pair_sum_over_12_b03_j256_r3_pp`: a stream libaec's decoder refuses and
  the standard reads, so `fieldglass-aec` must decode it to `source_sha256`
  (ADR-0012 decision 4); see below.

**szip (`sz_*`, 84).** libsz's `SZ_BufftoBuffCompress` output, decoded with
`SZ_BufftoBuffDecompress`: bits per pixel 8, 12, 16, 24, 32 and 64 against
pixels per block 2, 8, 10, 16, 18 and 32, each once with a scanline that is a
multiple of the block and once padded; one block per scanline (RSI 1) with a
scanline shorter than and equal to the block; one pixel per scanline; a 32-bit
and a 64-bit chunk whose byte-plane edge falls mid-scanline; the option
bits libsz ignores (K13, CHIP, RAW); and six 32- and 64-bit chunks
(`sz_b*_short_last_line_*`) whose last scanline is short by more than a block
with the scanline a multiple of the block, which libsz's `add_padding` still
fills to a whole scanline (#421). The first, 5,000 pixels at 4,096 per
scanline, is the shape libhdf5 writes for a long 1-D chunk. `NN` and `EC`, `MSB` and `LSB` rotate
through the grid.

## What libaec does that the ADR did not say

Found while building this corpus. ADR-0012 now records the first two
(decisions 3 and 4, amended in #759); they stay here as the corpus's own
provenance.

- **SIGNED without preprocessing is not sign-extended.** Sign extension happens
  in the postprocessor (`decode.c:55-127`); without `PREPROCESS` libaec copies
  the n-bit pattern out as it is. The `signed_nopp_*` cases, and the `width_*`
  cases with SIGNED alone, pin it.
- **libaec's decoder rejects a stream its own encoder writes.** The encoder
  bounds a second-extension block only by its total length
  (`assess_se_option`, `encode.c:396-416`). The decoder's table stops at a
  pair sum of 12 (`SE_TABLE_SIZE`, 90 entries past zero), and a larger codeword
  is `AEC_DATA_ERROR` (`decode.c:566, 599`). CCSDS 121.0-B-3 puts no bound on
  the sum. At 3 bits and block 256 the encoder writes sums of 13 and 14, and
  libaec cannot read its own stream back. The planning spike's example for
  ADR-0012 decision 4 (3 bits, block 256, RSI 3, 2,050 samples) was really this
  rejection, misread because `total_out` reports the full room after an error
  (next point). The trailing-fill behaviour decision 4 describes is real, and
  is pinned by `trailing_zero_block_overrun_b08`.
  `se_pair_sum_over_12_b03_j256_r3_pp` records the rejection, as kind
  `libaec_rejects`: libaec's status, the 2,047 samples it produced before the
  error (counted by decoding one sample per call) and their digest, and
  `source_sha256`, the digest of the field the encoder was given. The
  generator only accepts such a case after confirming the cause: it builds a
  third libaec whose decoder table covers pair sums up to 128, and that build
  must decode the stream to exactly the source field. It never supplies an
  expected output. The same check guards every rejection in the `--full`
  matrix; any other failure stops the generator.
- **`total_out` means nothing after an error.** `aec_decode` adds `avail_out`
  to `total_out` on entry (`decode.c:824`) and returns on `M_ERROR` before
  subtracting it or flushing (`decode.c:830-831`), so `aec_buffer_decode` reports the full room it
  was given, and the last partial RSI is never written. A reported
  `total_out` equal to the requested length is not evidence the output is
  complete.

## Where libsz and `sz::decompress` part

Found while writing `fieldglass_aec::sz` (#761), from libsz 1.1.7 built as
above and called through `ctypes`. All three are in ADR-0012's table of
divergences, and `tests/sz_corpus.rs` pins each.

- **A stream that runs out is success, in two ways.** `sz::decompress`
  returns `AecError::Truncated` for both.
  - Unpadded scanlines (`pps` a multiple of `ppb`): libsz returns `SZ_OK` and
    lowers `destLen` to what it decoded (`sz_compat.c:302-303`). All 39
    unpadded `sz_*` streams, cut in half, do this; `sz_b16_ppb16_exact` gives
    192 of 384 bytes.
  - Padded scanlines: libsz sets the length to `scanlines × pps` pixels
    (`sz_compat.c:295`), never lowers `destLen`, and returns `SZ_OK` at full
    length. The bytes past what it decoded are copied from its padded buffer,
    which `malloc` left uninitialised, so they vary from run to run. All 39
    padded `sz_*` streams, cut in half, do this; `sz_b16_ppb10_padded` cut to
    115 bytes gives `SZ_OK` with `destLen` 306 of 306.
  - HDF5 checks the length only with `assert(size_out == nalloc)`
    (`H5Zszip.c:300` on HDF5's `develop` branch), which a release build
    compiles out, and returns `size_out` as the chunk size (`:309`). So a
    release HDF5 passes either result on.
- **A 32- or 64-bit output that is not a whole number of pixels comes back
  misplaced.** libsz deinterleaves with planes of `destLen / w` bytes, rounded
  down (`sz_compat.c:84-93, 305-306`). Decoding `sz_b32_plane_edge_mid_scanline`
  with its own parameters (mask 40, 32 bits, 16 per block, 64 per scanline)
  into 599 bytes instead of 600 returns `SZ_OK` with `destLen` 599: 165 bytes
  are misplaced and the last 3 are never written (168 differ from the
  600-byte decode). This only happens at a length that is not whole pixels;
  HDF5 never asks for one. `sz::decompress` returns `AecError::OutputLength`.
- **libsz reads past the last pixel.** With padded scanlines it decodes every
  scanline whole, so a bad code in a block after the last requested pixel
  fails the call. `sz::decompress` stops at the last pixel and returns `Ok`
  with the same bytes. The test `a_bad_code_after_the_last_pixel_is_never_read`
  builds such a stream by hand (8 bits, 2 per block, 5 per scanline, a
  zero-block run past the end of the second scanline's interval): libsz
  returns -3 (`AEC_DATA_ERROR`) for a 6-byte output and `SZ_OK` for 5 bytes.

## Why the CCSDS sample data is not here

libaec's tarball ships the CCSDS 121.0-B-2 sample data (`data/121B2TestData`).
Permission to distribute it was given to libaec (its `THANKS` file thanks Aaron
Kiely "to let us distribute BB121B2 test data with libaec"), not to us. The CI
oracle job reads it from the pinned tarball instead (#763), and decodes the
66 streams libaec's `tests/sampledata.sh` names, with that script's parameters.

## Licence of the ported inputs

The option-forced inputs are ported from libaec's `tests/check_code_options.c`,
copyright 2026 Mathis Rosenhauer, Moritz Hanke, Joerg Behrens and Luis
Kornblueh, under the BSD-2-Clause licence. The full notice is in the crate's
`NOTICE` file.
