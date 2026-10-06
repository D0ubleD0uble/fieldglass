//! A symbol-table group whose index names one stored block from two places is
//! refused, with the block read once (#901).
//!
//! Fixtures from `tools/build_hdf5_shared_snod_fixture.py`; provenance in
//! `tests/fixtures/NOTICE.md`. Each root group holds eight datasets, one named
//! with 2,000 `L` characters, in one `SNOD`.

use fieldglass_core::testing::Recording;
use fieldglass_netcdf::{NetcdfBacking, NetcdfReader, list_root_children};

/// Two `SNOD` entries naming the long name.
const SHARED_NAME: &[u8] = include_bytes!("fixtures/hdf5_shared_group_name.h5");
/// A group B-tree leaf naming the one `SNOD` twice.
const SHARED_SNOD: &[u8] = include_bytes!("fixtures/hdf5_shared_snod.h5");

fn probe(bytes: &[u8]) -> fieldglass_netcdf::Hdf5Probe {
    match NetcdfReader::from_bytes(bytes.to_vec()).unwrap().backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5, got {}", other.label()),
    }
}

/// Reads long enough to have held the 2,000-byte name.
fn long_reads(source: &Recording<&[u8]>) -> usize {
    source.reads().iter().filter(|r| r.len >= 2000).count()
}

#[test]
fn two_members_sharing_one_name_are_refused() {
    let p = probe(SHARED_NAME);
    let source = Recording::new(SHARED_NAME);
    let err = list_root_children(&source, &p).expect_err("two members, one name");
    assert!(err.to_string().contains("group member name"), "{err}");
    assert!(long_reads(&source) <= 1, "the long name is read once");
}

#[test]
fn a_symbol_table_node_named_twice_is_refused() {
    // It used to be parsed once per reference: 8,192 references to one SNOD
    // whose entries all named one 60 KB name reached 3.85 GB.
    let p = probe(SHARED_SNOD);
    let source = Recording::new(SHARED_SNOD);
    let err = list_root_children(&source, &p).expect_err("one SNOD, two references");
    assert!(err.to_string().contains("symbol-table node"), "{err}");
    assert!(long_reads(&source) <= 1, "its long name is read once");
}
