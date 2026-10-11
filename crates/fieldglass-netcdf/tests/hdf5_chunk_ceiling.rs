//! A compressed chunk that would decode past what the reader holds for one
//! chunk is refused before it is read (#970).
//!
//! A chunk may legally be far larger than its dataset, and its dimensions are
//! the file's own numbers. So a 55 KB file can declare chunks that decode to a
//! gigabyte, and a valid file can hold one. Before #970 the reader inflated
//! such a chunk up to a 256 MiB ceiling and then reported the file as corrupt.
//! Now a chunk of more values than one field holds, or of more than 256 MiB,
//! is refused as too large, which a host reports as `unsupported`, before any
//! byte of it is read. A chunk the index does not store is never refused: it
//! reads as the fill value, as it always did.
//!
//! The fixture comes from `tools/build_hdf5_chunk_ceiling_fixture.py`;
//! provenance in `tests/fixtures/NOTICE.md`. Its oracle records where each
//! stored chunk lies, so "not read" is checked against the chunk's own bytes.

use std::ops::Range;

use fieldglass_core::array::ArrayError;
use fieldglass_core::testing::Recording;
use fieldglass_core::{ByteRange, FieldglassError};
use fieldglass_netcdf::hdf5::values::{read_dataset_region, read_dataset_values};
use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};
use serde_json::Value;

const FILE: &[u8] = include_bytes!("fixtures/hdf5_chunk_past_ceiling.h5");
const ORACLE: &str = include_str!("fixtures/hdf5_chunk_past_ceiling.h5.oracle.json");

/// What one chunk may decode to: `MAX_DECOMPRESSED_CHUNK` in `hdf5/filter.rs`.
const CHUNK_LIMIT: u64 = 256 << 20;

/// A `float64` chunk of `(5120, 8192)`: 40 Mi values, 320 MiB.
const F8_CHUNK_BYTES: u64 = 5120 * 8192 * 8;

/// The refusal for a chunk of [`F8_CHUNK_BYTES`].
fn f8_chunk_too_large() -> FieldglassError {
    ArrayError::ChunkTooLarge {
        bytes: F8_CHUNK_BYTES,
        limit: CHUNK_LIMIT,
    }
    .into()
}

/// One dataset's oracle entry.
fn oracle(name: &str) -> Value {
    let v: Value = serde_json::from_str(ORACLE).expect("oracle JSON");
    v["datasets"][name].clone()
}

/// The byte range of `name`'s one stored chunk, from the oracle.
fn stored_chunk(name: &str) -> ByteRange {
    let o = oracle(name);
    ByteRange::new(
        o["chunk_byte_offset"].as_u64().expect("a stored chunk"),
        o["chunk_stored_bytes"].as_u64().expect("a stored chunk"),
    )
}

/// What a read of a dataset returns.
type Decoded = Result<Vec<Option<f64>>, FieldglassError>;

/// Read `name` through a recording source: whole when `region` is `None`.
fn read(name: &str, region: Option<&[Range<u64>]>) -> (Decoded, Recording<&'static [u8]>) {
    let reader = NetcdfReader::from_bytes(FILE.to_vec()).expect("recognised NetCDF");
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        panic!("fixture is not HDF5");
    };
    let source = Recording::new(FILE);
    let address = list_root_children(&source, probe)
        .expect("list children")
        .into_iter()
        .find(|c| c.kind == ChildKind::Dataset && c.name == name)
        .unwrap_or_else(|| panic!("no dataset {name}"))
        .object_header_address;
    let result = match region {
        None => read_dataset_values(&source, address, probe),
        Some(region) => read_dataset_region(&source, address, probe, region),
    };
    (result, source)
}

/// No read and no planned fetch touches `chunk`'s bytes.
fn untouched(source: &Recording<&[u8]>, chunk: ByteRange) {
    let overlaps =
        |r: &ByteRange| r.start < chunk.start + chunk.len && chunk.start < r.start + r.len;
    let reads: Vec<ByteRange> = source.reads().into_iter().filter(overlaps).collect();
    assert!(
        reads.is_empty(),
        "the chunk at {chunk:?} was read: {reads:?}"
    );
    let planned: Vec<ByteRange> = source
        .prefetches()
        .into_iter()
        .flatten()
        .filter(overlaps)
        .collect();
    assert!(
        planned.is_empty(),
        "the chunk at {chunk:?} was planned: {planned:?}"
    );
}

/// The two regions every case reads: the whole `(4, 4)` dataset, and one cell.
const CELL: [Range<u64>; 2] = [1..2, 2..3];

/// A valid zstd chunk of 320 MiB, inside the field cap, is refused by its size
/// before it is read, whole and by region. libhdf5 reads it
/// (`libhdf5_values`); before #970 the reader inflated 256 MiB of it and
/// called the file corrupt.
#[test]
fn a_valid_chunk_past_256_mib_is_refused_unread() {
    let name = "zstd_f8_past_ceiling";
    assert_eq!(oracle(name)["chunk_bytes"], F8_CHUNK_BYTES);
    assert!(
        oracle(name)["libhdf5_values"].is_array(),
        "libhdf5 reads it"
    );
    for region in [None, Some(&CELL[..])] {
        let (result, source) = read(name, region);
        let err = result.expect_err("a chunk past 256 MiB");
        assert_eq!(
            err.to_string(),
            f8_chunk_too_large().to_string(),
            "{region:?}"
        );
        assert!(
            matches!(
                err,
                FieldglassError::Array(ArrayError::ChunkTooLarge { .. })
            ),
            "{err:?}"
        );
        untouched(&source, stored_chunk(name));
    }
}

/// A valid zstd chunk of 256 Mi `float32` values is past the field cap, the
/// check a Zarr chunk meets too, and is refused unread.
#[test]
fn a_valid_chunk_past_the_field_cap_is_refused_unread() {
    let name = "zstd_f4_past_field_cap";
    for region in [None, Some(&CELL[..])] {
        let (result, source) = read(name, region);
        let err = result.expect_err("a chunk past the field cap");
        assert!(
            matches!(
                err,
                FieldglassError::Array(ArrayError::FieldTooLarge {
                    elements: 268_435_456,
                    limit: 67_108_864,
                })
            ),
            "{region:?}: {err:?}"
        );
        untouched(&source, stored_chunk(name));
    }
}

/// A 17-byte deflate chunk declaring 320 MiB: a hostile file a reader would
/// have to inflate to find out it is short. libhdf5 reads past the 1 KiB it
/// inflates into memory it does not own; the reader refuses it unread.
#[test]
fn a_hostile_deflate_chunk_is_refused_unread() {
    let name = "deflate_f8_bomb";
    assert_eq!(stored_chunk(name).len, 17);
    for region in [None, Some(&CELL[..])] {
        let (result, source) = read(name, region);
        let err = result.expect_err("a chunk past 256 MiB");
        assert_eq!(
            err.to_string(),
            f8_chunk_too_large().to_string(),
            "{region:?}"
        );
        untouched(&source, stored_chunk(name));
    }
}

/// The same chunk shape with no chunk stored is not refused: nothing would be
/// inflated, and every value is the fill value, as libhdf5 reads it.
#[test]
fn an_unstored_chunk_past_the_limit_reads_as_fill() {
    let name = "deflate_f8_unwritten";
    assert!(
        oracle(name)["chunk_byte_offset"].is_null(),
        "no chunk stored"
    );
    let libhdf5: Vec<f64> = oracle(name)["libhdf5_values"]
        .as_array()
        .expect("libhdf5 reads it")
        .iter()
        .flat_map(|row| row.as_array().expect("a row").iter())
        .map(|v| v.as_f64().expect("a number"))
        .collect();
    assert_eq!(libhdf5, vec![0.0; 16]);
    let (whole, _) = read(name, None);
    assert_eq!(whole.expect("reads as fill"), vec![Some(0.0); 16]);
    let (cell, _) = read(name, Some(&CELL));
    assert_eq!(cell.expect("reads as fill"), vec![Some(0.0)]);
}

/// The public reader refuses the same way, whole and by region, so a host
/// sees the refusal it would see from `Session` (#942's path).
#[test]
fn the_public_reader_refuses_both_ways() {
    let reader = NetcdfReader::from_bytes(FILE.to_vec()).expect("parse");
    let view = reader.view().expect("view");
    let index = view
        .vars
        .iter()
        .find(|v| v.name() == "zstd_f8_past_ceiling")
        .expect("the variable is listed")
        .decode_index;
    for result in [
        reader.decode_variable_raw(index),
        reader.decode_region_raw(index, &CELL),
    ] {
        let err = result.expect_err("refused");
        assert_eq!(err.to_string(), f8_chunk_too_large().to_string());
    }
}
