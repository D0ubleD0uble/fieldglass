# fieldglass-zarr fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) target for the Zarr
chunk-decode path. `fieldglass-zarr` parses attacker-controllable bytes in two
shapes, and the target drives both on every input.

**A metadata document** — a [v2](https://zarr-specs.readthedocs.io/en/latest/v2/v2.0.html) `.zarray` or a [v3](https://zarr-specs.readthedocs.io/en/latest/v3/core/index.html) `zarr.json` — states the codec
chain, the chunk shape and the element type. `ChunkDecoder::from_v2_metadata`
and `from_v3_metadata` are driven on the input read as text, and a decoder that
builds is then applied to the input read as a chunk, which is the only way the
fuzzer reaches a codec chain *it* composed: a transpose with a bogus axis order,
a shuffle with a zero element width, a [sharding codec](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html) nested where it cannot be.

**A stored chunk** is length-prefixed binary: a blosc header declaring an
uncompressed size and a block count, a block-offset table that indexes into the
buffer, two hand-rolled LZ77 decoders (LZ4 and BloscLZ) doing overlap-copies
from a match distance the stream chooses, and a v3 [shard index](https://zarr-specs.readthedocs.io/en/latest/v3/codecs/sharding-indexed/index.html) of offset/length
pairs whose "absent" sentinel is an all-ones offset. That half runs against
three decoders built from real fixture documents — blosc, raw and sharded —
which between them reach all five inner compressors, both shuffle modes, the
element-width arithmetic and the shard index.

The crate has bounds checks and a decompression ceiling on each of those, and
unit tests for the malformed cases someone thought to hand-write. Its fixtures
are all well-formed stores written by zarr-python, so the malformed space is
what is otherwise uncovered.

This crate is intentionally **not** a member of the workspace, so the standard
stable-toolchain gates (`cargo fmt/clippy/test --workspace`) never try to build
the nightly-only libFuzzer target.

## Run

```sh
# from crates/fieldglass-zarr/fuzz
cargo +nightly fuzz run decode
```

The seed corpus under `corpus/decode/` is copied from the crate's own fixture
stores — four metadata documents, a blosc chunk, a multi-block blosc chunk, a
shard object, a shard compressed whole by gzip, and a chunk through the
`fixedscaleoffset` filter — so the fuzzer starts from bytes that decode rather than from
nothing. Their provenance is `tests/fixtures/NOTICE.md`.
`shuffle_huge_elementsize.zarray.json` is hand-written instead: a `.zarray`
whose `shuffle` filter states an `elementsize` of 4294967295 over a 16-byte
chunk. The unshuffle walked one empty row per byte of that width, four billion
of them (about 20 s in the fuzz build; an optimized build drops the empty
loop), before #1862 returned at once for a width past the buffer. A CI run
timed out on an input of this kind, and the seed keeps the case in every run.
The `shuffle_width` test decodes the same document. CI runs this target
time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
