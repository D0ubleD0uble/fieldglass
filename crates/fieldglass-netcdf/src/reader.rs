//! Top-level NetCDF reader. Detects which sub-format we have and dispatches
//! to either the pure-Rust classic header parser or the minimal HDF5
//! superblock probe. See `classic.rs` and `hdf5.rs` for the per-layout work.

use crate::classic::{self, ClassicHeader};
use crate::hdf5::{self, Hdf5Probe};
use fieldglass_core::FieldglassError;

/// Which on-disk layout backs a NetCDF file.
#[derive(Debug, Clone)]
pub enum NetcdfBacking {
    /// CDF-1 / CDF-2 / CDF-5 — fully parsed at the header level.
    Classic(ClassicHeader),
    /// NetCDF-4 / HDF5. Deep parsing is a follow-up; we surface enough to
    /// validate the file and tell the user.
    Hdf5(Hdf5Probe),
}

impl NetcdfBacking {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Classic(h) => match h.version {
                classic::ClassicVersion::Cdf1 => "NetCDF classic (CDF-1)",
                classic::ClassicVersion::Cdf2 => "NetCDF 64-bit offset (CDF-2)",
                classic::ClassicVersion::Cdf5 => "NetCDF 64-bit data (CDF-5)",
            },
            Self::Hdf5(_) => "NetCDF-4 / HDF5",
        }
    }

    /// Whether the metadata is fully exposed (`true` for classic) or whether
    /// we only have a format probe (`false` for HDF5 today). The provider
    /// uses this to render a "deep parsing not yet implemented" notice.
    pub fn is_fully_parsed(&self) -> bool {
        matches!(self, Self::Classic(_))
    }
}

/// Top-level reader. Always carries the raw bytes so future per-variable
/// decode work can pull data on demand without re-reading the file.
#[derive(Debug)]
pub struct NetcdfReader {
    pub backing: NetcdfBacking,
    #[allow(dead_code)]
    data: Vec<u8>,
}

impl NetcdfReader {
    /// Parse a NetCDF file from raw bytes. Errors only for files that are
    /// neither classic CDF nor HDF5; HDF5 files succeed but expose only the
    /// superblock probe (see `NetcdfBacking::is_fully_parsed`).
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, FieldglassError> {
        let backing = if data.len() >= 4 && &data[0..3] == b"CDF" {
            let header = classic::parse_header(&data)?;
            NetcdfBacking::Classic(header)
        } else if data.len() >= 8 && data[0..8] == hdf5::HDF5_SIGNATURE {
            let probe = hdf5::probe(&data)?;
            NetcdfBacking::Hdf5(probe)
        } else {
            return Err(FieldglassError::InvalidMagic);
        };
        Ok(Self { backing, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_magic() {
        let err = NetcdfReader::from_bytes(b"NOTANCDF".to_vec()).unwrap_err();
        assert!(matches!(err, FieldglassError::InvalidMagic));
    }
}
