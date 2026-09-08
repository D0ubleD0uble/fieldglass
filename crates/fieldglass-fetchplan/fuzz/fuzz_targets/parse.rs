//! libFuzzer target for the cloud-native manifest parsers.
//!
//! `fieldglass-fetchplan` reads sidecars fetched over the network — a wgrib2
//! `.idx` and an ECMWF `.index`, in two unrelated grammars — and turns them
//! into the byte ranges a host will then ask a bucket for. Nothing about their
//! shape is this crate's to assume, and #652 hardened three hostile-input paths
//! in it, which is evidence the surface has them rather than evidence it is now
//! clean.
//!
//! Five arms run on every input:
//!
//! 1. **wgrib2 `.idx`** — a colon-delimited line grammar with an offset per
//!    record and a length that is only ever implied by the *next* record.
//! 2. **ECMWF `.index`** — JSON lines, each stating its own offset and length.
//! 3. **Run discovery** — a `SourceSpec` deserialized from the same buffer,
//!    with a clock and a forecast step read off its tail, driving the key
//!    template scanner and the cycle arithmetic.
//! 4. **Kerchunk references** — one JSON document mapping store keys to inline
//!    data or to `[url, offset, length]`. Its hostile surface is wider than the
//!    two sidecars': a hand-rolled base64 decoder, a `{{name}}` scanner walking
//!    byte offsets through a string it does not control, and reference arrays
//!    whose numbers become a range.
//! 5. **Zarr metadata** — a `.zarray` or `zarr.json` read for the chunk grid.
//!    Every extent it states is divided by (`div_ceil` on a zero chunk extent
//!    is a division by zero) or multiplied out into a chunk key.
//!
//! A parse that succeeds is walked the way a host walks it: every record, the
//! collapsed one-per-message list, a query, and then the range arithmetic —
//! `close` against an object size, the HTTP `Range` header, and the §0 envelope
//! check against the input read back as message bytes. A reference document
//! that parses is walked the same way, through every key it declares and
//! through the chunk-grid arithmetic of every array it carries metadata for.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_fetchplan::{
    candidates, EcmwfIndex, KerchunkRefs, Manifest, NoResolver, PlanItem, Query, SourceSpec,
    Wgrib2Idx, ZarrArrayMeta,
};

/// Object sizes a closed range is tried against.
///
/// Zero and `u64::MAX` are the two ends `PlanRange::close`'s `checked_sub` is
/// about — an offset past the end of the object, and an open-ended range whose
/// length is the whole 64-bit space — and the middle value is an ordinary one
/// so the success path is exercised too.
const OBJECT_SIZES: [u64; 3] = [0, 4_096, u64::MAX];

/// Bytes taken off the tail of the input for the discovery arm: an `i64` clock
/// and a `u32` forecast step.
const TAIL: usize = 12;

fuzz_target!(|data: &[u8]| {
    // Lossy rather than a `from_utf8` early return. A sidecar is fetched text
    // and need not be ASCII — #652's ECMWF bug was a byte index landing inside
    // a multi-byte character — so the target has to be able to feed non-ASCII,
    // and discarding every input that is not valid UTF-8 would throw away most
    // of what the fuzzer generates.
    let text = String::from_utf8_lossy(data);

    if let Ok(idx) = Wgrib2Idx::parse("fuzz.grib2", &text) {
        walk(&idx, data);
    }
    if let Ok(index) = EcmwfIndex::parse("fuzz.grib2", &text) {
        walk(&index, data);
    }

    discovery(data);
    references(&text);
    grid(&text);
});

/// The kerchunk arm: a reference document, and everything a host asks of one.
///
/// A document that parses is walked in full rather than sampled. Every key is
/// asked for both ways — as inline data and as a range — because which of the
/// two an entry is decides which branch runs, and the fuzzer is the only thing
/// that will produce an entry of the wrong shape under a metadata key.
fn references(text: &str) {
    let Ok(refs) = KerchunkRefs::parse(text) else {
        return;
    };

    let keys: Vec<String> = refs.keys().map(str::to_string).collect();
    for key in &keys {
        let _ = refs.inline(key);
        if let Some(item) = refs.range_of(key) {
            ranges(&item, text.as_bytes());
        }
    }

    // `arrays` parses each metadata document to decide what to list, and
    // `array` parses it again for the caller; both reach the grid arithmetic
    // through whatever a fuzzer put under a `.zarray` key.
    for name in refs.arrays() {
        let Ok(meta) = refs.array(&name) else {
            continue;
        };
        walk_grid(&meta);
        // The indices a caller is most likely to hand over: the first chunk,
        // the last, and one past the end.
        for index in candidate_indices(&meta) {
            if let Ok(Some(item)) = refs.chunk_at(&name, &meta, &index) {
                ranges(&item, text.as_bytes());
            }
        }
        let region: Vec<std::ops::Range<u64>> =
            meta.shape().iter().map(|extent| 0..*extent).collect();
        let _ = refs.chunks_covering(&name, &meta, &region);
    }
}

/// The Zarr-metadata arm, reached without a reference document wrapped round it.
fn grid(text: &str) {
    if let Ok(meta) = ZarrArrayMeta::from_metadata(text) {
        walk_grid(&meta);
    }
    // The two editions are also reachable directly, and a document that sniffs
    // as one edition is still handed to the other's reader by a caller that
    // knows which file it opened.
    if let Ok(meta) = ZarrArrayMeta::from_v2_metadata(text) {
        walk_grid(&meta);
    }
    if let Ok(meta) = ZarrArrayMeta::from_v3_metadata(text) {
        walk_grid(&meta);
    }
}

/// The chunk-grid arithmetic, over the indices most likely to be off the end.
fn walk_grid(meta: &ZarrArrayMeta) {
    let _ = meta.chunk_grid();
    let _ = meta.rank();
    for index in candidate_indices(meta) {
        let _ = meta.chunk_key(&index);
        let _ = meta.chunk_containing(&index);
    }
    // A region spanning the whole array, and one that runs past its end.
    let whole: Vec<std::ops::Range<u64>> = meta.shape().iter().map(|e| 0..*e).collect();
    let _ = meta.chunks_covering(&whole);
    let over: Vec<std::ops::Range<u64>> = meta
        .shape()
        .iter()
        .map(|e| 0..e.saturating_add(1))
        .collect();
    let _ = meta.chunks_covering(&over);
    // A rank the array does not have, which every entry point has to refuse
    // before it indexes anything.
    let _ = meta.chunk_key(&[0]);
    let _ = meta.chunk_containing(&[0, 0, 0]);
}

/// Indices worth trying against an array: the origin, the last chunk, and one
/// past it.
fn candidate_indices(meta: &ZarrArrayMeta) -> Vec<Vec<u64>> {
    let grid = meta.chunk_grid();
    vec![
        vec![0; meta.rank()],
        grid.iter().map(|n| n.saturating_sub(1)).collect(),
        grid.clone(),
        grid.iter().map(|_| u64::MAX).collect(),
    ]
}

/// Everything a host does with a manifest once it has parsed.
///
/// The parse is only half the surface: `items` derives each record's length
/// from the next distinct offset (the O(n²) walk #652 replaced), `messages`
/// collapses the sub-message siblings, and the range arithmetic turns a stated
/// offset into a fetch.
fn walk(manifest: &dyn Manifest, bytes: &[u8]) {
    let _ = manifest.key();
    let items = manifest.items();
    let _ = manifest.messages();
    // The empty query selects every record, so this is the widest path through
    // the matcher rather than the cheapest.
    let _ = manifest.select(&Query::default(), &NoResolver);
    for item in &items {
        ranges(item, bytes);
    }
}

/// The arithmetic between a manifest's claim and a fetch.
fn ranges(item: &PlanItem, bytes: &[u8]) {
    let _ = item.range.http_range_header();
    let _ = item.range.offset();
    let _ = item.range.end_exclusive();
    let _ = item.range.length();
    for size in OBJECT_SIZES {
        let _ = item.range.close(size);
    }
    // The §0 envelope check reads the message's own declared length out of the
    // bytes a host fetched. Handing it the fuzz input is exactly the case it
    // exists for: bytes that are not the message the sidecar promised.
    let _ = item.expect.verify_envelope(bytes, &item.range);
}

/// The run-discovery arm: a source catalog entry, a clock, and a step.
///
/// The catalog is the host's *data* (ADR-0005 keeps it out of this crate), so a
/// `SourceSpec` arrives deserialized and its key template is a string this
/// crate scans. The clock is a parameter too, and #652 found a panic on
/// `i64::MIN` there, so the fuzzer is given control of it: the last twelve
/// bytes are the clock and the step, and everything before them is the JSON.
/// Framing it as a suffix keeps the seed readable as a document.
fn discovery(data: &[u8]) {
    let Some(split) = data.len().checked_sub(TAIL) else {
        return;
    };
    let (json, tail) = data.split_at(split);

    let mut clock = [0u8; 8];
    clock.copy_from_slice(&tail[..8]);
    let mut step = [0u8; 4];
    step.copy_from_slice(&tail[8..]);

    let Ok(spec) = serde_json::from_slice::<SourceSpec>(json) else {
        return;
    };
    // `candidates` validates the spec itself and returns on its error, so it
    // is the whole of this arm: the template scanner, then the arithmetic.
    let _ = candidates(&spec, i64::from_le_bytes(clock), u32::from_le_bytes(step));
}
