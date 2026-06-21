//! End-to-end cross-check of CF data-variable unpacking (#184).
//!
//! `cf_packed_data.nc` stores a `temp(lat, lon)` field as scaled `int16` with
//! `scale_factor` / `add_offset`, a `_FillValue`, and a `valid_range` — the GOES
//! / MERRA-2 / ERA5 on-disk encoding. The sibling `*.oracle.json` records what
//! the canonical `netCDF4` library produces with auto mask+scale on (the CF
//! physical values), so reproducing it is a genuine cross-tool check.
//!
//! The decode returns raw packed codes with only `_FillValue` masked;
//! [`unpack_cf_data`] then applies the `valid_range` mask and the scale/offset.
//! Both stages are asserted against the oracle.

use fieldglass_netcdf::{DatasetView, NetcdfBacking, NetcdfReader, unpack_cf_data};
use serde_json::Value;

const NC: &[u8] = include_bytes!("fixtures/cf_packed_data.nc");
const ORACLE: &str = include_str!("fixtures/cf_packed_data.nc.oracle.json");

/// JSON array of nullable numbers → `Vec<Option<f64>>`.
fn nullable_f64s(v: &Value) -> Vec<Option<f64>> {
    v.as_array()
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
fn temp_decodes_raw_then_unpacks_to_physical_units() {
    let reader = NetcdfReader::from_bytes(NC.to_vec()).expect("parse");
    let view = match &reader.backing {
        NetcdfBacking::Classic(h) => DatasetView::from_classic(h),
        NetcdfBacking::Hdf5(_) => {
            DatasetView::from_hdf5(&reader.hdf5_metadata().expect("hdf5 metadata"))
        }
    };
    let temp = view
        .vars
        .iter()
        .find(|v| v.name == "temp")
        .expect("temp variable present");

    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let expected_raw = nullable_f64s(&oracle["temp"]["decoded_raw"]);
    let expected_physical = nullable_f64s(&oracle["temp"]["physical"]);

    // Stage 1: decode masks only `_FillValue`; valid_range codes survive.
    let decoded = reader
        .decode_variable_values(temp.decode_index)
        .expect("decode");
    assert_eq!(decoded, expected_raw, "raw decode (only _FillValue masked)");

    // Stage 2: unpacking masks valid_range (in packed units) and applies
    // scale_factor/add_offset — exactly libnetcdf's auto mask+scale.
    let physical = unpack_cf_data(&decoded, &temp.attrs);
    assert_eq!(physical, expected_physical, "CF physical units");
}
