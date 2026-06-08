//! End-to-end decode coverage for simple packing (DRS template 5.0) against
//! the public fixtures bundled with the crate.
//!
//! The eta and ECMWF reduced-Gaussian fixtures are encoded with simple
//! packing. (The gfs.c255 fixture uses complex packing with spatial
//! differencing, template 5.3 — its decode is pinned to an eccodes oracle in
//! `decode_complex_spd.rs`.)
//!
//! Assertions pin the value count and a plausibility range for the
//! decoded field — strict numeric pins would couple the test to the
//! fixture's packing scheme, but a wide sanity envelope catches
//! regressions in R / E / D / bit-unpacking interactions.

use fieldglass_grib2::Grib2Reader;

const ETA_LAMBERT: &[u8] = include_bytes!("fixtures/eta_lambert_msg0.grib2");
const ECMWF_GAUSSIAN: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");
const REGULAR_LATLON: &[u8] = include_bytes!("fixtures/regular_latlon_surface.grib2");

#[test]
fn regular_latlon_2m_temperature_decodes_via_simple_packing() {
    // ECMWF 16×31 regular lat/lon 2-metre temperature, simple packing 5.0,
    // R ≈ 270 K. The compact size (1.2 KiB) makes it the canonical
    // end-to-end fixture for the §5–§7 path.
    let reader = Grib2Reader::from_bytes(REGULAR_LATLON.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 0, "simple packing");

    let (ni, nj) = msg.gds.dimensions().expect("lat/lon has dims");
    let expected = (ni as usize) * (nj as usize);
    let decoded = reader.decode_message_values(0).expect("decode");
    assert_eq!(decoded.len(), expected);
    assert!(
        decoded.iter().all(|v| v.is_some()),
        "no bitmap on this fixture"
    );

    let values: Vec<f64> = decoded.iter().map(|v| v.unwrap()).collect();
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    // 2-m temperature on a coarse global grid: realistic envelope is
    // roughly 220 K (cold poles) to 320 K (deserts). Generous bounds
    // catch a misdecoded byte while staying scheme-agnostic.
    assert!(
        (220.0..=325.0).contains(&min) && (220.0..=325.0).contains(&max),
        "2-m temperature should land in [220 K, 325 K], got {min}..{max}",
    );
    assert!(
        (max - min) > 1.0,
        "global temperature field should vary, got constant {min}",
    );
}

#[test]
fn eta_lambert_decodes_msl_pressure_via_simple_packing() {
    // First message of the Eta archive: 93×65 Lambert grid, MSL pressure
    // forecast at T+24h, simple packing R=97392 Pa, E=0, D=0, 13 bits/value.
    let reader = Grib2Reader::from_bytes(ETA_LAMBERT.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 0, "Eta uses simple packing");

    let (ni, nj) = msg.gds.dimensions().expect("Lambert has dims");
    let expected = (ni as usize) * (nj as usize);

    let decoded = reader.decode_message_values(0).expect("decode");
    assert_eq!(decoded.len(), expected, "one value per grid point");
    assert!(
        decoded.iter().all(|v| v.is_some()),
        "no bitmap → all points present",
    );

    // MSL pressure in Pa: realistic surface pressure lies in ~95000–105000
    // Pa for an Eta CONUS slice. Keep the envelope generous — packing
    // parameter variance across centres can widen the realised range.
    let values: Vec<f64> = decoded.iter().map(|v| v.unwrap()).collect();
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        (90_000.0..=110_000.0).contains(&min) && (90_000.0..=110_000.0).contains(&max),
        "MSL pressure should land in atmospheric range, got {min}..{max}",
    );
    assert!(
        (max - min).abs() > 1.0,
        "decoded field should vary across the grid, got constant {min}",
    );
}

#[test]
fn ecmwf_reduced_gaussian_decode_unsupported_for_reduced_grids() {
    // Reduced Gaussian grids carry per-row Ni in §3's optional list — the
    // GDS reports no dimensions, so decode rejects before walking §7.
    // (The byte-level path is still simple packing; this test pins the
    // reader's behaviour for the "no dims" branch.)
    let reader = Grib2Reader::from_bytes(ECMWF_GAUSSIAN.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.drs.template_number, 0, "ECMWF uses simple packing");
    assert!(
        msg.gds.dimensions().is_none(),
        "reduced Gaussian has no constant Ni",
    );
    let err = reader.decode_message_values(0).expect_err("must reject");
    assert!(
        err.to_string().contains("no declared dimensions"),
        "decode names the missing-dims path, got: {err}",
    );
}

#[test]
fn decode_out_of_range_index_errors() {
    let reader = Grib2Reader::from_bytes(ETA_LAMBERT.to_vec()).expect("parse");
    assert!(reader.decode_message_values(99).is_err());
}
