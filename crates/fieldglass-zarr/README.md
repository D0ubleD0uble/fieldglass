# fieldglass-zarr

Zarr chunk codecs: what a stored chunk has to be run through to become numbers.

Zarr keeps an array as a grid of chunks, each its own object in a store, and
each written through a chain of codecs the array's metadata names. This crate
reads that chain and reverses it. Both editions are covered — v2's `filters`
plus `compressor`, and v3's `codecs` list — including blosc and its five inner
compressors, the byte and bit transposes, gzip and zlib and zstd and LZ4, and
v3's `sharding_indexed`, which puts a grid of chunks in one object behind an
index.

```rust
use fieldglass_zarr::ChunkDecoder;

// A `.zarray`, as zarr-python writes one. `compressor: null` means the chunk
// is the raw little-endian elements.
let zarray = r#"{
    "zarr_format": 2, "shape": [4, 6], "chunks": [2, 3],
    "dtype": "<f4", "order": "C", "fill_value": 0.0,
    "compressor": null, "filters": null
}"#;
let decoder = ChunkDecoder::from_v2_metadata(zarray)?;

let chunk: Vec<u8> = (0..6u32).flat_map(|i| (i as f32).to_le_bytes()).collect();
assert_eq!(decoder.decode_values(&chunk)?, vec![0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
# Ok::<(), fieldglass_zarr::FieldglassError>(())
```

## What it is not

**It performs no I/O.** A chunk arrives as bytes the caller already has, the
same way every other Fieldglass decoder works (ADR-0005). Walking a directory or
a bucket to find the chunks is the host's job, and it is the only part of
reading a Zarr store that differs between a filesystem, an object store and a
browser.

**It does not address chunks.** Which key holds which region of the array —
`temp/0.1` under v2's dimension separator, `temp/c/0/1` under v3's chunk key
encoding, or an entry in a kerchunk reference document — is
`fieldglass-fetchplan`'s question, and it answers it for the remote case too, as
byte ranges. This crate reads the chunk *shape*, because a decode cannot check
its own output length or reverse a transpose without it, and nothing else about
the array's layout.

**It decodes; it never encodes.** The forward direction of each transform exists
only under `#[cfg(test)]`, where a round trip is the one check that does not
simply restate the inverse it is testing.

## Codecs

| Codec | v2 `id` | v3 `name` | Notes |
|---|---|---|---|
| blosc | `blosc` | `blosc` | Container plus one of blosclz, lz4, lz4hc, zlib, zstd; byte or bit shuffle. Snappy is refused. |
| zlib | `zlib` | — | RFC 1950. |
| gzip | `gzip` | `gzip` | RFC 1952, header fields and all. |
| zstd | `zstd` | `zstd` | Bounded window and output. |
| LZ4 | `lz4` | — | The numcodecs framing: a four-byte length, then a block. |
| shuffle | `shuffle` | — | Byte transpose; v3 reaches it through blosc. |
| bytes | — | `bytes` | States the element byte order. |
| transpose | — | `transpose` | Axis permutation; v2's `order: "F"` is the same thing. |
| crc32c | — | `crc32c` | Castagnoli, appended little-endian. |
| sharding | — | `sharding_indexed` | A grid of inner chunks in one object. |

Anything else — `bz2`, `vlen-utf8`, the extension codecs — is refused by name
rather than mis-decoded. Element types are the fixed-width numbers: `bool`, the
signed and unsigned integers to 64 bits, `float32` and `float64`. Structured
records, datetimes, strings, complex numbers and `float16` are refused, because
each is a different question about what a value even is.

## Validation

Every codec is checked against chunks written by `zarr-python` and `numcodecs`,
committed under `tests/fixtures/` with their provenance in
[`tests/fixtures/NOTICE.md`](tests/fixtures/NOTICE.md). The suite needs neither
library at run time: the stores and the expected values are committed, and
`tools/build_zarr_fixtures.py` regenerates them.

BloscLZ and LZ4 are decompressed here rather than pulled in as dependencies.
They are the two LZ77 dialects blosc carries, each under a hundred lines to
reverse, and having them side by side means one set of bounds and overlap-copy
tests covers both.

## Licence

MIT OR Apache-2.0
