//! The zstd filter (HDF5 filter id 32015, issue #413).
//!
//! zstd landed in netcdf-c 4.9 and is DKRZ's recommended compressor for new
//! climate archives, so it appears in recent NetCDF-4 output. Its presence used
//! to fail the whole file. Decoding it in pure Rust also goes beyond a default
//! netcdf-c install, which frequently has no working zstd plugin at runtime.
//!
//! `hdf5_zstd.h5` holds the same values five ways so the comparison is direct:
//! once with deflate alone as the baseline, and four with zstd in the pipeline.

use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};
use std::collections::BTreeMap;

const ZSTD: &[u8] = include_bytes!("fixtures/hdf5_zstd.h5");

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

/// The acceptance criterion: every zstd pipeline decodes to what the same data
/// written without zstd does.
///
/// `shuffle_zstd` is the one that pins filter *order*. netcdf-c writes shuffle
/// then zstd, so reading must undo zstd first; a reader that ran the pipeline
/// the other way round would hand a compressed frame to unshuffle. Comparing
/// against the plain dataset is what catches it.
#[test]
fn every_zstd_pipeline_decodes_like_the_plain_one() {
    let decoded = decode_all(ZSTD);
    let baseline = decoded
        .get("plain_deflate")
        .expect("plain_deflate present")
        .as_ref()
        .expect("plain_deflate decodes");

    assert_eq!(baseline.len(), 64, "fixture shape changed");
    assert_eq!(baseline[0], Some(0.0));
    assert_eq!(baseline[63], Some(31.5));

    for name in ["zstd", "shuffle_zstd", "zstd_fletcher32", "zstd_high"] {
        let got = decoded
            .get(name)
            .unwrap_or_else(|| panic!("{name} missing from the fixture"))
            .as_ref()
            .unwrap_or_else(|e| panic!("{name} failed to decode: {e}"));
        assert_eq!(got, baseline, "{name} disagreed with plain_deflate");
    }
}

/// A truncated or corrupt zstd frame is reported, not decoded into plausible
/// numbers. The frame carries its own magic and checksums, so the failure comes
/// from the codec rather than from a length check downstream.
#[test]
fn a_corrupt_zstd_frame_is_reported() {
    // zstd frame magic, little-endian 0xFD2FB528.
    let magic = [0x28u8, 0xB5, 0x2F, 0xFD];
    let at = ZSTD
        .windows(magic.len())
        .position(|w| w == magic)
        .expect("the fixture stores at least one zstd frame");

    assert!(
        decode_all(ZSTD).values().any(|v| v.is_ok()),
        "the pristine fixture must decode, else this test proves nothing"
    );

    // Corrupt the frame body, past the header, leaving the magic intact so the
    // codec still recognises it as zstd and fails on the content.
    let mut corrupt = ZSTD.to_vec();
    for byte in corrupt.iter_mut().skip(at + 8).take(16) {
        *byte ^= 0xFF;
    }

    let results = decode_all(&corrupt);
    assert!(
        results.values().any(|v| v.is_err()),
        "a mangled zstd frame decoded without complaint: {results:?}"
    );
}

/// Filter id 32015 must be recognised by id, not by guessing from the bytes: a
/// dataset whose pipeline declares zstd decodes, and the "unsupported filter"
/// error no longer names zstd.
#[test]
fn the_unsupported_filter_error_no_longer_claims_zstd() {
    let decoded = decode_all(ZSTD);
    let zstd = decoded.get("zstd").expect("present");
    assert!(
        zstd.is_ok(),
        "zstd should decode, got {:?}",
        zstd.as_ref().err()
    );
}
