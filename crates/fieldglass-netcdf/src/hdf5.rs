//! Minimal HDF5 superblock probe — just enough to confirm a file is HDF5,
//! report the superblock version, and surface a clear "deep parsing not yet
//! implemented" status to the metadata view.
//!
//! Going further into HDF5 internals (B-trees, local heaps, object headers,
//! attribute messages) is intentionally a follow-up task: the surface area is
//! large and a hand-rolled reader is a project of its own. The promise from
//! issue #29 is "parse enough to validate the file and tell the user what's
//! going on" — this module delivers that.
//!
//! Reference: HDF5 file format specification version 3
//! <https://docs.hdfgroup.org/hdf5/develop/_f_m_t3.html>.

use fieldglass_core::FieldglassError;

/// HDF5 signature: `\x89HDF\r\n\x1a\n`.
pub const HDF5_SIGNATURE: [u8; 8] = [0x89, b'H', b'D', b'F', b'\r', b'\n', 0x1a, b'\n'];

/// What we surface from the HDF5 header. Deliberately tiny.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hdf5Probe {
    /// Superblock version byte. Versions 0 and 1 share a layout; versions 2
    /// and 3 introduce a different header. We don't go beyond reporting it.
    pub superblock_version: u8,
    /// Size of file offsets in bytes (typically 8).
    pub offset_size: u8,
    /// Size of file lengths in bytes (typically 8).
    pub length_size: u8,
}

/// HDF5 stores the signature at one of a sequence of offsets — 0, 512, 1024,
/// 2048, … each doubled. This list covers the practical range; files with
/// signatures further out are rare enough we don't search forever.
fn signature_offsets() -> [usize; 7] {
    [0, 512, 1024, 2048, 4096, 8192, 16384]
}

/// Find the file offset at which the HDF5 signature appears, if any.
pub fn find_signature(bytes: &[u8]) -> Option<usize> {
    for &off in signature_offsets().iter() {
        if off + HDF5_SIGNATURE.len() > bytes.len() {
            return None;
        }
        if bytes[off..off + HDF5_SIGNATURE.len()] == HDF5_SIGNATURE {
            return Some(off);
        }
    }
    None
}

/// Probe the HDF5 superblock. Reads only the fields whose offsets are
/// version-independent (signature + version byte + sizes), so this is safe
/// against newer superblock layouts.
pub fn probe(bytes: &[u8]) -> Result<Hdf5Probe, FieldglassError> {
    let off = find_signature(bytes).ok_or(FieldglassError::InvalidMagic)?;

    // Superblock fields after the 8-byte signature, common to all versions:
    //   off + 8  : superblock version (1 byte)
    //
    // For versions 0 and 1 the next two bytes are free-space + root group
    // symbol table version. For version 2 / 3 the layout is:
    //   off + 9  : size of offsets
    //   off + 10 : size of lengths
    //
    // For versions 0 / 1:
    //   off + 13 : size of offsets
    //   off + 14 : size of lengths
    //
    // We report sizes by branching on version.
    let need = off + 16;
    if bytes.len() < need {
        return Err(FieldglassError::Parse(format!(
            "HDF5 superblock truncated: need at least {need} bytes, have {}",
            bytes.len()
        )));
    }
    let version = bytes[off + 8];
    let (offset_size, length_size) = match version {
        0 | 1 => (bytes[off + 13], bytes[off + 14]),
        2 | 3 => (bytes[off + 9], bytes[off + 10]),
        v => {
            return Err(FieldglassError::Parse(format!(
                "unrecognized HDF5 superblock version {v}"
            )));
        }
    };

    Ok(Hdf5Probe {
        superblock_version: version,
        offset_size,
        length_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synth(version: u8, size_offsets_v01: bool) -> Vec<u8> {
        // Build a fake superblock: signature, version, then a block of zeros
        // long enough to cover offset/length size fields at either layout.
        let mut v = Vec::new();
        v.extend_from_slice(&HDF5_SIGNATURE);
        v.push(version);
        v.extend_from_slice(&[0u8; 8]); // padding to land at off+9..=off+16
        // Place offset_size = 8 / length_size = 8 at the version-appropriate
        // slot. signature is 8 bytes, so off=0; field offsets are absolute.
        if size_offsets_v01 {
            v[13] = 8;
            v[14] = 8;
        } else {
            v[9] = 8;
            v[10] = 8;
        }
        v
    }

    #[test]
    fn probe_v0() {
        let bytes = synth(0, true);
        let p = probe(&bytes).unwrap();
        assert_eq!(p.superblock_version, 0);
        assert_eq!(p.offset_size, 8);
        assert_eq!(p.length_size, 8);
    }

    #[test]
    fn probe_v2() {
        let bytes = synth(2, false);
        let p = probe(&bytes).unwrap();
        assert_eq!(p.superblock_version, 2);
        assert_eq!(p.offset_size, 8);
        assert_eq!(p.length_size, 8);
    }

    #[test]
    fn missing_signature_errors() {
        let bytes = vec![0u8; 32];
        let err = probe(&bytes).unwrap_err();
        assert!(matches!(err, FieldglassError::InvalidMagic));
    }

    #[test]
    fn truncated_superblock_errors() {
        let mut bytes = HDF5_SIGNATURE.to_vec();
        bytes.push(0); // version
        // Only 9 bytes total — well short of off+16.
        let err = probe(&bytes).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }
}
