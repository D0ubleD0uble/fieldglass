//! Per-dataset shape + element type (issue #39, under #33). Ties together the
//! dataspace and datatype message decoders: given a dataset's object-header
//! address, walk the header and decode its Dataspace (`0x0001`) and Datatype
//! (`0x0003`) messages. This is the hook #33 will use to populate
//! `DatasetMeta.dimensions` / `.datatype` for NetCDF-4 files.

use super::Hdf5Probe;
use super::dataspace::{self, Dataspace};
use super::datatype::{self, Datatype};
use super::object_header;
use fieldglass_core::FieldglassError;

const MSG_DATASPACE: u16 = 0x0001;
const MSG_DATATYPE: u16 = 0x0003;

/// The decoded shape and element type of a dataset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetShape {
    pub dataspace: Dataspace,
    pub datatype: Datatype,
}

/// Describe the dataset whose object header is at `object_header_address`.
pub fn describe(
    bytes: &[u8],
    object_header_address: u64,
    probe: &Hdf5Probe,
) -> Result<DatasetShape, FieldglassError> {
    let header = object_header::walk(
        bytes,
        object_header_address,
        probe.offset_size,
        probe.length_size,
    )?;
    let body = |msg_type: u16, label: &str| {
        header
            .messages
            .iter()
            .find(|m| m.msg_type == msg_type)
            .map(|m| m.body.as_slice())
            .ok_or_else(|| FieldglassError::Parse(format!("dataset has no {label} message")))
    };

    let dataspace = dataspace::decode(body(MSG_DATASPACE, "dataspace")?, probe.length_size)?;
    let datatype = datatype::decode(body(MSG_DATATYPE, "datatype")?)?;
    Ok(DatasetShape {
        dataspace,
        datatype,
    })
}
