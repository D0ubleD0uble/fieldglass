//! HDF5 datatype message (`0x0003`) decoder (issue #39, under #33). Decodes the
//! element-type metadata of a dataset or attribute, mapping it to the classic
//! [`NcType`] used elsewhere in the crate.
//!
//! Only the datatype classes NetCDF-4 climate data actually uses are decoded:
//! fixed-point integers, IEEE floating point, and fixed-length strings. The
//! compound / enum / array / opaque / reference / variable-length classes are
//! out of scope (#39 non-goals) and rejected with
//! [`FieldglassError::UnsupportedSection`] naming the class — not `Parse`,
//! because a caller acts on the two differently: the dimension-scale layer
//! skips a dataset whose type it cannot decode and reports it, where a parse
//! failure still fails the file (#550).
//!
//! The on-disk message begins with a "class and version" byte whose **low
//! nibble is the class** and **high nibble is the version**, followed by a
//! 24-bit class bit field, a 4-byte element size, and class-specific
//! properties.
//!
//! A numeric element need not fill its container. A fixed-point type carries a
//! bit offset and a bit precision: the value is the `precision` bits starting
//! `offset` bits above the least significant bit, and the bits either side are
//! padding (#795). [`Datatype::element_bits`] is the one place those are
//! applied, and every numeric read (dataset values, the fill value they fall
//! back to, attribute values) goes through it. A floating-point type describes
//! its sign, exponent and mantissa fields the same way; only the IEEE 754
//! binary32 / binary64 layouts are decoded, and any other layout is refused
//! with [`FieldglassError::UnsupportedSection`] rather than read as IEEE.
//!
//! Reference: HDF5 file format specification version 3, "Datatype Message"
//! (IV.A.2.d), the Fixed-Point and Floating-Point bit field and property
//! tables.

use super::object_header::read_uint_le;
use crate::classic::NcType;
use fieldglass_core::FieldglassError;

/// Byte order of a numeric datatype.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// Least significant byte first.
    LittleEndian,
    /// Most significant byte first.
    BigEndian,
}

/// The datatype classes this decoder supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatatypeClass {
    /// Class 0 — integer.
    FixedPoint,
    /// Class 1 — IEEE float.
    FloatingPoint,
    /// Class 3 — fixed-length string.
    FixedLengthString,
}

/// Decoded element type of a dataset or attribute.
///
/// `#[non_exhaustive]`: the datatype message has more properties than these
/// (#795 added the bit offset and precision), so a caller outside this crate
/// gets one from [`decode`] rather than building it field by field.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Datatype {
    /// Which of the three supported classes this is.
    pub class: DatatypeClass,
    /// Element size in bytes.
    pub size: u32,
    /// Whether a fixed-point type is signed (always `false` for float/string).
    pub signed: bool,
    /// Byte order for numeric types; `None` for strings.
    pub byte_order: Option<ByteOrder>,
    /// The equivalent classic NetCDF type.
    pub nc_type: NcType,
    /// Bit offset of the value within the element: the number of padding bits
    /// below it. Read from a fixed-point message; `0` for floating-point (only
    /// full-width IEEE layouts decode) and for strings.
    pub bit_offset: u16,
    /// Number of significant bits in the value. Read from a fixed-point
    /// message; the full container width for floating-point; `0` for strings.
    /// A signed value's sign bit is the top one of these, not of the container.
    pub bit_precision: u16,
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
    /// What the reference points at.
    pub kind: ReferenceKind,
    /// The address width in bytes — the size of one reference value.
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
    /// A reference base, decoded — what `DIMENSION_LIST` carries.
    Reference(ReferenceDatatype),
    /// Any other base, reported by its class code so the caller can reject it
    /// by name rather than as "unsupported".
    Other(u8),
}

/// A decoded variable-length (class 9) datatype. `is_sequence` distinguishes a
/// vlen sequence (`H5T_VLEN_SEQUENCE`, e.g. `DIMENSION_LIST`) from a vlen string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VlenDatatype {
    /// `true` for `H5T_VLEN_SEQUENCE`, `false` for a vlen string.
    pub is_sequence: bool,
    /// The sequence's element type.
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
            let (bit_offset, bit_precision) = fixed_point_bits(body, size)?;
            Ok(Datatype {
                class: DatatypeClass::FixedPoint,
                size,
                signed,
                byte_order: Some(byte_order),
                nc_type,
                bit_offset,
                bit_precision,
            })
        }
        CLASS_FLOATING_POINT => {
            let byte_order = numeric_byte_order(bit_field);
            let nc_type = match size {
                4 => NcType::Float,
                8 => NcType::Double,
                other => {
                    return Err(FieldglassError::UnsupportedSection(format!(
                        "HDF5 floating-point datatype of {other} bytes"
                    )));
                }
            };
            check_ieee_layout(body, bit_field, size)?;
            Ok(Datatype {
                class: DatatypeClass::FloatingPoint,
                size,
                signed: false,
                byte_order: Some(byte_order),
                nc_type,
                bit_offset: 0,
                // `size` is 4 or 8 here, so this cannot truncate.
                bit_precision: (size * 8) as u16,
            })
        }
        CLASS_STRING => Ok(Datatype {
            class: DatatypeClass::FixedLengthString,
            size,
            signed: false,
            byte_order: None,
            nc_type: NcType::Char,
            bit_offset: 0,
            bit_precision: 0,
        }),
        other => Err(unsupported_class(other)),
    }
}

/// The fixed-point properties: a 2-byte bit offset then a 2-byte bit precision,
/// right after the 8-byte head. The value occupies bits `offset ..
/// offset + precision` of the element, so both must fit inside it and the
/// precision cannot be zero; a message that says otherwise is malformed, and
/// libhdf5 rejects the same cases when it reads one.
///
/// Reference: HDF5 file format specification version 3, IV.A.2.d, "Fixed-Point
/// Property Description".
fn fixed_point_bits(body: &[u8], size: u32) -> Result<(u16, u16), FieldglassError> {
    if body.len() < 12 {
        return Err(FieldglassError::Parse(
            "fixed-point datatype message has no bit offset or precision".into(),
        ));
    }
    let offset = read_uint_le(body, 8, 2)? as u16;
    let precision = read_uint_le(body, 10, 2)? as u16;
    let width = u64::from(size) * 8;
    if precision == 0 || u64::from(offset) + u64::from(precision) > width {
        return Err(FieldglassError::Parse(format!(
            "fixed-point datatype: {precision} bits at offset {offset} do not fit a \
             {width}-bit element"
        )));
    }
    Ok((offset, precision))
}

/// Refuse a floating-point layout other than IEEE 754 binary32 / binary64.
///
/// The message spells out the whole layout: byte order (bit 0, with bit 6 for
/// VAX order), mantissa normalization (bits 4-5) and sign position (bits 8-15)
/// in the bit field, then as properties the bit offset, bit precision,
/// exponent location and size, mantissa location and size, and exponent bias.
/// An element is read as an IEEE `f32` / `f64` bit pattern, which is right only
/// when every one of those matches IEEE. A type that differs in any of them (a
/// 24-bit float in a 32-bit container, another bias, VAX order) holds a number
/// this build would misread, so it is reported as not implemented instead. The
/// padding flags (bits 1-3) do not matter here: an IEEE layout fills its
/// container and has no internal gap.
///
/// Reference: HDF5 file format specification version 3, IV.A.2.d,
/// "Floating-Point Bit Field Description" and "Floating-Point Property
/// Description".
fn check_ieee_layout(body: &[u8], bit_field: u32, size: u32) -> Result<(), FieldglassError> {
    if body.len() < 20 {
        return Err(FieldglassError::Parse(
            "floating-point datatype message has no layout properties".into(),
        ));
    }
    let (sign, e_loc, e_size, m_size, bias) = match size {
        4 => (31, 23, 8, 23, 127),
        _ => (63, 52, 11, 52, 1023),
    };
    let found = FloatLayout {
        // Bit 6 (VAX order) is defined only from datatype message version 3;
        // refusing it on an older message too is deliberately conservative.
        vax_order: bit_field & 0x40 != 0,
        normalization: (bit_field >> 4) & 0x03,
        sign: (bit_field >> 8) & 0xff,
        offset: read_uint_le(body, 8, 2)? as u32,
        precision: read_uint_le(body, 10, 2)? as u32,
        e_loc: u32::from(body[12]),
        e_size: u32::from(body[13]),
        m_loc: u32::from(body[14]),
        m_size: u32::from(body[15]),
        bias: read_uint_le(body, 16, 4)? as u32,
    };
    let ieee = FloatLayout {
        vax_order: false,
        normalization: 2, // implied leading 1
        sign,
        offset: 0,
        precision: size * 8,
        e_loc,
        e_size,
        m_loc: 0,
        m_size,
        bias,
    };
    if found == ieee {
        Ok(())
    } else {
        Err(FieldglassError::UnsupportedSection(format!(
            "HDF5 floating-point datatype with a non-IEEE layout: {found:?} \
             (IEEE {}-bit is {ieee:?})",
            size * 8
        )))
    }
}

/// The parts of a floating-point datatype message that decide how its bits
/// are read, gathered so a non-IEEE one can be compared and reported whole.
#[derive(Debug, PartialEq, Eq)]
struct FloatLayout {
    vax_order: bool,
    normalization: u32,
    sign: u32,
    offset: u32,
    precision: u32,
    e_loc: u32,
    e_size: u32,
    m_loc: u32,
    m_size: u32,
    bias: u32,
}

/// The spelling of an HDF5 datatype class code, for an error a user reads.
/// The three [`decode`] handles are named too, so this reads as the class
/// table from the format specification rather than as a list of failures.
///
/// Reference: HDF5 file format specification version 3, "Datatype Message",
/// Table: Datatype class.
fn class_name(class: u8) -> &'static str {
    match class {
        CLASS_FIXED_POINT => "fixed-point",
        CLASS_FLOATING_POINT => "floating-point",
        2 => "time",
        CLASS_STRING => "string",
        4 => "bit field",
        5 => "opaque",
        6 => "compound",
        CLASS_REFERENCE => "reference",
        8 => "enumeration",
        CLASS_VARIABLE_LENGTH => "variable-length",
        10 => "array",
        _ => "reserved",
    }
}

/// A datatype class outside the decoded subset. [`FieldglassError::UnsupportedSection`],
/// not [`FieldglassError::Parse`]: the message parsed fine and says something
/// this build does not implement, and the two are acted on differently — the
/// dimension-scale layer skips a dataset it cannot describe and reports it
/// (#550), where a genuine parse failure still fails the file.
fn unsupported_class(class: u8) -> FieldglassError {
    FieldglassError::UnsupportedSection(format!(
        "HDF5 datatype class {class} ({})",
        class_name(class)
    ))
}

impl Datatype {
    /// The number of bytes one element is read from: the width of `nc_type`,
    /// or `None` for text.
    ///
    /// It is `nc_type` that decides how many bytes to take: [`Self::size`] is
    /// not consulted. `decode` derives `nc_type` from `size`, so the two agree
    /// for any datatype the parser produces — but a length check against
    /// `size` would not have bounded a read sized by `nc_type` if they ever
    /// came apart.
    fn read_width(&self) -> Option<usize> {
        Some(match self.nc_type {
            NcType::Char => return None,
            NcType::Byte | NcType::UByte => 1,
            NcType::Short | NcType::UShort => 2,
            NcType::Int | NcType::UInt | NcType::Float => 4,
            NcType::Double | NcType::Int64 | NcType::UInt64 => 8,
        })
    }

    /// The value bits of the first element of `bytes`, widened to 64 bits.
    /// This is the one place an HDF5 numeric element is read, so every caller
    /// (dataset values, the fill value, attribute values) gets the same answer.
    ///
    /// The element is assembled in its byte order. For a fixed-point type the
    /// value is then the [`Self::bit_precision`] bits starting
    /// [`Self::bit_offset`] bits up: shifted down, masked (which drops the
    /// padding on both sides whether the padding flags say it holds zeros or
    /// ones), and, when signed, sign-extended from the precision's top bit.
    /// The result is the value as a two's-complement `i64` (signed) or a `u64`
    /// (unsigned). For a floating-point type it is the IEEE bit pattern,
    /// unchanged; [`decode`] refuses any float layout for which that is wrong.
    ///
    /// `None` for the string class, when `bytes` is shorter than an element,
    /// or for a fixed-point struct whose offset and precision do not describe
    /// bits inside a 64-bit value (the parser never produces one).
    ///
    /// Reference: HDF5 file format specification version 3, IV.A.2.d,
    /// "Fixed-Point Bit Field Description" (byte order, padding, signed) and
    /// "Fixed-Point Property Description" (bit offset, bit precision).
    pub fn element_bits(&self, bytes: &[u8]) -> Option<u64> {
        let width = self.read_width()?;
        let src = bytes.get(..width)?;
        let mut buf = [0u8; 8];
        let raw = if self.byte_order == Some(ByteOrder::BigEndian) {
            buf[8 - width..].copy_from_slice(src);
            u64::from_be_bytes(buf)
        } else {
            buf[..width].copy_from_slice(src);
            u64::from_le_bytes(buf)
        };
        if self.class != DatatypeClass::FixedPoint {
            return Some(raw);
        }
        let precision = u32::from(self.bit_precision);
        if precision == 0 {
            return None;
        }
        let shifted = raw.checked_shr(u32::from(self.bit_offset))?;
        if precision >= 64 {
            return Some(shifted);
        }
        let mask = (1u64 << precision) - 1;
        let value = shifted & mask;
        let negative = self.signed && (value >> (precision - 1)) & 1 == 1;
        Some(if negative { value | !mask } else { value })
    }

    /// Whether this is a fixed-point type whose value does not fill its
    /// container: a non-zero bit offset, or a precision short of `size` bytes.
    /// Only then does [`Self::element_bits`] do more than a plain read.
    pub(crate) fn is_packed(&self) -> bool {
        self.class == DatatypeClass::FixedPoint
            && (self.bit_offset != 0 || u64::from(self.bit_precision) != u64::from(self.size) * 8)
    }

    /// Decode the first element from `bytes` into `f64`, honouring the byte
    /// order and a fixed-point type's bit offset and precision. Integer types
    /// widen (`i64` / `u64` may lose precision past 2^53, as elsewhere in the
    /// `f64` value pipeline). Returns `None` for the string class or when
    /// `bytes` is too short — a string holds text, not a number.
    ///
    /// A packed fixed-point type goes through [`Self::element_bits`]. Every
    /// other type (floats, and integers that fill their container, which is
    /// nearly every real file) takes a plain typed read instead: this runs once
    /// per element of a variable, and for those types the two agree bit for
    /// bit (`full_width_fast_path_agrees_with_element_bits` pins that). The
    /// dataset decode in this crate makes that choice once per variable
    /// (crate-private `is_packed`) and calls the matching reader directly.
    pub fn read_element_f64(&self, bytes: &[u8]) -> Option<f64> {
        if self.is_packed() {
            self.read_packed_f64(bytes)
        } else {
            self.read_full_width_f64(bytes)
        }
    }

    /// [`Self::read_element_f64`] for a packed fixed-point type: the value
    /// [`Self::element_bits`] extracts, as a signed or unsigned integer.
    pub(crate) fn read_packed_f64(&self, bytes: &[u8]) -> Option<f64> {
        let bits = self.element_bits(bytes)?;
        Some(if self.signed {
            bits as i64 as f64
        } else {
            bits as f64
        })
    }

    /// [`Self::read_element_f64`] for every type that is not packed: a plain
    /// typed read of the container in its byte order.
    pub(crate) fn read_full_width_f64(&self, bytes: &[u8]) -> Option<f64> {
        let big_endian = self.byte_order == Some(ByteOrder::BigEndian);
        macro_rules! read {
            ($ty:ty) => {{
                let buf = *bytes.first_chunk()?;
                if big_endian {
                    <$ty>::from_be_bytes(buf)
                } else {
                    <$ty>::from_le_bytes(buf)
                }
            }};
        }
        Some(match self.nc_type {
            NcType::Byte => (*bytes.first()? as i8) as f64,
            NcType::UByte => *bytes.first()? as f64,
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

    /// Rewrite `raw`, a run of elements as this datatype stores them, as
    /// big-endian elements of `nc_type`'s width that hold just the value: the
    /// form the classic decoders read. Each element goes through
    /// [`Self::element_bits`], so a fixed-point value is shifted, masked and
    /// sign-extended to fill its container. `None` for the string class or
    /// when `raw` ends in a partial element.
    pub(crate) fn to_classic_bytes(&self, raw: &[u8]) -> Option<Vec<u8>> {
        let width = self.read_width()?;
        if !raw.len().is_multiple_of(width) {
            return None;
        }
        let mut out = Vec::with_capacity(raw.len());
        for element in raw.chunks_exact(width) {
            let bits = self.element_bits(element)?;
            out.extend_from_slice(&bits.to_be_bytes()[8 - width..]);
        }
        Some(out)
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
            return Err(FieldglassError::UnsupportedSection(format!(
                "HDF5 fixed-point datatype of {other} bytes"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a datatype message body: class+version, 3-byte bit field, size,
    /// then the properties libhdf5 writes for a full-width type of that class
    /// (offset 0, precision = the container; the IEEE layout for a float).
    fn datatype(class: u8, bit_field: u32, size: u32) -> Vec<u8> {
        let bit_field = match (class, size) {
            // A float's sign position and normalization live in the bit field.
            (CLASS_FLOATING_POINT, 4) => bit_field | (31 << 8) | 0x20,
            (CLASS_FLOATING_POINT, 8) => bit_field | (63 << 8) | 0x20,
            _ => bit_field,
        };
        let mut v = vec![(1 << 4) | class]; // version 1 in the high nibble
        v.extend_from_slice(&bit_field.to_le_bytes()[..3]);
        v.extend_from_slice(&size.to_le_bytes());
        let precision = (size * 8) as u16;
        match class {
            CLASS_FIXED_POINT => {
                v.extend_from_slice(&0u16.to_le_bytes());
                v.extend_from_slice(&precision.to_le_bytes());
            }
            CLASS_FLOATING_POINT => {
                let (e_loc, e_size, m_size, bias): (u8, u8, u8, u32) = match size {
                    4 => (23, 8, 23, 127),
                    _ => (52, 11, 52, 1023),
                };
                v.extend_from_slice(&0u16.to_le_bytes());
                v.extend_from_slice(&precision.to_le_bytes());
                v.extend_from_slice(&[e_loc, e_size, 0, m_size]);
                v.extend_from_slice(&bias.to_le_bytes());
            }
            _ => {}
        }
        v
    }

    /// A fixed-point message with an explicit bit offset and precision.
    fn fixed_point(bit_field: u32, size: u32, offset: u16, precision: u16) -> Vec<u8> {
        let mut v = datatype(CLASS_FIXED_POINT, bit_field, size);
        v[8..10].copy_from_slice(&offset.to_le_bytes());
        v[10..12].copy_from_slice(&precision.to_le_bytes());
        v
    }

    /// The motivating case (#795): a signed 16-bit value in a 32-bit
    /// little-endian container, zero-padded above, as libhdf5 writes -20.
    #[test]
    fn signed_precision_is_sign_extended_from_its_top_bit() {
        let dt = decode(&fixed_point(0x08, 4, 0, 16)).unwrap();
        assert_eq!((dt.bit_offset, dt.bit_precision), (0, 16));
        assert_eq!(dt.read_element_f64(&[0xec, 0xff, 0, 0]), Some(-20.0));
        assert_eq!(dt.read_element_f64(&[0xff, 0xff, 0, 0]), Some(-1.0));
        assert_eq!(dt.read_element_f64(&[0x2c, 0x01, 0, 0]), Some(300.0));
    }

    /// A non-zero offset shifts the value down; padding of ones on either side
    /// (the lo_pad / hi_pad flags, bits 1 and 2) is masked away.
    #[test]
    fn offset_and_padding_bits_are_dropped() {
        // 12 bits at offset 4, pad-with-ones below and above: -20 is
        // 0xfec << 4 with the low nibble and top 16 bits set.
        let dt = decode(&fixed_point(0x08 | 0x02 | 0x04, 4, 4, 12)).unwrap();
        assert_eq!(dt.read_element_f64(&[0xcf, 0xfe, 0xff, 0xff]), Some(-20.0));
        assert_eq!(dt.read_element_f64(&[0x5f, 0x00, 0xff, 0xff]), Some(5.0));
        // Unsigned: the same top bit is a magnitude, not a sign.
        let u = decode(&fixed_point(0x00, 4, 4, 12)).unwrap();
        assert_eq!(u.read_element_f64(&[0xc0, 0xfe, 0, 0]), Some(4076.0));
    }

    /// Byte order is applied before the shift: the bit offset counts from the
    /// value's least significant bit, wherever that byte sits.
    #[test]
    fn big_endian_offset_counts_from_the_least_significant_bit() {
        let dt = decode(&fixed_point(0x08 | 0x01, 4, 4, 12)).unwrap();
        assert_eq!(dt.read_element_f64(&[0, 0, 0xfe, 0xc0]), Some(-20.0));
    }

    /// The classic rewrite used for attribute values goes through the same
    /// extraction, so an attribute and a dataset of one type agree.
    #[test]
    fn classic_bytes_hold_the_extracted_value() {
        let dt = decode(&fixed_point(0x08, 4, 4, 12)).unwrap();
        let raw = [0xc0, 0xfe, 0, 0, 0x50, 0, 0, 0];
        assert_eq!(
            dt.to_classic_bytes(&raw).unwrap(),
            [(-20i32).to_be_bytes(), 5i32.to_be_bytes()].concat()
        );
        assert_eq!(dt.to_classic_bytes(&raw[..5]), None);
    }

    /// `read_element_f64` skips `element_bits` for every type that is not a
    /// packed integer. For each such type, in both byte orders, the two must
    /// give the same number, or the fast path is a second, different decoder.
    #[test]
    fn full_width_fast_path_agrees_with_element_bits() {
        let bytes: [u8; 8] = [0xec, 0xff, 0x7f, 0x80, 0x01, 0xfe, 0x3f, 0xc0];
        for order in [0x00, 0x01] {
            for (class, signed, size) in [
                (CLASS_FIXED_POINT, 0x08, 1),
                (CLASS_FIXED_POINT, 0x00, 1),
                (CLASS_FIXED_POINT, 0x08, 2),
                (CLASS_FIXED_POINT, 0x00, 2),
                (CLASS_FIXED_POINT, 0x08, 4),
                (CLASS_FIXED_POINT, 0x00, 4),
                (CLASS_FIXED_POINT, 0x08, 8),
                (CLASS_FIXED_POINT, 0x00, 8),
                (CLASS_FLOATING_POINT, 0x00, 4),
                (CLASS_FLOATING_POINT, 0x00, 8),
            ] {
                let dt = decode(&datatype(class, order | signed, size)).unwrap();
                assert!(!dt.is_packed());
                let bits = dt.element_bits(&bytes).unwrap();
                let via_bits = match dt.nc_type {
                    NcType::Float => f64::from(f32::from_bits(bits as u32)),
                    NcType::Double => f64::from_bits(bits),
                    _ if dt.signed => bits as i64 as f64,
                    _ => bits as f64,
                };
                assert_eq!(
                    dt.read_element_f64(&bytes).map(f64::to_bits),
                    Some(via_bits.to_bits()),
                    "{:?} order {order}",
                    dt.nc_type
                );
            }
        }
    }

    /// A 64-bit container at full precision needs no mask, and a precision of
    /// 64 must not overflow the shift that builds one.
    #[test]
    fn full_width_int64_round_trips() {
        let dt = decode(&datatype(CLASS_FIXED_POINT, 0x08, 8)).unwrap();
        assert_eq!(dt.read_element_f64(&(-5i64).to_le_bytes()), Some(-5.0));
    }

    /// Offset + precision past the container, or a zero precision, is a
    /// malformed message, as libhdf5 also treats it; so is a message cut off
    /// before its properties.
    #[test]
    fn rejects_bits_outside_the_container() {
        for (offset, precision) in [(0u16, 0u16), (4, 29), (32, 1)] {
            assert!(
                matches!(
                    decode(&fixed_point(0x08, 4, offset, precision)).unwrap_err(),
                    FieldglassError::Parse(_)
                ),
                "offset {offset} precision {precision}"
            );
        }
        let whole = datatype(CLASS_FIXED_POINT, 0x08, 4);
        assert!(matches!(
            decode(&whole[..8]).unwrap_err(),
            FieldglassError::Parse(_)
        ));
        let float = datatype(CLASS_FLOATING_POINT, 0, 4);
        assert!(matches!(
            decode(&float[..12]).unwrap_err(),
            FieldglassError::Parse(_)
        ));
    }

    /// Any departure from the IEEE layout is refused rather than read as IEEE.
    #[test]
    fn refuses_non_ieee_float_layouts() {
        let base = datatype(CLASS_FLOATING_POINT, 0, 4);
        // (what, byte index, new value); byte 1 is the bit field's low byte,
        // 0x20 = implied normalization.
        let variants = [
            ("precision 24", 10, 24),
            ("offset 8", 8, 8),
            ("bias 63", 16, 63),
            ("exponent size 7", 13, 7),
            ("VAX order", 1, 0x20 | 0x41),
            ("no normalization", 1, 0x00),
        ];
        for (what, index, value) in variants {
            let mut body = base.clone();
            body[index] = value;
            assert!(
                matches!(
                    decode(&body).unwrap_err(),
                    FieldglassError::UnsupportedSection(_)
                ),
                "{what}"
            );
        }
        // Padding flags alone do not change how an IEEE element reads.
        let mut padded = base.clone();
        padded[1] |= 0x0e;
        assert!(decode(&padded).is_ok());
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

    /// `size` and `nc_type` are two spellings of one width, and `decode`
    /// derives the second from the first — but the struct's fields are public,
    /// so nothing in the type system makes them agree. The read is bounded by
    /// the width it actually uses, so a disagreement is a `None`, not a panic
    /// on a four-byte read into a two-byte slice.
    #[test]
    fn a_size_that_disagrees_with_the_type_yields_none_not_a_panic() {
        let mismatched = Datatype {
            class: DatatypeClass::FixedPoint,
            size: 2,
            signed: true,
            byte_order: Some(ByteOrder::LittleEndian),
            nc_type: NcType::Int, // four bytes wide, unlike `size`
            bit_offset: 0,
            bit_precision: 32,
        };
        assert_eq!(mismatched.read_element_f64(&[0x2A, 0]), None);
        // Given the bytes `nc_type` asks for, it reads them.
        assert_eq!(mismatched.read_element_f64(&[0x2A, 0, 0, 0]), Some(42.0));
    }

    /// A class outside the decoded subset is `UnsupportedSection`, not `Parse`
    /// — the distinction the dimension-scale layer acts on (#550) — and the
    /// message names the class so a user can tell compound from vlen.
    #[test]
    fn rejects_unsupported_class_as_unsupported_not_malformed() {
        for (class, name) in [
            (5u8, "opaque"),
            (6, "compound"),
            (8, "enumeration"),
            (9, "variable-length"),
            (10, "array"),
        ] {
            let err = decode(&datatype(class, 0x00, 16)).unwrap_err();
            match err {
                FieldglassError::UnsupportedSection(msg) => {
                    assert!(
                        msg.contains(&format!("class {class} ({name})")),
                        "class {class}: {msg:?}"
                    );
                }
                other => panic!("class {class}: expected UnsupportedSection, got {other:?}"),
            }
        }
    }

    /// A numeric class of a width with no `NcType` (a 16-bit half float, a
    /// 3-byte integer) is likewise not-implemented rather than malformed.
    #[test]
    fn rejects_unsupported_numeric_widths_as_unsupported() {
        assert!(matches!(
            decode(&datatype(CLASS_FLOATING_POINT, 0x00, 2)).unwrap_err(),
            FieldglassError::UnsupportedSection(_)
        ));
        assert!(matches!(
            decode(&datatype(CLASS_FIXED_POINT, 0x08, 3)).unwrap_err(),
            FieldglassError::UnsupportedSection(_)
        ));
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
