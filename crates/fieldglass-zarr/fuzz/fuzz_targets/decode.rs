//! libFuzzer target for the Zarr chunk-decode path.
//!
//! `fieldglass-zarr` reads two kinds of attacker-controllable input, and they
//! are different inputs rather than two spellings of one:
//!
//! * **A metadata document.** A `.zarray` or a `zarr.json` fetched from a store
//!   states the codec chain, the chunk shape and the element type. A malformed
//!   one must be refused by name, not mis-read into a chain that then decodes
//!   something.
//! * **A stored chunk.** Length-prefixed binary written by someone else: a
//!   blosc header declaring an uncompressed size and a block count, a
//!   block-offset table that indexes into the buffer, two hand-rolled LZ77
//!   decoders (LZ4 and BloscLZ) doing overlap-copies from a match distance the
//!   stream chooses, and a v3 shard index of offset/length pairs whose "absent"
//!   sentinel is an all-ones offset a decoder must not read as a position.
//!
//! Both halves run on every input. The crate's unit tests reach the malformed
//! cases someone thought to hand-write; its fixtures are all well-formed stores
//! written by zarr-python, so the malformed space is exactly what is otherwise
//! uncovered.

#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;

use fieldglass_zarr::ChunkDecoder;

/// The fixed decoders the chunk half runs against, built from real fixture
/// documents rather than copies of them: a copy drifts silently, and
/// `include_str!` of a renamed fixture is a compile error in the one job that
/// builds this crate.
///
/// Three, because they reach disjoint code:
///
/// * **blosc** is the container, so the fuzzer owning the chunk's header bytes
///   reaches all five inner compressors (blosclz, lz4, lz4hc, zlib, zstd) and
///   both shuffle modes through this one decoder.
/// * **raw** has no codec at all, so `decode_raw_values`' element-width and length
///   arithmetic is reached without a decompressor refusing the buffer first.
/// * **sharded** is the only way into the shard index.
///
/// `expect` rather than a silent skip: a decoder that stopped building would
/// leave a fuzz run that green-lights a target decoding nothing.
static FIXED: LazyLock<[ChunkDecoder; 3]> = LazyLock::new(|| {
    [
        ChunkDecoder::from_v2_metadata(include_str!(
            "../../tests/fixtures/blosc_zstd_shuffle/.zarray"
        ))
        .expect("the blosc fixture's .zarray builds a decoder"),
        ChunkDecoder::from_v2_metadata(include_str!("../../tests/fixtures/raw/.zarray"))
            .expect("the raw fixture's .zarray builds a decoder"),
        ChunkDecoder::from_v3_metadata(include_str!("../../tests/fixtures/v3_shard/zarr.json"))
            .expect("the shard fixture's zarr.json builds a decoder"),
    ]
});

/// The element count above which a fuzzer-declared chunk shape is not decoded.
///
/// `CodecChain::decode` bounds every decompression step by the *array's own*
/// geometry — element count times element width — which is exact for a real
/// store and unbounded for a metadata document the fuzzer wrote. Decoding a
/// declared 10^9-element chunk would reserve the buffer the document asked for
/// and be reported as a libFuzzer OOM on almost every input, drowning the
/// findings that are about this crate's arithmetic.
///
/// The guard is about keeping the run informative, not about a safe input:
/// that a fetched `.zarray` names an allocation directly is a real property of
/// the current API and is filed separately.
const MAX_DECLARED_ELEMENTS: usize = 1 << 16;

fuzz_target!(|data: &[u8]| {
    // ---- Metadata half -------------------------------------------------
    //
    // Lossy rather than a `from_utf8` early return: a metadata document is
    // fetched text and need not be well-formed UTF-8, and discarding those
    // inputs would throw away most of what the fuzzer generates.
    let text = String::from_utf8_lossy(data);
    for built in [
        ChunkDecoder::from_v2_metadata(&text),
        ChunkDecoder::from_v3_metadata(&text),
    ] {
        let Ok(decoder) = built else { continue };
        // The cheap accessors, which a host reads before it decodes anything.
        let _ = decoder.dtype();
        let _ = decoder.chain();
        let _ = decoder.fill_value();

        // A chain the *fuzzer* chose — a transpose with a bogus axis order, a
        // shuffle with a zero element width, a sharding codec nested where it
        // cannot be — applied to the fuzzer's own bytes. The fixed decoders
        // below never reach this space, because their chains come from
        // zarr-python.
        let elements = decoder
            .chunk_shape()
            .iter()
            .try_fold(1usize, |acc, &n| acc.checked_mul(n))
            .unwrap_or(usize::MAX);
        if elements <= MAX_DECLARED_ELEMENTS {
            drive(&decoder, data);
        }
    }

    // ---- Chunk half ----------------------------------------------------
    //
    // The same bytes as a stored chunk, against chains zarr-python really
    // wrote. Every allocation here is bounded by the fixture's geometry, so
    // the fuzzer steers the decoders' arithmetic and not their appetite.
    for decoder in FIXED.iter() {
        drive(decoder, data);
    }
});

/// Every decode entry point, on bytes that are expected to be refused.
///
/// The contract is no panic, no over-read and no hang; an error is the normal
/// outcome and says nothing either way.
fn drive(decoder: &ChunkDecoder, chunk: &[u8]) {
    let _ = decoder.decode(chunk);
    let _ = decoder.decode_raw_values(chunk);
    // Answers for any decoder, and gates nothing — `decode_shard` is driven
    // unconditionally so a chain that wrongly reports itself unsharded cannot
    // hide the shard-index walk from the fuzzer.
    let _ = decoder.is_sharded();
    let _ = decoder.decode_shard(chunk);
}
