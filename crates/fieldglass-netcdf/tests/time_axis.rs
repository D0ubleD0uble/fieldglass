//! Which axis of a renderable variable is time (#170).
//!
//! The render panel animates along it, so the rule has to hold on every way a
//! committed file says "time": all three CF markers at once (ERSST, and the
//! NetCDF-4 dimension-scale fixture, whose time axis is unlimited and two steps
//! long), `units` alone (OISST states only `days since …`), `axis = "T"` on a
//! coordinate not named time (RTOFS's `MT`), and WRF's `Time`, which has no
//! coordinate variable at all.

use fieldglass_netcdf::NetcdfReader;

fn time_dims(bytes: &[u8]) -> Vec<(String, Option<usize>, Option<String>)> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("fixture opens");
    let view = reader.view().expect("view resolves");
    view.renderable_variables()
        .into_iter()
        .map(|v| {
            let name = v.detected_time_dim.map(|i| v.dims[i].name.clone());
            // Never an image axis.
            if let Some(t) = v.detected_time_dim {
                assert_ne!(Some(t), v.detected_y_dim, "{}", v.name);
                assert_ne!(Some(t), v.detected_x_dim, "{}", v.name);
            }
            (v.name, v.detected_time_dim, name)
        })
        .collect()
}

fn time_of(bytes: &[u8], variable: &str) -> Option<String> {
    time_dims(bytes)
        .into_iter()
        .find(|(name, _, _)| name == variable)
        .unwrap_or_else(|| panic!("{variable} is renderable"))
        .2
}

#[test]
fn every_cf_marker_and_the_wrf_name_find_the_time_axis() {
    let fixture = |name: &str| std::fs::read(format!("tests/fixtures/{name}")).expect(name);
    assert_eq!(
        time_of(&fixture("ersst_v5_187001_cdf1.nc"), "sst"),
        Some("time".into())
    );
    assert_eq!(
        time_of(&fixture("oisst_avhrr_v2.nc"), "sst"),
        Some("time".into())
    );
    assert_eq!(
        time_of(&fixture("rtofs_tripolar_arctic.nc"), "ice_coverage"),
        Some("MT".into())
    );
    assert_eq!(
        time_of(&fixture("wrf_lambert.nc"), "T2"),
        Some("Time".into())
    );

    // The multi-step one. Its `lat_bnds(lat, nv)` has no time axis to find.
    let dimscale = fixture("netcdf4_dimscale.nc");
    assert_eq!(time_of(&dimscale, "temperature"), Some("time".into()));
    assert_eq!(time_of(&dimscale, "lat_bnds"), None);
}

#[test]
fn a_variable_without_a_time_axis_reports_none() {
    let goes = std::fs::read("tests/fixtures/goes_geostationary.nc").expect("goes");
    for (name, time, _) in time_dims(&goes) {
        assert_eq!(time, None, "{name}");
    }
}
