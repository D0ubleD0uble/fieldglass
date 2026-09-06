//! Open a NetCDF file, list the variables worth plotting, and decode one 2-D
//! plane of each in CF physical units.
//!
//!     cargo run -p fieldglass-netcdf --example decode
//!
//! Runs against three committed fixtures, so it works from a clean checkout
//! with no network and no `samples/`. The set is deliberate: one classic
//! (CDF-1) file and two NetCDF-4 / HDF5 files, opened by exactly the same four
//! calls — [`NetcdfReader::from_bytes`], [`NetcdfReader::view`],
//! [`DatasetView::renderable_variables`], [`NetcdfReader::decode_plane`] — so
//! nothing here names which on-disk layout it got. The third states its grid as
//! a WRF projection rather than as CF coordinate variables, so its horizontal
//! axes are not detected and the caller has to pick them; that is the other
//! branch a consumer has to handle.
//!
//! `decode_plane` is the whole chain in the order it has to run: decode the
//! variable, pick the plane out of it, then apply the CF mask-and-scale from
//! that variable's own attributes. Both fixtures store packed `int16` with a
//! `scale_factor`, so the printed range is in physical units rather than in
//! stored codes.

use fieldglass_netcdf::{FieldglassError, NetcdfReader};

/// A `temp(lat, lon)` field stored as scaled `int16` with `scale_factor`,
/// `add_offset`, `_FillValue` and `valid_range` — classic CDF-1.
const CLASSIC: &[u8] = include_bytes!("../tests/fixtures/cf_packed_data.nc");

/// NOAA OISST v2 AVHRR daily SST — NetCDF-4 / HDF5, chunked and
/// deflate-compressed, `sst(time, zlev, lat, lon)` as packed `int16`.
const NETCDF4: &[u8] = include_bytes!("../tests/fixtures/oisst_avhrr_v2.nc");

/// A WRF file on a Lambert conformal grid: `XLAT` / `XLONG` are 2-D and the
/// data variables do not name them as `coordinates`, so no CF axis pair is
/// detected and the skip branch below is taken.
const WRF: &[u8] = include_bytes!("../tests/fixtures/wrf_lambert.nc");

fn main() -> Result<(), FieldglassError> {
    for (label, bytes) in [("classic", CLASSIC), ("netcdf-4", NETCDF4), ("wrf", WRF)] {
        let reader = NetcdfReader::from_bytes(bytes.to_vec())?;
        let view = reader.view()?;
        println!(
            "{label}: {} dimension(s), {} variable(s)",
            view.dims.len(),
            view.vars.len()
        );

        for renderable in view.renderable_variables() {
            // A variable whose horizontal axes the CF conventions do not name
            // leaves the choice to the caller (a WRF field is the usual case);
            // the render host has a fallback for that, an example should not
            // pretend to.
            let (Some(y_dim), Some(x_dim)) = (renderable.detected_y_dim, renderable.detected_x_dim)
            else {
                println!("  {}: no CF lat/lon axis pair; skipping", renderable.name);
                continue;
            };

            let var = view
                .var(renderable.decode_index)
                .expect("a renderable variable is one of the view's variables");
            // One index per declared dimension. The entries for `y_dim` and
            // `x_dim` are ignored, so zero picks the first plane of every other
            // axis — the first time step, the surface level, and so on.
            let fixed = vec![0usize; renderable.dims.len()];
            let plane = reader.decode_plane(var, y_dim, x_dim, &fixed)?;

            let (nj, ni) = (renderable.dims[y_dim].length, renderable.dims[x_dim].length);
            let present: Vec<f64> = plane.iter().flatten().copied().collect();
            let range = match (
                present.iter().copied().fold(f64::INFINITY, f64::min),
                present.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            ) {
                _ if present.is_empty() => "all masked".to_string(),
                (lo, hi) => format!("{lo:.2} .. {hi:.2}"),
            };
            println!(
                "  {} {nj}×{ni}: {} value(s), {} present, {range}",
                renderable.name,
                plane.len(),
                present.len(),
            );
        }
    }

    Ok(())
}
