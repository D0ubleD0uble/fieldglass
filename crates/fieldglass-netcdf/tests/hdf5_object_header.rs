//! Walks the root-group object header of both bundled HDF5 fixtures (issue #37)
//! and pins the message-type sequence the walker produces.
//!
//! The v1 fixture (`libver='earliest'`) exercises **continuation-following**:
//! its chunk 0 holds a single continuation message that points at a second
//! chunk carrying the symbol-table + global-attribute messages. The v2 fixture
//! (`libver='v110'`) exercises **`OHDR` checksum verification** — the walk only
//! succeeds if every chunk's Jenkins lookup3 checksum matches the bytes libhdf5
//! wrote.
//!
//! Message type codes (HDF5 format spec §IV.A.2): `0x0002` Link Info, `0x000A`
//! Group Info, `0x000C` Attribute, `0x0010` Object Header Continuation,
//! `0x0011` Symbol Table, `0x0015` Attribute Info.

use fieldglass_netcdf::hdf5::{self, object_header};
use fieldglass_netcdf::{NetcdfBacking, NetcdfReader};

const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
const V2_LINKINFO: &[u8] = include_bytes!("fixtures/hdf5_v2_linkinfo.h5");

const MSG_LINK_INFO: u16 = 0x0002;
const MSG_GROUP_INFO: u16 = 0x000A;
const MSG_ATTRIBUTE: u16 = 0x000C;
const MSG_CONTINUATION: u16 = 0x0010;
const MSG_SYMBOL_TABLE: u16 = 0x0011;
const MSG_ATTRIBUTE_INFO: u16 = 0x0015;

fn walk_root(bytes: &[u8]) -> object_header::ObjectHeader {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised HDF5");
    let probe = match reader.backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected HDF5 backing, got {}", other.label()),
    };
    let addr = hdf5::root_group_address(bytes, &probe).expect("root group address");
    object_header::walk(bytes, addr, probe.offset_size, probe.length_size)
        .expect("walk root object header")
}

fn types(oh: &object_header::ObjectHeader) -> Vec<u16> {
    oh.messages.iter().map(|m| m.msg_type).collect()
}

/// v1 root group: chunk 0 is just a continuation; following it yields the
/// symbol-table message plus the three global attributes (scale/title/version).
#[test]
fn v1_root_group_messages() {
    let oh = walk_root(V1_SYMBOLTABLE);
    assert_eq!(oh.version, 1);
    assert_eq!(
        types(&oh),
        vec![
            MSG_CONTINUATION,
            MSG_SYMBOL_TABLE,
            MSG_ATTRIBUTE,
            MSG_ATTRIBUTE,
            MSG_ATTRIBUTE,
        ],
    );
}

/// The v1 continuation message body decodes to the second chunk's address and
/// length — i.e. continuation-following used real on-disk pointers.
#[test]
fn v1_continuation_points_into_the_file() {
    let oh = walk_root(V1_SYMBOLTABLE);
    let cont = &oh.messages[0];
    assert_eq!(cont.msg_type, MSG_CONTINUATION);
    // address (8) + length (8), little-endian, per the 8-byte superblock sizes.
    assert_eq!(cont.body.len(), 16);
    let addr = u64::from_le_bytes(cont.body[0..8].try_into().unwrap());
    let len = u64::from_le_bytes(cont.body[8..16].try_into().unwrap());
    assert!(
        addr as usize + len as usize <= V1_SYMBOLTABLE.len(),
        "continuation chunk lies within the file"
    );
    assert!(len > 0, "continuation chunk is non-empty");
}

/// v2 root group: the modern link-info layout. Reaching these messages means
/// every chunk's lookup3 checksum verified against the libhdf5-written bytes.
#[test]
fn v2_root_group_messages() {
    let oh = walk_root(V2_LINKINFO);
    assert_eq!(oh.version, 2);
    assert_eq!(
        types(&oh),
        vec![
            MSG_ATTRIBUTE_INFO,
            MSG_GROUP_INFO,
            MSG_ATTRIBUTE,
            MSG_ATTRIBUTE,
            MSG_ATTRIBUTE,
            MSG_LINK_INFO,
        ],
    );
}

/// Both fixtures carry exactly the three global attributes the oracle records.
#[test]
fn both_fixtures_expose_three_global_attributes() {
    for bytes in [V1_SYMBOLTABLE, V2_LINKINFO] {
        let oh = walk_root(bytes);
        let attrs = types(&oh).iter().filter(|&&t| t == MSG_ATTRIBUTE).count();
        assert_eq!(attrs, 3, "scale + title + version");
    }
}
