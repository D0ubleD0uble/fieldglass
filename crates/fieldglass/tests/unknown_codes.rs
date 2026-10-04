//! A code no table names keeps its number (#774).
//!
//! `originating_centre` (`Centre <n>`) and `parameter` (`Parameter <codes>`)
//! already kept the code when a lookup failed (#633). The sub-centre and the
//! GRIB2 identification did not: an unassigned sub-centre read as `null`, the
//! same as "no sub-centre", and an unassigned discipline, production status or
//! data type read as a placeholder every such code shared. This holds each of
//! those fields to the same contract, through [`fieldglass::Session::message`]
//! over a committed fixture with the one code byte changed.
//!
//! Each field is checked for a code its table names, one it does not (an
//! unassigned code and, where the table delegates a range to centres, a
//! local-use one), and the code the table itself defines as missing, which
//! stays `"Missing"`. The level-type and time-unit columns follow the same
//! rule in the format crates, and are tested there (`display.rs` and
//! `reader.rs`).

use fieldglass::{Identification, MessageInfo};

/// A committed GRIB2 message: ECMWF (centre 98), sub-centre 0, discipline 0,
/// production status 0, data type 255.
const GRIB2: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");

/// A committed GRIB1 message: centre 85 (Toulouse), sub-centre 105.
const GRIB1: &[u8] = include_bytes!("../../fieldglass-grib1/tests/fixtures/ecmwf_lfpw_msg0.grib1");

/// GRIB2 §0 octet 7 is the discipline.
const DISCIPLINE: usize = 6;
/// GRIB2 §1 starts after the 16-octet §0. Its octets 6-7 are the centre and
/// 8-9 the sub-centre, both big-endian, so the low byte is octet 7 and octet 9
/// (the fixture's high bytes are 0); octet 20 is the production status and 21
/// the data type.
const SECTION1: usize = 16;
const G2_CENTRE: usize = SECTION1 + 6;
const G2_SUB_CENTRE: usize = SECTION1 + 8;
const PRODUCTION_STATUS: usize = SECTION1 + 19;
const DATA_TYPE: usize = SECTION1 + 20;
/// GRIB1's PDS starts after the 8-octet §0; its octet 26 is the sub-centre.
const G1_SUB_CENTRE: usize = 8 + 25;

fn patched(bytes: &[u8], edits: &[(usize, u8)]) -> MessageInfo {
    let mut out = bytes.to_vec();
    for &(at, to) in edits {
        out[at] = to;
    }
    fieldglass::Session::open(out)
        .expect("the patched message still parses")
        .message(0)
        .expect("message 0")
}

/// The three GRIB2 identification strings, in the order the variant holds them.
fn grib2_identification(info: &MessageInfo) -> (String, String, String) {
    match &info.identification {
        Identification::Grib2 {
            discipline,
            production_status,
            data_type,
        } => (
            discipline.clone(),
            production_status.clone(),
            data_type.clone(),
        ),
        other => panic!("a GRIB2 message tagged {other:?}"),
    }
}

#[test]
fn a_discipline_the_table_does_not_name_keeps_its_code() {
    let cases = [
        (0u8, "Meteorological products"),
        // Code Table 0.0 leaves 21-190 unassigned and 192-254 to local use.
        (99, "Discipline 99"),
        (209, "Discipline 209"),
        // Code Table 0.0 defines 255 as missing.
        (255, "Missing"),
    ];
    for (code, want) in cases {
        let info = patched(GRIB2, &[(DISCIPLINE, code)]);
        assert_eq!(grib2_identification(&info).0, want, "discipline {code}");
    }
}

#[test]
fn a_production_status_the_table_does_not_name_keeps_its_code() {
    let cases = [
        (0u8, "Operational products"),
        // Code Table 1.3 leaves 18-191 unassigned and 192-254 to local use.
        (99, "Production status 99"),
        (200, "Production status 200"),
        // Code Table 1.3 defines 255 as missing.
        (255, "Missing"),
    ];
    for (code, want) in cases {
        let info = patched(GRIB2, &[(PRODUCTION_STATUS, code)]);
        assert_eq!(grib2_identification(&info).1, want, "status {code}");
    }
}

#[test]
fn a_data_type_the_table_does_not_name_keeps_its_code() {
    let cases = [
        (1u8, "Forecast products"),
        // Code Table 1.4 leaves 11-191 unassigned and 192-254 to local use.
        (99, "Data type 99"),
        (200, "Data type 200"),
        // Code Table 1.4 defines 255 as missing; the fixture states it.
        (255, "Missing"),
    ];
    for (code, want) in cases {
        let info = patched(GRIB2, &[(DATA_TYPE, code)]);
        assert_eq!(grib2_identification(&info).2, want, "data type {code}");
    }
}

/// C-12 keys the sub-centre on the pair, and defines no missing-value code
/// for it: 0 is the one value that means "none".
#[test]
fn a_grib2_sub_centre_the_table_does_not_name_keeps_its_code() {
    // Centre 7 (NCEP), whose C-12 entries include 4.
    let cases = [
        (0u8, None),
        (4, Some("Environmental Modeling Center")),
        (250, Some("Sub-centre 250")),
        (255, Some("Sub-centre 255")),
    ];
    for (code, want) in cases {
        let info = patched(GRIB2, &[(G2_CENTRE, 7), (G2_SUB_CENTRE, code)]);
        assert_eq!(info.sub_centre.as_deref(), want, "sub-centre {code}");
    }
}

#[test]
fn a_grib1_sub_centre_the_table_does_not_name_keeps_its_code() {
    // Centre 85 (Toulouse): C-12 names 200 under it and not 105, the code the
    // committed file states.
    let cases = [
        (0u8, None),
        (
            200,
            Some("Institut National de l'Environnement Industriel et des Risques (France)"),
        ),
        (105, Some("Sub-centre 105")),
        (255, Some("Sub-centre 255")),
    ];
    for (code, want) in cases {
        let info = patched(GRIB1, &[(G1_SUB_CENTRE, code)]);
        assert_eq!(info.sub_centre.as_deref(), want, "sub-centre {code}");
    }
}
