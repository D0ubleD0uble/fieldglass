//! A region read is the whole-variable read cut to the region (#939).
//!
//! `NetcdfReader::decode_region_raw` reads only what a region needs: a classic
//! variable's runs, a chunked dataset's overlapping chunks, the runs of a
//! contiguous one. The whole-variable decode reads everything and was the only
//! path before, so it is the oracle here: for every variable of every committed
//! fixture, random regions read on their own must equal the same regions cut
//! out of the whole decode — raw, with the fill and missing sentinels masked,
//! and physical, with the CF scale and offset applied — and a plane read must
//! equal the plane `extract_plane` picks out of the whole.
//!
//! The fixtures cover every chunk index the reader walks (v1 and v2 B-trees,
//! fixed and extensible arrays, implicit, single chunk), ragged edge chunks,
//! unstored chunks read as the fill value, every filter, contiguous and compact
//! storage, and classic fixed and record variables in all three versions.

use std::ops::Range;

use fieldglass_core::array::ArraySource;
use fieldglass_netcdf::{NetcdfArrays, NetcdfReader, extract_plane};

/// Every NetCDF and HDF5 file under `tests/fixtures`, embedded so the test runs
/// where files cannot be opened (the `wasm32` target). `every_fixture_is_listed`
/// keeps the list whole.
const FIXTURES: &[(&str, &[u8])] = &[
    (
        "cf_packed_data.nc",
        include_bytes!("fixtures/cf_packed_data.nc"),
    ),
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
    (
        "goes16_abi_cmip.nc",
        include_bytes!("fixtures/goes16_abi_cmip.nc"),
    ),
    (
        "goes_geostationary.nc",
        include_bytes!("fixtures/goes_geostationary.nc"),
    ),
    (
        "hdf5_btree_k_sb1.h5",
        include_bytes!("fixtures/hdf5_btree_k_sb1.h5"),
    ),
    (
        "hdf5_btree_k_sb2.h5",
        include_bytes!("fixtures/hdf5_btree_k_sb2.h5"),
    ),
    (
        "hdf5_btreev2_multilevel.h5",
        include_bytes!("fixtures/hdf5_btreev2_multilevel.h5"),
    ),
    (
        "hdf5_child_indirect.h5",
        include_bytes!("fixtures/hdf5_child_indirect.h5"),
    ),
    (
        "hdf5_conflicting_chunk_records.h5",
        include_bytes!("fixtures/hdf5_conflicting_chunk_records.h5"),
    ),
    (
        "hdf5_conflicting_chunk_records_swapped.h5",
        include_bytes!("fixtures/hdf5_conflicting_chunk_records_swapped.h5"),
    ),
    (
        "hdf5_duplicate_chunk_records.h5",
        include_bytes!("fixtures/hdf5_duplicate_chunk_records.h5"),
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
        "hdf5_fixed_point_precision.h5",
        include_bytes!("fixtures/hdf5_fixed_point_precision.h5"),
    ),
    (
        "hdf5_fletcher32.h5",
        include_bytes!("fixtures/hdf5_fletcher32.h5"),
    ),
    (
        "hdf5_huge_link_name.h5",
        include_bytes!("fixtures/hdf5_huge_link_name.h5"),
    ),
    (
        "hdf5_huge_only_heaps.h5",
        include_bytes!("fixtures/hdf5_huge_only_heaps.h5"),
    ),
    (
        "hdf5_implicit_index.h5",
        include_bytes!("fixtures/hdf5_implicit_index.h5"),
    ),
    (
        "hdf5_local_heap_short.h5",
        include_bytes!("fixtures/hdf5_local_heap_short.h5"),
    ),
    (
        "hdf5_local_heap_unterminated.h5",
        include_bytes!("fixtures/hdf5_local_heap_unterminated.h5"),
    ),
    (
        "hdf5_off_grid_chunk_record.h5",
        include_bytes!("fixtures/hdf5_off_grid_chunk_record.h5"),
    ),
    (
        "hdf5_outside_chunk_record.h5",
        include_bytes!("fixtures/hdf5_outside_chunk_record.h5"),
    ),
    (
        "hdf5_oversized_chunk.h5",
        include_bytes!("fixtures/hdf5_oversized_chunk.h5"),
    ),
    (
        "hdf5_phony_dims.h5",
        include_bytes!("fixtures/hdf5_phony_dims.h5"),
    ),
    (
        "hdf5_shared_chunk_records.h5",
        include_bytes!("fixtures/hdf5_shared_chunk_records.h5"),
    ),
    (
        "hdf5_shared_chunk_records_masks.h5",
        include_bytes!("fixtures/hdf5_shared_chunk_records_masks.h5"),
    ),
    (
        "hdf5_shared_chunk_records_sizes.h5",
        include_bytes!("fixtures/hdf5_shared_chunk_records_sizes.h5"),
    ),
    (
        "hdf5_shared_group_name.h5",
        include_bytes!("fixtures/hdf5_shared_group_name.h5"),
    ),
    (
        "hdf5_shared_huge_attribute.h5",
        include_bytes!("fixtures/hdf5_shared_huge_attribute.h5"),
    ),
    (
        "hdf5_shared_snod.h5",
        include_bytes!("fixtures/hdf5_shared_snod.h5"),
    ),
    (
        "hdf5_sizes_o4_l8.h5",
        include_bytes!("fixtures/hdf5_sizes_o4_l8.h5"),
    ),
    (
        "hdf5_sizes_o8_l4.h5",
        include_bytes!("fixtures/hdf5_sizes_o8_l4.h5"),
    ),
    (
        "hdf5_soft_links_earliest.h5",
        include_bytes!("fixtures/hdf5_soft_links_earliest.h5"),
    ),
    (
        "hdf5_soft_links_latest.h5",
        include_bytes!("fixtures/hdf5_soft_links_latest.h5"),
    ),
    ("hdf5_szip.h5", include_bytes!("fixtures/hdf5_szip.h5")),
    (
        "hdf5_szip_growth.h5",
        include_bytes!("fixtures/hdf5_szip_growth.h5"),
    ),
    (
        "hdf5_szip_hand.h5",
        include_bytes!("fixtures/hdf5_szip_hand.h5"),
    ),
    (
        "hdf5_szip_long_stream.h5",
        include_bytes!("fixtures/hdf5_szip_long_stream.h5"),
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
        "hdf5_v2_linkinfo.h5",
        include_bytes!("fixtures/hdf5_v2_linkinfo.h5"),
    ),
    (
        "hdf5_v4_chunk_index.h5",
        include_bytes!("fixtures/hdf5_v4_chunk_index.h5"),
    ),
    ("hdf5_zstd.h5", include_bytes!("fixtures/hdf5_zstd.h5")),
    (
        "mirs_swath_n21.nc",
        include_bytes!("fixtures/mirs_swath_n21.nc"),
    ),
    (
        "missing_value_classic.nc",
        include_bytes!("fixtures/missing_value_classic.nc"),
    ),
    (
        "missing_value_nc4.nc",
        include_bytes!("fixtures/missing_value_nc4.nc"),
    ),
    (
        "netcdf4_dimscale.nc",
        include_bytes!("fixtures/netcdf4_dimscale.nc"),
    ),
    (
        "netcdf4_grouped.nc",
        include_bytes!("fixtures/netcdf4_grouped.nc"),
    ),
    (
        "netcdf4_hdf5_dummy.nc",
        include_bytes!("fixtures/netcdf4_hdf5_dummy.nc"),
    ),
    (
        "netcdf4_large_sparse.nc",
        include_bytes!("fixtures/netcdf4_large_sparse.nc"),
    ),
    (
        "netcdf4_huge_attributes.nc",
        include_bytes!("fixtures/netcdf4_huge_attributes.nc"),
    ),
    (
        "netcdf4_unsupported_type.nc",
        include_bytes!("fixtures/netcdf4_unsupported_type.nc"),
    ),
    (
        "netcdf_classic_dummy.nc",
        include_bytes!("fixtures/netcdf_classic_dummy.nc"),
    ),
    (
        "oisst_avhrr_v2.nc",
        include_bytes!("fixtures/oisst_avhrr_v2.nc"),
    ),
    (
        "record_mixed_cdf1.nc",
        include_bytes!("fixtures/record_mixed_cdf1.nc"),
    ),
    (
        "record_single_short_cdf1.nc",
        include_bytes!("fixtures/record_single_short_cdf1.nc"),
    ),
    (
        "record_single_short_cdf2.nc",
        include_bytes!("fixtures/record_single_short_cdf2.nc"),
    ),
    (
        "record_single_short_cdf5.nc",
        include_bytes!("fixtures/record_single_short_cdf5.nc"),
    ),
    (
        "record_single_ubyte_cdf5.nc",
        include_bytes!("fixtures/record_single_ubyte_cdf5.nc"),
    ),
    (
        "rtofs_tripolar_arctic.nc",
        include_bytes!("fixtures/rtofs_tripolar_arctic.nc"),
    ),
    ("wrf_lambert.nc", include_bytes!("fixtures/wrf_lambert.nc")),
    ("wrf_latlon.nc", include_bytes!("fixtures/wrf_latlon.nc")),
    (
        "wrf_mercator.nc",
        include_bytes!("fixtures/wrf_mercator.nc"),
    ),
    ("wrf_polar.nc", include_bytes!("fixtures/wrf_polar.nc")),
];

/// The most elements a variable may have for its whole decode to stand as the
/// oracle. `netcdf4_large_sparse.nc` declares 124,588,800, which decoded whole
/// is gigabytes; reading it in parts is what this file is about, and
/// `a_variable_too_large_to_read_whole_reads_in_parts` checks it against the
/// values it was written with instead.
const WHOLE_DECODE_LIMIT: u64 = 1 << 24;

/// Whether the whole decode of `shape` is small enough to be the oracle.
fn affordable(shape: &[u64]) -> bool {
    shape.iter().product::<u64>() <= WHOLE_DECODE_LIMIT
}

/// Random regions drawn per variable, besides the whole one and the corners.
/// A variable of a few elements has few regions to draw, and the three
/// `shared_chunk_records` fixtures each inflate a 64 MB chunk per read, so
/// those draw [`REGIONS_PER_SMALL_VARIABLE`].
const REGIONS_PER_VARIABLE: usize = 24;
/// Random regions drawn for a variable of at most [`SMALL_VARIABLE`] elements.
const REGIONS_PER_SMALL_VARIABLE: usize = 4;
/// The element count at or under which a variable is small.
const SMALL_VARIABLE: u64 = 16;

/// SplitMix64: a fixed seed, so a failure names a region that reproduces.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A non-empty range inside `0..extent`, or `0..0` for an empty axis.
    fn range(&mut self, extent: u64) -> Range<u64> {
        if extent == 0 {
            return 0..0;
        }
        let a = self.next() % extent;
        let b = self.next() % extent;
        a.min(b)..a.max(b) + 1
    }
}

/// `region` cut out of a whole variable of `shape`, one element at a time: the
/// oracle is written without the block copy the reader places chunks with, so
/// a fault in that copy cannot hide on both sides.
fn cut(whole: &[Option<f64>], shape: &[u64], region: &[Range<u64>]) -> Vec<Option<f64>> {
    (0..whole.len() as u64)
        .filter_map(|i| {
            let mut rest = i;
            let inside = (0..shape.len()).rev().all(|d| {
                let at = rest % shape[d];
                rest /= shape[d];
                region[d].contains(&at)
            });
            inside.then(|| whole[i as usize])
        })
        .collect()
}

/// Bit-for-bit, so a `NaN` value compares equal to itself and a `-0.0` does not
/// pass for a `0.0`.
fn bits(values: &[Option<f64>]) -> Vec<Option<u64>> {
    values.iter().map(|v| v.map(f64::to_bits)).collect()
}

/// The regions checked for a variable of `shape`: the whole, each corner cell,
/// and random boxes.
fn regions(shape: &[u64], rng: &mut Rng) -> Vec<Vec<Range<u64>>> {
    let mut out = vec![shape.iter().map(|&n| 0..n).collect::<Vec<_>>()];
    if shape.iter().all(|&n| n > 0) {
        out.push(shape.iter().map(|_| 0..1).collect());
        out.push(shape.iter().map(|&n| n - 1..n).collect());
    }
    let draws = if shape.iter().product::<u64>() <= SMALL_VARIABLE {
        REGIONS_PER_SMALL_VARIABLE
    } else {
        REGIONS_PER_VARIABLE
    };
    for _ in 0..draws {
        out.push(shape.iter().map(|&n| rng.range(n)).collect());
    }
    out
}

#[test]
fn a_region_read_is_the_whole_read_cut_to_the_region() {
    let mut rng = Rng(939);
    let mut compared = 0usize;
    let mut chunked_planes = 0usize;
    for &(name, bytes) in FIXTURES {
        let Ok(reader) = NetcdfReader::from_bytes(bytes.to_vec()) else {
            continue;
        };
        for index in 0.. {
            let Ok(shape) = reader.variable_shape(index) else {
                break;
            };
            if !affordable(&shape) {
                continue;
            }
            let whole = match reader.decode_variable_raw(index) {
                Ok(whole) => whole,
                Err(whole_err) => {
                    // What fails whole fails over the whole region too: the
                    // region path reads the same records and the same bytes.
                    let all: Vec<Range<u64>> = shape.iter().map(|&n| 0..n).collect();
                    assert!(
                        reader.decode_region_raw(index, &all).is_err(),
                        "{name} variable {index}: the whole read failed ({whole_err}) \
                         and the whole region read did not"
                    );
                    continue;
                }
            };
            for region in regions(&shape, &mut rng) {
                let got = reader
                    .decode_region_raw(index, &region)
                    .unwrap_or_else(|e| panic!("{name} variable {index} {region:?}: {e}"));
                assert_eq!(
                    bits(&got),
                    bits(&cut(&whole, &shape, &region)),
                    "{name} variable {index} {shape:?}, region {region:?}"
                );
                compared += 1;
            }
            if shape.len() >= 2 && whole.len() > 1 {
                chunked_planes += 1;
            }
        }
    }
    // Enough to mean something: every fixture contributes, most several
    // variables, each the whole, its corners and the random boxes.
    assert!(compared > 3000, "only {compared} regions compared");
    assert!(
        chunked_planes > 40,
        "only {chunked_planes} multi-axis variables"
    );
}

/// The same through the array seam, in physical units: `read_region_physical`
/// over a region equals the variable's CF unpack of the whole decode, cut.
#[test]
fn a_physical_region_read_is_the_physical_whole_cut_to_the_region() {
    let mut rng = Rng(9390);
    let mut scaled = 0usize;
    for &(name, bytes) in FIXTURES {
        let Ok(reader) = NetcdfReader::from_bytes(bytes.to_vec()) else {
            continue;
        };
        let Ok(arrays) = NetcdfArrays::open(reader) else {
            continue;
        };
        for var in &arrays.view().vars {
            let index = var.decode_index;
            let shape = arrays.reader().variable_shape(index).expect("shape");
            if !affordable(&shape) {
                continue;
            }
            let Ok(physical) = arrays.reader().decode_variable_physical(index) else {
                continue;
            };
            let array = var.name().trim_start_matches('/').to_string();
            if var.unpack(&[Some(1.0)]) != [Some(1.0)] {
                scaled += 1;
            }
            for region in regions(&shape, &mut rng) {
                let got = arrays
                    .read_region_physical(&array, &region)
                    .unwrap_or_else(|e| panic!("{name} `{array}` {region:?}: {e}"));
                assert_eq!(
                    bits(&got),
                    bits(&cut(&physical, &shape, &region)),
                    "{name} `{array}` {shape:?}, region {region:?}"
                );
            }
        }
    }
    // The packed fixtures (GOES, `cf_packed_data`, OISST) put a scale or an
    // offset under test, not only the identity.
    assert!(scaled >= 3, "only {scaled} scaled variables");
}

/// `decode_plane` reads one plane through the region path, and equals the
/// plane `extract_plane` picks out of the whole decode, unpacked — for every
/// pair of axes, either way round, at a random index of every other axis.
#[test]
fn a_plane_read_is_the_plane_of_the_whole() {
    let mut rng = Rng(93_900);
    let mut planes = 0usize;
    for &(name, bytes) in FIXTURES {
        let Ok(reader) = NetcdfReader::from_bytes(bytes.to_vec()) else {
            continue;
        };
        let Ok(view) = reader.view() else {
            continue;
        };
        for var in &view.vars {
            let index = var.decode_index;
            let shape = reader.variable_shape(index).expect("shape");
            if !affordable(&shape) {
                continue;
            }
            let Ok(whole) = reader.decode_variable_raw(index) else {
                continue;
            };
            let rank = shape.len();
            if rank < 2 || shape.contains(&0) {
                continue;
            }
            for y in 0..rank {
                for x in 0..rank {
                    if x == y {
                        continue;
                    }
                    let fixed: Vec<usize> =
                        shape.iter().map(|&n| (rng.next() % n) as usize).collect();
                    let want =
                        var.unpack(&extract_plane(&whole, &shape, y, x, &fixed).expect("extract"));
                    let got = reader
                        .decode_plane(var, y, x, &fixed)
                        .unwrap_or_else(|e| panic!("{name} {} y={y} x={x}: {e}", var.name()));
                    assert_eq!(
                        bits(&got),
                        bits(&want),
                        "{name} {} {shape:?} y={y} x={x} fixed={fixed:?}",
                        var.name()
                    );
                    planes += 1;
                }
            }
        }
    }
    assert!(planes > 100, "only {planes} planes compared");
}

/// A region is checked against the variable before anything is read.
// `vec![0..1]` is a region of one axis, not a mistyped `Vec` of its values.
#[allow(clippy::single_range_in_vec_init)]
#[test]
fn a_region_outside_the_variable_is_refused() {
    let reader = NetcdfReader::from_bytes(
        FIXTURES
            .iter()
            .find(|(n, _)| *n == "hdf5_v2_btree_index.h5")
            .expect("listed")
            .1
            .to_vec(),
    )
    .expect("opens");
    let index = (0..)
        .find(|&i| reader.variable_shape(i).is_ok_and(|s| s.len() == 2))
        .expect("a 2-D dataset");
    let shape = reader.variable_shape(index).unwrap();
    let past = shape[1] + 1;
    for region in [vec![0..1], vec![0..1, 0..past], vec![0..1, 0..1, 0..1]] {
        assert!(
            reader.decode_region_raw(index, &region).is_err(),
            "{region:?} of {shape:?}"
        );
    }
    // An empty range selects nothing and is not an error.
    assert_eq!(
        reader.decode_region_raw(index, &[1..1, 0..1]).unwrap(),
        Vec::<Option<f64>>::new()
    );
}

/// The list above names every NetCDF and HDF5 file in `tests/fixtures`, so a
/// fixture added later is under this test without anyone remembering.
#[cfg(not(target_family = "wasm"))]
#[test]
fn every_fixture_is_listed() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut on_disk: Vec<String> = std::fs::read_dir(dir)
        .expect("fixtures directory")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".nc") || n.ends_with(".h5"))
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = FIXTURES.iter().map(|(n, _)| (*n).to_string()).collect();
    listed.sort();
    assert_eq!(listed, on_disk);
}

/// A variable far past what can be decoded whole reads in parts, each the
/// values it was written with: its two stored planes, and the fill value,
/// masked, wherever no chunk was stored.
#[test]
fn a_variable_too_large_to_read_whole_reads_in_parts() {
    let bytes = FIXTURES
        .iter()
        .find(|(n, _)| *n == "netcdf4_large_sparse.nc")
        .expect("listed")
        .1;
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
    let view = reader.view().expect("view");
    let var = view.vars.iter().find(|v| v.name() == "t2m").expect("t2m");
    let index = var.decode_index;
    let shape = reader.variable_shape(index).expect("shape");
    assert_eq!(shape, [120, 721, 1440]);
    assert!(!affordable(&shape));

    // Row `j` of a stored plane holds the latitude rounded to a whole degree,
    // half to even as numpy rounds.
    let row = |j: u64| (90.0 - j as f64 * 0.25).round_ties_even();
    for (step, offset) in [(7u64, 0.0), (119, 100.0)] {
        let plane = reader
            .decode_plane(var, 1, 2, &[step as usize, 0, 0])
            .expect("a stored plane");
        assert_eq!(plane.len(), 721 * 1440);
        for j in [0u64, 1, 2, 6, 359, 360, 361, 720] {
            for i in [0u64, 1, 719, 1439] {
                assert_eq!(
                    plane[(j * 1440 + i) as usize],
                    Some(row(j) + offset),
                    "step {step} row {j} column {i}"
                );
            }
        }
    }
    // An unstored plane is the fill value, which the CF mask removes.
    let empty = reader
        .decode_plane(var, 1, 2, &[0, 0, 0])
        .expect("an unstored plane");
    assert!(empty.iter().all(Option::is_none));
    // Raw, the fill value is the variable's own `_FillValue` and is masked too.
    let raw = reader
        .decode_region_raw(index, &[3..4, 10..11, 0..4])
        .expect("raw");
    assert_eq!(raw, [None; 4]);
    // A time series through a stored and an unstored plane, along the
    // variable's longest stride.
    let series = reader
        .decode_region_raw(index, &[0..120, 360..361, 5..6])
        .expect("a time series");
    for (t, v) in series.iter().enumerate() {
        let want = match t {
            7 => Some(0.0),
            119 => Some(100.0),
            _ => None,
        };
        assert_eq!(*v, want, "time {t}");
    }
}
