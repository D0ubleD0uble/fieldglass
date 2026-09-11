//! Which geometry one variable's 2-D slice has.
//!
//! The precedence — 2-D coordinates › WRF `MAP_PROJ` › CF `grid_mapping` › 1-D
//! lat/lon › source-only, with its guard against reading a projected CRS's
//! metres as degrees — is CF's, and since #704 it lives in
//! [`fieldglass_core::cf::placement`], where a Zarr store reaches it too. These
//! are this crate's entry points to it: they keep the signatures a host has
//! always called, over the [`DatasetView`] it already holds, and answer
//! through [`NetcdfArrays`].

use fieldglass_core::FieldglassError;
use fieldglass_core::projection::GridGeometry;

pub use fieldglass_core::cf::placement::{
    CfMapping, SOURCE_ONLY, SlicePlacement, classify_grid_mapping,
};

use crate::arrays::{NetcdfArrays, presented};
use crate::geometry::{DatasetView, RenderableVariable};
use crate::reader::NetcdfReader;

impl NetcdfReader {
    /// Which geometry the 2-D slice of `var` on axes `(y_dim, x_dim)` has.
    ///
    /// The precedence and its guard are described on
    /// [`fieldglass_core::cf::placement`]. The `view` is passed in rather than
    /// rebuilt because resolving it walks the whole file, and a caller placing
    /// many slices holds one already.
    ///
    /// Returns [`GridGeometry::Unsupported`] with the label [`SOURCE_ONLY`]
    /// whenever no family could place the grid. That is a real answer, not an
    /// error: the raster is still renderable in its own source projection, and
    /// it is the *safe* answer for a projected CRS this build cannot resolve.
    pub fn slice_geometry(
        &self,
        view: &DatasetView,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
    ) -> Result<GridGeometry, FieldglassError> {
        Ok(self.slice_placement(view, var, y_dim, x_dim)?.geometry)
    }

    /// [`Self::slice_geometry`] and the storage order beside it.
    ///
    /// What a host that renders wants: placing the cells and knowing which way
    /// the rows and columns run are the same question asked of the same
    /// coordinate arrays, and reading them twice to answer it separately would
    /// be both slower and a chance for the two answers to disagree.
    pub fn slice_placement(
        &self,
        view: &DatasetView,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
    ) -> Result<SlicePlacement, FieldglassError> {
        let source = NetcdfArrays::new(self, view);
        fieldglass_core::cf::slice_placement(&source, presented(&var.name), y_dim, x_dim)
    }

    /// The spatial index over a variable's 2-D coordinate arrays, or `None`
    /// when it declares none.
    ///
    /// Public because building it walks and copies every cell, so a caller
    /// placing the same variable repeatedly — a probe runs per mouse move —
    /// wants to hold the result rather than have [`Self::slice_geometry`]
    /// rebuild it. Returns the wrapped [`GridGeometry::Lookup`] rather than the
    /// bare index, so a caller caches the same type [`Self::slice_geometry`]
    /// hands back.
    pub fn curvilinear_index(
        &self,
        view: &DatasetView,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
    ) -> Result<Option<GridGeometry>, FieldglassError> {
        let source = NetcdfArrays::new(self, view);
        fieldglass_core::cf::curvilinear_lookup(&source, presented(&var.name), y_dim, x_dim)
    }

    /// A 2-D auxiliary coordinate array, CF-scaled, with a masked cell carried
    /// through as `NaN` rather than refused.
    ///
    /// Public as the sibling of [`Self::curvilinear_index`]: a host checking
    /// where a swath's cells actually landed reads the same array the index was
    /// built from, and reading it a second way would not be a check.
    pub fn coordinate_plane(
        &self,
        view: &DatasetView,
        index: usize,
    ) -> Result<Vec<f64>, FieldglassError> {
        match view.var(index) {
            Some(var) => {
                let source = NetcdfArrays::new(self, view);
                fieldglass_core::cf::coordinate_plane(&source, presented(var.name()))
            }
            // A decodable index the view has no variable for — a NetCDF-4
            // pure-dimension placeholder — has no attributes to scale by, and
            // is read as it is stored.
            None => Ok(self
                .decode_variable_raw(index)?
                .into_iter()
                .map(|v| v.unwrap_or(f64::NAN))
                .collect()),
        }
    }
}
