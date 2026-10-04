//! A field a message does not state is `None`, not a placeholder (#775).
//!
//! The library used to write its own display text in band: `"—"` for a level,
//! level type or forecast it had none of, `"unknown"` for a packing it could
//! not read, `""` for a missing short name or units, and `0` for the size of a
//! grid with no raster. Each read as data to a type checker, and each was the
//! library choosing a host's placeholder for it. These tests go through
//! [`fieldglass::Session::message`] over committed fixtures — one code byte
//! changed where no committed file reaches the case — and hold every such field
//! to `None`.

use fieldglass::{MessageInfo, Session};

const G1: &str = "../fieldglass-grib1/tests/fixtures/";
const G2: &str = "../fieldglass-grib2/tests/fixtures/";

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn message(bytes: Vec<u8>) -> MessageInfo {
    Session::open(bytes)
        .expect("opens")
        .message(0)
        .expect("message 0")
}

/// `regular_latlon_surface.grib2`'s §4 starts at byte 126: octets 8-9 are the
/// product template number, and octets 10-11 the parameter category and
/// number of a template with a horizontal product common.
const SECTION4: usize = 126;
const PRODUCT_TEMPLATE_LOW: usize = SECTION4 + 8;
const CATEGORY: usize = SECTION4 + 9;
const NUMBER: usize = SECTION4 + 10;

fn regular_grib2() -> Vec<u8> {
    read(&format!("{G2}regular_latlon_surface.grib2"))
}

/// A GRIB2 product template this build does not model (4.1, an individual
/// ensemble forecast) has no horizontal product common, so the message states
/// no parameter codes, no surface and no forecast time that the reader can
/// find. Every field built from them is `None`; the ones that come from other
/// sections are untouched.
#[test]
fn a_grib2_template_with_no_product_common_states_none_of_its_fields() {
    let mut bytes = regular_grib2();
    assert_eq!(
        (bytes[PRODUCT_TEMPLATE_LOW - 1], bytes[PRODUCT_TEMPLATE_LOW]),
        (0, 0),
        "fixture precondition: template 4.0"
    );
    bytes[PRODUCT_TEMPLATE_LOW] = 1;
    let info = message(bytes);
    assert_eq!(info.parameter, None);
    assert_eq!(info.abbreviation, None);
    assert_eq!(info.units, None);
    assert_eq!(info.level, None);
    assert_eq!(info.level_type, None);
    assert_eq!(info.forecast, None);
    assert_eq!(info.forecast_hours, None);
    // §5 and §3 are another section's answer, and still stated.
    assert_eq!(info.packing.as_deref(), Some("simple"));
    assert!(info.grid.as_ref().is_some_and(|g| g.ni.is_some()));
}

/// The same message with its own template states every one of them, so the
/// test above is about the template and not the fixture.
#[test]
fn the_unpatched_message_states_them_all() {
    let info = message(regular_grib2());
    for (what, field) in [
        ("parameter", &info.parameter),
        ("abbreviation", &info.abbreviation),
        ("units", &info.units),
        ("level", &info.level),
        ("levelType", &info.level_type),
        ("forecast", &info.forecast),
        ("packing", &info.packing),
    ] {
        assert!(
            field.as_deref().is_some_and(|s| !s.is_empty()),
            "{what}: {field:?}"
        );
    }
}

/// A grid with no raster shape — spectral coefficients here, in both editions
/// — has no `ni`/`nj`, where it used to report `0×0`. Its size is still stated,
/// in the file's own terms, by `sizeLabel`.
#[test]
fn a_grid_with_no_raster_has_no_dimensions() {
    for path in [
        format!("{G1}spectral_simple_t63.grib1"),
        format!("{G2}spectral_simple_t63.grib2"),
    ] {
        let info = message(read(&path));
        let grid = info.grid.as_ref().expect("the message declares a grid");
        assert_eq!((grid.ni, grid.nj), (None, None), "{path}");
        assert_eq!(info.size_label.as_deref(), Some("T63"), "{path}");
    }
}

/// A resolved parameter whose table gives no units — a dimensionless quantity,
/// written as an empty string in the generated table — has `None` units, and
/// one whose table gives no short name has `None` for that. The name is still
/// stated in both. Found by asking the tables rather than hard-coding a code,
/// so a table update cannot quietly empty the case.
#[test]
fn a_resolved_parameter_without_units_or_a_short_name_states_none() {
    let bytes = regular_grib2();
    let originator = fieldglass_grib2::Grib2Reader::from_bytes(bytes.clone())
        .expect("parses")
        .messages[0]
        .ids
        .originator();
    let find = |want: fn(&(&str, &str, &str)) -> bool| {
        (0u8..=191)
            .flat_map(|c| (0u8..=191).map(move |n| (c, n)))
            .find(|&(c, n)| {
                fieldglass_grib2::lookup_parameter(originator, 0, c, n)
                    .is_some_and(|entry| want(&entry))
            })
            .expect("discipline 0 has such an entry")
    };
    let patched = |(c, n): (u8, u8)| {
        let mut b = bytes.clone();
        b[CATEGORY] = c;
        b[NUMBER] = n;
        message(b)
    };

    let no_units = find(|(abbr, _, units)| units.is_empty() && !abbr.is_empty());
    let info = patched(no_units);
    assert_eq!(info.units, None, "{no_units:?}");
    assert!(info.abbreviation.is_some(), "{no_units:?}");
    assert!(info.parameter.is_some(), "{no_units:?}");

    let no_short_name = find(|(abbr, _, units)| abbr.is_empty() && !units.is_empty());
    let info = patched(no_short_name);
    assert_eq!(info.abbreviation, None, "{no_short_name:?}");
    assert!(info.units.is_some(), "{no_short_name:?}");
    assert!(info.parameter.is_some(), "{no_short_name:?}");
}

/// A GRIB1 fixed surface has no level value: the octets that would carry it
/// mean nothing for a cloud-base level. The level type still names it.
#[test]
fn a_grib1_surface_level_has_no_value() {
    let info = message(read(&format!("{G1}j_consecutive_latlon.grib1")));
    assert_eq!(info.level, None);
    assert_eq!(info.level_type.as_deref(), Some("Cloud base level"));
}
