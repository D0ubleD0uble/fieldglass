//! Top-level NetCDF reader. Detects which sub-format we have and dispatches
//! to either the pure-Rust classic header parser or the minimal HDF5
//! superblock probe. See `classic.rs` and `hdf5.rs` for the per-layout work.

use crate::classic::{self, ClassicHeader};
use crate::geometry::{DatasetView, VarView, extract_plane};
use crate::hdf5::{self, Hdf5Probe};
use fieldglass_core::FieldglassError;

/// Which on-disk layout backs a NetCDF file.
#[derive(Debug, Clone)]
pub enum NetcdfBacking {
    /// CDF-1 / CDF-2 / CDF-5 — fully parsed at the header level.
    Classic(ClassicHeader),
    /// NetCDF-4 / HDF5. The superblock probe is eager; the dimensions,
    /// variables, and attributes are resolved on demand from the raw bytes via
    /// [`NetcdfReader::hdf5_metadata`] (decision 0003).
    Hdf5(Hdf5Probe),
}

impl NetcdfBacking {
    /// The backing's human-readable name, e.g. `"NetCDF classic (CDF-1)"`.
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

    /// Whether the metadata is parsed eagerly at construction (`true` for
    /// classic). HDF5 carries only the superblock probe in the backing and
    /// resolves its metadata lazily via [`NetcdfReader::hdf5_metadata`], so this
    /// is `false` for HDF5 even though that metadata is now fully available.
    pub fn is_fully_parsed(&self) -> bool {
        matches!(self, Self::Classic(_))
    }
}

/// Top-level reader. Always carries the raw bytes so per-variable decode can
/// pull data on demand without re-reading the file.
#[derive(Debug)]
pub struct NetcdfReader {
    /// Which on-disk layout the file turned out to be, with whatever the probe
    /// resolved eagerly for it.
    pub backing: NetcdfBacking,
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

    /// Decode one variable's values into row-major (C / on-disk order)
    /// `Vec<Option<f64>>` — `Some(v)` for present points, `None` where the
    /// element equals the variable's `_FillValue` or CF `missing_value`.
    /// Mirrors the GRIB `decode_message_values` surface.
    ///
    /// **These are the raw on-disk codes.** Masking the sentinels is the only
    /// thing done to them: the CF `scale_factor` / `add_offset` / `valid_range`
    /// mask-and-scale is a separate stage, so a packed field (scaled `int16`, as
    /// GOES / MERRA-2 / ERA5 store it) comes back as integer codes, not physical
    /// units. Use [`Self::decode_variable_physical`] to get both stages in one
    /// call, or [`Self::decode_plane`] to also pick a 2-D plane out of an N-D
    /// variable.
    ///
    /// For HDF5 / NetCDF-4 backings a "variable" is any dataset in the file
    /// (nested groups included, #219), indexed in the same whole-file depth-first
    /// order [`Self::variable_shape`] uses. Datasets
    /// stored with a Data Layout the reader doesn't decode yet (e.g. a
    /// version-4 chunk index) return [`FieldglassError::UnsupportedSection`].
    pub fn decode_variable_values(
        &self,
        var_index: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        match &self.backing {
            NetcdfBacking::Classic(header) => {
                classic::decode_variable_values(header, &self.data, var_index)
            }
            NetcdfBacking::Hdf5(probe) => {
                let addr = hdf5_dataset_address(&self.data, probe, var_index)?;
                hdf5::values::read_dataset_values(&self.data, addr, probe)
            }
        }
    }

    /// Resolve a NetCDF-4 / HDF5 file's metadata — named dimensions, variables
    /// with ordered dimension lists, and global attributes — across the whole
    /// file, descending into nested groups (#219, variables path-qualified as
    /// `/GROUP/name`), by reading the dimension-scale convention (decision 0003).
    /// Errors for a non-HDF5 backing and for HDF5 layouts outside the decoded
    /// subset.
    pub fn hdf5_metadata(&self) -> Result<hdf5::dimensions::Hdf5Metadata, FieldglassError> {
        match &self.backing {
            NetcdfBacking::Hdf5(probe) => hdf5::dimensions::resolve(&self.data, probe),
            NetcdfBacking::Classic(_) => Err(FieldglassError::Parse(
                "hdf5_metadata is only available for the NetCDF-4 / HDF5 backing".into(),
            )),
        }
    }

    /// Runtime shape of a variable in declared (C) order. For classic backings
    /// the record dimension resolves to `numrecs`; for HDF5 it is the dataset's
    /// current dataspace dimensions (empty for a scalar).
    pub fn variable_shape(&self, var_index: usize) -> Result<Vec<u64>, FieldglassError> {
        match &self.backing {
            NetcdfBacking::Classic(header) => classic::variable_shape(header, var_index),
            NetcdfBacking::Hdf5(probe) => {
                let addr = hdf5_dataset_address(&self.data, probe, var_index)?;
                let shape = hdf5::dataset::describe(&self.data, addr, probe)?;
                Ok(shape.dataspace.dims)
            }
        }
    }

    /// The neutral [`DatasetView`] of this file's dimensions, variables, and
    /// global attributes, built from whichever backing it has. The one call
    /// replaces matching on [`Self::backing`] and picking `from_classic` /
    /// `from_hdf5` by hand — a host that does not care which layout it opened
    /// should not have to name both.
    ///
    /// Fallible only for the HDF5 backing, whose metadata is resolved lazily and
    /// can fail on a layout outside the decoded subset (decision 0003). A host
    /// that wants to keep going on that failure — showing the format-level
    /// metadata with no slice picker — can take `DatasetView::default()`, the
    /// empty view.
    pub fn view(&self) -> Result<DatasetView, FieldglassError> {
        match &self.backing {
            NetcdfBacking::Classic(header) => Ok(DatasetView::from_classic(header)),
            NetcdfBacking::Hdf5(_) => Ok(DatasetView::from_hdf5(&self.hdf5_metadata()?)),
        }
    }

    /// Decode one variable into CF **physical** units:
    /// [`Self::decode_variable_values`] followed by the mask-and-scale its own
    /// attributes call for ([`VarView::unpack`]). This is what a caller reading
    /// a packed field almost always wants; the raw method is the stage below it.
    ///
    /// Resolves [`Self::view`] to reach the attributes, which for the HDF5
    /// backing walks every dataset. A caller decoding many variables should
    /// hold one view and take [`Self::decode_plane`] per variable; a caller
    /// re-slicing *one* variable wants neither, since both decode it again —
    /// cache [`Self::decode_variable_values`] yourself and apply
    /// [`VarView::unpack`] to each plane, which is what the render host does.
    ///
    /// Errors for a decodable index the view has no variable for — a NetCDF-4
    /// pure-dimension placeholder, which carries no attributes and so has no
    /// physical form. The raw method still reads it. The view is resolved
    /// before the decode so that failure costs nothing.
    pub fn decode_variable_physical(
        &self,
        var_index: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let view = self.view()?;
        let var = view.var(var_index).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "no variable at decode index {var_index}, so no CF attributes to unpack with"
            ))
        })?;
        Ok(var.unpack(&self.decode_variable_values(var_index)?))
    }

    /// Decode one 2-D plane of a variable in CF physical units — the whole chain
    /// in one call, in the order it has to run: decode, then [`extract_plane`],
    /// then the CF mask-and-scale.
    ///
    /// `y_dim` / `x_dim` are axis positions within the variable's declared (C)
    /// dimension order, and `fixed` holds one index per dimension for the axes
    /// neither of them names (its entries for `y_dim` and `x_dim` are ignored),
    /// exactly as [`extract_plane`] takes them. The output is row-major over the
    /// picked plane, `nj` rows of `ni` values.
    ///
    /// Takes the [`VarView`] rather than a bare index because the CF attributes
    /// live on it: passing it in is what keeps this from re-resolving the view
    /// per call. It still decodes the variable on every call, so pulling many
    /// planes out of one variable wants a cached decode and [`VarView::unpack`]
    /// per plane instead.
    pub fn decode_plane(
        &self,
        var: &VarView,
        y_dim: usize,
        x_dim: usize,
        fixed: &[usize],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let raw = self.decode_variable_values(var.decode_index)?;
        let shape = self.variable_shape(var.decode_index)?;
        let plane = extract_plane(&raw, &shape, y_dim, x_dim, fixed)?;
        Ok(var.unpack(&plane))
    }
}

/// Resolve the object-header address of the `var_index`-th HDF5 dataset, in the
/// whole-file depth-first order (groups excluded, committed datatypes excluded).
/// This is the identical order [`hdf5::dimensions::resolve`] walks, so a
/// variable's `decode_index` from the resolved metadata indexes here directly —
/// including variables in nested groups (#219).
fn hdf5_dataset_address(
    bytes: &[u8],
    probe: &hdf5::Hdf5Probe,
    var_index: usize,
) -> Result<u64, FieldglassError> {
    hdf5::group::all_children(bytes, probe)?
        .iter()
        .filter(|c| c.kind == hdf5::group::ChildKind::Dataset)
        .nth(var_index)
        .map(|c| c.object_header_address)
        .ok_or(FieldglassError::OutOfRange)
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
