# Test fixture provenance

Every store in this directory was written by **zarr-python 3.3.0 with numcodecs
0.16.5**, by `tools/build_zarr_fixtures.py` in the repository root. None of the
chunk bytes were produced by this repository: they come from the reference
implementation, so a decode that agrees with `oracle.json` is agreeing with
zarr-python rather than with a second copy of our own arithmetic.

Regenerate with:

```sh
python3 tools/build_zarr_fixtures.py
```

## The source array

All but three stores hold the same 4×6 `float32` ramp in 2×3 chunks:

```python
(np.arange(24, dtype="<f4") * 0.5).reshape(4, 6)
```

It is deliberately tiny. What is under test is the framing — the codec chain,
the shuffle, the block table — not the volume, and a ramp makes a wrong answer
obvious: any transposition, byte-order slip or off-by-one block boundary breaks
the arithmetic progression, which the builder asserts before it writes.

Some stores depart from it on purpose:

| Store | Array | Why |
| --- | --- | --- |
| `blosc_multiblock` | 400×500 ramp, step 0.25 | Large enough that blosc splits the chunk into several blocks, so the block-offset table, the per-block split rule and the short final block are all exercised. |
| `blosc_bitshuffle_ragged` | 10×15 ramp, step 3.0 | An element count that is not a multiple of eight — the case blosc leaves un-shuffled at the tail. |
| `int16` | 4×6 `<i2`, values −12..11 | A non-float dtype, including negatives, so the dtype grammar is not only exercised on `<f4`. |
| `big_endian` / `v3_big_endian` | the ramp as `>f4` | The same numbers behind different bytes, which is the only way a byte-order bug shows up as a value error rather than as nothing. |

## Zarr v2 stores

Written with `zarr_format=2`, so each is a directory of `.zarray`, `.zattrs`
and dot-separated chunk keys.

- `raw` — no compressor at all, the floor case.
- `zlib`, `gzip`, `zstd`, `lz4` — one compressor each, no filter.
- `shuffle_zlib` — the shuffle filter ahead of a compressor, which is the
  filter-then-compressor ordering v2 specifies.
- `blosc_{blosclz,lz4,lz4hc,zlib,zstd}_{noshuffle,shuffle,bitshuffle}` — the
  blosc cross-product. Fifteen stores, because blosc's sub-codec and its
  shuffle are independent settings and a decoder can get either one right on
  its own.
- `blosc_multiblock`, `blosc_bitshuffle_ragged` — the two blosc edge cases
  described above.
- `fortran_order` — `order="F"`, which stores different bytes for the same
  numbers. Paired with a C-order store in the tests so the difference is
  asserted rather than assumed.
- `big_endian`, `int16` — the dtype-grammar cases above.

## Zarr v3 stores

Written with `zarr_format=3`, so each is a directory of `zarr.json` and
slash-separated `c/` chunk keys, and the codec chain is an ordered list rather
than a filter/compressor pair.

- `v3_bytes` — the `bytes` codec alone, the v3 floor case.
- `v3_gzip`, `v3_zstd`, `v3_blosc` — one compression codec after `bytes`.
- `v3_crc32c` — the checksum codec, which appends a trailing digest the decoder
  must verify and strip rather than hand back as data.
- `v3_transpose` — the `transpose` codec, v3's replacement for v2's `order`.
- `v3_big_endian` — `bytes` with `endian: "big"`.
- `v3_shard` — the `sharding_indexed` codec: several inner chunks in one stored
  object, with an index that says where each begins.
- `v3_shard_sparse` — a shard whose index marks inner chunks as absent. An
  absent inner chunk is the fill value, not an error, and the index encodes it
  as an all-ones offset that a decoder must not read as a real position.

## `oracle.json`

The value oracle for every store above: for each one, each chunk's key, its
index in the chunk grid, and the exact values that chunk must decode to, taken
from the array zarr-python was handed. `every_committed_store_decodes_to_the_values_it_was_written_from`
in `tests/real_stores.rs` reads it, and
`the_corpus_covers_every_codec_the_crate_decodes` asserts the corpus has not
fallen behind the codecs the crate claims to support.

## Whole stores (#658)

`stores/` holds eight whole stores, written by the same script with
`--stores-only` (zarr-python 3.3.0, numcodecs 0.16.5, xarray 2026.7.0), and
`stores_oracle.json` records what zarr-python and xarray see in each. The
codec corpus above is one root array per store because it tests a chunk; these
test the walk and the region read.

| Store | What it covers |
| --- | --- |
| `v2_nested` | v2, consolidated (`.zmetadata`), `.` chunk keys, a nested group, `_ARRAY_DIMENSIONS`, a 5x7 array in 2x3 chunks — ragged on both axes — with only its top-left written, so whole chunks are absent. |
| `v2_slash` | The same layout with `/` chunk keys and no consolidated metadata, so the walker has to list the store. |
| `v3_nested` | The same layout in v3, consolidated inline in the root `zarr.json`, with `dimension_names`. |
| `v3_v2keys` | v3 with the `v2` chunk key encoding, a NaN fill value, no dimension names, unconsolidated. |
| `v3_sharded` | 4x4 shards of 2x2 inner chunks over a 5x7 array, sparse at both levels. |
| `v2_problems` | A `bz2` array (listed; reading it fails), a `<U4` array and a dimension-length clash (both left out), beside an array that reads. |
| `cf_v2`, `cf_v3` | One xarray dataset written in each edition: a packed `int16` with `scale_factor`, `add_offset` and `_FillValue`, and a float with a `-9999` sentinel. Their physical values are xarray's own decode. |

The expected attributes apply the two things xarray does to `_FillValue` — v2
keeps it as the array's `fill_value`, and v3 writes a float one as base64 of its
bytes — computed in the script independently of the Rust that undoes them.

## Licensing

These are synthetic arrays generated by the build script — `np.arange` ramps,
not anyone's data. No third-party content, and no licence obligations beyond
the repository's own.
