//! CF `missing_value` masking, cross-checked against netCDF4 on both backings.
//!
//! `temp` marks gaps with a distinct `_FillValue` and `missing_value`; the
//! oracle records the values `netCDF4` produces with auto-mask on. Decode must
//! mask a point equal to *either* sentinel. The same logical field is bundled in
//! both on-disk encodings so the classic and HDF5 decoders are each exercised.

use fieldglass_netcdf::{DatasetView, NetcdfBacking, NetcdfReader};
use serde_json::Value;

const CLASSIC: &[u8] = include_bytes!("fixtures/missing_value_classic.nc");
const NC4: &[u8] = include_bytes!("fixtures/missing_value_nc4.nc");
const ORACLE: &str = include_str!("fixtures/missing_value.oracle.json");

fn decode_temp(bytes: &[u8]) -> Vec<Option<f64>> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("parse");
    let view = match &reader.backing {
        NetcdfBacking::Classic(h) => DatasetView::from_classic(h),
        NetcdfBacking::Hdf5(_) => {
            DatasetView::from_hdf5(&reader.hdf5_metadata().expect("hdf5 metadata"))
        }
    };
    let idx = view
        .vars
        .iter()
        .find(|v| v.name == "temp")
        .expect("temp present")
        .decode_index;
    reader.decode_variable_values(idx).expect("decode")
}

fn expected() -> Vec<Option<f64>> {
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    oracle["temp"]["values"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            if x.is_null() {
                None
            } else {
                Some(x.as_f64().unwrap())
            }
        })
        .collect()
}

#[test]
fn classic_masks_fill_value_and_missing_value() {
    assert_eq!(decode_temp(CLASSIC), expected());
}

#[test]
fn hdf5_masks_fill_value_and_missing_value() {
    assert_eq!(decode_temp(NC4), expected());
}
