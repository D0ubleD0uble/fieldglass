//! Value-decode cross-check for classic NetCDF (#108).
//!
//! For each bundled classic fixture we ship a sibling `*.values.json` oracle
//! produced by the canonical Unidata `netCDF4` library (see
//! `tools/regenerate-netcdf-oracles.py` and `fixtures/NOTICE.md`). This test
//! decodes every numeric variable with our pure-Rust reader and asserts it
//! reproduces the oracle's per-variable accounting: element count,
//! present/missing split against `_FillValue`, value statistics, and the
//! anchored raw samples in on-disk (C) order. Char variables hold text, not
//! numbers, and are covered by the header-parsing tests instead.
//!
//! The oracle is committed, so this test needs no `netCDF4` at runtime —
//! mirroring the eccodes snapshots on the GRIB side.

use fieldglass_netcdf::{ClassicHeader, NetcdfBacking, NetcdfReader};
use serde_json::Value;

struct Fixture {
    bytes: &'static [u8],
    oracle: &'static str,
}

const FIXTURES: &[Fixture] = &[
    Fixture {
        bytes: include_bytes!("fixtures/netcdf_classic_dummy.nc"),
        oracle: include_str!("fixtures/netcdf_classic_dummy.nc.values.json"),
    },
    Fixture {
        bytes: include_bytes!("fixtures/ersst_v5_187001_cdf1.nc"),
        oracle: include_str!("fixtures/ersst_v5_187001_cdf1.nc.values.json"),
    },
];

fn classic_header(reader: &NetcdfReader) -> &ClassicHeader {
    match &reader.backing {
        NetcdfBacking::Classic(h) => h,
        other => panic!("expected Classic backing, got {:?}", other.label()),
    }
}

fn var_index(header: &ClassicHeader, name: &str) -> usize {
    header
        .variables
        .iter()
        .position(|v| v.name == name)
        .unwrap_or_else(|| panic!("variable {name} present in header"))
}

/// Absolute + relative tolerance: samples and min/max are exact `f64` widenings
/// of the on-disk element (the oracle rounds min/max to 8 decimals), so a tiny
/// epsilon suffices. Means are looser — `netCDF4` accumulates float32 means in
/// float32 while we sum in `f64`.
fn approx(got: f64, want: f64, abs: f64) -> bool {
    if got == want {
        return true;
    }
    let diff = (got - want).abs();
    diff <= abs || diff <= want.abs() * 1e-6
}

#[test]
fn classic_variables_match_value_oracles() {
    for fixture in FIXTURES {
        let reader = NetcdfReader::from_bytes(fixture.bytes.to_vec()).expect("fixture parses");
        let header = classic_header(&reader);
        let oracle: Value = serde_json::from_str(fixture.oracle).expect("oracle parses");
        let vars = oracle["variables"]
            .as_object()
            .expect("oracle has a variables map");

        for (name, spec) in vars {
            // Char variables carry `text`, not numeric stats — skip; the
            // decoder rejects them and header parsing covers their content.
            if spec.get("text").is_some() {
                let idx = var_index(header, name);
                assert!(
                    reader.decode_variable_values(idx).is_err(),
                    "{name}: char variable value decode should be rejected"
                );
                continue;
            }

            let idx = var_index(header, name);
            let decoded = reader
                .decode_variable_values(idx)
                .unwrap_or_else(|e| panic!("{name}: decode failed: {e}"));

            // Element count.
            let want_count = spec["count"].as_u64().expect("count") as usize;
            assert_eq!(decoded.len(), want_count, "{name}: element count");

            // Present / missing split against `_FillValue`.
            let present = decoded.iter().filter(|v| v.is_some()).count();
            let missing = decoded.len() - present;
            assert_eq!(
                present,
                spec["present_count"].as_u64().unwrap() as usize,
                "{name}: present_count"
            );
            assert_eq!(
                missing,
                spec["missing_count"].as_u64().unwrap() as usize,
                "{name}: missing_count"
            );

            // Statistics over present values (the oracle omits these when the
            // variable is empty or fully masked).
            let present_vals: Vec<f64> = decoded.iter().filter_map(|v| *v).collect();
            if let Some(want_min) = spec.get("min").and_then(Value::as_f64) {
                let got_min = present_vals.iter().cloned().fold(f64::INFINITY, f64::min);
                assert!(
                    approx(got_min, want_min, 1e-5),
                    "{name}: min {got_min} vs oracle {want_min}"
                );
                let want_max = spec["max"].as_f64().unwrap();
                let got_max = present_vals
                    .iter()
                    .cloned()
                    .fold(f64::NEG_INFINITY, f64::max);
                assert!(
                    approx(got_max, want_max, 1e-5),
                    "{name}: max {got_max} vs oracle {want_max}"
                );
                let want_mean = spec["mean"].as_f64().unwrap();
                let got_mean = present_vals.iter().sum::<f64>() / present_vals.len() as f64;
                assert!(
                    approx(got_mean, want_mean, 2e-2),
                    "{name}: mean {got_mean} vs oracle {want_mean}"
                );
            }

            // Anchored raw samples (fills included). A masked position holds
            // the fill exactly, so `unwrap_or(fill)` reconstructs the on-disk
            // value the oracle recorded.
            let fill = header.variables[idx].fill_value().unwrap_or(f64::NAN);
            if let Some(samples) = spec.get("samples").and_then(Value::as_object) {
                for (i, want) in samples {
                    let i: usize = i.parse().expect("sample index");
                    let want = want.as_f64().expect("sample value");
                    let got = decoded[i].unwrap_or(fill);
                    assert!(
                        approx(got, want, 1e-9),
                        "{name}[{i}]: {got} vs oracle {want}"
                    );
                }
            }
        }
    }
}
