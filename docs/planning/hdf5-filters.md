# HDF5 filter coverage and cost

*Verified against the code 2026-08-23.*

`crates/fieldglass-netcdf/src/hdf5/filter.rs` decodes exactly two filters:
deflate (id 1) and shuffle (id 2). Any other filter in a pipeline fails the
whole file. Chunk indexing is already ahead of the field (all five v4 index
types plus the v1 B-tree), so filters are the gap that blocks real files.

| ID | Filter | Why it matters | Cost |
|---|---|---|---|
| 3 | fletcher32 | A checksum, not compression. Its presence fails files whose compression we handle fine. | Verify or skip the trailing 4 bytes. One afternoon. |
| 32015 | zstd | netcdf-c ≥ 4.9; DKRZ-recommended for climate archives. | Pure-Rust decoder (`ruzstd`). Small; needs an ADR-0001-style pin note. |
| 307 | bzip2 | Rare. | Pure-Rust decoder (`bzip2-rs`). Small. |
| 4 | szip | Common across the NASA EOS archive (AIRS, MODIS). | Same entropy coder as GRIB2 5.42, different framing, plus an upstream change. Multi-week. See below. |

Blosc/LZ4: rare in NetCDF, defer. This set would exceed default netcdf-c
installs, which frequently lack working szip/zstd plugins at runtime.

## Why szip is a project, not a quick win

The entropy coder (CCSDS 121.0 extended-Rice) is shared with GRIB2 5.42, but
`rust-aec` is an externally pinned crates.io dependency (`= 0.1.1`), not
vendored, and it rejects the parameters HDF5 actually uses:

- `validate_params` accepts `block_size` only in {8, 16, 32, 64}. HDF5's
  `pixels_per_block` is any even value 2–32; NASA EOS commonly ships 8, 10,
  16, 18. Real files return `Unsupported` immediately.
- `bits_per_sample` is capped at 1..=32, so 64-bit doubles, the common NetCDF
  case, are outside its range.
- Our four GRIB2 fixtures all use block 32 / RSI 128. HDF5 szip RSI is
  `ceil(pixels_per_scanline / pixels_per_block)`, typically 1–128 and often 1,
  an untested path.

HDF5 szip framing also differs from GRIB2 §7: a 4-byte little-endian
uncompressed-size prefix per chunk, scanline padding when
`pixels_per_scanline % pixels_per_block != 0`, and byte-interleaving for
32/64-bit samples (libaec decodes those as 8-bit streams and deinterleaves).
That is roughly libaec's `sz_compat.c` reimplemented (~300 lines), plus either
an upstream relaxation of `rust-aec`'s whitelist or a fork; ADR-0001 flags
that crate as bus-factor-1 and pinned exactly, so a fork is a real commitment.
It also needs its own oracle (an h5py/netCDF4 wheel built with szip;
`tools/build_hdf5_fixtures.py` is the pattern).

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
