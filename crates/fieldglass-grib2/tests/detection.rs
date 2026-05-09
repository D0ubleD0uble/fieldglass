//! Detection tests for the GRIB2 fixture. The GRIB2 reader itself is still
//! a stub (Phase 4 of the roadmap); this file only verifies that
//! `fieldglass-core`'s magic-byte detector recognises the fixture. Once
//! parsing lands, extend with section-level assertions.
//!
//! Fixture: `reduced_gaussian_pressure_level.grib2`, sourced from the public
//! ECMWF eccodes test data corpus
//! (`https://get.ecmwf.int/test-data/eccodes/data/`).

use fieldglass_core::{Format, detect_from_bytes};

const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

#[test]
fn fixture_has_grib_magic_with_edition_2() {
    assert_eq!(&FIXTURE[0..4], b"GRIB");
    assert_eq!(FIXTURE[7], 2);
}

#[test]
fn detects_as_grib2() {
    assert!(matches!(detect_from_bytes(FIXTURE), Format::Grib2));
}
