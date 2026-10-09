//! libFuzzer target for the NetCDF parse path.
//!
//! `fieldglass-netcdf` parses attacker-controllable bytes. The classic
//! (CDF-1/2/5) path does offset- and length-driven header walking
//! (`classic::parse_header` — dim_list / gatt_list / var_list), the same bug
//! class fuzzing surfaced in GRIB1. The NetCDF-4 / HDF5 path adds a second deep
//! surface: from the superblock probe, the on-demand walk reads object headers,
//! group and link tables, dense-attribute fractal heaps + B-tree v2 indexes, and
//! the filter pipeline. This target drives both — `from_bytes` for the eager
//! parse, then `hdf5_metadata` for the deep HDF5 walk — asserting the parser
//! never panics, over-reads, or hangs. It then decodes the first few
//! variables' values, which runs every chunk through the chunk indexes and
//! the filter pipeline, szip included.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_netcdf::NetcdfReader;

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    let Ok(reader) = NetcdfReader::from_bytes(data.to_vec()) else {
        return;
    };
    // For an HDF5 backing `from_bytes` reads only the superblock probe; the deep
    // object-model walk (the bounded, fail-safe traversal hardened under #33)
    // runs on demand, so drive it too. Errors are expected on crafted input; the
    // contract is no panic / over-read / hang. (Returns a clean error for the
    // classic backing.)
    let _ = reader.hdf5_metadata();
    // Value decode reads every chunk through the filter pipeline (deflate,
    // shuffle, fletcher32, zstd, szip), which the walk above never touches.
    // The first few variables are enough to reach it from the seeds.
    for index in 0..MAX_DECODED_VARIABLES {
        // The shape is read first so a decode is never started that the run
        // cannot afford; without one there is nothing to bound it by, and an
        // error here is also how the loop finds the last variable.
        let Ok(shape) = reader.variable_shape(index) else {
            break;
        };
        if within_decode_budget(&shape) {
            let _ = reader.decode_variable_raw(index);
        }
        // Two region reads, the path a plane read takes (#939): the first
        // element of every axis but the last and up to `REGION_RUN` along it,
        // then the same at the far corner. Affordable whatever the variable's
        // shape, so a shape too large to decode whole still has its chunk
        // index, filters and offsets fuzzed.
        let last = shape.len().saturating_sub(1);
        for far in [false, true] {
            let region: Vec<std::ops::Range<u64>> = shape
                .iter()
                .enumerate()
                .map(|(axis, &n)| {
                    let len = if axis == last {
                        REGION_RUN.min(n)
                    } else {
                        1.min(n)
                    };
                    if far {
                        n - len..n
                    } else {
                        0..len
                    }
                })
                .collect();
            let _ = reader.decode_region_raw(index, &region);
        }
    }
});

/// Elements a fuzzed region read takes along its last axis.
const REGION_RUN: u64 = 64;

/// Variables decoded per input, so one input with thousands of datasets
/// cannot turn a fuzz iteration into a long decode.
const MAX_DECODED_VARIABLES: usize = 16;

/// The declared element count above which a variable's values are not decoded,
/// unless the reader refuses it outright.
///
/// A whole-variable decode is bounded by `MAX_VARIABLE_BYTES` (2 GiB held at
/// once), which is sized for a real reanalysis variable, not for libFuzzer's
/// 2 GB RSS limit: the decode returns one `Option<f64>` per element (16 bytes)
/// and holds the stored bytes alongside while it assembles them. A chunked
/// dataset need not store its chunks (one it omits reads as the fill value),
/// so a small file can ask for most of that budget. Decoding a variable just
/// inside it is the reader working as designed, and libFuzzer reporting it as
/// out-of-memory would end the run at the first such input.
///
/// Nothing the decode does needs a large shape: the chunk indexes, the filter
/// pipeline and the fill path are all reached at small sizes. A shape past
/// `MAX_VARIABLE_ELEMENTS`, which the reader refuses whatever its type, is
/// still decoded, so the refusal itself stays fuzzed; it fails before the
/// values are allocated. `corpus/parse/oom_large_fill_dataset.h5`, a 13 KB
/// file asking for 2.9 GB (#847), is one.
const MAX_FUZZ_DECODE_ELEMENTS: u64 = 1 << 22;

fn within_decode_budget(shape: &[u64]) -> bool {
    match shape.iter().try_fold(1u64, |acc, &d| acc.checked_mul(d)) {
        Some(total) => {
            total <= MAX_FUZZ_DECODE_ELEMENTS
                || total > fieldglass_netcdf::MAX_VARIABLE_ELEMENTS as u64
        }
        // Overflows: refused by the reader before it allocates the values.
        None => true,
    }
}
