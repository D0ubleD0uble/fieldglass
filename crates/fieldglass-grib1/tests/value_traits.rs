//! The value traits this crate's public types promise, pinned by use (#556).
//!
//! `missing_debug_implementations` in `[workspace.lints]` holds `Debug` across
//! the workspace; nothing holds `Clone` or `PartialEq`, so a dropped derive is
//! caught here or not at all. The bounds are the assertion — a removed derive
//! fails to compile — and the cases beneath them run against a real message,
//! because a bound proves a trait exists and says nothing about it being right.
//!
//! The fixture is a regular lat/lon message; provenance in
//! `tests/fixtures/NOTICE.md`. Every GRIB1 fixture in this crate holds exactly
//! one message, so the cases that need two comparable values make the second
//! by mutating a clone rather than by reading another message.

use fieldglass_grib1::gds::LatLonGrid;
use fieldglass_grib1::{
    BdsHeader, Bitmap, Grib1Message, Grib1Reader, GridDescription, IndicatorSection, MatrixField,
    ProductDefinition,
};

const FIXTURE: &[u8] = include_bytes!("fixtures/j_consecutive_latlon.grib1");

fn assert_copy<T: Copy>() {}
fn assert_clone_eq<T: Clone + PartialEq>() {}

#[test]
fn the_fixed_size_sections_are_plain_values() {
    // Every field an integer or a float, and no owned buffer, so they copy.
    assert_copy::<IndicatorSection>();
    assert_copy::<ProductDefinition>();
    assert_copy::<BdsHeader>();
    assert_copy::<LatLonGrid>();
}

#[test]
fn the_sections_that_own_a_buffer_clone_rather_than_copy() {
    // A reduced grid carries its row lengths and a bitmap its bits, so `Copy`
    // is not available to them; `GridDescription` inherits that from the
    // reduced variants it wraps.
    assert_clone_eq::<Bitmap>();
    assert_clone_eq::<GridDescription>();
    assert_clone_eq::<MatrixField>();
    assert_clone_eq::<Grib1Message>();
}

#[test]
fn a_cloned_message_equals_the_one_it_came_from() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("the fixture opens");
    let first = reader.messages.first().expect("at least one message");

    // What the derive is for: comparing a message read now against one read
    // earlier, without hand-writing a field-by-field check that goes stale
    // whenever a section gains a field.
    assert_eq!(
        first.clone(),
        *first,
        "a clone compares equal to its source"
    );

    // And that equality actually discriminates, rather than everything
    // comparing equal — the failure a reflexive check alone cannot see. One
    // changed field is enough, and it has to be a derived comparison for this
    // to hold: every GRIB1 fixture here is a single message, so the second
    // value is made rather than read.
    let mut other = first.clone();
    other.message_index += 1;
    assert_ne!(&other, first, "a changed field is a different message");
}

#[test]
fn a_grid_compares_by_its_geometry() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("the fixture opens");
    let gds = reader.messages[0]
        .gds
        .as_ref()
        .expect("the fixture declares a §2");

    let same = gds.clone();
    assert_eq!(&same, gds);

    // A single changed number has to break equality; a derive that compared
    // nothing would pass the clone check above and fail this one. Matched with
    // an `else` that panics rather than an `if let` that skips — the first
    // draft of this test used `if let` against a polar-stereographic fixture
    // and passed without ever running the assertion.
    let GridDescription::LatLon(grid) = gds else {
        panic!("the fixture is a regular lat/lon grid; got {gds:?}");
    };
    let mut nudged = *grid;
    nudged.ni += 1;
    assert_ne!(
        &nudged, grid,
        "a different column count is a different grid"
    );
}
