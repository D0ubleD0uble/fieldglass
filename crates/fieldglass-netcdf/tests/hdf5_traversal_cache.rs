//! The HDF5 traversal memo (#414): repeated work is done once, and doing it
//! once does not change any answer.
//!
//! Two things have to hold, and they pull in opposite directions. The memo has
//! to actually eliminate the re-walks — asserted here by counting structure
//! walks with [`Hdf5Probe::traversals`] rather than timing, which would drown
//! the signal in I/O noise. And it has to be a pure memo: a warm reader must
//! decode every variable to exactly the values a cold reader does, byte for
//! byte, including the errors it returns for datasets outside the decoded
//! subset.

use fieldglass_netcdf::{Hdf5Probe, NetcdfBacking, NetcdfReader};

/// Every NetCDF-4 / HDF5 fixture in the crate, across the object-header
/// versions, link layouts, and chunk indexes the reader decodes.
const FIXTURES: &[(&str, &[u8])] = &[
    (
        "hdf5_v1_symboltable.h5",
        include_bytes!("fixtures/hdf5_v1_symboltable.h5"),
    ),
    (
        "hdf5_v2_linkinfo.h5",
        include_bytes!("fixtures/hdf5_v2_linkinfo.h5"),
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
        "hdf5_v4_chunk_index.h5",
        include_bytes!("fixtures/hdf5_v4_chunk_index.h5"),
    ),
    (
        "hdf5_v2_btree_index.h5",
        include_bytes!("fixtures/hdf5_v2_btree_index.h5"),
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
        "netcdf4_dimscale.nc",
        include_bytes!("fixtures/netcdf4_dimscale.nc"),
    ),
    (
        "netcdf4_grouped.nc",
        include_bytes!("fixtures/netcdf4_grouped.nc"),
    ),
    (
        "missing_value_nc4.nc",
        include_bytes!("fixtures/missing_value_nc4.nc"),
    ),
    (
        "goes16_abi_cmip.nc",
        include_bytes!("fixtures/goes16_abi_cmip.nc"),
    ),
];

fn reader(bytes: &[u8]) -> NetcdfReader {
    NetcdfReader::from_bytes(bytes.to_vec()).expect("fixture parses")
}

fn probe_of(reader: &NetcdfReader) -> &Hdf5Probe {
    match &reader.backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected an HDF5 backing, got {}", other.label()),
    }
}

/// How many datasets the file holds, in decode-index order.
fn dataset_count(reader: &NetcdfReader) -> usize {
    (0..)
        .take_while(|&i| {
            reader.variable_shape(i).is_ok() || reader.decode_variable_values(i).is_ok()
        })
        .count()
}

/// Decode results as comparable text, so a mismatch names the fixture, the
/// variable, and whether the two runs disagreed on the value or on the error.
fn decode_all(reader: &NetcdfReader, count: usize) -> Vec<String> {
    (0..count)
        .map(|i| match reader.decode_variable_values(i) {
            Ok(values) => format!("ok:{values:?}"),
            Err(e) => format!("err:{e}"),
        })
        .collect()
}

/// The acceptance criterion: after the first decode, decoding again walks
/// nothing.
///
/// The first call is what warms the memo, and it warms most of it — locating a
/// dataset means classifying every child of every group, which parses every
/// object header in the file on the way past. So a second decode of the same
/// variable, and every shape and metadata call that shares that traversal, is
/// free.
///
/// The one thing the first decode cannot have read is another dataset's chunk
/// index: nothing has pointed at it yet. That is new information, not repeated
/// work, so the first pass over the remaining variables may still walk — and a
/// *second* pass over all of them must not.
#[test]
fn a_warm_reader_walks_nothing() {
    for (name, bytes) in FIXTURES {
        let reader = reader(bytes);
        let probe = probe_of(&reader);
        assert_eq!(
            probe.traversals(),
            0,
            "{name}: opening a file should not walk it (the probe reads only the superblock)"
        );

        // First decode: pays for the whole-file object-header walk.
        let _ = reader.decode_variable_values(0);
        let first = probe.traversals();
        assert!(
            first > 0,
            "{name}: the first decode must actually walk the file, else this test proves nothing"
        );

        let _ = reader.decode_variable_values(0);
        assert_eq!(
            probe.traversals(),
            first,
            "{name}: decoding the same variable twice walked the file twice"
        );

        let count = dataset_count(&reader);
        assert!(count > 0, "{name}: fixture has no datasets");

        let sweep = |_pass: u8| {
            for i in 0..count {
                let _ = reader.decode_variable_values(i);
                let _ = reader.variable_shape(i);
            }
            let _ = reader.hdf5_metadata();
            probe.traversals()
        };
        let after_first_sweep = sweep(1);
        assert_eq!(
            sweep(2),
            after_first_sweep,
            "{name}: a second full pass over all {count} variables, their shapes, and the \
             metadata should walk nothing"
        );
    }
}

/// Chunked datasets specifically: re-decoding must not re-collect the chunk
/// index. The counter covers chunk-index collections as well as object headers,
/// so a regression that re-walked only the chunk B-tree would still be caught.
#[test]
fn re_decoding_a_chunked_dataset_does_not_recollect_its_chunk_index() {
    // One fixture per chunk-index form the reader decodes.
    for name in [
        "hdf5_v1_symboltable.h5",
        "hdf5_v4_chunk_index.h5",
        "hdf5_v2_btree_index.h5",
        "hdf5_ea_chunk_index.h5",
        "hdf5_ea_filtered.h5",
        "hdf5_implicit_index.h5",
    ] {
        let (_, bytes) = FIXTURES.iter().find(|(n, _)| *n == name).expect("listed");
        let reader = reader(bytes);
        let probe = probe_of(&reader);

        let first = reader.decode_variable_values(0);
        let warm = probe.traversals();
        let second = reader.decode_variable_values(0);

        assert_eq!(
            probe.traversals(),
            warm,
            "{name}: the second decode of the same dataset walked the file again"
        );
        assert_eq!(
            format!("{first:?}"),
            format!("{second:?}"),
            "{name}: the memoised decode disagreed with the first"
        );
    }
}

/// The memo must not be observable in any answer. A reader that has already
/// decoded everything, resolved metadata, and read every shape has a fully warm
/// cache; it must still produce exactly what a cold reader produces.
#[test]
fn a_warm_reader_decodes_what_a_cold_one_does() {
    for (name, bytes) in FIXTURES {
        let cold = reader(bytes);
        let count = dataset_count(&cold);
        let expected = decode_all(&cold, count);
        let cold_meta = format!("{:?}", cold.hdf5_metadata());
        let cold_shapes: Vec<String> = (0..count)
            .map(|i| format!("{:?}", cold.variable_shape(i)))
            .collect();

        // Warm every cache line the reader has, in a different order than the
        // cold run took, then re-ask for everything.
        let warm = reader(bytes);
        let _ = warm.hdf5_metadata();
        for i in (0..count).rev() {
            let _ = warm.variable_shape(i);
            let _ = warm.decode_variable_values(i);
        }

        assert_eq!(decode_all(&warm, count), expected, "{name}: decoded values");
        assert_eq!(
            format!("{:?}", warm.hdf5_metadata()),
            cold_meta,
            "{name}: metadata"
        );
        let warm_shapes: Vec<String> = (0..count)
            .map(|i| format!("{:?}", warm.variable_shape(i)))
            .collect();
        assert_eq!(warm_shapes, cold_shapes, "{name}: shapes");
    }
}

/// A probe's memo belongs to the probe, not to the type: cloning one hands back
/// a cold cache rather than another file's remembered offsets. Equality and
/// `Debug` ignore it, so the probe still compares and prints as the three
/// superblock fields it always did.
#[test]
fn the_memo_is_not_part_of_the_probes_identity() {
    let (_, bytes) = FIXTURES[0];
    let reader = reader(bytes);
    let probe = probe_of(&reader);
    let _ = reader.decode_variable_values(0);
    assert!(probe.traversals() > 0);

    let cloned = probe.clone();
    assert_eq!(cloned.traversals(), 0, "a clone starts cold");
    assert_eq!(&cloned, probe, "the memo does not affect equality");
    assert_eq!(
        format!("{cloned:?}"),
        format!("{probe:?}"),
        "the memo does not appear in Debug"
    );
    assert!(
        !format!("{probe:?}").contains("cache"),
        "Debug should print the superblock fields only"
    );
}
