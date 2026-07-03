//! Snapshot-based cross-check of `fieldglass-grib2` against eccodes.
//!
//! For each fixture under `tests/fixtures/`, we ship a sibling
//! `.eccodes.ref.json` that captures the output of `grib_dump -j` for a
//! curated subset of WMO keys. This test loads each snapshot, decodes the
//! fixture with our parser, and asserts that the two agree on every key
//! present in the snapshot.
//!
//! The snapshots are checked into git so this test has zero runtime
//! dependencies — eccodes is only required when regenerating snapshots
//! via `tools/regenerate-eccodes-snapshots.py` (typically after upgrading
//! eccodes or adding a new fixture).
//!
//! The key-to-parser-field mapping is intentionally explicit in
//! [`assert_message_matches`]: when eccodes adds a new key we want surfaced,
//! add it to both `CURATED_KEYS` (in the regen script) and the dispatch
//! match here.

use fieldglass_grib2::{Grib2Reader, GridTemplate, parse_bit_map};
use serde_json::Value;
use std::path::Path;

/// Float tolerance for fields encoded as scaled integers by GRIB2 but
/// emitted as decimals by eccodes (e.g. 2.5° increments stored as
/// 2_500_000 μ°). 1e-3 absorbs any rounding eccodes applies; mismatches
/// here mean a real scale-factor bug.
const FLOAT_EPS: f64 = 1e-3;

fn snapshot_for(fixture: &str) -> Value {
    let path = Path::new("tests/fixtures").join(format!("{fixture}.eccodes.ref.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read snapshot {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse snapshot {}: {e}", path.display()))
}

fn assert_message_matches(
    fixture: &str,
    msg: &fieldglass_grib2::Grib2Message,
    snap: &serde_json::Map<String, Value>,
    raw_file_bytes: &[u8],
) {
    for (key, expected) in snap {
        // `null` in the snapshot means eccodes itself omitted the field —
        // skip the comparison rather than asserting on a missing value.
        if expected.is_null() {
            continue;
        }
        let pinned = match key.as_str() {
            // §0 Indicator
            "discipline" => check_u64(key, expected, msg.is.discipline as u64),
            "editionNumber" => check_u64(key, expected, msg.is.edition as u64),
            "totalLength" => check_u64(key, expected, msg.is.total_length),

            // §1 Identification
            "centre" => check_u64(key, expected, msg.ids.centre as u64),
            "subCentre" => check_u64(key, expected, msg.ids.sub_centre as u64),
            "significanceOfReferenceTime" => {
                check_u64(key, expected, msg.ids.reference_time_significance as u64)
            }
            "dataDate" => check_u64(
                key,
                expected,
                msg.ids.year as u64 * 10_000 + msg.ids.month as u64 * 100 + msg.ids.day as u64,
            ),
            "dataTime" => check_u64(
                key,
                expected,
                msg.ids.hour as u64 * 100 + msg.ids.minute as u64,
            ),
            "productionStatusOfProcessedData" => {
                check_u64(key, expected, msg.ids.production_status as u64)
            }
            "typeOfProcessedData" => check_u64(key, expected, msg.ids.data_type as u64),

            // §3 Grid Definition
            "gridDefinitionTemplateNumber" => {
                check_u64(key, expected, msg.gds.template_number as u64)
            }
            "shapeOfTheEarth" => match &msg.gds.template {
                GridTemplate::LatLon(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::RotatedLatLon(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::Mercator(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::PolarStereographic(t) => {
                    check_u64(key, expected, t.shape_of_earth as u64)
                }
                GridTemplate::Lambert(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::Gaussian(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::SpaceView(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::Unsupported(_) => true, // can't check
            },
            "numberOfDataPoints" => check_u64(key, expected, msg.gds.num_data_points as u64),
            "Ni" => match msg.gds.dimensions() {
                Some((ni, _)) => check_u64(key, expected, ni as u64),
                None => true, // reduced grid — eccodes also emits null here
            },
            "Nj" => match msg.gds.dimensions() {
                Some((_, nj)) => check_u64(key, expected, nj as u64),
                None => match &msg.gds.template {
                    GridTemplate::Gaussian(t) => check_u64(key, expected, t.nj as u64),
                    _ => true,
                },
            },
            "latitudeOfFirstGridPointInDegrees" => match msg.gds.bounds() {
                Some((la1, _, _, _)) => check_f64(key, expected, la1),
                None => true,
            },
            "longitudeOfFirstGridPointInDegrees" => match msg.gds.bounds() {
                Some((_, lo1, _, _)) => check_f64(key, expected, lo1),
                None => true,
            },
            "latitudeOfLastGridPointInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => check_f64(key, expected, t.la2),
                GridTemplate::RotatedLatLon(t) => check_f64(key, expected, t.la2),
                GridTemplate::Gaussian(t) => check_f64(key, expected, t.la2),
                _ => true,
            },
            "longitudeOfLastGridPointInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => check_f64(key, expected, t.lo2),
                GridTemplate::RotatedLatLon(t) => check_f64(key, expected, t.lo2),
                GridTemplate::Gaussian(t) => check_f64(key, expected, t.lo2),
                _ => true,
            },
            "iDirectionIncrementInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => match t.di {
                    Some(di) => check_f64(key, expected, di),
                    None => true,
                },
                GridTemplate::RotatedLatLon(t) => match t.di {
                    Some(di) => check_f64(key, expected, di),
                    None => true,
                },
                GridTemplate::Gaussian(t) => match t.di {
                    Some(di) => check_f64(key, expected, di),
                    None => true,
                },
                _ => true,
            },
            "jDirectionIncrementInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => match t.dj {
                    Some(dj) => check_f64(key, expected, dj),
                    None => true,
                },
                GridTemplate::RotatedLatLon(t) => match t.dj {
                    Some(dj) => check_f64(key, expected, dj),
                    None => true,
                },
                _ => true,
            },

            // §4 Product Definition
            "productDefinitionTemplateNumber" => {
                check_u64(key, expected, msg.pds.template_number as u64)
            }
            "parameterCategory" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.parameter_category as u64),
                None => true,
            },
            "parameterNumber" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.parameter_number as u64),
                None => true,
            },
            "typeOfGeneratingProcess" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.generating_process_type as u64),
                None => true,
            },
            "indicatorOfUnitOfTimeRange" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.forecast_time_unit as u64),
                None => true,
            },
            "forecastTime" => match msg.pds.common() {
                Some(c) => check_i64(key, expected, c.forecast_time),
                None => true,
            },
            "typeOfFirstFixedSurface" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.first_surface.surface_type as u64),
                None => true,
            },
            "scaleFactorOfFirstFixedSurface" => match msg.pds.common() {
                Some(c) => check_i64(
                    key,
                    expected,
                    c.first_surface.scale_factor.unwrap_or(0) as i64,
                ),
                None => true,
            },
            "scaledValueOfFirstFixedSurface" => match msg.pds.common() {
                Some(c) => check_i64(key, expected, c.first_surface.scaled_value.unwrap_or(0)),
                None => true,
            },

            // §5 Data Representation
            "dataRepresentationTemplateNumber" => {
                check_u64(key, expected, msg.drs.template_number as u64)
            }
            "referenceValue" => match msg.drs.simple() {
                Some(t) => check_f64(key, expected, t.reference_value as f64),
                None => true,
            },
            "binaryScaleFactor" => match msg.drs.simple() {
                Some(t) => check_i64(key, expected, t.binary_scale_factor as i64),
                None => true,
            },
            "decimalScaleFactor" => match msg.drs.simple() {
                Some(t) => check_i64(key, expected, t.decimal_scale_factor as i64),
                None => true,
            },
            "bitsPerValue" => match msg.drs.simple() {
                Some(t) => check_u64(key, expected, t.bits_per_value as u64),
                None => true,
            },

            // §6 Bit-Map
            "bitMapIndicator" => {
                let (start, end) = msg.bms_range;
                // grid_points argument is only used by the inline-bitmap
                // branch, and that path needs an accurate count; use the
                // GDS-declared num_data_points so this works for every
                // template.
                let grid_points = msg.gds.num_data_points as usize;
                let bms =
                    parse_bit_map(&raw_file_bytes[start..end], grid_points).expect("BMS parse");
                check_u64(key, expected, bms.indicator as u64)
            }

            unknown => panic!(
                "{fixture}: snapshot has key {unknown:?} with no parser-field mapping; \
                 update assert_message_matches in eccodes_reference.rs",
            ),
        };
        assert!(pinned, "{fixture}: key {key:?} mismatch");
    }
}

fn check_u64(key: &str, expected: &Value, actual: u64) -> bool {
    let exp = expected
        .as_u64()
        .or_else(|| expected.as_i64().map(|i| i as u64))
        .unwrap_or_else(|| panic!("snapshot {key:?} is not an integer: {expected}"));
    if exp != actual {
        eprintln!("key {key}: eccodes={exp}, parser={actual}");
        return false;
    }
    true
}

fn check_i64(key: &str, expected: &Value, actual: i64) -> bool {
    let exp = expected
        .as_i64()
        .unwrap_or_else(|| panic!("snapshot {key:?} is not an integer: {expected}"));
    if exp != actual {
        eprintln!("key {key}: eccodes={exp}, parser={actual}");
        return false;
    }
    true
}

fn check_f64(key: &str, expected: &Value, actual: f64) -> bool {
    let exp = expected
        .as_f64()
        .unwrap_or_else(|| panic!("snapshot {key:?} is not a number: {expected}"));
    if (exp - actual).abs() > FLOAT_EPS {
        eprintln!(
            "key {key}: eccodes={exp}, parser={actual}, diff={}",
            (exp - actual).abs()
        );
        return false;
    }
    true
}

fn assert_fixture_matches_snapshot(fixture: &str, bytes: &[u8]) {
    let reader = Grib2Reader::from_bytes(bytes.to_vec())
        .unwrap_or_else(|e| panic!("{fixture}: parse failed: {e}"));
    let snap = snapshot_for(fixture);
    let msgs = snap["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("{fixture}: snapshot has no `messages` array"));
    assert_eq!(
        msgs.len(),
        reader.messages.len(),
        "{fixture}: message count mismatch (snapshot={}, parser={})",
        msgs.len(),
        reader.messages.len(),
    );
    for (i, msg_snap) in msgs.iter().enumerate() {
        let snap_obj = msg_snap
            .as_object()
            .unwrap_or_else(|| panic!("{fixture}: snapshot message {i} is not an object"));
        assert_message_matches(fixture, &reader.messages[i], snap_obj, bytes);
    }
}

#[test]
fn regular_latlon_surface_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "regular_latlon_surface.grib2",
        include_bytes!("fixtures/regular_latlon_surface.grib2"),
    );
}

#[test]
fn gfs_c255_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "gfs_c255_latlon.grib2",
        include_bytes!("fixtures/gfs_c255_latlon.grib2"),
    );
}

#[test]
fn eta_lambert_msg0_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "eta_lambert_msg0.grib2",
        include_bytes!("fixtures/eta_lambert_msg0.grib2"),
    );
}

#[test]
fn reduced_gaussian_pressure_level_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "reduced_gaussian_pressure_level.grib2",
        include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2"),
    );
}

#[test]
fn rotated_latlon_surface_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "rotated_latlon_surface.grib2",
        include_bytes!("fixtures/rotated_latlon_surface.grib2"),
    );
}

#[test]
fn polar_stereographic_surface_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "polar_stereographic_surface.grib2",
        include_bytes!("fixtures/polar_stereographic_surface.grib2"),
    );
}

// §5 packing fixtures: these cross-check that §0–§4 metadata and the §5
// template number parse for templates 5.2 / 5.3 / 5.40 / 5.41 / 5.42. The
// value decode for each is pinned to the sibling `*_expected.json` oracle in
// the matching `decode_*.rs` test (complex → `decode_complex.rs` /
// `decode_complex_spd.rs`; JPEG 2000 5.40 → `decode_jpeg2000.rs`; PNG 5.41 →
// `decode_png.rs`; CCSDS 5.42 → `decode_ccsds.rs`).
#[test]
fn complex_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "complex_regular_latlon.grib2",
        include_bytes!("fixtures/complex_regular_latlon.grib2"),
    );
}

#[test]
fn complex_spd2_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "complex_spd2_regular_latlon.grib2",
        include_bytes!("fixtures/complex_spd2_regular_latlon.grib2"),
    );
}

// Missing-value management / row-by-row splitting fixtures (#217); value
// decode pinned in `decode_complex_missing.rs`.
#[test]
fn complex_mvm1_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "complex_mvm1_regular_latlon.grib2",
        include_bytes!("fixtures/complex_mvm1_regular_latlon.grib2"),
    );
}

#[test]
fn complex_spd2_mvm1_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "complex_spd2_mvm1_regular_latlon.grib2",
        include_bytes!("fixtures/complex_spd2_mvm1_regular_latlon.grib2"),
    );
}

#[test]
fn complex_mvm2_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "complex_mvm2_regular_latlon.grib2",
        include_bytes!("fixtures/complex_mvm2_regular_latlon.grib2"),
    );
}

#[test]
fn complex_rowbyrow_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "complex_rowbyrow_regular_latlon.grib2",
        include_bytes!("fixtures/complex_rowbyrow_regular_latlon.grib2"),
    );
}

#[test]
fn jpeg2000_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "jpeg2000_regular_latlon.grib2",
        include_bytes!("fixtures/jpeg2000_regular_latlon.grib2"),
    );
}

#[test]
fn png_eta_lambert_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "png_eta_lambert.grib2",
        include_bytes!("fixtures/png_eta_lambert.grib2"),
    );
}

#[test]
fn ccsds_regular_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "ccsds_regular_latlon.grib2",
        include_bytes!("fixtures/ccsds_regular_latlon.grib2"),
    );
}

#[test]
fn ccsds_regular_latlon_8bit_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "ccsds_regular_latlon_8bit.grib2",
        include_bytes!("fixtures/ccsds_regular_latlon_8bit.grib2"),
    );
}

#[test]
fn ccsds_regular_latlon_24bit_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "ccsds_regular_latlon_24bit.grib2",
        include_bytes!("fixtures/ccsds_regular_latlon_24bit.grib2"),
    );
}

/// The real-model fixtures below are ~1 MB each — two orders of magnitude
/// larger than the re-encoded ones — so read them at runtime rather than
/// embedding them in the test binary with `include_bytes!`.
fn read_fixture(fixture: &str) -> Vec<u8> {
    let path = Path::new("tests/fixtures").join(fixture);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

#[test]
fn hrrr_complex_spd_lambert_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "hrrr_complex_spd_lambert.grib2",
        &read_fixture("hrrr_complex_spd_lambert.grib2"),
    );
}

#[test]
fn ecmwf_ccsds_latlon_matches_eccodes() {
    assert_fixture_matches_snapshot(
        "ecmwf_ccsds_latlon.grib2",
        &read_fixture("ecmwf_ccsds_latlon.grib2"),
    );
}
