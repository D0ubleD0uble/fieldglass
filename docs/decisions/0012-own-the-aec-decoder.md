# 0012 — Own the AEC decoder

**Status:** Accepted (2026-09-29). Amends ADR-0001's 5.42 decision. Shapes
milestone 13, "Own the AEC decoder (CCSDS 121.0 / szip)": #756, #758–#764, and
the szip half of #421.

**Amended** (2026-09-29, #759): the standard defines correct and libaec is the
oracle, not the goal (decision 3); decision 4 lists every known divergence with
its evidence; and SIGNED output is sign-extended only with PREPROCESS
(decisions 3 and 7).

**Amended** (2026-09-29, #762): GRIB2 5.42 now decodes through this crate
under decision 5's rules, so the "fieldglass today" column of the table below
is the state before #762. Flag 46 now decodes to the source field. Its
eccodes snapshot still records eccodes' failure, so it stays exempt from the
snapshot value check beside flags 13 and 36, with the source fixture's values
as its oracle.

**Amended** (2026-09-29, #763): decision 8's full matrix is 4,819 cases, not
the planning estimate of about 5,300, and the CI job pins that count.

**Amended** (2026-09-29, #421): the NetCDF reader decodes the HDF5 szip filter
through `fieldglass_aec::sz` under decision 6's rules. Decision 6 now records
what the reader does when a length-changing filter precedes szip (it decodes,
with the prefix bounded by the chunk's length plus an eighth plus 4 KiB), and
decision 4 gains two rows: a size prefix libhdf5 reads through and the reader
refuses, and, folded in from #794, a 32- or 64-bit szip output shorter than
its stream, which libsz returns scrambled and `sz::decompress` now refuses.
The value oracle is the h5py 3.16 wheel (libhdf5 2.0.0, bundled libaec
1.1.4).

**Amended** (2026-10-03, #813): the bound behind a length-changing filter is
now each earlier filter's worst-case growth chained in write order: an eighth
plus 4 KiB for deflate, zstd and fletcher32 as before, and 33 times for an
szip, and it applies to deflate and zstd as well as to szip's prefix.
Decision 6 records the szip factor and the one case it does not cover.

## Context

GRIB2 template 5.42 and the HDF5 szip filter (id 4) use the same entropy coder,
the Adaptive Entropy Coder of CCSDS 121.0-B. ADR-0001 took
[`rust-aec`](https://crates.io/crates/rust-aec) 0.1.1 for 5.42, pinned exactly,
with a single maintainer, and named vendoring as the way out if it fell short.
Four things now make it fall short:

- **It rejects the block sizes szip uses.** `validate_params`
  (`decoder.rs:1084-1101`) accepts block sizes 8, 16, 32 and 64 only. libaec's
  decoder accepts any even size from 2 to 256 (`decode.c:692-699`), HDF5 writes
  any even size from 2 to 32, and NASA EOS files use 10 and 18. This blocks
  #421. A 2026-09-29 spike found no other blocker: stock rust-aec decoded szip
  chunks byte for byte at block sizes 8, 16 and 32.
- **It allocates per block.** `vec![0u32; n]` for every split block and
  snapshot clones in its streaming path (`decoder.rs:955, 252-283`). This is
  finding 3 in `docs/performance.md`: 529 allocations at `S` and 2,054 at `L`,
  513 and 2,038 of them inside `rust_aec`. It is the one allocation bound 5.42
  breaks.
- **It has side effects inside a decode.** `decode_into` reads the
  `RUST_AEC_TRACE_SAMPLE` environment variable on every call and has
  `eprintln!` paths (`decoder.rs:664-666, 761-768, 826, 880, 932, 1013, 1037`).
- **Bus factor.** One maintainer, and after #421 two format crates would depend
  on it.

rust-aec also differs from libaec in two ways. It masks signed output to `n`
bits instead of sign-extending it (`decoder.rs:1338-1346`), and it ignores
`PAD_RSI` on the non-zero-block path when preprocessing is off
(`decoder.rs:1071-1078`). **These are not reasons for this decision.** On a
5.42 message eccodes can write, neither one ever gives a wrong field: the
SIGNED and PAD_RSI streams decode exactly, where libaec's behaviour would not,
and the one that fails fails in eccodes too. #756 pins this with four fixtures
re-flagged from committed ones by the pinned eccodes 2.34.1
(`grib_set -r -s ccsdsFlags=…`):

| ccsdsFlags | eccodes 2.34.1 decode | fieldglass today | Source field |
| --- | --- | --- | --- |
| 13 (SIGNED + PP + MSB), 12-bit | max 2234.68 | source field, exact | max 314.675 |
| 13, 24-bit | max 16631.1 | source field, exact | max 311.099 |
| 36 (PAD_RSI + MSB) | max 2251.58 | source field, exact | max 314.675 |
| 46 (PAD_RSI + PP + 3BYTE + MSB) | `AEC_DATA_ERROR` | `UnsupportedSection` | max 314.675 |

The cause is on the encoding side. eccodes hands libaec the unsigned,
reference-subtracted value `X` as an n-bit pattern, and libaec's default build
never writes RSI padding (`encode.c:480` is `#ifdef ENABLE_RSI_PADDING`, which
nothing in its CMake defines). So an eccodes-written SIGNED or PAD_RSI stream
is really an unsigned, unpadded stream. rust-aec's masking and its skipped
padding happen to undo that. libaec, and so eccodes' own decode, does not.
Provenance and the full table are in
`crates/fieldglass-grib2/tests/fixtures/NOTICE.md` (#756).

The maintainer decided on #421 (2026-09-29) to own the whole decode path rather
than patch, fork or vendor rust-aec. Where the decoder lives, what it promises
and how it is checked are architecture decisions, so they are settled here
before any code is written.

The libaec, rust-aec, HDF5 and eccodes line references in this record were
checked against libaec v1.1.7 (tag commit `0c4c014`, built twice, default and
with `-DCMAKE_C_FLAGS=-DENABLE_RSI_PADDING`), rust-aec 0.1.1 from the cargo
registry, HDF5 `develop` (`H5Zszip.c`, `H5Zpublic.h`), the eccodes 2.45.0
source tree (`DataCcsdsPacking.cc`), and the pinned eccodes 2.34.1 CLI (the
flag table), during the milestone's planning review.

## Decision

### 1. A new published crate, `fieldglass-aec`

The decoder is its own crate, published on crates.io and versioned in lockstep
with the rest of the workspace, with `=` pins from its consumers (conventions,
*Versioning*). It depends on **no workspace crate** and on nothing outside the
workspace except `thiserror`, which every consumer already links through
`fieldglass-core`. `fieldglass-grib2` swaps `rust-aec` for it (#762) and
`fieldglass-netcdf` gains it (#421). `cargo tree -p` on each is the check: grib2
changes by exactly `-rust-aec +fieldglass-aec`, and netcdf gains exactly
`fieldglass-aec` (ADR-0010 decision 6).

The name is `fieldglass-aec` (Q1), libaec's own vocabulary. The error type is
`AecError` (D3), not `Error`, so it does not collide with `fieldglass::Error`
in `tools/check_architecture_diagrams.py`'s duplicate-name warning.

It does not reuse `fieldglass_core::bits::BitReader`. That reader returns
`FieldglassError`, which a crate with no workspace dependencies cannot name, and
it does its offset arithmetic and bounds check on every call. The AEC hot path
reads unary codes and short fields back to back and wants a `u64` accumulator
refilled eight bytes at a time. `fieldglass-zarr`'s `blosclz.rs` and `lz4.rs`
are the precedent for a codec written here with its own reader.

### 2. Scope: decode only, a whole slice at a time

- **Decode only.** No encoder.
- **Whole slice.** Every caller already holds the whole §7 payload or the whole
  HDF5 chunk (ADR-0005: hosts hand bytes). There is no streaming state machine,
  which removes rust-aec's snapshot and restore code with it.
- **No RSI random access.** Neither GRIB2 nor HDF5 stores RSI bit offsets, so
  finding one is a full decode. `aec_decode_range` has no counterpart. The API
  takes a sample count, so "the first N samples" stays possible.
- **No allocation.** `decode`, `decode_to_bytes` and `sz::decompress` write into
  caller-owned output. Block size is at most 256, so the block buffer is a
  `[u32; 256]` on the stack. Parameters only bound counters. The caller owns the
  size cap: GRIB2 bounds the count by `MAX_FIELD_POINTS`, and HDF5 bounds the
  chunk by `MAX_DECOMPRESSED_CHUNK` before allocating. A test pins zero
  allocations per decode (#759), and the perf gate pins one allocation per 5.42
  codec call at both sizes (#762).
- `#![forbid(unsafe_code)]`, no panics on any input, and fuzzed before either
  reader switches to it (#760).

### 3. Correct per CCSDS 121.0-B-3, with libaec 1.1.7 as the oracle

The standard, CCSDS 121.0-B-3, defines what a stream means. libaec 1.1.7 is
the reference oracle: `fieldglass-aec` matches it wherever libaec is correct,
and where libaec disagrees with the standard or accepts a stream no valid
encoder writes, it follows the standard. Matching libaec is how correctness is
checked, not the goal (maintainer, on #759, 2026-09-29). Each divergence is
listed in decision 4 with the clause of the standard and a case that
reproduces it.

**Parameter acceptance** is exactly the complement of `aec_decode_init`'s
`AEC_CONF_ERROR` in libaec 1.1.7:

- bits per sample 1–32;
- even block sizes 2–256, with 0 rejected (`decode.c:692-699`);
- RSI 1–4096;
- `RESTRICTED` accepted for 1–4 bits, rejected for 5–8, and ignored above 8
  (`decode.c:740-754`);
- `NOT_ENFORCE` has no effect on decoding.

**Output.** `decode_to_bytes` writes libaec's output layout, and equals
libaec's output byte for byte on every stream libaec decodes correctly: 1, 2, 3
or 4 bytes per sample, MSB or LSB. SIGNED samples are sign-extended only with
PREPROCESS, where the postprocessor sign-extends the reference sample and so
every sample after it. Without PREPROCESS libaec writes the raw n-bit pattern,
zero-extended (the `FLUSH` macro, `decode.c:55-127`), and so does this crate;
the `signed_nopp_*` corpus cases pin it. The standard defines samples, not a
byte layout, so this is libaec's convention, kept for the oracle.

### 4. Known divergences from libaec

Each row is a stream on which libaec's decoder and the standard disagree, or
on which libaec returns success with output nobody asked for (Q4). Each has
the clause of CCSDS 121.0-B-3 that decides it, or says plainly that it is an
API choice with no clause, and a case that reproduces it.

| Case | libaec 1.1.7 | `fieldglass-aec` | Evidence |
| --- | --- | --- | --- |
| A second-extension pair sum above 12 | `AEC_DATA_ERROR` part-way: its table stops at codeword 90 (`create_se_table`, `decode.c:674-685`; `SE_TABLE_SIZE`, `decode.h:53`), though its own encoder writes such streams (`assess_se_option`, `encode.c:396-416`) | Decoded as the standard defines | §3.4.2 extends the codewords "in the obvious manner" with no bound. Corpus case `se_pair_sum_over_12_b03_j256_r3_pp`: libaec gives 2,047 of 2,050 samples, this crate gives the encoder's input (`source_sha256`). Maintainer decision on #759. |
| A second-extension pair beside a reference sample whose first value is not 0 | Ignores the first value and keeps the second (`decode.c:570-575, 604-607`) | `AecError` | §3.4.1 and §5.2.6: with a reference sample, a 0 goes in front of the J - 1 mapped errors, so the first pair is (0, δ2). libaec's encoder writes that 0 (`encode.c:236, 272`), so no corpus stream has another value; a unit test in `tests/decode.rs` does. |
| A zero-block fundamental sequence longer than 63 zeros | Accepted as that many blocks, if the run fits the RSI (`decode.c:527-541`) | `AecError` | Table 3-2 ends at "63 … (63 0s and a 1)", one whole segment. No libaec-encoded corpus stream has one; unit tests in `src/decode.rs` and `tests/decode.rs` do. |
| Truncated input | `AEC_OK` with short output (`decode.c:833-849`) | `AecError`, with the count of samples decoded. Never zero-filled. | No clause: an API choice (Q4), so a short input is never mistaken for a short field. Corpus cases `truncated_half_b16` (libaec gives 4,098 of 8,192 bytes) and `truncated_one_byte_b16`. |
| A value of 2^n or more before postprocessing | Wraps silently | `AecError` | §4.4 maps every prediction error into `0..2^n`, so no valid encoder emits one. The corpus test asserts no libaec-encoded case hits it; unit tests build each kind (split high part, split with k above n, second extension). |
| szip stream that runs out before the output is full | `SZ_OK` either way. Unpadded scanlines: a smaller `destLen` (`sz_compat.c:302-303`). Padded scanlines: the full `destLen` (`total_out = scanlines·pps·pixel_size`, `sz_compat.c:295`), with the undecoded tail copied from an uninitialised `malloc` buffer | `AecError::Truncated`. The output must fill exactly. | No clause: szip framing is libsz's contract, not CCSDS 121.0. An API choice (Q4): the caller knows the exact length (HDF5 stores it in the chunk's size prefix), so a short result is lost data, and the padded case is garbage reported as success. HDF5's only check is `assert(size_out == nalloc)` (`H5Zszip.c:300`), compiled out of release builds, so libhdf5 passes both through. All 39 unpadded and all 39 padded corpus streams cut in half show the two behaviours (`tests/fixtures/NOTICE.md`). #761. |
| szip output of 32- or 64-bit pixels that is not a whole number of pixels | `SZ_OK`. It deinterleaves with planes of `destLen / w` bytes rounded down (`sz_compat.c:84-93, 305-306`), so bytes land in the wrong places and the last `destLen mod w` are never written | `AecError::OutputLength`, before anything is decoded | No clause: an API choice. At such a length libsz's result is not the data; HDF5 never asks for one. Corpus stream `sz_b32_plane_edge_mid_scanline` decoded into 599 bytes instead of 600: libsz 1.1.7 returns `SZ_OK` with 165 bytes misplaced and the last 3 unwritten (168 differ from the 600-byte decode). `tests/sz_corpus.rs` pins the error. #761. |
| szip bad code after the last requested pixel | With padded scanlines it decodes every scanline whole (`sz_compat.c:264-267`), including the blocks after the last pixel of a partial last scanline, and fails on a bad code there: a zero-block run past the RSI (`decode.c:529-541`) or a second-extension codeword past its table (`decode.c:567, 600`) | Returns `Ok` with the pixels asked for. Below 32 bits it stops at the last pixel and never reads those blocks; at 32 and 64 bits it reads them only to find the end of the stream (next row), and decodes again up to the last pixel if they are bad | Not the fill of §5.3.1: these are real CDSs, but they code samples the caller did not ask for (pads, and pixels past the output), and the output is identical wherever libsz succeeds. `a_bad_code_after_the_last_pixel_is_never_read` in `tests/sz_corpus.rs`: libsz 1.1.7 returns `AEC_DATA_ERROR` for a 6-byte output, `sz::decompress` returns the 6 pixels. The `sz` fuzz target accepts exactly this difference: `Ok` here where its whole-scanline reference fails at or after the last pixel's block. #761. |
| szip output of 32- or 64-bit pixels that is a whole number of pixels but shorter than its stream | `SZ_OK`. It decodes `destLen` bytes and deinterleaves them with planes of `destLen / w` bytes (`sz_compat.c:84-93`), so every byte after the first plane lands in the wrong place | `AecError::TrailingInput`. For byte planes it decodes on to the end of the stream the output's length implies, the end of the last scanline (libsz's `add_padding` fills every scanline, the last one included, to whole blocks), hands nothing past the last pixel to the output, and refuses a whole byte of input left after that. A shortfall inside that last scanline ends the stream in the same place and cannot be seen | No clause: an API choice, as in the row above it, whose one-byte-short case is refused. The output is laid out by its own length, so a shorter one is not a prefix of the data. Measured in #794: every 32- and 64-bit corpus case decoded into half its length returns `SZ_OK`-equivalent output with 73% to 87% of the bytes wrong (`sz_b32_ppb16_exact`: 286 of 384; `sz_b64_ppb32_padded`: 1,677 of 1,928). `a_byte_plane_output_shorter_than_its_stream_is_an_error` in `tests/sz_corpus.rs` refuses every byte-plane corpus case at half length when that half ends in an earlier scanline (31 of the 32; the other, three pixels in one scanline, is the undetectable limit and is pinned as such), still decodes each at full length, and refuses each with one byte appended. The six `sz_b*_short_last_line_*` cases pin that libsz pads the last scanline whole even when the scanline is a multiple of the block. The HDF5 reader's reason to care: a chunk whose stream codes more pixels than its size prefix says passes every length rule in `hdf5/filter.rs`, and is now refused (`a_chunk_whose_stream_codes_more_pixels_is_refused` in `crates/fieldglass-netcdf/tests/hdf5_szip.rs`). Coordinator decision on #794 (option a). #421. |
| HDF5 szip chunk whose size prefix exceeds its data (libhdf5, not libaec) | libhdf5 2.0.0 allocates the prefix, and libsz returns `SZ_OK` with the shorter length it decoded; `H5Zszip.c` checks the two only in a debug-build `assert`, so a release build reads the chunk | `FieldglassError::Parse` before allocating: the prefix must be at most `MAX_DECOMPRESSED_CHUNK` and, when only shuffle precedes szip, equal the chunk's length | No clause: the prefix is `H5Zszip.c`'s framing, written as the chunk's length, so a prefix that disagrees is a corrupt chunk (Q4). `i2_ppb16` in `hdf5_szip.h5` with its first prefix set to 256 MiB + 1 reads back correctly through h5py 3.16 after a 256 MiB allocation; the reader refuses it (`a_chunk_whose_size_prefix_is_wrong_is_refused` in `crates/fieldglass-netcdf/tests/hdf5_szip.rs`). Prefixes of 511 and 513 on that 512-byte chunk fail in libhdf5 too. #421. |
| Trailing fill after the last sample | Can return `AEC_DATA_ERROR` *after* producing every sample, when the fill parses as a zero block that overruns the RSI (`decode.c:529-541` is checked before `avail_out`) | Stops at the requested count and never reads the fill, so the same bytes with `Ok` | §5.3.1: "Fill bits of zero value may be needed to force the packet to end on a byte boundary", so bits after the last CDS are fill, not codes. Corpus case `trailing_zero_block_overrun_b08`. |

One leniency is deliberate and matches libaec: a zero-block run other than ROS
may cross a 64-block segment boundary. B-3 lists "specifies the size of a
segment as 64 blocks" among its changes affecting backward compatibility, so an
encoder written to the earlier issue can place runs that way, and refusing them
would reject streams that decode unambiguously. `zero_run_blocks` in
`src/decode.rs` records the same reasoning, and a unit test pins it.

The planning spike's example of the last row (3 bits, block 256, RSI 3, 2,050
samples) was really the first: `total_out` reports the full room after an
error (`decode.c:824` runs before the return at `:830`), which made a rejection
look like complete output. The behaviour in the last row is real, and is what
`trailing_zero_block_overrun_b08` pins.

GRIB2 already errors on truncation today, where eccodes zero-fills, so the
truncation row changes nothing a user sees.

### 5. GRIB2 rules, which are not libaec's

eccodes passes `ccsdsFlags` straight to libaec, clears `3BYTE`, forces native
byte order, and reads every sample as unsigned (`DataCcsdsPacking.cc:60-68,
450-529`). The GRIB2 reader also takes the flags from the message, and byte order stops
mattering once samples reach it as integers. It replaces the unsigned read with
two rules that match what eccodes' encoder wrote:

- **SIGNED:** `X = sample & mask(n)` (Q5). This recovers the n-bit pattern the
  encoder saw, and is what fieldglass decodes today. It is not eccodes parity:
  eccodes' own encode and decode disagree (the table above).
- **PAD_RSI:** the reader clears the flag before decoding (D1). This keeps flag
  36 files correct, as today, and turns flag 46 files from an error into an
  exact decode. It knowingly diverges from eccodes, whose output for these flags
  is wrong. The alternative, libaec semantics, would turn flag 36 files into the
  garbage in the table.

These live in `fieldglass-grib2`, in the flags it passes and the sink it
decodes into, not in the codec crate. The codec
decodes what the standard says; the consumer decides what a GRIB2 message meant. #756's fixtures
pin both rules, and #762 must keep them green.

### 6. szip: libsz in the codec crate, HDF5 in the reader

libaec splits itself the same way: `libaec` is the coder and `libsz`
(`sz_compat.c`) is the SZIP-compatible layer on top. Neither the scanline
padding nor the byte-plane deinterleave is HDF5-specific, and HDF4 also uses
SZIP (#248). So:

- **`fieldglass_aec::sz`** carries `SZ_BufftoBuffDecompress` semantics (#761):
  `rsi = ceil(pps / ppb)`; scanline padding when `pps % ppb ≠ 0`, dropped after
  postprocessing; an 8-bit stream deinterleaved into byte planes for 32- and
  64-bit pixels; 1, 2 or 4 bytes per sample, never 3; only the `NN` and `MSB`
  option bits change decoding (`sz_compat.c:47-71, 84-93, 119-130, 222-313`).
  Validation mirrors `sz_compat.c:229-235`. `SzParams` is in `SZ_com_t` order
  (mask, bpp, ppb, pps), and its docs say that is not HDF5's order. libsz builds
  a padded copy of up to `ppb` times the output (32× at pps = 1) plus a
  deinterleave copy; `sz` maps indices through the sink and builds neither.
- **`fieldglass-netcdf`** keeps the HDF5 framing (#421): `cd_values` in
  `(mask, ppb, bpp, pps)` order (`H5Zpublic.h`), the 4-byte little-endian
  uncompressed-size prefix (`H5Zszip.c:273-300`), and the length rules. The
  prefix must be at most `MAX_DECOMPRESSED_CHUNK` and, when every filter before
  szip preserves length, equal the expected chunk length. The chunk length must
  be a multiple of the szip pixel width (and of `bpp/8` for 32 and 64). The
  element width need not equal the pixel width (D2): HDF5 writes a precision-16
  `<i4` with bpp 16, and libsz decodes it exactly.
- **A length-changing filter before szip** (deflate, zstd or fletcher32 earlier
  in write order, and applied to the chunk) is decoded (#421). libhdf5 writes
  `[deflate, szip]` and reads it back, so refusing it would reject valid
  files, and the prefix is then the earlier filter's output length, which
  nothing outside the stream records. So the prefix is bounded rather than
  matched: by the chunk's length grown by each earlier filter's worst case,
  an eighth plus 4 KiB for deflate, zstd and fletcher32 (deflate's and zstd's
  worst-case growth and fletcher32's four bytes), and 33 times for an szip,
  which keeps a tiny chunk from committing a 256 MiB buffer. deflate and zstd
  are bounded the same way when something before them changes the length,
  and by the chunk's own length otherwise (#813). The reader then requires
  the chunk to come back exactly its own length, whatever the pipeline.
  `hdf5_szip_hand.h5`'s `deflate_szip` and `hdf5_szip_growth.h5` pin it end
  to end.
- **szip's factor of 33** bounds any encoder that never codes a block longer
  than its uncompressed option, an ID of 3 to 5 bits plus `J·n` bits, at most
  32 pixels per block (`H5Pset_szip`) and each scanline padded to whole
  blocks: a one-pixel scanline of 8-bit pixels would code about 32.4 times
  its input. libsz itself stays under it: it pads short scanlines with zeros
  or the last pixel rather than coding them raw, and a sweep of bundled
  libaec 1.1.4 over 8- to 64-bit pixels, 8 to 32 pixels per block and one
  pixel per scanline found 26.2 times at most. `hdf5_szip_growth.h5` is that
  worst stream, behind which deflate decodes. `sz` accepts blocks of up to 256 pixels for other
  writers; a stream from one with blocks over 32 and scanlines shorter than a
  block can grow past 33 times, and so can one from an encoder that codes a
  block in an option longer than the uncompressed one (CCSDS 121.0-B leaves
  the choice to the encoder), and a codec after either would be refused.
  No known writer produces one, and the bound is what keeps a file from
  inflating a bomb behind an szip it put there itself.
- **A stream that codes more pixels than the chunk** is refused by the codec
  for 32- and 64-bit pixels (`AecError::TrailingInput`, decision 4, #794).
  With no filter or only shuffle before szip and a correct prefix, it passes
  every rule above, and libhdf5 reads it back scrambled
  (`hdf5_szip_long_stream.h5`).

### 7. The sink API

```rust
pub trait Sink {
    fn samples(&mut self, block: &[u32]);
    fn repeat(&mut self, value: u32, count: usize);
}
pub fn decode(input: &[u8], params: &Params, count: usize, sink: &mut dyn Sink)
    -> Result<(), AecError>;
pub fn decode_to_bytes(input: &[u8], params: &Params, out: &mut [u8])
    -> Result<(), AecError>;
```

These are the names #759 shipped. The kernel hands each decoded
block, or each zero-block run, to a sink, one `u32` per sample as libaec's
postprocessor leaves it: sign-extended for SIGNED data with PREPROCESS, and the
raw n-bit pattern without it (decision 3). The GRIB2 reader's
sink scales integers straight into its `Vec<f64>`, which drops today's byte
buffer and re-parse. The szip sink drops pad samples and scatters byte planes by index.
`decode_to_bytes` is one more sink, and it is what the oracle compares.

The sink is `&mut dyn Sink`, not a generic, so the wasm bundle carries one copy
of the kernel instead of one per sink. The call is per block (at least 2
samples, usually 8 to 64) or per zero run, so its cost is amortised. If
profiling ever says otherwise, the kernel can go generic behind the same
signature.

### 8. The oracle is libaec itself

- **A pinned libaec source build.** `tools/build_aec_fixtures.py` (#758)
  downloads the libaec v1.1.7 tag tarball, checks its SHA-256, builds it with
  cmake twice (default, and `-DCMAKE_C_FLAGS=-DENABLE_RSI_PADDING`, because the
  default encoder cannot write PAD_RSI; libaec's CMake has no option for it),
  and drives it through `ctypes`. cmake and a C compiler
  are needed only to regenerate; the tests need nothing, as with eccodes.
- **Streams from libaec's own encoder.** Inputs are ported from libaec's
  `tests/check_code_options.c`, which forces every coding option (zero block,
  split for every k, uncompressed, FS, second extension) at every sample width,
  both byte orders, with preprocessing and with SIGNED, and asserts the option
  the encoder chose. A hand-built stream is allowed only for a case the encoder
  cannot be forced into, and each is listed with its reason in `NOTICE.md`.
  libaec's **decoder** is always the value oracle. A self-round-trip encoder is
  how `oxiarc-szip` passed its own tests and failed eccodes (ADR-0001).
- **A committed digest corpus.** About 300 streams, under 1 MB, and a manifest
  of parameters, libaec's status, its output length and the SHA-256 of its
  output. The tests assert the case count and that every stream is referenced,
  so an empty or partial corpus fails.
- **Regenerated and diffed in CI** (#763). A path-filtered job rebuilds libaec,
  regenerates the committed corpus and fails on any diff, so a hand-edited
  manifest is caught. It also runs the full matrix (4,819 cases, a count the
  job pins with `AEC_ORACLE_EXPECT_AEC=4819`) and the 66 CCSDS 121.0-B-2
  sample files from the tarball, neither of which is committed. The job is
  `.github/workflows/aec-oracle.yml`, and `tools/aec_oracle.py` runs it
  locally.
- **No differential testing against rust-aec.** It diverges from libaec in two
  places (see Context), and on random bytes both decoders mostly error, so the
  comparison says nothing. Fuzzing covers panics, hangs and exact output length.

The four eccodes 5.42 fixtures stay unchanged as the GRIB2 backstop, and h5py
read-back is the szip value oracle (#421).

### 9. Licensing

The decoder is written from CCSDS 121.0-B-3, with libaec as the behavioural
oracle, and carries a `Reference:` line like `blosclz.rs`. Nothing is taken from
rust-aec (MIT). It still carries **libaec's BSD-2-Clause notice** (Q3), in the
crate and in `crates/fieldglass-wasm/npm/NOTICE`: #758 ports test inputs from
`check_code_options.c`, and two routines have to follow libaec's formulation for
the output to be exact (the postprocessor, and libsz's index mapping). The
CCSDS sample data is distributed with libaec under a permission given to libaec
(its `THANKS` file), so it is read from the tarball in CI and never committed.

### 10. First publish is by hand

crates.io Trusted Publishing can only be configured for a crate that already
exists, and `fieldglass-grib2` cannot be published until its dependency is on
the index. So at the first release that contains #762, the maintainer runs
`cargo publish -p fieldglass-aec` with an API token, configures Trusted
Publishing for it, and re-runs the release workflow (Q2). `release.yml`'s
`published()` check skips what already went out. #762 adds the crate to the
publish loop **first**, so a Trusted Publishing failure stops the job before
anything else is published. Nothing is added to the loop before then, so a
release cut earlier cannot publish a half-built crate.

### 11. Answers to the milestone questions

All recorded on the milestone by the maintainer on 2026-09-29.

| # | Question | Answer | Where |
| --- | --- | --- | --- |
| Q1 | Crate name | `fieldglass-aec` | decision 1 |
| Q2 | First publish | By hand at the first release containing #762, then Trusted Publishing | decision 10 |
| Q3 | libaec attribution | Carry libaec's BSD-2-Clause notice | decision 9 |
| Q4 | Strictness | Stricter than libaec: truncated input and over-width values are errors | decision 4 |
| Q5 | Signed GRIB2 5.42 | `X = sample & mask(n)`, not eccodes parity | decision 5 |
| Q6 | Real NASA granules | A manual `tools/fetch_samples.sh` entry; synthetic h5py fixtures are the gate | #421 |
| Q7 | Exact-length bounds for deflate and zstd | Out of this milestone; a separate issue later, for the maintainer to file | — |
| D1 | PAD_RSI in GRIB2 | Clear the flag before decoding | decision 5 |
| D2 | szip pixel width ≠ element width | Accept when the chunk length is a multiple of the pixel width | decision 6 |
| D3 | Error type name | `AecError` | decision 1 |

### 12. What was rejected

- **A module in `core`, behind a feature.** It adds no dependency, but it puts
  a codec on `core`'s gated surface, and `core` is traits, geometry and
  projection. `tools/check_parsing_surface.py` would have to learn about a
  codec, and a consumer of the codec alone would take `core` with it.
- **A module in `fieldglass-grib2`, with `fieldglass-netcdf` depending on it.**
  Breaks "no format crate depends on another" (`01-crates.md`). A NetCDF-only
  consumer would link a GRIB2 reader, which ADR-0010 decision 6 calls a defect.
- **A copy in each format crate.** Breaks "one fix pattern covers every
  instance". The second copy is the hoist the conventions warn about.
- **Patch rust-aec upstream, fork it, or vendor it.** Upstreaming leaves the
  bus factor where it is. A fork or a vendored copy is ownership anyway, of a
  streaming design built around per-block allocation that would then have to be
  taken apart. The maintainer chose to write the decoder on #421.
- **`libaec-sys`.** A C build on six targets, which ADR-0001 exists to avoid.
- **Match libaec in the GRIB2 reader too.** It would turn today's correct
  decodes of flags 13 and 36 into eccodes' wrong values (decision 5).
- **A `no_std` crate.** Cheap, but no consumer has asked, and it adds a cfg
  surface.

## Consequences

- #421 is unblocked. szip decodes through the same kernel as 5.42, at every
  block size HDF5 writes, with no header-chosen allocation multiplier.
- Finding 3 in `docs/performance.md` resolves: one allocation per 5.42 codec
  call at both sizes, and codec instructions at `L` at most half of rust-aec's
  23,764,511 (#762's gate).
- `rust-aec` leaves every manifest and lockfile in #762. ADR-0001's 5.42 row and
  section are updated there, and its dated amendment points here.
- The workspace has a fifth published library crate and one more `=` pin in the
  version bump, plus a fuzz lockfile (#760).
- The project now owns a codec. A libaec release that changes decode behaviour
  is noticed only when someone bumps the pin in `tools/build_aec_fixtures.py`
  and the CI job diffs the corpus.
- GRIB2 5.42 knowingly diverges from eccodes for SIGNED and PAD_RSI messages.
  Both divergences are where eccodes is wrong, and #756's fixtures record it.

## When to revisit

- **A producer writes real RSI padding or real signed samples into 5.42.** A
  libaec build with `ENABLE_RSI_PADDING`, or an encoder that sign-extends before
  encoding, would make decision 5's rules wrong for its files. The rules would
  then need a signal in the message to tell the two apart, and a fixture from
  that producer.
- **A container stores RSI offsets.** Then random access (decision 2) is cheap
  and worth adding.
- **The `dyn` sink shows up in a profile.** Make the kernel generic behind the
  same signature (decision 7).
- **libaec 1.1.7 is superseded with a decode change.** Bump the pinned tarball,
  regenerate, and read the corpus diff.

## References

- ADR-0001 (rust-aec for 5.42), ADR-0005 (hosts hand bytes), ADR-0010 decision 6
  (a format crate stays lightweight).
- CCSDS 121.0-B-3, *Lossless Data Compression* (2020).
- libaec v1.1.7: <https://github.com/MathisRosenhauer/libaec> (tag commit `0c4c014`).
- rust-aec 0.1.1: <https://crates.io/crates/rust-aec>
- #421 (szip), #756 (the SIGNED and PAD_RSI pins), #248 (HDF4 SZIP, parked).
