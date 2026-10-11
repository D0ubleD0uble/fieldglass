//! The reader reads a NetCDF file by range (#950).
//!
//! `region_io.rs` proves the region paths read only their runs and chunks when
//! handed a source directly. This proves the same of the reader a host holds:
//! `NetcdfReader::from_source` over a source that records what it is asked
//! for, opened, listed and then read a plane at a time, as a viewer does. Each
//! step reads what it needs and no more:
//!
//! * opening reads the classic header, or the HDF5 superblock, and no data;
//! * a plane reads its own runs (classic) or the metadata and the chunks it
//!   overlaps (HDF5), and never a byte of a chunk or run of another plane.
//!
//! Before #950 the reader held the file as a `Vec` and had no source to read
//! through, so none of this could be asked of it.

use std::ops::Range;

use std::borrow::Cow;

use fieldglass_core::array::ArrayError;
use fieldglass_core::bytes::FIRST_WINDOW_BYTES;
use fieldglass_core::testing::Recording;
use fieldglass_core::{ByteRange, ByteSource, FieldglassError};
use fieldglass_netcdf::classic::MAX_HEADER_BYTES;
use fieldglass_netcdf::{NetcdfBacking, NetcdfReader};

/// Classic files: fixed variables in all three versions, interleaved record
/// variables, and a lone record variable.
const CLASSIC: &[(&str, &[u8])] = &[
    (
        "ersst_v5_187001_cdf1.nc",
        include_bytes!("fixtures/ersst_v5_187001_cdf1.nc"),
    ),
    (
        "ersst_v5_187001_cdf2.nc",
        include_bytes!("fixtures/ersst_v5_187001_cdf2.nc"),
    ),
    (
        "ersst_v5_187001_cdf5.nc",
        include_bytes!("fixtures/ersst_v5_187001_cdf5.nc"),
    ),
    ("wrf_lambert.nc", include_bytes!("fixtures/wrf_lambert.nc")),
    (
        "record_mixed_cdf1.nc",
        include_bytes!("fixtures/record_mixed_cdf1.nc"),
    ),
    (
        "record_single_short_cdf2.nc",
        include_bytes!("fixtures/record_single_short_cdf2.nc"),
    ),
];

/// NetCDF-4 and HDF5 files: every chunk index the reader walks, contiguous
/// storage, a userblock, and real files with nested groups and dense
/// attributes.
const HDF5: &[(&str, &[u8])] = &[
    (
        "oisst_avhrr_v2.nc",
        include_bytes!("fixtures/oisst_avhrr_v2.nc"),
    ),
    (
        "goes16_abi_cmip.nc",
        include_bytes!("fixtures/goes16_abi_cmip.nc"),
    ),
    (
        "netcdf4_dimscale.nc",
        include_bytes!("fixtures/netcdf4_dimscale.nc"),
    ),
    (
        "rtofs_tripolar_arctic.nc",
        include_bytes!("fixtures/rtofs_tripolar_arctic.nc"),
    ),
    (
        "mirs_swath_n21.nc",
        include_bytes!("fixtures/mirs_swath_n21.nc"),
    ),
    (
        "hdf5_v1_symboltable.h5",
        include_bytes!("fixtures/hdf5_v1_symboltable.h5"),
    ),
    (
        "hdf5_v2_btree_index.h5",
        include_bytes!("fixtures/hdf5_v2_btree_index.h5"),
    ),
    (
        "hdf5_v4_chunk_index.h5",
        include_bytes!("fixtures/hdf5_v4_chunk_index.h5"),
    ),
    (
        "hdf5_ea_chunk_index.h5",
        include_bytes!("fixtures/hdf5_ea_chunk_index.h5"),
    ),
    (
        "hdf5_ea_filtered.h5",
        include_bytes!("fixtures/hdf5_ea_filtered.h5"),
    ),
    (
        "hdf5_implicit_index.h5",
        include_bytes!("fixtures/hdf5_implicit_index.h5"),
    ),
    (
        "hdf5_btreev2_multilevel.h5",
        include_bytes!("fixtures/hdf5_btreev2_multilevel.h5"),
    ),
    (
        "hdf5_userblock_earliest.h5",
        include_bytes!("fixtures/hdf5_userblock_earliest.h5"),
    ),
    (
        "netcdf4_large_sparse.nc",
        include_bytes!("fixtures/netcdf4_large_sparse.nc"),
    ),
];

/// The most elements a variable may have for its whole region to be read, to
/// learn every byte that holds it.
const WHOLE_LIMIT: u64 = 1 << 22;

/// Half-open byte spans, merged and sorted.
fn spans(ranges: &[ByteRange]) -> Vec<(u64, u64)> {
    let mut all: Vec<(u64, u64)> = ranges
        .iter()
        .filter(|r| r.len > 0)
        .map(|r| (r.start, r.start + r.len))
        .collect();
    all.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::new();
    for (s, e) in all {
        match out.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => out.push((s, e)),
        }
    }
    out
}

/// Bytes covered by merged spans.
fn covered(spans: &[(u64, u64)]) -> u64 {
    spans.iter().map(|(s, e)| e - s).sum()
}

/// `a` less `b`, both merged.
fn minus(a: &[(u64, u64)], b: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    for &(mut s, e) in a {
        for &(bs, be) in b {
            if be <= s || bs >= e {
                continue;
            }
            if bs > s {
                out.push((s, bs));
            }
            s = s.max(be);
            if s >= e {
                break;
            }
        }
        if s < e {
            out.push((s, e));
        }
    }
    out
}

/// Bytes `a` and `b` share, both merged.
fn shared(a: &[(u64, u64)], b: &[(u64, u64)]) -> u64 {
    a.iter()
        .flat_map(|&(s, e)| b.iter().map(move |&(bs, be)| (s.max(bs), e.min(be))))
        .filter(|(s, e)| s < e)
        .map(|(s, e)| e - s)
        .sum()
}

/// Every range a recording saw: its reads and its batches.
fn asked<S>(source: &Recording<S>) -> Vec<ByteRange> {
    let mut all = source.reads();
    all.extend(source.prefetches().into_iter().flatten());
    all
}

/// The plane of `shape` at index `at` of every axis but the last two.
fn plane(shape: &[u64], at: u64) -> Vec<Range<u64>> {
    let rank = shape.len();
    shape
        .iter()
        .enumerate()
        .map(|(axis, &n)| {
            if axis + 2 >= rank {
                0..n
            } else {
                let i = at.min(n - 1);
                i..i + 1
            }
        })
        .collect()
}

/// Opening a classic file reads its header and no data: the first variable's
/// data begins after the header, and the reads stop short of twice that,
/// which is what a header read in growing windows costs.
#[test]
fn opening_a_classic_file_reads_its_header_and_no_data() {
    let mut larger = 0usize;
    for &(name, bytes) in CLASSIC {
        let source = Recording::new(bytes);
        let reader = NetcdfReader::from_source(&source).expect("opens");
        let NetcdfBacking::Classic(header) = &reader.backing else {
            panic!("{name} is not classic");
        };
        let data = header
            .variables
            .iter()
            .map(|v| v.begin)
            .min()
            .expect("a variable");
        let read = spans(&asked(&source));
        let end = read.last().map_or(0, |&(_, e)| e);
        assert_eq!(read.first().map(|&(s, _)| s), Some(0), "{name}");
        assert!(
            end <= 2 * data + FIRST_WINDOW_BYTES as u64,
            "{name}: read to {end}, data at {data}"
        );
        // A file smaller than the first windows is read whole, rightly; one
        // with more data than header is not.
        if bytes.len() as u64 > 2 * data + FIRST_WINDOW_BYTES as u64 {
            assert!(
                covered(&read) < bytes.len() as u64,
                "{name}: read the whole file"
            );
            larger += 1;
        }
    }
    assert!(larger >= 3, "only {larger} files larger than their header");
}

/// Opening an HDF5 file reads where its signature may be and the start of its
/// superblock, and nothing else: the metadata walk waits for a call that
/// needs it.
#[test]
fn opening_an_hdf5_file_reads_its_superblock_only() {
    for &(name, bytes) in HDF5 {
        let source = Recording::new(bytes);
        let reader = NetcdfReader::from_source(&source).expect("opens");
        assert!(
            matches!(reader.backing, NetcdfBacking::Hdf5(_)),
            "{name} is not HDF5"
        );
        let read = covered(&spans(&asked(&source)));
        // Eight bytes at each offset a signature may be at before the one it
        // is at, and sixteen of superblock after it.
        assert!(read <= 64, "{name}: opening read {read} bytes");
    }
}

/// A plane of a classic variable reads exactly its runs, in one batch, and
/// nothing else: the header was read when the file was opened.
#[test]
fn a_classic_plane_reads_its_runs_and_nothing_else() {
    let mut planes = 0usize;
    for &(name, bytes) in CLASSIC {
        let source = Recording::new(bytes);
        let reader = NetcdfReader::from_source(&source).expect("opens");
        let NetcdfBacking::Classic(header) = &reader.backing else {
            panic!("{name} is not classic");
        };
        let view = reader.view().expect("view");
        for var in &view.vars {
            let index = var.decode_index;
            let shape = reader.variable_shape(index).expect("shape");
            if shape.len() < 2 || shape.contains(&0) {
                continue;
            }
            let region = plane(&shape, shape[0] - 1);
            let Ok(plan) = fieldglass_netcdf::classic::region_plan(header, index, &region) else {
                continue; // text
            };
            source.clear();
            let fixed = vec![(shape[0] - 1) as usize; shape.len()];
            let rank = shape.len();
            if reader
                .decode_plane(var, rank - 2, rank - 1, &fixed)
                .is_err()
            {
                continue; // text
            }
            assert_eq!(
                source.prefetches(),
                std::slice::from_ref(&plan),
                "{name} {}",
                var.name()
            );
            assert_eq!(source.reads(), plan, "{name} {}", var.name());
            planes += 1;
        }
    }
    assert!(planes >= 10, "only {planes} classic planes");
}

/// A plane of an HDF5 variable reads metadata and the chunks or runs it
/// overlaps, and never a byte that holds only other planes' values. Listing
/// the variables reads none of a variable's values at all. A 2-D variable's
/// plane is all of it, so there a row stands in for the plane.
///
/// Which bytes hold a variable is learned the way the reader does it: a whole
/// region read of the same variable on a fresh reader, whose batch names every
/// chunk or run. Each plane is then read on its own fresh reader, so no chunk
/// the file's memo kept from an earlier read can hide a read.
#[test]
fn an_hdf5_plane_reads_its_own_chunks_and_no_others() {
    let mut planes = 0usize;
    let mut apart = 0usize;
    for &(name, bytes) in HDF5 {
        let lister = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        let view = lister.view().expect("view");
        for var in &view.vars {
            let index = var.decode_index;
            let shape = lister.variable_shape(index).expect("shape");
            if shape.len() < 2 || shape.contains(&0) || shape.iter().product::<u64>() > WHOLE_LIMIT
            {
                continue;
            }

            // Every byte the variable's values live in.
            let whole_source = Recording::new(bytes);
            let whole_reader = NetcdfReader::from_source(&whole_source).expect("opens");
            let whole_view = whole_reader.view().expect("view");
            let listed = spans(&asked(&whole_source));
            whole_source.clear();
            let all: Vec<Range<u64>> = shape.iter().map(|&n| 0..n).collect();
            if whole_reader.decode_region_raw(index, &all).is_err() {
                continue; // text, or a type outside the decoded subset
            }
            let values = spans(&whole_source.prefetches().concat());
            assert_eq!(
                shared(&listed, &values),
                0,
                "{name} {}: listing the variables read its values",
                var.name()
            );
            drop(whole_view);

            for at in [0, shape[0] - 1] {
                let source = Recording::new(bytes);
                let reader = NetcdfReader::from_source(&source).expect("opens");
                let view = reader.view().expect("view");
                let var = view.var(index).expect("the same variable");
                source.clear();
                let rank = shape.len();
                if rank > 2 {
                    // A plane, through the call a viewer makes for one.
                    let fixed = vec![at as usize; rank];
                    reader
                        .decode_plane(var, rank - 2, rank - 1, &fixed)
                        .map(drop)
                } else {
                    // A 2-D variable's plane is the whole of it, so a row.
                    reader
                        .decode_region_raw(index, &[at..at + 1, 0..shape[1]])
                        .map(drop)
                }
                .unwrap_or_else(|e| panic!("{name} {} slab {at}: {e}", var.name()));
                let own = spans(&source.prefetches().concat());
                let others = minus(&values, &own);
                let read = spans(&asked(&source));
                assert_eq!(
                    shared(&read, &others),
                    0,
                    "{name} {} plane {at}: read bytes only other planes hold",
                    var.name()
                );
                if covered(&others) > 0 {
                    apart += 1;
                }
                planes += 1;
            }
        }
    }
    assert!(planes >= 20, "only {planes} HDF5 planes");
    // Planes that had other planes' bytes to stay out of. Without enough of
    // them the check above holds trivially, and a plane read that fetched the
    // whole variable would pass it.
    assert!(
        apart >= 10,
        "only {apart} planes had another plane's bytes beside them"
    );
}

/// A source that holds a few bytes and declares far more, as a remote object's
/// stated length can. Past the held bytes it serves zeros, as a cursor's
/// look-ahead window may ask for, but a read of more than a megabyte is
/// refused, so a fetch of what the header claims fails the test rather than
/// allocating it.
struct Declared {
    held: Vec<u8>,
    size: u64,
}

impl ByteSource for Declared {
    fn size(&self) -> u64 {
        self.size
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        if range.len > 1 << 20 {
            return Err(FieldglassError::Parse(format!(
                "read of {} bytes at {}",
                range.len, range.start
            )));
        }
        let start = range.start as usize;
        let out = (0..range.len as usize)
            .map(|i| self.held.get(start + i).copied().unwrap_or(0))
            .collect();
        Ok(Cow::Owned(out))
    }
}

/// A classic header claiming one attribute far past `MAX_HEADER_BYTES`, over a
/// source that declares itself large enough to hold it, is refused before the
/// attribute is read (#950): as a listing too large, which says the file is
/// fine, and with no read anywhere near the size the header claimed.
#[test]
fn a_header_past_its_cap_is_refused_before_it_is_read() {
    let header = |value_len: u32| {
        let mut h = Vec::new();
        h.extend_from_slice(b"CDF\x01");
        h.extend_from_slice(&0u32.to_be_bytes()); // numrecs
        h.extend_from_slice(&[0; 8]); // dim_list ABSENT
        h.extend_from_slice(&0x0Cu32.to_be_bytes()); // NC_ATTRIBUTE
        h.extend_from_slice(&1u32.to_be_bytes()); // one global attribute
        h.extend_from_slice(&1u32.to_be_bytes()); // name length
        h.extend_from_slice(b"a\0\0\0");
        h.extend_from_slice(&1u32.to_be_bytes()); // NC_BYTE
        h.extend_from_slice(&value_len.to_be_bytes());
        h
    };

    // A gigabyte of attribute value, in a source that says it has two.
    let source = Recording::new(Declared {
        held: header(1 << 30),
        size: 1 << 31,
    });
    let err = NetcdfReader::from_source(&source).expect_err("past the cap");
    assert!(
        matches!(
            err,
            FieldglassError::Array(ArrayError::ListingTooLarge { .. })
        ),
        "{err}"
    );
    assert!(err.to_string().contains("The file itself is fine"), "{err}");
    let largest = asked(&source).iter().map(|r| r.len).max().unwrap_or(0);
    assert!(
        largest <= FIRST_WINDOW_BYTES as u64,
        "read {largest} bytes at once"
    );

    // Just past the cap is refused too; the same header with a small value,
    // in a source that holds it all, opens.
    let past = (MAX_HEADER_BYTES as u32).saturating_sub(32);
    let source = Declared {
        held: header(past),
        size: 1 << 31,
    };
    assert!(matches!(
        NetcdfReader::from_source(source),
        Err(FieldglassError::Array(ArrayError::ListingTooLarge { .. }))
    ));
    let mut small = header(4);
    small.extend_from_slice(b"\x01\x02\x03\x04");
    small.extend_from_slice(&[0; 8]); // var_list ABSENT
    let size = small.len() as u64;
    NetcdfReader::from_source(Declared { held: small, size }).expect("a small value opens");
}
