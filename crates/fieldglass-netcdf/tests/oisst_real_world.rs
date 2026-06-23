//! End-to-end parse of a *real* NOAA OISST v2.1 NetCDF-4 file (issue #123).
//!
//! `oisst_avhrr_v2.nc` is a tiny Hudson Bay window subset of a genuine
//! operational NOAA/NCEI Optimum Interpolation SST analysis (AVHRR, 2025-01-01)
//! from the public `noaa-cdr-sea-surface-temp-optimum-interpolation-pds` S3
//! archive — see `tests/fixtures/NOTICE.md` and
//! `tools/build_oisst_real_fixture.py`. It complements the geostationary GOES-16
//! fixture with a second real NetCDF-4 / HDF5 file that exercises a different
//! slice of the stack on genuine data:
//!
//! * a **regular 1/4° lat/lon** analysis grid (vs the GOES fixed scan grid),
//! * the HDF5 chunked + **deflate + shuffle** value path (GOES used deflate
//!   without shuffle),
//! * CF unpacking driven by scalar `valid_min` / `valid_max` (GOES used the
//!   two-element `valid_range`), over a real land/sea-ice fill mask, and
//! * a 4-D `(time, zlev, lat, lon)` variable with singleton `time` / `zlev`.
//!
//! The dense global-attribute storage (the fixture retains 25 attributes, well
//! past libhdf5's 8-attribute compact threshold) is the fractal-heap layout the
//! #33 robustness work hardened — asserted here by reading global attribute
//! values back from a real file. The oracle is the canonical `netCDF4`/libnetcdf
//! decode; the Rust reader must reproduce its masking, scaling, and statistics.

use fieldglass_netcdf::{DatasetView, NetcdfBacking, NetcdfReader, unpack_cf_data};
use serde_json::Value;

const OISST: &[u8] = include_bytes!("fixtures/oisst_avhrr_v2.nc");
const ORACLE: &str = include_str!("fixtures/oisst_avhrr_v2.nc.oracle.json");

fn view(bytes: &[u8]) -> (NetcdfReader, DatasetView) {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("parse OISST NetCDF-4");
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

fn oracle() -> Value {
    serde_json::from_str(ORACLE).unwrap()
}

/// The genuine OISST file is NetCDF-4 / HDF5, not classic, and its dense
/// (fractal-heap) global-attribute storage parses: the identity globals read
/// back with their real values, not silently empty.
#[test]
fn oisst_real_file_is_hdf5_backed_with_dense_global_attrs() {
    let (reader, view) = view(OISST);
    assert!(matches!(reader.backing, NetcdfBacking::Hdf5(_)));

    let want = oracle();
    let want = want["global_attrs"].as_object().unwrap();
    assert!(!want.is_empty(), "oracle records identity globals");
    for (k, v) in want {
        let got = view
            .global_attrs
            .iter()
            .find(|(name, _)| name == k)
            .map(|(_, val)| val.as_str());
        assert_eq!(got, v.as_str(), "global attr {k}");
    }
}

/// Dimension-scale resolution surfaces the real 4-D `(time, zlev, lat, lon)`
/// structure and the variables, and the `sst` CF packing attributes are
/// surfaced for the viewer.
#[test]
fn dimensions_and_variables_match_the_real_file() {
    let (_, view) = view(OISST);
    let oracle = oracle();

    let want_dims = oracle["dimensions"].as_object().unwrap();
    assert_eq!(view.dims.len(), want_dims.len(), "all dimensions resolved");
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

    let sst = var(&view, "sst");
    assert_eq!(sst.nc_type, fieldglass_netcdf::NcType::Short);
    let attr = |n: &str| {
        sst.attrs
            .iter()
            .find(|(k, _)| k == n)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(attr("units"), oracle["sst"]["units"].as_str());
    assert_eq!(attr("long_name"), oracle["sst"]["long_name"].as_str());
    // The CF packing attributes the unpack path keys on are all present.
    for k in [
        "scale_factor",
        "add_offset",
        "_FillValue",
        "valid_min",
        "valid_max",
    ] {
        assert!(attr(k).is_some(), "sst carries {k}");
    }
}

/// The `lat` / `lon` coordinate variables resolve to the real regular 1/4° grid.
#[test]
fn coordinates_are_regular_quarter_degree() {
    let (reader, view) = view(OISST);
    let g = &oracle()["grid"];

    let lat: Vec<f64> = decode_plane(&reader, &view, "lat")
        .into_iter()
        .flatten()
        .collect();
    let lon: Vec<f64> = decode_plane(&reader, &view, "lon")
        .into_iter()
        .flatten()
        .collect();

    let approx = |a: f64, b: f64| (a - b).abs() < 1e-4;
    assert!(
        approx(lat[0], g["lat0"].as_f64().unwrap()),
        "lat0 {}",
        lat[0]
    );
    assert!(
        approx(lon[0], g["lon0"].as_f64().unwrap()),
        "lon0 {}",
        lon[0]
    );
    assert!(
        approx(*lat.last().unwrap(), g["lat_last"].as_f64().unwrap()),
        "lat_last {}",
        lat.last().unwrap()
    );
    assert!(
        approx(*lon.last().unwrap(), g["lon_last"].as_f64().unwrap()),
        "lon_last {}",
        lon.last().unwrap()
    );
    // Uniform spacing across the whole axis.
    let (dlat, dlon) = (g["dlat"].as_f64().unwrap(), g["dlon"].as_f64().unwrap());
    for w in lat.windows(2) {
        assert!(approx(w[1] - w[0], dlat), "lat spacing {}", w[1] - w[0]);
    }
    for w in lon.windows(2) {
        assert!(approx(w[1] - w[0], dlon), "lon spacing {}", w[1] - w[0]);
    }
}

/// Decode each packed field through the chunked + deflate + shuffle HDF5 value
/// path and CF-unpack it, asserting the masking, scaling, and statistics match
/// the libnetcdf oracle exactly.
#[test]
fn packed_fields_decode_and_cf_unpack_to_oracle() {
    let (reader, view) = view(OISST);
    let oracle = oracle();

    for name in ["sst", "ice"] {
        let o = &oracle[name];
        let raw = decode_plane(&reader, &view, name);
        assert_eq!(raw.len(), 32 * 32, "{name}: full window decoded");

        let unpacked = unpack_cf_data(&raw, &var(&view, name).attrs);
        let present: Vec<f64> = unpacked.iter().flatten().copied().collect();
        assert_eq!(
            present.len() as u64,
            o["present_count"].as_u64().unwrap(),
            "{name}: present count (land / sea-ice fill mask)"
        );

        let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mean = present.iter().sum::<f64>() / present.len() as f64;
        assert!(
            (min - o["min"].as_f64().unwrap()).abs() < 1e-3,
            "{name} min {min}"
        );
        assert!(
            (max - o["max"].as_f64().unwrap()).abs() < 1e-3,
            "{name} max {max}"
        );
        assert!(
            (mean - o["mean"].as_f64().unwrap()).abs() < 1e-3,
            "{name} mean {mean}"
        );

        // Anchored per-index samples match the netCDF4 ground-truth decode.
        for (k, v) in o["scaled_samples"].as_object().unwrap() {
            let idx: usize = k.parse().unwrap();
            match (v.as_f64(), unpacked[idx]) {
                (Some(want), Some(got)) => assert!(
                    (got - want).abs() < 1e-3,
                    "{name}[{idx}] = {got}, oracle {want}"
                ),
                (None, got) => {
                    assert!(got.is_none(), "{name}[{idx}] expected missing, got {got:?}")
                }
                (Some(want), None) => panic!("{name}[{idx}] missing, oracle {want}"),
            }
        }
    }
}
