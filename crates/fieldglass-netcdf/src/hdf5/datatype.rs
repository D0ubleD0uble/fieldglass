//! HDF5 datatype message (`0x0003`) decoder (issue #39, under #33). Decodes the
//! element-type metadata of a dataset or attribute, mapping it to the classic
//! [`NcType`] used elsewhere in the crate.
//!
//! Only the datatype classes NetCDF-4 climate data actually uses are decoded:
//! fixed-point integers, IEEE floating point, and fixed-length strings. The
//! compound / enum / array / opaque / reference / variable-length classes are
//! out of scope (#39 non-goals) and rejected with a clear error.
//!
//! The on-disk message begins with a "class and version" byte whose **low
//! nibble is the class** and **high nibble is the version**, followed by a
//! 24-bit class bit field, a 4-byte element size, and class-specific
//! properties (which this layer doesn't need).
//!
//! Reference: HDF5 file format specification version 3, "Datatype Message".

use super::object_header::read_uint_le;
use crate::classic::NcType;
use fieldglass_core::FieldglassError;

/// Byte order of a numeric datatype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    LittleEndian,
    BigEndian,
}

/// The datatype classes this decoder supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatatypeClass {
    FixedPoint,
    FloatingPoint,
    FixedLengthString,
}

/// Decoded element type of a dataset or attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datatype {
    pub class: DatatypeClass,
    /// Element size in bytes.
    pub size: u32,
    /// Whether a fixed-point type is signed (always `false` for float/string).
    pub signed: bool,
    /// Byte order for numeric types; `None` for strings.
    pub byte_order: Option<ByteOrder>,
    /// The equivalent classic NetCDF type.
    pub nc_type: NcType,
}

// HDF5 datatype class codes (low nibble of the class-and-version byte).
const CLASS_FIXED_POINT: u8 = 0;
const CLASS_FLOATING_POINT: u8 = 1;
const CLASS_STRING: u8 = 3;
const CLASS_REFERENCE: u8 = 7;
const CLASS_VARIABLE_LENGTH: u8 = 9;

/// The datatype class in the low nibble of a message's class-and-version byte,
/// without decoding the rest. Lets the dimension-scale layer dispatch on the
/// structural classes (reference, variable-length) that [`decode`] rejects.
pub fn class_of(body: &[u8]) -> Result<u8, FieldglassError> {
    Ok(body
        .first()
        .ok_or_else(|| FieldglassError::Parse("empty datatype message".into()))?
        & 0x0f)
}

/// What an HDF5 **reference** (class 7) datatype points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceKind {
    /// An object reference — the referenced object's header address.
    Object,
    /// A dataset-region reference (object address + a selection). Not decoded.
    DatasetRegion,
}

/// A decoded reference (class 7) datatype. `size` is the address width in bytes
/// (the superblock's offset size); an object reference value is that many bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceDatatype {
    pub kind: ReferenceKind,
    pub size: u32,
}

/// Decode a reference (class 7) datatype message body. The low nibble of the bit
/// field selects object vs. dataset-region reference.
pub fn decode_reference(body: &[u8]) -> Result<ReferenceDatatype, FieldglassError> {
    if body.len() < 8 {
        return Err(FieldglassError::Parse(
            "reference datatype message too small".into(),
        ));
    }
    if body[0] & 0x0f != CLASS_REFERENCE {
        return Err(FieldglassError::Parse(
            "datatype is not a reference (class 7)".into(),
        ));
    }
    let kind = match read_uint_le(body, 1, 3)? & 0x0f {
        0 => ReferenceKind::Object,
        1 => ReferenceKind::DatasetRegion,
        other => {
            return Err(FieldglassError::Parse(format!(
                "unsupported reference type {other}"
            )));
        }
    };
    let size = read_uint_le(body, 4, 4)? as u32;
    Ok(ReferenceDatatype { kind, size })
}

/// The base element of a variable-length datatype, as far as the dimension-scale
/// layer needs it: a reference base is decoded; any other class is reported by
/// its class code so callers can reject it with a clear message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VlenBase {
    Reference(ReferenceDatatype),
    Other(u8),
}

/// A decoded variable-length (class 9) datatype. `is_sequence` distinguishes a
/// vlen sequence (`H5T_VLEN_SEQUENCE`, e.g. `DIMENSION_LIST`) from a vlen string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VlenDatatype {
    pub is_sequence: bool,
    pub base: VlenBase,
}

/// Decode a variable-length (class 9) datatype message body. The low nibble of
/// the bit field is the vlen type (0 = sequence, 1 = string); the base datatype
/// message follows the 8-byte fixed head.
pub fn decode_vlen(body: &[u8]) -> Result<VlenDatatype, FieldglassError> {
    if body.len() < 8 {
        return Err(FieldglassError::Parse(
            "variable-length datatype message too small".into(),
        ));
    }
    if body[0] & 0x0f != CLASS_VARIABLE_LENGTH {
        return Err(FieldglassError::Parse(
            "datatype is not variable-length (class 9)".into(),
        ));
    }
    let vlen_type = read_uint_le(body, 1, 3)? & 0x0f;
    let is_sequence = vlen_type == 0;
    let base_bytes = &body[8..];
    let base = match class_of(base_bytes)? {
        CLASS_REFERENCE => VlenBase::Reference(decode_reference(base_bytes)?),
        other => VlenBase::Other(other),
    };
    Ok(VlenDatatype { is_sequence, base })
}

/// Decode a datatype message body.
pub fn decode(body: &[u8]) -> Result<Datatype, FieldglassError> {
    // class-and-version (1) + class bit field (3) + size (4) = 8-byte fixed head.
    if body.len() < 8 {
        return Err(FieldglassError::Parse("datatype message too small".into()));
    }
    let class = class_of(body)?;
    let bit_field = read_uint_le(body, 1, 3)? as u32;
    let size = read_uint_le(body, 4, 4)? as u32;

    match class {
        CLASS_FIXED_POINT => {
            let byte_order = numeric_byte_order(bit_field);
            let signed = bit_field & 0x08 != 0; // bit 3
            let nc_type = fixed_point_nc_type(size, signed)?;
            Ok(Datatype {
                class: DatatypeClass::FixedPoint,
                size,
                signed,
                byte_order: Some(byte_order),
                nc_type,
            })
        }
        CLASS_FLOATING_POINT => {
            let byte_order = numeric_byte_order(bit_field);
            let nc_type = match size {
                4 => NcType::Float,
                8 => NcType::Double,
                other => {
                    return Err(FieldglassError::Parse(format!(
                        "unsupported floating-point size {other} bytes"
                    )));
                }
            };
            Ok(Datatype {
                class: DatatypeClass::FloatingPoint,
                size,
                signed: false,
                byte_order: Some(byte_order),
                nc_type,
            })
        }
        CLASS_STRING => Ok(Datatype {
            class: DatatypeClass::FixedLengthString,
            size,
            signed: false,
            byte_order: None,
            nc_type: NcType::Char,
        }),
        other => Err(FieldglassError::Parse(format!(
            "unsupported HDF5 datatype class {other}"
        ))),
    }
}

impl Datatype {
    /// Decode the first element from `bytes` (which must be at least
    /// [`Self::size`] long) into `f64`, honouring the datatype's byte order.
    /// Integer types widen (`i64` / `u64` may lose precision past 2^53, as
    /// elsewhere in the `f64` value pipeline). Returns `None` for the string
    /// class or when `bytes` is too short — a string holds text, not a number.
    pub fn read_element_f64(&self, bytes: &[u8]) -> Option<f64> {
        let size = self.size as usize;
        if bytes.len() < size || size == 0 {
            return None;
        }
        let big_endian = self.byte_order == Some(ByteOrder::BigEndian);
        macro_rules! read {
            ($ty:ty) => {{
                const N: usize = std::mem::size_of::<$ty>();
                let mut buf = [0u8; N];
                buf.copy_from_slice(&bytes[..N]);
                if big_endian {
                    <$ty>::from_be_bytes(buf)
                } else {
                    <$ty>::from_le_bytes(buf)
                }
            }};
        }
        Some(match self.nc_type {
            NcType::Byte => (bytes[0] as i8) as f64,
            NcType::UByte => bytes[0] as f64,
            NcType::Char => return None,
            NcType::Short => read!(i16) as f64,
            NcType::UShort => read!(u16) as f64,
            NcType::Int => read!(i32) as f64,
            NcType::UInt => read!(u32) as f64,
            NcType::Float => read!(f32) as f64,
            NcType::Double => read!(f64),
            NcType::Int64 => read!(i64) as f64,
            NcType::UInt64 => read!(u64) as f64,
        })
    }
}

/// Bit 0 of a numeric class bit field selects byte order: 0 = little, 1 = big.
fn numeric_byte_order(bit_field: u32) -> ByteOrder {
    if bit_field & 0x01 != 0 {
        ByteOrder::BigEndian
    } else {
        ByteOrder::LittleEndian
    }
}

/// Map a fixed-point integer to the classic `NcType` by size and signedness.
fn fixed_point_nc_type(size: u32, signed: bool) -> Result<NcType, FieldglassError> {
    Ok(match (size, signed) {
        (1, true) => NcType::Byte,
        (1, false) => NcType::UByte,
        (2, true) => NcType::Short,
        (2, false) => NcType::UShort,
        (4, true) => NcType::Int,
        (4, false) => NcType::UInt,
        (8, true) => NcType::Int64,
        (8, false) => NcType::UInt64,
        (other, _) => {
            return Err(FieldglassError::Parse(format!(
                "unsupported fixed-point size {other} bytes"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a datatype message body: class+version, 3-byte bit field, size.
    fn datatype(class: u8, bit_field: u32, size: u32) -> Vec<u8> {
        let mut v = vec![(1 << 4) | class]; // version 1 in the high nibble
        v.extend_from_slice(&bit_field.to_le_bytes()[..3]);
        v.extend_from_slice(&size.to_le_bytes());
        v
    }

    #[test]
    fn decodes_signed_little_endian_int() {
        let dt = decode(&datatype(CLASS_FIXED_POINT, 0x08, 4)).unwrap();
        assert_eq!(dt.class, DatatypeClass::FixedPoint);
        assert_eq!(dt.size, 4);
        assert!(dt.signed);
        assert_eq!(dt.byte_order, Some(ByteOrder::LittleEndian));
        assert_eq!(dt.nc_type, NcType::Int);
    }

    #[test]
    fn decodes_big_endian_signed_int() {
        // bit 0 set ⇒ big-endian; bit 3 set ⇒ signed.
        let dt = decode(&datatype(CLASS_FIXED_POINT, 0x09, 4)).unwrap();
        assert_eq!(dt.byte_order, Some(ByteOrder::BigEndian));
        assert_eq!(dt.nc_type, NcType::Int);
    }

    #[test]
    fn decodes_unsigned_byte() {
        let dt = decode(&datatype(CLASS_FIXED_POINT, 0x00, 1)).unwrap();
        assert!(!dt.signed);
        assert_eq!(dt.nc_type, NcType::UByte);
    }

    #[test]
    fn decodes_float_and_double() {
        assert_eq!(
            decode(&datatype(CLASS_FLOATING_POINT, 0x00, 4))
                .unwrap()
                .nc_type,
            NcType::Float
        );
        assert_eq!(
            decode(&datatype(CLASS_FLOATING_POINT, 0x00, 8))
                .unwrap()
                .nc_type,
            NcType::Double
        );
    }

    #[test]
    fn decodes_fixed_length_string() {
        let dt = decode(&datatype(CLASS_STRING, 0x00, 8)).unwrap();
        assert_eq!(dt.class, DatatypeClass::FixedLengthString);
        assert_eq!(dt.size, 8);
        assert_eq!(dt.byte_order, None);
        assert_eq!(dt.nc_type, NcType::Char);
    }

    #[test]
    fn reads_element_honouring_byte_order() {
        // Same 32-bit int, little- vs big-endian.
        let le = decode(&datatype(CLASS_FIXED_POINT, 0x08, 4)).unwrap();
        assert_eq!(le.read_element_f64(&[0x2A, 0, 0, 0]), Some(42.0));
        let be = decode(&datatype(CLASS_FIXED_POINT, 0x09, 4)).unwrap();
        assert_eq!(be.read_element_f64(&[0, 0, 0, 0x2A]), Some(42.0));
    }

    #[test]
    fn reads_float_and_rejects_short_or_string() {
        let f = decode(&datatype(CLASS_FLOATING_POINT, 0x00, 4)).unwrap();
        assert_eq!(f.read_element_f64(&1.5f32.to_le_bytes()), Some(1.5));
        // Too few bytes → None rather than panic.
        assert_eq!(f.read_element_f64(&[0, 0]), None);
        // Strings hold text, not a number.
        let s = decode(&datatype(CLASS_STRING, 0x00, 8)).unwrap();
        assert_eq!(s.read_element_f64(b"degC\0\0\0\0"), None);
    }

    #[test]
    fn rejects_unsupported_class() {
        // Class 6 = compound.
        let err = decode(&datatype(6, 0x00, 16)).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn rejects_truncated_body() {
        let err = decode(&[0x10, 0x00, 0x00]).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    /// Build a structural datatype head: class+version byte, 3-byte bit field,
    /// 4-byte size, then any class-specific tail.
    fn structural(class: u8, bit_field: u32, size: u32, tail: &[u8]) -> Vec<u8> {
        let mut v = vec![(1 << 4) | class];
        v.extend_from_slice(&bit_field.to_le_bytes()[..3]);
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(tail);
        v
    }

    #[test]
    fn decodes_object_reference() {
        let r = decode_reference(&structural(CLASS_REFERENCE, 0x00, 8, &[])).unwrap();
        assert_eq!(r.kind, ReferenceKind::Object);
        assert_eq!(r.size, 8);
    }

    #[test]
    fn rejects_region_reference_as_unsupported_kind() {
        let r = decode_reference(&structural(CLASS_REFERENCE, 0x01, 8, &[])).unwrap();
        assert_eq!(r.kind, ReferenceKind::DatasetRegion);
    }

    #[test]
    fn decodes_vlen_sequence_of_object_references() {
        // vlen sequence (type 0) whose base is an 8-byte object reference.
        let base = structural(CLASS_REFERENCE, 0x00, 8, &[]);
        let dt = decode_vlen(&structural(CLASS_VARIABLE_LENGTH, 0x00, 16, &base)).unwrap();
        assert!(dt.is_sequence);
        assert_eq!(
            dt.base,
            VlenBase::Reference(ReferenceDatatype {
                kind: ReferenceKind::Object,
                size: 8,
            })
        );
    }

    #[test]
    fn vlen_string_is_not_a_sequence() {
        // A vlen string (type 1) with a fixed-point base — not what DIMENSION_LIST
        // is, but the decoder should still classify it without error.
        let base = structural(CLASS_FIXED_POINT, 0x08, 1, &[]);
        let dt = decode_vlen(&structural(CLASS_VARIABLE_LENGTH, 0x01, 16, &base)).unwrap();
        assert!(!dt.is_sequence);
        assert_eq!(dt.base, VlenBase::Other(CLASS_FIXED_POINT));
    }

    #[test]
    fn class_of_reads_low_nibble() {
        assert_eq!(class_of(&[(2 << 4) | CLASS_VARIABLE_LENGTH]).unwrap(), 9);
        assert!(class_of(&[]).is_err());
    }
}
