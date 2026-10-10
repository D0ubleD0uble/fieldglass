//! A NetCDF file as an [`ArraySource`] (#704).
//!
//! The array-level IO seam every container of named arrays presents
//! (ADR-0010's amendment to decision 3). A Zarr store reaches it by spelling
//! chunk keys; this reaches it the way this crate always has — the classic
//! header's offsets or the HDF5 chunk index, over the file's bytes — and above
//! it nothing needs to know which: CF placement, the renderable list and
//! `Session` are written once, in `fieldglass-core` and the umbrella.
//!
//! # Names
//!
//! Every variable keeps the name the [`DatasetView`] gives it, except that a
//! nested NetCDF-4 variable's leading `/` is dropped: the view spells it
//! `/PRODUCT/latitude`, the way HDF5 paths read, and a Zarr store's
//! [`Group::arrays_qualified`] spells the same position `PRODUCT/latitude`. Two
//! containers presenting one shape of file should present one spelling, so this
//! is where they are made to agree. A root-level variable has no `/` in either.

use std::borrow::Borrow;
use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{ArraySource, Group, LeftOut};

use crate::geometry::DatasetView;
use crate::reader::NetcdfReader;

/// A name as [`NetcdfArrays`] presents it: the view's, less a leading `/`.
pub(crate) fn presented(name: &str) -> &str {
    name.strip_prefix('/').unwrap_or(name)
}

/// A NetCDF file's variables as an [`ArraySource`].
///
/// Generic over whether it owns its reader and view or borrows them, so one
/// implementation serves both a `Session` that keeps the file open (owned) and
/// this crate's own [`NetcdfReader::slice_placement`] family, which is handed
/// a view it must not rebuild (borrowed).
#[derive(Debug)]
pub struct NetcdfArrays<R = NetcdfReader, V = DatasetView> {
    reader: R,
    view: V,
    group: Group,
}

impl NetcdfArrays {
    /// Open a reader's variables, resolving its view once.
    ///
    /// # Errors
    ///
    /// Where [`NetcdfReader::view`] does: an HDF5 layout outside the decoded
    /// subset.
    pub fn open(reader: NetcdfReader) -> Result<Self, FieldglassError> {
        let view = reader.view()?;
        Ok(Self::new(reader, view))
    }
}

impl<R: Borrow<NetcdfReader>, V: Borrow<DatasetView>> NetcdfArrays<R, V> {
    /// Present `reader`'s variables as `view` describes them.
    pub fn new(reader: R, view: V) -> Self {
        let mut group = view.borrow().group();
        for array in &mut group.arrays {
            array.name = presented(&array.name).to_string();
            for dim in &mut array.dimensions {
                *dim = presented(dim).to_string();
            }
        }
        for dim in &mut group.dimensions {
            dim.name = presented(&dim.name).to_string();
        }
        Self {
            reader,
            view,
            group,
        }
    }

    /// The reader the values come from.
    pub fn reader(&self) -> &NetcdfReader {
        self.reader.borrow()
    }

    /// The view the structure comes from, names as the file spells them.
    pub fn view(&self) -> &DatasetView {
        self.view.borrow()
    }
}

impl<R: Borrow<NetcdfReader>, V: Borrow<DatasetView>> ArraySource for NetcdfArrays<R, V> {
    fn group(&self) -> &Group {
        &self.group
    }

    /// The view's `unsupported` list, in the shape every container states it
    /// (#709), with names spelled the way this seam spells a *readable* array's —
    /// `PRODUCT/sub` and not `/PRODUCT/sub`. Without that a host could not match
    /// a left-out name against the list it did get, which is the whole point of
    /// having one.
    fn left_out(&self) -> Vec<LeftOut> {
        self.view()
            .unsupported
            .iter()
            .map(|v| LeftOut {
                name: presented(&v.name).to_string(),
                reason: v.reason.clone(),
            })
            .collect()
    }

    /// Reads the region alone, through [`NetcdfReader::decode_region_raw`]:
    /// the bytes of its runs in a classic file, the chunks it overlaps in a
    /// NetCDF-4 one (#939). The values are the raw decode's, fill and missing
    /// sentinels already masked.
    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let var = self
            .view()
            .vars
            .iter()
            .find(|v| presented(v.name()) == array)
            .ok_or_else(|| {
                FieldglassError::Parse(format!("this file holds no variable {array:?}"))
            })?;
        self.reader().decode_region_raw(var.decode_index, region)
    }
}
