//! A WRF grid whose origin corner is masked is refused naming the coordinate
//! (#1648).
//!
//! `XLAT` and `XLONG` fix the Lambert origin from their `(0, 0)` cell. A
//! corner with no value cannot place anything, and the refusal names the
//! coordinate in double quotes as the 1-D coordinate refusals do. The source
//! reads the committed file and withholds that one value, as a masked
//! `_FillValue` cell would be.

use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{ArraySource, Group};
use fieldglass_core::cf::slice_placement;
use fieldglass_netcdf::{NetcdfArrays, NetcdfReader, presented_name};

const WRF: &[u8] = include_bytes!("fixtures/wrf_lambert.nc");

struct MaskedCorner {
    inner: NetcdfArrays,
}

impl ArraySource for MaskedCorner {
    fn group(&self) -> &Group {
        self.inner.group()
    }

    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let mut values = self.inner.read_region(array, region)?;
        if presented_name(array) == "XLAT" {
            values[0] = None;
        }
        Ok(values)
    }
}

#[test]
fn a_masked_wrf_corner_is_refused_naming_the_coordinate_in_double_quotes() {
    let reader = NetcdfReader::from_bytes(WRF.to_vec()).expect("fixture opens");
    let view = reader.view().expect("view resolves");
    let var = view
        .renderable_variables()
        .into_iter()
        .find(|v| v.dims.len() >= 2)
        .expect("a renderable variable");
    let (y, x) = (var.dims.len() - 2, var.dims.len() - 1);
    let name = presented_name(&var.name).to_string();
    let source = MaskedCorner {
        inner: NetcdfArrays::open(reader).expect("a view"),
    };

    let err = slice_placement(&source, &name, y, x).expect_err("a masked origin places nothing");
    assert!(
        err.to_string()
            .contains("coordinate \"XLAT\" has no value at flat index 0"),
        "{err}"
    );
}
