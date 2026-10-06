//! A symbol-table name is read only from inside its local heap's data segment
//! (#908). The reader read the segment's address and threw its size away, so a
//! name offset at or past the end resolved to whatever bytes followed.
//!
//! Fixtures from `tools/build_hdf5_local_heap_fixture.py`, each a libhdf5
//! file with `alpha` and `beta` whose heap header states a shorter segment;
//! provenance in `tests/fixtures/NOTICE.md`, libhdf5's outcome in each oracle.

use fieldglass_core::testing::Recording;
use fieldglass_netcdf::{NetcdfBacking, NetcdfReader, list_root_children};

/// Segment size 16: `beta`'s offset 16 is at the end. libhdf5 refuses it too.
const SHORT: &[u8] = include_bytes!("fixtures/hdf5_local_heap_short.h5");
/// Segment size 24, `beta`'s name running to offset 24. libhdf5 2.0.0 lists
/// it as `betaxxxx`; the spec puts a name inside the segment, so the reader
/// refuses it, a known divergence.
const UNTERMINATED: &[u8] = include_bytes!("fixtures/hdf5_local_heap_unterminated.h5");

fn probe(bytes: &[u8]) -> fieldglass_netcdf::Hdf5Probe {
    match NetcdfReader::from_bytes(bytes.to_vec()).unwrap().backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5, got {}", other.label()),
    }
}

/// The local heap's data segment, as `(address, end)`: header `HEAP`,
/// version, reserved, segment size, free-list head, segment address. Checked
/// against the size the fixture states, so a rebuilt file whose first `HEAP`
/// is something else fails here rather than letting both filters pass.
fn segment(bytes: &[u8], size: u64) -> (u64, u64) {
    let heap = bytes
        .windows(4)
        .position(|w| w == b"HEAP")
        .expect("a local heap");
    let field = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
    assert_eq!(bytes[heap + 4], 0, "a version-0 local heap");
    assert_eq!(field(heap + 8), size, "the fixture's segment size");
    let address = field(heap + 24);
    (address, address + size)
}

fn refused_without_reading_past_the_segment(bytes: &[u8], size: u64, why: &str) {
    let p = probe(bytes);
    let source = Recording::new(bytes);
    let err = list_root_children(&source, &p).expect_err("refused");
    assert!(err.to_string().contains(why), "{err}");
    let (address, end) = segment(bytes, size);
    let reads = source.reads();
    // A name read starts inside the segment, so it is caught by where it
    // *ends* (#915): a scan from `beta`'s offset that fetched a window
    // crossing the end would otherwise pass. A read of an unrelated
    // structure before the heap may cross `end` and is not a name read.
    let overrun: Vec<_> = reads
        .iter()
        .filter(|r| (address..end).contains(&r.start) && r.end().is_none_or(|e| e > end))
        .collect();
    assert!(
        overrun.is_empty(),
        "a read inside the segment [{address}, {end}) runs past its end: {overrun:?}"
    );
    let past: Vec<_> = reads
        .iter()
        .filter(|r| r.start >= end && r.start < end + 64)
        .collect();
    assert!(
        past.is_empty(),
        "a read starts at or past the segment's end {end}: {past:?}"
    );
}

#[test]
fn a_name_offset_past_the_segment_is_refused() {
    refused_without_reading_past_the_segment(
        SHORT,
        16,
        "past the local heap's 16-byte data segment",
    );
}

#[test]
fn a_name_running_past_the_segment_is_refused() {
    refused_without_reading_past_the_segment(
        UNTERMINATED,
        24,
        "runs past the end of its local heap",
    );
}
