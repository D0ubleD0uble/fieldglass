# fieldglass-zarr fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) target for the Zarr
chunk-decode path. `fieldglass-zarr` parses attacker-controllable bytes in two
shapes, and the target drives both on every input.

**A metadata document** — a v2 `.zarray` or a v3 `zarr.json` — states the codec
chain, the chunk shape and the element type. `ChunkDecoder::from_v2_metadata`
and `from_v3_metadata` are driven on the input read as text, and a decoder that
builds is then applied to the input read as a chunk, which is the only way the
fuzzer reaches a codec chain *it* composed: a transpose with a bogus axis order,
a shuffle with a zero element width, a sharding codec nested where it cannot be.

**A stored chunk** is length-prefixed binary: a blosc header declaring an
uncompressed size and a block count, a block-offset table that indexes into the
buffer, two hand-rolled LZ77 decoders (LZ4 and BloscLZ) doing overlap-copies
from a match distance the stream chooses, and a v3 shard index of offset/length
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
stores — two metadata documents, a blosc chunk, a multi-block blosc chunk and a
shard object — so the fuzzer starts from bytes that decode rather than from
nothing. Their provenance is `tests/fixtures/NOTICE.md`. CI runs this target
time-boxed on pull requests that touch the crate; see
`.github/workflows/fuzz.yml`.
