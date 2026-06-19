//! HDF5 attribute decoder (issue #40, under #33). Decodes Attribute messages
//! (`0x000C`) — name, datatype, dataspace, and value — for an object header,
//! covering both the inline form (messages in the header) and the dense form
//! (Attribute Info message `0x0015` → fractal heap + version-2 B-tree) that
//! `netCDF4` uses once an object has many attributes.
//!
//! The same entry point serves global attributes (pass the root group's object
//! header) and per-dataset attributes. Values are rendered to a display string
//! the same way as the classic NetCDF path.
//!
//! Reference: HDF5 file format specification version 3, "Attribute Message" and
//! "Attribute Info Message".

use super::Hdf5Probe;
use super::dataspace::{self, Dataspace};
use super::datatype::{self, ByteOrder, Datatype, DatatypeClass};
use super::heap::{self, Cursor, FractalHeap};
use super::object_header::{self, read_uint_le};
use crate::classic;
use fieldglass_core::FieldglassError;

const MSG_ATTRIBUTE: u16 = 0x000C;
const MSG_ATTRIBUTE_INFO: u16 = 0x0015;

/// Attribute-name B-tree v2 record: the fractal-heap ID comes first, followed
/// by message flags, creation order, and a name hash.
const ATTR_RECORD_HEAP_ID_OFFSET: usize = 0;

/// Upper bound on attributes per object — guards a malformed dense count.
const MAX_ATTRIBUTES: usize = 1 << 20;

/// A decoded HDF5 attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hdf5Attribute {
    pub name: String,
    pub datatype: Datatype,
    pub dataspace: Dataspace,
    /// Display value: UTF-8 text for strings, comma-separated decimals for
    /// numeric types (matching the classic NetCDF render path).
    pub value: String,
}

/// List the attributes attached to the object header at `object_header_address`,
/// sorted by name. Works for the root group (global attributes) and datasets.
pub fn list_attributes(
    bytes: &[u8],
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<Vec<Hdf5Attribute>, FieldglassError> {
    let header = object_header::walk(
        bytes,
        object_header_address,
        probe.offset_size,
        probe.length_size,
    )?;

    let mut attrs = Vec::new();
    // Inline attributes: Attribute messages directly in the header.
    for msg in header
        .messages
        .iter()
        .filter(|m| m.msg_type == MSG_ATTRIBUTE)
    {
        push_attribute(
            &mut attrs,
            parse_attribute_message(&msg.body, probe.length_size)?,
        )?;
    }
    // Dense attributes: Attribute Info → fractal heap + B-tree v2.
    if let Some(msg) = header
        .messages
        .iter()
        .find(|m| m.msg_type == MSG_ATTRIBUTE_INFO)
    {
        read_dense_attributes(bytes, &msg.body, probe, &mut attrs)?;
    }

    attrs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(attrs)
}

/// Resolve dense attributes from an Attribute Info message body.
fn read_dense_attributes(
    bytes: &[u8],
    body: &[u8],
    probe: &Hdf5Probe,
    out: &mut Vec<Hdf5Attribute>,
) -> Result<(), FieldglassError> {
    let o = probe.offset_size as usize;
    // version (1) + flags (1), then optional max-creation-index (2).
    if body.len() < 2 {
        return Err(FieldglassError::Parse(
            "attribute info message too small".into(),
        ));
    }
    let flags = body[1];
    let mut pos = 2usize;
    if flags & 0x01 != 0 {
        pos += 2; // maximum creation index
    }
    let heap_addr = read_uint_le(body, pos, o)?;
    // No fractal heap ⇒ no dense attributes (everything was inline).
    if is_undefined(heap_addr, probe.offset_size) {
        return Ok(());
    }
    let btree_addr = read_uint_le(body, pos + o, o)?;

    let heap = FractalHeap::parse(bytes, heap_addr, probe.offset_size, probe.length_size)?;
    let (btree_type, records) =
        heap::btree_v2_leaf_records(bytes, btree_addr, probe.offset_size, probe.length_size)?;
    if btree_type != 8 && btree_type != 9 {
        return Err(FieldglassError::Parse(format!(
            "unsupported B-tree v2 type {btree_type} for attributes"
        )));
    }

    for record in records {
        let id = record
            .get(ATTR_RECORD_HEAP_ID_OFFSET..ATTR_RECORD_HEAP_ID_OFFSET + heap.heap_id_len)
            .ok_or_else(|| {
                FieldglassError::Parse("attribute record too small for a heap ID".into())
            })?;
        let message = heap.managed_object(bytes, id)?;
        push_attribute(out, parse_attribute_message(&message, probe.length_size)?)?;
    }
    Ok(())
}

/// Parse a single Attribute message body (versions 1, 2, and 3).
fn parse_attribute_message(body: &[u8], length_size: u8) -> Result<Hdf5Attribute, FieldglassError> {
    let mut cur = Cursor::over(body);
    let version = cur.byte()?;
    cur.skip(1)?; // reserved (v1) / flags (v2, v3)
    let name_size = cur.u16()? as usize;
    let datatype_size = cur.u16()? as usize;
    let dataspace_size = cur.u16()? as usize;

    // Version 1 pads each block to an 8-byte boundary; 2 and 3 don't. Version 3
    // adds a name character-set byte before the name.
    let padded = match version {
        1 => true,
        2 | 3 => false,
        other => {
            return Err(FieldglassError::Parse(format!(
                "unsupported attribute message version {other}"
            )));
        }
    };
    if version == 3 {
        cur.skip(1)?; // name character set encoding
    }

    let name = decode_name(cur.take(name_size)?);
    skip_padding(&mut cur, name_size, padded)?;
    let datatype_bytes = cur.take(datatype_size)?.to_vec();
    skip_padding(&mut cur, datatype_size, padded)?;
    let dataspace_bytes = cur.take(dataspace_size)?.to_vec();
    skip_padding(&mut cur, dataspace_size, padded)?;
    let data = cur.remaining();

    let datatype = datatype::decode(&datatype_bytes)?;
    let dataspace = dataspace::decode(&dataspace_bytes, length_size)?;
    let value = render_value(data, &datatype, &dataspace)?;
    Ok(Hdf5Attribute {
        name,
        datatype,
        dataspace,
        value,
    })
}

/// For version-1 messages, advance past the zero padding that rounds a block of
/// `size` bytes up to an 8-byte boundary.
fn skip_padding(cur: &mut Cursor, size: usize, padded: bool) -> Result<(), FieldglassError> {
    if padded {
        cur.skip(size.next_multiple_of(8) - size)?;
    }
    Ok(())
}

/// Render an attribute's raw value bytes to a display string.
fn render_value(
    data: &[u8],
    datatype: &Datatype,
    dataspace: &Dataspace,
) -> Result<String, FieldglassError> {
    match datatype.class {
        DatatypeClass::FixedLengthString => {
            let len = (datatype.size as usize).min(data.len());
            Ok(decode_name(&data[..len]))
        }
        DatatypeClass::FixedPoint | DatatypeClass::FloatingPoint => {
            let elem = datatype.size as usize;
            let count = element_count(dataspace);
            let total = count
                .checked_mul(elem)
                .ok_or_else(|| FieldglassError::Parse("attribute value size overflow".into()))?;
            let raw = data.get(..total).ok_or_else(|| {
                FieldglassError::Parse("attribute value past end of message".into())
            })?;
            // The classic renderer reads big-endian; swap each little-endian
            // element so it can be reused verbatim.
            let normalized: Vec<u8> = if datatype.byte_order == Some(ByteOrder::LittleEndian) {
                raw.chunks_exact(elem)
                    .flat_map(|c| c.iter().rev().copied())
                    .collect()
            } else {
                raw.to_vec()
            };
            Ok(classic::render_numeric_values(
                &normalized,
                datatype.nc_type,
            ))
        }
    }
}

/// Number of elements in a dataspace (1 for scalar).
fn element_count(dataspace: &Dataspace) -> usize {
    if dataspace.is_scalar {
        return 1;
    }
    dataspace
        .dims
        .iter()
        .try_fold(1usize, |acc, &d| acc.checked_mul(d as usize))
        .unwrap_or(usize::MAX)
}

fn push_attribute(
    out: &mut Vec<Hdf5Attribute>,
    attr: Hdf5Attribute,
) -> Result<(), FieldglassError> {
    if out.len() >= MAX_ATTRIBUTES {
        return Err(FieldglassError::Parse(
            "object has too many attributes".into(),
        ));
    }
    out.push(attr);
    Ok(())
}

/// Decode a possibly null-terminated, possibly null-padded name as UTF-8.
fn decode_name(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// Whether an address field is the HDF5 "undefined address" sentinel (all ones).
fn is_undefined(address: u64, osize: u8) -> bool {
    let o = osize as usize;
    if o >= 8 {
        address == u64::MAX
    } else {
        address == (1u64 << (8 * o)) - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NcType;

    /// Build an inline (version 1, padded) attribute message.
    fn attr_v1(name: &str, datatype: &[u8], dataspace: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = vec![1u8, 0];
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&(datatype.len() as u16).to_le_bytes());
        v.extend_from_slice(&(dataspace.len() as u16).to_le_bytes());
        let pad = |v: &mut Vec<u8>, n: usize| v.resize(v.len() + (n.next_multiple_of(8) - n), 0);
        v.extend_from_slice(name.as_bytes());
        pad(&mut v, name.len());
        v.extend_from_slice(datatype);
        pad(&mut v, datatype.len());
        v.extend_from_slice(dataspace);
        pad(&mut v, dataspace.len());
        v.extend_from_slice(data);
        v
    }

    /// Build a version-3 (charset byte, unpadded) attribute message.
    fn attr_v3(name: &str, datatype: &[u8], dataspace: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = vec![3u8, 0];
        v.extend_from_slice(&(name.len() as u16).to_le_bytes());
        v.extend_from_slice(&(datatype.len() as u16).to_le_bytes());
        v.extend_from_slice(&(dataspace.len() as u16).to_le_bytes());
        v.push(0); // name charset
        v.extend_from_slice(name.as_bytes());
        v.extend_from_slice(datatype);
        v.extend_from_slice(dataspace);
        v.extend_from_slice(data);
        v
    }

    /// Little-endian fixed-point datatype message (signed, given size).
    fn dt_int(size: u32) -> Vec<u8> {
        let mut v = vec![(1 << 4), 0x08, 0, 0]; // class 0, signed bit
        v.extend_from_slice(&size.to_le_bytes());
        v
    }
    fn dt_double() -> Vec<u8> {
        let mut v = vec![(1 << 4) | 1, 0x00, 0, 0]; // class 1, little-endian
        v.extend_from_slice(&8u32.to_le_bytes());
        v
    }
    fn dt_string(size: u32) -> Vec<u8> {
        let mut v = vec![(1 << 4) | 3, 0x00, 0, 0]; // class 3
        v.extend_from_slice(&size.to_le_bytes());
        v
    }
    fn ds_scalar() -> Vec<u8> {
        vec![1u8, 0, 0, 0, 0, 0, 0, 0]
    }

    #[test]
    fn decodes_v1_scalar_int() {
        let msg = attr_v1("version", &dt_int(4), &ds_scalar(), &5i32.to_le_bytes());
        let a = parse_attribute_message(&msg, 8).unwrap();
        assert_eq!(a.name, "version");
        assert_eq!(a.datatype.nc_type, NcType::Int);
        assert_eq!(a.value, "5");
    }

    #[test]
    fn decodes_v3_scalar_double() {
        let msg = attr_v3("scale", &dt_double(), &ds_scalar(), &0.25f64.to_le_bytes());
        let a = parse_attribute_message(&msg, 8).unwrap();
        assert_eq!(a.name, "scale");
        assert_eq!(a.datatype.nc_type, NcType::Double);
        assert_eq!(a.value, "0.25");
    }

    #[test]
    fn decodes_string_attribute() {
        let text = b"meters";
        let msg = attr_v3("units", &dt_string(text.len() as u32), &ds_scalar(), text);
        let a = parse_attribute_message(&msg, 8).unwrap();
        assert_eq!(a.name, "units");
        assert_eq!(a.value, "meters");
    }

    #[test]
    fn decodes_big_endian_value() {
        // Big-endian datatype: byte-order bit set; value bytes are big-endian.
        let mut dt = vec![(1 << 4), 0x09, 0, 0]; // class 0, signed + big-endian
        dt.extend_from_slice(&4u32.to_le_bytes());
        let msg = attr_v3("be", &dt, &ds_scalar(), &7i32.to_be_bytes());
        let a = parse_attribute_message(&msg, 8).unwrap();
        assert_eq!(a.value, "7");
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut msg = attr_v3("x", &dt_int(4), &ds_scalar(), &1i32.to_le_bytes());
        msg[0] = 9; // version 9
        assert!(parse_attribute_message(&msg, 8).is_err());
    }

    #[test]
    fn rejects_truncated_value() {
        // Declares a 4-byte int but supplies only 2 value bytes.
        let msg = attr_v3("short", &dt_int(4), &ds_scalar(), &[1, 2]);
        assert!(parse_attribute_message(&msg, 8).is_err());
    }
}
