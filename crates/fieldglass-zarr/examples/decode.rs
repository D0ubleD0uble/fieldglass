//! Read a Zarr array's metadata, then decode its chunks to numbers.
//!
//!     cargo run -p fieldglass-zarr --example decode
//!
//! Runs against the committed fixture stores, so it works from a clean
//! checkout with no network. The set is deliberate: one v2 store with no
//! compressor at all, one v2 store behind blosc (the container that holds five
//! inner compressors and three shuffle modes), and one v3 store, which states
//! its codecs as an ordered chain rather than as a filter/compressor pair.
//! All three go through the same two calls — a `from_*_metadata` constructor
//! and [`ChunkDecoder::decode_values`] — because which edition wrote the store
//! is the metadata's problem, not the caller's.
//!
//! **This crate does not read stores.** A chunk arrives as bytes the caller
//! already has, which is what lets the same code serve a filesystem, an object
//! store and a test. Walking a directory to find those bytes is what the
//! example does by hand here, with `include_bytes!`, and what a store reader
//! will do properly (#658).

use fieldglass_zarr::{ChunkDecoder, FieldglassError};

/// v2, `compressor: null` — the chunk is the raw little-endian elements.
const RAW_META: &str = include_str!("../tests/fixtures/raw/.zarray");
const RAW_CHUNK: &[u8] = include_bytes!("../tests/fixtures/raw/0.0");

/// v2 behind blosc with LZ4 and a byte shuffle: a container header, a block
/// offset table, and a transpose to undo before the numbers are there.
const BLOSC_META: &str = include_str!("../tests/fixtures/blosc_lz4_shuffle/.zarray");
const BLOSC_CHUNK: &[u8] = include_bytes!("../tests/fixtures/blosc_lz4_shuffle/0.0");

/// v3, whose `zarr.json` states a codec chain. This one is `bytes` then
/// `zstd`, and the chunk key is slash-separated rather than dot-separated.
const V3_META: &str = include_str!("../tests/fixtures/v3_zstd/zarr.json");
const V3_CHUNK: &[u8] = include_bytes!("../tests/fixtures/v3_zstd/c/0/0");

fn main() -> Result<(), FieldglassError> {
    let stores: [(&str, ChunkDecoder, &[u8]); 3] = [
        (
            "v2 raw",
            ChunkDecoder::from_v2_metadata(RAW_META)?,
            RAW_CHUNK,
        ),
        (
            "v2 blosc+lz4+shuffle",
            ChunkDecoder::from_v2_metadata(BLOSC_META)?,
            BLOSC_CHUNK,
        ),
        (
            "v3 zstd",
            ChunkDecoder::from_v3_metadata(V3_META)?,
            V3_CHUNK,
        ),
    ];

    for (label, decoder, chunk) in stores {
        let values = decoder.decode_values(chunk)?;

        // The fixtures are a ramp on purpose: a transposition or a byte-order
        // slip breaks the progression instead of hiding in plausible noise.
        let (min, max) = values
            .iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        println!(
            "{label}: element type {:?}, chunk shape {:?}, {} value(s) in the raster, range {min}..={max}",
            decoder.dtype(),
            decoder.chunk_shape(),
            values.len(),
        );
    }

    // A codec this crate does not decode is refused by name rather than
    // mis-decoded, which is the branch a consumer has to handle.
    let unsupported = r#"{
        "zarr_format": 2, "shape": [4, 6], "chunks": [2, 3],
        "dtype": "<f4", "order": "C", "fill_value": 0.0,
        "compressor": {"id": "bz2", "level": 1}, "filters": null
    }"#;
    match ChunkDecoder::from_v2_metadata(unsupported) {
        Err(e) => println!("an unsupported compressor is refused: {e}"),
        Ok(_) => println!("UNEXPECTED: bz2 is not a codec this crate decodes"),
    }

    Ok(())
}
