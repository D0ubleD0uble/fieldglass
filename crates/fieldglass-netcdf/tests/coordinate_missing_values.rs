//! A NetCDF coordinate value at any entry of its own `missing_value` is no
//! position (#1305).
//!
//! The reader's raw decode masks only the first entry of a multi-valued
//! `missing_value`, as `libnetcdf` does. CF placement reads a coordinate's
//! values by every fill value it states, so a latitude at the second entry
//! is refused rather than placed as a row at that latitude, as 0.6.1 did.
//!
//! The file is a minimal CDF-1 dataset built here and read through the real
//! reader: `t(lat, lon)` on its own `lat` and `lon` axes.

use fieldglass_core::array::ArraySource;
use fieldglass_core::cf::slice_placement;
use fieldglass_core::projection::GridGeometry;
use fieldglass_netcdf::{NetcdfArrays, NetcdfReader};

const NC_CHAR: u32 = 2;
const NC_DOUBLE: u32 = 6;
const NC_DIMENSION: u32 = 0x0A;
const NC_VARIABLE: u32 = 0x0B;
const NC_ATTRIBUTE: u32 = 0x0C;

fn u32_be(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn pad(out: &mut Vec<u8>) {
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn name(out: &mut Vec<u8>, s: &str) {
    u32_be(out, u32::try_from(s.len()).unwrap());
    out.extend_from_slice(s.as_bytes());
    pad(out);
}

/// An attribute: either text or doubles.
enum Value<'a> {
    Text(&'a str),
    Doubles(&'a [f64]),
}

fn attributes(out: &mut Vec<u8>, list: &[(&str, Value<'_>)]) {
    if list.is_empty() {
        u32_be(out, 0);
        u32_be(out, 0);
        return;
    }
    u32_be(out, NC_ATTRIBUTE);
    u32_be(out, u32::try_from(list.len()).unwrap());
    for (n, value) in list {
        name(out, n);
        match value {
            Value::Text(s) => {
                u32_be(out, NC_CHAR);
                u32_be(out, u32::try_from(s.len()).unwrap());
                out.extend_from_slice(s.as_bytes());
            }
            Value::Doubles(d) => {
                u32_be(out, NC_DOUBLE);
                u32_be(out, u32::try_from(d.len()).unwrap());
                for v in *d {
                    out.extend_from_slice(&v.to_be_bytes());
                }
            }
        }
        pad(out);
    }
}

/// One variable: its name, dimension ids, attributes and values.
type Var<'a> = (&'a str, &'a [u32], &'a [(&'a str, Value<'a>)], &'a [f64]);

/// `t(lat, lon)` on `lat` (whose `missing_value` is `missing`) and `lon`.
fn dataset(lats: &[f64], missing: &[f64]) -> Vec<u8> {
    let lons = [0.0, 90.0, 180.0, 270.0];
    let field: Vec<f64> = (0..lats.len() * lons.len()).map(|i| i as f64).collect();
    let lat_attrs = [
        ("units", Value::Text("degrees_north")),
        ("missing_value", Value::Doubles(missing)),
    ];
    let lon_attrs = [("units", Value::Text("degrees_east"))];
    let vars: [Var<'_>; 3] = [
        ("lat", &[0], &lat_attrs, lats),
        ("lon", &[1], &lon_attrs, &lons),
        ("t", &[0, 1], &[], &field),
    ];

    // The header with every `begin` at 0, to measure it; then again with
    // each variable's data placed after it.
    let header = |begins: &[u32]| {
        let mut out = b"CDF\x01".to_vec();
        u32_be(&mut out, 0); // numrecs
        u32_be(&mut out, NC_DIMENSION);
        u32_be(&mut out, 2);
        name(&mut out, "lat");
        u32_be(&mut out, u32::try_from(lats.len()).unwrap());
        name(&mut out, "lon");
        u32_be(&mut out, u32::try_from(lons.len()).unwrap());
        attributes(&mut out, &[]);
        u32_be(&mut out, NC_VARIABLE);
        u32_be(&mut out, u32::try_from(vars.len()).unwrap());
        for ((n, dims, attrs, values), begin) in vars.iter().zip(begins) {
            name(&mut out, n);
            u32_be(&mut out, u32::try_from(dims.len()).unwrap());
            for d in *dims {
                u32_be(&mut out, *d);
            }
            attributes(&mut out, attrs);
            u32_be(&mut out, NC_DOUBLE);
            u32_be(&mut out, u32::try_from(values.len() * 8).unwrap());
            u32_be(&mut out, *begin);
        }
        out
    };
    let mut begin = u32::try_from(header(&[0; 3]).len()).unwrap();
    let mut begins = [0; 3];
    for (slot, (_, _, _, values)) in begins.iter_mut().zip(&vars) {
        *slot = begin;
        begin += u32::try_from(values.len() * 8).unwrap();
    }
    let mut out = header(&begins);
    for (_, _, _, values) in &vars {
        for v in *values {
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
    out
}

fn arrays(bytes: Vec<u8>) -> NetcdfArrays {
    NetcdfArrays::open(NetcdfReader::from_bytes(bytes).expect("the file opens")).expect("a view")
}

/// `lat` as the reader reads it, before placement.
fn latitudes(arrays: &NetcdfArrays) -> Vec<Option<f64>> {
    arrays
        .read_region("lat", std::slice::from_ref(&(0..3)))
        .expect("lat reads")
}

#[test]
fn a_latitude_at_the_second_missing_value_is_refused() {
    let arrays = arrays(dataset(&[10.0, 20.0, 30.0], &[-1.0, 20.0]));
    // The reader masks the first entry only, so the raw read keeps 20.
    assert_eq!(latitudes(&arrays), [Some(10.0), Some(20.0), Some(30.0)]);
    let err = slice_placement(&arrays, "t", 0, 1).expect_err("refused");
    let message = err.to_string();
    assert!(
        message.contains("\"lat\"") && message.contains("a fill value at index 1"),
        "{message}"
    );
}

#[test]
fn a_latitude_at_neither_missing_value_is_placed() {
    let arrays = arrays(dataset(&[10.0, 20.0, 30.0], &[-1.0, 99.0]));
    let placement = slice_placement(&arrays, "t", 0, 1).expect("the slice resolves");
    let GridGeometry::LatLon(p) = placement.geometry else {
        panic!("placed: {:?}", placement.geometry);
    };
    assert_eq!((p.lat_first, p.lat_last), (10.0, 30.0), "{p:?}");
}

#[test]
fn a_latitude_at_the_first_missing_value_is_refused_as_before() {
    let arrays = arrays(dataset(&[10.0, 20.0, 30.0], &[20.0, -1.0]));
    assert_eq!(latitudes(&arrays), [Some(10.0), None, Some(30.0)]);
    let message = slice_placement(&arrays, "t", 0, 1)
        .expect_err("refused")
        .to_string();
    assert!(message.contains("a fill value at index 1"), "{message}");
}
