//! Pins the structural facts the HDF5 deep-parse chain builds on, for the two
//! bundled fixtures. Deep parsing (object-header walker #37, group/link
//! traversal #38, dataspace/datatype #39, attributes #40, value decode #121 —
//! under #33) isn't implemented yet; the committed `*.h5.oracle.json` files are
//! the targets. This test guards what's verifiable today: both fixtures are
//! recognised as HDF5 and probe to the expected superblock, and the raw bytes
//! confirm each exercises a *different* on-disk layout — so the chain starts
//! from fixtures that genuinely cover both the legacy and modern paths.

use fieldglass_netcdf::{NetcdfBacking, NetcdfReader};

const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
const V2_LINKINFO: &[u8] = include_bytes!("fixtures/hdf5_v2_linkinfo.h5");

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn probe_superblock(bytes: &[u8]) -> u8 {
    match NetcdfReader::from_bytes(bytes.to_vec())
        .expect("HDF5 file is recognised")
        .backing
    {
        NetcdfBacking::Hdf5(p) => p.superblock_version,
        other => panic!("expected HDF5 backing, got {:?}", other.label()),
    }
}

/// `libver='earliest'`: superblock v0, **v1** object headers, **symbol-table**
/// groups (B-tree v1 + local heap → `SNOD` nodes). No `OHDR` signature. This
/// is the legacy layout the #38 group walker must handle.
#[test]
fn v1_fixture_is_symbol_table_layout() {
    assert_eq!(probe_superblock(V1_SYMBOLTABLE), 0, "v1 fixture superblock");
    assert!(
        contains(V1_SYMBOLTABLE, b"SNOD"),
        "v1 fixture must contain symbol-table nodes (SNOD)"
    );
    assert!(
        !contains(V1_SYMBOLTABLE, b"OHDR"),
        "v1 fixture must use v1 object headers (no OHDR signature)"
    );
}

/// `libver='v110'`: superblock v3, **v2** object headers (`OHDR`),
/// **link-info** groups, and — via a 12-attribute dataset — **dense**
/// attribute storage (fractal heap `FRHP`, #40). No symbol-table nodes.
#[test]
fn v2_fixture_is_link_info_layout_with_dense_attrs() {
    assert_eq!(probe_superblock(V2_LINKINFO), 3, "v2 fixture superblock");
    assert!(
        contains(V2_LINKINFO, b"OHDR"),
        "v2 fixture must use v2 object headers (OHDR)"
    );
    assert!(
        contains(V2_LINKINFO, b"FRHP"),
        "v2 fixture must store dense attributes in a fractal heap (FRHP)"
    );
    assert!(
        !contains(V2_LINKINFO, b"SNOD"),
        "v2 fixture must not use legacy symbol-table nodes"
    );
}

/// Both layouts are flagged not-fully-parsed today (the provider's "deep parse
/// pending" notice). This flips to fully-parsed when #33 lands.
#[test]
fn both_fixtures_report_partial_parse() {
    for (label, bytes) in [("v1", V1_SYMBOLTABLE), ("v2", V2_LINKINFO)] {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).unwrap();
        assert!(
            !reader.backing.is_fully_parsed(),
            "{label} HDF5 fixture should report partial parse until deep parsing lands"
        );
    }
}
