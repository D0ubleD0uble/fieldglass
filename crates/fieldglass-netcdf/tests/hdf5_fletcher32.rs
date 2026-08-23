//! The fletcher32 checksum filter (HDF5 filter id 3, issue #412).
//!
//! fletcher32 is not a compressor. It appends a four-byte checksum to each
//! stored chunk, which reading verifies and strips, so a file whose pipeline is
//! `fletcher32 + deflate` used to fail outright even though the compression was
//! one already decoded — a spurious failure rather than a missing capability.
//!
//! `hdf5_fletcher32.h5` holds the same values five ways so the comparison is
//! direct: once with deflate alone, and four times with fletcher32 composed
//! into the pipeline. Every checksummed dataset must decode to what the plain
//! one does, and a chunk whose bytes have been altered must be reported rather
//! than returned.

use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};
use std::collections::BTreeMap;

const FLETCHER32: &[u8] = include_bytes!("fixtures/hdf5_fletcher32.h5");

/// Decode every root dataset, keyed by name.
fn decode_all(bytes: &[u8]) -> BTreeMap<String, Result<Vec<Option<f64>>, String>> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
    let probe = match &reader.backing {
        NetcdfBacking::Hdf5(p) => p.clone(),
        other => panic!("expected HDF5 backing, got {}", other.label()),
    };
    let names: Vec<String> = list_root_children(bytes, &probe)
        .expect("list children")
        .into_iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .map(|c| c.name)
        .collect();
    names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                name.clone(),
                reader
                    .decode_variable_values(index)
                    .map_err(|e| e.to_string()),
            )
        })
        .collect()
}

/// Chunk 0 of `f32_only`, as it sits on disk: that dataset has no compressor,
/// so its stored chunk is these bytes verbatim followed by the four checksum
/// bytes. The fixture holds `arange(64) * 0.5` shaped 8×8 in 4×8 chunks, so
/// chunk 0 is the first 32 elements.
fn f32_only_chunk0() -> Vec<u8> {
    (0..32)
        .flat_map(|i| ((i as f32) * 0.5).to_le_bytes())
        .collect()
}

/// The acceptance criterion: a checksummed pipeline decodes to exactly what the
/// same data written without the checksum does.
///
/// `f32_shuffle` is the one that matters most: shuffle and fletcher32 with no
/// compressor, which libhdf5 does write. fletcher32 is written last, so it
/// reverses first, and a reader that returned the chunk unchanged would hand
/// unshuffle four extra bytes. 132 divides evenly by the 4-byte element, so
/// unshuffle would regroup 33 elements instead of 32 and return wrong values
/// with no error at all. Comparing against the plain dataset is what catches
/// it. The compressed pipelines do *not* expose this — the trailing bytes are
/// absorbed by the zlib decoder — so dropping this dataset would leave the
/// suite passing against a passthrough implementation.
#[test]
fn every_checksummed_pipeline_decodes_like_the_plain_one() {
    let decoded = decode_all(FLETCHER32);
    let baseline = decoded
        .get("plain_deflate")
        .expect("plain_deflate present")
        .as_ref()
        .expect("plain_deflate decodes");

    assert_eq!(baseline.len(), 64, "fixture shape changed");
    assert_eq!(baseline[0], Some(0.0));
    assert_eq!(baseline[63], Some(31.5));

    for name in [
        "f32_deflate",
        "f32_only",
        "f32_shuffle_deflate",
        "f32_shuffle",
    ] {
        let got = decoded
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing from the fixture"))
            .as_ref()
            .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
        assert_eq!(got, baseline, "{name} disagreed with plain_deflate");
    }
}

/// An odd-length chunk, so the checksum's trailing-byte branch runs on a real
/// file rather than only in the unit tests.
///
/// This is also the fixture's only single-chunk dataset, so it covers the
/// chunk-index path that takes the stored length from the layout message's
/// filtered size (11 B) instead of the chunk shape (7 B). A reader using the
/// shape would split three bytes short and fail the checksum, so this passing
/// is what says that path reads the right length.
#[test]
fn an_odd_length_chunk_decodes() {
    let decoded = decode_all(FLETCHER32);
    let odd = decoded
        .get("f32_odd")
        .expect("f32_odd present")
        .as_ref()
        .expect("f32_odd decodes");
    assert_eq!(
        odd,
        &(0..7).map(|i| Some(i as f64)).collect::<Vec<_>>(),
        "7-byte chunk with a fletcher32 suffix"
    );
}

/// A corrupted chunk is reported, not silently returned — the whole point of
/// carrying the checksum.
///
/// The byte flipped is inside the chunk's data, leaving the stored checksum
/// alone, which is the shape of real bit-rot. `f32_only` is used because it has
/// no compressor: its chunk is the element bytes verbatim, so the corruption
/// lands exactly where intended rather than wherever a deflate stream happens
/// to put it.
#[test]
fn a_corrupted_chunk_is_reported() {
    let payload = f32_only_chunk0();
    let at = FLETCHER32
        .windows(payload.len())
        .position(|w| w == payload)
        .expect("f32_only chunk 0 is stored uncompressed and should be findable");

    // Sanity: unmodified, it decodes.
    assert!(
        decode_all(FLETCHER32)
            .get("f32_only")
            .expect("present")
            .is_ok(),
        "the pristine fixture must decode, else this test proves nothing"
    );

    let mut corrupt = FLETCHER32.to_vec();
    corrupt[at + 5] ^= 0xFF;

    let err = decode_all(&corrupt)
        .remove("f32_only")
        .expect("present")
        .expect_err("a flipped data byte must not decode as though it were fine");
    assert!(
        err.contains("fletcher32 checksum mismatch"),
        "expected a checksum mismatch, got: {err}"
    );
}

/// Corrupting the stored checksum itself is caught too — the comparison is
/// symmetric, and this pins that we are not simply ignoring the four bytes.
#[test]
fn a_corrupted_checksum_is_reported() {
    let payload = f32_only_chunk0();
    let at = FLETCHER32
        .windows(payload.len())
        .position(|w| w == payload)
        .expect("findable");

    let mut corrupt = FLETCHER32.to_vec();
    corrupt[at + payload.len()] ^= 0xFF; // first byte of the checksum suffix

    let err = decode_all(&corrupt)
        .remove("f32_only")
        .expect("present")
        .expect_err("a flipped checksum byte must be reported");
    assert!(
        err.contains("fletcher32 checksum mismatch"),
        "expected a checksum mismatch, got: {err}"
    );
}
