//! Integration coverage for §4 PDS template parsing.
//!
//! Template 4.0 is exercised against three real-world fixtures (NCEP GFS,
//! NCEP Eta, ECMWF reduced-Gaussian). Templates 4.8 and 4.11 don't ship in
//! the eccodes public test corpus at a size we can bundle (gep10's ensemble
//! sample is 1.5 MB and contains only 4.1/4.11; deterministic 4.8 samples
//! are similarly large), so we exercise the full reader walk through them
//! via in-memory synthesized messages. The per-template byte layout is
//! still covered by the unit tests in `src/pds.rs`; these integration tests
//! validate that the IS → IDS → GDS → PDS hand-off in `Grib2Reader` works
//! cleanly for the time-interval and ensemble templates as well.

use fieldglass_grib2::{
    Grib2Reader, ProductTemplate, lookup_fixed_surface, lookup_generating_process_type,
    lookup_parameter, lookup_statistical_process, lookup_time_range_unit,
};

const GFS_LATLON: &[u8] = include_bytes!("fixtures/gfs_c255_latlon.grib2");
const ETA_LAMBERT: &[u8] = include_bytes!("fixtures/eta_lambert_msg0.grib2");
const ECMWF_GAUSSIAN: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

#[test]
fn gfs_latlon_pds_template_4_0_decodes() {
    let reader = Grib2Reader::from_bytes(GFS_LATLON.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.pds.template_number, 0);
    let common = msg.pds.common().expect("4.0 has common");
    assert_eq!(msg.is.discipline, 0);

    // Pin the exact triple from this fixture: GFS T+204 pressure forecast at
    // an NCEP local-use surface (242). Spec-defined fields (parameter +
    // forecast time + generating process) come from WMO tables; the surface
    // code falls in the 192..=254 local-use range.
    assert_eq!(common.parameter_category, 3); // mass
    assert_eq!(common.parameter_number, 0); // PRES
    assert_eq!(
        lookup_parameter(
            msg.is.discipline,
            common.parameter_category,
            common.parameter_number
        ),
        Some(("PRES", "Pressure", "Pa")),
    );
    assert_eq!(common.forecast_time, 204);
    assert_eq!(lookup_time_range_unit(common.forecast_time_unit), "Hour");
    assert_eq!(
        lookup_generating_process_type(common.generating_process_type),
        "Forecast"
    );
    assert_eq!(
        lookup_fixed_surface(common.first_surface.surface_type),
        "Reserved for local use",
    );
}

#[test]
fn eta_lambert_pds_template_4_0_decodes() {
    let reader = Grib2Reader::from_bytes(ETA_LAMBERT.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.pds.template_number, 0);
    assert!(matches!(
        msg.pds.template,
        ProductTemplate::HorizontalAnalysisForecast(_)
    ));
    let common = msg.pds.common().expect("4.0 has common");

    // First message of the Eta archive is a T+24 mean-sea-level pressure
    // forecast. Parameter (3, 192) is an NCEP-local mass field that our
    // curated table does not cover; surface 101 is the WMO MSL code.
    assert_eq!(msg.is.discipline, 0);
    assert_eq!(common.forecast_time, 24);
    assert_eq!(lookup_time_range_unit(common.forecast_time_unit), "Hour");
    assert_eq!(
        lookup_generating_process_type(common.generating_process_type),
        "Forecast"
    );
    assert_eq!(
        lookup_fixed_surface(common.first_surface.surface_type),
        "Mean sea level",
    );
}

#[test]
fn ecmwf_reduced_gaussian_pds_template_4_0_decodes() {
    let reader = Grib2Reader::from_bytes(ECMWF_GAUSSIAN.to_vec()).expect("parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.pds.template_number, 0);
    let common = msg.pds.common().expect("4.0 has common");

    // ECMWF reduced-Gaussian pressure-level analysis of 1000 hPa temperature.
    assert_eq!(msg.is.discipline, 0);
    assert_eq!(
        lookup_parameter(
            msg.is.discipline,
            common.parameter_category,
            common.parameter_number
        ),
        Some(("TMP", "Temperature", "K")),
    );
    assert_eq!(common.forecast_time, 0);
    assert_eq!(
        lookup_generating_process_type(common.generating_process_type),
        "Analysis"
    );
    assert_eq!(common.first_surface.surface_type, 100); // isobaric
    let pressure_pa = common.first_surface.value().expect("scaled pressure");
    assert!(
        (pressure_pa - 100_000.0).abs() < 1e-6,
        "expected 1000 hPa = 100000 Pa, got {pressure_pa}",
    );
}

// ---------------------------------------------------------------------------
// Synthesized end-to-end coverage for templates 4.8 and 4.11.
// ---------------------------------------------------------------------------

fn push_be(buf: &mut Vec<u8>, v: u64, width: usize) {
    let bytes = v.to_be_bytes();
    buf.extend_from_slice(&bytes[(8 - width)..]);
}

/// Build a minimal IS + IDS + §3 GDS prefix for a synthesized message,
/// returning the buffer with the cursor positioned where §4 PDS goes.
/// `pds_len` lets the caller pre-allocate the IS total-length field
/// correctly; the PDS bytes are appended by the caller, then this helper's
/// companion [`append_minimal_drs_bms_ds`] tail finishes the message
/// before the caller writes "7777".
fn build_message_prefix(pds_len: u32) -> Vec<u8> {
    let ids_len: u32 = 21;
    let gds_len: u32 = 72;
    let drs_len: u32 = 21;
    let bms_len: u32 = 6;
    let ds_len: u32 = 6;
    let total_len: u64 = 16
        + ids_len as u64
        + gds_len as u64
        + pds_len as u64
        + drs_len as u64
        + bms_len as u64
        + ds_len as u64
        + 4;
    let mut buf = Vec::with_capacity(total_len as usize);

    // IS (16 bytes): GRIB | reserved | discipline=0 | edition=2 | total_length(8)
    buf.extend_from_slice(b"GRIB");
    buf.extend_from_slice(&[0, 0]);
    buf.push(0); // discipline = meteorological products
    buf.push(2); // edition
    buf.extend_from_slice(&total_len.to_be_bytes());

    // IDS (21 bytes)
    buf.extend_from_slice(&ids_len.to_be_bytes());
    buf.push(1); // section number
    buf.extend_from_slice(&7u16.to_be_bytes()); // centre = NCEP
    buf.extend_from_slice(&0u16.to_be_bytes()); // sub-centre
    buf.push(5);
    buf.push(0);
    buf.push(1);
    buf.extend_from_slice(&2024u16.to_be_bytes());
    buf.push(1);
    buf.push(1);
    buf.push(0);
    buf.push(0);
    buf.push(0);
    buf.push(0); // production status = operational
    buf.push(1); // data type = forecast

    // §3 GDS (72 bytes) — template 3.0, 1-point lat/lon grid.
    buf.extend_from_slice(&gds_len.to_be_bytes());
    buf.push(3);
    buf.push(0); // source
    buf.extend_from_slice(&1u32.to_be_bytes()); // num data points
    buf.push(0); // optional list size
    buf.push(0); // interp
    buf.extend_from_slice(&0u16.to_be_bytes()); // template 3.0
    let mut payload = vec![0u8; 58];
    payload[0] = 6; // sphere
    payload[16..20].copy_from_slice(&1u32.to_be_bytes()); // Ni
    payload[20..24].copy_from_slice(&1u32.to_be_bytes()); // Nj
    payload[49..53].copy_from_slice(&1_000_000u32.to_be_bytes()); // Di
    payload[53..57].copy_from_slice(&1_000_000u32.to_be_bytes()); // Dj
    buf.extend_from_slice(&payload);

    buf
}

/// Append the minimal §5/§6/§7 trio that a synthesized GRIB2 message needs
/// to satisfy `Grib2Reader::from_bytes`. The DRS declares simple packing
/// (template 5.0), the BMS declares "no bitmap", and the DS carries one
/// 8-bit packed value of 0 for the 1-point grid built by
/// [`build_message_prefix`].
fn append_minimal_drs_bms_ds(buf: &mut Vec<u8>) {
    // §5 DRS — template 5.0, R=0, E=0, D=0, 8 bits/value.
    push_be(buf, 21, 4);
    buf.push(5);
    push_be(buf, 1, 4); // num data points
    push_be(buf, 0, 2); // template 5.0
    buf.extend_from_slice(&0.0_f32.to_be_bytes()); // R
    push_be(buf, 0, 2); // E
    push_be(buf, 0, 2); // D
    buf.push(8); // bits per value
    buf.push(0); // original field type
    // §6 BMS — indicator 255 = no bitmap.
    push_be(buf, 6, 4);
    buf.push(6);
    buf.push(255);
    // §7 DS — 1 byte = one 8-bit value (X = 0 → decoded value = 0).
    push_be(buf, 6, 4);
    buf.push(7);
    buf.push(0);
}

#[test]
fn template_4_8_round_trips_via_full_reader() {
    // §4 template 4.8: 6-hour total-precipitation (APCP) accumulation, end
    // of interval 2024-01-01T06:00:00Z. Length = 9-byte PDS header + 25-byte
    // horizontal core + 12-byte stats header + 12-byte single spec = 58.
    let mut pds = Vec::new();
    push_be(&mut pds, 58, 4);
    pds.push(4); // PDS section number
    push_be(&mut pds, 0, 2); // NV
    push_be(&mut pds, 8, 2); // template 4.8
    // Horizontal common: parameter 0/1/8 = APCP, forecast process type 2,
    // 0-hour forecast time at ground, missing second surface.
    pds.extend_from_slice(&[
        1, 8, 2, 0, 96, 0, 0, 0,
        1, // category=1, number=8, gp=2, bg=0, fp=96, cutoff h+m, unit=hour
    ]);
    push_be(&mut pds, 0, 4); // forecast time = 0 (start of accumulation)
    pds.extend_from_slice(&[1, 0]); // surface 1 = ground, scale 0
    push_be(&mut pds, 0, 4); // scaled value 0
    pds.extend_from_slice(&[0xFF, 0xFF]); // second surface = missing
    pds.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    // Stats header
    push_be(&mut pds, 2024, 2);
    pds.extend_from_slice(&[1, 1, 6, 0, 0, 1]); // 2024-01-01T06:00:00Z, 1 spec
    push_be(&mut pds, 0, 4); // 0 missing values
    // Spec: accumulation, unit=hour, length=6, no increment
    pds.extend_from_slice(&[1, 2, 1]);
    push_be(&mut pds, 6, 4);
    pds.push(255);
    push_be(&mut pds, 0, 4);
    assert_eq!(pds.len(), 58);

    let mut bytes = build_message_prefix(pds.len() as u32);
    bytes.extend_from_slice(&pds);
    append_minimal_drs_bms_ds(&mut bytes);
    bytes.extend_from_slice(b"7777");

    let reader = Grib2Reader::from_bytes(bytes).expect("end-to-end parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.pds.template_number, 8);
    assert!(matches!(
        msg.pds.template,
        ProductTemplate::HorizontalTimeInterval(_)
    ));
    let common = msg.pds.common().unwrap();
    assert_eq!(
        lookup_parameter(
            msg.is.discipline,
            common.parameter_category,
            common.parameter_number
        ),
        Some(("APCP", "Total precipitation", "kg m⁻²")),
    );
    let stats = msg.pds.stats().unwrap();
    assert_eq!(stats.end_time_iso8601(), "2024-01-01T06:00:00Z");
    assert_eq!(stats.specs.len(), 1);
    assert_eq!(
        lookup_statistical_process(stats.specs[0].stat_process),
        "Accumulation",
    );
    assert_eq!(stats.specs[0].stat_length, 6);
    assert_eq!(
        lookup_time_range_unit(stats.specs[0].stat_length_unit),
        "Hour",
    );
    // Confirm the surface lookup table covers the ground code seen in real
    // accumulation fields.
    assert_eq!(
        lookup_fixed_surface(common.first_surface.surface_type),
        "Ground or water surface",
    );
}

#[test]
fn template_4_11_round_trips_via_full_reader() {
    // §4 template 4.11: 6-hour mean 2-m temperature, ensemble member 5/20,
    // positively perturbed, end of interval 2024-01-01T18:00:00Z. Length =
    // 9 + 25 (common) + 3 (ensemble) + 12 (stats hdr) + 12 (spec) = 61.
    let mut pds = Vec::new();
    push_be(&mut pds, 61, 4);
    pds.push(4);
    push_be(&mut pds, 0, 2);
    push_be(&mut pds, 11, 2);
    pds.extend_from_slice(&[
        0, 0, 4, 0, 96, 0, 0, 0, 1, // 0/0 (TMP), gp=4 ensemble forecast
    ]);
    push_be(&mut pds, 0, 4); // forecast time = 0
    pds.extend_from_slice(&[103, 0]); // surface 103 = height above ground, scale 0
    push_be(&mut pds, 2, 4); // 2 m
    pds.extend_from_slice(&[0xFF, 0xFF]);
    pds.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
    // Ensemble triple
    pds.extend_from_slice(&[3, 5, 20]);
    // Stats header
    push_be(&mut pds, 2024, 2);
    pds.extend_from_slice(&[1, 1, 18, 0, 0, 1]);
    push_be(&mut pds, 0, 4);
    // Spec: average
    pds.extend_from_slice(&[0, 2, 1]);
    push_be(&mut pds, 6, 4);
    pds.push(255);
    push_be(&mut pds, 0, 4);
    assert_eq!(pds.len(), 61);

    let mut bytes = build_message_prefix(pds.len() as u32);
    bytes.extend_from_slice(&pds);
    append_minimal_drs_bms_ds(&mut bytes);
    bytes.extend_from_slice(b"7777");

    let reader = Grib2Reader::from_bytes(bytes).expect("end-to-end parse");
    let msg = &reader.messages[0];
    assert_eq!(msg.pds.template_number, 11);

    let t = match &msg.pds.template {
        ProductTemplate::EnsembleTimeInterval(t) => t,
        other => panic!("expected EnsembleTimeInterval, got {other:?}"),
    };
    assert_eq!(t.perturbation_number, 5);
    assert_eq!(t.num_forecasts_in_ensemble, 20);
    assert_eq!(t.ensemble_type, 3);

    let common = msg.pds.common().unwrap();
    assert_eq!(
        lookup_parameter(
            msg.is.discipline,
            common.parameter_category,
            common.parameter_number
        ),
        Some(("TMP", "Temperature", "K")),
    );
    assert_eq!(common.first_surface.value(), Some(2.0));
    assert_eq!(
        lookup_generating_process_type(common.generating_process_type),
        "Ensemble forecast",
    );

    let stats = msg.pds.stats().unwrap();
    assert_eq!(stats.end_time_iso8601(), "2024-01-01T18:00:00Z");
    assert_eq!(
        lookup_statistical_process(stats.specs[0].stat_process),
        "Average"
    );
}
