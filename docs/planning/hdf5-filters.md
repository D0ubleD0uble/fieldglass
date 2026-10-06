# HDF5 filter coverage and cost

*Verified against the code 2026-09-29.*

`crates/fieldglass-netcdf/src/hdf5/filter.rs` decodes five filters: deflate
(id 1), shuffle (id 2), fletcher32 (id 3, #412), szip (id 4, #421), and zstd
(id 32015, #413).
Any other filter in a pipeline fails the whole file. Chunk indexing is already ahead of the field (all five v4 index
types plus the v1 B-tree), so filters are the gap that blocks real files.

| ID | Filter | Why it matters | Cost |
|---|---|---|---|
| 3 | fletcher32 | ~~A checksum, not compression. Its presence fails files whose compression we handle fine.~~ **Done (#412).** | Not the no-op it looks like: it *appends* 4 bytes, so reading must strip them, and libhdf5 accepts two checksum byte orders (a pre-1.6.3 bug). See below. |
| 32015 | zstd | ~~netcdf-c ≥ 4.9; DKRZ-recommended for climate archives.~~ **Done (#413)** via `ruzstd` 0.9 (MIT, pure Rust, one transitive dep). | Cross-compile verified to `x86_64-pc-windows-msvc` and `wasm32` with no C toolchain, which is ADR-0001's actual deciding criterion. |
| 307 | bzip2 | Rare. | Pure-Rust decoder (`bzip2-rs`). Small. |
| 4 | szip | ~~Unblocks HDF5 files that use szip.~~ **Done (#421)** via `fieldglass-aec`, the project's own CCSDS 121.0 decoder ([ADR-0012](../decisions/0012-own-the-aec-decoder.md)). | Same entropy coder as GRIB2 5.42 (#762); libsz framing in `fieldglass_aec::sz` (#761), HDF5 framing in the reader (#421). See below. |

Blosc/LZ4: rare in NetCDF, defer. This set would exceed default netcdf-c
installs, which frequently lack working szip/zstd plugins at runtime.

## fletcher32: what it turned out to be (#412)

Worth recording, because the issue's own framing was wrong and the same trap
applies to any future "checksum, not compression" filter:

- It is **not** size-neutral. libhdf5 appends a four-byte checksum to the
  stored chunk (`FLETCHER_LEN`), so reading shortens the chunk by four. A
  passthrough leaves the pipeline four bytes long.
- Being wrong here is quiet. With a compressor in the pipeline the extra bytes
  are absorbed by the zlib decoder and the values come out right anyway; it is
  the *uncompressed* `shuffle + fletcher32` case — which libhdf5 does write —
  where 132 bytes divides evenly by a 4-byte element and unshuffles into one
  element too many with no error.
- The checksum covers the **filtered** bytes, not the values: fletcher32 is
  written last, so it reverses first.
- libhdf5 accepts either of two stored values, the computed checksum or the
  same with the bytes of each 16-bit half swapped. Releases before 1.6.3
  computed it inconsistently across endianness, and the fix kept those files
  readable (`H5Zfletcher32.c`, "the reversed checksum"). A reader that accepts
  only the correct one rejects valid old files.

## Decompression is bounded (#413)

Adding a second compressor made the shape of the risk obvious enough to fix for
both: a compressed chunk is attacker-controlled and its expansion ratio is
unbounded, so a few kilobytes on disk could name an arbitrarily large
allocation. `inflate` had used the unbounded `decompress_to_vec_zlib` since the
deflate filter shipped.

Both codecs then stopped at `MAX_DECOMPRESSED_CHUNK` (256 MiB) — far past any
real HDF5 chunk, libhdf5's own chunk cache defaults to 1 MiB — and the ceiling
is exercised by unit tests through `_bounded` helpers that take the limit as an
argument, so the guarantee is tested without the suite paying to build a
256 MiB stream.

Since #813 each codec stops at the chunk's own length when every filter before
it keeps the length (only shuffle does). Behind filters that change it, the
bound is the chunk's length grown by each one's worst case in turn: an eighth
plus 4 KiB for fletcher32, deflate and zstd (the margin szip's prefix already
had), and 33 times for szip, whose uncompressed blocks and scanline padding
can make one-pixel scanlines about 32 times their input. szip's factor is
large, but bounding it by the 256 MiB ceiling instead, as the first draft of
#813 did, let a file sidestep the bound by putting an szip in front. 256 MiB
is only the outer ceiling now: a 12-byte chunk had been able to inflate to
all of it before the caller's length check refused the result, and on wasm
that is a real allocation.

zstd needs a **second, separate** ceiling, and this is the part worth carrying
to the next codec. A zstd frame header declares its own window size, and the
decoder sizes that buffer during frame init — before any output exists, so an
output bound cannot see it. The two are independent: a *small* window producing
enormous output is precisely what a bomb is. `MAX_ZSTD_WINDOW` (64 MiB) bounds
it, deliberately tighter than `ruzstd`'s own 100 MiB default, because a
guarantee inherited from an upstream default is one a version bump can widen
without anyone noticing.

Ask of any future codec here: *what does it allocate from a header, before it
produces a byte of output?* That allocation needs its own bound, and a test that
distinguishes your bound from the library's — one that merely proves "an absurd
value is rejected" will pass against the library's default and tell you nothing.

## Why szip is a project, not a quick win

The entropy coder (CCSDS 121.0 extended-Rice) is shared with GRIB2 5.42. The
external decoder GRIB2 used first rejected the block sizes HDF5 writes, so the
project now owns the coder, and szip decodes through `fieldglass-aec` (#421,
[ADR-0012](../decisions/0012-own-the-aec-decoder.md)), the crate GRIB2 5.42
already decodes with (#762). It accepts every even block size from 2 to 256
(HDF5's `pixels_per_block` is any even value 2–32, so 10 and 18 are as valid
as CCSDS's 8, 16 and 32) and every RSI from 1 to 4096. HDF5 szip RSI is
`ceil(pixels_per_scanline / pixels_per_block)`, typically 1–128 and often 1,
which libaec's own test inputs in the crate's corpus cover.

HDF5 szip framing also differs from GRIB2 §7: a 4-byte little-endian
uncompressed-size prefix per chunk, scanline padding when
`pixels_per_scanline % pixels_per_block != 0`, and byte-interleaving for
32/64-bit samples (libaec decodes those as 8-bit streams and deinterleaves).
That is libaec's `sz_compat.c`, which `fieldglass_aec::sz` carries (#761); the
HDF5 framing stays in the NetCDF reader. Its oracle is the h5py wheel, whose
libhdf5 writes szip with a bundled libaec (`tools/build_hdf5_fixtures.py`,
`build_szip`).

## What szip turned out to be (#421)

Once the coder was ours, the HDF5 side was small. What is worth carrying
forward:

- **szip in HDF4 is not szip in HDF5.** The plan for #421 said it would open
  AIRS and MODIS. The standard AIRS (v7) and MODIS (Collection 6.1) products
  are HDF-EOS2, which is HDF4, so they still need HDF4 reading (#248); the
  release notes had to be corrected before they shipped (#816). Name a product
  as opening only after a real HDF5 granule of it has been read.

- **The `cd_values` order is a trap.** HDF5 stores `(mask, pixels per block,
  bits per pixel, pixels per scanline)` (`H5Zpublic.h`); libsz's `SZ_com_t`,
  and so `fieldglass_aec::sz::SzParams`, is `(mask, bits per pixel, pixels per
  block, pixels per scanline)`. Passing them through positionally swaps the
  middle two, and most swaps are still valid parameters, so the mistake decodes
  to garbage instead of failing. Fixtures where the two differ (block 10 at 16
  bits, block 18 at 32) are what catch it; a block of 16 at 16 bits would not.
- **The 32× padding copy is gone.** libsz decodes into a buffer padded to whole
  blocks per scanline, up to 32 times the output at one pixel per scanline and
  32 per block, then copies it again to reorder byte planes. `sz::decompress`
  skips pads as they arrive and writes each byte to its place, so the only
  allocation is the chunk itself.
- **The size prefix is checked before it is allocated.** Every chunk starts
  with a 4-byte little-endian uncompressed size. It must be at most
  `MAX_DECOMPRESSED_CHUNK`; when every filter before szip keeps the length
  (only shuffle does), it must equal the chunk's length; and it must be a whole
  number of szip pixels. The pixel is set by bits per pixel, not by the element:
  libhdf5 codes a 16-bit-precision `int32` at 16 bits per pixel, so the pixel is
  half the element (ADR-0012 decision D2).
- **A length-changing filter before szip is decoded, not refused.** libhdf5
  writes `[deflate, szip]` and reads it back, so refusing it would reject valid
  files. The prefix is then deflate's output length, which nothing outside the
  stream records, so it is bounded, not matched: at most the chunk's length
  plus an eighth plus 4 KiB, which covers deflate's, zstd's and fletcher32's
  growth and stops a tiny chunk from committing a 256 MiB buffer, and 33
  times the chunk plus 4 KiB behind an szip (#813). The chunk
  must then come back exactly its own length; before #421 a longer result was
  silently cut.
- **A stream longer than its chunk is refused, for 32- and 64-bit pixels.**
  Those are coded as byte planes laid out by the output's length, so a chunk
  whose stream codes more pixels than it holds decodes to bytes in the wrong
  places. With a correct prefix and nothing but shuffle before szip, that
  passes every length rule, and libhdf5 returns the scrambled values.
  `fieldglass_aec::sz` now reads to the end of the stream the length implies
  and refuses a whole byte left over (#794, `hdf5_szip_long_stream.h5`).
- **Stricter than libhdf5, on purpose.** libhdf5 checks the prefix only in a
  debug-build `assert` (`H5Zszip.c`), and libsz returns success with short
  output when the stream runs out. So a chunk whose prefix is larger than its
  data reads without complaint in a release libhdf5: measured on
  `hdf5_szip.h5`, a prefix of 256 MiB + 1 on a 512-byte chunk reads back
  correctly through h5py 3.16, after allocating 256 MiB. Here the same chunk is
  refused, before allocating. A prefix one byte either side of the chunk's
  length fails in libhdf5 too.
- **szip is an optional filter.** When a chunk does not shrink, libhdf5 stores
  it as it is and sets the filter's bit in the chunk's filter mask. Random bytes
  do this every time, so the fixture has one.
- **libhdf5 never writes a scanline shorter than a block.** Its `set_local`
  takes pixels per scanline from the chunk's fastest dimension, capped at 128
  blocks, or from the whole chunk when that dimension is shorter than a block.
  Other writers can, so the fixture patches one in and lets libhdf5 read it back.

## Other NetCDF / HDF5 gaps

- **String/char data display.** Classic `char` variables and HDF5 string
  datasets refuse value decode; station names and time labels are table
  stakes for ocean and observation files.
- **Paged Fixed/Extensible Array data blocks** (today a clean error).
- **HDF5 2.0 awareness.** Detect the new `H5T_COMPLEX` class and report it
  cleanly; files using it are unreadable by all older readers including
  netcdf-c < 4.10.

Prior art worth reading: pyfive (pure-Python HDF5 reader; the best map of the
sufficient subset). There is no battle-tested pure-Rust HDF5 reader.
