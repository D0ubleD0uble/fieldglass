//! A version-1 B-tree node holds at most 2K entries, with K taken from the
//! file (#920).
//!
//! The superblock states Group Leaf Node K (symbol-table nodes), Group
//! Internal Node K (group B-tree nodes) and, from version 1, Indexed Storage
//! Internal Node K (chunk B-tree nodes). A version-2 superblock states them in
//! a B-tree 'K' Values message in its superblock extension. libhdf5 refuses a
//! node past 2K; so does the reader, and before #920 it took any count up to
//! 65,535.
//!
//! Fixtures from `tools/build_hdf5_btree_k_fixtures.py`: written with
//! `H5Pset_sym_k(32, 8)` and `H5Pset_istore_k(64)`, so every kind of node is
//! fuller than the default K allows. A reader that capped at the defaults, or
//! read K from the wrong bytes, refuses them. The version-1 file is then
//! patched to a K just large enough for its fullest node, and one less, to
//! show each walker's cap sits at exactly 2K.

use fieldglass_netcdf::hdf5::{BtreeK, btree_k, group::list_group_children};
use fieldglass_netcdf::{NetcdfBacking, NetcdfReader, list_root_children};
use serde_json::Value;

const SB1: &[u8] = include_bytes!("fixtures/hdf5_btree_k_sb1.h5");
const SB1_ORACLE: &str = include_str!("fixtures/hdf5_btree_k_sb1.h5.oracle.json");
const SB2: &[u8] = include_bytes!("fixtures/hdf5_btree_k_sb2.h5");
const SB2_ORACLE: &str = include_str!("fixtures/hdf5_btree_k_sb2.h5.oracle.json");

/// The fixtures' stated K, pinned as well as read from the oracle.
const STATED: BtreeK = BtreeK {
    group_leaf: 8,
    group_internal: 32,
    chunk_internal: 64,
};

/// Where a version-1 superblock states each K: Group Leaf Node K, Group
/// Internal Node K, Indexed Storage Internal Node K.
const LEAF_K_AT: usize = 16;
const INTERNAL_K_AT: usize = 18;
const CHUNK_K_AT: usize = 24;

/// Open `bytes`, walk every group and decode `v`, as far as it gets.
fn read(bytes: &[u8]) -> Result<(Vec<String>, Vec<Option<f64>>), String> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).map_err(|e| e.to_string())?;
    let meta = reader.hdf5_metadata().map_err(|e| e.to_string())?;
    let v = meta
        .variables
        .iter()
        .find(|v| v.name == "v")
        .ok_or("no variable v")?;
    let values = reader
        .decode_variable_raw(v.decode_index)
        .map_err(|e| e.to_string())?;
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        return Err("not HDF5".into());
    };
    let mut members: Vec<String> = list_root_children(bytes, probe)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|c| c.name)
        .collect();
    members.sort_unstable();
    Ok((members, values))
}

fn oracle_values(oracle: &Value) -> Vec<Option<f64>> {
    oracle["v"]
        .as_array()
        .unwrap()
        .iter()
        .map(Value::as_f64)
        .collect()
}

fn fullest(oracle: &Value, kind: &str) -> u16 {
    u16::try_from(oracle["fullest_nodes"][kind].as_u64().unwrap()).unwrap()
}

#[test]
fn nodes_past_the_default_k_read_under_the_stated_k() {
    for (bytes, oracle, version) in [(SB1, SB1_ORACLE, 1u8), (SB2, SB2_ORACLE, 2)] {
        let oracle: Value = serde_json::from_str(oracle).unwrap();
        let source = oracle["source"].as_str().unwrap();
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("opens");
        let NetcdfBacking::Hdf5(probe) = &reader.backing else {
            panic!("{source}: expected HDF5");
        };
        assert_eq!(probe.superblock_version, version, "{source}");
        assert_eq!(btree_k(bytes, probe).unwrap(), STATED, "{source}");

        // Every chunk sits in one node of 100, past the default 2 x 32.
        assert_eq!(fullest(&oracle, "chunk_node"), 100, "{source}");
        let (members, values) = read(bytes).unwrap_or_else(|e| panic!("{source}: {e}"));
        assert_eq!(values, oracle_values(&oracle), "{source}");
        let want: Vec<String> = oracle["root_members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(members, want, "{source}");
    }

    // The version-1 file's groups are symbol-table groups, fuller than the
    // default 2 x 4 entries per symbol-table node and 2 x 16 per group node.
    let oracle: Value = serde_json::from_str(SB1_ORACLE).unwrap();
    assert!(fullest(&oracle, "symbol_node") > 8);
    assert!(fullest(&oracle, "group_node") > 32);
    let reader = NetcdfReader::from_bytes(SB1.to_vec()).unwrap();
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        unreachable!()
    };
    let wide = list_root_children(SB1, probe)
        .unwrap()
        .into_iter()
        .find(|c| c.name == "wide")
        .expect("group wide");
    // Its 300 members are soft links, which a listing skips; walking their
    // nodes is the point.
    assert_eq!(oracle["wide_links"], 300);
    assert!(
        list_group_children(SB1, wide.object_header_address, probe)
            .unwrap()
            .is_empty()
    );
}

/// The version-1 fixture with the K at `at` replaced by `k`.
fn with_k(at: usize, k: u16) -> Vec<u8> {
    let mut bytes = SB1.to_vec();
    bytes[at..at + 2].copy_from_slice(&k.to_le_bytes());
    bytes
}

#[test]
fn each_walker_holds_a_node_at_2k_and_refuses_one_past_it() {
    let oracle: Value = serde_json::from_str(SB1_ORACLE).unwrap();
    let cases = [
        (LEAF_K_AT, "symbol_node", "symbol-table node"),
        (INTERNAL_K_AT, "group_node", "group B-tree node"),
        (CHUNK_K_AT, "chunk_node", "chunk B-tree node"),
    ];
    for (at, kind, named) in cases {
        let entries = fullest(&oracle, kind);
        // The smallest K whose 2K holds the fullest node.
        let k = entries.div_ceil(2);
        read(&with_k(at, k)).unwrap_or_else(|e| panic!("{kind} at K = {k}: {e}"));
        let err = read(&with_k(at, k - 1)).expect_err(kind);
        assert!(
            err.contains(&format!(
                "{named} holds {entries} entries, more than 2K = {}",
                2 * (k - 1)
            )),
            "{kind} at K = {}: {err}",
            k - 1
        );
    }
}

#[test]
fn a_zero_k_is_refused() {
    for at in [LEAF_K_AT, INTERNAL_K_AT, CHUNK_K_AT] {
        let err = read(&with_k(at, 0)).expect_err("zero K");
        assert!(err.contains("B-tree K of zero"), "{at}: {err}");
    }
}

#[test]
fn a_version_2_superblock_without_an_extension_takes_the_default_k() {
    // Clear the superblock extension address (after twelve fixed bytes and
    // the base address). The superblock's checksum then no longer matches,
    // which the reader does not check; libhdf5 would refuse the file outright.
    let mut bytes = SB2.to_vec();
    bytes[20..28].fill(0xFF);
    let reader = NetcdfReader::from_bytes(bytes.clone()).unwrap();
    let NetcdfBacking::Hdf5(probe) = &reader.backing else {
        unreachable!()
    };
    assert_eq!(btree_k(&bytes[..], probe).unwrap(), BtreeK::default());
    // So the 100-entry chunk node the stated K allowed is now refused.
    let err = read(&bytes).expect_err("default K");
    assert!(
        err.contains("chunk B-tree node holds 100 entries, more than 2K = 64"),
        "{err}"
    );
}
