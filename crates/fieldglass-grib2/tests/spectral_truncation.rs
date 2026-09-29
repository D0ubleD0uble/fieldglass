//! A spectral field past what the 0.5° raster can carry (#637): the map is
//! band-limited to T359 and labelled, and a point evaluation is the full sum.
//!
//! The fixture declares T383 and its oracle is an independent pyshtools
//! synthesis of the coefficients eccodes decodes from it
//! (`tools/build_spectral_truncation_oracle.py`, provenance in
//! `fixtures/NOTICE.md`). The T63 render oracle beside it stays valid: T63 is
//! below the limit, so its map is the full sum as before.

use fieldglass_grib2::{GlobalGrid, Grib2Reader, SpectralTruncation};
use serde_json::Value;

const FIXTURE: &[u8] = include_bytes!("fixtures/spectral_simple_t383.grib2");
const ORACLE: &str = include_str!("fixtures/spectral_simple_t383.truncation.oracle.json");

fn oracle() -> Value {
    serde_json::from_str(ORACLE).expect("oracle parses")
}

fn floats(v: &Value) -> Vec<f64> {
    v.as_array()
        .expect("array")
        .iter()
        .map(|x| x.as_f64().expect("number"))
        .collect()
}

/// pyshtools and the Rust transform run the same sum in a different order over
/// ~74,000 terms of a field whose values reach ~500; they agree far closer than
/// this, and the removed band moves values by 5 to 18.
const TOLERANCE: f64 = 1e-7;

fn reader() -> Grib2Reader {
    Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("parse")
}

#[test]
fn the_map_is_the_field_band_limited_to_t359() {
    let reader = reader();
    let oracle = oracle();
    assert_eq!(reader.decode_spectral_message(0).expect("decodes").j, 383);

    let (grid, values) = reader.synthesize_spectral_global(0).expect("synthesise");
    assert_eq!(grid, GlobalGrid::FINEST);
    let (ni, nj) = (grid.ni, grid.nj);
    assert_eq!(values.len(), ni * nj);

    // Every 5° oracle point is a node of the 0.5° map: row 10·r, column 10·c.
    let lats = floats(&oracle["gridLats"]);
    let lons = floats(&oracle["gridLons"]);
    let want = floats(&oracle["map"]);
    let (axis_lats, axis_lons) = grid.axes();
    let mut worst = 0.0f64;
    for (r, &lat) in lats.iter().enumerate() {
        for (c, &lon) in lons.iter().enumerate() {
            let (row, col) = (10 * r, 10 * c);
            assert_eq!((axis_lats[row], axis_lons[col]), (lat, lon));
            let got = values[row * ni + col];
            let d = (got - want[r * lons.len() + c]).abs();
            worst = worst.max(d);
            assert!(d < TOLERANCE, "({lat}, {lon}): map {got}, oracle Δ={d}");
        }
    }
    eprintln!("map vs pyshtools T359: max |Δ| = {worst:.3e}");
}

#[test]
fn the_map_says_it_is_band_limited() {
    let reader = reader();
    assert_eq!(
        reader.synthesis_truncation(0),
        Some(SpectralTruncation {
            declared: 383,
            truncated_to: 359
        })
    );
    // The T63 field is below the limit and carries no label.
    let t63 =
        Grib2Reader::from_bytes(include_bytes!("fixtures/spectral_simple_t63.grib2").to_vec())
            .expect("parse");
    assert_eq!(t63.synthesis_truncation(0), None);
    // Nor does anything that is not spectral.
    let healpix =
        Grib2Reader::from_bytes(include_bytes!("fixtures/healpix_n4_ring.grib2").to_vec())
            .expect("parse");
    assert_eq!(healpix.synthesis_truncation(0), None);
}

/// The probe's half: at every oracle point, on the synthesis grid and off it,
/// the point evaluation is the full T383 sum and not the map's T359 one.
#[test]
fn a_point_evaluation_is_the_full_sum() {
    let reader = reader();
    let oracle = oracle();
    let mut worst = 0.0f64;
    for point in oracle["points"].as_array().expect("points") {
        let (lat, lon) = (
            point["lat"].as_f64().unwrap(),
            point["lon"].as_f64().unwrap(),
        );
        let (full, truncated) = (
            point["full"].as_f64().unwrap(),
            point["truncated"].as_f64().unwrap(),
        );
        let got = reader
            .evaluate_spectral_point(0, lat, lon)
            .expect("evaluate");
        let d = (got - full).abs();
        worst = worst.max(d);
        assert!(
            d < TOLERANCE,
            "({lat}, {lon}): {got} vs full {full} (Δ={d})"
        );
        // And the full sum is really a different number from the map's here.
        assert!((got - truncated).abs() > 1.0, "({lat}, {lon}) barely moved");
    }
    eprintln!("point evaluation vs pyshtools T383: max |Δ| = {worst:.3e}");
}

/// Below the limit, the point evaluation and the map agree to the last bit at
/// every grid node — the probe changes nothing where the map is exact.
#[test]
fn below_the_limit_a_point_evaluation_is_the_map() {
    let reader =
        Grib2Reader::from_bytes(include_bytes!("fixtures/spectral_simple_t63.grib2").to_vec())
            .expect("parse");
    let (grid, values) = reader.synthesize_spectral_global(0).expect("synthesise");
    let (lats, lons) = grid.axes();
    for (row, col) in [(0, 0), (0, 719), (180, 360), (97, 13), (360, 500)] {
        let point = reader
            .evaluate_spectral_point(0, lats[row], lons[col])
            .expect("evaluate");
        assert_eq!(
            point.to_bits(),
            values[row * grid.ni + col].to_bits(),
            "({row}, {col})"
        );
    }
}
