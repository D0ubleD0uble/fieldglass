//! A `shuffle` filter's work is bounded by the chunk, not by the element
//! width the metadata states (#1862).
//!
//! numcodecs' `shuffle` takes any `elementsize`, and the filter transposes
//! only the whole elements of that width in the buffer. A width wider than
//! the chunk holds no whole element, so the chunk is left as it is. Before
//! #1862 the unshuffle still walked one empty row per byte of the stated
//! width, so a `.zarray` stating `elementsize: 4294967295` over a short chunk
//! ran four billion iterations wherever the empty loop was not optimized away,
//! as in a debug or fuzz build: the input the `decode` fuzz target timed out
//! on.
#![cfg(feature = "codecs")]

use std::time::{Duration, Instant};

use fieldglass_zarr::ChunkDecoder;

/// The width the fuzz input states: `u32::MAX`, so it is a valid `usize` on
/// a 32-bit target too, where a wider one is not read as a width at all.
const HUGE: u64 = u32::MAX as u64;

/// The time a short chunk may take. Generous for a debug build on a loaded
/// machine; the walk this replaced took far longer unoptimized.
const BUDGET: Duration = Duration::from_secs(2);

/// A `.zarray` for four `<f4` elements in one chunk under a `shuffle` of
/// `elementsize`. At [`HUGE`] it is byte for byte the fuzz corpus's
/// `shuffle_huge_elementsize.zarray.json`, which is not read from here because
/// the fuzz crate is not packaged with this one.
fn document(elementsize: u64) -> String {
    format!(
        r#"{{"shape":[4],"chunks":[4],"dtype":"<f4","compressor":null,"filters":[{{"id":"shuffle","elementsize":{elementsize}}}],"fill_value":0,"order":"C","zarr_format":2}}"#
    )
}

fn decoder(elementsize: u64) -> ChunkDecoder {
    ChunkDecoder::from_v2_metadata(&document(elementsize))
        .expect("a shuffle filter of any width is a valid chain")
}

/// The correct input: a chunk under a width it cannot hold one element of
/// decodes to its own bytes, as numcodecs leaves it, and at once.
#[test]
fn a_width_past_the_chunk_decodes_the_chunk_as_stored_at_once() {
    let decoder = decoder(HUGE);
    let stored: Vec<u8> = [1.0f32, 2.0, 3.0, 4.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let started = Instant::now();
    let values = decoder.decode_raw_values(&stored).expect("decodes");
    let took = started.elapsed();
    assert_eq!(values, [1.0, 2.0, 3.0, 4.0]);
    assert!(took < BUDGET, "a 16-byte chunk took {took:?}");
}

/// The fuzz input itself: the document read as its own chunk, as the fuzz
/// target drives it. It is longer than the 16 bytes the chunk shape asks for,
/// so it is refused, and the refusal comes as quickly as the decode above.
#[test]
fn the_fuzz_input_is_refused_at_once() {
    let document = document(HUGE);
    let decoder = decoder(HUGE);
    let started = Instant::now();
    assert!(decoder.decode(document.as_bytes()).is_err());
    let took = started.elapsed();
    assert!(took < BUDGET, "the fuzz input took {took:?}");
}
