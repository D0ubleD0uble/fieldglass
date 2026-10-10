//! A region read reads the region (#939): its own bytes in a classic file or a
//! contiguous dataset, the chunks it overlaps in a chunked one, and nothing
//! else. `region_reads.rs` proves the values; this proves the cost, through a
//! source that records what it is asked for.

use std::ops::Range;

use fieldglass_core::testing::Recording;
use fieldglass_netcdf::classic::{decode_region_raw_from, parse_header, region_plan};
use fieldglass_netcdf::hdf5::layout::{self, DataLayout};
use fieldglass_netcdf::hdf5::object_header;
use fieldglass_netcdf::hdf5::values::read_dataset_region;
use fieldglass_netcdf::{
    ChildKind, Hdf5Probe, NcType, NetcdfBacking, NetcdfReader, list_all_children,
};

/// Deterministic boxes inside a shape: every axis whole, each corner cell, and
/// a spread of sub-ranges.
fn regions(shape: &[u64]) -> Vec<Vec<Range<u64>>> {
    let mut out = vec![shape.iter().map(|&n| 0..n).collect::<Vec<_>>()];
    if shape.iter().all(|&n| n > 0) {
        out.push(shape.iter().map(|_| 0..1).collect());
        out.push(shape.iter().map(|&n| n - 1..n).collect());
        for k in 1..6u64 {
            out.push(
                shape
                    .iter()
                    .map(|&n| {
                        let a = (k * 7) % n;
                        let b = (k * 13 + 1) % n;
                        a.min(b)..a.max(b) + 1
                    })
                    .collect(),
            );
        }
    }
    out
}

/// Bytes per element of a classic type, from the format specification.
fn width(nc_type: NcType) -> u64 {
    match nc_type {
        NcType::Byte | NcType::UByte | NcType::Char => 1,
        NcType::Short | NcType::UShort => 2,
        NcType::Int | NcType::UInt | NcType::Float => 4,
        NcType::Double | NcType::Int64 | NcType::UInt64 => 8,
    }
}

fn hdf5(reader: &NetcdfReader) -> &Hdf5Probe {
    match &reader.backing {
        NetcdfBacking::Hdf5(probe) => probe,
        other => panic!("expected HDF5, got {}", other.label()),
    }
}

/// The object-header address of the dataset `name` in `bytes`.
fn dataset(bytes: &[u8], probe: &Hdf5Probe, name: &str) -> u64 {
    list_all_children(bytes, probe)
        .expect("children")
        .into_iter()
        .find(|c| c.name.trim_start_matches('/') == name)
        .unwrap_or_else(|| panic!("no dataset {name}"))
        .object_header_address
}

/// The data layout of the dataset at `addr`.
fn data_layout(bytes: &[u8], probe: &Hdf5Probe, addr: u64) -> DataLayout {
    let header = object_header::walk(bytes, addr, probe.offset_size, probe.length_size)
        .expect("object header");
    let body = header
        .messages
        .iter()
        .find(|m| m.msg_type == 0x0008)
        .expect("a layout message");
    layout::decode(&body.body, probe).expect("layout")
}

/// A classic region's plan is exactly what its decode reads, in one batch, and
/// is the region's own bytes: as many as its elements take, never the
/// variable's.
#[test]
fn a_classic_region_reads_its_own_bytes_and_no_others() {
    let mut checked = 0usize;
    // A record variable interleaved with others, a lone record variable of
    // `short` (unpadded records), and fixed variables in all three versions.
    for bytes in [
        &include_bytes!("fixtures/wrf_lambert.nc")[..],
        &include_bytes!("fixtures/record_mixed_cdf1.nc")[..],
        &include_bytes!("fixtures/record_single_short_cdf2.nc")[..],
        &include_bytes!("fixtures/ersst_v5_187001_cdf5.nc")[..],
    ] {
        let header = parse_header(bytes).expect("classic header");
        for (index, var) in header.variables.iter().enumerate() {
            let shape = fieldglass_netcdf::classic::variable_shape(&header, index).expect("shape");
            for region in regions(&shape) {
                let Ok(plan) = region_plan(&header, index, &region) else {
                    continue; // text
                };
                let source = Recording::new(bytes);
                decode_region_raw_from(&header, &source, index, &region)
                    .unwrap_or_else(|e| panic!("{} {region:?}: {e}", var.name));
                assert_eq!(source.reads(), plan, "{} {region:?}", var.name);
                assert_eq!(
                    source.prefetches(),
                    std::slice::from_ref(&plan),
                    "{}",
                    var.name
                );
                let elements: u64 = region.iter().map(|r| r.end - r.start).product();
                assert_eq!(
                    plan.iter().map(|r| r.len).sum::<u64>(),
                    elements * width(var.nc_type),
                    "{} {region:?} read bytes outside the region",
                    var.name
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 100, "only {checked} regions checked");
}

/// A chunked region reads the chunks it overlaps and no others: one batch
/// naming exactly those, then those reads, whatever the chunk index.
// One-axis regions are spelled `&[a..b]`, a region of one axis, which is what
// the lint takes for a mistyped `Vec` of the range's values.
#[allow(clippy::single_range_in_vec_init)]
#[test]
fn a_chunked_region_reads_only_the_chunks_it_overlaps() {
    type Case = (
        &'static [u8],
        &'static str,
        &'static [u64],
        &'static [&'static [Range<u64>]],
    );
    let cases: &[Case] = &[
        // A version-2 B-tree over a 4 x 4 grid of 2 x 2 chunks.
        (
            include_bytes!("fixtures/hdf5_v2_btree_index.h5"),
            "bt2_multi",
            &[2, 2],
            &[&[3..4, 0..8], &[5..6, 5..6], &[0..8, 0..8], &[1..5, 3..4]],
        ),
        // A version-1 B-tree, deflated, 4 x 4 chunks.
        (
            include_bytes!("fixtures/hdf5_v1_symboltable.h5"),
            "compressed",
            &[4, 4],
            &[&[0..1, 0..1], &[3..5, 3..5], &[7..8, 0..8]],
        ),
        // A fixed array.
        (
            include_bytes!("fixtures/hdf5_v4_chunk_index.h5"),
            "fixed_array",
            &[4, 4],
            &[&[6..7, 1..2], &[0..8, 4..5]],
        ),
        // Extensible arrays: through secondary blocks, and filtered.
        (
            include_bytes!("fixtures/hdf5_ea_chunk_index.h5"),
            "ea_secondary",
            &[4],
            &[&[1001..1002], &[3..9], &[0..1120]],
        ),
        (
            include_bytes!("fixtures/hdf5_ea_filtered.h5"),
            "ea_filtered_direct",
            &[4],
            &[&[597..600], &[100..101]],
        ),
        // An implicit index with ragged edge chunks.
        (
            include_bytes!("fixtures/hdf5_implicit_index.h5"),
            "implicit_partial",
            &[4, 4],
            &[&[4..5, 6..7], &[0..5, 0..1]],
        ),
        // A real NetCDF-4 file: GOES CMI, deflated 12 x 12 chunks.
        (
            include_bytes!("fixtures/goes16_abi_cmip.nc"),
            "CMI",
            &[12, 12],
            &[&[0..1, 0..24], &[13..14, 13..14]],
        ),
    ];
    for &(bytes, name, chunk, regions) in cases {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        let probe = hdf5(&reader);
        let addr = dataset(bytes, probe, name);
        assert!(
            matches!(data_layout(bytes, probe, addr), DataLayout::Chunked(_)),
            "{name} is not chunked, so this proves nothing"
        );
        for &region in regions {
            // A cold reader each time: the file's memo keeps decompressed
            // chunks, and a warm one would rightly leave them out of the plan.
            let cold = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
            let probe = hdf5(&cold);
            let source = Recording::new(bytes);
            read_dataset_region(&source, addr, probe, region)
                .unwrap_or_else(|e| panic!("{name} {region:?}: {e}"));
            let overlapped: u64 = region
                .iter()
                .zip(chunk)
                .map(|(r, &c)| (r.end - 1) / c - r.start / c + 1)
                .product();
            let batches = source.prefetches();
            assert_eq!(batches.len(), 1, "{name} {region:?}");
            assert_eq!(
                batches[0].len() as u64,
                overlapped,
                "{name} {region:?} planned {:?}",
                batches[0]
            );
            let after = source.reads_after_batch();
            assert_eq!(
                after, batches[0],
                "{name} {region:?} read outside its batch"
            );
        }
    }
}

/// A contiguous region reads its runs, the region's bytes and no more, in
/// one batch.
#[test]
fn a_contiguous_region_reads_its_own_bytes() {
    let mut checked = 0usize;
    for bytes in [
        &include_bytes!("fixtures/netcdf4_dimscale.nc")[..],
        &include_bytes!("fixtures/hdf5_v4_chunk_index.h5")[..],
        &include_bytes!("fixtures/rtofs_tripolar_arctic.nc")[..],
        &include_bytes!("fixtures/mirs_swath_n21.nc")[..],
        &include_bytes!("fixtures/oisst_avhrr_v2.nc")[..],
    ] {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        let probe = hdf5(&reader);
        for child in list_all_children(bytes, probe)
            .expect("children")
            .iter()
            .filter(|c| c.kind == ChildKind::Dataset)
        {
            let addr = child.object_header_address;
            if !matches!(
                data_layout(bytes, probe, addr),
                DataLayout::Contiguous {
                    address: Some(_),
                    ..
                }
            ) {
                continue;
            }
            let described =
                fieldglass_netcdf::describe_dataset(bytes, addr, probe).expect("describe");
            let shape = described.dataspace.dims;
            let elem = u64::from(described.datatype.size);
            for region in regions(&shape) {
                let source = Recording::new(bytes);
                if read_dataset_region(&source, addr, probe, &region).is_err() {
                    break; // text
                }
                let batches = source.prefetches();
                assert_eq!(batches.len(), 1, "{} {region:?}", child.name);
                let elements: u64 = region.iter().map(|r| r.end - r.start).product();
                assert_eq!(
                    batches[0].iter().map(|r| r.len).sum::<u64>(),
                    elements * elem,
                    "{} {region:?}",
                    child.name
                );
                assert_eq!(source.reads_after_batch(), batches[0], "{}", child.name);
                checked += 1;
            }
        }
    }
    assert!(checked > 30, "only {checked} contiguous regions checked");
}

/// A filtered chunk read once is not read again while the file's memo holds
/// it (#939): a second row of a chunk already read plans and reads nothing,
/// and its values are unchanged. A chunk too small to be worth holding is read
/// again, which is what keeps a file of a million tiny chunks from filling the
/// memo (#939 review).
#[test]
fn a_held_chunk_is_not_read_again() {
    // `t2m(120, 721, 1440)`, deflated, one 4 MB chunk a time step.
    let bytes: &[u8] = include_bytes!("fixtures/netcdf4_large_sparse.nc");
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
    let probe = hdf5(&reader);
    let addr = dataset(bytes, probe, "t2m");
    let cold = |region: &[Range<u64>]| {
        let fresh = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        read_dataset_region(bytes, addr, hdf5(&fresh), region).expect("reads")
    };

    let first = Recording::new(bytes);
    let row0 = read_dataset_region(&first, addr, probe, &[7..8, 0..1, 0..1440]).expect("row 0");
    assert_eq!(first.prefetches()[0].len(), 1, "row 0 is in one chunk");
    assert_eq!(row0, cold(&[7..8, 0..1, 0..1440]));

    // Row 300 lies in the same chunk: nothing left to fetch.
    let second = Recording::new(bytes);
    let row300 =
        read_dataset_region(&second, addr, probe, &[7..8, 300..301, 0..1440]).expect("row 300");
    assert!(
        second.prefetches().iter().all(Vec::is_empty),
        "{:?}",
        second.prefetches()
    );
    assert!(second.reads_after_batch().is_empty());
    assert_eq!(row300, cold(&[7..8, 300..301, 0..1440]));

    // The other stored step is its own chunk, read once.
    let third = Recording::new(bytes);
    let other =
        read_dataset_region(&third, addr, probe, &[119..120, 5..6, 0..10]).expect("step 119");
    assert_eq!(third.prefetches()[0].len(), 1);
    assert_eq!(other, cold(&[119..120, 5..6, 0..10]));

    // A deflated 4 x 4 chunk of doubles, 128 bytes, is under the memo's floor:
    // the second row of it reads the chunk again.
    let small: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
    let reader = NetcdfReader::from_bytes(small.to_vec()).expect("opens");
    let probe = hdf5(&reader);
    let addr = dataset(small, probe, "compressed");
    read_dataset_region(small, addr, probe, &[0..1, 0..8]).expect("row 0");
    let again = Recording::new(small);
    read_dataset_region(&again, addr, probe, &[1..2, 0..8]).expect("row 1");
    assert_eq!(
        again.prefetches()[0].len(),
        2,
        "small chunks are read again"
    );
}
