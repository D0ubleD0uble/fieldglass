//! End-to-end parse of a *real* GOES-16 ABI NetCDF-4 file (issue #123).
//!
//! `goes16_abi_cmip.nc` is a tiny center-window subset of a genuine operational
//! GOES-16 ABI L2 Cloud & Moisture Imagery file (band 13, 10.3 µm IR) from the
//! public `noaa-goes16` S3 bucket — see `tests/fixtures/NOTICE.md` and
//! `tools/build_goes_real_fixture.py`. It is the first real NetCDF-4 / HDF5 file
//! in the corpus, so it exercises the whole stack on genuine data: the HDF5
//! object-header + dimension-scale resolution, a CF `geostationary` grid mapping
//! with the real GRS80 / sub-satellite-longitude parameters, scaled `int16`
//! `x` / `y` scan-angle coordinates, and the chunked + deflate `CMI`
//! brightness-temperature field with CF `scale_factor` / `add_offset` /
//! `valid_range`.
//!
//! Unlike the synthetic `goes_geostationary.nc` (whose geometry is generated to
//! cross-check the projector), the facts asserted here come from the genuine
//! file; the geolocation oracle is still an *independent* NumPy transcription of
//! the GOES-R PUG fixed-grid algorithm, so the Rust projector reproducing it is
//! a real cross-language check.

use fieldglass_core::{GeostationaryParams, GeostationaryProjector};
use fieldglass_netcdf::{
    DatasetView, NetcdfBacking, NetcdfReader, apply_scale_offset, resolve_cf_geostationary,
    unpack_cf_data,
};
use serde_json::Value;

const GOES: &[u8] = include_bytes!("fixtures/goes16_abi_cmip.nc");
const ORACLE: &str = include_str!("fixtures/goes16_abi_cmip.nc.oracle.json");

fn view(bytes: &[u8]) -> (NetcdfReader, DatasetView) {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("parse GOES NetCDF-4");
    let view = match &reader.backing {
        NetcdfBacking::Hdf5(_) => {
            DatasetView::from_hdf5(&reader.hdf5_metadata().expect("hdf5 metadata"))
        }
        other => panic!("expected HDF5 backing, got {:?}", other.label()),
    };
    (reader, view)
}

fn var<'a>(view: &'a DatasetView, name: &str) -> &'a fieldglass_netcdf::VarView {
    view.vars
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("{name} present"))
}

fn decode_plane(reader: &NetcdfReader, view: &DatasetView, name: &str) -> Vec<Option<f64>> {
    reader
        .decode_variable_values(var(view, name).decode_index)
        .expect("decode")
}

/// The genuine GOES-16 file is NetCDF-4 / HDF5, not classic.
#[test]
fn goes_real_file_is_hdf5_backed() {
    let (reader, _) = view(GOES);
    assert!(matches!(reader.backing, NetcdfBacking::Hdf5(_)));
}

/// Dimension-scale resolution surfaces the real dimensions and variables, the
/// same way the classic path does — the closing criterion of the #33 chain, on
/// a real file.
#[test]
fn dimensions_and_variables_match_the_real_file() {
    let (_, view) = view(GOES);
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();

    for d in view.dims.iter() {
        let want = oracle["dimensions"][&d.name].as_u64().unwrap();
        assert_eq!(d.length, want, "dim {} length", d.name);
    }
    let names: std::collections::BTreeSet<&str> =
        view.vars.iter().map(|v| v.name.as_str()).collect();
    for v in oracle["variables"].as_array().unwrap() {
        let want = v.as_str().unwrap();
        assert!(names.contains(want), "variable {want} present in view");
    }

    // CMI carries the real CF data-packing attributes, surfaced for the viewer.
    let cmi = var(&view, "CMI");
    assert_eq!(cmi.nc_type, fieldglass_netcdf::NcType::Short);
    let attr = |n: &str| {
        cmi.attrs
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(attr("units"), Some("K"));
    assert_eq!(attr("grid_mapping"), Some("goes_imager_projection"));
}

/// The geostationary projection resolves from the real `goes_imager_projection`
/// grid mapping and the scaled `x` / `y` coordinates, and reproduces the
/// independent geolocation oracle.
#[test]
fn geostationary_projection_reproduces_oracle_geolocation() {
    let (reader, view) = view(GOES);
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();

    let gm_attrs = var(&view, "goes_imager_projection").attrs.clone();
    let read_scaled = |name: &str| {
        let raw: Vec<f64> = decode_plane(&reader, &view, name)
            .into_iter()
            .flatten()
            .collect();
        apply_scale_offset(&raw, &var(&view, name).attrs)
    };
    let x = read_scaled("x");
    let y = read_scaled("y");
    let g = resolve_cf_geostationary(&gm_attrs, &x, &y).expect("geostationary resolves");

    // Resolved parameters are the genuine GOES-16 (GOES-East) values.
    assert!(
        (g.sub_lon_deg - oracle["longitude_of_projection_origin"].as_f64().unwrap()).abs() < 1e-9
    );
    assert!((g.h_metres - oracle["h_metres"].as_f64().unwrap()).abs() < 1e-3);
    assert!((g.r_eq - oracle["semi_major_axis"].as_f64().unwrap()).abs() < 1e-6);
    assert!((g.r_pol - oracle["semi_minor_axis"].as_f64().unwrap()).abs() < 1e-6);
    assert!(g.sweep_x, "GOES sweep axis is x");

    let proj = GeostationaryProjector::new(GeostationaryParams {
        ni: g.ni,
        nj: g.nj,
        h_metres: g.h_metres,
        r_eq: g.r_eq,
        r_pol: g.r_pol,
        sub_lon_deg: g.sub_lon_deg,
        sweep_x: g.sweep_x,
        x0: g.x0,
        dx_rad: g.dx_rad,
        y0: g.y0,
        dy_rad: g.dy_rad,
    });

    for s in oracle["geolocation_samples"].as_array().unwrap() {
        let (i, j) = (
            s["i"].as_u64().unwrap() as f64,
            s["j"].as_u64().unwrap() as f64,
        );
        let (olat, olon) = (s["lat"].as_f64().unwrap(), s["lon"].as_f64().unwrap());
        let xs = g.x0 + i * g.dx_rad;
        let ys = g.y0 + j * g.dy_rad;
        let (lat, lon) = proj
            .scan_to_lonlat(xs, ys)
            .unwrap_or_else(|| panic!("pixel ({i},{j}) on disk"));
        // Both sides feed the identical scan angles into the GOES-R PUG formula
        // (the oracle is an independent NumPy transcription), so agreement is to
        // floating-point precision, not pixel resolution.
        assert!(
            (lat - olat).abs() < 1e-6 && (lon - olon).abs() < 1e-6,
            "scan ({xs},{ys}) → ({lat},{lon}), oracle ({olat},{olon})",
        );
    }
}

/// The chunked + deflate `CMI` field decodes through the HDF5 value path on a
/// real file, and CF unpacking turns the raw `int16` codes into the oracle
/// brightness temperatures (Kelvin).
#[test]
fn cmi_chunked_deflate_field_decodes_to_brightness_temperature() {
    let (reader, view) = view(GOES);
    let oracle: Value = serde_json::from_str(ORACLE).unwrap();
    let cmi_o = &oracle["cmi"];

    let raw = decode_plane(&reader, &view, "CMI");
    assert_eq!(raw.len(), 24 * 24, "full window decoded");

    let unpacked = unpack_cf_data(&raw, &var(&view, "CMI").attrs);
    let present: Vec<f64> = unpacked.iter().flatten().copied().collect();
    assert_eq!(
        present.len() as u64,
        cmi_o["present_count"].as_u64().unwrap(),
        "all in-range pixels present"
    );

    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (min - cmi_o["min_k"].as_f64().unwrap()).abs() < 1e-3,
        "min K {min}"
    );
    assert!(
        (max - cmi_o["max_k"].as_f64().unwrap()).abs() < 1e-3,
        "max K {max}"
    );

    // Anchored per-index samples match the netCDF4 ground-truth decode exactly.
    for (k, v) in cmi_o["scaled_samples"].as_object().unwrap() {
        let idx: usize = k.parse().unwrap();
        match (v.as_f64(), unpacked[idx]) {
            (Some(want), Some(got)) => assert!(
                (got - want).abs() < 1e-3,
                "CMI[{idx}] = {got} K, oracle {want} K"
            ),
            (None, got) => assert!(got.is_none(), "CMI[{idx}] expected missing, got {got:?}"),
            (Some(want), None) => panic!("CMI[{idx}] missing, oracle {want} K"),
        }
    }
}
