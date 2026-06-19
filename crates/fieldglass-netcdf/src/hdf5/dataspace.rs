//! HDF5 dataspace message (`0x0001`) decoder (issue #39, under #33). Decodes a
//! dataset's shape — rank, current dimensions, and maximum dimensions — for the
//! scalar and simple classes that NetCDF-4 uses.
//!
//! Two message versions exist and both bundled fixtures exercise one each:
//!
//! * **Version 1** — `version(1) rank(1) flags(1)` then 5 reserved bytes
//!   (8-byte header); scalar is inferred from `rank == 0`.
//! * **Version 2** — `version(1) rank(1) flags(1) type(1)` (4-byte header) with
//!   an explicit class byte (0 = scalar, 1 = simple, 2 = null).
//!
//! After the header come `rank` current dimensions, then — if the "max dims"
//! flag is set — `rank` maximum dimensions, each a length-sized integer. A
//! maximum dimension of all-ones is `H5S_UNLIMITED`, surfaced as `None`.
//!
//! Reference: HDF5 file format specification version 3, "Dataspace Message".

use super::object_header::read_uint_le;
use fieldglass_core::FieldglassError;

/// Decoded dataset shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dataspace {
    pub is_scalar: bool,
    /// Current size of each dimension (empty for scalar).
    pub dims: Vec<u64>,
    /// Maximum size of each dimension; `None` is `H5S_UNLIMITED`.
    pub max_dims: Vec<Option<u64>>,
}

// Dataspace class codes (version-2 "type" byte).
const CLASS_SCALAR: u8 = 0;
const CLASS_SIMPLE: u8 = 1;

/// Decode a dataspace message body. `length_size` is the superblock's "size of
/// lengths", the width of each dimension field.
pub fn decode(body: &[u8], length_size: u8) -> Result<Dataspace, FieldglassError> {
    let version = *body
        .first()
        .ok_or_else(|| FieldglassError::Parse("empty dataspace message".into()))?;
    let rank = *body
        .get(1)
        .ok_or_else(|| FieldglassError::Parse("truncated dataspace message".into()))?
        as usize;
    let flags = *body
        .get(2)
        .ok_or_else(|| FieldglassError::Parse("truncated dataspace message".into()))?;

    let (header_len, is_scalar) = match version {
        1 => (8, rank == 0),
        2 => {
            let class = *body
                .get(3)
                .ok_or_else(|| FieldglassError::Parse("truncated dataspace message".into()))?;
            match class {
                CLASS_SCALAR => (4, true),
                CLASS_SIMPLE => (4, false),
                other => {
                    return Err(FieldglassError::Parse(format!(
                        "unsupported dataspace class {other}"
                    )));
                }
            }
        }
        other => {
            return Err(FieldglassError::Parse(format!(
                "unsupported dataspace message version {other}"
            )));
        }
    };

    let lsize = length_size as usize;
    let mut pos = header_len;
    let mut dims = Vec::with_capacity(rank);
    for _ in 0..rank {
        dims.push(read_uint_le(body, pos, lsize)?);
        pos += lsize;
    }

    let mut max_dims = Vec::new();
    if flags & 0x01 != 0 {
        for _ in 0..rank {
            let raw = read_uint_le(body, pos, lsize)?;
            max_dims.push(if is_unlimited(raw, length_size) {
                None
            } else {
                Some(raw)
            });
            pos += lsize;
        }
    }

    Ok(Dataspace {
        is_scalar,
        dims,
        max_dims,
    })
}

/// `H5S_UNLIMITED` is encoded as all-ones in a length-sized field.
fn is_unlimited(value: u64, length_size: u8) -> bool {
    let l = length_size as usize;
    if l >= 8 {
        value == u64::MAX
    } else {
        value == (1u64 << (8 * l)) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims_field(dims: &[u64]) -> Vec<u8> {
        dims.iter().flat_map(|d| d.to_le_bytes()).collect()
    }

    #[test]
    fn decodes_v1_simple_with_max_dims() {
        // version 1, rank 2, flags = max-dims present, 5 reserved.
        let mut body = vec![1u8, 2, 0x01, 0, 0, 0, 0, 0];
        body.extend(dims_field(&[3, 4]));
        body.extend(dims_field(&[3, 4]));
        let ds = decode(&body, 8).unwrap();
        assert!(!ds.is_scalar);
        assert_eq!(ds.dims, vec![3, 4]);
        assert_eq!(ds.max_dims, vec![Some(3), Some(4)]);
    }

    #[test]
    fn decodes_v1_scalar() {
        let body = vec![1u8, 0, 0x00, 0, 0, 0, 0, 0];
        let ds = decode(&body, 8).unwrap();
        assert!(ds.is_scalar);
        assert!(ds.dims.is_empty());
    }

    #[test]
    fn decodes_v1_unlimited_max_dim() {
        let mut body = vec![1u8, 1, 0x01, 0, 0, 0, 0, 0];
        body.extend(dims_field(&[4]));
        body.extend(dims_field(&[u64::MAX]));
        let ds = decode(&body, 8).unwrap();
        assert_eq!(ds.dims, vec![4]);
        assert_eq!(ds.max_dims, vec![None]);
    }

    #[test]
    fn decodes_v2_simple() {
        // version 2, rank 2, flags = max dims, type = simple.
        let mut body = vec![2u8, 2, 0x01, CLASS_SIMPLE];
        body.extend(dims_field(&[10, 10]));
        body.extend(dims_field(&[10, 10]));
        let ds = decode(&body, 8).unwrap();
        assert!(!ds.is_scalar);
        assert_eq!(ds.dims, vec![10, 10]);
    }

    #[test]
    fn decodes_v2_scalar() {
        let body = vec![2u8, 0, 0x00, CLASS_SCALAR];
        let ds = decode(&body, 8).unwrap();
        assert!(ds.is_scalar);
        assert!(ds.dims.is_empty());
    }

    #[test]
    fn rejects_unknown_version() {
        let err = decode(&[9u8, 0, 0, 0, 0, 0, 0, 0], 8).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn rejects_truncated_dims() {
        // Claims rank 2 but supplies no dimension bytes.
        let body = vec![1u8, 2, 0x00, 0, 0, 0, 0, 0];
        let err = decode(&body, 8).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }
}
