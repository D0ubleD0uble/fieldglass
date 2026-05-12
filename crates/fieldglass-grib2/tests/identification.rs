//! Integration coverage for §1 IDS + §2 LUS parsing on the public ECMWF
//! `reduced_gaussian_pressure_level.grib2` fixture.

use fieldglass_grib2::{
    Grib2Reader, lookup_centre, lookup_data_type, lookup_production_status,
    lookup_reference_time_significance,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

#[test]
fn fixture_ids_fields_decode() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];

    // Sourced from `xxd` of the fixture's IDS (octets 17..=37 of the file).
    assert_eq!(msg.ids.section_length, 21);
    assert_eq!(msg.ids.centre, 98);
    assert_eq!(msg.ids.sub_centre, 0);
    assert_eq!(msg.ids.master_tables_version, 5);
    assert_eq!(msg.ids.local_tables_version, 0);
    assert_eq!(msg.ids.reference_time_significance, 1);
    assert_eq!(msg.ids.year, 2008);
    assert_eq!(msg.ids.month, 2);
    assert_eq!(msg.ids.day, 6);
    assert_eq!(msg.ids.hour, 12);
    assert_eq!(msg.ids.minute, 0);
    assert_eq!(msg.ids.second, 0);
    assert_eq!(msg.ids.production_status, 0);
    assert_eq!(msg.ids.data_type, 255);
}

#[test]
fn fixture_ids_renders_iso8601_reference_time() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    assert_eq!(msg.ids.reference_time_iso8601(), "2008-02-06T12:00:00Z");
}

#[test]
fn fixture_centre_lookup_resolves_ecmwf() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    assert_eq!(
        lookup_centre(msg.ids.centre),
        Some("European Centre for Medium-Range Weather Forecasts (ECMWF)")
    );
}

#[test]
fn fixture_table_lookups_resolve() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    assert_eq!(
        lookup_reference_time_significance(msg.ids.reference_time_significance),
        "Start of forecast"
    );
    assert_eq!(
        lookup_production_status(msg.ids.production_status),
        "Operational products"
    );
    assert_eq!(lookup_data_type(msg.ids.data_type), "Missing");
}

#[test]
fn fixture_includes_local_use_section() {
    // The ECMWF reduced-Gaussian fixture has a §2 LUS following the IDS.
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];
    let (start, end) = msg.lus_range.expect("fixture has LUS");
    // §2 starts immediately after IS (16) + IDS (21) = byte offset 37.
    assert_eq!(start, 37);
    // Section header places `length` in octets 1..=4 → bytes 37..=40 of the file.
    let declared_len = u32::from_be_bytes([
        FIXTURE[start],
        FIXTURE[start + 1],
        FIXTURE[start + 2],
        FIXTURE[start + 3],
    ]);
    assert_eq!(end - start, declared_len as usize);
    // Section number byte (octet 5) must be 2.
    assert_eq!(FIXTURE[start + 4], 2);
}
