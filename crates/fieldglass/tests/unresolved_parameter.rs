//! The unresolved-parameter contract (#633).
//!
//! When no table in the build resolves a message's parameter codes, every host
//! shows `Parameter <codes>` naming the codes that went unresolved. Before
//! this, the three display paths rendered the same message three different
//! ways — `"Unknown"` from GRIB1, the empty string from the umbrella's GRIB2
//! seam, and `Parameter d/c/n` from the napi binding — and nothing failed,
//! because **no committed fixture reaches the path**. Measured while taking
//! #633: all 73 messages across the 109 committed GRIB fixtures resolve.
//!
//! So the fixture is made here rather than committed: a single byte of a
//! committed fixture is changed to a code no table defines, which is both the
//! smallest possible synthetic input and a statement of exactly what makes the
//! message unresolvable. Everything else about the message still decodes, so a
//! failure here is about the parameter path and nothing else.

/// A committed GRIB2 message on a regular lat/lon grid, parameter 0/0/0
/// (Temperature).
const GRIB2: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");

/// A committed GRIB1 message, ECMWF (centre 98) local table 128, id 167
/// (2 metre temperature).
const GRIB1: &[u8] =
    include_bytes!("../../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1");

/// GRIB2 §0 octet 7 is the discipline — 0-based index 6, the byte after
/// `"GRIB"` and the two reserved octets.
const GRIB2_DISCIPLINE: usize = 6;

/// GRIB1 PDS octet 9 is `indicatorOfParameter`. §0 is 8 octets (`"GRIB"`, a
/// 3-octet length, the edition), so the PDS starts at index 8 and the id is at
/// index 16.
const GRIB1_PARAMETER_ID: usize = 16;

/// Discipline 209 is a local NSSL/MRMS assignment. The generated WMO master
/// table stops at 191 in all three dimensions, so no table in this build
/// defines it — which is what makes it unresolvable rather than merely absent.
const UNASSIGNED_DISCIPLINE: u8 = 209;

/// Id 0 is undefined in ECMWF local table 128.
const UNASSIGNED_GRIB1_ID: u8 = 0;

/// The same file with one byte changed.
fn patched(bytes: &[u8], at: usize, to: u8) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[at] = to;
    out
}

fn session(bytes: Vec<u8>) -> fieldglass::Session {
    fieldglass::Session::open(bytes).expect("the patched message still parses")
}

#[test]
fn grib2_names_the_codes_that_went_unresolved() {
    let s = session(patched(GRIB2, GRIB2_DISCIPLINE, UNASSIGNED_DISCIPLINE));
    let info = s.message(0).expect("message 0");
    assert_eq!(info.parameter, "Parameter 209/0/0");
    // The name is the only field that gains anything: there is no abbreviation
    // and no unit to state for a parameter no table defines.
    assert_eq!(info.abbreviation, "");
    assert_eq!(info.units, "");
}

#[test]
fn grib1_names_the_codes_that_went_unresolved() {
    let s = session(patched(GRIB1, GRIB1_PARAMETER_ID, UNASSIGNED_GRIB1_ID));
    let info = s.message(0).expect("message 0");
    // Centre 98, local table 128, id 0 — outermost first, so the string names
    // the table the id was looked up in as well as the id.
    assert_eq!(info.parameter, "Parameter 98/128/0");
    assert_eq!(info.abbreviation, "");
    assert_eq!(info.units, "");
}

/// `Session::decode` and `Session::message` build the parameter through
/// separate code paths — `decode` deliberately skips building a `MessageInfo`
/// so it does not pay for the `Georef`. Two paths is how they came to disagree
/// in the first place, so pin that they agree.
#[test]
fn decode_and_message_agree_on_the_unresolved_name() {
    for (bytes, at, to) in [
        (GRIB2, GRIB2_DISCIPLINE, UNASSIGNED_DISCIPLINE),
        (GRIB1, GRIB1_PARAMETER_ID, UNASSIGNED_GRIB1_ID),
    ] {
        let s = session(patched(bytes, at, to));
        let from_message = s.message(0).expect("message 0").parameter;
        let opts = fieldglass::DecodeOptions::new(fieldglass::Dtype::F64);
        let from_decode = s.decode(0, &opts).expect("decode 0");
        assert_eq!(
            from_decode.parameter, from_message,
            "the two seams disagree about an unresolved parameter"
        );
        assert!(from_decode.parameter.starts_with("Parameter "));
        assert_eq!(from_decode.units, "");
    }
}

/// The fallback must fire only when the lookup fails. A fallback that fired on
/// a resolved parameter would pass every test above while replacing every
/// name in the message table.
#[test]
fn a_resolved_parameter_is_untouched() {
    let g2 = session(GRIB2.to_vec());
    assert_eq!(g2.message(0).expect("message 0").parameter, "Temperature");

    let g1 = session(GRIB1.to_vec());
    let info = g1.message(0).expect("message 0");
    assert_eq!(info.parameter, "2 metre temperature");
    assert_eq!(info.abbreviation, "2t");
}
